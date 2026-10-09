//! Routines (ruling 25, K28, WP20): tools the owner (or any agent) writes as folders under
//! `$XDG_DATA_HOME/kai/routines/<name>/`, each offered by Kai's hosted server like any tool.
//!
//! - [`def`]: `routine.toml` and the three kinds: `script` ([`script`]: any executable in its own
//!   app scope, stdout as the result, the exit code deciding), `steps` ([`steps`]: fixed tool
//!   calls, no model) and `goal` ([`goal`]: K25's delegate loop with the routine's tools).
//! - **On and off:** a routine is offered only while `routines.enabled.<name>` is on ([`settings`]); a
//!   new one is off. Decided by the owner 2026-10-09: no pinning, the latest version always runs
//!   (an edit after the switch-on takes effect without a second look). So a routine that is on is
//!   exactly as trustworthy as everything that can write its folder; `routine:<name>` says which
//!   routine called, not what its folder holds. `routine_enable` (and the tray's switch, which
//!   calls it) asks the owner first, showing the routine in full: its `routine.toml`, its script
//!   and every other file at the top of its folder ([`Inner::show`]); the dialog is built from the
//!   bytes it read, and the routine must load from them. K15: confirmed by default, ruling 28 lets
//!   the owner allow it; a thread's approval (F) does not switch one on
//!   (`crate::host::policy::approval_applies`). Switching one off asks nobody from the tray. A name
//!   a running `routine_create` writes is not switched on until that task has ended.
//! - **What a routine may do** (K28, a draft of 2026-10-09): its `routine.toml` gets at most
//!   confirm and the owner's level ([`def`]); what it asks beyond (unasked runs for trusted
//!   callers, a wider level) is the owner's `policy.tools.<name>` / `policy.levels.<name>`. The
//!   switch-on dialog says what it asks and has one box per grant, each unticked, and writes
//!   the ticked ones as the owner with the switch (`Shown`), undone if the switch fails; without the
//!   dialog (`routine_enable` set to `allow`) nothing is granted. `allow_trusted` on
//!   `routine_enable` does not skip the dialog either (`Policy::decide_call`).
//! - **Orphans:** a switch that is on and whose folder is gone (the entry itself is not there; a folder that
//!   cannot be read concludes nothing) becomes `false` at the next read ([`Inner::rescan`]), so a new folder of that name, anyone's, is off until the owner's look; the
//!   name stays taken (`routine_create` leaves it alone) and the tray lists it to forget.
//! - **Writes** of the configuration (the switches, an orphan's `false`, the grants) go through the
//!   core in the running Kai ([`Settings::Core`]), the one writer, so a write here never loses
//!   another's and a changed `[policy]` reaches the host at once.
//! - **The list:** this module is a provider of the host's (`registry::providers`). Its entries
//!   are `routine_create`, `routine_enable` (`contract::routines`) and every routine that is on;
//!   an `async = true` routine is K26's open-ended class, which lmgw calls as an MCP task and the
//!   host runs as a job (WP18, `crate::host::tasks`), its `announce` the job's. The folders are read at the start and again on every change
//!   ([`watch`]: inotify on the folder, each routine's folder and the configuration's), and a change
//!   of what is offered lists the tools again (`list_changed`, K22).
//! - **Triggers:** a tool call (voice, chat, phone, any principal lmgw lets reach Kai), the CLI
//!   (`kai tool <name>`, `crate::cli`), the tray's Routines menu and a global shortcut per routine
//!   ([`shortcut_actions`]); the tray's and a key's run is the owner's own call (a routine that asked
//!   to run unasked, read-only or trusted, is not confirmed again for it; one that changes state is), its result a
//!   notification (stdout within a notification's room, [`crate::notify::BODY_ROOM`]; more is the
//!   run's error naming the size), stopped by the tray's Stop or after `routines.run_timeout_s`
//!   (0, the default: no deadline). Schedules are open (§16 Q16).
//! - **Callers (K15, K28):** a routine's own calls (its steps, its goal's loop, its script's
//!   `kai tool`) are the caller `routine:<name>`; the policy decides them as for any other caller
//!   (trusted only in `policy.trusted_callers`). A routine does not start routines: a call of one
//!   by a routine is refused, so none recurses by accident. That rule reads the caller, which
//!   identifies, it does not bound: a script can start `systemd-run --user kai tool <routine>` and
//!   be the caller `cli` (`crate::cli`), as any program of the user can.
//! - **The runtime** ([`serve`]) starts with the running Kai: the CLI's socket, the watcher and the
//!   tray's and keys' triggers. Without it (`kai --tool-list`) the routines are listed, not run.

mod create;
pub mod def;
mod goal;
mod script;
pub mod settings;
mod steps;
#[cfg(test)]
pub(crate) mod tests;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, RwLock};
use std::time::Duration;

