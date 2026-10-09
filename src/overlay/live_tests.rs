//! The ring's live check over the owner's desktop (`just live-check-ring <clip>` runs
//! scripts/live-check-ring.sh, which runs this). Ignored by default; it refuses to run unless
//! `KAI_LIVE_RING=1`, the audio is a private PipeWire instance (scripts/pw-private.sh) and a
//! Wayland display is named.
//!
//! The overlay runs as in Kai, with this test's thread as GTK's main thread. Its levels come from
//! `Audio::taps` of the private instance: a second audio handle plays the clip into the test
//! microphone (listening), the first plays it on the null sink (speaking); nothing is audible. The
//! script drives the voice session's facts as Kai's voice sets them: continuous listening between
//! turns (active: the ring up; "ring over fullscreen" toggled while it is up; its cost over 10 s
//! of silence in two ways and 10 s of a quiet room's noise, with KWin's CPU and the GPU's power
//! and P-state, against 10 s with no view and 10 s hidden; how late it shows tone bursts on the
//! microphone, see [`latency`]), a turn and back to listening, mute, its own web process killed
//! twice (a crash) and the timed retries after that, the session's end, push-to-talk (shown from
//! a press through its turn, hidden between presses). Env:
//! - `KAI_LIVE_CLIP` a 24 kHz mono PCM16 WAV; `KAI_LIVE_OUT` the report directory;
//! - `KAI_LIVE_SHOT` a command `<name> <x> <y> <w> <h>` that saves a crop of the screen (physical
//!   pixels of the full capture);
//! - `KAI_LIVE_RING_OUTPUT` the connector to configure as `ring.output`;
//! - `KAI_LIVE_RING_PLACE` `anchor,margin,size` (the ring's placement; default
//!   `bottom-right,24,240`).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use tokio::sync::mpsc;

use super::placement::{Anchor, Layer};
use super::*;
use crate::audio::{Audio, AudioSettings, Capture, EchoMode, Player, RATE};

/// The ring when its output goes away and comes back, in a nested KWin (review KAI-5 R5-14).
mod output_removed;

const MIC_IN: &str = "kai-test-mic-in";

fn guard() -> Result<(), String> {
    if std::env::var("KAI_LIVE_RING").as_deref() != Ok("1") {
        return Err("KAI_LIVE_RING is not 1".into());
    }
    let pw = std::env::var("PIPEWIRE_RUNTIME_DIR").unwrap_or_default();
    if pw.is_empty() || pw.starts_with("/run/user/") {
        return Err("not in a private PipeWire instance".into());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("no WAYLAND_DISPLAY".into());
    }
    if crate::nvidia::nvidia_present() && std::env::var_os(crate::nvidia::VAR).is_none() {
        return Err(format!(
            "NVIDIA is present: {} must be in the environment (a test cannot set it once threads exist)",
            crate::nvidia::VAR
        ));
    }
    Ok(())
}

fn read_wav(path: &str) -> Vec<i16> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut i = 12;
    while i + 8 <= b.len() {
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &b[i + 8..(i + 8 + len).min(b.len())];
        if &b[i..i + 4] == b"fmt " {
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            assert_eq!(
                (body[2], rate, body[14]),
                (1, RATE, 16),
                "{path}: 24 kHz mono PCM16"
            );
        } else if &b[i..i + 4] == b"data" {
            return body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c))
                .collect();
        }
        i += 8 + len + (len & 1);
    }
    panic!("{path}: no data chunk");
}

fn audio_settings(dir: &Path, output: Option<&str>) -> AudioSettings {
    AudioSettings {
        echo_mode: EchoMode::Device,
        input: None,
        output: output.map(str::to_owned),
        capture_ring_ms: 2000,
        playback_ring_ms: 2000,
        retry: Duration::from_secs(2),
        aec_ready_timeout: Duration::from_secs(4),
        aec_restore_timeout: Duration::from_secs(4),
        aec_file: dir.join("aec.json"),
        runtime_dir: dir.join("runtime"),
    }
}

// ------------------------------------------------------------------ processes
/// (pid, comm, ppid) of every process.
fn processes() -> Vec<(u32, String, u32)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let (Some(a), Some(b)) = (s.find('('), s.rfind(')')) else {
            continue;
        };
        let ppid = s[b + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        v.push((pid, s[a + 1..b].to_owned(), ppid));
    }
    v
}

/// This process's WebKit descendants: (pid, comm).
fn webkit_children() -> Vec<(u32, String)> {
    let all = processes();
    let mut out = Vec::new();
    let mut stack = vec![std::process::id()];
    while let Some(p) = stack.pop() {
        for (pid, comm, ppid) in &all {
            if *ppid == p {
                stack.push(*pid);
                if comm.starts_with("WebKit") {
                    out.push((*pid, comm.clone()));
                }
            }
        }
    }
    out
}

fn pss_mb(pid: u32) -> f64 {
    std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Pss:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
        / 1024.0
}

/// utime + stime in clock ticks.
fn cpu_ticks(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            let rest = &s[s.rfind(')')? + 2..];
            let f: Vec<&str> = rest.split_whitespace().collect();
            Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
        })
        .unwrap_or(0)
}

