//! The ring when its output goes away and comes back (review KAI-5 R5-14), against a nested KWin
//! with two virtual outputs (scripts/nested-kwin.sh; scripts/live-check-ring.sh runs this). Nothing
//! of it shows on the session's screen, and the session's outputs are never touched. Ignored by
//! default; it refuses to run unless `KAI_LIVE_NESTED=1` and the display is that nested KWin.
//!
//! The overlay runs as in Kai, its levels silent (no audio). KWin itself says where the ring is: a
//! KWin script lists this process's windows with their output and whether KWin still shows them
//! (a closed window lingers as deleted for its animation). The overlay's probes say what Kai saw
//! (the compositor's close, the outputs' changes) and which output it put the surface on.
//! - `ring.output` Virtual-1: the ring up on it; Virtual-1 disabled (as a monitor that powers off
//!   and unplugs): the ring shows again on Virtual-0, its page still drawing, on the same view;
//!   Virtual-1 back: the ring goes back to it.
//! - No `ring.output`: the ring up on KWin's active output; that output disabled: the ring on the
//!   other one; the output back: the ring on KWin's active output. A hidden ring (standby) whose
//!   output goes away shows on the remaining one when Kai leaves standby.
//! - No output at all (review KAI-6 R6-2), the one case KWin does not produce: it keeps at least
//!   one output on (it refuses to disable the last, and turns a disabled one back on when the last
//!   enabled one goes), and its virtual backend has one at least. So the overlay's output
//!   resolution is told the list is empty ([`OverlayCmd::NoOutputs`]) while a real output goes
//!   away under the ring, with KWin's own close and output events: nothing maps, the tray says
//!   so, a turn's start maps nothing, and the output back shows the ring on it, on the same view.

use std::sync::atomic::{AtomicU32, Ordering};

use gtk::gio;

use super::*;

/// The nested KWin's outputs (scripts/nested-kwin.sh).
const OUTPUTS: [&str; 2] = ["Virtual-0", "Virtual-1"];

/// Runs only against scripts/nested-kwin.sh's KWin: the display is its socket in its directory,
/// and the bus is exactly the private one it wrote down there (review KAI-6 R6-6).
fn guard() -> Result<(), String> {
    if std::env::var("KAI_LIVE_NESTED").as_deref() != Ok("1") {
        return Err("KAI_LIVE_NESTED is not 1".into());
    }
    if std::env::var_os("KAI_NESTED_OUTPUT").is_none() {
        return Err("no KAI_NESTED_OUTPUT".into());
    }
    let log = std::env::var_os("KAI_NESTED_KWIN_LOG").ok_or("no KAI_NESTED_KWIN_LOG")?;
    let dir = Path::new(&log)
        .parent()
        .ok_or("KAI_NESTED_KWIN_LOG names no directory")?;
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    if Path::new(&display) != dir.join("run/kai-nested") {
        return Err(format!(
            "WAYLAND_DISPLAY {display:?} is not the nested KWin's socket in {}",
            dir.display()
        ));
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let private = std::fs::read_to_string(dir.join("bus")).unwrap_or_default();
    if bus.is_empty()
        || bus != private.trim()
        || !bus.contains(&format!("path={}/run/", dir.display()))
    {
        return Err("not the nested KWin's private bus".into());
    }
    let mut names: Vec<String> = super::super::outputs::wayland_outputs()?
        .into_iter()
        .map(|o| o.name)
        .collect();
    names.sort();
    if names != OUTPUTS {
        return Err(format!(
            "the display's outputs are {names:?}, not the nested KWin's {OUTPUTS:?}"
        ));
    }
    if crate::nvidia::nvidia_present() && std::env::var_os(crate::nvidia::VAR).is_none() {
        return Err(format!(
            "NVIDIA is present: {} must be in the environment",
            crate::nvidia::VAR
        ));
    }
    Ok(())
}

/// One of this process's windows as KWin has it.
#[derive(Debug, Clone)]
struct KWinWindow {
    output: String,
    /// KWin still shows it (not a closed window kept for its animation).
    live: bool,
    #[allow(dead_code)] // read through Debug, in the report
    geometry: String,
}

/// Exactly one window of this process that KWin shows, on `output`.
fn live_on(ws: &[KWinWindow], output: &str) -> bool {
    let live: Vec<&KWinWindow> = ws.iter().filter(|w| w.live).collect();
    live.len() == 1 && live[0].output == output
}

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

/// KWin's active output, as Kai asks for it (`org.kde.KWin.activeOutputName`).
fn kwin_active() -> String {
    busctl(&[
        "call",
        "org.kde.KWin",
        "/KWin",
        "org.kde.KWin",
        "activeOutputName",
    ])
    .map(|s| s.trim_start_matches("s ").trim_matches('"').to_owned())
    .unwrap_or_else(|e| format!("(unknown: {e})"))
}

static QUERY: AtomicU32 = AtomicU32::new(0);

/// This process's windows, from a script run in the nested KWin; it prints to KWin's log.
async fn kwin_windows() -> Result<Vec<KWinWindow>, String> {
    let n = QUERY.fetch_add(1, Ordering::Relaxed);
    let tag = format!("KAI-NESTED-{n}");
    let log = PathBuf::from(std::env::var("KAI_NESTED_KWIN_LOG").unwrap());
    let script = log.with_file_name(format!("query-{n}.js"));
    let pid = std::process::id();
    std::fs::write(
        &script,
        format!(
            r#"for (const w of workspace.stackingOrder) {{
    if (w.pid !== {pid}) continue;
    const g = w.frameGeometry;
    print("{tag} output=" + (w.output ? w.output.name : "none") + " deleted=" + w.deleted
        + " geometry=" + g.x + "," + g.y + "," + g.width + "x" + g.height);
}}
print("{tag} end");
"#
        ),
    )
    .map_err(|e| e.to_string())?;
    let name = format!("kai-nested-{n}");
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
        if text.contains(&format!("{tag} end")) {
            break text;
        }
        if Instant::now() > end {
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
    let field = |l: &str, k: &str| {
        l.split_whitespace()
            .find_map(|f| f.strip_prefix(k))
            .unwrap_or("")
            .to_owned()
    };
    Ok(text
        .lines()
        .filter(|l| l.contains(&format!("{tag} output=")))
        .map(|l| KWinWindow {
            output: field(l, "output="),
            live: field(l, "deleted=") == "false",
            geometry: field(l, "geometry="),
        })
        .collect())
}

