//! The switch is a plain on/off (owner's decision, 2026-10-09: no pinning, the latest version
//! always runs): an edit after switching on takes effect; the dialog shows every file at the top of
//! the folder; an old pin reads as on; a switch whose folder is gone becomes `false`, so a new folder
//! of its name is off until the owner's look, and is listed to forget; a thread's approval does not
//! switch a routine on; a draft never takes a switched name, read at every write, and is not
//! switched on while its task runs; what `routine.toml` asks beyond confirm and the owner's level
//! is granted only by the dialog's box.

use std::collections::BTreeSet;

use serde_json::{Map, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::host::policy::Caller;
use crate::routines::{Standing, Trigger, def, on_trigger};

const GREET: &str = "description = \"Greet.\"\nkind = \"script\"\n";

fn enable_args(name: &str) -> Map<String, serde_json::Value> {
    let mut args = Map::new();
    args.insert("name".into(), json!(name));
    args.insert("on".into(), json!(true));
    args
}

/// `routine_enable {name, on: true}` by the owner, as the tray sends it.
fn enable(rig: &Rig, name: &str) -> tokio::task::JoinHandle<Reply> {
    let entry = rig.local.entry_any("routine_enable").unwrap();
    let local = rig.local.clone();
    let args = enable_args(name);
    tokio::spawn(async move {
        local
            .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
            .await
    })
}

async fn switch_on(rig: &mut Rig, name: &str) {
    let pending = enable(rig, name);
    rig.ask().await.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert!(!r.error, "{}", text(&r));
}

fn standing(rig: &Rig, name: &str) -> Standing {
    let s = rig.inner.state();
    let l = s.loaded.iter().find(|l| l.name == name).unwrap();
    s.standing(l)
}

#[tokio::test]
async fn an_edit_after_switching_on_takes_effect() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    switch_on(&mut rig, "greet").await;
    assert!(rig.names().contains(&"greet".to_owned()));
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(file.contains("greet = true"), "a plain true: {file}");
    // A changed script, routine.toml and a new file: still on, no second look.
    crate::desktop::fdo::launch::tests::script(
        &rig.home.routines().join("greet/run.sh"),
        "echo v2",
    );
    std::fs::write(
        rig.home.routines().join("greet/helper.sh"),
        "echo sourced\n",
    )
    .unwrap();
    std::fs::write(
        rig.home.routines().join("greet").join(def::FILE),
        format!("{GREET}read_only = true\n"),
    )
    .unwrap();
    rig.inner.rescan(Some(&rig.home.config_file()));
    assert_eq!(standing(&rig, "greet"), Standing::On);
    assert!(rig.names().contains(&"greet".to_owned()), "still offered");
    let entry = rig
        .local
        .entries()
        .into_iter()
        .find(|e| e.spec.name == "greet")
        .unwrap();
    // Even before the watch reads an edit, the run is the latest version.
    crate::desktop::fdo::launch::tests::script(
        &rig.home.routines().join("greet/run.sh"),
        "echo v3",
    );
    let r = (entry.handler)(crate::host::registry::Call::new(
        entry.spec,
        crate::host::registry::Args::default(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await;
    assert!(!r.error, "{}", text(&r));
    assert!(text(&r).contains("v3"), "{}", text(&r));
    assert!(
        rig.inner
            .tray_items(&rig.inner.state())
            .iter()
            .all(|t| t.note.is_none()),
        "no changed-since state in the tray"
    );
}

/// The dialog is built from the bytes it read; what changes while it is open is not a second
/// look (the latest version runs), and the switch is a plain true.
#[tokio::test]
async fn the_dialog_shows_the_bytes_it_read_and_the_switch_is_plain() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let pending = enable(&rig, "greet");
    let ask = rig.ask().await;
    assert!(ask.lines.contains(&"  echo hi".to_owned()));
    assert!(
        !ask.lines.iter().any(|l| l.starts_with("pin:")),
        "no pin line"
    );
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert!(!r.error, "{}", text(&r));
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(file.contains("greet = true"), "{file}");
    assert_eq!(standing(&rig, "greet"), Standing::On);
}

#[tokio::test]
async fn the_dialog_shows_every_file_at_the_top_of_the_folder() {
    let home = Home::new();
    home.routine("greet", &format!("{GREET}script = \"main.sh\"\n"), None);
    let d = home.routines().join("greet");
    crate::desktop::fdo::launch::tests::script(&d.join("main.sh"), ". ./helper.sh");
    std::fs::write(d.join("helper.sh"), "echo from the helper\n").unwrap();
    std::fs::write(d.join("data.bin"), b"\x00\x01\x02binary").unwrap();
    std::fs::write(d.join(".hidden"), "export X=1\n").unwrap();
    std::fs::create_dir_all(d.join("state")).unwrap();
    // A pipe is named, never opened for a read that would wait.
    let fifo = std::ffi::CString::new(d.join("pipe").to_str().unwrap()).unwrap();
    // SAFETY: a path we own, a plain mode.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let pending = enable(&rig, "greet");
    let ask = rig.ask().await;
    let lines = ask.lines.clone();
    let at = |l: &str| {
        lines
            .iter()
            .position(|x| x == l)
            .unwrap_or_else(|| panic!("{l:?} not in {lines:?}"))
    };
    // routine.toml first, then the entry script, then the rest by name.
    assert!(at("routine.toml:") < at("main.sh:"));
    assert!(at("main.sh:") < at(".hidden:"));
    at("  . ./helper.sh");
    at("helper.sh:");
    at("  echo from the helper");
    at("  export X=1");
    let sha = def::read_folder(&rig.home.routines().join("greet"), |_| false)
        .unwrap()
        .entries
        .into_iter()
        .find_map(|e| match (e.name.as_str(), e.kind) {
            ("data.bin", def::EntryKind::File { sha256, .. }) => Some(sha256),
            _ => None,
        })
        .unwrap();
    at(&format!(
        "data.bin: not text, 9 bytes, sha256 {sha} (not shown)"
    ));
    at("state/: a subfolder, not shown (a script may keep its state there)");
    at("pipe: a pipe, not read");
    ask.reply.send(true).unwrap();
    assert!(!pending.await.unwrap().error);
    assert_eq!(standing(&rig, "greet"), Standing::On);
}

#[tokio::test]
async fn an_old_pin_or_true_reads_as_on() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    home.routine("older", GREET, Some("echo hi"));
    std::fs::write(
        home.config_file(),
        format!(
            "[routines.enabled]\ngreet = true\nolder = \"sha256:{}\"\n",
            "0".repeat(64)
        ),
    )
    .unwrap();
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    for name in ["greet", "older"] {
        assert_eq!(standing(&rig, name), Standing::On, "{name}");
        assert!(rig.names().contains(&name.to_owned()), "{name}");
    }
}

#[tokio::test]
async fn a_stale_switch_is_listed_to_forget() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    home.switch_on("greet");
    home.switch_on("gone");
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    assert!(rig.names().contains(&"greet".to_owned()));
    // The folder goes: the switch stays, and is listed to forget while no folder is there.
    std::fs::remove_dir_all(rig.home.routines().join("greet")).unwrap();
    rig.inner.rescan(Some(&rig.home.config_file()));
    assert!(!rig.names().contains(&"greet".to_owned()));
    assert!(rig.inner.state().orphans().contains(&"greet".to_owned()));
    // A switch without a folder: listed to forget, and forgotten.
    let items = rig.inner.tray_items(&rig.inner.state());
    let gone = items.iter().find(|t| t.name == "gone").unwrap();
    assert!(gone.orphan && !gone.on && !gone.runnable, "{gone:?}");
    assert!(items.iter().find(|t| t.name == "greet").unwrap().orphan);
    let said = on_trigger(&rig.local, &rig.inner, Trigger::Forget("gone".into())).await;
    assert_eq!(said, None);
    assert!(!rig.inner.state().switches.has_switch("gone"));
    assert!(
        rig.inner
            .tray_items(&rig.inner.state())
            .iter()
            .all(|t| t.name != "gone")
    );
}

