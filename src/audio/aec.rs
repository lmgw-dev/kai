//! App AEC (§6.2 `app`, K20): Kai hosts `libpipewire-module-echo-cancel` in a `pipewire -c` child,
//! pinned to the real devices, and routes the system defaults through it while the mode is on.
//!
//! App AEC with K20's rules:
//! - **The real defaults are saved first.** `$XDG_STATE_HOME/kai/aec.json` holds the configured
//!   defaults (WirePlumber's `default.configured.audio.*`, absent when the owner never chose one)
//!   and the effective ones, written and synced before anything is flipped.
//! - **Restored on exit, on a switch away, and on the next start** after Kai died without
//!   restoring. A key is put back only while it still names Kai's node: a default the owner chose
//!   meanwhile stays. The file goes only once PipeWire confirmed the restore. While Kai runs, an
//!   unconfirmed restore is sent again every `aec.restore_timeout_s`, with the canceller still up,
//!   so the defaults never name a node that is gone; at quit it is given up after one round, and
//!   the next start restores.
//! - **Kai's own node is never saved as a real default**: a configured default that names it (a
//!   leftover whose `aec.json` was unreadable) is saved as unset, and an unreadable `aec.json` is
//!   set aside as `aec.json.bad`, never overwritten.
//! - **The child dies with Kai** (`PR_SET_PDEATHSIG`), so a crash leaves no canceller behind.
//! - **The wait for the module's nodes** is `aec.ready_timeout_s`, named in its error.
//!
//! *Design choices:*
//! - Kai's nodes are `kai_aec_sink`/`kai_aec_source`, distinct from the common
//!   `aec_sink`/`aec_source` names that another echo-cancel setup may already provide. There is no
//!   reuse path: an existing pair is somebody else's.
//! - Defaults are set in PipeWire's `default` metadata (what `wpctl set-default` does), not with
//!   `pactl`, so pipewire-pulse is not needed.
//! - Kai moves no stream itself: WirePlumber moves streams that follow the default
//!   (`linking.follow-default-target`, on by default), and a stream the owner routed elsewhere keeps
//!   its route, so nothing per stream needs restoring.
//!
//! [`AecDriver`] is the logic without I/O: it returns [`AecAction`]s, which the PipeWire thread
//! carries out, and is told what happened.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::echo::AecPhase;

pub const SINK: &str = "kai_aec_sink";
pub const SOURCE: &str = "kai_aec_source";
const CAPTURE: &str = "kai_aec_capture";
const PLAYBACK: &str = "kai_aec_playback";

/// Kai's own nodes (the module's four), never offered as devices.
pub fn is_own_node(name: &str) -> bool {
    [SINK, SOURCE, CAPTURE, PLAYBACK].contains(&name)
}

/// A default key in PipeWire's `default` metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultKey {
    Sink,
    Source,
}

impl DefaultKey {
    /// The configured key (persisted by WirePlumber, what the owner chose).
    pub fn configured(self) -> &'static str {
        match self {
            DefaultKey::Sink => "default.configured.audio.sink",
            DefaultKey::Source => "default.configured.audio.source",
        }
    }

    fn ours(self) -> &'static str {
        match self {
            DefaultKey::Sink => SINK,
            DefaultKey::Source => SOURCE,
        }
    }
}

/// What app AEC needs to know about the graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphView {
    pub configured_sink: Option<String>,
    pub configured_source: Option<String>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub has_sink: bool,
    pub has_source: bool,
}

impl GraphView {
    fn configured(&self, key: DefaultKey) -> Option<&str> {
        match key {
            DefaultKey::Sink => self.configured_sink.as_deref(),
            DefaultKey::Source => self.configured_source.as_deref(),
        }
    }
}

/// `aec.json`: the defaults as they were before Kai flipped them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDefaults {
    pub version: u32,
    /// `default.configured.audio.sink` before the flip; `None`: the key was absent.
    pub configured_sink: Option<String>,
    pub configured_source: Option<String>,
    /// The effective defaults the module was pinned to (for the log and the owner).
    pub default_sink: String,
    pub default_source: String,
}

impl SavedDefaults {
    fn saved(&self, key: DefaultKey) -> Option<String> {
        match key {
            DefaultKey::Sink => self.configured_sink.clone(),
            DefaultKey::Source => self.configured_source.clone(),
        }
    }
}

