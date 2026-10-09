//! WP15's live check in a nested KWin (scripts/nested-kwin.sh, run by scripts/live-check-read.sh):
//! its own bus, XDG directories and home, test windows only; nothing shows on the session's screen
//! and nothing of the session's (its KWin, clipboard, accessibility bus, file index) is read.
//! Ignored by default; it refuses to run outside the nested session (WP14's guard).
//!
//! The steps: a capture without the grant (§11.1's error); a `.desktop` naming this binary with
//! `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`, written while KWin runs; then
//! `see_window` (a GTK window of a known colour, sampled) and `see_screen` on both outputs, each
//! PNG saved as a crop in `KAI_LIVE_OUT`; `read_window` and `read_selection` over a private AT-SPI
//! bus (a GTK window, a Qt dialog); `find_files` over a private Baloo index of test files, with
//! the indexer running and stopped; `recent_files` from a list written in the nested data
//! directory.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::desktop::fdo::atspi::switch::{A11ySwitch, SwitchMode, SwitchSettings};
use crate::desktop::live_tests::guard;
use crate::desktop::settings::DesktopConfig;
use crate::desktop::{DesktopTools, Env, SystemBus};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Changes, Entry, Provider, SessionBus};

const T: Duration = Duration::from_secs(5);

/// A GTK 3 window of colour rgb(30,160,60) with a label and an entry whose first five characters
/// are selected, app id `kai-live`.
const GTK_WINDOW: &str = "import sys, gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import GLib, Gtk, Gdk
GLib.set_prgname('kai-live')
css = Gtk.CssProvider()
css.load_from_data(b'window { background-color: rgb(30,160,60); }')
Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), css, 600)
w = Gtk.Window(title=sys.argv[1])
w.set_default_size(640, 400)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
box.pack_start(Gtk.Label(label='Kai reads this label'), False, False, 0)
e = Gtk.Entry()
e.set_text('typed in the entry')
box.pack_start(e, False, False, 0)
w.add(box)
w.connect('destroy', Gtk.main_quit)
w.show_all()
def sel():
    e.grab_focus()
    e.select_region(0, 5)
    return False
GLib.timeout_add(500, sel)
Gtk.main()";

/// Every process the check starts, killed and reaped however it ends.
struct Started(Vec<Child>);

impl Drop for Started {
    /// SIGTERM first, so at-spi-bus-launcher stops the bus it started; SIGKILL after a second.
    fn drop(&mut self) {
        for c in &mut self.0 {
            // SAFETY: a plain signal to a child of ours that has not been reaped yet.
            unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGTERM) };
        }
        let end = Instant::now() + Duration::from_secs(1);
        for c in &mut self.0 {
            while Instant::now() < end && matches!(c.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = c.kill();
            let _ = c.wait();
            crate::desktop::resolve::stand_in_gone(c.id());
        }
    }
}

