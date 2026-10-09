//! The list: a change in the folders sends `list_changed`; a routine is offered only while it is
//! on; switching one on shows it in full first; a draft is off.

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::host::policy::Caller;
use crate::host::tests::FakeHost;

const SAY: &str = "description = \"Say hello.\"\nkind = \"steps\"\n\
                   [[steps]]\ntool = \"echo\"\nargs = { text = \"hello\" }\n";

fn names(list: &[Value]) -> Vec<String> {
    list.iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_change_in_the_folder_sends_list_changed() {
    let home = Home::new();
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let fake = FakeHost::new().await;
    let _events = rig.start(&fake.url);
    let mut c = fake.accept().await;
    c.initialize().await;
    let first = names(&c.list().await);
    assert!(first.contains(&"routine_create".to_owned()));
    assert!(
        !first.contains(&"routine_enable".to_owned()),
        "a configuration tool: not listed while policy.config_tools is off"
    );
    let watching = tokio::spawn(super::super::watch_loop(
        rig.inner.clone(),
        rig.home.config_file(),
    ));
    // Let the watch start before the change.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    rig.home.routine("say_hello", SAY, None);
    rig.home.switch_on("say_hello");
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    let listed = c.list().await;
    let tool = listed
        .iter()
        .find(|t| t["name"] == "say_hello")
        .expect("listed");
    assert_eq!(tool["description"], "Say hello.");
    assert_eq!(tool["annotations"]["readOnlyHint"], false);
    // Removed: listed no more.
    std::fs::remove_dir_all(rig.home.routines().join("say_hello")).unwrap();
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    assert_eq!(names(&c.list().await), first);
    watching.abort();
}

#[tokio::test]
async fn a_draft_is_off_until_the_owner_switches_it_on_after_seeing_it() {
    let home = Home::new();
    home.routine(
        "greet",
        "description = \"Greet.\"\nkind = \"script\"\n",
        Some("echo \"hello $1\""),
    );
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    assert!(!rig.names().contains(&"greet".to_owned()), "a draft is off");
    assert!(rig.local.entries().iter().all(|e| e.spec.name != "greet"));
    // The tray's switch: routine_enable, listed or not, by the owner.
    let entry = rig.local.entry_any("routine_enable").unwrap();
    let mut args = Map::new();
    args.insert("name".into(), json!("greet"));
    args.insert("on".into(), json!(true));
    let local = rig.local.clone();
    let call = tokio::spawn(async move {
        local
            .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
            .await
    });
    let ask = rig.ask().await;
    assert_eq!(ask.caller, "the owner");
    assert!(ask.lines.contains(&"name: greet".to_owned()));
    assert!(ask.lines.contains(&"routine.toml:".to_owned()));
    assert!(ask.lines.contains(&"run.sh:".to_owned()));
    assert!(
        ask.lines.contains(&"  echo \"hello $1\"".to_owned()),
        "the script in full: {:?}",
        ask.lines
    );
    ask.reply.send(true).unwrap();
    let r = call.await.unwrap();
    assert!(!r.error, "{}", text(&r));
    assert_eq!(text(&r), "greet is on");
    let file = std::fs::read_to_string(rig.home.config_file()).unwrap();
    assert!(
        file.contains("[routines.enabled]") && file.contains("greet = true"),
        "a plain true: {file}"
    );
    assert!(rig.names().contains(&"greet".to_owned()));
    // Off asks nobody from the tray, and is gone from the list.
    rig.inner.switch("greet", false).await.unwrap();
    assert!(!rig.names().contains(&"greet".to_owned()));
    assert!(rig.asks.try_recv().is_err());
}

#[tokio::test]
async fn a_declined_or_broken_routine_stays_off() {
    let home = Home::new();
    home.routine("broken", "description = \"x\"\nkind = \"steps\"\n", None);
    home.routine("fine", SAY, None);
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let local = rig.local.clone();
    let enable = |name: &str| {
        let entry = local.entry_any("routine_enable").unwrap();
        let mut args = Map::new();
        args.insert("name".into(), json!(name));
        args.insert("on".into(), json!(true));
        let local = local.clone();
        tokio::spawn(async move {
            local
                .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
                .await
        })
    };
    let r = enable("broken").await.unwrap();
    assert!(r.error);
    assert!(text(&r).contains("does not load"), "{}", text(&r));
    let r = enable("nothing").await.unwrap();
    assert!(
        text(&r).contains("Kai has no routine nothing"),
        "{}",
        text(&r)
    );
    let pending = enable("fine");
    rig.ask().await.reply.send(false).unwrap();
    let r = pending.await.unwrap();
    assert!(r.error && text(&r).contains("denied"), "{}", text(&r));
    assert!(!rig.names().contains(&"fine".to_owned()));
    // A key with routine_enable set to allow (ruling 28): no question.
    rig.policy
        .write()
        .unwrap()
        .clone_from(&Policy::new(&PolicyConfig {
            tools: [(
                "routine_enable".to_owned(),
                crate::host::settings::ToolState::Allow,
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }));
    let r = enable("fine").await.unwrap();
    assert!(!r.error, "{}", text(&r));
    assert!(rig.names().contains(&"fine".to_owned()));
}

#[test]
fn config_set_cannot_switch_a_routine_on() {
    let home = Home::new();
    let e = crate::settings::apply(
        &home.config_file(),
        crate::settings::Request::Set {
            key: "routines.enabled.x".into(),
            value: "true".into(),
        },
    )
    .unwrap_err();
    assert!(e.0.contains("routine_enable"), "{e}");
    home.switch_on("x");
    let out = crate::settings::apply(
        &home.config_file(),
        crate::settings::Request::Unset {
            key: "routines.enabled.x".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        out.response,
        crate::settings::Response::Changed {
            effect: crate::settings::Effect::Now,
            ..
        }
    ));
    assert!(!out.config.unwrap().routines.switched_on("x"));
}

#[test]
fn an_async_routine_is_offered_as_a_job() {
    let home = Home::new();
    home.routine(
        "later",
        "description = \"Say hello later.\"\nkind = \"steps\"\nasync = true\n\
         [[steps]]\ntool = \"echo\"\nargs = { text = \"hello\" }\n",
        None,
    );
    home.switch_on("later");
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let s = rig.inner.state();
    assert!(
        s.loaded[0]
            .routine
            .as_ref()
            .is_ok_and(|r| r.spec.class == kai_desktop::contract::Class::Async)
    );
    assert_eq!(s.standing(&s.loaded[0]), super::super::Standing::On);
    // WP18: offered like any routine; lmgw calls it as a task (`execution.taskSupport`).
    assert!(rig.names().contains(&"later".to_owned()));
    let tool = crate::host::server::render(s.loaded[0].routine.as_ref().unwrap().spec);
    assert_eq!(
        tool.task_support(),
        rmcp::model::TaskSupport::Required,
        "{tool:?}"
    );
}
