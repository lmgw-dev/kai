//! WP14's live check in a nested KWin (scripts/nested-kwin.sh, run by
//! scripts/live-check-desktop.sh): two virtual outputs, its own bus and XDG directories, test
//! windows (two resizable GTK 3 windows of one app id, `kai-live`, and a kdialog); nothing shows on the session's screen and the session's KWin is
//! never scripted or read. Ignored by default; it refuses to run unless `KAI_LIVE_NESTED=1` and the
//! display and the bus are the nested session's.
//!
//! Twice, with the same steps: first with no `.desktop` for this binary (KWin scripting gives the
//! window facts), then with one granting `org_kde_plasma_window_management` written while KWin
//! runs (WP13 (d): the grant follows about 1.4 s later; the protocol gives the facts). The steps:
//! the list, `desktop_context`, an ambiguous app, activate, move to the other output, tile left and
//! untile (KWin's own geometry read back by a script of the test), maximize and restore,
//! fullscreen and back, minimize and activate, move to another virtual desktop, the errors for an
//! unknown output and side; the second run ends by closing a window.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::plasma::kwin_script::KwinScript;
use super::plasma::wm::Wm;
use super::probe::ProviderName;
use super::settings::DesktopConfig;
use super::{DesktopTools, Env, SystemBus};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Changes, Entry, Provider, SessionBus};
use kai_desktop::caps::Capability;

const T: Duration = Duration::from_secs(5);