/// F's approval in a thread saw `routine_enable`'s arguments, not the routine: Kai still asks.
#[tokio::test]
async fn a_threads_approval_does_not_switch_a_routine_on() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let entry = rig.local.entry_any("routine_enable").unwrap();
    let local = rig.local.clone();
    let pending = tokio::spawn(async move {
        local
            .call_approved_by(
                &entry,
                Some(enable_args("greet")),
                &Caller::Owner,
                &Caller::Owner,
            )
            .await
    });
    let ask = rig.ask().await;
    assert!(
        ask.lines.contains(&"  echo hi".to_owned()),
        "the routine is shown"
    );
    ask.reply.send(false).unwrap();
    let r = pending.await.unwrap();
    assert!(r.error && text(&r).contains("denied"), "{}", text(&r));
    assert_eq!(standing(&rig, "greet"), Standing::Off);
}

#[test]
fn a_draft_never_takes_a_switched_name() {
    use kai_delegate::Tools;
    let home = Home::new();
    let switched = BTreeSet::from(["backup".to_owned()]);
    let tools = crate::routines::create::DraftTools::new(
        home.routines(),
        Box::new(move |n| switched.contains(n)),
        Default::default(),
    );
    let e = tools
        .check(
            "draft_write",
            &json!({"path": "backup/run.sh", "content": "echo"}),
        )
        .unwrap_err();
    assert!(e.contains("routines.enabled.backup"), "{e}");
    assert!(
        tools
            .check(
                "draft_write",
                &json!({"path": "fresh/run.sh", "content": "echo"}),
            )
            .is_ok()
    );
}