use kai_desktop::contract::{ToolSpec, routines::AREA};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot, watch as tokio_watch};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::host::confirm::{Confirmer, Dialog, DialogOption, shown_text};
use crate::host::local::Local;
use crate::host::policy::{Caller, Policy, SharedPolicy};
use crate::host::registry::{Call, Changes, Deps, Entry, Prepared, Preparer, Provider, Reply};
use crate::lmgw_link::Credentials;
use crate::settings::Request;
use def::{Body, Loaded, Routine};
pub use script::{Env as ScriptEnv, LiveScopes, unit_prefix};
use settings::RoutinesConfig;

/// The routines' provider (module doc).
pub struct Routines {
    pub inner: Arc<Inner>,
}

/// What every part of the routines shares.
pub struct Inner {
    /// The routines' folder; `None`: none (the unit tests' inert provider, or no home).
    dir: Option<PathBuf>,
    state: RwLock<State>,
    changes: Changes,
    /// Whether the tray, the shortcuts and the configuration operations see this provider's
    /// routines (the running Kai's, not a test's).
    publish: bool,
    runtime: OnceLock<Runtime>,
    /// The runs the tray and the keys started that have not ended, by a number of their own: the
    /// tray's Stop cancels them.
    runs: Mutex<BTreeMap<u64, (String, CancellationToken)>>,
    pub(crate) policy: SharedPolicy,
    pub(crate) confirmer: Arc<dyn Confirmer>,
    pub(crate) creds: tokio_watch::Receiver<Option<Credentials>>,
    pub(crate) client: Option<reqwest::Client>,
    /// The configuration as read at the start (the conversation's folder, `lmgw.timeout_s`,
    /// `delegate.max_turns`, `routines.create_alias`); the switches are read live ([`State`]).
    pub(crate) config: Arc<Config>,
    /// The names running `routine_create` tasks write: `routine_enable` does not switch them on
    /// until the task has ended.
    pub(crate) drafting: create::Drafting,
}

/// The routines as last read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub loaded: Vec<Loaded>,
    pub switches: RoutinesConfig,
}

/// Where a routine folder stands with its switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Off,
    /// On: offered (when it loads), in whatever version the folder holds now.
    On,
}

impl State {
    pub fn standing(&self, l: &Loaded) -> Standing {
        if self.switches.switched_on(&l.name) {
            Standing::On
        } else {
            Standing::Off
        }
    }

    /// The switches whose routine folder is gone.
    pub fn orphans(&self) -> Vec<String> {
        self.switches
            .enabled
            .keys()
            .filter(|n| !self.loaded.iter().any(|l| &l.name == *n))
            .cloned()
            .collect()
    }
}

/// What runs routines: set by [`serve`] in the running Kai.
pub struct Runtime {
    pub local: Local,
    pub script: script::Env,
    /// Kai's configuration file: the switches are read from it.
    pub config_file: PathBuf,
    /// How the switches and the grants are written.
    pub settings: Settings,
}

/// How the routines write Kai's configuration, always as the owner's own UI (the tray, the
/// switch-on dialog): the switches, an orphan's `false`, the grants of what a routine asks.
pub enum Settings {
    /// Through the core (`Command::Setting`), the running Kai's: one writer for every write, so
    /// none is lost to another's read-modify-write, and a changed `[policy]` reaches the host.
    Core(mpsc::UnboundedSender<crate::core::Command>),
    /// Straight to the file, the policy taken into the provider's (the tests').
    #[cfg_attr(not(test), allow(dead_code))]
    File,
}

/// The provider `registry::providers` lists. In the unit tests it reads no folder (the tests build
/// their own over temp folders).
pub fn provider(deps: &Deps) -> Routines {
    #[cfg(not(test))]
    let (dir, publish) = match crate::paths::routines_dir() {
        Ok(d) => (Some(d), true),
        Err(e) => {
            tracing::warn!("routines: no folder for them ({e}); none is offered");
            (None, true)
        }
    };
    #[cfg(test)]
    let (dir, publish) = (None, false);
    let timeout = deps.config.lmgw.timeout();
    let client = match reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(timeout)
        .build()
    {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::error!(
                "routines: cannot build the HTTP client: {}; goals, routine_create and kai model \
                 will fail",
                crate::lmgw_link::describe(&e)
            );
            None
        }
    };
    Routines::new(Inner {
        dir,
        state: RwLock::new(State {
            loaded: Vec::new(),
            switches: deps.config.routines.clone(),
        }),
        changes: deps.changes.clone(),
        publish,
        runtime: OnceLock::new(),
        runs: Mutex::default(),
        policy: deps.policy.clone(),
        confirmer: Arc::new(Dialog(deps.ui.clone())),
        creds: deps.creds.clone(),
        client,
        config: deps.config.clone(),
        drafting: Default::default(),
    })
}

impl Routines {
    /// Reads the folder once (no runtime yet: the switches are the configuration's at the start).
    pub fn new(inner: Inner) -> Self {
        let inner = Arc::new(inner);
        inner.rescan(None);
        Self { inner }
    }
}

