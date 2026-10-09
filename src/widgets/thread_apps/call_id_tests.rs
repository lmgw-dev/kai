//! An approval request carries the call's id (lmgw since 2026-10-09): the flow matches the
//! `approval` frame and `done`'s `pending_approvals` to calls by it, and falls back to the tool and
//! the arguments for an older gateway.

use serde_json::json;

use super::*;

fn ready_id(index: u64, call_id: &str) -> Value {
    json!({"event": "ready", "index": index, "name": "fx__send", "arguments": {"to": "x"},
           "call_id": call_id, "server_label": "fx", "needs_approval": true,
           "ui_resource": "ui://fx__card"})
}

fn approval_id(id: &str, call_id: Option<&str>) -> Value {
    let mut v = json!({"event": "approval", "approval_request_id": id, "server_label": "fx",
                       "name": "send", "arguments": r#"{"to": "x"}"#});
    if let Some(c) = call_id {
        v["call_id"] = json!(c);
    }
    v
}

fn pending_id(id: &str, call_id: Option<&str>) -> Value {
    let mut v = approval_id(id, call_id);
    v.as_object_mut().unwrap().remove("event");
    json!({"pending_approvals": [v]})
}

fn outcome(s: &Start) -> Option<Outcome> {
    s.progress.borrow().outcome.clone()
}

/// Two calls of the same tool with the same arguments: only the call id tells which waits.
#[test]
fn an_approval_finds_its_call_by_id_not_by_tool_and_arguments() {
    let mut f = Flow::default();
    let _a = f.frame("r1", "tool", &ready_id(0, "call_a"), true);
    let b = f.frame("r1", "tool", &ready_id(1, "call_b"), true);
    assert!(b.is_none(), "a gated call shows nothing yet");
    // Only call_b is gated, and the tool and arguments fit both: by tool it would be call_a.
    f.frame("r1", "tool", &approval_id("mcpr_b", Some("call_b")), true);
    f.frame("r1", "done", &pending_id("mcpr_b", Some("call_b")), true);
    assert_eq!(f.following(), 2);
    // The request's id went to call_b, though the tool and arguments fit both calls.
    assert_eq!(
        f.marks(),
        [
            (Some("call_a".to_owned()), None),
            (Some("call_b".to_owned()), Some("mcpr_b".to_owned()))
        ]
    );
}

/// The same request is marked by its frame and again by `done`: the same call both times, not the
/// next open one of the tool.
#[test]
fn the_frame_and_done_mark_the_same_call() {
    let mut f = Flow::default();
    f.frame("r1", "tool", &ready_id(0, "call_a"), true);
    f.frame("r1", "tool", &ready_id(1, "call_b"), true);
    f.frame("r1", "tool", &approval_id("mcpr_a", Some("call_a")), true);
    f.frame("r1", "done", &pending_id("mcpr_a", Some("call_a")), true);
    assert_eq!(
        f.marks(),
        [
            (Some("call_a".to_owned()), Some("mcpr_a".to_owned())),
            (Some("call_b".to_owned()), None)
        ]
    );
}

/// An older gateway's request has no id, and a call without one cannot be found by it: the tool
/// and the arguments find it, as before.
#[test]
fn without_an_id_the_tool_and_arguments_still_match() {
    let mut f = Flow::default();
    let ready = json!({"event": "ready", "index": 0, "name": "fx__send",
                       "arguments": {"to": "x"}});
    let a = f.frame("r1", "tool", &ready, true).unwrap();
    f.frame("r1", "tool", &approval_id("mcpr_1", None), true);
    f.frame("r1", "done", &pending_id("mcpr_1", None), true);
    assert_eq!(f.marks(), [(None, Some("mcpr_1".to_owned()))]);
    f.decided("mcpr_1", None, false, Some("the dashboard"));
    assert!(matches!(outcome(&a), Some(Outcome::Cancelled(_))));
    // A request with an id no call of the response has (a ready without one): the fallback too.
    let mut g = Flow::default();
    g.frame("r1", "tool", &ready, true);
    g.frame(
        "r1",
        "tool",
        &approval_id("mcpr_2", Some("call_unknown")),
        true,
    );
    assert_eq!(g.marks(), [(None, Some("mcpr_2".to_owned()))]);
}

/// The decision names the approval id; when the frame was not matched, the card's call id finds
/// the held call.
#[test]
fn a_decision_finds_a_held_call_by_the_cards_call_id() {
    let mut f = Flow::default();
    let a = f.frame("r1", "tool", &ready_id(0, "call_a"), true);
    assert!(a.is_none());
    let b = f.frame("r1", "tool", &ready_id(1, "call_b"), true);
    assert!(b.is_none());
    // The turn stops with the approval unmatched (no frame relayed, none named in `done`).
    f.frame(
        "r1",
        "done",
        &json!({"pending_approvals": [{"x": 1}]}),
        true,
    );
    assert_eq!(f.following(), 2);
    f.decided("mcpr_x", Some("call_b"), true, Some("the phone"));
    assert_eq!(
        f.following(),
        0,
        "call_b ended by its id, and its turn's last gated call is decided"
    );
}