/// The orphan's `true` becomes `false` when a rescan finds its folder gone: a new folder of that
/// name (anyone's) is off until the owner switched it on through the dialog.
#[tokio::test]
async fn a_new_folder_under_an_orphans_name_is_not_on() {
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    home.switch_on("greet");
    home.switch_on("gone");
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    assert_eq!(standing(&rig, "greet"), Standing::On);
    let file = || std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(file().contains("gone = false"), "{}", file());
    std::fs::remove_dir_all(rig.home.routines().join("greet")).unwrap();
    rig.inner.rescan(Some(&rig.home.config_file()));
    assert!(file().contains("greet = false"), "{}", file());
    assert!(
        rig.inner.state().switches.has_switch("greet"),
        "the name stays taken"
    );
    assert!(
        rig.inner
            .tray_items(&rig.inner.state())
            .iter()
            .any(|t| t.name == "greet" && t.orphan),
        "listed to forget"
    );
    // A new folder of that name: off, not offered, until the dialog.
    rig.home
        .routine("greet", GREET, Some("echo someone else's"));
    rig.home.routine("gone", GREET, Some("echo back"));
    rig.inner.rescan(Some(&rig.home.config_file()));
    for name in ["greet", "gone"] {
        assert_eq!(standing(&rig, name), Standing::Off, "{name}");
        assert!(!rig.names().contains(&name.to_owned()), "{name}");
    }
    let pending = enable(&rig, "greet");
    let ask = rig.ask().await;
    assert!(ask.lines.contains(&"  echo someone else's".to_owned()));
    ask.reply.send(true).unwrap();
    assert!(!pending.await.unwrap().error);
    assert_eq!(standing(&rig, "greet"), Standing::On);
    // The tools may write such a false (it is off); never a true.
    let e = crate::settings::apply_from(
        &rig.home.config_file(),
        crate::settings::Request::Set {
            key: "routines.enabled.gone".into(),
            value: "true".into(),
        },
        crate::settings::Source::Tool,
    )
    .unwrap_err();
    assert!(e.0.contains("routine_enable"), "{e}");
}

/// `routine_create` reads the switches at every write: a draft the owner switched on meanwhile
/// takes no more writes. And routine_enable does not switch on a name a running task writes.
#[tokio::test]
async fn a_draft_is_not_switched_on_while_its_task_writes_it() {
    use kai_delegate::Tools;
    let home = Home::new();
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let live = rig.inner.clone();
    let tools = crate::routines::create::DraftTools::new(
        rig.home.routines(),
        Box::new(move |n| live.state().switches.has_switch(n)),
        rig.inner.drafting.clone(),
    );
    let write = |path: &str, content: &str| {
        let step = tools
            .check("draft_write", &json!({"path": path, "content": content}))
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(step)
    };
    for (path, content) in [
        ("greet/routine.toml", GREET),
        ("greet/run.sh", "#!/bin/bash\necho draft\n"),
    ] {
        let step = write(path, content).unwrap();
        let out = tools.call(&step, &CancellationToken::new()).await;
        assert!(!out.error, "{:?}", out.parts);
    }
    rig.inner.rescan(Some(&rig.home.config_file()));
    assert!(rig.inner.drafting.contains("greet"));
    // While the task runs: refused before any dialog.
    let r = enable(&rig, "greet").await.unwrap();
    assert!(
        r.error && text(&r).contains("routine_create is still writing greet"),
        "{}",
        text(&r)
    );
    assert!(rig.asks.try_recv().is_err(), "nobody was asked");
    // A switch written meanwhile (the file, the tray of another Kai): the next write is refused.
    rig.home.switch_on("greet");
    rig.inner.rescan(Some(&rig.home.config_file()));
    let e = write("greet/run.sh", "#!/bin/bash\necho replaced\n").unwrap_err();
    assert!(e.contains("routines.enabled.greet"), "{e}");
    // The task ends: its names are free to switch on.
    drop(tools);
    assert!(!rig.inner.drafting.contains("greet"));
    let pending = enable(&rig, "greet");
    rig.ask().await.reply.send(true).unwrap();
    assert!(!pending.await.unwrap().error);
}

