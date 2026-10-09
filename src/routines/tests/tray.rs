//! The tray's and keys' runs (the launch tests' fake user manager): the result as a notification,
//! the tray's Stop, `routines.run_timeout_s`, and a result over a call's room as the run's error
//! naming the room.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::desktop::fdo::launch::settings::LaunchConfig;
use crate::host::local::LocalBounds;
use crate::host::policy::Caller;
use crate::routines::{Trigger, on_trigger};

async fn until_running(rig: &Rig, name: &str, running: bool) {
    let end = std::time::Instant::now() + T;
    loop {
        let now = rig
            .inner
            .tray_items(&rig.inner.state())
            .iter()
            .any(|t| t.name == name && t.running);
        if now == running {
            return;
        }
        assert!(std::time::Instant::now() < end, "{name} running: {now}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn run(rig: &Rig, name: &str) -> tokio::task::JoinHandle<Option<(String, String)>> {
    let (local, inner, name) = (rig.local.clone(), rig.inner.clone(), name.to_owned());
    tokio::spawn(async move { on_trigger(&local, &inner, Trigger::Run(name)).await })
}

#[tokio::test]
async fn a_tray_run_says_its_result_and_can_be_stopped_or_run_out_of_time() {
    let Some(launch) = crate::desktop::fdo::launch::tests::rig(LaunchConfig::default()).await
    else {
        return;
    };
    let home = Home::new();
    home.routine(
        "hello",
        "description = \"Say hello.\"\nkind = \"script\"\nread_only = true\n",
        Some("echo hello"),
    );
    // Each run writes its leader's pid (its own session: the group's id), so the test can see
    // that nothing of it outlives the stop or the deadline.
    let pids = home.dir.path().join("slow.pids");
    home.routine(
        "slow",
        "description = \"Take long.\"\nkind = \"script\"\nread_only = true\n",
        Some(&format!("echo $$ >> '{}'\nsleep 30", pids.display())),
    );
    home.switch_on("hello");
    home.switch_on("slow");
    let (rig, _) = rig_with(home, granted(PolicyConfig::default(), &["hello", "slow"]));
    rig.run_with(Some(launch.launch.manager.clone()));
    assert_eq!(
        run(&rig, "hello").await.unwrap(),
        Some(("Routine hello".to_owned(), "hello".to_owned()))
    );
    // Stop from the tray.
    let pending = run(&rig, "slow");
    until_running(&rig, "slow", true).await;
    assert_eq!(rig.inner.stop("slow"), 1);
    let (title, body) = tokio::time::timeout(T, pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(title, "Routine slow stopped");
    assert!(body.contains("stopped from the tray"), "{body}");
    until_running(&rig, "slow", false).await;
    assert_eq!(
        on_trigger(&rig.local, &rig.inner, Trigger::Stop("slow".into())).await,
        Some(("Routine slow".to_owned(), "it was not running".to_owned()))
    );
    // The deadline, read live from the configuration.
    crate::settings::apply(
        &rig.home.config_file(),
        crate::settings::Request::Set {
            key: "routines.run_timeout_s".into(),
            value: "1".into(),
        },
    )
    .unwrap();
    rig.inner.rescan(Some(&rig.home.config_file()));
    let started = std::time::Instant::now();
    let (title, body) = tokio::time::timeout(T, run(&rig, "slow"))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(title, "Routine slow stopped");
    assert!(body.contains("routines.run_timeout_s = 1 s"), "{body}");
    // Neither run's script nor its `sleep` is left: the group dies when the run ends, not in a
    // task the test's runtime would cancel as it shuts down.
    // (A run stopped before its script's first line, as under a loaded test run, wrote none.)
    let groups: Vec<i32> = std::fs::read_to_string(&pids)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().parse().unwrap())
        .collect();
    assert!(groups.len() <= 2, "{groups:?}");
    for pgid in groups {
        let end = std::time::Instant::now() + T;
        while group_alive(pgid) {
            assert!(
                std::time::Instant::now() < end,
                "process group {pgid} is still running"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// A process of group `pgid` still runs (a zombie waiting to be reaped runs nothing).
fn group_alive(pgid: i32) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            return false;
        };
        // `pid (comm) state ppid pgrp …`; comm may hold spaces and parentheses.
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
            return false;
        };
        let mut f = rest.split_whitespace();
        let state = f.next();
        let group = f.nth(1).and_then(|g| g.parse::<i32>().ok());
        group == Some(pgid) && !matches!(state, Some("Z" | "X"))
    })
}

#[tokio::test]
async fn a_result_over_the_calls_room_is_the_runs_error_naming_the_room() {
    let Some(launch) = crate::desktop::fdo::launch::tests::rig(LaunchConfig::default()).await
    else {
        return;
    };
    let home = Home::new();
    home.routine(
        "chatty",
        "description = \"Say a lot.\"\nkind = \"script\"\nread_only = true\n",
        Some("head -c 5000 /dev/zero | tr '\\0' x"),
    );
    home.switch_on("chatty");
    let (rig, _) = rig_with(home, granted(PolicyConfig::default(), &["chatty"]));
    rig.run_with(Some(launch.launch.manager.clone()));
    let entry = rig.local.entry_any("chatty").unwrap();
    let r = rig
        .local
        .call_with(
            &entry,
            None,
            &Caller::Owner,
            LocalBounds {
                cancel: CancellationToken::new(),
                room: Some((100, "a test's room: 100 bytes".into())),
                out: None,
            },
        )
        .await;
    assert!(r.error);
    assert_eq!(
        text(&r),
        "chatty wrote 5000 bytes to stdout, more than its result may take (a test's room: 100 \
         bytes); it exited with 0"
    );
    // The tray's room is the notification's, named.
    assert!(crate::notify::body_room_says().contains(&crate::notify::BODY_ROOM.to_string()));
}

/// A tray's or key's run is the owner's own act: a read-only routine (it asked to run unasked) is
/// not confirmed again, one that changes state keeps its confirmation, and the owner's own
/// `policy.tools` entry for the routine still decides.
#[tokio::test]
async fn an_owners_run_of_a_read_only_routine_is_not_asked_but_a_changing_one_is() {
    let steps = |read_only: bool| {
        format!(
            "description = \"Say.\"\nkind = \"steps\"\nread_only = {read_only}\n\
             [[steps]]\ntool = \"echo\"\nargs = {{ text = \"hi\" }}\n"
        )
    };
    let home = Home::new();
    home.routine("peek", &steps(true), None);
    home.routine("change", &steps(false), None);
    home.routine("pinned", &steps(true), None);
    for n in ["peek", "change", "pinned"] {
        home.switch_on(n);
    }
    let mut policy = PolicyConfig::default();
    policy
        .tools
        .insert("echo".into(), crate::host::settings::ToolState::Allow);
    policy
        .tools
        .insert("pinned".into(), crate::host::settings::ToolState::Confirm);
    let (mut rig, _) = rig_with(home, policy);
    rig.run_with(None);
    let (title, body) = run(&rig, "peek").await.unwrap().unwrap();
    assert_eq!(title, "Routine peek");
    assert!(body.contains("hi"), "{body}");
    assert!(rig.asks.try_recv().is_err(), "the owner was not asked");
    // Changes state: asked as before.
    let pending = run(&rig, "change");
    rig.ask().await.reply.send(false).unwrap();
    let (title, _) = pending.await.unwrap().unwrap();
    assert_eq!(title, "Routine change failed");
    // The owner's entry decides when set.
    let pending = run(&rig, "pinned");
    rig.ask().await.reply.send(false).unwrap();
    let (title, _) = pending.await.unwrap().unwrap();
    assert_eq!(title, "Routine pinned failed");
}
