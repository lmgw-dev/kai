//! The context: the static part first and byte-identical, the dynamic part last, the user's words
//! at its very end, no conversation, a screenshot only in the delegate's own request.

use kai_delegate::{CancellationToken, Message, PLAN_TOOL, Part, Utterance, run};
use serde_json::json;

use crate::fakes::*;

#[tokio::test]
async fn the_static_part_is_byte_identical_across_calls_and_holds_nothing_of_the_task() {
    let desk = Desk::standard();
    let owner = Owner1::answers(true);
    let a = Scripted::new(vec![says("done: a clock")]);
    let b = Scripted::new(vec![says("done: a chart")]);
    let none = CancellationToken::new();
    run(
        &LOOK,
        &task("what time is it?", None, heard("Wie spät ist es?")),
        &a,
        &desk,
        &owner,
        &settings(0),
        &none,
    )
    .await
    .unwrap();
    run(
        &LOOK,
        &task(
            "which chart is open?",
            Some("w1"),
            Utterance::Unknown("the call came from the dashboard".into()),
        ),
        &b,
        &desk,
        &owner,
        &settings(0),
        &none,
    )
    .await
    .unwrap();
    let (a, b) = (&a.asked()[0].prefix, &b.asked()[0].prefix);
    assert_eq!(a, b);
    assert_eq!(format!("{a:?}").as_bytes(), format!("{b:?}").as_bytes());
    for leak in [
        "what time",
        "Wie spät",
        "chart",
        "dashboard",
        "w1 firefox",
        "WINDOW",
    ] {
        assert!(!a.system.contains(leak), "{leak} in the static part");
    }
    assert!(a.system.starts_with(
        "You are Kai's look delegate. Answer a question about what is on the screen."
    ));
    assert!(
        a.system
            .contains("Examples:\n\nquestion: which song plays?")
    );
    let names: Vec<&str> = a.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "window_list",
            "see_window",
            "see_screen",
            "window_close",
            PLAN_TOOL
        ],
        "the offer's order, the plan tool last"
    );
}

#[tokio::test]
async fn the_dynamic_part_is_the_reads_then_the_parameters_then_the_users_words_verbatim() {
    let desk = Desk::standard();
    let model = Scripted::new(vec![says("done: 14:02")]);
    let words = "  Was zeigt die Uhr?\n\"genau\"  ";
    run(
        &LOOK,
        &task("what does the clock show?", Some("w1"), heard(words)),
        &model,
        &desk,
        &Owner1::default(),
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let asked = model.asked();
    assert_eq!(asked.len(), 1);
    // No conversation: the task message is the whole dynamic part of the first request.
    assert_eq!(asked[0].messages.len(), 1);
    let Message::User(parts) = &asked[0].messages[0] else {
        panic!("a user message")
    };
    assert_eq!(parts.len(), 3, "{parts:?}");
    let Part::Text(head) = &parts[0] else {
        panic!()
    };
    assert_eq!(
        head,
        "Read for this task just now:\n\n## Open windows\nw1 firefox \"News\"\nw2 konsole \"~\"\n\n## The window\nwindow w1, 800×600"
    );
    assert_eq!(
        parts[1],
        png("WINDOW"),
        "the screenshot is in the delegate's request"
    );
    let Part::Text(tail) = &parts[2] else {
        panic!()
    };
    assert_eq!(
        tail,
        &format!(
            "\nTask (look):\nquestion: what does the clock show?\nwindow: w1\n\nThe user's last words, verbatim:\n{words}"
        )
    );
    assert!(tail.ends_with(words), "the utterance is last and unchanged");
    // The context reads went under the policy, with the call's window handed on.
    assert_eq!(
        desk.ran(),
        ["window_list {}", r#"see_window {"window":"w1"}"#]
    );
}

#[tokio::test]
async fn a_screenshot_stays_in_the_delegates_requests_and_the_answer_is_text() {
    let desk = Desk::standard();
    let model = Scripted::new(vec![
        calls(vec![call("c1", "see_screen", json!({}))]),
        says("done: a dialog asks to save"),
    ]);
    let answer = run(
        &LOOK,
        &task("is a dialog open?", None, heard("Ist da ein Dialog?")),
        &model,
        &desk,
        &Owner1::default(),
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let asked = model.asked();
    let second = &asked[1].messages;
    // The task, the model's call, the tool's text, its image as the next user message.
    assert_eq!(second.len(), 4);
    assert_eq!(
        second[0], asked[0].messages[0],
        "the earlier messages stay as they were"
    );
    assert_eq!(
        texts(&second[2]),
        "c1: output DP-1, 1920×1080\n(its image follows in the next message)"
    );
    assert_eq!(images(&second[2]), 0, "a tool message carries text only");
    assert_eq!(images(&second[3]), 1);
    assert_eq!(
        texts(&second[3]),
        "The image see_screen returned (call c1):|[image iVBORw0KGgoSCREEN]"
    );
    let shown = answer.render();
    assert_eq!(shown, "done: a dialog asks to save\n(used: see_screen)");
    assert!(!shown.contains("iVBOR"));
}

#[tokio::test]
async fn a_read_the_policy_does_not_allow_is_named_in_its_place() {
    let mut desk = Desk::standard();
    desk.tools.get_mut("see_window").unwrap().gate = kai_delegate::Gate::Confirm;
    desk.tools.get_mut("window_list").unwrap().gate = kai_delegate::Gate::Deny(
        "window_list is switched off (policy.tools.window_list = \"deny\")".into(),
    );
    let model = Scripted::new(vec![says("failed: I cannot see it")]);
    run(
        &LOOK,
        &task("what is open?", None, heard("Was ist offen?")),
        &model,
        &desk,
        &Owner1::default(),
        &settings(0),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(desk.ran().is_empty(), "neither read ran");
    let first = texts(&model.asked()[0].messages[0]);
    assert!(first.contains("## Open windows\n(not read: window_list is switched off (policy.tools.window_list = \"deny\"))"), "{first}");
    assert!(first.contains("## The window\n(not read: see_window needs the owner's confirmation; propose it in a plan to read it)"), "{first}");
    assert_eq!(images(&model.asked()[0].messages[0]), 0);
}
