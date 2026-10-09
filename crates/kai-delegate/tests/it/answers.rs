//! The answers: done, partial or failed, each stop saying why.

use kai_delegate::{CancellationToken, Status, Turn, run};
use serde_json::json;

use crate::fakes::*;

#[tokio::test]
async fn done_partial_and_failed_come_from_the_model_or_say_why_the_loop_stopped() {
    let desk = Desk::standard();
    let owner = Owner1::default();
    let one = |turns| async {
        let model = Scripted::new(turns);
        run(
            &LOOK,
            &task("q", None, heard("w")),
            &model,
            &desk,
            &owner,
            &settings(3),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    };
    let a = one(vec![says("Done: the kettle is at 80 °C")]).await;
    assert_eq!(
        (a.status, a.render()),
        (Status::Done, "done: the kettle is at 80 °C".to_owned())
    );
    let a = one(vec![says(
        "partial: the chart has no legend; the axis says kWh",
    )])
    .await;
    assert_eq!(a.status, Status::Partial);
    let a = one(vec![says("failed: the window is minimized")]).await;
    assert_eq!(a.render(), "failed: the window is minimized");
    let a = one(vec![says("It shows 14:02.")]).await;
    assert_eq!(
        a.render(),
        "done: It shows 14:02.",
        "no word: it answered, so done"
    );
    let a = one(vec![Err(
        "lmgw answered 503 gpu_hold: the GPU is held".into()
    )])
    .await;
    assert_eq!(
        a.render(),
        "failed: the delegate's model did not answer: lmgw answered 503 gpu_hold: the GPU is held"
    );
    let a = one(vec![Ok(Turn::default())]).await;
    assert_eq!(a.render(), "failed: the delegate's model answered nothing");
    // The step setting: three model turns, each a call, and the stop names the setting.
    let a = one(vec![
        calls(vec![call("a", "see_screen", json!({}))]),
        calls(vec![call("b", "see_screen", json!({}))]),
        calls(vec![call("c", "window_list", json!({}))]),
    ])
    .await;
    assert_eq!(
        a.render(),
        "partial: stopped after 3 model turns (delegate.max_turns = 3) before the delegate answered\n\
         (used: see_screen ×2, window_list)"
    );
    assert_eq!(a.turns, 3);
}

#[tokio::test]
async fn a_bad_call_is_the_models_to_read_not_the_end() {
    let desk = Desk::standard();
    let model = Scripted::new(vec![
        calls(vec![
            call("a", "format_disk", json!({})),
            kai_delegate::ToolCall {
                id: "b".into(),
                name: "see_screen".into(),
                arguments: "[1".into(),
            },
        ]),
        says("failed: no such tool"),
    ]);
    let a = run(
        &LOOK,
        &task("q", None, heard("w")),
        &model,
        &desk,
        &Owner1::default(),
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let asked = model.asked();
    let msgs: Vec<String> = asked[1].messages.iter().map(texts).collect();
    assert!(
        msgs.contains(&"a: error: no tool format_disk for this delegate".to_owned()),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("b: error: the arguments of see_screen are not JSON"))
    );
    assert_eq!(a.status, Status::Failed);
    assert!(a.used.is_empty());
}