/// This process's largest mappings by PSS, by file (anonymous memory as one entry).
fn top_mappings(n: usize) -> String {
    let text = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let mut by: std::collections::HashMap<String, f64> = Default::default();
    let mut current = String::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let first = parts.next().unwrap_or("");
        if first.contains('-') && first.chars().next().is_some_and(|c| c.is_ascii_hexdigit()) {
            current = parts.nth(4).unwrap_or("[anon]").to_owned();
        } else if first == "Pss:" {
            let kb: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            *by.entry(current.clone()).or_default() += kb / 1024.0;
        }
    }
    let mut v: Vec<(String, f64)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.iter()
        .take(n)
        .map(|(k, mb)| format!("{} {mb:.0} MB", k.rsplit('/').next().unwrap_or(k)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn pss_line(label: &str) -> String {
    let me = std::process::id();
    let mut s = format!("PSS {label}: UI process {:.0} MB", pss_mb(me));
    for (pid, comm) in webkit_children() {
        s += &format!(", {comm} {:.0} MB", pss_mb(pid));
    }
    s
}

/// The compositor's process (`kwin_wayland`), read only.
fn kwin_pid() -> Option<u32> {
    processes()
        .into_iter()
        .find(|p| p.1 == "kwin_wayland")
        .map(|p| p.0)
}

/// `nvidia-smi` sampling every 500 ms (read only), this check's own child; `None` without NVIDIA.
fn gpu_sampler() -> Option<std::process::Child> {
    if !crate::nvidia::nvidia_present() {
        return None;
    }
    std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=pstate,power.draw,clocks.gr,utilization.gpu",
            "--format=csv,noheader,nounits",
            "-lms",
            "500",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

/// The GPU's samples: the P-states seen, the power draw (mean, min to max), the graphics clock and
/// the utilisation (means).
fn gpu_summary(child: std::process::Child) -> String {
    let mut child = child;
    // The sampler is this check's own child; it samples until it is ended.
    let _ = child.kill();
    let out = child
        .wait_with_output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let mut states: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut watts, mut clocks, mut util) = (Vec::new(), Vec::new(), Vec::new());
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() != 4 {
            continue;
        }
        *states.entry(f[0].to_owned()).or_default() += 1;
        watts.extend(f[1].parse::<f64>().ok());
        clocks.extend(f[2].parse::<f64>().ok());
        util.extend(f[3].parse::<f64>().ok());
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let min = watts.iter().copied().fold(f64::INFINITY, f64::min);
    let max = watts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    format!(
        "{} samples, P-states {states:?}, power {:.1} W ({min:.1} to {max:.1}), graphics clock {:.0} \
         MHz, utilisation {:.1} %",
        watts.len(),
        mean(&watts),
        mean(&clocks),
        mean(&util)
    )
}

/// Over `ms`: this process and its WebKit children (as [`cpu_line`]), `kwin_wayland`, and the
/// GPU, each line labelled.
async fn system_lines(label: &str, ms: u64) -> Vec<String> {
    let kwin = kwin_pid();
    let k0 = kwin.map(cpu_ticks);
    let gpu = gpu_sampler();
    let cpu = cpu_line(label, ms).await;
    let k1 = kwin.map(cpu_ticks);
    let kwin_line = match (k0, k1) {
        (Some(a), Some(b)) => format!(
            "kwin_wayland {label} over {ms} ms: {:.1} % of a core",
            (b - a) as f64 / 100.0 / (ms as f64 / 1000.0) * 100.0
        ),
        _ => format!("kwin_wayland {label}: no kwin_wayland process found"),
    };
    let gpu_line = match gpu {
        Some(child) => format!("GPU {label}: {}", gpu_summary(child)),
        None => format!("GPU {label}: no NVIDIA GPU (or nvidia-smi did not start)"),
    };
    vec![cpu, kwin_line, gpu_line]
}

/// CPU of this process and its WebKit children over `ms`, in % of one core.
async fn cpu_line(label: &str, ms: u64) -> String {
    let me = std::process::id();
    let kids = webkit_children();
    let before: Vec<u64> = std::iter::once(me)
        .chain(kids.iter().map(|k| k.0))
        .map(cpu_ticks)
        .collect();
    glib::timeout_future(Duration::from_millis(ms)).await;
    let after: Vec<u64> = std::iter::once(me)
        .chain(kids.iter().map(|k| k.0))
        .map(cpu_ticks)
        .collect();
    let pct = |i: usize| (after[i] - before[i]) as f64 / 100.0 / (ms as f64 / 1000.0) * 100.0;
    let mut s = format!("CPU {label} over {ms} ms: UI process {:.1} %", pct(0));
    for (i, (_, comm)) in kids.iter().enumerate() {
        s += &format!(", {comm} {:.1} %", pct(i + 1));
    }
    s
}

// ------------------------------------------------------------------ the run
struct Sound {
    capture: Option<Capture>,
    mic: Option<Player>,
    speaker: Option<Player>,
    buf: Vec<i16>,
}

struct Run {
    handle: Option<OverlayHandle>,
    probes: async_channel::Receiver<Probe>,
    /// Every probe received, in order.
    seen: Vec<Probe>,
    lines: Vec<String>,
    fails: Vec<String>,
    shot: Option<String>,
    rect: (i32, i32, i32, i32),
    shots: Vec<std::thread::JoinHandle<()>>,
    /// The refresh rate of the ring's display in mHz (GDK's), 0 where unknown.
    refresh_mhz: i32,
}

impl Run {
    fn line(&mut self, s: String) {
        eprintln!("{s}");
        self.lines.push(s);
    }
    fn check(&mut self, ok: bool, what: &str) {
        let s = format!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.fails.push(what.to_owned());
        }
        self.line(s);
    }
    fn send(&self, cmd: OverlayCmd) {
        if let Some(h) = &self.handle {
            h.send(cmd);
        }
    }
    fn ring(&self, bound: bool, active: bool, turn: bool, state: RingState) {
        self.send(OverlayCmd::Ring(RingInput {
            bound,
            active,
            turn,
            state,
            ..RingInput::default()
        }));
    }
    /// The next probe that `f` accepts, within `secs`; the others are kept as lines.
    async fn wait(
        &mut self,
        secs: u64,
        what: &str,
        mut f: impl FnMut(&Probe) -> bool,
    ) -> Option<Probe> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match glib::future_with_timeout(left, self.probes.recv()).await {
                Ok(Ok(p)) => {
                    self.seen.push(p.clone());
                    if f(&p) {
                        return Some(p);
                    }
                }
                _ => {
                    self.check(false, &format!("{what} within {secs} s"));
                    return None;
                }
            }
        }
    }
    /// The page's engine stats.
    async fn stats(&mut self) -> serde_json::Value {
        self.send(OverlayCmd::Eval("__kai.report()".into()));
        match self
            .wait(3, "the page's stats", |p| {
                matches!(p, Probe::Page(PageMsg::Stats(_)))
            })
            .await
        {
            Some(Probe::Page(PageMsg::Stats(v))) => v,
            _ => serde_json::Value::Null,
        }
    }
    /// An index into `seen` after every probe received so far, the queued ones too: what comes
    /// after it is new.
    fn mark(&mut self) -> usize {
        while let Ok(p) = self.probes.try_recv() {
            self.seen.push(p);
        }
        self.seen.len()
    }
    /// None of the probes received since `from` (an index into `seen`), nor any still queued, is
    /// of the kind `f` names.
    fn none_since(&mut self, from: usize, what: &str, f: impl Fn(&Probe) -> bool) {
        while let Ok(p) = self.probes.try_recv() {
            self.seen.push(p);
        }
        let hit = self.seen[from..]
            .iter()
            .find(|p| f(p))
            .map(|p| format!("{p:?}"));
        let ok = hit.is_none();
        self.check(
            ok,
            &format!(
                "{what}{}",
                hit.map(|s| format!(" (got {s})")).unwrap_or_default()
            ),
        );
    }
    /// Before a measurement: the shots taken so far are written (a screen capture loads the GPU
    /// and KWin), and the GPU had 3 s to clock down after them.
    async fn calm(&mut self) {
        while self.shots.iter().any(|h| !h.is_finished()) {
            sleep(100).await;
        }
        sleep(3000).await;
    }
    fn shot(&mut self, name: &str) {
        let Some(cmd) = self.shot.clone() else { return };
        let (x, y, w, h) = self.rect;
        let name = name.to_owned();
        self.shots.push(std::thread::spawn(move || {
            let r = std::process::Command::new(&cmd)
                .args([
                    name.as_str(),
                    &x.to_string(),
                    &y.to_string(),
                    &w.to_string(),
                    &h.to_string(),
                ])
                .status();
            if !r.as_ref().is_ok_and(|s| s.success()) {
                eprintln!("the shot {name} failed: {r:?}");
            }
        }));
    }
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// The GDK monitor the ring is put on: `output`'s, else the first.
fn ring_monitor(output: Option<&str>) -> gdk::Monitor {
    let monitors = super::outputs::gdk_monitors();
    let index = output
        .and_then(|name| {
            let outs = super::outputs::wayland_outputs().ok()?;
            let infos: Vec<_> = monitors.iter().map(|(_, i)| i.clone()).collect();
            super::placement::monitor_for(name, &outs, &infos)
        })
        .unwrap_or(0);
    monitors[index].0.clone()
}

