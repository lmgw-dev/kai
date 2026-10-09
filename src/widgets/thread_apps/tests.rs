//! The flow's bookkeeping, driven by the frames lmgw's bound session relays (`lmgw.chat.frame`,
//! shaped as lmgw's `web/agentchat.rs` sends them).

use serde_json::json;

use lmgw_client::mcp_apps::ToolResultFrame;

use super::flow::call_tool_result;
use super::*;

fn ready(index: u64, name: &str, arguments: Value) -> Value {
    json!({"event": "ready", "index": index, "name": name, "arguments": arguments})
}

fn result(index: u64, name: &str, ui: Option<(&str, &str)>) -> Value {
    json!({"event": "result", "index": index, "name": name, "output": "sunny", "is_error": false,
           "ms": 12, "server_label": ui.map(|u| u.0), "ui_resource": ui.map(|u| u.1),
           "structured_content": {"temp": 21}})
}

fn outcome(s: &Start) -> Option<Outcome> {
    s.progress.borrow().outcome.clone()
}

fn runs(s: &Start) -> bool {
    s.progress.borrow().runs
}

#[test]
fn a_calls_input_and_result_are_followed_by_response_and_index() {
    let mut f = Flow::default();
    let start = f
        .frame(
            "r1",
            "tool",
            &json!({"event": "start", "index": 0, "name": "fx__show"}),
            true,
        )
        .map(|_| ());
    assert_eq!(start, None, "start carries no arguments yet");
    let s = f
        .frame(
            "r1",
            "tool",
            &ready(0, "fx__show", json!({"city": "Berlin"})),
            true,
        )
        .expect("a call to look for a view for");
    assert_eq!(s.name, "fx__show");
    assert_eq!(s.arguments["city"], "Berlin");
    assert!(s.look_up);
    assert_eq!(outcome(&s), None);
    assert!(
        f.frame("r1", "tool", &ready(0, "fx__show", json!({})), true)
            .is_none(),
        "the same call once"
    );
    // Another response's call of the same index is another call.
    let other = f
        .frame("r2", "tool", &ready(0, "fx__show", json!({})), true)
        .unwrap();
    assert!(
        f.frame(
            "r1",
            "tool",
            &result(0, "fx__show", Some(("fx", "ui://fx__card"))),
            true
        )
        .is_none()
    );
    assert_eq!(
        outcome(&s),
        Some(Outcome::Result {
            result: json!({"content": [{"type": "text", "text": "sunny"}],
                           "structuredContent": {"temp": 21}, "isError": false}),
            ui: Some(("fx".into(), "ui://fx__card".into())),
        })
    );
    assert_eq!(outcome(&other), None);
    assert_eq!(f.following(), 1);
}

#[test]
fn a_turn_that_ends_first_cancels_its_calls_with_why() {
    let mut f = Flow::default();
    let a = f
        .frame("r1", "tool", &ready(0, "fx__a", json!({})), true)
        .unwrap();
    let b = f
        .frame("r2", "tool", &ready(0, "fx__b", json!({})), true)
        .unwrap();
    let c = f
        .frame("r3", "tool", &ready(0, "fx__c", json!({})), true)
        .unwrap();
    f.frame("r1", "stop", &json!({}), true);
    f.frame("r1", "done", &json!({"aborted": true}), true);
    assert_eq!(
        outcome(&a),
        Some(Outcome::Cancelled("the turn was stopped".into()))
    );
    assert_eq!(outcome(&b), None, "another turn's call goes on");
    f.frame("r2", "done", &json!({"aborted": true}), true);
    assert_eq!(
        outcome(&b),
        Some(Outcome::Cancelled("the turn was aborted".into()))
    );
    f.session_ended();
    assert_eq!(
        outcome(&c),
        Some(Outcome::Cancelled("Kai's voice session ended".into()))
    );
    assert_eq!(f.following(), 0);
    // A done after its result changes nothing.
    let d = f
        .frame("r4", "tool", &ready(1, "fx__d", json!({})), true)
        .unwrap();
    f.frame("r4", "tool", &result(1, "fx__d", None), true);
    f.frame("r4", "done", &json!({}), true);
    assert!(matches!(
        outcome(&d),
        Some(Outcome::Result { ui: None, .. })
    ));
}