/// The routine folders in `dir`, in name order (K22: a stable list). Hidden entries and files are
/// not routines. A folder that cannot be read is an error, not an empty list: nothing may be
/// concluded from it (an orphan's switch stays as it is, [`Inner::rescan`]).
pub fn scan(dir: &Path) -> std::io::Result<Vec<Loaded>> {
    let rd = std::fs::read_dir(dir)?;
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Ok(dirs.iter().map(|d| def::load(d)).collect())
}

impl Inner {
    /// Reads the routines again, and the switches from `config_file` (`None`: kept as they are). A
    /// change of what is offered signals the host. Returns whether anything changed.
    pub fn rescan(&self, config_file: Option<&Path>) -> bool {
        // A folder that cannot be read keeps the routines as they were and concludes nothing:
        // no switch becomes false because of it. One that is not there is a plain empty list.
        let (loaded, readable) = match self.dir.as_deref().map(scan) {
            None => (Vec::new(), true),
            Some(Ok(l)) => (l, true),
            Some(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), true),
            Some(Err(e)) => {
                tracing::warn!(
                    "routines: the routines' folder cannot be read ({e}); the routines and the \
                     switches stay as they were"
                );
                (self.state().loaded, false)
            }
        };
        let mut switches = match config_file {
            None => self.state().switches,
            Some(f) => match Config::load(f) {
                Ok(c) => c.routines,
                Err(e) => {
                    tracing::warn!(
                        "routines: the configuration does not load ({e}); the switches stay as \
                         they were"
                    );
                    self.state().switches
                }
            },
        };
        // A switch that is on and whose folder is gone becomes `false` (module doc): the name stays
        // taken and listed to forget, and a new folder of that name waits for the owner's look.
        // Only with the runtime, reading the file, and the routines' folder there to read.
        if let (Some(dir), Some(_), Some(_)) = (&self.dir, self.runtime.get(), config_file)
            && readable
            && dir.is_dir()
        {
            let gone: Vec<String> = switches
                .enabled
                .keys()
                .filter(|n| switches.switched_on(n) && !loaded.iter().any(|l| &l.name == *n))
                // Gone only when the entry itself is not there (not a folder that failed to
                // read, a dangling link, a permission): anything else leaves the switch alone.
                .filter(|n| {
                    matches!(
                        std::fs::symlink_metadata(dir.join(n)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound
                    )
                })
                .cloned()
                .collect();
            for name in gone {
                tracing::info!(
                    routine = %name,
                    "routines: its folder is gone; its switch is now false, so a new folder of \
                     that name waits for the owner's look"
                );
                switches
                    .enabled
                    .insert(name.clone(), settings::Switch::Flag(false));
                self.write_now(Request::Set {
                    key: format!("{}.{name}", settings::ENABLED_TABLE),
                    value: "false".into(),
                });
            }
        }
        let fresh = State { loaded, switches };
        let before = {
            let mut s = self.state.write().expect("never poisoned");
            std::mem::replace(&mut *s, fresh.clone())
        };
        if before == fresh {
            return false;
        }
        for l in &fresh.loaded {
            if let Err(why) = &l.routine
                && !before
                    .loaded
                    .iter()
                    .any(|b| b.name == l.name && b.routine == l.routine)
            {
                tracing::warn!(routine = %l.name, "routines: does not load: {why}");
            }
        }
        tracing::info!(
            routines = fresh.loaded.len(),
            on = fresh
                .loaded
                .iter()
                .filter(|l| fresh.standing(l) == Standing::On)
                .count(),
            "routines: read"
        );
        self.publish_state(&fresh);
        self.changes.signal();
        true
    }

    pub fn state(&self) -> State {
        self.state.read().expect("never poisoned").clone()
    }

    /// The routine folder `name` as last read.
    fn loaded(&self, name: &str) -> Option<Loaded> {
        self.state
            .read()
            .expect("never poisoned")
            .loaded
            .iter()
            .find(|l| l.name == name)
            .cloned()
    }

    /// The routines that are on, as the owner saw them, and loaded.
    fn offered(&self) -> Vec<Routine> {
        let s = self.state.read().expect("never poisoned");
        s.loaded
            .iter()
            .filter(|l| s.standing(l) == Standing::On)
            .filter_map(|l| l.routine.as_ref().ok().cloned())
            .collect()
    }