/// KWin's windows of this process until `want` holds of them, within `secs`; checked and said.
async fn kwin_until(
    r: &mut Run,
    secs: u64,
    what: &str,
    want: impl Fn(&[KWinWindow]) -> bool,
) -> Vec<KWinWindow> {
    let t = Instant::now();
    let end = t + Duration::from_secs(secs);
    let mut last = Vec::new();
    loop {
        match kwin_windows().await {
            Ok(ws) => last = ws,
            Err(e) => {
                r.check(false, &format!("{what}: {e}"));
                return last;
            }
        }
        if want(&last) || Instant::now() > end {
            break;
        }
        sleep(100).await;
    }
    let ok = want(&last);
    r.check(
        ok,
        &format!(
            "{what} ({}; KWin has {last:?})",
            if ok {
                format!("{:.0} ms", t.elapsed().as_secs_f64() * 1000.0)
            } else {
                format!("not within {secs} s")
            }
        ),
    );
    last
}

/// Switches a nested output (scripts/nested-kwin.sh's `KAI_NESTED_OUTPUT`), off the GTK thread.
async fn switch_output(r: &mut Run, how: &'static str, name: &'static str) {
    let cmd = std::env::var("KAI_NESTED_OUTPUT").unwrap();
    let status =
        gio::spawn_blocking(move || std::process::Command::new(cmd).args([how, name]).status())
            .await;
    let ok = matches!(status, Ok(Ok(s)) if s.success());
    r.check(ok, &format!("the nested KWin's {name}: {how}"));
}

/// The next map's output, as Kai resolved it, within `secs`.
async fn mapped_on(r: &mut Run, secs: u64, what: &str, output: &str) {
    let p = r
        .wait(secs, what, |p| matches!(p, Probe::Mapped { .. }))
        .await;
    if let Some(Probe::Mapped {
        output: got, note, ..
    }) = &p
    {
        r.check(
            got.as_deref() == Some(output),
            &format!("{what}: Kai put it on {got:?} (note: {note:?})"),
        );
    }
}

/// The page draws: the engine runs, visible, and its frames advance.
async fn page_draws(r: &mut Run, what: &str) {
    let s0 = r.stats().await;
    sleep(600).await;
    let s1 = r.stats().await;
    let advanced = s1["frames"].as_f64().unwrap_or(0.0) > s0["frames"].as_f64().unwrap_or(0.0);
    r.check(
        s1["running"] == true && s1["hidden"] == false && advanced && s1["state"] == "listening",
        &format!(
            "{what}: the page draws the listening state (frames {} -> {})",
            s0["frames"], s1["frames"]
        ),
    );
}