/// The ring's rectangle in the full capture, from the placement and the output's geometry.
fn ring_rect(place: Place, output: Option<&str>) -> (i32, i32, i32, i32) {
    let m = ring_monitor(output);
    let g = m.geometry();
    let s = m.scale_factor();
    let (size, margin) = (place.size as i32, place.margin as i32);
    let e = place.anchor.edges();
    let x = if e.left {
        g.x() + margin
    } else if e.right {
        g.x() + g.width() - margin - size
    } else {
        g.x() + (g.width() - size) / 2
    };
    let y = if e.top {
        g.y() + margin
    } else if e.bottom {
        g.y() + g.height() - margin - size
    } else {
        g.y() + (g.height() - size) / 2
    };
    (x * s, y * s, size * s, size * s)
}

fn place_from_env() -> Place {
    let v = std::env::var("KAI_LIVE_RING_PLACE").unwrap_or_else(|_| "bottom-right,24,240".into());
    let p: Vec<&str> = v.split(',').collect();
    let anchor: Anchor = serde_json::from_value(serde_json::Value::from(p[0])).expect("an anchor");
    Place {
        anchor,
        margin: p[1].parse().expect("a margin"),
        size: p[2].parse().expect("a size"),
    }
}

/// The surface's size as the page saw it.
fn place_size(stats: &serde_json::Value) -> i64 {
    stats["w"].as_i64().unwrap_or(-1)
}

/// White noise at `dbfs` RMS, `secs` long: a quiet room on the microphone (deterministic).
fn noise(secs: u32, dbfs: f64) -> Vec<i16> {
    let amplitude = 10f64.powf(dbfs / 20.0) * 32768.0 * 3f64.sqrt();
    let mut x: u32 = 0x2545_f491;
    (0..secs * RATE)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let u = (x >> 8) as f64 / (1u32 << 24) as f64;
            ((u * 2.0 - 1.0) * amplitude).round() as i16
        })
        .collect()
}

/// A tone burst: `ms` of 1 kHz at -12 dBFS peak (the level's target about 0.92).
fn burst(ms: u32) -> Vec<i16> {
    (0..RATE * ms / 1000)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            (8192.0 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()).round() as i16
        })
        .collect()
}

/// The input level a burst must reach to count as heard: half of the range, -31 dBFS RMS.
const HEARD: f32 = 0.5;

