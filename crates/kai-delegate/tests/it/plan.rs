//! The plan: confirmed once, its steps run once each, nothing outside it that needs a
//! confirmation, a second plan refused unasked, a declined one ends the task.

use std::sync::atomic::Ordering;

use kai_delegate::{CancellationToken, PLAN_TOOL, Status, run};
use serde_json::json;

use crate::fakes::*;

fn plan(steps: serde_json::Value) -> kai_delegate::ToolCall {
    call("p", PLAN_TOOL, json!({ "steps": steps }))
}

#[tokio::test]
async fn the_owner_confirms_the_plan_once_and_each_step_runs_once() {
    let desk = Desk::standard();
    let owner = Owner1::answers(true);
    let model = Scripted::new(vec![
        // Unplanned: refused, nothing asked.
        calls(vec![call("c1", "window_close", json!({"window": "w2"}))]),
        calls(vec![plan(json!([
            {"tool": "window_close", "arguments": {"window": "w2"}},
            {"tool": "window_close", "arguments": {"window": "w3", "unused": null}},
            {"tool": "see_screen"}
        ]))]),
        calls(vec![
            call("c2", "window_close", json!({"window": "w2"})),
            call("c3", "window_close", json!({"window": "w3"})),
            // Spent already: refused.
            call("c4", "window_close", json!({"window": "w2"})),
            // Never planned.
            call("c5", "window_close", json!({"window": "w9"})),
        ]),
        // A second plan is refused without asking.
        calls(vec![plan(
            json!([{"tool": "window_close", "arguments": {"window": "w9"}}]),
        )]),
        says("partial: closed w2 and w3, not w9"),
    ]);
    let a = run(
        &LOOK,
        &task("close the terminals", None, heard("Mach die Terminals zu")),
        &model,
        &desk,
        &owner,
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(owner.asked.load(Ordering::SeqCst), 1, "one confirmation");
    assert_eq!(
        owner.shown.lock().unwrap()[0],
        [
            r#"window_close {"window":"w2"}"#,
            r#"window_close {"window":"w3"}"#
        ],
        "the owner sees the steps that need a confirmation"
    );
    assert_eq!(
        desk.ran()[2..],
        [
            r#"window_close {"window":"w2"}"#,
            r#"window_close {"window":"w3"}"#
        ],
        "after the two reads, the two planned closes and nothing else"
    );
    let asked = model.asked();
    let after = |n: usize, id: &str| {
        asked[n]
            .messages
            .iter()
            .map(texts)
            .find(|t| t.starts_with(&format!("{id}: ")))
            .unwrap()
    };
    assert!(after(1, "c1").contains(
        "window_close needs the owner's confirmation; it did not run. Propose it with propose_plan"
    ));
    assert_eq!(
        after(2, "p"),
        "p: The owner confirmed the plan: make its calls as proposed, each once."
    );
    assert_eq!(after(3, "c2"), "c2: closed");
    assert!(after(3, "c4").contains("not a step of the confirmed plan (or ran already)"));
    assert!(after(3, "c5").contains("not a step of the confirmed plan"));
    let last = &asked[4].messages;
    assert!(texts(last.last().unwrap()).contains("the owner answered this task's plan already"));
    assert_eq!(
        (a.status, a.used.clone()),
        (Status::Partial, vec![("window_close".to_owned(), 2)])
    );
}

#[tokio::test]
async fn a_declined_plan_ends_the_task_failed_and_nothing_of_it_runs() {
    let desk = Desk::standard();
    let owner = Owner1::answers(false);
    let model = Scripted::new(vec![calls(vec![
        plan(json!([{"tool": "window_close", "arguments": {"window": "w2"}}])),
        call("c1", "window_close", json!({"window": "w2"})),
    ])]);
    let a = run(
        &LOOK,
        &task("close it", None, heard("zu damit")),
        &model,
        &desk,
        &owner,
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(a.status, Status::Failed);
    assert_eq!(
        a.render(),
        "failed: the owner declined the plan (window_close); nothing of it ran"
    );
    assert_eq!(desk.ran().len(), 2, "the two reads only");
    assert_eq!(model.asked().len(), 1, "no further model turn");
}

#[tokio::test]
async fn a_plan_is_not_shown_when_a_step_is_refused_malformed_or_needs_no_confirmation() {
    let mut desk = Desk::standard();
    desk.tools.get_mut("see_screen").unwrap().gate =
        kai_delegate::Gate::Deny("see_screen is off".into());
    let owner = Owner1::answers(true);
    let model = Scripted::new(vec![
        calls(vec![plan(
            json!([{"tool": "window_close", "arguments": {"window": "w2"}}, {"tool": "see_screen"}]),
        )]),
        calls(vec![plan(json!([{"tool": "rm_rf"}]))]),
        calls(vec![plan(json!([{"tool": "window_list"}]))]),
        calls(vec![plan(json!([]))]),
        calls(vec![kai_delegate::ToolCall {
            id: "x".into(),
            name: PLAN_TOOL.into(),
            arguments: "{not json".into(),
        }]),
        says("failed: nothing to do"),
    ]);
    let a = run(
        &LOOK,
        &task("q", None, heard("w")),
        &model,
        &desk,
        &owner,
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(owner.asked.load(Ordering::SeqCst), 0, "never asked");
    let asked = model.asked();
    let reply = |n: usize| texts(asked[n].messages.last().unwrap());
    assert!(
        reply(1)
            .contains("step 2 (see_screen) is refused: see_screen is off; the plan was not shown"),
        "{}",
        reply(1)
    );
    assert!(
        reply(2).contains("step 1: no tool rm_rf for this delegate; the plan was not shown"),
        "{}",
        reply(2)
    );
    assert_eq!(
        reply(3),
        "p: No step of this plan needs the owner's confirmation: make the calls."
    );
    assert!(reply(4).contains("the plan has no steps"));
    assert!(
        reply(5).starts_with("x: error: the arguments of propose_plan are not JSON"),
        "{}",
        reply(5)
    );
    assert_eq!(a.status, Status::Failed);
}

#[tokio::test]
async fn a_plan_that_could_not_be_asked_is_not_a_refusal() {
    let desk = Desk::standard();
    let owner = Owner1 {
        unavailable: true,
        ..Owner1::default()
    };
    let model = Scripted::new(vec![calls(vec![plan(
        json!([{"tool": "window_close", "arguments": {"window": "w2"}}]),
    )])]);
    let a = run(
        &LOOK,
        &task("close it", None, heard("zu damit")),
        &model,
        &desk,
        &owner,
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(a.status, Status::Failed);
    assert!(a.text.contains("could not ask the owner"), "{}", a.text);
    assert!(!a.text.contains("declined"), "{}", a.text);
    assert_eq!(desk.ran().len(), 2, "the two reads only");
}

#[tokio::test]
async fn the_used_line_names_each_calls_target() {
    let desk = Desk::standard();
    let owner = Owner1::answers(true);
    let model = Scripted::new(vec![
        calls(vec![
            call("a", "see_screen", json!({"label": "DP-1"})),
            call("b", "see_screen", json!({"label": "DP-1"})),
            call("c", "see_screen", json!({"label": "HDMI-1"})),
            call("d", "window_list", json!({})),
        ]),
        says("done: seen"),
    ]);
    let a = run(
        &LOOK,
        &task("what", None, heard("x")),
        &model,
        &desk,
        &owner,
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        a.used,
        [
            ("see_screen (DP-1)".to_owned(), 2),
            ("see_screen (HDMI-1)".to_owned(), 1),
            ("window_list".to_owned(), 1)
        ]
    );
}