fn start_overlay(r: &mut Run, output: Option<&str>) {
    let settings = RingSettings {
        place: place_from_env(),
        output: output.map(str::to_owned),
        over_fullscreen: true,
    };
    let handle = super::super::start(settings, LevelTaps::silent(), Box::new(|_| {}));
    let (ptx, probes) = async_channel::unbounded();
    handle.send(OverlayCmd::Probe(ptx));
    r.handle = Some(handle);
    r.probes = probes;
    r.seen.clear();
}

async fn up(r: &mut Run, what: &str) {
    use RingState::*;
    r.ring(true, true, false, Idle);
    r.wait(10, &format!("{what}: the page is ready"), |p| {
        matches!(p, Probe::Ready { .. })
    })
    .await;
    r.wait(5, &format!("{what}: the surface maps"), |p| {
        matches!(p, Probe::Mapped { .. })
    })
    .await;
}

async fn session_end(r: &mut Run, what: &str) {
    r.ring(false, false, false, RingState::Idle);
    r.wait(5, &format!("{what}: the view is destroyed"), |p| {
        matches!(p, Probe::Destroyed)
    })
    .await;
    if let Some(h) = r.handle.take() {
        h.quit().await;
    }
}

async fn drive(r: &mut Run) {
    use RingState::*;
    let is_new_view = |p: &Probe| matches!(p, Probe::Created | Probe::Destroyed);

    // ---- 1. ring.output names Virtual-1, continuous listening: the ring up there
    r.line("== ring.output Virtual-1".into());
    start_overlay(r, Some("Virtual-1"));
    up(r, "ring.output Virtual-1").await;
    kwin_until(r, 5, "KWin shows the ring on Virtual-1", |w| {
        live_on(w, "Virtual-1")
    })
    .await;
    page_draws(r, "on Virtual-1").await;

    // ---- 2. Virtual-1 goes away while the ring is up on it
    let from = r.seen.len();
    let t = Instant::now();
    switch_output(r, "disable", "Virtual-1").await;
    r.wait(
        5,
        "Virtual-1 disabled: the compositor's close reaches Kai",
        |p| matches!(p, Probe::Closed),
    )
    .await;
    mapped_on(r, 5, "Virtual-1 disabled: the ring maps again", "Virtual-0").await;
    kwin_until(
        r,
        5,
        "Virtual-1 disabled under the ring: KWin shows it on Virtual-0",
        |w| live_on(w, "Virtual-0"),
    )
    .await;
    r.line(format!(
        "Virtual-1 disabled: KWin's active output {}, {:.0} ms after the switch",
        kwin_active(),
        t.elapsed().as_secs_f64() * 1000.0
    ));
    page_draws(r, "on Virtual-0 after Virtual-1 went").await;
    r.none_since(from, "Virtual-1 disabled: the same view", is_new_view);
    let saw = |r: &Run, f: fn(&Probe) -> bool| r.seen[from..].iter().any(f);
    r.check(
        saw(r, |p| matches!(p, Probe::Outputs(false))),
        "Virtual-1 disabled: Kai heard the output go",
    );

    // ---- 3. Virtual-1 comes back: the ring goes back to ring.output
    let from = r.seen.len();
    switch_output(r, "enable", "Virtual-1").await;
    r.wait(5, "Virtual-1 back: Kai hears the output come", |p| {
        matches!(p, Probe::Outputs(true))
    })
    .await;
    mapped_on(
        r,
        5,
        "Virtual-1 back: the ring is placed again",
        "Virtual-1",
    )
    .await;
    kwin_until(
        r,
        5,
        "Virtual-1 back: KWin shows the ring on it again (ring.output)",
        |w| live_on(w, "Virtual-1"),
    )
    .await;
    page_draws(r, "on Virtual-1 again").await;
    // A turn's start places the ring that is up again: it stays.
    r.ring(true, true, true, Listening);
    r.wait(5, "a turn's start places the ring again", |p| {
        matches!(p, Probe::Mapped { .. })
    })
    .await;
    r.ring(true, true, false, Idle);
    kwin_until(
        r,
        5,
        "after a turn: KWin shows the ring on Virtual-1",
        |w| live_on(w, "Virtual-1"),
    )
    .await;
    r.none_since(from, "Virtual-1 back: the same view", is_new_view);
    session_end(r, "ring.output Virtual-1").await;

    // ---- 4. no ring.output: the ring on KWin's active output, which goes away
    r.line("== no ring.output: KWin's active output".into());
    start_overlay(r, None);
    let active = kwin_active();
    r.line(format!("KWin's active output: {active}"));
    let (gone, other): (&'static str, &'static str) = if active == OUTPUTS[1] {
        (OUTPUTS[1], OUTPUTS[0])
    } else {
        (OUTPUTS[0], OUTPUTS[1])
    };
    up(r, "no ring.output").await;
    kwin_until(r, 5, &format!("KWin shows the ring on {gone}"), |w| {
        live_on(w, gone)
    })
    .await;
    let from = r.seen.len();
    switch_output(r, "disable", gone).await;
    r.wait(
        5,
        &format!("{gone} disabled: the compositor's close reaches Kai"),
        |p| matches!(p, Probe::Closed),
    )
    .await;
    mapped_on(
        r,
        5,
        &format!("{gone} disabled: the ring maps again"),
        other,
    )
    .await;
    kwin_until(
        r,
        5,
        &format!("{gone} disabled under the ring: KWin shows it on {other}"),
        |w| live_on(w, other),
    )
    .await;
    page_draws(r, &format!("on {other} after {gone} went")).await;
    switch_output(r, "enable", gone).await;
    sleep(1000).await;
    let active = kwin_active();
    r.line(format!("{gone} back: KWin's active output is {active}"));
    kwin_until(
        r,
        5,
        &format!("{gone} back: KWin shows the ring on the active output, {active}"),
        |w| live_on(w, &active),
    )
    .await;
    r.none_since(
        from,
        "the active output away and back: the same view",
        is_new_view,
    );

    // ---- 5. a hidden ring (standby) whose output goes away shows on the remaining one
    let on = kwin_windows()
        .await
        .ok()
        .and_then(|ws| ws.into_iter().find(|w| w.live).map(|w| w.output))
        .unwrap_or_default();
    let (gone, other): (&'static str, &'static str) = if on == OUTPUTS[1] {
        (OUTPUTS[1], OUTPUTS[0])
    } else {
        (OUTPUTS[0], OUTPUTS[1])
    };
    r.send(OverlayCmd::Ring(RingInput {
        bound: true,
        muted: true,
        ..RingInput::default()
    }));
    r.wait(4, "muted: the ring hides", |p| matches!(p, Probe::Hidden))
        .await;
    let from = r.seen.len();
    switch_output(r, "disable", gone).await;
    sleep(800).await;
    r.none_since(
        from,
        &format!("hidden, {gone} disabled: nothing maps"),
        |p| {
            matches!(
                p,
                Probe::Mapped { .. } | Probe::Created | Probe::Destroyed | Probe::Closed
            )
        },
    );
    r.ring(true, true, false, Idle);
    mapped_on(r, 5, "unmuted: the surface maps", other).await;
    kwin_until(
        r,
        5,
        &format!("unmuted with {gone} away: KWin shows the ring on {other}"),
        |w| live_on(w, other),
    )
    .await;
    switch_output(r, "enable", gone).await;
    sleep(800).await;

    // ---- 6. no output at all: the resolution finds none while a real output goes away under it
    no_output(r).await;
    session_end(r, "no ring.output").await;
    // The web process ends shortly after its view (as at the session's end in the desktop check).
    sleep(1500).await;
    let leftover = webkit_children();
    r.check(
        leftover.is_empty(),
        &format!("no WebKit process of this process 1.5 s after the last view: {leftover:?}"),
    );
}