/// How late the ring shows a sound on the microphone, measured with tone bursts while it is up in
/// quiet listening. Per burst, from the push into the test microphone (t0):
/// - `tap`: a reader of the input tap polled every 2 ms sees the level (the samples reached Kai's
///   capture callback);
/// - `fed`: the ring's feed read it (`Probe::Heard`);
/// - `drawn`: the page drew its first frame with it (`heard`).
///
/// The bound Kai's part (`tap` to `drawn`) must hold, and why: two feed intervals (the wait for the
/// next feed, and one feed skipped while the previous call completes) plus two of the display's
/// frame intervals (the frame the call just missed, and the one that draws it). The frame interval
/// is the display's, from its refresh rate, not the page's own: a page that draws slowly is one
/// cause of a late ring, so its rate is checked against the display's instead (review KAI-6
/// R6-7). The rig's injection (`t0` to `tap`) is held to three capture quanta: the player's cycle,
/// the test sink's monitor a period later (a driving sink's monitor lags by one), and Kai's capture
/// cycle.
async fn latency(
    r: &mut Run,
    sound: &Rc<RefCell<Sound>>,
    b: &Audio,
    taps: &crate::audio::LevelTaps,
) {
    {
        let mut s = sound.borrow_mut();
        // The microphone's player runs with nothing queued (silence), so a push plays next cycle.
        s.mic = Some(b.open_playback());
    }
    sleep(800).await;
    let quantum = sound.borrow().capture.as_ref().map_or(0, |c| c.cycle());
    let quantum_ms = quantum as f64 * 1000.0 / RATE as f64;
    let feed_ms = 1000.0 / FEED_HZ as f64;
    let display_hz = r.refresh_mhz as f64 / 1000.0;
    r.check(
        display_hz > 0.0,
        &format!(
            "latency: the ring's display reports its refresh rate ({} mHz)",
            r.refresh_mhz
        ),
    );
    // The page's own rate over 2 s, timed here between the two reports.
    let s0 = r.stats().await;
    let t = Instant::now();
    sleep(2000).await;
    let s1 = r.stats().await;
    let secs = t.elapsed().as_secs_f64();
    let frames = s1["frames"].as_f64().unwrap_or(0.0) - s0["frames"].as_f64().unwrap_or(0.0);
    let fps = frames / secs;
    let frame_ms = 1000.0 / display_hz;
    let kai_bound = 2.0 * feed_ms + 2.0 * frame_ms;
    let rig_bound = 3.0 * quantum_ms;
    r.line(format!(
        "latency: capture quantum {quantum} samples ({quantum_ms:.1} ms), feed interval \
         {feed_ms:.1} ms, the display at {display_hz:.2} Hz ({frame_ms:.1} ms a frame), the page \
         at {fps:.1} frames/s ({frames} frames in {secs:.2} s): Kai's part bound {kai_bound:.0} \
         ms, the rig's injection bound {rig_bound:.0} ms"
    ));
    r.check(quantum > 0, "latency: the capture's quantum is known");
    r.check(
        fps >= display_hz - 1.0,
        &format!(
            "latency: the page draws at the display's rate, within a frame a second ({fps:.1} of \
             {display_hz:.2} frames/s)"
        ),
    );
    let mut kai = Vec::new();
    let mut total = Vec::new();
    for n in 0..5 {
        // A reader of its own on the input tap, polled every 2 ms: when the burst reached the tap.
        let tap_at: Rc<Cell<Option<Instant>>> = Rc::default();
        let mut reader = taps.reader(crate::audio::Tap::Input);
        let seen = tap_at.clone();
        let poll = glib::timeout_add_local(Duration::from_millis(2), move || {
            if seen.get().is_none() && reader.read().level >= HEARD {
                seen.set(Some(Instant::now()));
            }
            glib::ControlFlow::Continue
        });
        r.send(OverlayCmd::WatchInput(HEARD));
        sleep(150).await;
        let t0 = Instant::now();
        if let Some(mic) = sound.borrow_mut().mic.as_mut() {
            mic.push(&format!("burst-{n}"), &burst(300));
            mic.end(&format!("burst-{n}"));
        }
        let fed = r
            .wait(6, &format!("burst {n}: the feed reads it"), |p| {
                matches!(p, Probe::Heard)
            })
            .await
            .map(|_| t0.elapsed());
        let drawn = r
            .wait(
                6,
                &format!("burst {n}: the page draws it"),
                |p| matches!(p, Probe::Page(PageMsg::Other(t)) if t.contains("\"heard\"")),
            )
            .await
            .map(|_| t0.elapsed());
        poll.remove();
        let tap = tap_at.get().map(|t| t.duration_since(t0));
        let ms = |d: Option<Duration>| d.map_or(f64::NAN, |d| d.as_secs_f64() * 1000.0);
        r.line(format!(
            "latency burst {n}: tap {:.0} ms, fed {:.0} ms, drawn {:.0} ms after the push",
            ms(tap),
            ms(fed),
            ms(drawn)
        ));
        if let (Some(tap), Some(drawn)) = (tap, drawn) {
            kai.push(ms(Some(drawn)) - ms(Some(tap)));
            total.push(ms(Some(drawn)));
            r.check(
                ms(Some(tap)) <= rig_bound,
                &format!("latency burst {n}: the rig's injection within {rig_bound:.0} ms"),
            );
        }
        sleep(700).await;
    }
    sound.borrow_mut().mic = None;
    let worst = |v: &[f64]| v.iter().copied().fold(f64::NAN, f64::max);
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    };
    r.line(format!(
        "latency: Kai's part (the tap to the page's frame) median {:.0} ms, worst {:.0} ms; from the \
         push, median {:.0} ms, worst {:.0} ms",
        median(&kai),
        worst(&kai),
        median(&total),
        worst(&total)
    ));
    r.check(
        kai.len() == 5 && worst(&kai) <= kai_bound,
        &format!(
            "latency: the ring shows a sound at most {kai_bound:.0} ms after it reached Kai's \
             capture (two feed intervals and two frames)"
        ),
    );
}

/// Kills this process's own ring web process (SIGKILL), as a crash would end it; false (and a
/// failed check) when there is none.
fn kill_web_process(r: &mut Run) -> bool {
    let web = webkit_children()
        .into_iter()
        .find(|w| w.1.starts_with("WebKitWebProces"));
    let killed = web.is_some_and(|(pid, _)| {
        // SAFETY: a plain kill(2) of a child process of this one.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
    });
    r.check(killed, "this check's own web process is killed");
    killed
}

/// The page's counters between two stats, per second over `secs`.
fn rates(before: &serde_json::Value, after: &serde_json::Value, secs: f64) -> (f64, f64) {
    let d = |k: &str| (after[k].as_f64().unwrap_or(0.0) - before[k].as_f64().unwrap_or(0.0)) / secs;
    (d("frames"), d("fed"))
}

fn ms_of(p: &Option<Probe>) -> f64 {
    match p {
        Some(Probe::Ready { ms } | Probe::Mapped { ms, .. } | Probe::Drawn { ms }) => *ms,
        _ => f64::NAN,
    }
}

