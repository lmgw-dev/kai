//! The kinds that think, against the delegate tests' loopback lmgw with scripted turns: a goal runs
//! the delegate loop with its own tools as the routine's calls; `routine_create` writes a draft
//! that stays off, and never into an existing routine.

use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::delegate::tests::{Lmgw, folders, text_turn, tool_turn};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Call};

async fn call(rig: &Rig, name: &str, args: serde_json::Value) -> Reply {
    let entry = rig
        .local
        .entry_any(name)
        .unwrap_or_else(|| panic!("{name}"));
    (entry.handler)(Call::new(
        entry.spec,
        Args::read(entry.spec, args.as_object().cloned()).unwrap(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await
}

#[tokio::test]
async fn a_goal_runs_the_delegate_loop_with_its_tools_as_the_routines_calls() {
    let lmgw = Lmgw::start(
        folders(Some("gemma")),
        vec![
            tool_turn(&[("c1", "echo", json!({"text": "hi"}))]),
            text_turn("done: said hi"),
        ],
    )
    .await;
    let home = Home::new();
    home.routine(
        "greet_goal",
        "description = \"Greet by echo.\"\nkind = \"goal\"\ngoal = \"Say hi with echo.\"\n\
         tools = [\"echo\"]\n",
        None,
    );
    home.switch_on("greet_goal");
    let (mut rig, _) = rig_with(
        home,
        PolicyConfig {
            trusted_callers: vec!["routine:greet_goal".into()],
            ..Default::default()
        },
    );
    rig.link(&lmgw.url);
    rig.run_with(None);
    let r = call(&rig, "greet_goal", json!({})).await;
    assert!(!r.error, "{}", text(&r));
    assert_eq!(text(&r), "done: said hi\n(used: echo (hi))");
    assert_eq!(rig.ran.all(), ["echo by routine 'greet_goal': hi"]);
    let sent = lmgw.completions();
    assert_eq!(sent.len(), 2);
    let key = DeviceKey::parse(KEY).unwrap();
    assert_eq!(sent[0].auth, format!("Bearer {}", key.expose()));
    let body: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
    assert_eq!(body["model"], "gemma", "the Kai conversation's alias");
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("You run the owner's routine greet_goal. Its goal: Say hi with echo."),
        "{system}"
    );
    let tools: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["echo", "propose_plan"], "its own tools only");
}

const DRAFT_TOML: &str = "description = \"Say how full the home file system is.\"\n\
                          kind = \"script\"\nscript = \"run.sh\"\nread_only = true\n";

#[tokio::test]
async fn routine_create_writes_a_draft_that_stays_off() {
    let lmgw = Lmgw::start(
        folders(Some("coder")),
        vec![
            tool_turn(&[(
                "c1",
                "draft_write",
                json!({"path": "disk_usage/routine.toml", "content": DRAFT_TOML}),
            )]),
            tool_turn(&[(
                "c2",
                "draft_write",
                json!({"path": "disk_usage/run.sh", "content": "#!/bin/bash\ndf -h \"$HOME\"\n"}),
            )]),
            text_turn("done: drafted disk_usage"),
        ],
    )
    .await;
    let (mut rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.link(&lmgw.url);
    rig.run_with(None);
    let r = call(
        &rig,
        "routine_create",
        json!({"goal": "how full is my disk"}),
    )
    .await;
    assert!(!r.error, "{}", text(&r));
    assert!(
        text(&r).starts_with("done: drafted disk_usage"),
        "{}",
        text(&r)
    );
    assert!(
        text(&r).contains(
            "draft disk_usage (script): Say how full the home file system is.; off until the \
             owner switches it on"
        ),
        "{}",
        text(&r)
    );
    let dir = rig.home.routines().join("disk_usage");
    assert_eq!(
        std::fs::read_to_string(dir.join("routine.toml")).unwrap(),
        DRAFT_TOML
    );
    let mode = std::fs::metadata(dir.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    rig.inner.rescan(None);
    assert!(
        rig.inner
            .state()
            .loaded
            .iter()
            .any(|l| l.name == "disk_usage")
    );
    assert!(
        !rig.names().contains(&"disk_usage".to_owned()),
        "a draft is off"
    );
    // The task message carried the goal and Kai's tools.
    let sent = lmgw.completions();
    let body: serde_json::Value = serde_json::from_str(&sent[0].body).unwrap();
    let task = body["messages"][1]["content"].to_string();
    assert!(task.contains("how full is my disk"), "{task}");
    assert!(
        task.contains("echo --text <a text>: Answer with the text."),
        "{task}"
    );
    assert_eq!(body["model"], "coder");
}

#[tokio::test]
async fn routine_create_never_writes_into_an_existing_routine() {
    let lmgw = Lmgw::start(
        folders(Some("coder")),
        vec![
            tool_turn(&[(
                "c1",
                "draft_write",
                json!({"path": "greet/run.sh", "content": "#!/bin/bash\nrm -rf ~\n"}),
            )]),
            text_turn("failed: greet exists"),
        ],
    )
    .await;
    let home = Home::new();
    home.routine(
        "greet",
        "description = \"Greet.\"\nkind = \"script\"\n",
        Some("echo hi"),
    );
    let before = std::fs::read_to_string(home.routines().join("greet/run.sh")).unwrap();
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.link(&lmgw.url);
    rig.run_with(None);
    let r = call(&rig, "routine_create", json!({"goal": "greet"})).await;
    assert!(r.error);
    assert!(text(&r).contains("no draft was written"), "{}", text(&r));
    assert_eq!(
        std::fs::read_to_string(rig.home.routines().join("greet/run.sh")).unwrap(),
        before
    );
    let sent = lmgw.completions();
    let second: serde_json::Value = serde_json::from_str(&sent[1].body).unwrap();
    assert!(
        second
            .to_string()
            .contains("a routine greet exists already"),
        "the model reads the refusal"
    );
}
