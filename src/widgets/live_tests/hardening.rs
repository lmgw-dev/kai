//! The live check's hardening sections (§10.1's as-built notes): no sound, no script or print
//! dialog, no WebGL, no fullscreen; a script that never yields; and, in a run of its own under
//! `WAYLAND_DEBUG=client` (scripts/live-check-widgets.sh parses its log with
//! scripts/wayland-input-regions.py), the widgets' input regions.
//!
//! **Sound** is checked twice: the document's own view of the APIs (absent, or refused), and the
//! private PipeWire instance the live check runs in (scripts/pw-private.sh around the nested KWin,
//! `PIPEWIRE_RUNTIME_DIR` only, so nothing reaches the session's PipeWire or its speakers): no
//! client of this process or of any of its WebKit processes connects while the document tries.
//! A GStreamer `autoaudiosink` started in the same environment is the control: it must show up.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gtk::{gio, glib};
use serde_json::Value;

use super::{OUTPUTS, Run, look, report_json, sleep, webkit_children};
use crate::overlay::placement::Anchor;
use crate::widgets::gtk_host::Probe;
use crate::widgets::kinds::{Body, Kind};

// ------------------------------------------------------------------ PipeWire

/// The private PipeWire's clients whose peer is one of `pids`: `client <id> pid <pid> <name>`.
async fn pw_clients(pids: Vec<u32>) -> Result<Vec<String>, String> {
    let out = gio::spawn_blocking(|| Command::new("timeout").args(["5", "pw-dump"]).output())
        .await
        .map_err(|_| "pw-dump's thread panicked".to_owned())?
        .map_err(|e| format!("pw-dump: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pw-dump: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let all: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("pw-dump: {e}"))?;
    let num = |v: &Value| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    Ok(all
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["type"] == "PipeWire:Interface:Client")
        .filter_map(|o| {
            let p = &o["info"]["props"];
            let pid = num(&p["pipewire.sec.pid"]).or_else(|| num(&p["application.process.id"]))?;
            pids.contains(&(pid as u32)).then(|| {
                format!(
                    "client {} pid {pid} {}",
                    o["id"],
                    p["application.name"].as_str().unwrap_or("?")
                )
            })
        })
        .collect())
}

/// This process and its WebKit processes.
fn ours() -> Vec<u32> {
    std::iter::once(std::process::id())
        .chain(webkit_children().into_iter().map(|(p, _)| p))
        .collect()
}

/// The control: `gst-launch-1.0 … ! autoaudiosink` in this environment is a client of the private
/// PipeWire (into its null sink).
async fn gst_control() -> Result<String, String> {
    let mut child = Command::new("gst-launch-1.0")
        .args([
            "-q",
            "audiotestsrc",
            "is-live=true",
            "num-buffers=400",
            "!",
            "autoaudiosink",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("gst-launch-1.0: {e}"))?;
    let pid = child.id();
    let end = Instant::now() + Duration::from_secs(5);
    let mut found = Err("no client of its pid within 5 s".to_owned());
    while Instant::now() < end {
        match pw_clients(vec![pid]).await {
            Ok(c) if !c.is_empty() => {
                found = Ok(c.join("; "));
                break;
            }
            Ok(_) => {}
            Err(e) => {
                found = Err(e);
                break;
            }
        }
        sleep(200).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    found
}

// ------------------------------------------------------------------ sound and the rest

/// 0.25 s of 440 Hz, 8 kHz 8-bit mono WAV.
fn tone_wav() -> Vec<u8> {
    let n: u32 = 2000;
    let mut w = Vec::with_capacity(44 + n as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + n).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&8000u32.to_le_bytes());
    w.extend_from_slice(&8000u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&8u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&n.to_le_bytes());
    for i in 0..n {
        let x = (i as f64 * 440.0 * std::f64::consts::TAU / 8000.0).sin();
        w.push((128.0 + 100.0 * x) as u8);
    }
    w
}

/// A document that tries every way to make a sound, a WebGL context and fullscreen, and reports
/// what it saw by a refused navigation.
fn sound_document() -> String {
    let wav = super::base64_free::encode(&tone_wav());
    format!(
        r#"<!doctype html><meta charset="utf-8">
<body style="margin:0;color:#eff0f1;font:16px sans-serif">sound probe
<audio id="tag" autoplay loop src="data:audio/wav;base64,{wav}"></audio>
<script>
const within = (p) => Promise.race([p, new Promise((_, no) => setTimeout(() => no({{ name: "no answer" }}), 1000))]);
(async () => {{
  const r = {{}};
  r.AudioContext = typeof AudioContext;
  r.webkitAudioContext = typeof webkitAudioContext;
  try {{
    const C = window.AudioContext || window.webkitAudioContext;
    const c = new C();
    const o = c.createOscillator();
    o.connect(c.destination);
    o.start();
    r.webaudio = "started (" + c.state + ")";
  }} catch (e) {{ r.webaudio = "threw " + e.name; }}
  r.speechSynthesis = typeof speechSynthesis;
  try {{
    speechSynthesis.speak(new SpeechSynthesisUtterance("Kai, this is the widget sound probe."));
    r.speak = "queued";
  }} catch (e) {{ r.speak = "threw " + e.name; }}
  r.HTMLMediaElement = typeof HTMLMediaElement;
  r.tag = document.getElementById("tag").constructor.name;
  try {{
    const a = new Audio("data:audio/wav;base64,{wav}");
    a.loop = true;
    await within(a.play());
    r.play = "playing";
  }} catch (e) {{ r.play = "refused " + e.name; }}
  try {{
    const c = document.createElement("canvas");
    r.webgl = String(c.getContext("webgl2") || c.getContext("webgl") || null);
  }} catch (e) {{ r.webgl = "threw " + e.name; }}
  r.fullscreenEnabled = String(document.fullscreenEnabled);
  try {{ await within(document.body.requestFullscreen()); r.fullscreen = "entered"; }}
  catch (e) {{ r.fullscreen = "refused " + e.name; }}
  setTimeout(() => {{
    location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(r));
  }}, 300);
}})();
</script>"#
    )
}

pub(super) async fn sound(r: &mut Run) {
    r.line("== 1b. no sound, no dialog, no WebGL, no fullscreen".into());
    let control = gst_control().await;
    r.check(
        control.is_ok(),
        &format!(
            "control: GStreamer's autoaudiosink in this environment is a client of the private \
             PipeWire ({control:?})"
        ),
    );
    let shown = r
        .show(
            Kind::Html,
            Body::Document(sound_document()),
            look(Anchor::BottomLeft, Some(OUTPUTS[0]), 480, 200),
        )
        .await;
    let id = match shown {
        Ok(s) => s.id,
        Err(e) => {
            r.check(false, &format!("the sound probe shows: {e}"));
            r.clear("after the sound probe").await;
            return;
        }
    };
    // Four seconds of looks at PipeWire while the document tries.
    let mut seen = Vec::new();
    let mut looked = 0;
    for _ in 0..14 {
        match pw_clients(ours()).await {
            Ok(c) => {
                looked += 1;
                seen.extend(c);
            }
            Err(e) => {
                r.check(false, &format!("PipeWire's clients: {e}"));
                break;
            }
        }
        sleep(300).await;
    }
    let report = r
        .until(10, |p| {
            matches!(p, Probe::Refused { uri, .. } if uri.starts_with("kai-sandbox://widget/report?"))
        })
        .await;
    let json = match report {
        Some(Probe::Refused { uri, .. }) => report_json(&uri),
        _ => Value::Null,
    };
    r.line(format!("sound: the document saw {json}"));
    let s = |k: &str| json[k].as_str().unwrap_or("(missing)").to_owned();
    for (key, ok, what) in [
        (
            "speechSynthesis",
            s("speechSynthesis") == "undefined",
            "no speechSynthesis",
        ),
        ("speak", s("speak").starts_with("threw"), "speaking throws"),
        (
            "AudioContext",
            s("AudioContext") == "undefined" && s("webkitAudioContext") == "undefined",
            "no Web Audio",
        ),
        (
            "webaudio",
            s("webaudio").starts_with("threw"),
            "an oscillator throws",
        ),
        ("play", s("play").starts_with("refused"), "no media plays"),
        (
            "webgl",
            s("webgl") == "null" || s("webgl").starts_with("threw"),
            "no WebGL context",
        ),
        (
            "fullscreen",
            s("fullscreen").starts_with("refused"),
            "no fullscreen",
        ),
    ] {
        r.check(ok, &format!("sound: {what} ({key}: {})", s(key)));
    }
    let dedup: std::collections::BTreeSet<String> = seen.into_iter().collect();
    r.check(
        looked > 10 && dedup.is_empty(),
        &format!(
            "no PipeWire client of this process or its WebKit processes in {looked} looks while the \
             document tried ({dedup:?})"
        ),
    );
    // Dialogs from the host page's main world (the caller's frame has no allow-modals at all):
    // each swallowed, none blocks.
    let t = Instant::now();
    let dialogs = glib::future_with_timeout(
        Duration::from_secs(5),
        r.eval_main(
            id,
            "alert('a'); const c = confirm('b'); const p = prompt('c'); print(); \
             JSON.stringify([c, p])",
        ),
    )
    .await;
    let ms = t.elapsed().as_millis();
    r.check(
        matches!(&dialogs, Ok(Ok(v)) if v == "[false,null]"),
        &format!("alert, confirm, prompt and print are swallowed in {ms} ms ({dialogs:?})"),
    );
    r.clear("after the sound probe").await;
}

// ------------------------------------------------------------------ a script that never yields

const HUNG: &str = "stopped responding";

pub(super) async fn unresponsive(r: &mut Run) {
    r.line("== 6b. a script that never yields".into());
    let healthy = r
        .show(
            Kind::Text,
            Body::Text("healthy".into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 300, 0),
        )
        .await;
    let hid = healthy.as_ref().map(|s| s.id).unwrap_or(0);
    r.check(healthy.is_ok(), &format!("a healthy widget: {healthy:?}"));
    // Before its load: the show never gets there. Its show timeout is long, so the ping
    // (overlay.unresponsive_s) is what ends it.
    let mut l = look(Anchor::TopLeft, Some(OUTPUTS[0]), 300, 120);
    l.show_timeout = Duration::from_secs(120);
    let t = Instant::now();
    let before = r
        .show_within(
            130,
            Kind::Html,
            Body::Document(
                "<!doctype html><p>loops before its load</p><script>for(;;);</script>".into(),
            ),
            l,
        )
        .await;
    let secs = t.elapsed().as_secs_f64();
    r.check(
        matches!(&before, Err(e) if e.contains(HUNG)),
        &format!(
            "a document that loops before its load: its show fails after {secs:.1} s ({before:?})"
        ),
    );
    let lines = r.list().await;
    r.check(
        lines.len() == 1
            && lines[0].starts_with(&format!("{hid} text top-right ended "))
            && lines[0].contains(HUNG),
        &format!(
            "overlay_list says the healthy widget in the same process went, and why: {lines:?}"
        ),
    );
    r.seen.clear();
    sleep(500).await;
    let procs = webkit_children();
    r.check(
        procs.is_empty(),
        &format!("the web process is gone ({procs:?})"),
    );
    // After its load: shown, then it stalls.
    let fresh = r
        .show(
            Kind::Text,
            Body::Text("a new widget".into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 300, 0),
        )
        .await;
    r.check(
        fresh.is_ok(),
        &format!("a new widget shows, in a new web process: {fresh:?}"),
    );
    let t = Instant::now();
    let after = r
        .show(
            Kind::Html,
            Body::Document(
                "<!doctype html><p>loops after its load</p>\
                 <script>setTimeout(() => { for(;;); }, 300)</script>"
                    .into(),
            ),
            look(Anchor::TopLeft, Some(OUTPUTS[0]), 300, 120),
        )
        .await;
    let aid = after.as_ref().map(|s| s.id).unwrap_or(0);
    r.check(
        after.is_ok(),
        &format!("a document that loops after its load shows: {after:?}"),
    );
    let gone = r
        .until(120, |p| matches!(p, Probe::Crashed(i) if *i == aid))
        .await;
    let secs = t.elapsed().as_secs_f64();
    let unresponsive = match r.until(1, |p| matches!(p, Probe::Unresponsive(_))).await {
        Some(Probe::Unresponsive(i)) => Some(i),
        _ => None,
    };
    r.check(
        gone.is_some() && unresponsive.is_some(),
        &format!(
            "Kai found its web process unresponsive (through widget {unresponsive:?}'s view) and ended \
             it, {secs:.1} s after the show"
        ),
    );
    let lines = r.list().await;
    let fid = fresh.as_ref().map(|s| s.id).unwrap_or(0);
    r.check(
        lines.len() == 2
            && lines.iter().all(|l| l.contains(HUNG))
            && lines.iter().any(|l| l.starts_with(&format!("{fid} text ")))
            && lines.iter().any(|l| l.starts_with(&format!("{aid} html "))),
        &format!("overlay_list says both went, and why: {lines:?}"),
    );
    let again = r
        .show(
            Kind::Text,
            Body::Text("after the hangs".into()),
            look(Anchor::Bottom, Some(OUTPUTS[0]), 300, 0),
        )
        .await;
    r.check(again.is_ok(), &format!("a widget shows again: {again:?}"));
    r.clear("after the hangs").await;
}

// ------------------------------------------------------------------ input regions

/// The input run: a click-through text widget, then an interactive HTML one, a second each, then
/// an interactive MCP Apps view while a K15 dialog is open and after it closed; the script reads
/// their input regions from the Wayland log. Prints `INPUT <id> <w>x<h> <interactive|guarded>`.
pub(super) async fn input(r: &mut Run) {
    r.line("== input regions (WAYLAND_DEBUG run)".into());
    let a = r
        .show(
            Kind::Text,
            Body::Text("click-through".into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 300, 0),
        )
        .await;
    let mut b_look = look(Anchor::Center, Some(OUTPUTS[0]), 320, 160);
    b_look.interactive = true;
    let b = r
        .show(
            Kind::Html,
            Body::Document("<p>interactive</p>".into()),
            b_look,
        )
        .await;
    r.check(a.is_ok() && b.is_ok(), &format!("two widgets: {a:?} {b:?}"));
    let mut others = Vec::new();
    for (id, interactive) in [(&a, false), (&b, true)] {
        let Ok(s) = id else { continue };
        if let Some(Probe::Mapped { width, height, .. }) = r
            .until(2, |p| matches!(p, Probe::Mapped { id, .. } if *id == s.id))
            .await
        {
            r.line(format!("INPUT {} {width}x{height} {interactive}", s.id));
            others.push((width as i32, height as i32));
        }
    }
    guarded_input(r, &others).await;
    sleep(1000).await;
    r.clear("after the input run").await;
}

/// An interactive app view takes clicks, takes none while Kai's K15 dialog is open (as
/// `crate::ui` builds it), and takes them again once it closed.
async fn guarded_input(r: &mut Run, others: &[(i32, i32)]) {
    use std::sync::Arc;

    use gtk::prelude::*;

    use super::apps::{No, Scripted, bound, calls};
    use crate::widgets::app_bridge::{AppBody, Bridge, Rules};
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let ends = {
        let _in = rt.enter();
        Bridge::new(
            Arc::new(Scripted),
            Rules::of_list("fx", &serde_json::json!({"tools": []})),
            Arc::new(No::default()),
            bound(),
            calls(),
            (360, 200),
        )
        .spawn()
    };
    let mut l = look(Anchor::BottomLeft, Some(OUTPUTS[0]), 360, 200);
    l.interactive = true;
    let body = Body::App(AppBody {
        server: "fx".into(),
        html: "<p style='color:#eff0f1'>an app view</p>".into(),
        ends,
        notes: vec![],
        bound: bound(),
    });
    let c = r.show(Kind::App, body, l).await;
    let Ok(c) = c else {
        r.check(false, &format!("an app view shows: {c:?}"));
        return;
    };
    let Some(Probe::Mapped { width, height, .. }) = r
        .until(2, |p| matches!(p, Probe::Mapped { id, .. } if *id == c.id))
        .await
    else {
        r.check(false, "the app view is mapped");
        return;
    };
    r.line(format!("INPUT {} {width}x{height} guarded", c.id));
    sleep(800).await;
    let (reply, _answer) = tokio::sync::oneshot::channel();
    let dialog = crate::ui::build_tool_dialog(crate::host::confirm::ConfirmRequest {
        tool: "fx__card_reset".into(),
        description: "Start counting again.".into(),
        caller: "the view of app 'fx'".into(),
        lines: vec![],
        destructive: true,
        deadline: None,
        options: Vec::new(),
        notes: Vec::new(),
        reply,
    });
    dialog.show_all();
    let on = r
        .until(
            5,
            |p| matches!(p, Probe::Guarded { id, on: true } if *id == c.id),
        )
        .await
        .is_some();
    r.check(on, "a K15 dialog open: the app view is held");
    let (clear, ws) = super::nothing_over_the_dialog(others).await;
    r.check(
        clear,
        &format!(
            "input: the held view's strip covers no part of the dialog (the other kinds' widgets \
             {others:?} are not held; {ws:?})"
        ),
    );
    sleep(1000).await;
    dialog.close();
    let off = r
        .until(
            5,
            |p| matches!(p, Probe::Guarded { id, on: false } if *id == c.id),
        )
        .await
        .is_some();
    r.check(off, "the dialog closed: the app view as configured again");
    sleep(1000).await;
    rt.shutdown_background();
}