/// Step 6 (review KAI-6 R6-2): a single monitor powered off overnight, as far as a nested KWin
/// can show it. The ring is up, with no `ring.output`.
async fn no_output(r: &mut Run) {
    let on = kwin_windows()
        .await
        .ok()
        .and_then(|ws| ws.into_iter().find(|w| w.live).map(|w| w.output))
        .unwrap_or_default();
    r.line(format!("== no output at all (the ring is on {on})"));
    let gone: &'static str = if on == OUTPUTS[1] {
        OUTPUTS[1]
    } else {
        OUTPUTS[0]
    };
    let is_map = |p: &Probe| matches!(p, Probe::Mapped { .. } | Probe::Created | Probe::Destroyed);
    let from = r.mark();
    r.send(OverlayCmd::NoOutputs(true));
    switch_output(r, "disable", gone).await;
    r.wait(
        5,
        &format!("{gone} disabled, no output listed: Kai finds none"),
        |p| matches!(p, Probe::NoOutput),
    )
    .await;
    r.wait(
        2,
        "no output: the tray hears the ring is down",
        |p| matches!(p, Probe::Note(Some(n)) if n.starts_with("no output is connected")),
    )
    .await;
    sleep(600).await;
    let saw = |r: &Run, f: fn(&Probe) -> bool| r.seen[from..].iter().any(f);
    r.line(format!(
        "no output: Kai heard the output go {}, and the compositor's close {}",
        saw(r, |p| matches!(p, Probe::Outputs(false))),
        saw(r, |p| matches!(p, Probe::Closed))
    ));
    r.none_since(from, "no output: nothing maps, the same view", is_map);
    kwin_until(r, 5, "no output: KWin shows no window of Kai's", |w| {
        w.iter().all(|w| !w.live)
    })
    .await;
    // A turn's start while there is no output maps nothing.
    let from = r.mark();
    r.ring(true, true, true, RingState::Listening);
    sleep(600).await;
    r.ring(true, true, false, RingState::Idle);
    sleep(200).await;
    r.none_since(from, "no output, a turn's start: nothing maps", is_map);
    kwin_until(
        r,
        2,
        "no output, after a turn: still no window of Kai's",
        |w| w.iter().all(|w| !w.live),
    )
    .await;
    // The output is back: the ring shows on the output resolved then.
    let from = r.mark();
    r.send(OverlayCmd::NoOutputs(false));
    switch_output(r, "enable", gone).await;
    r.wait(5, "the output back: Kai hears it come", |p| {
        matches!(p, Probe::Outputs(true))
    })
    .await;
    let p = r
        .wait(5, "the output back: the ring maps", |p| {
            matches!(p, Probe::Mapped { .. })
        })
        .await;
    let to = match p {
        Some(Probe::Mapped {
            output: Some(o), ..
        }) => o,
        other => {
            r.check(
                false,
                &format!("the output back: no output named ({other:?})"),
            );
            String::new()
        }
    };
    let active = kwin_active();
    r.check(
        to == active,
        &format!("the output back: Kai put the ring on {to:?}, KWin's active output {active:?}"),
    );
    kwin_until(
        r,
        5,
        &format!("the output back: KWin shows the ring on {to}"),
        |w| live_on(w, &to),
    )
    .await;
    // The line goes with the output's arrival, before the map.
    let cleared = r.seen[from..]
        .iter()
        .any(|p| matches!(p, Probe::Note(None)));
    r.check(cleared, "the output back: the tray's line goes");
    page_draws(r, "the output back").await;
    r.none_since(from, "the output back: the same view", |p| {
        matches!(p, Probe::Created | Probe::Destroyed)
    });
}