const ASKING: &str = "description = \"Greet.\"\nkind = \"script\"\nread_only = true\n";

/// What `routine.toml` asks beyond confirm and the owner's level: shown, and granted only when the
/// owner ticks the dialog's box (as the owner: the policy follows at once).
#[tokio::test]
async fn what_a_routine_asks_is_granted_only_by_the_box() {
    let home = Home::new();
    home.routine("greet", ASKING, Some("echo hi"));
    home.routine("plain", ASKING, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let cli = Caller::Named(crate::host::settings::Named::cli());
    let decide = |rig: &Rig, name: &str, caller: &Caller| {
        let spec = crate::routines::def::load(&rig.home.routines().join(name))
            .routine
            .unwrap()
            .spec;
        rig.policy.read().unwrap().decide(spec, caller, None)
    };
    // Not granted: the box left alone.
    let pending = enable(&rig, "plain");
    let ask = rig.ask().await;
    // After the arguments, before the folder.
    let at = |prefix: &str| ask.lines.iter().position(|l| l.starts_with(prefix));
    assert!(
        at("routine.toml asks for more than Kai gives") < at("routine.toml:"),
        "{:?}",
        ask.lines
    );
    assert!(at("routine.toml asks").is_some());
    assert!(
        ask.lines
            .iter()
            .any(|l| l.contains("to run unasked for trusted callers"))
    );
    assert!(
        ask.lines
            .iter()
            .any(|l| l.contains("level = \"any\", or no level"))
    );
    // One box per grant, so the one is given without the other.
    let labels: Vec<&str> = ask.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Also grant: policy.tools.plain = \"allow_trusted\"",
            "Also grant: policy.levels.plain = \"any\""
        ]
    );
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert_eq!(text(&r), "plain is on");
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(!file.contains("[policy"), "{file}");
    // Kai honours confirm and the owner's level: the owner is asked, cli is not admitted.
    assert!(matches!(
        decide(&rig, "plain", &Caller::Owner),
        crate::host::policy::Decision::Confirm { .. }
    ));
    assert!(matches!(
        decide(&rig, "plain", &cli),
        crate::host::policy::Decision::Deny(_)
    ));
    // Granted: the box ticked, then Allow.
    let pending = enable(&rig, "greet");
    let ask = rig.ask().await;
    assert_eq!(ask.options.len(), 2);
    for o in &ask.options {
        o.ticked.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert!(!r.error, "{}", text(&r));
    assert_eq!(
        text(&r),
        "greet is on; granted policy.tools.greet = \"allow_trusted\", policy.levels.greet = \"any\""
    );
    let c = crate::config::Config::load(&rig.home.config_file()).unwrap();
    assert_eq!(
        c.policy.tools["greet"],
        crate::host::settings::ToolState::AllowTrusted
    );
    assert_eq!(
        c.policy.levels["greet"],
        crate::host::settings::LevelSetting::Any
    );
    assert_eq!(
        decide(&rig, "greet", &Caller::Owner),
        crate::host::policy::Decision::Allow
    );
    assert!(matches!(
        decide(&rig, "greet", &cli),
        crate::host::policy::Decision::Confirm { .. }
    ));
    // Switched on again: the owner's entries stand, nothing is offered for them.
    rig.inner.switch("greet", false).await.unwrap();
    let pending = enable(&rig, "greet");
    let ask = rig.ask().await;
    assert!(ask.options.is_empty(), "{:?}", ask.options);
    assert!(
        ask.lines
            .iter()
            .any(|l| l.contains("your own policy.tools"))
    );
    ask.reply.send(true).unwrap();
    assert!(!pending.await.unwrap().error);
}

