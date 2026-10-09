//! KWin scripting from Kai against a fake KWin on a private bus: the answer through Kai's own
//! object, the script unloaded and its file removed however the run ends, an answer from another
//! sender ignored, the timeout named.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use kai_desktop::caps::Failure;
use kai_desktop::caps::windows::{Action, Windows};
use serde_json::{Value, json};

use super::bus::{Kwin, Mode, PrivateBus, args_of, kwin, kwin_snapshot, temp_dir};
use crate::desktop::plasma::kwin_script::KwinScript;
use crate::desktop::plasma::windows::PlasmaWindows;
use crate::host::registry::SessionBus;

const T: Duration = Duration::from_secs(1);

struct Rig {
    bus: PrivateBus,
    kwin: Kwin,
    _kwin_conn: zbus::Connection,
    dir: tempfile::TempDir,
    script: KwinScript,
}

async fn rig() -> Option<Rig> {
    let bus = PrivateBus::start()?;
    let (conn, kwin) = kwin(&bus).await;
    let dir = temp_dir();
    let script = KwinScript::new(
        SessionBus::connected(bus.connect().await, T),
        dir.path().join("kwin"),
        T,
    );
    Some(Rig {
        bus,
        kwin,
        _kwin_conn: conn,
        dir,
        script,
    })
}