/// Writes `aec.json` atomically and syncs it, so a crash right after the flip still finds it.
pub fn save(path: &Path, saved: &SavedDefaults) -> Result<(), String> {
    use std::io::Write;
    let fail = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(fail)?;
    let tmp = dir.join(".aec.json.tmp");
    let mut f = std::fs::File::create(&tmp).map_err(fail)?;
    let text = serde_json::to_string_pretty(saved).map_err(|e| e.to_string())?;
    f.write_all(text.as_bytes()).map_err(fail)?;
    f.sync_all().map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Reads `aec.json`; `None` when there is none (the last run restored, or never flipped).
pub fn load(path: &Path) -> Result<Option<SavedDefaults>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

/// The writes that put the defaults back: a key is restored only while it still names Kai's
/// node. `None` as the value removes the key (it was absent before).
pub fn restore_plan(saved: &SavedDefaults, graph: &GraphView) -> Vec<(DefaultKey, Option<String>)> {
    [DefaultKey::Sink, DefaultKey::Source]
        .into_iter()
        .filter(|k| graph.configured(*k) == Some(k.ours()))
        .map(|k| (k, saved.saved(k)))
        .collect()
}

/// Where `libspa-aec-webrtc` must be: `SPA_PLUGIN_DIR`, else the usual library directories.
pub fn plugin_check() -> Result<(), String> {
    let dirs: Vec<PathBuf> = match std::env::var_os("SPA_PLUGIN_DIR") {
        Some(v) => std::env::split_paths(&v).collect(),
        None => [
            "/usr/lib64/spa-0.2",
            "/usr/lib/spa-0.2",
            "/usr/lib/x86_64-linux-gnu/spa-0.2",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
    };
    let found = dirs
        .iter()
        .any(|d| d.join("aec").join("libspa-aec-webrtc.so").exists());
    if found {
        Ok(())
    } else {
        Err(format!(
            "PipeWire's WebRTC echo canceller (aec/libspa-aec-webrtc.so) is not installed under {}",
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// The child's configuration: the client modules of PipeWire's `filter-chain.conf` plus the
/// echo-cancel module, AEC only (no AGC, noise suppression, high-pass or VAD, which clip soft
/// onsets), pinned to the real devices.
pub fn conf_text(capture_target: &str, playback_target: &str) -> String {
    format!(
        r#"# Written by Kai (app echo cancellation); recreated on every start.
context.properties = {{
    log.level = 0
}}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
    {{ name = libpipewire-module-rt flags = [ ifexists nofail ] }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-echo-cancel
        args = {{
            library.name = aec/libspa-aec-webrtc
            aec.args = {{
                webrtc.gain_control      = false
                webrtc.noise_suppression = false
                webrtc.high_pass_filter  = false
                webrtc.voice_detection   = false
                webrtc.extended_filter   = true
                webrtc.delay_agnostic    = true
            }}
            capture.props = {{
                node.name     = "{CAPTURE}"
                target.object = {capture}
                node.dont-fallback = true
            }}
            source.props = {{
                node.name        = "{SOURCE}"
                node.description = "Kai: echo-cancelled microphone"
                media.class      = "Audio/Source"
            }}
            sink.props = {{
                node.name        = "{SINK}"
                node.description = "Kai: echo reference (all sound plays through here)"
                media.class      = "Audio/Sink"
            }}
            playback.props = {{
                node.name     = "{PLAYBACK}"
                target.object = {playback}
                node.dont-fallback = true
            }}
        }}
    }}
]
"#,
        capture = spa_json_string(capture_target),
        playback = spa_json_string(playback_target),
    )
}

/// A quoted SPA-JSON string (node names are free text).
fn spa_json_string(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// Starts `pipewire -c <conf>`. The child gets `PR_SET_PDEATHSIG` = SIGKILL: when the thread that
/// spawned it ends (Kai's PipeWire thread, which lives as long as Kai), the kernel kills it.
pub fn spawn_child(conf: &Path) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    let mut cmd = std::process::Command::new("pipewire");
    cmd.arg("-c")
        .arg(conf)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as libc::c_ulong,
                0,
                0,
                0,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            // Kai died between the fork and the prctl: do not outlive it.
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    cmd.spawn()
}

/// What the PipeWire thread does for the driver, in order. When one fails, it stops there and
/// calls [`AecDriver::failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AecAction {
    /// Write `aec.json`.
    Save(SavedDefaults),
    /// Delete `aec.json`.
    Remove,
    /// Rename an unreadable `aec.json` to `aec.json.bad`, so nothing overwrites it.
    SetAside,
    /// Write the configuration and start the child.
    Spawn { conf: String },
    /// Kill the child and reap it.
    Kill,
    /// Set (or, with `None`, remove) a configured default.
    SetDefault {
        key: DefaultKey,
        value: Option<String>,
    },
    /// A PipeWire roundtrip; answer with [`AecDriver::confirmed`] when it comes back.
    Sync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Off,
    Starting {
        deadline: Instant,
    },
    Active,
    /// Restoring at a stop. The child runs on until PipeWire confirms the restore: on a switch
    /// away that may take several rounds; at quit the restore is given up at its deadline.
    Stopping,
    /// Restoring after the child went away while active.
    Recovering {
        reason: String,
    },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// The start-up recovery's restore.
    Recovery,
    /// A stop's restore.
    Stop,
    /// A restore after the child went away.
    Lost,
}

/// A restore round: its writes and roundtrip went out (or failed), and PipeWire's confirmation is
/// awaited until `deadline`. Past it, the round is sent again while Kai runs; at quit it is given
/// up, and the next start restores from `aec.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Restore {
    kind: Pending,
    deadline: Instant,
    /// Rounds sent so far (the first unconfirmed one is said once).
    rounds: u32,
}

/// What the module is pinned to: Kai's device picks, else the effective defaults.
#[derive(Debug, Clone, Default)]
pub struct Picks {
    pub input: Option<String>,
    pub output: Option<String>,
}

pub struct AecDriver {
    phase: Phase,
    saved: Option<SavedDefaults>,
    restore: Option<Restore>,
    /// Kai quits: a restore is not retried, only waited for until its deadline.
    quitting: bool,
    /// The capture and playback targets the module was started with (K-9).
    pinned: Option<(String, String)>,
    ready_timeout: Duration,
    restore_timeout: Duration,
    /// A line for the log and the tooltip about the last recovery or restore problem.
    pub note: Option<String>,
}

impl AecDriver {
    pub fn new(ready_timeout: Duration, restore_timeout: Duration) -> Self {
        Self {
            phase: Phase::Off,
            saved: None,
            restore: None,
            quitting: false,
            pinned: None,
            ready_timeout,
            restore_timeout,
            note: None,
        }
    }

    pub fn phase(&self) -> AecPhase {
        match &self.phase {
            Phase::Off | Phase::Stopping => AecPhase::Off,
            Phase::Starting { .. } => AecPhase::Starting,
            Phase::Active => AecPhase::Active,
            Phase::Recovering { reason } | Phase::Failed(reason) => {
                AecPhase::Failed(reason.clone())
            }
        }
    }

    /// True while the defaults run through Kai's nodes (Kai's own streams then target them).
    pub fn routes(&self) -> bool {
        self.phase == Phase::Active
    }

    /// True while there is something to restore or confirm before Kai may exit.
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Starting { .. } | Phase::Active | Phase::Stopping
        ) || self.restore.is_some()
    }

    /// The next time [`AecDriver::tick`] must run.
    pub fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Starting { deadline } => Some(deadline),
            _ => self.restore.map(|r| r.deadline),
        }
    }

    /// A restore round of `kind`: the writes for every key that still names Kai's node, then a
    /// roundtrip. It is confirmed by [`AecDriver::confirmed`], or retried at its deadline.
    fn round(&mut self, kind: Pending, graph: &GraphView, now: Instant) -> Vec<AecAction> {
        let plan = self
            .saved
            .as_ref()
            .map(|s| restore_plan(s, graph))
            .unwrap_or_default();
        let rounds = self
            .restore
            .filter(|r| r.kind == kind)
            .map_or(0, |r| r.rounds)
            + 1;
        self.restore = Some(Restore {
            kind,
            deadline: now + self.restore_timeout,
            rounds,
        });
        let mut actions = set_actions(plan);
        actions.push(AecAction::Sync);
        actions
    }

    /// The restore of `kind` is done: confirmed, or nothing names Kai's node any more.
    fn finish(&mut self, kind: Pending) -> Vec<AecAction> {
        self.restore = None;
        self.saved = None;
        match kind {
            Pending::Recovery => vec![AecAction::Remove],
            Pending::Stop => {
                self.phase = Phase::Off;
                self.pinned = None;
                vec![AecAction::Kill, AecAction::Remove]
            }
            Pending::Lost => {
                if let Phase::Recovering { reason } = &self.phase {
                    self.phase = Phase::Failed(reason.clone());
                }
                vec![AecAction::Remove]
            }
        }
    }

    /// At quit, an unconfirmed restore is given up: `aec.json` stays for the next start.
    fn give_up(&mut self, why: String) -> Vec<AecAction> {
        let Some(r) = self.restore.take() else {
            return vec![];
        };
        self.note = Some(format!(
            "{why}; the next start restores the default devices from aec.json"
        ));
        self.saved = None;
        match r.kind {
            Pending::Stop => {
                self.phase = Phase::Off;
                self.pinned = None;
                vec![AecAction::Kill]
            }
            Pending::Lost => {
                if let Phase::Recovering { reason } = &self.phase {
                    self.phase = Phase::Failed(reason.clone());
                }
                vec![]
            }
            Pending::Recovery => vec![],
        }
    }

    /// At the first complete view of the graph: a leftover `aec.json` means a run died with the
    /// defaults flipped. Put them back. An unreadable one is set aside, never overwritten.
    pub fn recover(
        &mut self,
        file: Result<Option<SavedDefaults>, String>,
        graph: &GraphView,
        now: Instant,
    ) -> Vec<AecAction> {
        let saved = match file {
            Ok(Some(saved)) => saved,
            Ok(None) => return vec![],
            Err(e) => {
                self.note = Some(format!(
                    "cannot read the saved default devices ({e}); the file is set aside as \
                     aec.json.bad, and a default that still names Kai's canceller is treated as \
                     unset"
                ));
                return vec![AecAction::SetAside];
            }
        };
        let plan = restore_plan(&saved, graph);
        if plan.is_empty() {
            self.note = Some(
                "aec.json from a run that did not end cleanly: the defaults no longer name Kai's \
                 canceller, nothing to restore"
                    .into(),
            );
            return vec![AecAction::Remove];
        }
        self.note = Some(format!(
            "restored the default devices a run that did not end cleanly had switched to Kai's \
             canceller ({})",
            describe(&plan)
        ));
        self.saved = Some(saved);
        self.round(Pending::Recovery, graph, now)
    }

    /// The mode became `app` (or Kai started in it). `plugin` is [`plugin_check`]'s answer.
    pub fn start(
        &mut self,
        graph: &GraphView,
        picks: &Picks,
        plugin: Result<(), String>,
        now: Instant,
    ) -> Vec<AecAction> {
        if !matches!(self.phase, Phase::Off | Phase::Failed(_)) || self.restore.is_some() {
            return vec![];
        }
        if let Err(e) = plugin {
            self.phase = Phase::Failed(e);
            return vec![];
        }
        if graph.has_sink || graph.has_source {
            self.phase = Phase::Failed(format!(
                "nodes named {SINK}/{SOURCE} exist already (another program hosts them)"
            ));
            return vec![];
        }
        let (Some(default_sink), Some(default_source)) =
            (graph.default_sink.clone(), graph.default_source.clone())
        else {
            self.phase = Phase::Failed(
                "PipeWire names no default output and input (is WirePlumber running?)".into(),
            );
            return vec![];
        };
        let capture = picks.input.clone().unwrap_or(default_source.clone());
        let playback = picks.output.clone().unwrap_or(default_sink.clone());
        if is_own_node(&capture) || is_own_node(&playback) {
            self.phase = Phase::Failed(format!(
                "the devices to cancel echo on are Kai's own canceller ({capture}, {playback}), \
                 which is gone; choose a microphone and speaker, or set the default devices"
            ));
            return vec![];
        }
        // A configured default that still names Kai's node is a leftover of a run whose restore
        // never happened (its aec.json unreadable): saved as unset, never as the dead node, so a
        // restore cannot write it back.
        let mut leftover = Vec::new();
        let mut real = |key: DefaultKey, value: Option<String>| match value {
            Some(v) if is_own_node(&v) => {
                leftover.push(format!("{} = {v}", key.configured()));
                None
            }
            other => other,
        };
        let saved = SavedDefaults {
            version: 1,
            configured_sink: real(DefaultKey::Sink, graph.configured_sink.clone()),
            configured_source: real(DefaultKey::Source, graph.configured_source.clone()),
            default_sink,
            default_source,
        };
        if !leftover.is_empty() {
            self.note = Some(format!(
                "{} named Kai's canceller from a run that did not restore it; saved as unset, so \
                 the stop removes it",
                leftover.join(" and ")
            ));
        }
        self.saved = Some(saved.clone());
        self.pinned = Some((capture.clone(), playback.clone()));
        self.phase = Phase::Starting {
            deadline: now + self.ready_timeout,
        };
        vec![
            AecAction::Save(saved),
            AecAction::Spawn {
                conf: conf_text(&capture, &playback),
            },
        ]
    }

    /// K-9: the canceller runs (or starts) pinned to other devices than `picks` now name: it must
    /// restart to apply them.
    pub fn repin_needed(&self, picks: &Picks) -> bool {
        let (Some((capture, playback)), Some(saved)) = (&self.pinned, &self.saved) else {
            return false;
        };
        if !matches!(self.phase, Phase::Starting { .. } | Phase::Active) {
            return false;
        }
        let want_capture = picks.input.as_ref().unwrap_or(&saved.default_source);
        let want_playback = picks.output.as_ref().unwrap_or(&saved.default_sink);
        want_capture != capture || want_playback != playback
    }

    /// The graph changed: Kai's nodes may have come up.
    pub fn on_graph(&mut self, graph: &GraphView, now: Instant) -> Vec<AecAction> {
        match self.phase {
            Phase::Starting { .. } if graph.has_sink && graph.has_source => {
                self.phase = Phase::Active;
                vec![
                    AecAction::SetDefault {
                        key: DefaultKey::Sink,
                        value: Some(SINK.into()),
                    },
                    AecAction::SetDefault {
                        key: DefaultKey::Source,
                        value: Some(SOURCE.into()),
                    },
                ]
            }
            Phase::Active if !graph.has_sink || !graph.has_source => {
                self.lost("Kai's echo-cancel nodes went away".into(), graph, now)
            }
            _ => vec![],
        }
    }

    /// The child exited by itself.
    pub fn child_exited(
        &mut self,
        status: &str,
        graph: &GraphView,
        now: Instant,
    ) -> Vec<AecAction> {
        match self.phase {
            Phase::Starting { .. } => {
                self.phase = Phase::Failed(format!(
                    "the echo-cancel process exited before its nodes appeared ({status})"
                ));
                self.saved = None;
                self.pinned = None;
                vec![AecAction::Remove]
            }
            Phase::Active => self.lost(
                format!("the echo-cancel process exited ({status})"),
                graph,
                now,
            ),
            _ => vec![],
        }
    }

    fn lost(&mut self, reason: String, graph: &GraphView, now: Instant) -> Vec<AecAction> {
        self.phase = Phase::Recovering { reason };
        self.pinned = None;
        let mut actions = vec![AecAction::Kill];
        actions.extend(self.round(Pending::Lost, graph, now));
        actions
    }

    pub fn tick(&mut self, now: Instant, graph: &GraphView) -> Vec<AecAction> {
        if let Phase::Starting { deadline } = self.phase
            && now >= deadline
        {
            self.phase = Phase::Failed(format!(
                "the echo-cancel nodes did not appear within {} s (aec.ready_timeout_s)",
                self.ready_timeout.as_secs()
            ));
            self.saved = None;
            self.pinned = None;
            return vec![AecAction::Kill, AecAction::Remove];
        }
        let Some(r) = self.restore.filter(|r| now >= r.deadline) else {
            return vec![];
        };
        let late = format!(
            "PipeWire did not confirm the restored default devices within {} s \
             (aec.restore_timeout_s)",
            self.restore_timeout.as_secs()
        );
        if self.quitting {
            return self.give_up(late);
        }
        // While Kai runs the restore is sent again; the canceller stays up meanwhile, so the
        // defaults never name a node that is gone.
        let plan = self
            .saved
            .as_ref()
            .map(|s| restore_plan(s, graph))
            .unwrap_or_default();
        if plan.is_empty() {
            return self.finish(r.kind);
        }
        if r.rounds == 1 {
            self.note = Some(format!(
                "{late}; Kai keeps trying every {} s",
                self.restore_timeout.as_secs()
            ));
        }
        self.round(r.kind, graph, now)
    }

    /// The mode left `app` (`quit` false), or Kai quits: restore, confirm, then stop the child and
    /// forget. On a switch away the child runs until the restore is confirmed.
    pub fn stop(&mut self, graph: &GraphView, now: Instant, quit: bool) -> Vec<AecAction> {
        self.quitting |= quit;
        match self.phase {
            Phase::Starting { .. } => {
                self.phase = Phase::Off;
                self.saved = None;
                self.pinned = None;
                vec![AecAction::Kill, AecAction::Remove]
            }
            Phase::Active => {
                self.phase = Phase::Stopping;
                self.round(Pending::Stop, graph, now)
            }
            Phase::Failed(_) if self.restore.is_none() => {
                self.phase = Phase::Off;
                vec![]
            }
            _ => vec![],
        }
    }

    /// The roundtrip after the restore writes came back: PipeWire has them.
    pub fn confirmed(&mut self) -> Vec<AecAction> {
        match self.restore {
            Some(r) => self.finish(r.kind),
            None => vec![],
        }
    }

    /// An action failed; the rest of its batch was not carried out.
    pub fn failed(
        &mut self,
        action: &AecAction,
        error: String,
        graph: &GraphView,
        now: Instant,
    ) -> Vec<AecAction> {
        match action {
            AecAction::Save(_) | AecAction::Spawn { .. } => {
                self.phase = Phase::Failed(error);
                self.saved = None;
                self.pinned = None;
                vec![AecAction::Kill, AecAction::Remove]
            }
            AecAction::SetDefault { .. } | AecAction::Sync => match self.restore {
                Some(_) if self.quitting => self.give_up(error),
                Some(r) => {
                    // The round's deadline sends it again.
                    if r.rounds == 1 {
                        self.note = Some(format!(
                            "{error}; Kai tries to restore the default devices again every {} s \
                             (aec.restore_timeout_s)",
                            self.restore_timeout.as_secs()
                        ));
                    }
                    vec![]
                }
                // The flip itself failed: the canceller goes, and what was flipped is restored.
                None => {
                    self.note = Some(error.clone());
                    self.lost(error, graph, now)
                }
            },
            AecAction::Remove | AecAction::SetAside | AecAction::Kill => {
                self.note = Some(error);
                vec![]
            }
        }
    }
}