    /// The tray's entries for `s` and the runs going now.
    pub fn tray_items(&self, s: &State) -> Vec<TrayRoutine> {
        let running: Vec<String> = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|(n, _)| n.clone())
            .collect();
        let mut items: Vec<TrayRoutine> = s
            .loaded
            .iter()
            .map(|l| {
                let standing = s.standing(l);
                let on = standing == Standing::On;
                let (runnable, note) = match (&l.routine, ()) {
                    (Err(why), ()) => (false, Some(why.clone())),
                    (Ok(r), ()) if r.spec.params.iter().any(|p| p.required) => (
                        false,
                        Some("takes parameters: run it as a tool or with kai tool".to_owned()),
                    ),
                    (Ok(_), ()) => (on, None),
                };
                TrayRoutine {
                    name: l.name.clone(),
                    on,
                    runnable,
                    running: running.contains(&l.name),
                    note,
                    orphan: false,
                }
            })
            .collect();
        items.extend(s.orphans().into_iter().map(|name| TrayRoutine {
            running: running.contains(&name),
            name,
            on: false,
            runnable: false,
            note: Some("its folder is gone".into()),
            orphan: true,
        }));
        items
    }

    fn publish_state(&self, s: &State) {
        if !self.publish {
            return;
        }
        *remote::SPECS.write().unwrap_or_else(|e| e.into_inner()) = s
            .loaded
            .iter()
            .filter_map(|l| l.routine.as_ref().ok().map(|r| (l.name.clone(), r.spec)))
            .collect();
        remote::TRAY.send_replace(self.tray_items(s));
    }

    /// Runs the routine `name` for `call` (it was listed when the call came), as the folder holds
    /// it now: its last read, and a script's file as it is when it starts.
    async fn run(self: Arc<Self>, name: String, call: Call) -> Reply {
        if let Caller::Named(n) = &call.caller
            && n.kind == "routine"
        {
            return Reply::error(format!(
                "a routine does not start routines ({name} was called by the routine {})",
                n.name
            ));
        }
        let Some(rt) = self.runtime.get() else {
            return Reply::error("routines run only in the running Kai");
        };
        let Some(loaded) = self.loaded(&name) else {
            return Reply::error(format!("the routine {name} is gone"));
        };
        let standing = self.state().standing(&loaded);
        let routine = match (loaded.routine, standing) {
            (Ok(r), Standing::On) => r,
            (Ok(_), Standing::Off) => return Reply::error(format!("the routine {name} is off")),
            (Err(why), _) => {
                return Reply::error(format!("the routine {name} does not load: {why}"));
            }
        };
        tracing::info!(routine = %name, kind = routine.body.word(), caller = %call.caller.describe(), "routines: a run");
        // Ruling 22: an async routine's own `announce` decides for its job (WP18).
        if let Some(on) = routine.announce {
            crate::jobs::set_announce(on);
        }
        match &routine.body {
            Body::Script { path } => script::run(&rt.script, &routine, path, &call).await,
            Body::Steps(list) => steps::run(&rt.local, &routine, list, &call).await,
            Body::Goal { goal, tools, alias } => {
                goal::run(
                    &self,
                    &rt.local,
                    &routine,
                    goal,
                    tools,
                    alias.as_deref(),
                    &call,
                )
                .await
            }
        }
    }

    /// `routine_enable`'s preparation: switching on shows the routine in full from the bytes it
    /// read. The folder is read once: what `routine.toml` asks beyond Kai's default for a routine
    /// first (with the box that grants it, less what the owner's entries set already), then
    /// `routine.toml`, a script's file and every other file at its top, each in full when it is
    /// text (UTF-8, no NUL) and otherwise by its size and SHA-256; subfolders are named as not
    /// shown. The routine must load from exactly these bytes.
    fn show(&self, name: &str, on: bool) -> Result<Prepared, String> {
        if !on {
            if self.loaded(name).is_none() && !self.state().switches.has_switch(name) {
                return Err(format!("Kai has no routine {name}"));
            }
            return Ok(Prepared {
                lines: Vec::new(),
                state: Arc::new(()),
                notes: Vec::new(),
                options: Vec::new(),
            });
        }
        let Some(root) = &self.dir else {
            return Err(format!("Kai has no routine {name}"));
        };
        if self.loaded(name).is_none() || !def::plain_name(name) {
            return Err(format!("Kai has no routine {name}"));
        }
        if self.drafting.contains(name) {
            return Err(drafting_refusal(name));
        }
        let dir = root.join(name);
        let folder = def::read_folder(&dir, |_| true)
            .map_err(|why| format!("{name} cannot be switched on: {why}"))?;
        let routine = folder
            .routine(&dir, name)
            .map_err(|why| format!("{name} cannot be switched on: it does not load: {why}"))?;
        // What it asks beyond confirm and the owner's level, less what the owner set already.
        let set = self
            .runtime
            .get()
            .and_then(|rt| Config::load(&rt.config_file).ok())
            .map(|c| c.policy)
            .unwrap_or_default();
        let grants: Vec<(String, String)> = routine
            .asks
            .grants(name)
            .into_iter()
            .filter(|(key, _)| {
                !(key.starts_with("policy.tools.") && set.tools.contains_key(name)
                    || key.starts_with("policy.levels.") && set.levels.contains_key(name))
            })
            .collect();
        let mut lines = asks_lines(&routine.asks, &grants);
        lines.extend(shown_folder(&folder, &routine));
        // One box per grant, so the owner can give one without the other.
        let options: Vec<DialogOption> = grants
            .iter()
            .map(|(k, v)| DialogOption::new(format!("Also grant: {k} = {v}")))
            .collect();
        Ok(Prepared {
            lines,
            state: Arc::new(Shown {
                grants,
                options: options.clone(),
            }),
            notes: Vec::new(),
            options,
        })
    }

    /// Takes back grants that were written (a later write failed): each key is unset. A failure
    /// of its own is logged, it cannot be told to anyone better.
    async fn undo(&self, granted: &[&(String, String)]) {
        for (key, _) in granted.iter().rev() {
            if let Err(why) = self.write(Request::Unset { key: key.clone() }).await {
                tracing::warn!("routines: {key} could not be taken back: {why}");
            }
        }
    }

    /// Writes the switch `routines.enabled.<name>` and reads the routines again.
    async fn switch(&self, name: &str, on: bool) -> Result<(), String> {
        self.write(Request::Routine {
            name: name.to_owned(),
            enabled: on,
        })
        .await?;
        if let Some(rt) = self.runtime.get() {
            self.rescan(Some(&rt.config_file));
        }
        Ok(())
    }

    /// One write of the configuration as the owner ([`Settings`]), answered once it is written.
    async fn write(&self, request: Request) -> Result<(), String> {
        let Some(rt) = self.runtime.get() else {
            return Err("routines are switched only in the running Kai".into());
        };
        match &rt.settings {
            Settings::Core(tx) => {
                let (reply, rx) = oneshot::channel();
                let quitting = || "Kai is quitting".to_owned();
                tx.send(crate::core::Command::Setting { request, reply })
                    .map_err(|_| quitting())?;
                rx.await.map_err(|_| quitting())?.map(drop).map_err(|e| e.0)
            }
            Settings::File => self.write_file(&rt.config_file, request),
        }
    }

    /// [`Inner::write`] without waiting for the answer (a rescan's); a refusal is logged.
    fn write_now(&self, request: Request) {
        let Some(rt) = self.runtime.get() else {
            return;
        };
        let said =
            |why: String| tracing::warn!("routines: the configuration was not written: {why}");
        match &rt.settings {
            Settings::Core(tx) => {
                let (reply, rx) = oneshot::channel();
                if tx
                    .send(crate::core::Command::Setting { request, reply })
                    .is_err()
                {
                    return said("Kai is quitting".into());
                }
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Ok(Err(e)) = rx.await {
                            said(e.0);
                        }
                    });
                }
            }
            Settings::File => {
                if let Err(why) = self.write_file(&rt.config_file, request) {
                    said(why);
                }
            }
        }
    }

    /// [`Settings::File`]'s write: the file, then the provider's policy.
    fn write_file(&self, file: &Path, request: Request) -> Result<(), String> {
        let outcome = crate::settings::apply(file, request).map_err(|e| e.0)?;
        if let Some(c) = outcome.config {
            *self.policy.write().expect("never poisoned") = Policy::new(&c.policy);
        }
        Ok(())
    }

    /// Cancels the tray's and keys' runs of `name`; how many there were.
    pub fn stop(&self, name: &str) -> usize {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for (r, token) in runs.values() {
            if r == name {
                token.cancel();
                n += 1;
            }
        }
        n
    }

    fn enable_entry(self: &Arc<Self>) -> Entry {
        let spec = &AREA.tools[1];
        let inner = self.clone();
        let shower = self.clone();
        let prepare: Preparer = Arc::new(move |args, _| {
            let name = args.text("name").unwrap_or_default().to_owned();
            let on = args.boolean("on").unwrap_or(false);
            let shown = shower.show(&name, on);
            Box::pin(async move { shown })
        });
        Entry {
            area: AREA.name,
            spec,
            handler: Arc::new(move |call| {
                let inner = inner.clone();
                Box::pin(async move {
                    let name = call.args.text("name").unwrap_or_default().to_owned();
                    let on = call.args.boolean("on").unwrap_or(false);
                    // On only after the dialog showed it (the preparation ran).
                    if on && call.prepared.is_none() {
                        return Reply::error(format!(
                            "{name} was not shown to the owner; it stays off"
                        ));
                    }
                    if on && inner.drafting.contains(&name) {
                        return Reply::error(drafting_refusal(&name));
                    }
                    // The grants the owner ticked in the dialog (each its own box), written before
                    // the switch so a routine is never on with half of what was granted. The
                    // settings layer has no batch, so a failure undoes what was written: the keys
                    // are ones the owner had not set (`Inner::show`), so unsetting restores them.
                    let ticked: Vec<(String, String)> = if on
                        && let Some(shown) = call
                            .prepared
                            .as_ref()
                            .and_then(|p| p.state.downcast_ref::<Shown>())
                    {
                        shown
                            .grants
                            .iter()
                            .zip(&shown.options)
                            .filter(|(_, o)| o.is_ticked())
                            .map(|(g, _)| g.clone())
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let mut granted: Vec<&(String, String)> = Vec::new();
                    for grant in &ticked {
                        let (key, value) = grant;
                        let set = Request::Set {
                            key: key.clone(),
                            value: value.clone(),
                        };
                        if let Err(why) = inner.write(set).await {
                            inner.undo(&granted).await;
                            return Reply::error(format!(
                                "{name} stays off: {key} = {value} was not written: {why}"
                            ));
                        }
                        granted.push(grant);
                    }
                    let said: Vec<String> =
                        granted.iter().map(|(k, v)| format!("{k} = {v}")).collect();
                    match inner.switch(&name, on).await {
                        Ok(()) if on && said.is_empty() => Reply::text(format!("{name} is on")),
                        Ok(()) if on => {
                            Reply::text(format!("{name} is on; granted {}", said.join(", ")))
                        }
                        Ok(()) => Reply::text(format!("{name} is off")),
                        Err(why) => {
                            inner.undo(&granted).await;
                            Reply::error(format!(
                                "{name} stays off: {why}{}",
                                if granted.is_empty() {
                                    String::new()
                                } else {
                                    format!("; the grants {} were taken back", said.join(", "))
                                }
                            ))
                        }
                    }
                })
            }),
            prepare: Some(prepare),
        }
    }

    fn create_entry(self: &Arc<Self>) -> Entry {
        let inner = self.clone();
        Entry {
            area: AREA.name,
            spec: &AREA.tools[0],
            handler: Arc::new(move |call| {
                let inner = inner.clone();
                Box::pin(async move {
                    let (Some(rt), Some(dir)) = (inner.runtime.get(), inner.dir.clone()) else {
                        return Reply::error("routines are written only by the running Kai");
                    };
                    create::run(&inner, &rt.local, &dir, &call).await
                })
            }),
            prepare: None,
        }
    }
}