/// Each grant has its own box: only the ticked one is written.
#[tokio::test]
async fn each_grant_has_its_own_box() {
    let home = Home::new();
    home.routine("plain", ASKING, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let pending = enable(&rig, "plain");
    let ask = rig.ask().await;
    assert_eq!(ask.options.len(), 2);
    // The level's box only.
    ask.options[1]
        .ticked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert_eq!(
        text(&r),
        "plain is on; granted policy.levels.plain = \"any\""
    );
    let c = crate::config::Config::load(&rig.home.config_file()).unwrap();
    assert!(
        !c.policy.tools.contains_key("plain"),
        "{:?}",
        c.policy.tools
    );
    assert_eq!(
        c.policy.levels["plain"],
        crate::host::settings::LevelSetting::Any
    );
}

/// The grants and the switch go together: when the switch is not written, the grants written
/// before it are taken back and the routine stays off.
#[tokio::test]
async fn a_failed_switch_takes_the_grants_back() {
    let home = Home::new();
    home.routine("plain", ASKING, Some("echo hi"));
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    // The core's role: the file for every write, but the switch is refused.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let file = rig.home.config_file();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            if let crate::core::Command::Setting { request, reply } = cmd {
                let answer = match request {
                    crate::settings::Request::Routine { .. } => {
                        Err(crate::settings::Refusal("the switch was refused".into()))
                    }
                    r => crate::settings::apply(&file, r).map(|o| o.response),
                };
                let _ = reply.send(answer);
            }
        }
    });
    rig.run_with_settings(None, None, crate::routines::Settings::Core(tx));
    let pending = enable(&rig, "plain");
    let ask = rig.ask().await;
    for o in &ask.options {
        o.ticked.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(r.error, "{} / {file}", text(&r));
    assert!(text(&r).contains("were taken back"), "{}", text(&r));
    assert!(!file.contains("allow_trusted"), "{file}");
    assert!(!file.contains("plain = "), "{file}");
    assert_ne!(standing(&rig, "plain"), Standing::On);
}

/// A folder that cannot be read concludes nothing: no switch becomes false because of it.
#[tokio::test]
async fn an_unreadable_routines_folder_switches_nothing_off() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    home.routine("greet", GREET, Some("echo hi"));
    home.switch_on("greet");
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    assert_eq!(standing(&rig, "greet"), Standing::On);
    let dir = rig.home.routines();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = std::fs::read_dir(&dir).is_err();
    rig.inner.rescan(Some(&rig.home.config_file()));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !unreadable {
        return; // root: the permission does not bite
    }
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(!file.contains("greet = false"), "{file}");
    assert_eq!(standing(&rig, "greet"), Standing::On, "kept as it was");
    // A real removal still becomes false.
    std::fs::remove_dir_all(dir.join("greet")).unwrap();
    rig.inner.rescan(Some(&rig.home.config_file()));
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(file.contains("greet = false"), "{file}");
}

/// `allow_trusted` on routine_enable does not let a trusted caller switch a routine on unasked;
/// only `allow` skips the dialog.
#[test]
fn allow_trusted_does_not_skip_the_switch_on_dialog() {
    use crate::host::policy::Decision;
    use crate::host::registry::Args;
    use crate::host::settings::ToolState;
    use kai_desktop::contract::routines::ENABLE;
    let spec = kai_desktop::contract::find(ENABLE).unwrap().1;
    let args = |on: bool| Args::read(spec, Some(enable_args_on("x", on))).unwrap();
    let policy = |state| {
        let mut c = PolicyConfig::default();
        c.tools.insert(ENABLE.into(), state);
        // The gateway reaches it only where the owner widened the level.
        c.levels
            .insert(ENABLE.into(), crate::host::settings::LevelSetting::Any);
        Policy::new(&c)
    };
    let gateway = Caller::Gateway;
    let p = policy(ToolState::AllowTrusted);
    assert!(matches!(
        p.decide_call(spec, &args(true), &gateway, None),
        Decision::Confirm { .. }
    ));
    assert!(matches!(
        p.decide_call(spec, &args(true), &Caller::Owner, None),
        Decision::Confirm { .. }
    ));
    // Off is switching off: nobody is asked.
    assert_eq!(
        p.decide_call(spec, &args(false), &gateway, None),
        Decision::Allow
    );
    assert_eq!(
        policy(ToolState::Allow).decide_call(spec, &args(true), &gateway, None),
        Decision::Allow
    );
}

fn enable_args_on(name: &str, on: bool) -> Map<String, serde_json::Value> {
    let mut args = enable_args(name);
    args.insert("on".into(), json!(on));
    args
}

/// A routine's confirmation says what is true of it.
#[test]
fn a_routines_dialog_reason_is_its_own() {
    use crate::host::policy::Decision;
    let home = Home::new();
    home.routine("plain", ASKING, Some("echo hi"));
    let spec = def::load(&home.routines().join("plain"))
        .routine
        .unwrap()
        .spec;
    let d = Policy::new(&PolicyConfig::default()).decide(spec, &Caller::Owner, None);
    assert_eq!(
        d,
        Decision::Confirm {
            why: "routine.toml asks at most for confirmation (K28)"
        }
    );
}