fn set_actions(plan: Vec<(DefaultKey, Option<String>)>) -> Vec<AecAction> {
    plan.into_iter()
        .map(|(key, value)| AecAction::SetDefault { key, value })
        .collect()
}

fn describe(plan: &[(DefaultKey, Option<String>)]) -> String {
    plan.iter()
        .map(|(k, v)| match v {
            Some(v) => format!("{} = {v}", k.configured()),
            None => format!("{} removed", k.configured()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn driver() -> AecDriver {
        AecDriver::new(4 * S, 4 * S)
    }

    fn real() -> GraphView {
        GraphView {
            configured_sink: Some("spk".into()),
            configured_source: None,
            default_sink: Some("spk".into()),
            default_source: Some("mic".into()),
            has_sink: false,
            has_source: false,
        }
    }

    fn up(mut g: GraphView) -> GraphView {
        g.has_sink = true;
        g.has_source = true;
        g
    }

    fn flipped(mut g: GraphView) -> GraphView {
        g.configured_sink = Some(SINK.into());
        g.configured_source = Some(SOURCE.into());
        g
    }

    fn saved() -> SavedDefaults {
        SavedDefaults {
            version: 1,
            configured_sink: Some("spk".into()),
            configured_source: None,
            default_sink: "spk".into(),
            default_source: "mic".into(),
        }
    }

    /// Start until active; returns the driver and the graph as PipeWire has it then.
    fn active(t0: Instant) -> (AecDriver, GraphView) {
        let mut d = driver();
        let a = d.start(&real(), &Picks::default(), Ok(()), t0);
        assert!(matches!(&a[..], [AecAction::Save(s), AecAction::Spawn { .. }] if *s == saved()));
        assert_eq!(d.phase(), AecPhase::Starting);
        assert_eq!(d.deadline(), Some(t0 + 4 * S));
        let a = d.on_graph(&up(real()), t0);
        assert_eq!(
            a,
            vec![
                AecAction::SetDefault {
                    key: DefaultKey::Sink,
                    value: Some(SINK.into())
                },
                AecAction::SetDefault {
                    key: DefaultKey::Source,
                    value: Some(SOURCE.into())
                },
            ]
        );
        assert!(d.routes());
        (d, flipped(up(real())))
    }

    #[test]
    fn the_real_defaults_are_saved_before_the_flip_and_the_module_pinned_to_them() {
        let mut d = driver();
        let a = d.start(&real(), &Picks::default(), Ok(()), Instant::now());
        let AecAction::Spawn { conf } = &a[1] else {
            panic!("{a:?}")
        };
        assert!(conf.contains(r#"target.object = "mic""#), "{conf}");
        assert!(conf.contains(r#"target.object = "spk""#), "{conf}");
        assert!(conf.contains(r#"node.name        = "kai_aec_sink""#));
        // Kai's picks win over the defaults.
        let mut d = driver();
        let picks = Picks {
            input: Some("usb mic".into()),
            output: Some("hdmi".into()),
        };
        let a = d.start(&real(), &picks, Ok(()), Instant::now());
        let AecAction::Spawn { conf } = &a[1] else {
            panic!()
        };
        assert!(conf.contains(r#"target.object = "usb mic""#) && conf.contains(r#""hdmi""#));
        // Nothing flips before the nodes are there.
        assert_eq!(d.on_graph(&real(), Instant::now()), vec![]);
    }

    #[test]
    fn a_clean_stop_restores_confirms_then_kills_and_forgets() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        let a = d.stop(&g, t0 + S, true);
        assert_eq!(
            a,
            vec![
                AecAction::SetDefault {
                    key: DefaultKey::Sink,
                    value: Some("spk".into())
                },
                // The source key was absent before: removed, not pinned to the effective default.
                AecAction::SetDefault {
                    key: DefaultKey::Source,
                    value: None
                },
                AecAction::Sync,
            ]
        );
        assert!(d.busy());
        assert_eq!(d.confirmed(), vec![AecAction::Kill, AecAction::Remove]);
        assert!(!d.busy());
        assert_eq!(d.phase(), AecPhase::Off);
    }

    #[test]
    fn a_default_the_owner_changed_meanwhile_is_left_alone() {
        let t0 = Instant::now();
        let (mut d, mut g) = active(t0);
        g.configured_sink = Some("headset".into());
        let a = d.stop(&g, t0, false);
        assert_eq!(
            a,
            vec![
                AecAction::SetDefault {
                    key: DefaultKey::Source,
                    value: None
                },
                AecAction::Sync
            ]
        );
    }

    #[test]
    fn at_quit_an_unconfirmed_restore_keeps_aec_json_for_the_next_start() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        d.stop(&g, t0, true);
        assert_eq!(d.tick(t0 + 3 * S, &g), vec![]);
        assert_eq!(d.tick(t0 + 4 * S, &g), vec![AecAction::Kill], "no Remove");
        assert!(d.note.as_ref().unwrap().contains("aec.restore_timeout_s"));
        assert!(d.note.as_ref().unwrap().contains("next start"));
        assert!(!d.busy(), "the quit goes on");
    }

    fn restore_writes(a: &[AecAction]) -> usize {
        a.iter()
            .filter(|a| matches!(a, AecAction::SetDefault { .. }))
            .count()
    }

    #[test]
    fn on_a_switch_away_the_canceller_stays_until_the_restore_is_confirmed() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        let a = d.stop(&g, t0, false);
        assert_eq!(restore_writes(&a), 2);
        // No confirmation within aec.restore_timeout_s: sent again, the child kept, said once.
        let a = d.tick(t0 + 4 * S, &g);
        assert!(!a.contains(&AecAction::Kill), "{a:?}");
        assert_eq!(restore_writes(&a), 2);
        assert_eq!(a.last(), Some(&AecAction::Sync));
        let note = d.note.take().unwrap();
        assert!(
            note.contains("aec.restore_timeout_s") && note.contains("keeps trying"),
            "{note}"
        );
        let a = d.tick(t0 + 8 * S, &g);
        assert_eq!(restore_writes(&a), 2);
        assert_eq!(d.note, None, "said once");
        assert!(d.busy());
        assert_eq!(d.deadline(), Some(t0 + 12 * S));
        // Confirmed at last: only now the child goes, and the file.
        assert_eq!(d.confirmed(), vec![AecAction::Kill, AecAction::Remove]);
        assert!(!d.busy());
        assert_eq!(d.phase(), AecPhase::Off);
    }

    #[test]
    fn a_failed_restore_write_on_a_switch_away_is_retried_not_given_up() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        let a = d.stop(&g, t0, false);
        let a = d.failed(&a[0], "no default metadata".into(), &g, t0);
        assert_eq!(a, vec![], "the child is not killed");
        assert!(d.note.take().unwrap().contains("tries to restore"));
        assert!(d.busy());
        let a = d.tick(t0 + 4 * S, &g);
        assert_eq!(restore_writes(&a), 2, "sent again at the deadline");
        // Meanwhile the owner chose defaults: nothing names Kai any more, so it is done.
        let mut owner = g.clone();
        owner.configured_sink = Some("headset".into());
        owner.configured_source = Some("mic".into());
        assert_eq!(
            d.tick(t0 + 8 * S, &owner),
            vec![AecAction::Kill, AecAction::Remove]
        );
        assert!(!d.busy());
    }

    #[test]
    fn a_default_naming_kais_node_is_never_saved_as_the_real_one() {
        // A crashed run left the configured defaults on Kai's nodes, and its aec.json was
        // unreadable: it is set aside, never overwritten.
        let mut d = driver();
        let mut g = flipped(real());
        assert_eq!(
            d.recover(Err("aec.json: EOF".into()), &g, Instant::now()),
            vec![AecAction::SetAside]
        );
        assert!(d.note.take().unwrap().contains("aec.json.bad"));
        // The next start saves those keys as unset...
        g.default_sink = Some("spk".into());
        let a = d.start(&g, &Picks::default(), Ok(()), Instant::now());
        let AecAction::Save(saved) = &a[0] else {
            panic!("{a:?}")
        };
        assert_eq!(saved.configured_sink, None);
        assert_eq!(saved.configured_source, None);
        let note = d.note.take().unwrap();
        assert!(
            note.contains("default.configured.audio.sink = kai_aec_sink"),
            "{note}"
        );
        // ...so the stop removes them instead of writing the dead node back.
        d.on_graph(&up(g.clone()), Instant::now());
        let a = d.stop(&flipped(up(g)), Instant::now(), false);
        assert_eq!(
            &a[..2],
            [
                AecAction::SetDefault {
                    key: DefaultKey::Sink,
                    value: None
                },
                AecAction::SetDefault {
                    key: DefaultKey::Source,
                    value: None
                }
            ]
        );
        // Pinned to Kai's own node (an effective default or a pick naming it): refused.
        let mut d = driver();
        let picks = Picks {
            input: Some(SOURCE.into()),
            output: None,
        };
        assert_eq!(d.start(&real(), &picks, Ok(()), Instant::now()), vec![]);
        assert!(matches!(d.phase(), AecPhase::Failed(r) if r.contains("Kai's own canceller")));
    }

    #[test]
    fn a_new_pick_while_active_needs_a_restart_onto_it() {
        let t0 = Instant::now();
        let (d, _) = active(t0);
        assert!(!d.repin_needed(&Picks::default()), "still on the defaults");
        let usb = Picks {
            input: Some("usb mic".into()),
            output: None,
        };
        assert!(d.repin_needed(&usb));
        let same = Picks {
            input: Some("mic".into()),
            output: Some("spk".into()),
        };
        assert!(
            !d.repin_needed(&same),
            "the picks name what it is pinned to"
        );
        assert!(!driver().repin_needed(&usb), "nothing runs");
    }

    #[test]
    fn the_nodes_not_appearing_in_time_fails_naming_the_setting_and_cleans_up() {
        let t0 = Instant::now();
        let mut d = driver();
        d.start(&real(), &Picks::default(), Ok(()), t0);
        assert_eq!(d.tick(t0 + 3 * S, &real()), vec![]);
        assert_eq!(
            d.tick(t0 + 4 * S, &real()),
            vec![AecAction::Kill, AecAction::Remove]
        );
        let AecPhase::Failed(reason) = d.phase() else {
            panic!()
        };
        assert_eq!(
            reason,
            "the echo-cancel nodes did not appear within 4 s (aec.ready_timeout_s)"
        );
        // A later start is allowed (the owner switches the mode again).
        assert_eq!(d.start(&real(), &Picks::default(), Ok(()), t0).len(), 2);
    }

    #[test]
    fn no_plugin_or_no_defaults_or_foreign_nodes_fail_without_touching_anything() {
        let mut d = driver();
        assert_eq!(
            d.start(
                &real(),
                &Picks::default(),
                Err("no plugin".into()),
                Instant::now()
            ),
            vec![]
        );
        assert_eq!(d.phase(), AecPhase::Failed("no plugin".into()));
        let mut d = driver();
        let mut g = real();
        g.default_source = None;
        assert_eq!(
            d.start(&g, &Picks::default(), Ok(()), Instant::now()),
            vec![]
        );
        assert!(matches!(d.phase(), AecPhase::Failed(r) if r.contains("WirePlumber")));
        let mut d = driver();
        assert_eq!(
            d.start(&up(real()), &Picks::default(), Ok(()), Instant::now()),
            vec![]
        );
        assert!(matches!(d.phase(), AecPhase::Failed(r) if r.contains("exist already")));
    }

    #[test]
    fn the_child_dying_while_active_restores_and_fails() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        let a = d.child_exited("signal 9", &g, t0);
        assert_eq!(a[0], AecAction::Kill);
        assert_eq!(a.last(), Some(&AecAction::Sync));
        assert_eq!(a.len(), 4);
        assert!(matches!(d.phase(), AecPhase::Failed(r) if r.contains("signal 9")));
        assert!(!d.routes());
        assert_eq!(d.confirmed(), vec![AecAction::Remove]);
        assert!(matches!(d.phase(), AecPhase::Failed(_)));
    }

    #[test]
    fn the_nodes_vanishing_while_active_is_a_loss_too() {
        let t0 = Instant::now();
        let (mut d, g) = active(t0);
        let mut gone = g.clone();
        gone.has_sink = false;
        let a = d.on_graph(&gone, t0);
        assert_eq!(a.first(), Some(&AecAction::Kill));
        assert!(matches!(d.phase(), AecPhase::Failed(r) if r.contains("went away")));
    }

    #[test]
    fn a_failed_save_never_spawns() {
        let mut d = driver();
        let a = d.start(&real(), &Picks::default(), Ok(()), Instant::now());
        assert_eq!(
            d.failed(
                &a[0],
                "cannot write aec.json".into(),
                &real(),
                Instant::now()
            ),
            vec![AecAction::Kill, AecAction::Remove]
        );
        assert_eq!(d.phase(), AecPhase::Failed("cannot write aec.json".into()));
    }

    #[test]
    fn crash_recovery_restores_only_what_still_names_kai_then_removes_the_file() {
        let mut d = driver();
        // Kai died with both flipped; the nodes are gone with the child.
        let a = d.recover(Ok(Some(saved())), &flipped(real()), Instant::now());
        assert_eq!(
            a,
            vec![
                AecAction::SetDefault {
                    key: DefaultKey::Sink,
                    value: Some("spk".into())
                },
                AecAction::SetDefault {
                    key: DefaultKey::Source,
                    value: None
                },
                AecAction::Sync,
            ]
        );
        assert!(d.busy(), "a start waits for the recovery");
        assert_eq!(
            d.start(&real(), &Picks::default(), Ok(()), Instant::now()),
            vec![]
        );
        assert_eq!(d.confirmed(), vec![AecAction::Remove]);
        assert!(d.note.as_ref().unwrap().contains("did not end cleanly"));
        // The owner already chose other defaults: nothing to restore, the file goes.
        let mut d = driver();
        let mut g = real();
        g.configured_source = Some("mic2".into());
        assert_eq!(
            d.recover(Ok(Some(saved())), &g, Instant::now()),
            vec![AecAction::Remove]
        );
        // No file, nothing to do.
        assert_eq!(driver().recover(Ok(None), &real(), Instant::now()), vec![]);
    }

    #[test]
    fn aec_json_round_trips_and_is_replaced_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kai").join("aec.json");
        assert_eq!(load(&path), Ok(None));
        save(&path, &saved()).unwrap();
        assert_eq!(load(&path), Ok(Some(saved())));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"configured_source\": null"), "{text}");
        let mut other = saved();
        other.configured_sink = None;
        save(&path, &other).unwrap();
        assert_eq!(load(&path), Ok(Some(other)));
        assert!(!dir.path().join("kai").join(".aec.json.tmp").exists());
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert_eq!(load(&path), Ok(None));
        std::fs::write(&path, "{").unwrap();
        assert!(load(&path).unwrap_err().contains("aec.json"));
    }

    #[test]
    fn node_names_are_quoted_for_spa_json() {
        let c = conf_text(r#"a "quoted" mic"#, "spk");
        assert!(c.contains(r#"target.object = "a \"quoted\" mic""#), "{c}");
        assert!(is_own_node(SINK) && is_own_node("kai_aec_capture") && !is_own_node("aec_sink"));
    }
}