impl Provider for Routines {
    fn entries(&self) -> Vec<Entry> {
        let mut all = vec![self.inner.create_entry(), self.inner.enable_entry()];
        for r in self.inner.offered() {
            // K26: an asynchronous routine (`async = true`) is the same entry; lmgw calls it as an
            // MCP task and the host runs it as a job (WP18, `crate::host::tasks`), whose announce
            // the run sets from `r.announce`.
            let inner = self.inner.clone();
            let name = r.name.clone();
            all.push(Entry {
                area: AREA.name,
                spec: r.spec,
                handler: Arc::new(move |call| Box::pin(inner.clone().run(name.clone(), call))),
                prepare: None,
            });
        }
        all
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// What `routine_enable`'s preparation hands its handler: the grants the dialog offered and its
/// checkboxes, one per grant in the same order (`options[i]` ticked: `grants[i]` is written).
struct Shown {
    grants: Vec<(String, String)>,
    options: Vec<DialogOption>,
}

fn drafting_refusal(name: &str) -> String {
    format!(
        "routine_create is still writing {name}; switch it on once that task has ended, so what \
         the dialog shows is the finished draft"
    )
}

/// The dialog's first lines when the routine asks for more than Kai gives one (`def::Asks`):
/// what it asks, and that only the box grants it.
fn asks_lines(asks: &def::Asks, grants: &[(String, String)]) -> Vec<String> {
    if asks.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        "routine.toml asks for more than Kai gives a routine from its folder (confirmed, the \
         owner's level):"
            .to_owned(),
    ];
    if asks.allow_trusted {
        lines.push(
            "  to run unasked for trusted callers (policy = \"trusted\" or read_only = true)"
                .into(),
        );
    }
    match asks.level {
        Some(crate::host::settings::LevelSetting::Trusted) => {
            lines.push("  to be callable by the trusted callers (level = \"trusted\")".to_owned())
        }
        Some(_) => lines.push(
            "  to be callable by any caller lmgw lets reach Kai, kai tool from the command line \
             included (level = \"any\", or no level)"
                .to_owned(),
        ),
        None => {}
    }
    lines.push(if grants.is_empty() {
        "  your own policy.tools and policy.levels entries for it decide instead.".to_owned()
    } else {
        "  Kai gives it only what you grant with the boxes below (the folder can change after it is \
         on, the latest version always runs)."
            .to_owned()
    });
    lines
}

/// The dialog's lines for `folder` (`Inner::show`).
fn shown_folder(folder: &def::Folder, routine: &Routine) -> Vec<String> {
    use def::EntryKind;
    let script = match &routine.body {
        Body::Script { path } => path.file_name().map(|n| n.to_string_lossy().into_owned()),
        _ => None,
    };
    let rank = |n: &str| {
        if n == def::FILE {
            0
        } else if Some(n) == script.as_deref() {
            1
        } else {
            2
        }
    };
    let mut entries: Vec<&def::FolderEntry> = folder.entries.iter().collect();
    entries.sort_by(|a, b| (rank(&a.name), &a.name).cmp(&(rank(&b.name), &b.name)));
    let mut lines = Vec::new();
    for e in entries {
        let name = shown_text(&e.name);
        match &e.kind {
            EntryKind::File {
                size,
                sha256,
                bytes,
            } => {
                let text = bytes
                    .as_deref()
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .filter(|t| !t.contains('\0'));
                match text {
                    Some(t) => {
                        lines.push(format!("{name}:"));
                        lines.extend(t.lines().map(|l| format!("  {}", shown_text(l))));
                    }
                    None => lines.push(format!(
                        "{name}: not text, {size} bytes, sha256 {sha256} (not shown)"
                    )),
                }
            }
            EntryKind::Folder => lines.push(format!(
                "{name}/: a subfolder, not shown (a script may keep its state there)"
            )),
            EntryKind::Other(word) => lines.push(format!("{name}: a {word}, not read")),
        }
    }
    lines
}

/// One routine as the tray shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayRoutine {
    pub name: String,
    /// Offered: on.
    pub on: bool,
    /// The tray (and a key) can start it: on, loaded, synchronous, no required parameter.
    pub runnable: bool,
    /// A run the tray or a key started has not ended: the tray offers Stop.
    pub running: bool,
    /// Why it cannot run from the tray, does not load, or is not offered.
    pub note: Option<String>,
    /// A switch whose folder is gone: the tray offers to forget it.
    pub orphan: bool,
}