#[test]
#[ignore = "needs the desktop and a private PipeWire instance (scripts/live-check-ring.sh)"]
fn live_ring_over_the_desktop() {
    if let Err(why) = guard() {
        eprintln!("skipped: {why} (run scripts/live-check-ring.sh)");
        return;
    }
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "kai=debug".into()),
        std::io::stderr,
        false,
    ));
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    let out =
        PathBuf::from(std::env::var("KAI_LIVE_OUT").unwrap_or_else(|_| "target/live-ring".into()));
    std::fs::create_dir_all(&out).unwrap();
    gtk::init().expect("GTK");
    let dir = tempfile::tempdir().unwrap();
    let (tx_a, _rx_a) = mpsc::unbounded_channel();
    let a = Audio::start(audio_settings(dir.path(), None), tx_a).expect("audio");
    let (tx_b, _rx_b) = mpsc::unbounded_channel();
    let b = Audio::start(audio_settings(dir.path(), Some(MIC_IN)), tx_b).expect("audio");
    let place = place_from_env();
    let output = std::env::var("KAI_LIVE_RING_OUTPUT")
        .ok()
        .filter(|s| !s.is_empty());
    let settings = RingSettings {
        place,
        output: output.clone(),
        over_fullscreen: true,
    };
    let taps = a.taps();
    let handle = super::start(settings, taps.clone(), Box::new(|_| {}));
    let (ptx, probes) = async_channel::unbounded();
    handle.send(OverlayCmd::Probe(ptx));
    let mut run = Run {
        handle: Some(handle),
        probes,
        seen: Vec::new(),
        lines: Vec::new(),
        fails: Vec::new(),
        shot: std::env::var("KAI_LIVE_SHOT")
            .ok()
            .filter(|s| !s.is_empty()),
        rect: ring_rect(place, output.as_deref()),
        shots: Vec::new(),
        refresh_mhz: ring_monitor(output.as_deref()).refresh_rate(),
    };
    run.line(format!(
        "placement {place:?}, ring.output {output:?}, crop rect {:?}",
        run.rect
    ));

    let sound = Rc::new(RefCell::new(Sound {
        capture: None,
        mic: None,
        speaker: None,
        buf: Vec::new(),
    }));
    // The players' queues and the capture's ring, kept moving as WP4's session would.
    let s2 = sound.clone();
    let pump = glib::timeout_add_local(Duration::from_millis(20), move || {
        let mut s = s2.borrow_mut();
        let Sound {
            capture,
            mic,
            speaker,
            buf,
        } = &mut *s;
        for p in [mic.as_mut(), speaker.as_mut()].into_iter().flatten() {
            p.pump();
        }
        if let Some(c) = capture {
            buf.clear();
            c.read(buf);
        }
        glib::ControlFlow::Continue
    });

    let done: Rc<RefCell<Option<Run>>> = Rc::default();
    let d2 = done.clone();
    let s3 = sound.clone();
    glib::MainContext::default().spawn_local(async move {
        drive(&mut run, &s3, &a, &b, &taps, &clip).await;
        if let Some(h) = run.handle.take() {
            h.quit().await;
        }
        *d2.borrow_mut() = Some(run);
        // Dropping the audio handles ends their threads.
        drop((a, b));
        gtk::main_quit();
    });
    gtk::main();
    pump.remove();
    drop(sound);

    let mut run = done.borrow_mut().take().expect("the script ran");
    for t in run.shots.drain(..) {
        let _ = t.join();
    }
    let leftover = webkit_children();
    run.check(
        leftover.is_empty(),
        &format!("no WebKit process of this process at the end: {leftover:?}"),
    );
    let report = run.lines.join("\n") + "\n";
    std::fs::write(out.join("report.txt"), &report).unwrap();
    assert!(run.fails.is_empty(), "failed: {:?}", run.fails);
}