#[test]
#[ignore = "needs a nested KWin (scripts/nested-kwin.sh; scripts/live-check-ring.sh runs it)"]
fn live_ring_output_removed() {
    if let Err(why) = guard() {
        eprintln!("skipped: {why} (run scripts/live-check-ring.sh)");
        return;
    }
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "kai=debug".into()),
        std::io::stderr,
        false,
    ));
    let out =
        PathBuf::from(std::env::var("KAI_LIVE_OUT").unwrap_or_else(|_| "target/live-ring".into()));
    std::fs::create_dir_all(&out).unwrap();
    gtk::init().expect("GTK");
    let (_, probes) = async_channel::unbounded();
    let mut run = Run {
        handle: None,
        probes,
        seen: Vec::new(),
        lines: Vec::new(),
        fails: Vec::new(),
        shot: None,
        rect: (0, 0, 0, 0),
        shots: Vec::new(),
        refresh_mhz: 0,
    };
    let done: Rc<RefCell<Option<Run>>> = Rc::default();
    let d2 = done.clone();
    glib::MainContext::default().spawn_local(async move {
        drive(&mut run).await;
        if let Some(h) = run.handle.take() {
            h.quit().await;
        }
        *d2.borrow_mut() = Some(run);
        gtk::main_quit();
    });
    gtk::main();
    let run = done.borrow_mut().take().expect("the script ran");
    let report = run.lines.join("\n") + "\n";
    std::fs::write(out.join("nested-report.txt"), &report).unwrap();
    assert!(run.fails.is_empty(), "failed: {:?}", run.fails);
}