/// What the tray and a key ask of the routines' runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    Run(String),
    Switch(String, bool),
    /// Cancel the tray's and keys' runs of the routine.
    Stop(String),
    /// Remove the switch of a routine whose folder is gone.
    Forget(String),
}

/// The running Kai's routines for the tray, the shortcuts and the configuration operations, which
/// reach them without a path through the core (like `crate::widgets`'s).
mod remote {
    use super::*;

    pub static TX: Mutex<Option<mpsc::UnboundedSender<Trigger>>> = Mutex::new(None);

    pub static TRAY: LazyLock<tokio_watch::Sender<Vec<TrayRoutine>>> =
        LazyLock::new(|| tokio_watch::channel(Vec::new()).0);

    /// The specs of the routines that load, by name, whether on or off.
    pub static SPECS: LazyLock<RwLock<BTreeMap<String, &'static ToolSpec>>> =
        LazyLock::new(RwLock::default);
}

/// The tray's Routines entries, as they change.
pub fn tray_entries() -> tokio_watch::Receiver<Vec<TrayRoutine>> {
    remote::TRAY.subscribe()
}

/// The tray's or a key's request (the owner's own): dropped while no runtime runs.
pub fn trigger(t: Trigger) {
    let tx = remote::TX.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match tx {
        Some(tx) => {
            let _ = tx.send(t);
        }
        None => tracing::warn!("routines: not running yet; the request is dropped"),
    }
}

