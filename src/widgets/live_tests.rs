//! The widgets' live check (`just live-check-widgets`: scripts/live-check-widgets.sh runs this in a
//! nested KWin with two virtual outputs, scripts/nested-kwin.sh: its own bus and XDG directories,
//! nothing on the session's screen, inside a private PipeWire instance, scripts/pw-private.sh:
//! nothing audible). Ignored by default; it refuses to run unless `KAI_LIVE_NESTED=1`, the display,
//! the bus and the runtime directory are that nested KWin's, and `PIPEWIRE_RUNTIME_DIR` is the
//! private instance's under the report directory.
//!
//! The widgets run as in Kai, driven through their commands; KWin itself is asked where each
//! surface is (a KWin script lists this process's windows). In order:
//! 1. **The sandbox**: an HTML document that tries an `http:` image, fetch and a WebSocket (to a
//!    loopback server that counts connections), preconnect, dns-prefetch, prefetch, preload,
//!    modulepreload, speculation rules, a stylesheet, CSS `@import`, `@font-face`, a CSS
//!    background, EventSource, `sendBeacon`, a meta refresh and a form submit, a `file:` image, a
//!    `kai:` load, a popup and a navigation to another widget's document, with `data:` and `blob:`
//!    images as the positive control, and reports what it saw by navigating to
//!    `kai-sandbox://widget/report?…` (refused, and seen here, which also shows its inline script
//!    ran); nothing of it arrives at the loopback server. Kai's host script and message handler
//!    are not visible from the page's main world.
//!    **1b** ([`hardening`]): no sound (the APIs, and no PipeWire client), no dialog, no WebGL, no
//!    fullscreen.
//! 2. **PSS** of the first widget and a second one (K1's figures: +66 to 72 MB, +15 MB).
//! 3. **Every position on both outputs**, checked against KWin's geometry; crops of each kind
//!    (spectacle's capture of the nested outputs, cut to each surface), and each output whole
//!    (`shots/full-kinds-Virtual-0.png` holds the nine positions, `full-app*-…` the app views).
//! 4. **`overlay_list`'s lines**, a size fitted to the output, **expiry**, a widget whose output
//!    goes away.
//! 5. **No web process** once the last widget is hidden.
//! 6. **A widget's crash leaves the ring's**: the ring's view up, the widgets' web process ended.
//!    **6b** ([`hardening`]): a script that never yields, before and after its document's load.
//!    **5d, 5e** ([`apps`], WP12): the MCP Apps bridge's postMessage path into Kai's isolated
//!    world and back, only the placed frame answered; `overlay_app` against a dev lmgw with the
//!    fixture.
//! 7. **The log** holds no content of a widget, at trace.
//!
//! Env: `KAI_LIVE_OUT` the report directory (crops in `<out>/shots/`); `KAI_LIVE_SECTIONS` a comma
//! list of sections to run (`sandbox,sound,memory,positions,list,last,unresponsive,crash,kinds,apps`; all by
//! default). [`live_widgets_input_nested`] is the input-region run (`hardening::input`).

mod apps;
mod hardening;
mod kinds;
mod readme;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gtk::glib;
use serde_json::Value;
use tokio::sync::oneshot;

use super::gtk_host::{Look, Probe, ShowRequest, Shown, WidgetsCmd, WidgetsHandle, start};
use super::kinds::{Body, Kind};
use crate::audio::LevelTaps;
use crate::logging::test_support::Capture;
use crate::overlay::placement::Anchor;
use crate::overlay::{self, OverlayCmd, Place, RingInput, RingSettings, RingState};

const OUTPUTS: [&str; 2] = ["Virtual-0", "Virtual-1"];
/// The nested outputs' size (scripts/nested-kwin.sh), side by side.
const OUT_W: i32 = 1280;
const OUT_H: i32 = 720;

const SECRET_TEXT: &str = "WIDGET-TEXT-5f2e-never-logged";
const SECRET_HTML: &str = "WIDGET-HTML-81c3-never-logged";

/// A 1×1 PNG.
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