fn start(cmd: &str, args: &[&str], env: &[(&str, &str)], accessible: bool) -> Child {
    let mut c = Command::new(cmd);
    c.args(args)
        .env("GDK_BACKEND", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if accessible {
        c.env_remove("NO_AT_BRIDGE");
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let child = c.spawn().unwrap_or_else(|e| panic!("{cmd}: {e}"));
    if accessible {
        // A test window stands in for an app of the owner's (`resolve::STAND_INS`), so the window
        // actions do not refuse it as one of Kai's own.
        crate::desktop::resolve::STAND_INS
            .lock()
            .unwrap()
            .push(child.id());
    }
    child
}

fn env_read() -> crate::desktop::read::ReadEnv {
    crate::desktop::read::ReadEnv::from_env(
        &crate::desktop::read::SearchConfig::default(),
        &crate::desktop::read::AccessibilityConfig::default(),
    )
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
        read: env_read(),
        ..Env::inert()
    }
}

async fn desktop() -> Vec<Entry> {
    desktop_with(None).await
}

/// The tools with Kai's accessibility switch `a11y` (`None`: none, as the unit tests' providers).
async fn desktop_with(a11y: Option<A11ySwitch>) -> Vec<Entry> {
    let d = DesktopTools::new(
        Env { a11y, ..env() },
        DesktopConfig::default(),
        Changes::new().0,
        false,
    );
    d.probe_now().await;
    for line in crate::desktop::probe::log_lines(&d.chosen()) {
        println!("table {line}");
    }
    d.entries()
}

async fn tool(entries: &[Entry], name: &str, args: Value) -> (Vec<Block>, bool) {
    let e = entries
        .iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    let shown = args.to_string();
    let started = Instant::now();
    let reply = (e.handler)(Call::new(
        e.spec,
        Args::read(e.spec, args.as_object().cloned()).unwrap(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await;
    let said: Vec<String> = reply
        .blocks
        .iter()
        .map(|b| match b {
            Block::Text(t) => t.replace('\n', " | "),
            Block::Image { mime, base64 } => format!("[{mime}, {} base64 bytes]", base64.len()),
        })
        .collect();
    println!(
        "{name} {shown} -> {} in {} ms: {}",
        if reply.error { "error" } else { "ok" },
        started.elapsed().as_millis(),
        said.join(" ")
    );
    (reply.blocks, reply.error)
}

async fn text(entries: &[Entry], name: &str, args: Value) -> (String, bool) {
    let (blocks, err) = tool(entries, name, args).await;
    match &blocks[..] {
        [Block::Text(t)] => (t.clone(), err),
        other => panic!("{other:?}"),
    }
}

/// The image of a capture's reply, saved as `<out>/<name>.png`: its size and its pixels as RGB(A).
fn save(out: &Path, name: &str, blocks: &[Block]) -> (u32, u32, usize, Vec<u8>) {
    use base64::Engine;
    let [Block::Text(_), Block::Image { base64, .. }] = blocks else {
        panic!("{blocks:?}");
    };
    let png = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap();
    std::fs::write(out.join(format!("{name}.png")), &png).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    (info.width, info.height, channels, buf)
}

async fn until<F: std::future::Future<Output = bool>>(
    what: &str,
    secs: u64,
    mut f: impl FnMut() -> F,
) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f().await {
        assert!(Instant::now() < end, "{what}: not within {secs} s");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a nested KWin only: scripts/live-check-read.sh"]
async fn live_read_in_nested_kwin() {
    let dir = match guard() {
        Ok(d) => d,
        Err(e) => {
            println!("skipped: {e}");
            return;
        }
    };
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    assert!(home.starts_with(&dir), "HOME is the nested session's");
    let data = PathBuf::from(std::env::var("XDG_DATA_HOME").unwrap());
    let config = PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap());
    let mut started = Started(Vec::new());

    // The private accessibility bus first, so the test apps join it (WP13 (a)'s pre-test).
    started.0.push(start(
        "/usr/libexec/at-spi-bus-launcher",
        &[],
        &[("GSETTINGS_BACKEND", "memory")],
        false,
    ));
    let session = zbus::Connection::session().await.unwrap();
    until("org.a11y.Bus", 10, || {
        let s = session.clone();
        async move {
            crate::desktop::has_owner(&s, "org.a11y.Bus", T)
                .await
                .unwrap_or(false)
        }
    })
    .await;
    started
        .0
        .push(start("/usr/libexec/at-spi2-registryd", &[], &[], false));
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Test files, Baloo's configuration for them, and a recent list.
    let docs = home.join("docs");
    std::fs::create_dir_all(docs.join("sub")).unwrap();
    std::fs::write(docs.join("quarterly-report.txt"), "The zebra budget.\n").unwrap();
    std::fs::write(docs.join("notes.md"), "zebra crossing\n").unwrap();
    std::fs::write(docs.join("sub/deep-zebra.txt"), "a zebra below\n").unwrap();
    std::fs::write(docs.join("other.txt"), "nothing here\n").unwrap();
    std::fs::write(
        config.join("baloofilerc"),
        format!(
            "[Basic Settings]\nIndexing-Enabled=true\n\n[General]\nfolders[$e]={}/\n\
             exclude folders[$e]=\nonly basic indexing=false\n",
            docs.display()
        ),
    )
    .unwrap();
    std::fs::write(
        data.join("recently-used.xbel"),
        format!(
            "<?xml version=\"1.0\"?>\n<xbel version=\"1.0\" \
             xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\">\n\
             <bookmark href=\"file://{}\" added=\"2026-10-09T08:00:00Z\" \
             modified=\"2026-10-09T08:00:00Z\" visited=\"2026-10-09T08:01:00Z\"><info><metadata \
             owner=\"http://freedesktop.org\"><bookmark:applications><bookmark:application \
             name=\"kate\" exec=\"kate %u\" modified=\"2026-10-09T08:01:00Z\" count=\"1\"/>\
             </bookmark:applications></metadata></info></bookmark>\n</xbel>\n",
            docs.join("notes.md").display()
        ),
    )
    .unwrap();
    let indexer = start("/usr/libexec/kf6/baloo_file", &[], &[], false);
    let indexer_pid = indexer.id();
    started.0.push(indexer);

    started.0.push(start(
        "python3",
        &["-I", "-c", GTK_WINDOW, "Kai Live See"],
        &[],
        true,
    ));
    started.0.push(start(
        "kdialog",
        &["--title", "Kai Live Qt", "--inputbox", "Qt asks", "qt text"],
        &[("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")],
        true,
    ));

    let entries = desktop().await;
    until("the test windows", 15, || {
        let entries = entries.clone();
        async move {
            let (list, _) = text(&entries, "window_list", json!({})).await;
            list.contains("Kai Live See") && list.contains("Kai Live Qt")
        }
    })
    .await;

    println!("== capture without the grant");
    let (blocks, err) = tool(&entries, "see_window", json!({"window": "Kai Live See"})).await;
    assert!(err);
    assert!(
        matches!(&blocks[..], [Block::Text(t)] if t == "needs the installed Kai: KWin grants org.kde.KWin.ScreenShot2 per .desktop file"),
        "{blocks:?}"
    );

    println!("== capture with the grant");
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let apps = data.join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("kai-live-read.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Kai live read\nNoDisplay=true\nExec={}\n\
             X-KDE-DBUS-Restricted-Interfaces={}\n",
            exe.display(),
            crate::desktop::plasma::capture::SCREENSHOT
        ),
    )
    .unwrap();
    let written = Instant::now();
    let window = loop {
        let (blocks, err) = tool(&entries, "see_window", json!({"window": "Kai Live See"})).await;
        if !err {
            break blocks;
        }
        assert!(written.elapsed() < Duration::from_secs(15), "no grant");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    println!(
        "granted {} ms after the .desktop was written",
        written.elapsed().as_millis()
    );
    let (w, h, ch, px) = save(&out, "see_window", &window);
    let at = |x: u32, y: u32| {
        let o = (y as usize * w as usize + x as usize) * ch;
        (px[o], px[o + 1], px[o + 2])
    };
    let sample = at(w / 2, h * 3 / 4);
    println!("  see_window: {w}x{h}, {ch} channels, pixel at the centre's 3/4 height {sample:?}");
    assert_eq!(sample, (30, 160, 60), "the window's own colour");
    for o in ["Virtual-0", "Virtual-1"] {
        let (blocks, err) = tool(&entries, "see_screen", json!({"output": o})).await;
        assert!(!err);
        let (w, h, _, _) = save(&out, &format!("see_screen-{o}"), &blocks);
        assert_eq!((w, h), (1280, 720), "{o} at its real resolution");
    }
    let (blocks, err) = tool(&entries, "see_screen", json!({})).await;
    assert!(!err && matches!(&blocks[0], Block::Text(t) if t.starts_with("output Virtual-")));
    // `active` (2026-10-09) is the active window, as an omitted `window` is.
    let (omitted, err) = tool(&entries, "see_window", json!({})).await;
    assert!(!err);
    let (active, err) = tool(&entries, "see_window", json!({"window": "active"})).await;
    assert!(!err);
    assert!(
        matches!((&omitted[0], &active[0]), (Block::Text(a), Block::Text(b)) if a == b),
        "the same window"
    );

    println!("== accessibility");
    let (t, err) = text(&entries, "read_window", json!({"window": "Kai Live See"})).await;
    assert!(!err, "{t}");
    assert!(t.contains("label \"Kai reads this label\""), "{t}");
    assert!(t.contains("\"typed in the entry\""), "{t}");
    let (t, err) = text(&entries, "read_window", json!({"window": "Kai Live Qt"})).await;
    assert!(!err, "{t}");
    assert!(t.contains("Qt asks"), "{t}");
    let (_, err) = text(
        &entries,
        "window_activate",
        json!({"window": "Kai Live See"}),
    )
    .await;
    assert!(!err);
    let asked = Instant::now();
    loop {
        let (t, _) = text(&entries, "read_selection", json!({})).await;
        if t == "typed" {
            break;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "the selection: {t}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    println!("== the accessibility switch");
    // The owner's rule (2026-10-09): Kai switches org.a11y.Status IsEnabled on around a read and
    // puts it back; never ScreenReaderEnabled. The private launcher keeps the switches in memory
    // (GSETTINGS_BACKEND=memory): nothing of the session's GSettings is written.
    let status = |name: &'static str| {
        let s = session.clone();
        async move {
            crate::desktop::property::<bool>(
                &s,
                "org.a11y.Bus",
                "/org/a11y/bus",
                "org.a11y.Status",
                name,
                T,
            )
            .await
            .unwrap()
        }
    };
    assert!(
        !status("IsEnabled").await,
        "the private launcher starts with it off"
    );
    // A Qt dialog that follows the switch (no QT_LINUX_ACCESSIBILITY_ALWAYS_ON).
    started.0.push(start(
        "kdialog",
        &[
            "--title",
            "Kai Live Switch",
            "--msgbox",
            "Qt follows the switch",
        ],
        &[],
        true,
    ));
    until("the switch's test window", 15, || {
        let entries = entries.clone();
        async move {
            let (list, _) = text(&entries, "window_list", json!({})).await;
            list.contains("Kai Live Switch")
        }
    })
    .await;
    let switch = |mode| {
        A11ySwitch::new(
            SessionBus::lazy(T),
            T,
            SwitchSettings {
                mode,
                wait: Duration::from_millis(5000),
            },
            Some(out.join("a11y-switched-on")),
        )
    };
    let never = desktop_with(Some(switch(SwitchMode::Never))).await;
    let (t, err) = text(&never, "read_window", json!({"window": "Kai Live Switch"})).await;
    assert!(err && t.contains("accessibility.switch = \"never\""), "{t}");
    let kais = switch(SwitchMode::Read);
    let reading = desktop_with(Some(kais.clone())).await;
    for round in 1..=2 {
        let began = Instant::now();
        let (t, err) = text(
            &reading,
            "read_window",
            json!({"window": "Kai Live Switch"}),
        )
        .await;
        assert!(!err, "{t}");
        assert!(t.contains("Qt follows the switch"), "{t}");
        println!(
            "switch read {round}: {} ms from the switch going on to the tree read (wait_ms 5000)",
            began.elapsed().as_millis()
        );
        assert!(!status("IsEnabled").await, "put back after read {round}");
    }
    assert!(!status("ScreenReaderEnabled").await);
    assert_eq!(
        kais.written(),
        [
            ("IsEnabled", true),
            ("IsEnabled", false),
            ("IsEnabled", true),
            ("IsEnabled", false)
        ],
        "only IsEnabled, on and back off around each read"
    );
    assert!(!out.join("a11y-switched-on").exists());

    println!("== files");
    let (t, err) = text(&entries, "recent_files", json!({})).await;
    assert!(!err && t.contains("notes.md (kate)"), "{t}");
    until("Baloo's index", 30, || {
        let entries = entries.clone();
        async move {
            let (t, _) = text(&entries, "find_files", json!({"query": "zebra"})).await;
            t.lines().filter(|l| l.starts_with('/')).count() == 3
        }
    })
    .await;
    let (t, _) = text(&entries, "find_files", json!({"query": "zebra"})).await;
    assert!(
        t.contains(&format!("indexed folders: {}", docs.display())),
        "{t}"
    );
    assert!(!t.contains("not running"), "{t}");
    let (t, _) = text(
        &entries,
        "find_files",
        json!({"query": "filename:quarterly*"}),
    )
    .await;
    assert!(
        t.starts_with(&docs.join("quarterly-report.txt").display().to_string()),
        "{t}"
    );
    // The indexer stopped: the result says so.
    let _ = Command::new("kill").arg(indexer_pid.to_string()).status();
    until("the indexer's end", 10, || {
        let s = session.clone();
        async move {
            !crate::desktop::has_owner(&s, "org.kde.baloo", T)
                .await
                .unwrap_or(true)
        }
    })
    .await;
    let (t, _) = text(&entries, "find_files", json!({"query": "zebra"})).await;
    assert!(t.contains("Baloo's indexer is not running"), "{t}");
    println!("done");
}