/// The shortcut id of a routine (§8: one action per routine that the tray can run; no key bound by
/// default, the owner sets one in System Settings).
pub const SHORTCUT_PREFIX: &str = "routine-";

/// The global shortcuts' actions for the routines that can run from a key, `(id, description)`, as
/// read when Kai started (a routine switched on later gets its action at the next start).
pub fn shortcut_actions() -> Vec<(String, String)> {
    remote::TRAY
        .borrow()
        .iter()
        .filter(|r| r.runnable)
        .map(|r| {
            (
                format!("{SHORTCUT_PREFIX}{}", r.name),
                format!("Kai: run the routine {}", r.name),
            )
        })
        .collect()
}

/// A routine folder of that name exists (the configuration operations' `known_tool`).
pub fn known(name: &str) -> bool {
    remote::TRAY.borrow().iter().any(|r| r.name == name)
}

/// The spec of the routine `name` as loaded now, on or off (K30's guard reads its defaults:
/// `crate::settings::widen`).
pub fn spec(name: &str) -> Option<&'static ToolSpec> {
    remote::SPECS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .copied()
}

/// The runtime (module doc): runs until Kai's runtime ends. `socket` is where the CLI is served.
pub async fn serve(local: Local, rt: Runtime, socket: PathBuf, timeout: Duration) {
    let Some(inner) = local.with_provider(|r: &Routines| r.inner.clone()) else {
        tracing::error!("routines: the host has no routines provider; none runs");
        return;
    };
    let config_file = rt.config_file.clone();
    let bus = rt.script.manager.bus.clone();
    let scopes = rt.script.scopes.clone();
    if inner.runtime.set(rt).is_err() {
        tracing::error!("routines: the runtime runs already");
        return;
    }
    if let Some(dir) = &inner.dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("routines: cannot make {}: {e}", dir.display());
    }
    inner.rescan(Some(&config_file));
    let (tx, mut rx) = mpsc::unbounded_channel();
    *remote::TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    let watching = watch_loop(inner.clone(), config_file.clone());
    let cli = crate::cli::server::serve(
        socket,
        crate::cli::server::Backend {
            local: local.clone(),
            creds: inner.creds.clone(),
            client: inner.client.clone(),
            cgroup_of: crate::cli::server::proc_cgroup,
            scopes,
        },
    );
    let triggers = async {
        while let Some(t) = rx.recv().await {
            let (local, inner, bus) = (local.clone(), inner.clone(), bus.clone());
            tokio::spawn(async move {
                let said = on_trigger(&local, &inner, t).await;
                if let Some((title, body)) = said
                    && let Ok(conn) = bus.get().await
                {
                    crate::notify::send(&conn, &title, &body, timeout).await;
                }
            });
        }
    };
    tokio::join!(watching, cli, triggers);
}