pub(crate) fn guard() -> Result<PathBuf, String> {
    if std::env::var("KAI_LIVE_NESTED").as_deref() != Ok("1") {
        return Err("KAI_LIVE_NESTED is not 1".into());
    }
    let log = std::env::var_os("KAI_NESTED_KWIN_LOG").ok_or("no KAI_NESTED_KWIN_LOG")?;
    let dir = Path::new(&log)
        .parent()
        .ok_or("KAI_NESTED_KWIN_LOG names no directory")?
        .to_owned();
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    if Path::new(&display) != dir.join("run/kai-nested") {
        return Err(format!(
            "WAYLAND_DISPLAY {display:?} is not the nested KWin's socket"
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
    if std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().as_deref() != Some(bus.as_str()) {
        return Err("DBUS_SYSTEM_BUS_ADDRESS must be the private bus too".into());
    }
    Ok(dir)
}

/// A resizable GTK 3 window titled by its argument, app id `kai-live`.
const GTK_WINDOW: &str = "import sys, gi
gi.require_version('Gtk', '3.0')
from gi.repository import GLib, Gtk
GLib.set_prgname('kai-live')
w = Gtk.Window(title=sys.argv[1])
w.set_default_size(400, 300)
w.connect('destroy', Gtk.main_quit)
w.show_all()
Gtk.main()";

struct Windows(Vec<std::process::Child>);

impl Drop for Windows {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
            crate::desktop::resolve::stand_in_gone(c.id());
        }
    }
}

/// A test window: a child of the test process standing in for an app of the owner's
/// (`resolve::STAND_INS`), so the window actions do not refuse it as one of Kai's own.
fn spawn(cmd: &str, args: &[&str]) -> std::process::Child {
    let child = std::process::Command::new(cmd)
        .args(args)
        .env("GDK_BACKEND", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{cmd}: {e}"));
    crate::desktop::resolve::STAND_INS
        .lock()
        .unwrap()
        .push(child.id());
    child
}

fn env() -> Env {
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    Env {
        current_desktop: "KDE".into(),
        session: Some(SessionBus::lazy(T)),
        system: Some(SystemBus::new(Some(bus), T)),
        wayland: true,
        timeout: T,
        runtime_dir: PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("kai"),
        ..Env::inert()
    }
}

async fn desktop() -> (Vec<Entry>, String) {
    let d = DesktopTools::new(env(), DesktopConfig::default(), Changes::new().0, false);
    d.probe_now().await;
    let chosen = d.chosen();
    for line in super::probe::log_lines(&chosen) {
        println!("table {line}");
    }
    let windows = chosen
        .iter()
        .find(|c| c.capability == Capability::Windows)
        .unwrap();
    assert_eq!(
        windows.provider,
        Some(ProviderName::Plasma),
        "{}",
        windows.note
    );
    (d.entries(), windows.note.clone())
}

async fn tool(entries: &[Entry], name: &str, args: Value) -> (String, bool) {
    let e = entries
        .iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    let shown = args.to_string();
    let args = Args::read(e.spec, args.as_object().cloned()).unwrap();
    let started = Instant::now();
    let reply = (e.handler)(Call::new(
        e.spec,
        args,
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await;
    let text: Vec<String> = reply
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let text = text.join("\n");
    println!(
        "{name} {shown} -> {} in {} ms: {}",
        if reply.error { "error" } else { "ok" },
        started.elapsed().as_millis(),
        text.replace('\n', " | ")
    );
    (text, reply.error)
}

/// The window list's line of the window whose title holds `title`.
fn line_of(list: &str, title: &str) -> String {
    list.lines()
        .find(|l| l.contains(title))
        .unwrap_or_default()
        .to_owned()
}

/// Polls `window_list` until `ok` holds for the line of `title` (5 s at most).
async fn until(entries: &[Entry], title: &str, what: &str, ok: impl Fn(&str) -> bool) -> String {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let (list, _) = quiet_list(entries).await;
        let line = line_of(&list, title);
        if ok(&line) {
            println!("  {what}: {line}");
            return line;
        }
        assert!(Instant::now() < end, "{what}: not within 5 s: {list}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn quiet_list(entries: &[Entry]) -> (String, bool) {
    let e = entries
        .iter()
        .find(|e| e.spec.name == "window_list")
        .unwrap();
    let reply = (e.handler)(Call::new(
        e.spec,
        Args::default(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await;
    match &reply.blocks[0] {
        Block::Text(t) => (t.clone(), reply.error),
        _ => unreachable!(),
    }
}

/// KWin's own view of the window titled `title` (the test's script, not Kai's tools).
async fn kwin_view(script: &KwinScript, title: &str) -> Value {
    script
        .run(
            r#"function run() {
    const w = workspace.windowList().find(w => w.caption === ARGS.title);
    if (!w) return null;
    const g = w.frameGeometry;
    return { x: g.x, y: g.y, width: g.width, height: g.height, tile: w.tile !== null,
             output: w.output.name, desktops: w.desktops.map(d => d.name) };
}"#,
            json!({"title": title}),
        )
        .await
        .unwrap()
}

async fn until_kwin(script: &KwinScript, title: &str, what: &str, ok: impl Fn(&Value) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let v = kwin_view(script, title).await;
        if ok(&v) {
            println!("  {what}: KWin says {v}");
            return;
        }
        assert!(Instant::now() < end, "{what}: KWin says {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn exercise(entries: &[Entry], script: &KwinScript, run: &str) {
    println!("== {run}");
    let names: Vec<&str> = entries
        .iter()
        .filter(|e| e.area == "windows")
        .map(|e| e.spec.name)
        .collect();
    assert_eq!(names.len(), 10, "{names:?}");
    let (list, err) = tool(entries, "window_list", json!({})).await;
    assert!(!err);
    for t in ["Kai Live One", "Kai Live Two", "Kai Live Three"] {
        assert!(list.contains(t), "{t} missing: {list}");
    }
    let (ctx, err) = tool(entries, "desktop_context", json!({})).await;
    assert!(!err);
    assert!(ctx.contains("outputs: Virtual-0 1280x720 at 0,0"), "{ctx}");
    assert!(ctx.contains("Virtual-1 1280x720 at 1280,0"), "{ctx}");
    assert!(ctx.contains("active window: "), "{ctx}");
    assert!(ctx.contains("media: nothing playing"), "{ctx}");
    let (e, err) = tool(entries, "window_activate", json!({"window": "kai-live"})).await;
    assert!(err && e.starts_with("2 windows match"), "{e}");
    let (_, err) = tool(entries, "window_activate", json!({"window": "Live One"})).await;
    assert!(!err);
    until(entries, "Kai Live One", "active", |l| l.contains("active")).await;
    // The other output than the one it is on.
    let on = kwin_view(script, "Kai Live One").await["output"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = if on == "Virtual-0" {
        "Virtual-1"
    } else {
        "Virtual-0"
    };
    let (_, err) = tool(
        entries,
        "window_to_output",
        json!({"window": "Live One", "output": other.to_lowercase()}),
    )
    .await;
    assert!(!err);
    until(entries, "Kai Live One", "moved", |l| l.contains(other)).await;
    let x0 = if other == "Virtual-0" { 0 } else { 1280 };
    let (_, err) = tool(
        entries,
        "window_tile",
        json!({"window": "Live One", "side": "left"}),
    )
    .await;
    assert!(!err);
    until_kwin(script, "Kai Live One", "tiled left", |v| {
        v["x"] == x0 && v["width"] == 640 && v["tile"] == true
    })
    .await;
    let (_, err) = tool(
        entries,
        "window_tile",
        json!({"window": "Live One", "side": "none"}),
    )
    .await;
    assert!(!err);
    until_kwin(script, "Kai Live One", "untiled", |v| {
        v["tile"] == false && v["width"] != 640
    })
    .await;
    for (tool_name, word) in [
        ("window_maximize", "maximized"),
        ("window_fullscreen", "fullscreen"),
    ] {
        let (r, err) = tool(entries, tool_name, json!({"window": "Live One"})).await;
        assert!(!err && r.starts_with(word), "{r}");
        until(entries, "Kai Live One", word, |l| l.contains(word)).await;
        let (r, err) = tool(entries, tool_name, json!({"window": "Live One"})).await;
        assert!(!err && !r.starts_with(word), "{r}");
        until(entries, "Kai Live One", &format!("not {word}"), |l| {
            !l.contains(word)
        })
        .await;
    }
    let (_, err) = tool(entries, "window_minimize", json!({"window": "Live Two"})).await;
    assert!(!err);
    until(entries, "Kai Live Two", "minimized", |l| {
        l.contains("minimized")
    })
    .await;
    let (_, err) = tool(entries, "window_activate", json!({"window": "Live Two"})).await;
    assert!(!err);
    until(entries, "Kai Live Two", "back", |l| {
        l.contains("active") && !l.contains("minimized")
    })
    .await;
    // Onto the desktop it is not on.
    let desktops = kwin_view(script, "Kai Live Three").await["desktops"].clone();
    let target = if desktops == json!(["Second"]) {
        "1"
    } else {
        "2"
    };
    let name = if target == "1" { "Desktop 1" } else { "Second" };
    let (_, err) = tool(
        entries,
        "window_to_desktop",
        json!({"window": "kdialog", "desktop": target}),
    )
    .await;
    assert!(!err);
    until(entries, "Kai Live Three", "desktop", |l| {
        l.contains(&format!(", {name}"))
    })
    .await;
    let (e, err) = tool(entries, "window_to_output", json!({"output": "DP-9"})).await;
    assert!(
        err && e == "no such output; the outputs are: Virtual-0, Virtual-1",
        "{e}"
    );
    let (e, err) = tool(entries, "window_tile", json!({"side": "middle"})).await;
    assert!(err && e.starts_with("side is one of left, right"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a nested KWin only: scripts/live-check-desktop.sh"]
async fn live_windows_in_nested_kwin() {
    let dir = match guard() {
        Ok(d) => d,
        Err(e) => {
            println!("skipped: {e}");
            return;
        }
    };
    let script = KwinScript::new(SessionBus::lazy(T), dir.join("run/kai-test"), T);
    // A second virtual desktop to move a window to.
    let bus = zbus::Connection::session().await.unwrap();
    bus.call_method(
        Some("org.kde.KWin"),
        "/VirtualDesktopManager",
        Some("org.kde.KWin.VirtualDesktopManager"),
        "createDesktop",
        &(1u32, "Second"),
    )
    .await
    .unwrap();
    let _windows = Windows(vec![
        spawn("python3", &["-I", "-c", GTK_WINDOW, "Kai Live One"]),
        spawn("python3", &["-I", "-c", GTK_WINDOW, "Kai Live Two"]),
        spawn(
            "kdialog",
            &["--title", "Kai Live Three", "--msgbox", "three"],
        ),
    ]);
    let end = Instant::now() + Duration::from_secs(15);
    while kwin_view(&script, "Kai Live Three").await.is_null()
        || kwin_view(&script, "Kai Live Two").await.is_null()
        || kwin_view(&script, "Kai Live One").await.is_null()
    {
        assert!(Instant::now() < end, "the test windows did not show");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let (entries, note) = desktop().await;
    assert!(note.contains("is not granted"), "{note}");
    exercise(&entries, &script, "KWin scripting (no grant)").await;

    // The grant: a .desktop naming this binary, written while KWin runs.
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let apps = PathBuf::from(std::env::var("XDG_DATA_HOME").unwrap()).join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("kai-live-test.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Kai live test\nNoDisplay=true\nExec={}\n\
             X-KDE-Wayland-Interfaces={}\n",
            exe.display(),
            super::plasma::WINDOW_MANAGEMENT
        ),
    )
    .unwrap();
    let written = Instant::now();
    loop {
        match Wm::start(T) {
            Ok(_) => break,
            Err(e) => {
                assert!(
                    written.elapsed() < Duration::from_secs(15),
                    "no grant: {e:?}"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    println!(
        "granted {} ms after the .desktop was written",
        written.elapsed().as_millis()
    );
    let (entries, note) = desktop().await;
    assert!(note.starts_with("the window management protocol"), "{note}");
    exercise(&entries, &script, "the window management protocol").await;
    let (_, err) = tool(&entries, "window_close", json!({"window": "Live Three"})).await;
    assert!(!err);
    let end = Instant::now() + Duration::from_secs(5);
    while quiet_list(&entries).await.0.contains("Kai Live Three") {
        assert!(Instant::now() < end, "not closed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("  closed");
}