pub(crate) fn guard() -> Result<(), String> {
    if std::env::var("KAI_LIVE_NESTED").as_deref() != Ok("1") {
        return Err("KAI_LIVE_NESTED is not 1".into());
    }
    let log = std::env::var_os("KAI_NESTED_KWIN_LOG").ok_or("no KAI_NESTED_KWIN_LOG")?;
    let dir = Path::new(&log)
        .parent()
        .ok_or("KAI_NESTED_KWIN_LOG names no directory")?;
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    if Path::new(&display) != dir.join("run/kai-nested") {
        return Err("WAYLAND_DISPLAY is not the nested KWin's socket".into());
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let private = std::fs::read_to_string(dir.join("bus")).unwrap_or_default();
    if bus.is_empty()
        || bus != private.trim()
        || !bus.contains(&format!("path={}/run/", dir.display()))
    {
        return Err("not the nested KWin's private bus".into());
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    if Path::new(&runtime) != dir.join("run") {
        return Err("XDG_RUNTIME_DIR is not the nested one".into());
    }
    // The private PipeWire (scripts/pw-private.sh) under the report directory, and no other.
    let out = std::env::var("KAI_LIVE_OUT").unwrap_or_default();
    let pw = std::env::var("PIPEWIRE_RUNTIME_DIR").unwrap_or_default();
    if out.is_empty() || pw.is_empty() || !Path::new(&pw).starts_with(&out) {
        return Err("PIPEWIRE_RUNTIME_DIR is not a private instance's under KAI_LIVE_OUT".into());
    }
    if !Path::new(&pw).join("pipewire-0").exists() {
        return Err("no private PipeWire runs in PIPEWIRE_RUNTIME_DIR".into());
    }
    if std::env::var_os("PULSE_SERVER").is_some() {
        return Err("PULSE_SERVER is set: it could name the session's sound server".into());
    }
    if crate::nvidia::nvidia_present() && std::env::var_os(crate::nvidia::VAR).is_none() {
        return Err(format!(
            "NVIDIA is present: {} must be set",
            crate::nvidia::VAR
        ));
    }
    Ok(())
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
    out.sort();
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

/// The UI process's and every WebKit child's PSS, and their sum.
fn pss() -> (String, f64) {
    let me = pss_mb(std::process::id());
    let mut total = me;
    let mut s = format!("UI process {me:.0} MB");
    for (pid, comm) in webkit_children() {
        let mb = pss_mb(pid);
        total += mb;
        s += &format!(", {comm} {mb:.0} MB");
    }
    (s, total)
}

// ------------------------------------------------------------------ KWin

fn busctl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("busctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("busctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "busctl {args:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// One of this process's windows as KWin shows it.
#[derive(Debug, Clone)]
pub(crate) struct KWinWindow {
    /// Its caption (`Kai: confirm a tool call` for K15's dialog; a layer surface's may be empty).
    pub(crate) caption: String,
    pub(crate) output: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
}

static QUERY: AtomicU32 = AtomicU32::new(0);

/// This process's windows KWin shows, from a script run in the nested KWin (it prints to KWin's
/// log).
pub(crate) async fn kwin_windows() -> Result<Vec<KWinWindow>, String> {
    let n = QUERY.fetch_add(1, Ordering::Relaxed);
    let tag = format!("KAI-WIDGETS-{n}");
    let log = PathBuf::from(std::env::var("KAI_NESTED_KWIN_LOG").unwrap());
    let script = log.with_file_name(format!("query-{n}.js"));
    let pid = std::process::id();
    std::fs::write(
        &script,
        format!(
            r#"for (const w of workspace.stackingOrder) {{
    if (w.pid !== {pid} || w.deleted) continue;
    const g = w.frameGeometry;
    print("{tag} output=" + (w.output ? w.output.name : "none") + " g=" + g.x + "," + g.y + "," + g.width + "," + g.height + " c=" + w.caption);
}}
print("{tag} end");
"#
        ),
    )
    .map_err(|e| e.to_string())?;
    let name = format!("kai-widgets-{n}");
    let loaded = busctl(&[
        "call",
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
        "loadScript",
        "ss",
        script.to_str().unwrap(),
        &name,
    ])?;
    let id = loaded.trim_start_matches("i ").to_owned();
    busctl(&[
        "call",
        "org.kde.KWin",
        &format!("/Scripting/Script{id}"),
        "org.kde.kwin.Script",
        "run",
    ])?;
    let end = Instant::now() + Duration::from_secs(3);
    let text = loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains(&format!("{tag} end")) || Instant::now() > end {
            break text;
        }
        sleep(50).await;
    };
    let _ = busctl(&[
        "call",
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
        "unloadScript",
        "s",
        &name,
    ]);
    let _ = std::fs::remove_file(&script);
    if !text.contains(&format!("{tag} end")) {
        return Err(format!("KWin's script {tag} printed nothing within 3 s"));
    }
    let marker = format!("{tag} output=");
    Ok(text
        .lines()
        .filter_map(|l| l.split_once(marker.as_str())?.1.split_once(" g="))
        .filter_map(|(output, rest)| {
            let (g, caption) = rest.split_once(" c=").unwrap_or((rest, ""));
            let v: Vec<i32> = g
                .split(',')
                .filter_map(|x| x.trim().parse::<f64>().ok())
                .map(|x| x.round() as i32)
                .collect();
            (v.len() == 4).then(|| KWinWindow {
                caption: caption.trim_end().to_owned(),
                output: output.to_owned(),
                x: v[0],
                y: v[1],
                w: v[2],
                h: v[3],
            })
        })
        .collect())
}

impl KWinWindow {
    fn intersects(&self, o: &KWinWindow) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// The caption of K15's dialog as `crate::ui` builds it.
pub(crate) const DIALOG_CAPTION: &str = "Kai: confirm a tool call";

/// While Kai's dialog is open, no app view's surface may cover any of it (§10.1, the owner's
/// decision of 2026-10-09: a held app view is its strip alone, away from the dialog): every surface
/// of this process but the dialog and the other kinds' widgets (`others`, by their mapped size; they
/// are not held) must miss the dialog, and there must be one. Polls up to 3 s (the shrink is a
/// configure and a commit away); the windows as KWin last showed them.
pub(crate) async fn nothing_over_the_dialog(others: &[(i32, i32)]) -> (bool, Vec<KWinWindow>) {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let ws = kwin_windows().await.unwrap_or_default();
        let dialog = ws.iter().find(|w| w.caption.starts_with(DIALOG_CAPTION));
        let views: Vec<&KWinWindow> = ws
            .iter()
            .filter(|w| !w.caption.starts_with(DIALOG_CAPTION) && !others.contains(&(w.w, w.h)))
            .collect();
        let clear =
            !views.is_empty() && dialog.is_some_and(|d| views.iter().all(|w| !w.intersects(d)));
        if clear || Instant::now() > end {
            return (clear, ws);
        }
        sleep(150).await;
    }
}

/// Where a `w`×`h` surface anchored at `a`, `margin` px in, sits on output `index`.
fn expected(a: Anchor, margin: i32, index: usize, w: i32, h: i32) -> (i32, i32) {
    let e = a.edges();
    let ox = index as i32 * OUT_W;
    let x = if e.left {
        margin
    } else if e.right {
        OUT_W - margin - w
    } else {
        (OUT_W - w) / 2
    };
    let y = if e.top {
        margin
    } else if e.bottom {
        OUT_H - margin - h
    } else {
        (OUT_H - h) / 2
    };
    (ox + x, y)
}

// ------------------------------------------------------------------ the loopback server

/// Counts the connections it accepts until dropped.
struct Loopback {
    port: u16,
    count: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Loopback {
    fn start() -> Loopback {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let (count, stop) = (
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (c, s) = (count.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                match l.accept() {
                    Ok((mut conn, _)) => {
                        c.fetch_add(1, Ordering::Relaxed);
                        let _ = conn.set_read_timeout(Some(Duration::from_millis(200)));
                        let mut buf = [0u8; 1024];
                        let _ = conn.read(&mut buf);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Loopback {
            port,
            count,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------ the run

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

struct Run {
    out: PathBuf,
    report: Vec<String>,
    failed: usize,
    handle: Option<WidgetsHandle>,
    probes: async_channel::Receiver<Probe>,
    seen: Vec<Probe>,
}

impl Run {
    fn line(&mut self, s: String) {
        println!("{s}");
        self.report.push(s);
    }

    fn check(&mut self, ok: bool, what: &str) {
        if !ok {
            self.failed += 1;
        }
        self.line(format!("{} {what}", if ok { "ok  " } else { "FAIL" }));
    }

    fn send(&self, cmd: WidgetsCmd) {
        self.handle.as_ref().unwrap().send(cmd);
    }

    /// The first probe (buffered or new) that `want` holds of, within `secs`.
    async fn until(&mut self, secs: u64, want: impl Fn(&Probe) -> bool) -> Option<Probe> {
        if let Some(i) = self.seen.iter().position(&want) {
            return Some(self.seen.remove(i));
        }
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match glib::future_with_timeout(left, self.probes.recv()).await {
                Ok(Ok(p)) if want(&p) => return Some(p),
                Ok(Ok(p)) => self.seen.push(p),
                _ => return None,
            }
        }
    }

    async fn show(&mut self, kind: Kind, body: Body, look: Look) -> Result<Shown, String> {
        self.show_within(20, kind, body, look).await
    }

    async fn show_within(
        &mut self,
        secs: u64,
        kind: Kind,
        body: Body,
        look: Look,
    ) -> Result<Shown, String> {
        let (reply, answer) = oneshot::channel();
        self.send(WidgetsCmd::Show(ShowRequest {
            kind,
            body,
            look,
            reply,
        }));
        match glib::future_with_timeout(Duration::from_secs(secs), answer).await {
            Ok(Ok(r)) => r,
            _ => Err(format!("no answer within {secs} s")),
        }
    }

    async fn hide(&mut self, id: Option<u64>) -> Result<Vec<u64>, String> {
        let (reply, answer) = oneshot::channel();
        self.send(WidgetsCmd::Hide {
            id,
            reply: Some(reply),
        });
        answer.await.unwrap_or_else(|_| Err("no answer".into()))
    }

    async fn list(&mut self) -> Vec<String> {
        let (reply, answer) = oneshot::channel();
        self.send(WidgetsCmd::List { reply });
        answer.await.unwrap_or_default()
    }

    async fn eval_main(&mut self, id: u64, js: &str) -> Result<String, String> {
        let (reply, answer) = oneshot::channel();
        self.send(WidgetsCmd::EvalMain {
            id,
            js: js.into(),
            reply,
        });
        answer.await.unwrap_or_else(|_| Err("no answer".into()))
    }

    /// Hides everything and waits for the widgets' context to go.
    async fn clear(&mut self, what: &str) {
        let _ = self.hide(None).await;
        let gone = self
            .until(5, |p| matches!(p, Probe::ContextGone))
            .await
            .is_some();
        self.check(
            gone,
            &format!("{what}: every widget hidden, the context dropped"),
        );
        self.seen.clear();
    }
}

fn look(position: Anchor, output: Option<&str>, width: u32, height: u32) -> Look {
    Look {
        position,
        margin: 24,
        width,
        height,
        timeout: None,
        interactive: false,
        output: output.map(str::to_owned),
        show_timeout: Duration::from_secs(10),
        unresponsive: Duration::from_secs(3),
    }
}

/// The JSON a document's report navigation carries.
fn report_json(uri: &str) -> Value {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.query().map(str::to_owned))
        .and_then(|q| {
            form_urlencoded::parse(format!("r={q}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
        })
        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
        .unwrap_or(Value::Null)
}

/// The sandbox probe: what the caller's document can reach, reported by a refused navigation.
fn probe_document(port: u16, file_png: &Path) -> String {
    format!(
        r#"<!doctype html><meta charset="utf-8">
<meta http-equiv="refresh" content="1;url=http://127.0.0.1:{port}/refresh">
<link rel="preconnect" href="http://127.0.0.1:{port}">
<link rel="dns-prefetch" href="http://127.0.0.1:{port}">
<link rel="prefetch" href="http://127.0.0.1:{port}/prefetch">
<link rel="preload" as="image" href="http://127.0.0.1:{port}/preload.png">
<link rel="preload" as="fetch" crossorigin href="http://127.0.0.1:{port}/preload">
<link rel="modulepreload" href="http://127.0.0.1:{port}/m.js">
<link rel="stylesheet" href="http://127.0.0.1:{port}/s.css">
<script type="speculationrules">{{"prefetch": [{{"source": "list", "urls": ["http://127.0.0.1:{port}/spec"]}}]}}</script>
<style>@import url("http://127.0.0.1:{port}/import.css");
@font-face {{ font-family: KaiProbe; src: url("http://127.0.0.1:{port}/font.woff"); }}
body {{ margin: 0; background: rgb(18, 52, 86); color: #eff0f1; font: 16px sans-serif; }}
.box {{ padding: 16px; }}
.font {{ font-family: KaiProbe; }}
.bg {{ width: 4px; height: 4px; background-image: url("http://127.0.0.1:{port}/bg.png"); }}</style>
<body><div class="box">sandbox probe {SECRET_HTML}</div><div class="font">f</div><div class="bg"></div>
<form id="form" action="http://127.0.0.1:{port}/form" method="post"><input name="x" value="y"></form>
<img id="http" src="http://127.0.0.1:{port}/i.png">
<img id="file" src="file://{file}">
<img id="kai" src="kai://app/ring.html">
<img id="data" src="data:image/png;base64,{PNG_B64}">
<img id="blob">
<script>
const r = {{ inline: true }};
const img = (id) => new Promise((done) => {{
  const i = document.getElementById(id);
  if (i.complete && i.src) return done(i.naturalWidth > 0);
  i.onload = () => done(i.naturalWidth > 0);
  i.onerror = () => done(false);
}});
const bytes = Uint8Array.from(atob("{PNG_B64}"), (c) => c.charCodeAt(0));
document.getElementById("blob").src = URL.createObjectURL(new Blob([bytes], {{ type: "image/png" }}));
const fetched = fetch("http://127.0.0.1:{port}/f").then(() => "loaded", (e) => "refused");
const ws = new Promise((done) => {{
  try {{
    const s = new WebSocket("ws://127.0.0.1:{port}/w");
    s.onopen = () => done("open");
    s.onerror = () => done("refused");
  }} catch (e) {{ done("refused"); }}
}});
(async () => {{
  for (const id of ["http", "file", "kai", "data", "blob"]) r[id] = await img(id);
  r.fetch = await fetched;
  r.ws = await ws;
  try {{
    const es = new EventSource("http://127.0.0.1:{port}/events");
    r.eventsource = await new Promise((done) => {{
      es.onopen = () => done("open");
      es.onerror = () => done("refused");
    }});
    es.close();
  }} catch (e) {{ r.eventsource = "threw " + e.name; }}
  try {{ r.beacon = String(navigator.sendBeacon("http://127.0.0.1:{port}/beacon", "x")); }}
  catch (e) {{ r.beacon = "threw " + e.name; }}
  try {{ document.getElementById("form").submit(); r.form = "submitted"; }}
  catch (e) {{ r.form = "threw " + e.name; }}
  try {{ r.parent = typeof parent.document.body; }} catch (e) {{ r.parent = "blocked"; }}
  r.webkit = typeof window.webkit;
  r.origin = String(self.origin);
  r.style = getComputedStyle(document.body).backgroundColor;
  try {{ r.popup = window.open("http://127.0.0.1:{port}/p") === null ? "null" : "opened"; }}
  catch (e) {{ r.popup = "refused"; }}
  location.href = "kai-sandbox://widget/frame/999999";
  setTimeout(() => {{ location.href = "http://127.0.0.1:{port}/nav"; }}, 200);
  setTimeout(() => {{
    location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(r));
  }}, 1500);
}})();
</script>"#,
        file = file_png.display()
    )
}

async fn sandbox(r: &mut Run) {
    r.line("== 1. the sandbox".into());
    let server = Loopback::start();
    let file_png = r.out.join("file-probe.png");
    std::fs::write(&file_png, base64_free::decode(PNG_B64)).unwrap();
    let t = Instant::now();
    let shown = r
        .show(
            Kind::Html,
            Body::Document(probe_document(server.port, &file_png)),
            look(Anchor::TopLeft, Some(OUTPUTS[0]), 480, 320),
        )
        .await;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let created = r.until(1, |p| matches!(p, Probe::Created(1))).await;
    let ready = r
        .until(1, |p| {
            matches!(p, Probe::Page(1, super::sandbox::PageMsg::Ready))
        })
        .await;
    r.check(
        created.is_some() && ready.is_some(),
        "its view was created, and the host page said ready (Kai's script runs in its world)",
    );
    let id = match shown {
        Ok(s) => {
            r.check(
                s.id == 1,
                &format!(
                    "the first widget is id 1 (mapped {ms:.0} ms after the call, a cold start)"
                ),
            );
            s.id
        }
        Err(e) => {
            r.check(false, &format!("the probe document shows: {e}"));
            return;
        }
    };
    let other = r
        .until(10, |p| {
            matches!(p, Probe::Refused { uri, .. } if uri.starts_with("kai-sandbox://widget/frame/999999"))
        })
        .await;
    r.check(
        matches!(&other, Some(Probe::Refused { id: 1, refused, .. }) if refused.why == "not this widget's page"),
        &format!("a navigation to another widget's document is refused ({other:?})"),
    );
    let report = r
        .until(10, |p| {
            matches!(p, Probe::Refused { uri, .. } if uri.starts_with("kai-sandbox://widget/report?"))
        })
        .await;
    let Some(Probe::Refused { uri, refused, .. }) = report else {
        r.check(false, "the document's report came (its inline script ran)");
        drop(server);
        r.clear("after the sandbox").await;
        return;
    };
    r.check(
        refused.why == "not this widget's page",
        "the inline script ran, and its report's navigation was refused",
    );
    let json = report_json(&uri);
    r.line(format!("sandbox: the document saw {json}"));
    for (key, want, what) in [
        ("inline", Value::Bool(true), "an inline script runs"),
        ("http", Value::Bool(false), "an http: image is refused"),
        ("file", Value::Bool(false), "a file: image is refused"),
        ("kai", Value::Bool(false), "a kai: load is refused"),
        ("data", Value::Bool(true), "a data: image loads"),
        ("blob", Value::Bool(true), "a blob: image loads"),
        ("fetch", Value::from("refused"), "fetch is refused"),
        ("ws", Value::from("refused"), "a WebSocket is refused"),
        (
            "parent",
            Value::from("blocked"),
            "the host page is out of reach",
        ),
        (
            "webkit",
            Value::from("undefined"),
            "no message handler in the document",
        ),
        (
            "origin",
            Value::from("null"),
            "the document's origin is opaque",
        ),
        (
            "style",
            Value::from("rgb(18, 52, 86)"),
            "an inline style applies",
        ),
    ] {
        let got = json.get(key).cloned().unwrap_or(Value::Null);
        r.check(got == want, &format!("sandbox: {what} ({key}: {got})"));
    }
    let popup = json.get("popup").cloned().unwrap_or(Value::Null);
    r.check(popup != "opened", &format!("sandbox: no popup ({popup})"));
    let es = json.get("eventsource").cloned().unwrap_or(Value::Null);
    r.check(
        es.as_str()
            .is_some_and(|e| e == "refused" || e.starts_with("threw")),
        &format!("sandbox: an EventSource is refused ({es})"),
    );
    // sendBeacon queues before it checks (it may say true); the loopback count below is the proof,
    // as for the form, whose submit() does not throw in a frame without allow-forms.
    for key in ["beacon", "form"] {
        let v = json.get(key).cloned().unwrap_or(Value::Null);
        r.line(format!("sandbox: {key}: {v} (nothing arrives, below)"));
    }
    // The meta refresh (1 s) has come and gone by now; give late loads a moment.
    sleep(1500).await;
    let refreshed: Vec<String> = r
        .seen
        .iter()
        .filter_map(|p| match p {
            Probe::Refused { uri, refused, .. } if uri.starts_with("http:") => {
                Some(format!("{uri} ({})", refused.why))
            }
            _ => None,
        })
        .collect();
    r.line(format!("sandbox: refused http: navigations: {refreshed:?}"));
    let connections = server.count.load(Ordering::Relaxed);
    r.check(
        connections == 0,
        &format!(
            "the loopback server saw {connections} connections (an http image, fetch, a \
             WebSocket, preconnect, dns-prefetch, prefetch, preload, modulepreload, speculation \
             rules, a stylesheet, @import, @font-face, a CSS background, EventSource, sendBeacon, \
             a meta refresh, a form, a popup, a navigation)"
        ),
    );
    // Kai's host world from the page's main world: nothing of Kai's there.
    let main = r
        .eval_main(
            id,
            "JSON.stringify([typeof window.__kaiWidget, typeof (window.webkit && \
             window.webkit.messageHandlers && window.webkit.messageHandlers.kai)])",
        )
        .await;
    r.check(
        main.as_deref() == Ok(r#"["undefined","undefined"]"#),
        &format!("Kai's host script and handler live in its own world only (main world: {main:?})"),
    );
    drop(server);
    let _ = std::fs::remove_file(&file_png);
    // Its refused navigations (another widget's document, the loopback server, the report): the
    // first is logged (checked in 7.), every one is counted here.
    let lines = r.list().await;
    let counted = lines.first().and_then(|l| {
        l.split_once("; refused: ")?
            .1
            .split_once(" navigations")?
            .0
            .parse::<u32>()
            .ok()
    });
    r.check(
        counted.is_some_and(|n| n >= 2),
        &format!("overlay_list counts its refused navigations: {lines:?}"),
    );
    r.clear("after the sandbox").await;
}

async fn memory(r: &mut Run) {
    r.line("== 2. PSS (K1: the first widget +66 to 72 MB, each further one +15 MB)".into());
    sleep(500).await;
    let (l0, before) = pss();
    let a = r
        .show(
            Kind::Text,
            Body::Text("one".into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 360, 0),
        )
        .await;
    sleep(1500).await;
    let (l1, one) = pss();
    let b = r
        .show(
            Kind::Text,
            Body::Text("two".into()),
            look(Anchor::BottomRight, Some(OUTPUTS[0]), 360, 0),
        )
        .await;
    sleep(1500).await;
    let (l2, two) = pss();
    r.check(a.is_ok() && b.is_ok(), "two widgets shown");
    let procs = webkit_children()
        .iter()
        .filter(|(_, c)| c.starts_with("WebKitWebProces"))
        .count();
    r.check(
        procs == 1,
        &format!("both widgets share one web process ({procs})"),
    );
    r.line(format!("PSS before: {l0}"));
    r.line(format!(
        "PSS the first widget: {l1}; +{:.0} MB in all",
        one - before
    ));
    r.line(format!(
        "PSS a second widget: {l2}; +{:.0} MB in all",
        two - one
    ));
    r.clear("after PSS").await;
}

async fn positions(r: &mut Run) {
    r.line("== 3. every position on both outputs".into());
    let names = super::kinds::POSITIONS;
    for (index, output) in OUTPUTS.iter().enumerate() {
        let mut ids = Vec::new();
        for (a, word) in names {
            match r
                .show(
                    Kind::Text,
                    Body::Text(format!("{word} on {output}")),
                    look(a, Some(output), 200, 0),
                )
                .await
            {
                Ok(s) => ids.push((s.id, a)),
                Err(e) => r.check(false, &format!("{word} on {output}: {e}")),
            }
        }
        let mut placed = Vec::new();
        for (id, a) in &ids {
            let p = r
                .until(2, |p| matches!(p, Probe::Mapped { id: i, .. } if i == id))
                .await;
            if let Some(Probe::Mapped {
                width,
                height,
                output: on,
                ..
            }) = p
            {
                placed.push((*a, width as i32, height as i32, on));
            }
        }
        sleep(400).await;
        let ws = kwin_windows().await.unwrap_or_default();
        let mut ok = 0;
        for (a, w, h, on) in &placed {
            let want = expected(*a, 24, index, *w, *h);
            let found = ws
                .iter()
                .any(|k| k.output == *output && (k.x, k.y) == want && (k.w, k.h) == (*w, *h));
            if found && on.as_deref() == Some(*output) {
                ok += 1;
            } else {
                r.check(
                    false,
                    &format!(
                        "{} on {output}: expected {w}x{h} at {want:?}, Kai put it on {on:?}; \
                         KWin has {ws:?}",
                        super::kinds::position_key(*a)
                    ),
                );
            }
        }
        r.check(
            ok == names.len(),
            &format!(
                "{output}: {ok} of {} positions where KWin shows them",
                names.len()
            ),
        );
        if index == 0 {
            // An HTML widget on the other output, for the crops of each kind.
            let html = r
                .show(
                    Kind::Html,
                    Body::Document(
                        "<!doctype html><style>body{margin:0;font:18px sans-serif;color:#eff0f1}\
                         div{margin:20px;padding:20px;border-radius:12px;\
                         background:rgba(61,174,233,.85)}</style><div id=d>HTML widget</div>\
                         <script>document.getElementById('d').textContent += \
                         ' (its inline script ran)'</script>"
                            .into(),
                    ),
                    look(Anchor::Center, Some(OUTPUTS[1]), 360, 160),
                )
                .await;
            r.check(
                html.is_ok(),
                &format!("an HTML widget on {}: {html:?}", OUTPUTS[1]),
            );
            sleep(800).await;
            crops(r, "kinds").await;
        }
        r.clear(&format!("after {output}")).await;
    }
}

/// Spectacle's capture of the nested outputs, cut to each of this process's surfaces
/// (`shots/<name>-NN.png`) and to each output whole (`shots/full-<name>-<output>.png`, where a
/// widget's place on its output shows), the capture itself kept as `shots/full-<name>.png`.
async fn crops(r: &mut Run, name: &str) {
    let shots = r.out.join("shots");
    let _ = std::fs::create_dir_all(&shots);
    let full = shots.join(format!("full-{name}.png"));
    let f = full.clone();
    let took = gtk::gio::spawn_blocking(move || {
        std::process::Command::new("spectacle")
            .args(["-b", "-n", "-f", "-o"])
            .arg(&f)
            .env("QT_QPA_PLATFORM", "wayland")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    })
    .await;
    if !matches!(took, Ok(Ok(s)) if s.success()) || !full.exists() {
        r.line(format!(
            "crops {name}: skipped (spectacle wrote no capture)"
        ));
        let _ = std::fs::remove_file(&full);
        return;
    }
    let cut = |rect: (i32, i32, i32, i32), out: &Path| {
        std::process::Command::new("magick")
            .arg(&full)
            .args([
                "-crop",
                &format!("{}x{}+{}+{}", rect.2, rect.3, rect.0, rect.1),
                "+repage",
            ])
            .arg(out)
            .status()
            .is_ok_and(|s| s.success())
    };
    let ws = kwin_windows().await.unwrap_or_default();
    let mut n = 0;
    for (i, w) in ws.iter().enumerate() {
        let out = shots.join(format!("{name}-{i:02}.png"));
        n += cut((w.x, w.y, w.w, w.h), &out) as usize;
    }
    let mut outputs = 0;
    for (i, o) in OUTPUTS.iter().enumerate() {
        let out = shots.join(format!("full-{name}-{o}.png"));
        outputs += cut((i as i32 * OUT_W, 0, OUT_W, OUT_H), &out) as usize;
    }
    r.line(format!(
        "crops {name}: {n} surfaces, {outputs} whole outputs (full-{name}-*.png) in {}",
        shots.display()
    ));
}

async fn list_expiry_and_outputs(r: &mut Run) {
    r.line("== 4. overlay_list, a fitted size, expiry, an output that goes away".into());
    let mut timed = look(Anchor::Top, Some(OUTPUTS[0]), 300, 0);
    timed.timeout = Some(Duration::from_secs(2));
    let a = r
        .show(Kind::Text, Body::Text(SECRET_TEXT.into()), timed)
        .await;
    let shown_at = Instant::now();
    let mut big = look(Anchor::Center, Some(OUTPUTS[1]), 2000, 900);
    big.interactive = true;
    let b = r
        .show(Kind::Html, Body::Document("<p>big</p>".into()), big)
        .await;
    r.check(
        b.as_ref().is_ok_and(|s| s.notes.len() == 2),
        &format!("a size beyond the output is fitted and said: {b:?}"),
    );
    let lines = r.list().await;
    r.line(format!("overlay_list:\n  {}", lines.join("\n  ")));
    let (ia, ib) = (
        a.as_ref().map(|s| s.id).unwrap_or(0),
        b.as_ref().map(|s| s.id).unwrap_or(0),
    );
    r.check(
        lines.len() == 2
            && lines[0].starts_with(&format!("{ia} text top shown "))
            && lines[0].contains(" until ")
            && !lines[0].contains("until hidden")
            && lines[0].ends_with("click-through")
            && lines[1].starts_with(&format!("{ib} html center shown "))
            && lines[1].contains("until hidden interactive; its width of 2000 px"),
        "overlay_list: one line each: id, kind, position, times, interactive, notes",
    );
    let gone = r
        .until(5, |p| matches!(p, Probe::Destroyed(i) if *i == ia))
        .await;
    let after = shown_at.elapsed().as_secs_f64();
    r.check(
        gone.is_some() && (1.5..3.5).contains(&after),
        &format!("the 2 s widget expired ({after:.1} s after it showed)"),
    );
    // Virtual-1 goes away under widget b: the compositor closes it, Kai hides it.
    if let Ok(cmd) = std::env::var("KAI_NESTED_OUTPUT") {
        let c = cmd.clone();
        let off = gtk::gio::spawn_blocking(move || {
            std::process::Command::new(c)
                .args(["disable", OUTPUTS[1]])
                .status()
        })
        .await;
        r.check(
            matches!(off, Ok(Ok(s)) if s.success()),
            "Virtual-1 disabled",
        );
        let closed = r
            .until(5, |p| matches!(p, Probe::Destroyed(i) if *i == ib))
            .await;
        r.check(
            closed.is_some(),
            "the widget on the output that went away is hidden",
        );
        let on = gtk::gio::spawn_blocking(move || {
            std::process::Command::new(cmd)
                .args(["enable", OUTPUTS[1]])
                .status()
        })
        .await;
        r.check(matches!(on, Ok(Ok(s)) if s.success()), "Virtual-1 back");
        sleep(500).await;
    }
    let lines = r.list().await;
    r.check(
        lines.len() == 1
            && lines[0].starts_with(&format!("{ib} html center ended "))
            && lines[0].ends_with(": the compositor closed its surface (its output went away)"),
        &format!("overlay_list says once that the widget on the output went: {lines:?}"),
    );
    let lines = r.list().await;
    r.check(
        lines == ["no overlays"],
        &format!("nothing is left: {lines:?}"),
    );
    r.clear("after the list").await;
}

/// The screen lock (the owner's decision, 2026-10-09): every shown widget's surface is unmapped
/// while the screen is locked (KWin lists none of Kai's surfaces), mapped again at the same place
/// after; a show while locked fails with the lock's words.
async fn screen_lock(r: &mut Run) {
    r.line("== the screen lock".into());
    let mut interactive = look(Anchor::Center, Some(OUTPUTS[0]), 320, 120);
    interactive.interactive = true;
    let a = r
        .show(
            Kind::Html,
            Body::Document("<p>locked away</p>".into()),
            interactive,
        )
        .await;
    let b = r
        .show(
            Kind::Text,
            Body::Text("a note".into()),
            look(Anchor::Top, Some(OUTPUTS[1]), 240, 0),
        )
        .await;
    r.check(
        a.is_ok() && b.is_ok(),
        &format!("two widgets shown: {a:?} {b:?}"),
    );
    sleep(400).await;
    let before = kwin_windows().await.unwrap_or_default();
    r.line(format!("before the lock: {before:?}"));
    r.check(before.len() == 2, "KWin shows both surfaces");
    let ids: Vec<u64> = [&a, &b]
        .iter()
        .filter_map(|x| x.as_ref().ok().map(|s| s.id))
        .collect();
    r.send(WidgetsCmd::Locked(true));
    let mut hidden = 0;
    for id in &ids {
        if r.until(
            3,
            |p| matches!(p, Probe::LockHidden { id: i, hidden: true } if i == id),
        )
        .await
        .is_some()
        {
            hidden += 1;
        }
    }
    sleep(400).await;
    let locked = kwin_windows().await.unwrap_or_default();
    r.line(format!("locked: {locked:?}"));
    r.check(
        hidden == 2 && locked.is_empty(),
        "locked: both surfaces unmapped, KWin shows none of Kai's",
    );
    let refused = r
        .show(
            Kind::Text,
            Body::Text("while locked".into()),
            look(Anchor::Bottom, Some(OUTPUTS[0]), 200, 0),
        )
        .await;
    r.check(
        refused
            .as_ref()
            .is_err_and(|e| e.starts_with("the screen is locked; Kai is not active")),
        &format!("a show while locked fails: {refused:?}"),
    );
    r.send(WidgetsCmd::Locked(false));
    let mut back = 0;
    for id in &ids {
        if r.until(
            3,
            |p| matches!(p, Probe::LockHidden { id: i, hidden: false } if i == id),
        )
        .await
        .is_some()
        {
            back += 1;
        }
    }
    sleep(400).await;
    let after = kwin_windows().await.unwrap_or_default();
    r.line(format!("unlocked: {after:?}"));
    let same = |w: &KWinWindow| {
        before
            .iter()
            .any(|v| v.output == w.output && (v.x, v.y, v.w, v.h) == (w.x, w.y, w.w, w.h))
    };
    r.check(
        back == 2 && after.len() == 2 && after.iter().all(same),
        "unlocked: both mapped again where they were",
    );
    r.clear("after the lock").await;
}

async fn no_process_left(r: &mut Run) {
    r.line("== 5. no web process once the last widget is hidden".into());
    let a = r
        .show(
            Kind::Text,
            Body::Text("the last one".into()),
            look(Anchor::Bottom, None, 300, 0),
        )
        .await;
    r.check(
        a.is_ok(),
        &format!("a widget on KWin's active output: {a:?}"),
    );
    let with = webkit_children().len();
    let hidden = r.hide(a.as_ref().ok().map(|s| s.id)).await;
    r.check(hidden.is_ok(), "hidden by its id");
    let gone = r.until(5, |p| matches!(p, Probe::ContextGone)).await;
    sleep(500).await;
    let left = webkit_children();
    r.check(
        gone.is_some() && with > 0 && left.is_empty(),
        &format!(
            "{with} WebKit processes with the widget, {} 0.5 s after it was hidden ({left:?})",
            left.len()
        ),
    );
}

async fn wait_ring(
    rx: &async_channel::Receiver<overlay::Probe>,
    secs: u64,
    want: impl Fn(&overlay::Probe) -> bool,
) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match glib::future_with_timeout(left, rx.recv()).await {
            Ok(Ok(p)) if want(&p) => return true,
            Ok(Ok(_)) => {}
            _ => return false,
        }
    }
}

async fn crash_leaves_the_ring(r: &mut Run) {
    r.line("== 6. a widget's crash leaves the ring's web process".into());
    let ring = overlay::start(
        RingSettings {
            place: Place {
                anchor: Anchor::BottomRight,
                margin: 24,
                size: 240,
            },
            output: Some(OUTPUTS[0].into()),
            over_fullscreen: true,
        },
        LevelTaps::silent(),
        Box::new(|_| {}),
    );
    let (ptx, ring_probes) = async_channel::unbounded();
    ring.send(OverlayCmd::Probe(ptx));
    ring.send(OverlayCmd::Ring(RingInput {
        bound: true,
        active: true,
        turn: false,
        state: RingState::Listening,
        muted: false,
        loading: None,
        held: false,
    }));
    let up = wait_ring(&ring_probes, 10, |p| {
        matches!(p, overlay::Probe::Mapped { .. })
    })
    .await;
    r.check(up, "the ring is up");
    sleep(300).await;
    let ring_procs = webkit_children();
    let a = r
        .show(
            Kind::Text,
            Body::Text("crash me".into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 300, 0),
        )
        .await;
    let Ok(a) = a else {
        r.check(false, &format!("a widget beside the ring: {a:?}"));
        ring.quit().await;
        return;
    };
    let both = webkit_children();
    r.send(WidgetsCmd::Terminate(a.id));
    let crashed = r
        .until(5, |p| matches!(p, Probe::Crashed(i) if *i == a.id))
        .await;
    let gone = r.until(5, |p| matches!(p, Probe::ContextGone)).await;
    r.check(
        crashed.is_some() && gone.is_some(),
        "the widgets' web process ended: the widget is hidden, its context dropped",
    );
    // The ring saw nothing: no crash, no new view; its processes are the ones it had.
    let ring_hit = wait_ring(&ring_probes, 2, |p| {
        matches!(p, overlay::Probe::Created | overlay::Probe::Destroyed)
    })
    .await;
    sleep(500).await;
    let after = webkit_children();
    r.check(
        !ring_hit && after == ring_procs && both.len() > ring_procs.len(),
        &format!(
            "the ring's view and processes are untouched (the ring alone {ring_procs:?}, with \
             the widget {both:?}, after the crash {after:?})"
        ),
    );
    let lines = r.list().await;
    r.check(
        lines.len() == 1
            && lines[0].starts_with(&format!("{} text top-right ended ", a.id))
            && lines[0].ends_with("the widgets' web process ended (TerminatedByApi)"),
        &format!("overlay_list says once that the crashed widget went, and why: {lines:?}"),
    );
    let lines = r.list().await;
    r.check(
        lines == ["no overlays"],
        &format!("then the crashed widget is not listed: {lines:?}"),
    );
    ring.send(OverlayCmd::Ring(RingInput::default()));
    let _ = wait_ring(&ring_probes, 5, |p| matches!(p, overlay::Probe::Destroyed)).await;
    ring.quit().await;
}

/// Whether section `name` runs (`KAI_LIVE_SECTIONS`, all by default).
fn runs(name: &str) -> bool {
    match std::env::var("KAI_LIVE_SECTIONS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').any(|s| s.trim() == name),
        _ => true,
    }
}

async fn drive(r: &mut Run) {
    let handle = start();
    let (ptx, probes) = async_channel::unbounded();
    handle.send(WidgetsCmd::Probe(ptx));
    r.handle = Some(handle);
    r.probes = probes;
    if runs("sandbox") {
        sandbox(r).await;
    }
    if runs("sound") {
        hardening::sound(r).await;
    }
    if runs("memory") {
        memory(r).await;
    }
    if runs("positions") {
        positions(r).await;
    }
    if runs("list") {
        list_expiry_and_outputs(r).await;
    }
    if runs("kinds") {
        kinds::run(r).await;
    }
    if runs("apps") {
        apps::run(r).await;
    }
    if runs("lock") {
        screen_lock(r).await;
        apps::lock_view(r, &crate::screen_lock::ScreenLock::default()).await;
    }
    if runs("last") {
        no_process_left(r).await;
    }
    if runs("unresponsive") {
        hardening::unresponsive(r).await;
    }
    if runs("crash") {
        crash_leaves_the_ring(r).await;
    }
    if let Some(h) = r.handle.take() {
        h.quit().await;
    }
}

fn new_run(out: PathBuf) -> Run {
    Run {
        out,
        report: Vec::new(),
        failed: 0,
        handle: None,
        probes: async_channel::unbounded().1,
        seen: Vec::new(),
    }
}

/// The input-region run, under `WAYLAND_DEBUG=client` (scripts/live-check-widgets.sh reads its
/// log): a click-through widget and an interactive one.
#[test]
#[ignore = "needs the nested KWin of scripts/nested-kwin.sh (just live-check-widgets)"]
fn live_widgets_input_nested() {
    if let Err(e) = guard() {
        println!("skipped: {e}");
        return;
    }
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    gtk::init().expect("GTK");
    let r = glib::MainContext::default().block_on(async move {
        let mut r = new_run(out);
        let handle = start();
        let (ptx, probes) = async_channel::unbounded();
        handle.send(WidgetsCmd::Probe(ptx));
        r.handle = Some(handle);
        r.probes = probes;
        hardening::input(&mut r).await;
        if let Some(h) = r.handle.take() {
            h.quit().await;
        }
        r
    });
    assert_eq!(r.failed, 0, "{} checks failed", r.failed);
}

#[test]
#[ignore = "needs the nested KWin of scripts/nested-kwin.sh (just live-check-widgets)"]
fn live_widgets_nested() {
    if let Err(e) = guard() {
        println!("skipped: {e}");
        return;
    }
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let log = Capture::default();
    let spec = std::env::var("KAI_LOG").unwrap_or_else(|_| "trace".into());
    let _log =
        tracing::subscriber::set_default(crate::logging::subscriber(&spec, log.clone(), false));
    gtk::init().expect("GTK");
    let mut r = glib::MainContext::default().block_on(async move {
        let mut r = new_run(out.clone());
        drive(&mut r).await;
        r
    });
    let out = r.out.clone();
    r.line("== 7. the log".into());
    let text = log.text();
    std::fs::write(out.join("kai.log"), &text).unwrap();
    for (secret, what) in [
        (SECRET_TEXT, "a text widget's text"),
        (SECRET_HTML, "an HTML widget's document"),
        ("widget/report", "a refused URL"),
        ("i.png", "a URL inside a document"),
        ("extra-text", "a resource an app view read (WP12)"),
        ("fixture-view", "an app view's message (WP12)"),
    ] {
        r.check(
            !text.contains(secret),
            &format!("the log ({spec}) holds no {what}"),
        );
    }
    r.check(
        text.contains("a widget's navigation was refused") && text.contains("a widget is shown"),
        "the log says what happened",
    );
    let first = text
        .lines()
        .filter(|l| l.contains("a widget's navigation was refused") && l.contains(" widget=1 "))
        .count();
    r.check(
        first == 1,
        &format!("the first widget's refused navigations: {first} log line (the first only)"),
    );
    let report = r.report.join("\n");
    std::fs::write(out.join("report.txt"), report).unwrap();
    assert_eq!(r.failed, 0, "{} checks failed", r.failed);
}

/// Base64 for the test's PNG and WAV (no dependency for two constants).
mod base64_free {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(s: &str) -> Vec<u8> {
        let val = |c: u8| -> u32 {
            match c {
                b'A'..=b'Z' => (c - b'A') as u32,
                b'a'..=b'z' => (c - b'a' + 26) as u32,
                b'0'..=b'9' => (c - b'0' + 52) as u32,
                b'+' => 62,
                _ => 63,
            }
        };
        let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=').collect();
        let mut out = Vec::new();
        for chunk in bytes.chunks(4) {
            let mut n = 0u32;
            for (i, c) in chunk.iter().enumerate() {
                n |= val(*c) << (18 - 6 * i);
            }
            for i in 0..chunk.len().saturating_sub(1) {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        out
    }

    #[test]
    fn decodes_the_png() {
        let png = decode(super::PNG_B64);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png.len(), 70);
        assert_eq!(encode(&png), super::PNG_B64);
        assert_eq!(encode(b"ab"), "YWI=");
    }
}