/// The text blocks of a reply, joined.
fn reply_text(r: &Reply) -> String {
    r.blocks
        .iter()
        .filter_map(|b| match b {
            crate::host::registry::Block::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A number for each run the tray or a key starts.
static NEXT_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A tray's or key's request, as the owner's own call; what to say in a notification.
pub async fn on_trigger(local: &Local, inner: &Arc<Inner>, t: Trigger) -> Option<(String, String)> {
    match t {
        Trigger::Run(name) => Some(tray_run(local, inner, name).await),
        Trigger::Stop(name) => match inner.stop(&name) {
            0 => Some((format!("Routine {name}"), "it was not running".into())),
            _ => None,
        },
        // Off asks nobody; so does forgetting the switch of a folder that is gone.
        Trigger::Switch(name, false) | Trigger::Forget(name) => inner
            .switch(&name, false)
            .await
            .err()
            .map(|why| (format!("Routine {name}"), why)),
        // On goes through routine_enable, which shows it first.
        Trigger::Switch(name, true) => {
            let entry = local.entry_any(kai_desktop::contract::routines::ENABLE)?;
            let mut args = Map::new();
            args.insert("name".into(), Value::String(name.clone()));
            args.insert("on".into(), Value::Bool(true));
            let r = local
                .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
                .await;
            r.error.then(|| (format!("Routine {name}"), reply_text(&r)))
        }
    }
}

/// A run from the tray or a key: the owner's call, its result within a notification's room,
/// stopped by the tray's Stop or after `routines.run_timeout_s` (read now; 0: no deadline).
async fn tray_run(local: &Local, inner: &Arc<Inner>, name: String) -> (String, String) {
    let Some(entry) = local.entries().into_iter().find(|e| e.spec.name == name) else {
        return (
            format!("Routine {name}"),
            "it is off or not offered now".into(),
        );
    };
    let id = NEXT_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cancel = CancellationToken::new();
    inner
        .runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, (name.clone(), cancel.clone()));
    inner.publish_state(&inner.state());
    let timeout_s = inner.state().switches.run_timeout_s;
    // A tray's or key's run is the owner's own act: a dialog protects nothing there, so a routine
    // that asked to run unasked (read-only, or trusted) is not confirmed again. Its owner entry
    // (`policy.tools.<name>`) still decides when set, and a routine that changes state keeps its
    // confirmation as before.
    let unasked = inner
        .loaded(&name)
        .and_then(|l| l.routine.ok())
        .is_some_and(|r| r.asks.allow_trusted)
        && inner
            .policy
            .read()
            .expect("never poisoned")
            .state(&name)
            .is_none();
    let call = local.call_approved(
        &entry,
        None,
        &Caller::Owner,
        unasked.then_some(&Caller::Owner),
        crate::host::local::LocalBounds {
            cancel: cancel.clone(),
            room: Some((crate::notify::BODY_ROOM, crate::notify::body_room_says())),
            out: None,
        },
    );
    tokio::pin!(call);
    let deadline = async {
        match timeout_s {
            0 => std::future::pending().await,
            s => tokio::time::sleep(Duration::from_secs(s)).await,
        }
    };
    let (reply, timed_out) = tokio::select! {
        r = &mut call => (r, false),
        () = deadline => {
            cancel.cancel();
            (call.await, true)
        }
    };
    inner
        .runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    inner.publish_state(&inner.state());
    if timed_out {
        return (
            format!("Routine {name} stopped"),
            format!(
                "it ran longer than routines.run_timeout_s = {timeout_s} s and was stopped (a \
                 script's scope is killed)"
            ),
        );
    }
    if cancel.is_cancelled() {
        return (
            format!("Routine {name} stopped"),
            "stopped from the tray's Routines menu (a script's scope is killed)".into(),
        );
    }
    let title = if reply.error {
        format!("Routine {name} failed")
    } else {
        format!("Routine {name}")
    };
    (title, reply_text(&reply))
}

/// Rescans on every burst of changes in the folders (module doc).
async fn watch_loop(inner: Arc<Inner>, config_file: PathBuf) {
    let w = match watch::Watch::new() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(
                "routines: cannot watch the folders ({e}); changes are read at the next start"
            );
            return;
        }
    };
    let add_all = |w: &watch::Watch| {
        if let Some(parent) = config_file.parent() {
            let _ = std::fs::create_dir_all(parent);
            if let Err(e) = w.add(parent) {
                tracing::warn!("routines: cannot watch the configuration's folder: {e}");
            }
        }
        if let Some(dir) = &inner.dir {
            if let Err(e) = w.add(dir) {
                tracing::warn!("routines: cannot watch {}: {e}", dir.display());
            }
            for l in inner.state().loaded {
                let _ = w.add(&dir.join(&l.name));
            }
        }
    };
    add_all(&w);
    loop {
        if let Err(e) = w.burst().await {
            tracing::error!("routines: the watch ended ({e}); changes are read at the next start");
            return;
        }
        inner.rescan(Some(&config_file));
        add_all(&w);
    }
}