#[test]
fn only_mcp_servers_calls_are_followed_and_only_with_auto_show() {
    let mut f = Flow::default();
    for name in ["lmgw__status", "docs__search", "kb__find", "plain", "fx__"] {
        assert!(
            f.frame("r", "tool", &ready(0, name, json!({})), true)
                .is_none(),
            "{name}"
        );
    }
    assert!(
        f.frame("r", "tool", &ready(0, "fx__show", json!({})), false)
            .is_none(),
        "auto_show off"
    );
    assert!(
        f.frame(
            "r",
            "tool",
            &result(0, "fx__show", Some(("fx", "ui://fx__c"))),
            false
        )
        .is_none()
    );
    // Kai's own tools are not looked up in /mcp's list (their result frame still counts).
    let own = f
        .frame("r", "tool", &ready(1, "kai__overlay_text", json!({})), true)
        .unwrap();
    assert!(!own.look_up);
    assert_eq!(f.following(), 1);
}

#[test]
fn a_result_whose_ready_was_not_seen_opens_only_the_view_it_names() {
    let mut f = Flow::default();
    assert!(
        f.frame("r", "tool", &result(0, "fx__show", None), true)
            .is_none()
    );
    let s = f
        .frame(
            "r",
            "tool",
            &result(0, "fx__show", Some(("fx", "ui://fx__c"))),
            true,
        )
        .unwrap();
    assert!(!s.look_up && s.arguments.is_empty());
    assert!(matches!(
        outcome(&s),
        Some(Outcome::Result { ui: Some(_), .. })
    ));
}

