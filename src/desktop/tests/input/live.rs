//! WP16c's live checks in a nested KWin (scripts/nested-kwin.sh, run by scripts/live-check-input.sh):
//! its own bus, XDG directories and home, test windows only. Input is injected into that KWin
//! alone; nothing of the session's is read or driven. Ignored by default; they refuse to run
//! outside the nested session (WP14's guard).
//!
//! 1. `live_input_fake_in_nested_kwin`: with no `.desktop` for this binary the plasma provider has
//!    no grant and, with no portal on the private bus, `type_text` and `key_press` are not listed
//!    (the table says why); a `.desktop` naming `org_kde_kwin_fake_input`, written while KWin runs,
//!    gives the grant. Then text typed into a GTK entry and a Qt (kdialog) entry, exact for
//!    Latin-1, symbols, CJK and an emoji, a thousand characters timed, key chords (select all,
//!    delete, tab, space), a call dropped half way (the typing stops), the focus moved to another
//!    window while Kai types (it stops, and says how far it got), and `ui_press` over a private
//!    AT-SPI bus on a GTK button, a check box and a Qt button.
//! 2. `live_input_portal_in_nested_kwin`: the RemoteDesktop portal (xdg-desktop-portal and
//!    portal-kde on the private bus) forced with `desktop.input = fdo`: the first use waits at the
//!    desktop's dialog and a dropped call leaves none open; with the app pre-authorised, text and
//!    keys go through, Kai stores the restore token (0600), and with the pre-authorisation revoked
//!    again the stored token alone is enough; a thousand characters timed (the pacing).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::desktop::live_tests::guard;
use crate::desktop::settings::{Choice, DesktopConfig};
use crate::desktop::{DesktopTools, Env, SystemBus};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Changes, Entry, Provider, SessionBus};

const T: Duration = Duration::from_secs(5);
const SAMPLE: &str = "Hallo Welt: äöü ß € ñ ç 日本 😀";

/// A GTK 3 window (app id `kai-live-input`): an entry that writes its text to `<out>/entry.txt`
/// at every change, a button (`clicked`, one `x` per press), a check box (`toggled`, `on`/`off`).
const GTK_WINDOW: &str = "import sys, gi
gi.require_version('Gtk', '3.0')
from gi.repository import GLib, Gtk
GLib.set_prgname('kai-live-input')
out = sys.argv[2]
w = Gtk.Window(title=sys.argv[1])
w.set_default_size(520, 300)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
e = Gtk.Entry()
e.connect('changed', lambda _: open(out + '/entry.txt', 'w').write(e.get_text()))
b = Gtk.Button(label='Press me')
b.connect('clicked', lambda _: open(out + '/clicked', 'a').write('x'))
c = Gtk.CheckButton(label='Check me')
c.connect('toggled', lambda _: open(out + '/toggled', 'w').write('on' if c.get_active() else 'off'))
for x in (e, b, c):
    box.pack_start(x, False, False, 4)
w.add(box)
w.connect('destroy', Gtk.main_quit)
w.show_all()
e.grab_focus()
Gtk.main()";

/// Every process the check starts, ended and reaped however it ends.
struct Started(Vec<Child>);