async fn drive(
    r: &mut Run,
    sound: &Rc<RefCell<Sound>>,
    a: &Audio,
    b: &Audio,
    taps: &crate::audio::LevelTaps,
    clip: &[i16],
) {
    use RingState::*;
    let wk = webkit_children();
    r.check(
        wk.is_empty(),
        &format!("no WebKit process before the session: {wk:?}"),
    );
    r.line(pss_line("before any view"));
    r.shot("0-before");
    sleep(800).await;
    // The baseline for the all-day cost: the desktop with no view of Kai's at all.
    r.calm().await;
    for l in system_lines("no view (before any session)", 10_000).await {
        r.line(l);
    }
    let is_mapped = |p: &Probe| matches!(p, Probe::Mapped { .. });
    let is_hidden = |p: &Probe| matches!(p, Probe::Hidden);

    // ---- 1. continuous listening starts (Kai leaves standby): the view is created and shows the
    // listening state between turns
    sound.borrow_mut().capture = Some(a.open_capture());
    let t = Instant::now();
    r.ring(true, true, false, Idle);
    r.wait(5, "the view is created", |p| matches!(p, Probe::Created))
        .await;
    let ready = r
        .wait(10, "the page is ready", |p| {
            matches!(p, Probe::Ready { .. })
        })
        .await;
    let mapped = r.wait(5, "the surface maps", is_mapped).await;
    let drawn = r
        .wait(5, "the first frame", |p| matches!(p, Probe::Drawn { .. }))
        .await;
    r.line(format!(
        "listening starts: page ready {:.0} ms after the view's creation, mapped {:.0} ms and first \
         frame {:.0} ms after the show, {:.0} ms after listening started; {mapped:?}",
        ms_of(&ready),
        ms_of(&mapped),
        ms_of(&drawn),
        t.elapsed().as_secs_f64() * 1000.0
    ));
    if let Some(Probe::Mapped { layer, note, .. }) = &mapped {
        r.line(format!(
            "output note: {}",
            note.as_deref().unwrap_or("none (named explicitly)")
        ));
        r.check(
            *layer == Layer::Overlay,
            "the layer is OVERLAY (ring_over_fullscreen on)",
        );
    }
    let wk = webkit_children();
    r.line(format!("while listening: {wk:?}"));
    r.check(
        wk.iter().any(|w| w.1.starts_with("WebKitWebProces")),
        "a web process exists while Kai is active",
    );
    r.check(
        taps.wanted() == (true, true),
        "while shown, both taps are read",
    );
    sleep(700).await;
    r.shot("1-listening-quiet");
    let st = r.stats().await;
    r.check(
        st["state"] == "listening" && st["hidden"] == false && st["running"] == true,
        "quiet listening: the page shows the listening state and the engine runs",
    );

    // ---- 1b. "ring over fullscreen" toggled in the tray while the ring is up: it moves to the
    // layer at once, with no turn, on the same view (review KAI-5 R5-2)
    let from = r.seen.len();
    for (on, layer) in [(false, Layer::Top), (true, Layer::Overlay)] {
        let t = Instant::now();
        r.send(OverlayCmd::RingOverFullscreen(on));
        let mapped = r
            .wait(5, "the ring that is up is placed again", is_mapped)
            .await;
        if let Some(Probe::Mapped { layer: got, .. }) = &mapped {
            r.check(
                *got == layer,
                &format!("ring_over_fullscreen {on} while up: the layer is {layer:?} at once"),
            );
        }
        r.line(format!(
            "ring_over_fullscreen {on} while up: placed again {:.0} ms after the toggle",
            t.elapsed().as_secs_f64() * 1000.0
        ));
        sleep(400).await;
    }
    r.none_since(
        from,
        "ring_over_fullscreen while up: no hide, no new view",
        |p| matches!(p, Probe::Hidden | Probe::Destroyed | Probe::Created),
    );

    // ---- 2. the cost of listening all day: the ring up, silence on the microphone, 10 s each:
    // as Kai feeds (a frame equal to the last one is not sent), then every frame sent (as before
    // that saving), in the same run
    for (label, send_equal) in [("as Kai feeds", false), ("every frame sent", true)] {
        r.send(OverlayCmd::SendEqual(send_equal));
        sleep(300).await;
        r.calm().await;
        let s0 = r.stats().await;
        let t = Instant::now();
        let lines =
            system_lines(&format!("quiet listening, shown, silence, {label}"), 10_000).await;
        let s1 = r.stats().await;
        let (fps, fed) = rates(&s0, &s1, t.elapsed().as_secs_f64());
        for l in lines {
            r.line(l);
        }
        r.line(format!(
            "quiet listening in silence, {label}: the engine drew {fps:.1} frames/s, the page got \
             {fed:.1} feeds/s, calm {}",
            s1["calm"]
        ));
    }
    r.send(OverlayCmd::SendEqual(false));
    r.line(pss_line("quiet listening, shown"));
    // A quiet room: white noise at -60 dBFS RMS, below the level's floor (-50 dBFS) and near the
    // top bands' floor, so the levels Kai sends change from feed to feed.
    {
        let mut s = sound.borrow_mut();
        let mut mic = b.open_playback();
        mic.push("noise", &noise(16, -60.0));
        mic.end("noise");
        s.mic = Some(mic);
    }
    sleep(800).await;
    r.calm().await;
    let s0 = r.stats().await;
    let t = Instant::now();
    let lines = system_lines("quiet listening, shown, room noise at -60 dBFS", 10_000).await;
    let s1 = r.stats().await;
    let (fps, fed) = rates(&s0, &s1, t.elapsed().as_secs_f64());
    for l in lines {
        r.line(l);
    }
    r.line(format!(
        "quiet listening in a quiet room (-60 dBFS noise): the engine drew {fps:.1} frames/s, the \
         page got {fed:.1} feeds/s"
    ));
    sound.borrow_mut().mic = None;
    sleep(300).await;

    // ---- 2b. how late the ring shows the voice: tone bursts into the microphone while it is up
    latency(r, sound, b, taps).await;

    // ---- 3. sound on the microphone between turns: the listening ring follows it
    {
        let mut s = sound.borrow_mut();
        let mut mic = b.open_playback();
        mic.push("room", clip);
        mic.end("room");
        s.mic = Some(mic);
    }
    sleep(900).await;
    r.shot("2-listening-sound");
    sleep(900).await;

    // ---- 4. a turn on the ring that is up: listening, thinking, speaking, interrupted
    let from = r.seen.len();
    let t = Instant::now();
    r.ring(true, true, true, Listening);
    let mapped = r.wait(5, "the turn places the ring again", is_mapped).await;
    r.line(format!(
        "turn 1 (the ring up): placed again {:.0} ms after the turn started ({:.0} ms here); \
         {mapped:?}",
        ms_of(&mapped),
        t.elapsed().as_secs_f64() * 1000.0
    ));
    {
        let mut s = sound.borrow_mut();
        let mut mic = b.open_playback();
        mic.push("talk", clip);
        mic.end("talk");
        s.mic = Some(mic);
    }
    sleep(700).await;
    r.shot("3-turn-listening");
    sleep(1100).await;
    r.ring(true, true, true, Thinking);
    sleep(700).await;
    r.shot("4-thinking");
    sleep(900).await;
    {
        let mut s = sound.borrow_mut();
        s.mic = None;
        let mut sp = a.open_playback();
        sp.push("reply", clip);
        sp.end("reply");
        s.speaker = Some(sp);
    }
    r.ring(true, true, true, Speaking);
    sleep(700).await;
    r.shot("5-speaking");
    let cpu = cpu_line("speaking (shown, fed at 30 Hz)", 1000).await;
    r.line(cpu);
    r.line(pss_line("speaking"));
    let st = r.stats().await;
    r.line(format!("speaking stats: {st}"));
    r.check(
        st["hidden"] == false && st["running"] == true && st["state"] == "speaking",
        "speaking: the engine runs",
    );
    // The feed keeps flowing with its back-pressure: each call completes before the next.
    sleep(1000).await;
    let st2 = r.stats().await;
    let fed = st2["fed"].as_u64().unwrap_or(0) - st["fed"].as_u64().unwrap_or(0);
    r.line(format!("fed in 1 s while speaking: {fed} (30 Hz)"));
    r.check(
        fed >= 20,
        "speaking: the feed keeps up (at least 20 of 30 feeds a second)",
    );
    let size = place_size(&st);
    r.check(
        st["canvas"] == serde_json::json!([0, 0, size, size])
            && st["scroll"] == serde_json::json!([size, size]),
        "the canvas fills the surface exactly (ring.css applied: no margin, no overflow)",
    );
    r.ring(true, true, true, Interrupted);
    sleep(120).await;
    r.shot("6-interrupted");
    sleep(500).await;
    sound.borrow_mut().speaker = None;

    // ---- 5. the turn ends: back to listening, the ring stays up
    r.ring(true, true, false, Idle);
    sleep(1000).await;
    r.none_since(
        from,
        "turn 1: the ring stayed up from its start to a second after its end (no hide, no new view)",
        |p| matches!(p, Probe::Hidden | Probe::Destroyed | Probe::Created),
    );
    let st = r.stats().await;
    r.check(
        st["state"] == "listening" && st["hidden"] == false && st["running"] == true,
        "after the turn: the listening state, drawn",
    );
    r.check(
        taps.wanted() == (true, true),
        "after the turn: the level readers stay",
    );
    r.shot("7-after-turn-listening");
    sleep(900).await;

    // ---- 6. mute: standby; the ring eases to idle and hides, the view stays
    let from = r.seen.len();
    let t = Instant::now();
    r.send(OverlayCmd::Ring(RingInput {
        bound: true,
        muted: true,
        ..RingInput::default()
    }));
    r.wait(4, "the surface hides after mute", is_hidden).await;
    r.line(format!(
        "mute -> hidden after the idle state was drawn: {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    ));
    r.check(
        taps.wanted() == (false, false),
        "muted: the level readers are gone",
    );
    sleep(300).await;
    r.shot("8-muted");
    r.calm().await;
    let s0 = r.stats().await;
    for l in system_lines("standby, muted (bound, hidden view)", 10_000).await {
        r.line(l);
    }
    let s1 = r.stats().await;
    r.check(
        s1["hidden"] == true && s0["frames"] == s1["frames"] && s1["running"] == false,
        "muted: the page is hidden and the engine drew no frame in 10 s",
    );
    // ---- 7. unmuted: active again, on the same view
    let t = Instant::now();
    r.ring(true, true, false, Idle);
    r.wait(5, "the surface maps after unmute", is_mapped).await;
    let drawn = r
        .wait(5, "the first frame after unmute", |p| {
            matches!(p, Probe::Drawn { .. })
        })
        .await;
    r.line(format!(
        "unmute -> first frame {:.0} ms after the show ({:.0} ms here)",
        ms_of(&drawn),
        t.elapsed().as_secs_f64() * 1000.0
    ));
    r.none_since(from, "mute and unmute keep the view", |p| {
        matches!(p, Probe::Destroyed | Probe::Created)
    });
    sleep(600).await;
    // Shown again in silence, after the hidden window: the cost's A/B/A.
    r.calm().await;
    for l in system_lines(
        "quiet listening, shown again, silence, as Kai feeds",
        10_000,
    )
    .await
    {
        r.line(l);
    }

    // ---- 7b. this check's own web process is killed while Kai is active: a new view at once; a
    // second kill waits for the next turn (no respawn loop), which brings the ring back
    let t = Instant::now();
    if kill_web_process(r) {
        r.wait(5, "the killed view is destroyed", |p| {
            matches!(p, Probe::Destroyed)
        })
        .await;
        r.wait(5, "a new view is created at once", |p| {
            matches!(p, Probe::Created)
        })
        .await;
        r.wait(5, "the new view maps", is_mapped).await;
        r.wait(5, "the new view's first frame", |p| {
            matches!(p, Probe::Drawn { .. })
        })
        .await;
        r.line(format!(
            "crash while listening -> the ring back {:.0} ms after the kill",
            t.elapsed().as_secs_f64() * 1000.0
        ));
        let st = r.stats().await;
        r.check(
            st["state"] == "listening" && st["running"] == true,
            "after a crash: the new view shows the listening state",
        );
    }
    // A second kill while Kai stays active: no new view at once (no loop), the tray hears the ring
    // is down, and the retry timer brings it back with no turn; a crash of the retried view waits
    // for the next retry, twice as long (review KAI-5 R5-3). The first wait is shortened here.
    let first = Duration::from_secs(3);
    r.send(OverlayCmd::RetryFirst(first));
    let from = r.seen.len();
    if kill_web_process(r) {
        let t = Instant::now();
        r.wait(5, "the second killed view is destroyed", |p| {
            matches!(p, Probe::Destroyed)
        })
        .await;
        let note = r
            .wait(2, "the tray hears the ring is down", |p| {
                matches!(p, Probe::Note(Some(_)))
            })
            .await;
        r.line(format!("retry: the tray's line: {note:?}"));
        sleep(1500).await;
        r.none_since(from, "a second crash: no new view at once", |p| {
            matches!(p, Probe::Created)
        });
        r.wait(6, "retry 1 creates a view with no turn", |p| {
            matches!(p, Probe::Created)
        })
        .await;
        let created_s = t.elapsed().as_secs_f64();
        r.wait(5, "retry 1's view maps", is_mapped).await;
        r.wait(5, "retry 1's view's first frame", |p| {
            matches!(p, Probe::Drawn { .. })
        })
        .await;
        let back = r.seen[from..]
            .iter()
            .any(|p| matches!(p, Probe::Note(None)));
        r.check(back, "retry 1: the tray hears the ring is no longer down");
        r.line(format!(
            "retry 1: a view {created_s:.1} s after the second kill (first wait {} s), mapped {:.1} \
             s after it",
            first.as_secs(),
            t.elapsed().as_secs_f64()
        ));
        let st = r.stats().await;
        r.check(
            st["state"] == "listening" && st["running"] == true,
            "retry 1: the ring is back in the listening state",
        );
        sleep(300).await;
        r.shot("7b-after-retry");
        // The retried view ends too: it waits for retry 2.
        let from = r.seen.len();
        if kill_web_process(r) {
            let t = Instant::now();
            r.wait(5, "the retried view is destroyed", |p| {
                matches!(p, Probe::Destroyed)
            })
            .await;
            sleep(1500).await;
            r.none_since(from, "the retried view's crash: no new view at once", |p| {
                matches!(p, Probe::Created)
            });
            r.wait(10, "retry 2 creates a view", |p| {
                matches!(p, Probe::Created)
            })
            .await;
            let created_s = t.elapsed().as_secs_f64();
            r.wait(5, "retry 2's view maps", is_mapped).await;
            r.line(format!(
                "retry 2: a view {created_s:.1} s after the retried view's kill (its wait {} s from \
                 retry 1)",
                2 * first.as_secs()
            ));
        }
        // The next turn ends the wait; a turn on a ring that is up places it again.
        let t = Instant::now();
        r.ring(true, true, true, Listening);
        r.wait(5, "the next turn places the ring", is_mapped).await;
        r.line(format!(
            "second crash -> the next turn's ring placed {:.0} ms after its start",
            t.elapsed().as_secs_f64() * 1000.0
        ));
        r.ring(true, true, false, Idle);
        sleep(600).await;
    }
    r.send(OverlayCmd::RetryFirst(RETRY_FIRST));

    // ---- 8. the session ends while the ring is up: it eases to idle, then the view goes
    let before = webkit_children();
    let t = Instant::now();
    r.ring(false, false, false, Idle);
    r.wait(4, "the surface hides at the session's end", is_hidden)
        .await;
    let hidden_ms = t.elapsed().as_secs_f64() * 1000.0;
    r.wait(2, "the view is destroyed", |p| {
        matches!(p, Probe::Destroyed)
    })
    .await;
    r.line(format!(
        "session end -> hidden after the idle state was drawn: {hidden_ms:.0} ms, then destroyed"
    ));
    sound.borrow_mut().capture = None;
    sleep(500).await;
    let after_half = webkit_children();
    sleep(1000).await;
    let after = webkit_children();
    r.line(format!(
        "session end: WebKit processes before {before:?}, +0.5 s {after_half:?}, +1.5 s {after:?}"
    ));
    r.check(
        after.is_empty(),
        "no web process left 1.5 s after the session ended",
    );
    r.line(pss_line("after the session"));
    r.line(format!(
        "the UI process's largest mappings now: {}",
        top_mappings(8)
    ));

    // ---- 9. push-to-talk (the owner's decision, 2026-10-08: Kai listens only while the key is
    // held, standby between presses), here as a press with no view yet (hold-to-talk with
    // listening off; push-to-talk mode with listening on gives the ring the same input): the view
    // is created at the press, and the ring shows from the press
    sound.borrow_mut().capture = Some(a.open_capture());
    let t = Instant::now();
    r.ring(true, true, true, Listening);
    let ready = r
        .wait(10, "the cold page is ready", |p| {
            matches!(p, Probe::Ready { .. })
        })
        .await;
    let drawn = r
        .wait(5, "the cold first frame", |p| {
            matches!(p, Probe::Drawn { .. })
        })
        .await;
    let total = t.elapsed().as_secs_f64() * 1000.0;
    r.line(format!(
        "push-to-talk press (view created at the press): page ready {:.0} ms after creation, first frame {total:.0} ms after the press ({:.0} ms after the map request)",
        ms_of(&ready),
        ms_of(&drawn),
    ));
    sleep(600).await;

    let st = r.stats().await;
    r.line(format!("cold view stats: {st}"));

    // ---- 10. reduced motion follows gtk-enable-animations
    let gs = gtk::Settings::default().expect("GTK settings");
    let animations = gs.is_gtk_enable_animations();
    gs.set_gtk_enable_animations(false);
    sleep(700).await;
    let st = r.stats().await;
    r.line(format!("reduced motion stats: {st}"));
    r.check(
        st["reduced"] == true && st["calm"] == true,
        "reduced motion: re-mounted calm",
    );
    let size = place_size(&st);
    r.check(
        st["canvas"] == serde_json::json!([0, 0, size, size])
            && st["scroll"] == serde_json::json!([size, size]),
        "reduced motion: the new canvas fills the surface exactly",
    );
    r.shot("9-reduced-motion");
    sleep(800).await;
    gs.set_gtk_enable_animations(animations);
    sleep(200).await;

    // ---- 11. the press's turn ends: standby between presses, so the ring eases out, hides, and
    // stays hidden
    let t = Instant::now();
    r.ring(true, false, false, Idle);
    r.wait(4, "the surface hides after the press's turn", is_hidden)
        .await;
    r.line(format!(
        "push-to-talk: turn end -> hidden after the idle state was drawn: {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0
    ));
    r.check(
        taps.wanted() == (false, false),
        "push-to-talk between presses: the level readers are gone",
    );
    let from = r.seen.len();
    let cpu = cpu_line("push-to-talk between presses (bound, hidden view)", 2000).await;
    r.line(cpu);
    r.none_since(
        from,
        "push-to-talk between presses: nothing mapped or drawn for 2 s",
        |p| matches!(p, Probe::Mapped { .. } | Probe::Page(PageMsg::Drawn)),
    );

    // ---- 12. the next press, with ring_over_fullscreen off: the ring shows again, on TOP
    r.send(OverlayCmd::RingOverFullscreen(false));
    r.ring(true, true, true, Speaking);
    let mapped = r.wait(5, "the surface maps on TOP", is_mapped).await;
    if let Some(Probe::Mapped { layer, .. }) = &mapped {
        r.check(
            *layer == Layer::Top,
            "ring_over_fullscreen off: the layer is TOP",
        );
    }
    sleep(700).await;
    r.ring(false, false, false, Idle);
    r.wait(4, "the view is destroyed", |p| {
        matches!(p, Probe::Destroyed)
    })
    .await;
    sound.borrow_mut().capture = None;
    sleep(1500).await;
    let after = webkit_children();
    r.check(
        after.is_empty(),
        &format!("no web process left after the second session: {after:?}"),
    );
    r.send(OverlayCmd::RingOverFullscreen(true));
    r.shot("10-after-session");
    sleep(1200).await;
}

/// The page in a view that is never mapped (nothing shows): the stylesheet applies and the CSP
/// holds (an inline script and an inline style are refused). Run by scripts/live-check-ring.sh.
#[test]
#[ignore = "needs a Wayland display (maps nothing; scripts/live-check-ring.sh)"]
fn live_page_loads_under_its_csp() {
    gtk::init().expect("GTK");
    let (tx, rx) = async_channel::unbounded();
    let view = super::view::RingView::create(
        1,
        Place {
            anchor: Anchor::BottomRight,
            margin: 24,
            size: 240,
        },
        false,
        tx,
    );
    let diag = glib::MainContext::default().block_on(async {
        let mut ready = false;
        let mut diag = None;
        let mut csp = Vec::new();
        while diag.is_none() {
            match glib::future_with_timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(super::Event::Page { msg, .. })) => match msg {
                    PageMsg::Ready if !ready => {
                        ready = true;
                        view.eval(
                            r#"(() => {
  const s = document.createElement("script");
  s.textContent = "window.__inline = true";
  document.head.appendChild(s);
  const d = document.createElement("div");
  d.setAttribute("style", "color: rgb(1, 2, 3)");
  document.body.appendChild(d);
  setTimeout(() => window.webkit.messageHandlers.kai.postMessage(JSON.stringify({
    ev: "diag",
    rules: document.querySelector("link").sheet.cssRules.length,
    margin: getComputedStyle(document.body).marginTop,
    overflow: getComputedStyle(document.documentElement).overflow,
    inline_script_ran: window.__inline === true,
    inline_style_applied: getComputedStyle(d).color === "rgb(1, 2, 3)",
  })), 100);
})()"#,
                        );
                    }
                    PageMsg::Csp { directive, .. } => csp.push(directive),
                    PageMsg::Other(t) if t.contains("\"diag\"") => {
                        diag = Some(serde_json::from_str::<serde_json::Value>(&t).unwrap())
                    }
                    other => eprintln!("page: {other:?}"),
                },
                Ok(Ok(_)) => {}
                _ => panic!("the page did not answer"),
            }
        }
        (diag.unwrap(), csp)
    });
    view.destroy();
    eprintln!("page: {}; CSP reports: {:?}", diag.0, diag.1);
    let d = diag.0;
    assert_eq!(d["rules"], 2, "ring.css applies");
    assert_eq!(
        (d["margin"].as_str(), d["overflow"].as_str()),
        (Some("0px"), Some("hidden"))
    );
    assert_eq!(
        d["inline_script_ran"], false,
        "script-src 'self' refuses inline script"
    );
    assert_eq!(
        d["inline_style_applied"], false,
        "style-src 'self' refuses inline style"
    );
    assert!(
        diag.1.iter().any(|d| d.starts_with("script-src")),
        "{:?}",
        diag.1
    );
}