fn read(v: Value) -> ToolResultFrame {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_result_frame_becomes_mcps_call_tool_result() {
    // An lmgw before 2026-10-09: the text as one block.
    assert_eq!(
        call_tool_result(&read(
            json!({"index": 0, "name": "fx__a", "output": "", "is_error": true, "ms": 1,
                   "structured_content": null})
        )),
        json!({"content": [], "isError": true})
    );
    assert_eq!(
        call_tool_result(&read(
            json!({"index": 0, "name": "fx__a", "output": "x", "is_error": false, "ms": 1})
        )),
        json!({"content": [{"type": "text", "text": "x"}], "isError": false})
    );
    // Since: the blocks as the server sent them (an image, a resource link, annotations), not the
    // flattened text.
    let blocks = json!([
        {"type": "text", "text": "x", "annotations": {"audience": ["user"]}},
        {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"},
        {"type": "resource_link", "uri": "file://fx__/a.txt", "name": "a.txt"}
    ]);
    assert_eq!(
        call_tool_result(&read(json!({
            "index": 0, "name": "fx__a", "output": "x [image] a.txt", "is_error": false, "ms": 1,
            "content": blocks, "structured_content": {"temp": 21}, "call_id": "c1"
        }))),
        json!({"content": blocks, "structuredContent": {"temp": 21}, "isError": false})
    );
    // A result with only structured content: no blocks, the structure for the view.
    assert_eq!(
        call_tool_result(&read(json!({
            "index": 0, "name": "fx__a", "output": "{\"temp\":21}", "is_error": false, "ms": 1,
            "content": [], "structured_content": {"temp": 21}
        }))),
        json!({"content": [], "structuredContent": {"temp": 21}, "isError": false})
    );
}

/// lmgw's ready frame as it is since 2026-10-09 (client-apps §7.6).
fn ready_now(index: u64, name: &str, call_id: &str, ui: Option<&str>, needs: bool) -> Value {
    json!({"event": "ready", "index": index, "name": name, "arguments": {"city": "Berlin"},
           "call_id": call_id, "server_label": name.split_once("__").map(|(l, _)| l),
           "needs_approval": needs, "ui_resource": ui})
}

fn result_now(index: u64, name: &str, call_id: &str) -> Value {
    json!({"event": "result", "index": index, "name": name, "output": "Berlin: 21",
           "is_error": false, "ms": 4, "call_id": call_id, "server_label": "fx",
           "ui_resource": "ui://fx__card", "structured_content": {"temp": 21},
           "content": [{"type": "text", "text": "Berlin: 21"},
                       {"type": "image", "data": "AAAA", "mimeType": "image/png"}]})
}

/// A ready frame that names its view and says the call runs: the view is read at once, no tool
/// list asked, the input goes at once; one that names none asks no list either.
#[test]
fn a_ready_that_names_its_view_needs_no_tool_list() {
    let mut f = Flow::default();
    let s = f
        .frame(
            "r1",
            "tool",
            &ready_now(0, "fx__show", "c1", Some("ui://fx__card"), false),
            true,
        )
        .unwrap();
    assert_eq!(s.known, Some(("fx".into(), "ui://fx__card".into())));
    assert!(!s.look_up, "the frame named the view");
    assert!(runs(&s), "needs_approval: false");
    assert_eq!(s.call_id.as_deref(), Some("c1"));
    assert_eq!(s.arguments["city"], "Berlin", "the input from the frame");
    // No view named by an lmgw that says needs_approval: none there, no list asked.
    let plain = f
        .frame(
            "r1",
            "tool",
            &ready_now(1, "fx__plain", "c2", None, false),
            true,
        )
        .unwrap();
    assert!(plain.known.is_none() && !plain.look_up);
    // Kai's own tool: never shown at ready, even when the frame names a resource.
    let own = f
        .frame(
            "r1",
            "tool",
            &ready_now(2, "kai__overlay_text", "c3", Some("ui://kai__x"), false),
            true,
        )
        .unwrap();
    assert!(own.known.is_none() && !own.look_up);
    // A moved tool (a collision gave it its server's name as prefix): the frame's label is its
    // server's, which the view is read under.
    let mut moved = ready_now(3, "gh_work__search", "c4", Some("ui://gh__card"), false);
    moved["server_label"] = json!("gh");
    let moved = f.frame("r1", "tool", &moved, true).unwrap();
    assert_eq!(moved.known, Some(("gh".into(), "ui://gh__card".into())));
    // Its result: the blocks as sent, matched by response and index.
    f.frame("r1", "tool", &result_now(0, "fx__show", "c1"), true);
    let Some(Outcome::Result { result, .. }) = outcome(&s) else {
        panic!("a result");
    };
    assert_eq!(result["content"][1]["type"], "image");
    assert_eq!(result["structuredContent"], json!({"temp": 21}));
    assert_eq!(result["isError"], false);
}

/// Held calls are found again by their call id, which is kept whole (Gemini's run to hundreds of
/// characters): the same tool twice, the arguments re-serialised, a result whose ready was not
/// seen.
#[test]
fn a_held_call_is_found_by_its_call_id() {
    let mut f = Flow::default();
    let long = |n: &str| format!("{n}{}", "c2lnbmF0dXJl".repeat(50));
    let (x, y) = (long("x"), long("y"));
    assert!(x.len() > 600);
    let mut ax = ready_now(0, "fx__send", &x, Some("ui://fx__card"), true);
    ax["arguments"] = json!({"to": "x"});
    let mut ay = ready_now(1, "fx__send", &y, Some("ui://fx__card"), true);
    ay["arguments"] = json!({"to": "y"});
    assert!(
        f.frame("r1", "tool", &ax, true).is_none(),
        "gated: nothing yet"
    );
    assert!(f.frame("r1", "tool", &ay, true).is_none());
    f.frame(
        "r1",
        "tool",
        &approval("send", r#"{"to":"x"}"#, "mcpr_x"),
        true,
    );
    f.frame(
        "r1",
        "done",
        &pending("send", r#"{"to":"x"}"#, "mcpr_x"),
        true,
    );
    assert_eq!(f.following(), 2);
    // The resumed turn: y first, its arguments not as they were (the id decides).
    let mut again = ready_now(0, "fx__send", &y, Some("ui://fx__card"), false);
    again["arguments"] = json!({"to": "y", "cc": null});
    let sy = f
        .frame("r2", "tool", &again, true)
        .expect("y's view is looked for now");
    assert_eq!(sy.call_id.as_deref(), Some(y.as_str()), "the id whole");
    assert_eq!(sy.known, Some(("fx".into(), "ui://fx__card".into())));
    assert!(!runs(&sy), "a decided call's verdict is not in its frame");
    // x's result arrives without its ready: its id finds it, though two held calls had the tool.
    let sx = f
        .frame("r2", "tool", &result_now(1, "fx__send", &x), true)
        .expect("x's view, named by its result");
    assert_eq!(sx.call_id.as_deref(), Some(x.as_str()));
    assert!(matches!(outcome(&sx), Some(Outcome::Result { .. })));
    f.frame("r2", "tool", &result_now(0, "fx__send", &y), true);
    assert!(matches!(outcome(&sy), Some(Outcome::Result { .. })));
    assert_eq!(f.following(), 0);
}

#[test]
fn a_start_never_shows_its_arguments() {
    let mut f = Flow::default();
    let s = f
        .frame(
            "r",
            "tool",
            &ready(0, "fx__show", json!({"secret": "s3cr3t"})),
            true,
        )
        .unwrap();
    assert!(!format!("{s:?}").contains("s3cr3t"));
    f.frame("r", "tool", &result(0, "fx__show", None), true);
    assert!(!format!("{:?}", outcome(&s)).contains("sunny"));
}

fn approval(name: &str, arguments: &str, id: &str) -> Value {
    json!({"event": "approval", "approval_request_id": id, "server_label": "fx", "name": name,
           "arguments": arguments})
}

fn pending(name: &str, arguments: &str, id: &str) -> Value {
    json!({"pending_approvals": [{"approval_request_id": id, "server_label": "fx", "name": name,
                                  "arguments": arguments}]})
}

/// lmgw's ready frames today say nothing of an approval: the view shows, its input waits for the
/// result. A ready that says `needs_approval` is believed.
#[test]
fn a_calls_input_waits_until_kai_knows_it_runs() {
    let mut f = Flow::default();
    let s = f
        .frame("r1", "tool", &ready(0, "fx__show", json!({})), true)
        .unwrap();
    assert!(!runs(&s), "no needs_approval: not known to run");
    f.frame("r1", "tool", &result(0, "fx__show", None), true);
    assert!(runs(&s), "its result says it ran");
    let mut gated = ready(1, "fx__show", json!({}));
    gated["needs_approval"] = json!(true);
    assert!(
        f.frame("r1", "tool", &gated, true).is_none(),
        "a gated call shows nothing at its ready"
    );
    let mut free = ready(2, "fx__show", json!({}));
    free["needs_approval"] = json!(false);
    let free = f.frame("r1", "tool", &free, true).unwrap();
    assert!(runs(&free), "needs_approval: false runs");
    assert!(free.seq > s.seq, "Starts are numbered in order");
}

/// A turn that stops on a gated call keeps its calls held, uncancelled and without input; a
/// decision elsewhere ends them, saying so.
#[test]
fn a_gated_turns_calls_wait_and_a_decision_elsewhere_ends_them() {
    let mut f = Flow::default();
    let a = f
        .frame(
            "r1",
            "tool",
            &ready(0, "fx__send", json!({"to": "x"})),
            true,
        )
        .unwrap();
    let b = f
        .frame("r1", "tool", &ready(1, "fx__show", json!({})), true)
        .unwrap();
    f.frame(
        "r1",
        "tool",
        &approval("send", r#"{"to": "x"}"#, "mcpr_1"),
        true,
    );
    f.frame(
        "r1",
        "done",
        &pending("send", r#"{"to": "x"}"#, "mcpr_1"),
        true,
    );
    assert_eq!(outcome(&a), None, "held, not cancelled");
    assert_eq!(outcome(&b), None, "its sibling waits too");
    assert!(!runs(&a) && !runs(&b));
    assert_eq!(f.following(), 2);
    f.decided("mcpr_other", None, false, Some("the dashboard"));
    assert_eq!(outcome(&a), None, "another call's decision");
    f.decided("mcpr_1", None, false, Some("the dashboard"));
    assert_eq!(
        outcome(&a),
        Some(Outcome::Cancelled(
            "the call was declined by the dashboard".into()
        ))
    );
    assert!(!runs(&a), "a declined call never gets its input");
    assert!(
        matches!(outcome(&b), Some(Outcome::Cancelled(w)) if w.contains("decided in another client"))
    );
    assert_eq!(f.following(), 0);
    // Approved elsewhere: that client runs it; Kai says so and sends no input.
    let c = f
        .frame("r2", "tool", &ready(0, "fx__send", json!({})), true)
        .unwrap();
    f.frame("r2", "tool", &approval("send", "{}", "mcpr_2"), true);
    f.frame("r2", "done", &pending("send", "{}", "mcpr_2"), true);
    f.decided("mcpr_2", None, true, None);
    assert!(
        matches!(outcome(&c), Some(Outcome::Cancelled(w)) if w.contains("approved by another client"))
    );
    assert!(!runs(&c));
    // Nobody decided (its reply went).
    let d = f
        .frame("r3", "tool", &ready(0, "fx__send", json!({})), true)
        .unwrap();
    f.frame("r3", "tool", &approval("send", "{}", "mcpr_3"), true);
    f.frame("r3", "done", &pending("send", "{}", "mcpr_3"), true);
    f.session_ended();
    assert_eq!(
        outcome(&d),
        Some(Outcome::Cancelled("Kai's voice session ended".into()))
    );
}

/// A resumed turn in Kai's own session announces its decided calls again under a new response:
/// they are the held calls, not new ones, and a result whose ready was not seen finds its call.
#[test]
fn a_resumed_turn_finds_its_held_calls_instead_of_opening_new_views() {
    let mut f = Flow::default();
    let a = f
        .frame(
            "r1",
            "tool",
            &ready(0, "fx__send", json!({"to": "x"})),
            true,
        )
        .unwrap();
    // The same tool twice: the approval names which by its arguments.
    let b = f
        .frame(
            "r1",
            "tool",
            &ready(1, "fx__send", json!({"to": "y"})),
            true,
        )
        .unwrap();
    f.frame(
        "r1",
        "tool",
        &approval("send", r#"{"to":"y"}"#, "mcpr_y"),
        true,
    );
    f.frame(
        "r1",
        "tool",
        &approval("send", r#"{"to":"x"}"#, "mcpr_x"),
        true,
    );
    f.frame("r1", "done", &json!({"pending_approvals": [{}, {}]}), true);
    assert_eq!(f.following(), 2);
    // The resumed turn (needs_approval false for every decided call: not believed for them).
    let mut again = ready(0, "fx__send", json!({"to": "x"}));
    again["needs_approval"] = json!(false);
    assert!(
        f.frame("r2", "tool", &again, true).is_none(),
        "the view shown for the call stays the one"
    );
    assert!(!runs(&a), "its verdict is not in the frame");
    f.frame("r2", "tool", &result(0, "fx__send", None), true);
    assert!(matches!(outcome(&a), Some(Outcome::Result { .. })));
    assert!(runs(&a));
    // A result whose ready was not seen: the one held call of that tool.
    assert!(
        f.frame("r2", "tool", &result(1, "fx__send", None), true)
            .is_none(),
        "no second, empty view"
    );
    assert!(matches!(outcome(&b), Some(Outcome::Result { .. })));
    assert_eq!(f.following(), 0);
    // A call not shown at its ready (needs_approval: true) shows once it is resumed.
    let mut gated = ready(0, "fx__send", json!({}));
    gated["needs_approval"] = json!(true);
    assert!(f.frame("r3", "tool", &gated, true).is_none());
    f.frame("r3", "done", &pending("send", "{}", "mcpr_3"), true);
    let shown = f
        .frame("r4", "tool", &ready(0, "fx__send", json!({})), true)
        .expect("the resumed call's view is looked for now");
    assert!(!runs(&shown));
}

/// Shows that finish out of order: only the newest call's view may stay, and nothing looked for
/// in an ended session shows.
#[test]
fn only_the_newest_calls_view_of_the_current_session_shows() {
    let st = State {
        shown: Some((5, 42)),
        generation: 2,
        ..Default::default()
    };
    assert!(may_show(&st, 6, 2));
    assert!(
        !may_show(&st, 4, 2),
        "an older call's view never replaces a newer one's"
    );
    assert!(
        !may_show(&st, 6, 1),
        "a view looked for in an ended session"
    );
    assert!(may_show(&State::default(), 1, 0));
}