async fn unloaded(kwin: &Kwin, n: usize) -> Vec<String> {
    for _ in 0..50 {
        let u = kwin.lock().unwrap().unloaded.clone();
        if u.len() >= n {
            return u;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    kwin.lock().unwrap().unloaded.clone()
}

#[tokio::test]
async fn a_script_answers_through_kais_object_and_is_unloaded() {
    let Some(r) = rig().await else { return };
    let v = r
        .script
        .run("function run() { return 1; }", json!({}))
        .await
        .unwrap();
    assert_eq!(v, kwin_snapshot());
    let loaded = r.kwin.lock().unwrap().loaded.clone();
    assert_eq!(loaded.len(), 1);
    let (plugin, file, mode, source) = &loaded[0];
    assert_eq!(mode & 0o777, 0o600, "the script's file is the user's alone");
    let dir_mode = std::fs::metadata(r.dir.path().join("kwin"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    assert!(source.contains("function run() { return 1; }"));
    assert!(source.contains("callDBus(ARGS.reply.service"));
    assert_eq!(unloaded(&r.kwin, 1).await, std::slice::from_ref(plugin));
    assert!(!file.exists(), "the file is removed");
    // A second run has its own plugin name; its arguments are a literal for any JavaScript
    // engine (U+2028 and U+2029 escaped).
    let odd = "a\u{2028}b\u{2029}c\"</script>";
    r.script
        .run("function run() {}", json!({"x": odd}))
        .await
        .unwrap();
    let loaded = r.kwin.lock().unwrap().loaded.clone();
    assert_ne!(loaded[0].0, loaded[1].0);
    assert!(!loaded[1].3.contains('\u{2028}') && loaded[1].3.contains("a\\u2028b\\u2029c"));
    assert_eq!(args_of(&loaded[1].3)["x"], odd);
}

/// Runs from two instances at once take turns: KWin never holds two of Kai's scripts, so a load
/// cannot be numbered like one being unloaded.
#[tokio::test]
async fn runs_take_turns_across_instances() {
    let Some(r) = rig().await else { return };
    let other = KwinScript::new(
        SessionBus::connected(r.bus.connect().await, T),
        r.dir.path().join("kwin"),
        T,
    );
    let runs = (0..6).map(|i| {
        let s = if i % 2 == 0 { &r.script } else { &other };
        s.run("function run() {}", json!({}))
    });
    for done in futures_util::future::join_all(runs).await {
        done.unwrap();
    }
    let k = r.kwin.lock().unwrap();
    assert_eq!(k.loaded.len(), 6);
    assert_eq!(k.most_at_once, 1);
}

#[tokio::test]
async fn a_throwing_script_is_an_error_with_its_message() {
    let Some(r) = rig().await else { return };
    r.kwin.lock().unwrap().mode = Some(Mode::Throw("TypeError: w is null".into()));
    let e = r
        .script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e, Failure::Failed("KWin: TypeError: w is null".into()));
    assert_eq!(unloaded(&r.kwin, 1).await.len(), 1);
}

#[tokio::test]
async fn a_silent_script_times_out_named_and_is_still_unloaded() {
    let Some(r) = rig().await else { return };
    r.kwin.lock().unwrap().mode = Some(Mode::Silent);
    let e = r
        .script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        Failure::Failed("Kai's KWin script did not answer within 1 s (dbus.timeout_s)".into())
    );
    assert_eq!(unloaded(&r.kwin, 1).await.len(), 1);
    let file = r.kwin.lock().unwrap().loaded[0].1.clone();
    assert!(!file.exists());
}

/// A call cancelled while its script runs: the run's future is dropped, and the script is
/// unloaded anyway.
#[tokio::test]
async fn a_cancelled_run_still_unloads_its_script() {
    let Some(r) = rig().await else { return };
    r.kwin.lock().unwrap().mode = Some(Mode::Silent);
    let script = r.script.clone();
    let run = tokio::spawn(async move { script.run("function run() {}", json!({})).await });
    for _ in 0..50 {
        if !r.kwin.lock().unwrap().loaded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    run.abort();
    assert_eq!(unloaded(&r.kwin, 1).await.len(), 1);
}

/// Only KWin's own answer counts: another connection that answers first with the right id is
/// ignored.
#[tokio::test]
async fn an_answer_from_another_sender_is_ignored() {
    let Some(r) = rig().await else { return };
    r.kwin.lock().unwrap().mode = Some(Mode::Late(Duration::from_millis(300)));
    let script = r.script.clone();
    let run = tokio::spawn(async move { script.run("function run() {}", json!({})).await });
    let source = loop {
        if let Some(l) = r.kwin.lock().unwrap().loaded.first() {
            break l.3.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let reply = args_of(&source)["reply"].clone();
    let forger = r.bus.connect().await;
    forger
        .call_method(
            Some(reply["service"].as_str().unwrap()),
            reply["path"].as_str().unwrap(),
            Some(reply["iface"].as_str().unwrap()),
            "Result",
            &(
                reply["id"].as_str().unwrap(),
                json!({"ok": true, "value": "forged"}).to_string(),
            ),
        )
        .await
        .unwrap();
    let v: Value = run.await.unwrap().unwrap();
    assert_eq!(v, kwin_snapshot(), "KWin's answer, not the forger's");
}

/// KWin is the sender of whatever a script says, any client's included; the run's token is what
/// makes an answer Kai's.
#[tokio::test]
async fn an_answer_with_the_wrong_token_is_ignored() {
    let Some(r) = rig().await else { return };
    r.kwin.lock().unwrap().mode = Some(Mode::WrongToken);
    let e = r
        .script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        Failure::Failed("Kai's KWin script did not answer within 1 s (dbus.timeout_s)".into())
    );
    // The token is 128 random bits, a new one per run, and not in the plugin's name.
    r.kwin.lock().unwrap().mode = None;
    r.script.run("function run() {}", json!({})).await.unwrap();
    let loaded = r.kwin.lock().unwrap().loaded.clone();
    let tokens: Vec<String> = loaded
        .iter()
        .map(|l| args_of(&l.3)["reply"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_ne!(tokens[0], tokens[1]);
    for (t, l) in tokens.iter().zip(&loaded) {
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!l.0.contains(t.as_str()));
        assert!(l.0.starts_with(&format!("kai-{}-", std::process::id())));
    }
}

#[tokio::test]
async fn a_planted_symlink_is_refused_not_followed() {
    use std::os::unix::fs::symlink;
    let Some(r) = rig().await else { return };
    let elsewhere = r.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The script directory is a link to a directory of another mode: neither used nor changed.
    symlink(&elsewhere, r.dir.path().join("kwin")).unwrap();
    let e = r
        .script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Failure::Failed(m) if m.contains("not a real directory")),
        "{e:?}"
    );
    assert_eq!(
        std::fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(r.kwin.lock().unwrap().loaded.is_empty());
    // A directory that is Kai's but open to others is refused as well.
    std::fs::remove_file(r.dir.path().join("kwin")).unwrap();
    std::fs::create_dir(r.dir.path().join("kwin")).unwrap();
    std::fs::set_permissions(
        r.dir.path().join("kwin"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let e = r
        .script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Failure::Failed(m) if m.contains("0700")),
        "{e:?}"
    );
    // A file planted at a name is not written through: create_new fails on it.
    let victim = r.dir.path().join("victim");
    std::fs::write(&victim, "mine").unwrap();
    let dir = r.dir.path().join("kwin2");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&victim, dir.join("planted.js")).unwrap();
    assert!(super::super::plasma::kwin_script::write_private_for_test(&dir, "planted.js").is_err());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "mine");
}

/// A Kai that died with a script loaded leaves its file; the next one unloads and removes it, and
/// only that: not a live process's file, not what is not named like Kai's.
#[tokio::test]
async fn leftovers_of_dead_kai_processes_are_swept_and_only_those() {
    let Some(r) = rig().await else { return };
    let dir = r.dir.path().join("kwin");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    // A pid that is certainly gone: a child that was waited for.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    child.wait().unwrap();
    let dead = child.id();
    let own = std::process::id();
    let dead_file = format!("kai-{dead}-00ff00ff00ff00ff.js");
    let live_file = format!("kai-{own}-aabbccddaabbccdd.js");
    for name in [
        dead_file.as_str(),
        live_file.as_str(),
        "notes.js",
        "kai-x-00.js",
        "kai-1-zz.js",
    ] {
        std::fs::write(dir.join(name), "// x").unwrap();
    }
    r.script.run("function run() {}", json!({})).await.unwrap();
    let plugin = dead_file.trim_end_matches(".js").to_owned();
    let gone = unloaded(&r.kwin, 2).await;
    assert!(gone.contains(&plugin), "{gone:?}");
    assert!(
        !gone
            .iter()
            .any(|p| p.contains(&own.to_string()) && p.contains("aabbcc"))
    );
    assert!(!dir.join(&dead_file).exists());
    for kept in [live_file.as_str(), "notes.js", "kai-x-00.js", "kai-1-zz.js"] {
        assert!(dir.join(kept).exists(), "{kept}");
    }
}

#[tokio::test]
async fn without_kwin_a_script_says_so() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let dir = temp_dir();
    let script = KwinScript::new(
        SessionBus::connected(bus.connect().await, T),
        dir.path().to_owned(),
        T,
    );
    let e = script
        .run("function run() {}", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e, Failure::Failed("KWin is not running".into()));
}

/// The Windows capability over scripting: its snapshot, an action's word, a window that closed
/// meanwhile.
#[tokio::test]
async fn plasma_windows_over_scripting() {
    let Some(r) = rig().await else { return };
    let w = PlasmaWindows::new(
        None,
        r.script.clone(),
        SessionBus::connected(r.bus.connect().await, T),
        T,
    );
    let s = w.snapshot().await.unwrap();
    assert_eq!(s.windows.len(), 2);
    assert_eq!(s.activity, "", "KWin's null activity is no activity");
    assert_eq!(s.outputs[1].name, "Virtual-1");
    assert_eq!(
        w.act("w1", &Action::ToggleMaximize).await.unwrap(),
        "did maximize"
    );
    assert_eq!(
        w.act("gone", &Action::Close).await.unwrap_err(),
        Failure::Refused("the window closed meanwhile".into())
    );
    let loaded = r.kwin.lock().unwrap().loaded.clone();
    assert_eq!(args_of(&loaded[1].3)["window"], "w1");
    assert_eq!(args_of(&loaded[1].3)["action"], "maximize");
}