impl Drop for Started {
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

/// A process of the check: its windows stand in for the owner's apps (`resolve::STAND_INS`), so
/// the input tools do not refuse them as Kai's own (they are the test process's children).
fn start(cmd: &str, args: &[&str], env: &[(&str, &str)], stdout: Option<&Path>) -> Child {
    let mut c = Command::new(cmd);
    c.args(args)
        .env("GDK_BACKEND", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        .env_remove("NO_AT_BRIDGE")
        .stderr(Stdio::null());
    match stdout {
        Some(p) => c.stdout(std::fs::File::create(p).unwrap()),
        None => c.stdout(Stdio::null()),
    };
    for (k, v) in env {
        c.env(k, v);
    }
    let child = c.spawn().unwrap_or_else(|e| panic!("{cmd}: {e}"));
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
        read: crate::desktop::read::ReadEnv::from_env(
            &crate::desktop::read::SearchConfig::default(),
            &crate::desktop::read::AccessibilityConfig::default(),
        ),
        state_dir: PathBuf::from(std::env::var("XDG_STATE_HOME").unwrap()).join("kai"),
        ..Env::inert()
    }
}

async fn desktop_with(env: Env, config: DesktopConfig) -> Vec<Entry> {
    let d = DesktopTools::new(env, config, Changes::new().0, false);
    d.probe_now().await;
    for line in crate::desktop::probe::log_lines(&d.chosen()) {
        println!("table {line}");
    }
    d.entries()
}

fn listed(entries: &[Entry], name: &str) -> bool {
    entries.iter().any(|e| e.spec.name == name)
}

async fn tool(entries: &[Entry], name: &str, args: Value) -> (String, bool) {
    let e = entries
        .iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    // Never the typed text in the report.
    let shown = if name == "type_text" {
        "{text: …}".to_owned()
    } else {
        args.to_string()
    };
    let started = Instant::now();
    let reply = (e.handler)(Call::new(
        e.spec,
        Args::read(e.spec, args.as_object().cloned()).unwrap(),
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

async fn until<F: std::future::Future<Output = bool>>(
    what: &str,
    secs: u64,
    mut f: impl FnMut() -> F,
) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f().await {
        assert!(Instant::now() < end, "{what}: not within {secs} s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Waits until the file holds exactly `want` (the entry writes at every change), else panics with
/// its length (never the text).
async fn entry_is(out: &Path, want: &str, what: &str) {
    let file = out.join("entry.txt");
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let got = read(&file);
        if got == want {
            println!(
                "  {what}: the entry holds exactly the {} characters",
                want.chars().count()
            );
            return;
        }
        assert!(
            Instant::now() < end,
            "{what}: the entry holds {} characters, expected {}",
            got.chars().count(),
            want.chars().count()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn window_up(entries: &[Entry], title: &str) {
    until(title, 20, || async {
        tool(entries, "window_list", json!({}))
            .await
            .0
            .contains(title)
    })
    .await;
}

/// The nested session's directories, and the clean ones this check writes into.
fn dirs() -> (PathBuf, PathBuf, PathBuf) {
    let dir = guard().unwrap_or_else(|e| panic!("{e}"));
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let data = PathBuf::from(std::env::var("XDG_DATA_HOME").unwrap());
    assert!(
        data.starts_with(&dir),
        "XDG_DATA_HOME is the nested session's"
    );
    (dir, out, data)
}

fn thousand() -> String {
    "Ünïcødé quick brown fox 0123456789 €日本😀 "
        .chars()
        .cycle()
        .take(1000)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a nested KWin only: scripts/live-check-input.sh"]
async fn live_input_fake_in_nested_kwin() {
    if let Err(e) = guard() {
        println!("skipped: {e}");
        return;
    }
    let (_, out, data) = dirs();
    let work = out.join("fake");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let mut started = Started(Vec::new());

    println!("== no grant, no portal");
    let entries = desktop_with(env(), DesktopConfig::default()).await;
    assert!(!listed(&entries, "type_text") && !listed(&entries, "key_press"));
    // The table says why.
    let d = DesktopTools::new(env(), DesktopConfig::default(), Changes::new().0, false);
    d.probe_now().await;
    let note = d
        .chosen()
        .into_iter()
        .find(|c| c.capability == kai_desktop::caps::Capability::Input)
        .unwrap()
        .note;
    println!("note {note}");
    assert!(
        note.contains(
            "this build holds no grant: KWin grants org_kde_kwin_fake_input per .desktop file"
        ),
        "{note}"
    );
    assert!(
        note.contains("xdg-desktop-portal is neither running nor activatable"),
        "{note}"
    );

    // The private accessibility bus first, so the test apps join it.
    started.0.push(start(
        "/usr/libexec/at-spi-bus-launcher",
        &[],
        &[("GSETTINGS_BACKEND", "memory")],
        None,
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
        .push(start("/usr/libexec/at-spi2-registryd", &[], &[], None));
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("== the grant: a .desktop naming this binary, written while KWin runs");
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let apps = data.join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("kai-live-input.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Kai live input\nNoDisplay=true\nExec={}\n\
             X-KDE-Wayland-Interfaces={}\n",
            exe.display(),
            crate::desktop::plasma::input::FAKE_INPUT
        ),
    )
    .unwrap();
    let written = Instant::now();
    let entries = loop {
        let entries = desktop_with(env(), DesktopConfig::default()).await;
        if listed(&entries, "type_text") {
            break entries;
        }
        assert!(written.elapsed() < Duration::from_secs(20), "no grant");
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    println!(
        "granted {} ms after the .desktop was written",
        written.elapsed().as_millis()
    );

    let w = work.to_str().unwrap();
    started.0.push(start(
        "python3",
        &["-I", "-c", GTK_WINDOW, "Kai Live Input", w],
        &[],
        None,
    ));
    window_up(&entries, "Kai Live Input").await;
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Input"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("== type_text into a GTK entry");
    let (text, err) = tool(&entries, "type_text", json!({"text": SAMPLE})).await;
    assert!(!err, "{text}");
    entry_is(&work, SAMPLE, "sample").await;
    let (_, err) = tool(&entries, "key_press", json!({"keys": "ctrl+a BackSpace"})).await;
    assert!(!err);
    entry_is(&work, "", "select all, BackSpace").await;
    let long = thousand();
    let t0 = Instant::now();
    let (_, err) = tool(&entries, "type_text", json!({"text": long})).await;
    assert!(!err);
    println!(
        "  1000 characters in {} ms (the focus read through KWin scripting)",
        t0.elapsed().as_millis()
    );
    entry_is(&work, &long, "1000 characters").await;
    tool(&entries, "key_press", json!({"keys": "ctrl+a Delete"})).await;
    entry_is(&work, "", "select all, Delete").await;

    println!("== a call dropped half way stops the typing");
    let typing = thousand().repeat(3);
    let e = entries
        .iter()
        .find(|e| e.spec.name == "type_text")
        .unwrap()
        .clone();
    let args = Args::read(e.spec, json!({"text": typing}).as_object().cloned()).unwrap();
    let call = (e.handler)(Call::new(
        e.spec,
        args,
        Caller::Owner,
        CancellationToken::new(),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(600), call)
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let a = read(&work.join("entry.txt")).chars().count();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let b = read(&work.join("entry.txt")).chars().count();
    println!("  typed {a} characters when dropped plus 400 ms, {b} after another 800 ms of 3000");
    assert!(
        a > 0 && a < 3000 && a == b,
        "the typing stops: {a} then {b}"
    );
    tool(&entries, "key_press", json!({"keys": "ctrl+a Delete"})).await;
    entry_is(&work, "", "cleared").await;

    println!("== the focus moves while Kai types: it stops before the next character");
    let other = work.join("other");
    std::fs::create_dir_all(&other).unwrap();
    started.0.push(start(
        "python3",
        &[
            "-I",
            "-c",
            GTK_WINDOW,
            "Kai Live Other",
            other.to_str().unwrap(),
        ],
        &[],
        None,
    ));
    window_up(&entries, "Kai Live Other").await;
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Input"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let typing = thousand().repeat(3);
    let args = Args::read(e.spec, json!({"text": typing}).as_object().cloned()).unwrap();
    let call = tokio::spawn((e.handler)(Call::new(
        e.spec,
        args,
        Caller::Owner,
        CancellationToken::new(),
    )));
    tokio::time::sleep(Duration::from_millis(600)).await;
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Other"}),
    )
    .await;
    let reply = call.await.unwrap();
    let said: String = reply
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    println!("type_text {{text: …}} -> {said}");
    assert!(reply.error, "{said}");
    let n: usize = said
        .strip_prefix("focus moved to kai-live-input; ")
        .and_then(|r| r.strip_suffix(" of 3000 characters typed"))
        .unwrap_or_else(|| panic!("{said}"))
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let a = read(&work.join("entry.txt")).chars().count();
    let b = read(&other.join("entry.txt")).chars().count();
    println!("  the confirmed window holds {a}, the other {b}; Kai typed {n}");
    assert!(n > 0 && n < 3000, "{n}");
    // The check and the character are two steps: one character may land in the new window.
    assert!(a + b == n && b <= 1, "{a} + {b} != {n}");
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Input"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    tool(&entries, "key_press", json!({"keys": "ctrl+a Delete"})).await;
    entry_is(&work, "", "cleared").await;

    println!("== keys: tab to the button, space presses it");
    tool(&entries, "key_press", json!({"keys": "tab space"})).await;
    until("the button", 5, || async {
        read(&work.join("clicked")) == "x"
    })
    .await;

    println!("== ui_press");
    let (text, err) = tool(
        &entries,
        "ui_press",
        json!({"element": "button \"Press me\""}),
    )
    .await;
    assert!(
        !err && text.starts_with("pressed button \"Press me\" in "),
        "{text}"
    );
    until("the second click", 5, || async {
        read(&work.join("clicked")) == "xx"
    })
    .await;
    let (text, err) = tool(&entries, "ui_press", json!({"element": "Check me"})).await;
    assert!(!err, "{text}");
    until("the check box", 5, || async {
        read(&work.join("toggled")) == "on"
    })
    .await;
    let (text, err) = tool(&entries, "ui_press", json!({"element": "me"})).await;
    assert!(err && text.starts_with("2 elements match"), "{text}");
    let (text, err) = tool(&entries, "ui_press", json!({"element": "Nothing like it"})).await;
    assert!(err && text.starts_with("no pressable element"), "{text}");

    println!("== a Qt entry (kdialog)");
    let qt_out = work.join("qt.out");
    let qt = start(
        "kdialog",
        &["--title", "Kai Live Qt", "--inputbox", "Qt asks", ""],
        &[("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")],
        Some(&qt_out),
    );
    started.0.push(qt);
    window_up(&entries, "Kai Live Qt").await;
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Qt"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let qt_text = format!("{SAMPLE} {}", thousand());
    let (_, err) = tool(&entries, "type_text", json!({"text": qt_text})).await;
    assert!(!err);
    let (_, err) = tool(&entries, "key_press", json!({"keys": "enter"})).await;
    assert!(!err);
    until("kdialog's answer", 10, || async {
        !read(&qt_out).is_empty()
    })
    .await;
    assert_eq!(
        read(&qt_out).trim_end_matches('\n'),
        qt_text,
        "Qt's entry is exact"
    );
    println!(
        "  Qt: the entry held exactly the {} characters",
        qt_text.chars().count()
    );

    println!("== ui_press on a Qt button");
    let ok_out = work.join("ok.out");
    let ok = start(
        "kdialog",
        &["--title", "Kai Live Qt OK", "--msgbox", "Press OK"],
        &[("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")],
        Some(&ok_out),
    );
    started.0.push(ok);
    window_up(&entries, "Kai Live Qt OK").await;
    let (text, err) = tool(
        &entries,
        "ui_press",
        json!({"window": "Kai Live Qt OK", "element": "OK"}),
    )
    .await;
    assert!(!err, "{text}");
    until("the message box to end", 10, || async {
        !tool(&entries, "window_list", json!({}))
            .await
            .0
            .contains("Kai Live Qt OK")
    })
    .await;
    println!("done");
}

/// The permission store's entry that lets `app` use the RemoteDesktop portal without the dialog.
async fn authorise(conn: &zbus::Connection, app: &str, yes: bool) {
    let allow = if yes { vec!["yes"] } else { vec!["no"] };
    conn.call_method(
        Some("org.freedesktop.impl.portal.PermissionStore"),
        "/org/freedesktop/impl/portal/PermissionStore",
        Some("org.freedesktop.impl.portal.PermissionStore"),
        "SetPermission",
        &("kde-authorized", true, "remote-desktop", app, allow),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a nested KWin only: scripts/live-check-input.sh"]
async fn live_input_portal_in_nested_kwin() {
    if let Err(e) = guard() {
        println!("skipped: {e}");
        return;
    }
    let (_, out, data) = dirs();
    let work = out.join("portal");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let mut started = Started(Vec::new());
    let app = "dev.rootshell.kai.live";
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let apps = data.join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    // The portal's registry refuses an app id without a .desktop file; this one has no grant.
    std::fs::write(
        apps.join(format!("{app}.desktop")),
        format!(
            "[Desktop Entry]\nType=Application\nName=Kai live\nNoDisplay=true\nExec={}\n",
            exe.display()
        ),
    )
    .unwrap();

    println!("== the portal stack on the private bus");
    let kde = [("XDG_CURRENT_DESKTOP", "KDE")];
    started
        .0
        .push(start("/usr/libexec/xdg-permission-store", &[], &kde, None));
    started.0.push(start(
        "/usr/libexec/xdg-desktop-portal-kde",
        &[],
        &kde,
        None,
    ));
    let session = zbus::Connection::session().await.unwrap();
    for name in [
        "org.freedesktop.impl.portal.PermissionStore",
        "org.freedesktop.impl.portal.desktop.kde",
    ] {
        until(name, 15, || {
            let s = session.clone();
            async move {
                crate::desktop::has_owner(&s, name, T)
                    .await
                    .unwrap_or(false)
            }
        })
        .await;
    }
    started
        .0
        .push(start("/usr/libexec/xdg-desktop-portal", &[], &kde, None));
    until("the portal", 15, || {
        let s = session.clone();
        async move {
            crate::desktop::has_owner(&s, "org.freedesktop.portal.Desktop", T)
                .await
                .unwrap_or(false)
        }
    })
    .await;

    // The portal learns the app id through the host registry, on the connection Kai's provider
    // uses (an installed Kai in its app scope needs none).
    let env = env();
    let conn = env.session.as_ref().unwrap().get().await.unwrap();
    ashpd::register_host_app_with_connection(conn, app.parse().unwrap())
        .await
        .unwrap();
    let state = env.state_dir.clone();
    let token = state.join(crate::desktop::fdo::input::TOKEN_NAME);
    let _ = std::fs::remove_file(&token);
    let config = DesktopConfig {
        input: Some(Choice::Fdo),
        ..Default::default()
    };
    let entries = desktop_with(env, config).await;
    assert!(listed(&entries, "type_text") && listed(&entries, "key_press"));

    let w = work.to_str().unwrap();
    started.0.push(start(
        "python3",
        &["-I", "-c", GTK_WINDOW, "Kai Live Portal", w],
        &[],
        None,
    ));
    window_up(&entries, "Kai Live Portal").await;
    tool(
        &entries,
        "window_activate",
        json!({"window": "Kai Live Portal"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("== not authorised: the first use waits at the desktop's dialog");
    let e = entries
        .iter()
        .find(|e| e.spec.name == "type_text")
        .unwrap()
        .clone();
    let args = Args::read(e.spec, json!({"text": SAMPLE}).as_object().cloned()).unwrap();
    let call = (e.handler)(Call::new(
        e.spec,
        args,
        Caller::Owner,
        CancellationToken::new(),
    ));
    let waited = tokio::time::timeout(Duration::from_secs(6), call).await;
    assert!(waited.is_err(), "the call waits for the owner");
    // The dropped call closed its session, which ends the desktop's dialog.
    until("the dialog to go", 10, || async {
        !tool(&entries, "window_list", json!({}))
            .await
            .0
            .contains("Remote Control")
    })
    .await;
    assert_eq!(
        read(&work.join("entry.txt")),
        "",
        "nothing typed without the owner"
    );
    assert!(!token.exists());

    println!("== pre-authorised (the owner's one-time answer)");
    authorise(&session, app, true).await;
    let (text, err) = tool(&entries, "type_text", json!({"text": SAMPLE})).await;
    assert!(!err, "{text}");
    entry_is(&work, SAMPLE, "portal, keysyms").await;
    let stored = std::fs::read_to_string(&token).unwrap_or_default();
    assert!(!stored.trim().is_empty(), "the restore token is stored");
    assert_eq!(
        std::fs::metadata(&token).unwrap().permissions().mode() & 0o777,
        0o600
    );
    println!(
        "  the restore token is stored ({} bytes, mode 0600)",
        stored.trim().len()
    );
    let (_, err) = tool(&entries, "key_press", json!({"keys": "ctrl+a BackSpace"})).await;
    assert!(!err);
    entry_is(&work, "", "portal, chords").await;

    let long = thousand();
    let t0 = Instant::now();
    let (_, err) = tool(&entries, "type_text", json!({"text": long})).await;
    assert!(!err);
    println!(
        "  portal: 1000 characters in {} ms",
        t0.elapsed().as_millis()
    );
    entry_is(&work, &long, "portal, 1000 characters").await;
    tool(&entries, "key_press", json!({"keys": "ctrl+a Delete"})).await;
    entry_is(&work, "", "portal, cleared").await;

    println!("== the pre-authorisation revoked: the stored token alone");
    authorise(&session, app, false).await;
    let (text, err) = tool(&entries, "type_text", json!({"text": "token works"})).await;
    assert!(!err, "{text}");
    entry_is(&work, "token works", "portal, restore token").await;

    println!("== no token and no authorisation: the dialog again");
    std::fs::remove_file(&token).unwrap();
    let args = Args::read(e.spec, json!({"text": "no"}).as_object().cloned()).unwrap();
    let call = (e.handler)(Call::new(
        e.spec,
        args,
        Caller::Owner,
        CancellationToken::new(),
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .is_err()
    );
    assert_eq!(read(&work.join("entry.txt")), "token works");
    println!("done");
}
