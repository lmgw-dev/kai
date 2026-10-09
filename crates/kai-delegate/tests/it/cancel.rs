//! The caller's cancellation ends the loop at every wait, and reaches work a tool spawned.

use std::sync::atomic::Ordering;
use std::time::Duration;

use kai_delegate::{CancellationToken, Cancelled, PLAN_TOOL, run};
use serde_json::json;

use crate::fakes::*;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("ended in time")
}

#[tokio::test]
async fn a_cancel_during_the_model_request_ends_the_task() {
    let desk = Desk::standard();
    let model = Scripted::new(vec![]);
    let cancel = CancellationToken::new();
    let owner = Owner1::default();
    let t = task("q", None, heard("w"));
    let s = settings(0);
    let fut = run(&LOOK, &t, &model, &desk, &owner, &s, &cancel);
    let stop = async {
        model.waiting.notified().await;
        cancel.cancel();
    };
    let (r, ()) = within(async { tokio::join!(fut, stop) }).await;
    assert_eq!(r, Err(Cancelled));
}

#[tokio::test]
async fn a_cancel_during_a_tool_call_stops_the_work_it_spawned() {
    let mut desk = Desk::standard();
    desk.tools.get_mut("see_screen").unwrap().hangs = true;
    let model = Scripted::new(vec![
        calls(vec![call("a", "see_screen", json!({}))]),
        says("done"),
    ]);
    let cancel = CancellationToken::new();
    let owner = Owner1::default();
    let t = task("q", None, heard("w"));
    let s = settings(0);
    let fut = run(&LOOK, &t, &model, &desk, &owner, &s, &cancel);
    let stop = async {
        desk.hanging.notified().await;
        cancel.cancel();
    };
    let (r, ()) = within(async { tokio::join!(fut, stop) }).await;
    assert_eq!(r, Err(Cancelled));
    within(async {
        while !desk.stopped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(model.asked().len(), 1, "no model turn after the cancel");
}

#[tokio::test]
async fn a_cancel_while_the_owner_is_asked_ends_the_task_and_runs_nothing() {
    let desk = Desk::standard();
    let owner = Owner1 {
        hangs: true,
        ..Owner1::default()
    };
    let model = Scripted::new(vec![calls(vec![
        call(
            "p",
            PLAN_TOOL,
            json!({"steps": [{"tool": "window_close", "arguments": {"window": "w2"}}]}),
        ),
        call("c", "window_close", json!({"window": "w2"})),
    ])]);
    let cancel = CancellationToken::new();
    let t = task("q", None, heard("w"));
    let s = settings(0);
    let fut = run(&LOOK, &t, &model, &desk, &owner, &s, &cancel);
    let stop = async {
        owner.asking.notified().await;
        cancel.cancel();
    };
    let (r, ()) = within(async { tokio::join!(fut, stop) }).await;
    assert_eq!(r, Err(Cancelled));
    assert_eq!(desk.ran().len(), 2, "the reads only, no close");
}

#[tokio::test]
async fn a_task_cancelled_before_it_starts_reads_nothing() {
    let desk = Desk::standard();
    let model = Scripted::new(vec![says("done")]);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let r = run(
        &LOOK,
        &task("q", None, heard("w")),
        &model,
        &desk,
        &Owner1::default(),
        &settings(0),
        &cancel,
    )
    .await;
    assert_eq!(r, Err(Cancelled));
    assert!(desk.ran().is_empty() && model.asked().is_empty());
}
