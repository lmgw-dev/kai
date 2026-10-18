//! The approvals' bookkeeping (WP19): which call the card shows, when and where the verdicts go
//! and when lmgw took them, what a refusal and a resync read again, whose calls the voice answers,
//! and the spoken answer's speech check and matching. The voice's part (no audio while a call of
//! Kai's own waits, the session's items) is `voice::tests::approvals`.

use std::sync::{Arc, Mutex};

use lmgw_client::requests::approvals::{ApprovalDecision, ApprovalRequest, code};

use super::card::{CardAnswer, CardCmd, CardEvent, Look};
use super::*;
use crate::overlay::placement::Anchor;

type Cards = Arc<Mutex<Vec<CardCmd>>>;

fn approvals() -> (Approvals, Cards) {
    let cards: Cards = Arc::default();
    let c = cards.clone();
    let d = settings::ApprovalsConfig::default();
    let s = ApprovalSettings {
        yes: d.yes,
        no: d.no,
        speech_dbfs: d.speech_dbfs,
        speech_ms: d.speech_ms,
        look: Look {
            anchor: Anchor::Top,
            margin: 48,
            width: 560,
            arm: std::time::Duration::from_millis(600),
            output: None,
        },
    };
    (
        Approvals::new(s, Box::new(move |cmd| c.lock().unwrap().push(cmd))),
        cards,
    )
}

fn req(id: &str, name: &str, args: &str) -> ApprovalRequest {
    ApprovalRequest {
        approval_request_id: id.into(),
        server_label: "kai".into(),
        name: name.into(),
        arguments: args.into(),
        call_id: None,
    }
}

/// The card's last command: `Some(call)` shown, `None` hidden.
fn last(cards: &Cards) -> Option<Option<String>> {
    cards.lock().unwrap().last().map(|c| match c {
        CardCmd::Show { model, .. } => Some(model.call.clone()),
        CardCmd::Hide => None,
    })
}

fn card(a: &mut Approvals, e: CardEvent, session: Option<(i64, u64)>) -> Vec<Act> {
    a.on_wake(Wake::Card(e), session)
}

fn answer(a: &mut Approvals, call: &str, approve: bool, session: Option<(i64, u64)>) -> Vec<Act> {
    card(
        a,
        CardEvent::Answer(CardAnswer {
            call: call.into(),
            approve,
        }),
        session,
    )
}

fn armed(a: &mut Approvals, call: &str) {
    card(a, CardEvent::Armed { call: call.into() }, None);
}

fn route_done(a: &mut Approvals, thread_id: i64, ids: &[&str], r: Result<(), Refusal>) -> Vec<Act> {
    a.on_wake(
        Wake::Done(
            Op::Decide {
                thread_id,
                ids: ids.iter().map(|s| (*s).to_owned()).collect(),
            },
            Done::Decided(r),
        ),
        None,
    )
}

fn reread_done(a: &mut Approvals, thread_id: i64, r: Result<Vec<ApprovalRequest>, Refusal>) {
    let acts = a.on_wake(Wake::Done(Op::Reread { thread_id }, Done::Pending(r)), None);
    assert!(acts.is_empty(), "{acts:?}");
}

fn refused(c: &str) -> Refusal {
    Refusal {
        status: 409,
        code: Some(c.into()),
        message: format!("{c} message"),
    }
}

/// `ms` of a word at -20 dBFS (a square wave): speech by the default check.
fn spoken(ms: u64) -> Vec<i16> {
    let n = (crate::audio::RATE as u64 * ms / 1000) as usize;
    (0..n)
        .map(|i| if i % 2 == 0 { 3277 } else { -3277 })
        .collect()
}

#[test]
fn the_card_shows_a_call_and_goes_when_it_is_decided_elsewhere() {
    let (mut a, cards) = approvals();
    assert!(!a.waiting());
    a.turn_started(7, "device 'phone'", false);
    a.thread_title(7, "Plans");
    a.requested(
        7,
        req("mcpr_1", "window_close", r#"{"window":"Firefox"}"#),
        None,
    );
    assert!(a.waiting());
    let m = a.card().unwrap().clone();
    assert_eq!(m.heading, "Approve kai__window_close?");
    assert_eq!(m.asked, "asked by device 'phone' in thread 'Plans'");
    assert_eq!(m.args, vec!["window: Firefox".to_owned()]);
    assert_eq!(m.count, None);
    assert_eq!(last(&cards), Some(Some("mcpr_1".into())));
    // The same call from the bound session (the live one) is one call.
    a.observe(vec![], Some(1));
    a.requested(7, req("mcpr_1", "window_close", "{}"), Some(1));
    assert_eq!(
        cards.lock().unwrap().len(),
        2,
        "the card says the voice may answer it now, nothing else"
    );
    assert!(a.note().unwrap().contains("kai__window_close"));
    // Decided elsewhere: the card goes.
    a.decided("mcpr_1", true, Some("the dashboard"));
    assert!(!a.waiting());
    assert_eq!(last(&cards), Some(None));
    assert!(a.note().is_none());
    // A decision for a call Kai never saw changes nothing.
    a.decided("mcpr_9", false, None);
    assert_eq!(cards.lock().unwrap().len(), 3);
}

/// Device link §7.3: the feed's open set, at every connection, is what Kai reconciles against. A
/// call it lacks was decided while the feed was down and goes, unless the live session showed it
/// (the session says its decision itself); a call Kai did not show yet is shown, one it shows stays
/// as it is. The feed's withdrawal takes a call away like a decision.
#[test]
fn the_feeds_open_set_is_reconciled_against() {
    let (mut a, cards) = approvals();
    let feed = |id: &str, thread_id: i64| Requested {
        thread_id,
        request: req(id, "window_close", "{}"),
    };
    a.observe(vec![], Some(1));
    a.requested(7, req("mcpr_1", "window_close", "{}"), None);
    a.requested(7, req("mcpr_2", "window_close", "{}"), None);
    a.requested(8, req("mcpr_3", "window_close", "{}"), Some(1));
    assert_eq!(last(&cards), Some(Some("mcpr_1".into())));
    a.open_set(vec![feed("mcpr_2", 7), feed("mcpr_4", 9)]);
    let mut ids: Vec<&str> = a.calls.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["mcpr_2", "mcpr_3", "mcpr_4"],
        "mcpr_1 went, the session's mcpr_3 stayed, mcpr_4 came"
    );
    assert_eq!(last(&cards), Some(Some("mcpr_2".into())));
    // The session ended: its call is the feed's to say from now on.
    a.observe(vec![], None);
    a.open_set(vec![feed("mcpr_2", 7)]);
    a.withdrawn(&["mcpr_2".into(), "mcpr_9".into()]);
    assert!(!a.waiting());
    assert_eq!(last(&cards), Some(None));
}

/// A call lmgw said was decided (the live session's `lmgw.approval.decided`) does not come back
/// for a moment from a stale open set or a late `Added` read before the decision; the feed's
/// withdrawal, or the next open set, ends the guard.
#[test]
fn a_call_decided_here_does_not_flicker_back() {
    let (mut a, cards) = approvals();
    let feed = |id: &str| Requested {
        thread_id: 7,
        request: req(id, "window_close", "{}"),
    };
    a.observe(vec![], Some(1));
    a.requested(7, req("mcpr_1", "window_close", "{}"), Some(1));
    a.requested(7, req("mcpr_2", "window_close", "{}"), None);
    a.decided("mcpr_1", true, Some("device 'desktop'"));
    let shown = cards.lock().unwrap().len();
    // The feed reconnects; its set was read before the decision.
    a.open_set(vec![feed("mcpr_1"), feed("mcpr_2")]);
    let ids: Vec<&str> = a.calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["mcpr_2"], "the decided call is not shown again");
    assert_eq!(last(&cards), Some(Some("mcpr_2".into())));
    assert_eq!(cards.lock().unwrap().len(), shown, "the card never changed");
    // Decided again in the session; the feed's late Added, then its withdrawal.
    a.requested(7, req("mcpr_3", "window_close", "{}"), Some(1));
    a.decided("mcpr_3", false, None);
    a.requested(7, req("mcpr_3", "window_close", "{}"), None);
    assert!(!a.calls.iter().any(|c| c.id == "mcpr_3"), "a late Added");
    a.withdrawn(&["mcpr_3".into()]);
    assert!(a.decided_here.is_empty(), "the withdrawal ends the guard");
    // The next open set ends it too: a call that waits again (a new one) shows.
    a.decided("mcpr_2", true, None);
    a.open_set(vec![]);
    assert!(a.decided_here.is_empty());
}

/// lmgw takes a reply's verdicts together (api §6.3): the card asks each call, then one request.
#[test]
fn a_reply_with_two_calls_is_sent_once_both_have_a_verdict() {
    let (mut a, _cards) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    a.requested(7, req("mcpr_2", "b", "{}"), None);
    assert_eq!(
        a.card().unwrap().count.as_deref(),
        Some("call 1 of 2 this reply waits on")
    );
    // An answer for the call the card does not show answers nothing.
    assert!(answer(&mut a, "mcpr_2", true, None).is_empty());
    assert_eq!(a.card().unwrap().call, "mcpr_1");
    assert!(answer(&mut a, "mcpr_1", true, None).is_empty());
    assert_eq!(a.card().unwrap().call, "mcpr_2");
    assert_eq!(
        a.card().unwrap().count.as_deref(),
        Some("call 2 of 2 this reply waits on")
    );
    let acts = answer(&mut a, "mcpr_2", false, None);
    assert_eq!(
        acts,
        vec![Act::Route {
            thread_id: 7,
            body: ApprovalsRequest {
                decisions: vec![
                    ApprovalDecision {
                        approval_request_id: "mcpr_1".into(),
                        approve: true,
                        reason: None,
                    },
                    ApprovalDecision {
                        approval_request_id: "mcpr_2".into(),
                        approve: false,
                        reason: Some("declined on Kai".into()),
                    },
                ],
                speak: false,
            },
        }]
    );
    // On their way: the card shows nothing of that thread, but the calls still wait.
    assert!(a.card().is_none());
    assert!(a.waiting());
    // A new call of the thread meanwhile (the resumed reply's next) is its own batch.
    a.requested(7, req("mcpr_3", "c", "{}"), None);
    assert_eq!(a.card().unwrap().call, "mcpr_3");
    assert_eq!(a.card().unwrap().count, None);
    route_done(&mut a, 7, &["mcpr_1", "mcpr_2"], Ok(()));
    assert_eq!(a.card().unwrap().call, "mcpr_3");
    let acts = answer(&mut a, "mcpr_3", true, None);
    assert!(
        matches!(&acts[..], [Act::Route { body, .. }] if body.decisions.len() == 1),
        "only the new call: {acts:?}"
    );
}

/// The session that showed every call of the thread takes the verdicts while it is the bound one;
/// the calls stay until lmgw took them (the end of the response that carried them, or a decision
/// said), and a session that ended first sends them by the route. Another session, or a call it
/// did not show, goes through the route.
#[test]
fn the_verdicts_go_through_the_session_that_showed_the_calls_and_stay_until_taken() {
    let (mut a, _) = approvals();
    assert!(a.observe(vec![812], Some(3)).is_empty());
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    assert!(a.waiting_in_session(3));
    assert!(!a.waiting_in_session(4));
    assert_eq!(
        a.card().unwrap().asked,
        "asked by Kai's voice session in thread 812"
    );
    let acts = answer(&mut a, "mcpr_1", true, Some((812, 3)));
    assert!(
        matches!(&acts[..], [Act::Session { thread_id: 812, decisions, create }] if decisions.len() == 1 && decisions[0].approve && create == "kai_approvals_1"),
        "{acts:?}"
    );
    // Sent, not taken yet: the card shows nothing, the session stays, and no audio goes until
    // lmgw created the response that carries them (a turn committed before would be taken as it).
    assert!(a.card().is_none());
    assert!(a.waiting() && a.waiting_in_session(3));
    assert!(a.gates_audio(), "the create is not answered yet");
    // A response before it is not the one that carries them; the next one is.
    assert!(
        a.session_response_done(3, "resp_old", "completed")
            .is_empty()
    );
    a.session_response_created(3, "resp_1");
    assert!(
        !a.gates_audio(),
        "the resumed reply was created: it may be heard (and barged into)"
    );
    a.session_response_created(3, "resp_2");
    assert!(a.waiting());
    assert!(a.session_response_done(3, "resp_2", "completed").is_empty());
    assert!(a.waiting(), "not the verdicts' response");
    assert!(a.session_response_done(3, "resp_1", "completed").is_empty());
    assert!(!a.waiting(), "lmgw took them with the response");
    assert!(!a.waiting_in_session(3));

    // The feed's decision takes them too.
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    answer(&mut a, "mcpr_1", false, Some((812, 3)));
    a.decided("mcpr_1", false, Some("device 'kai'"));
    assert!(!a.waiting() && !a.waiting_in_session(3));
    assert!(
        a.observe(vec![812], None).is_empty(),
        "nothing left to send"
    );

    // The session ends before lmgw took them: the route sends the same verdicts.
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    answer(&mut a, "mcpr_1", false, Some((812, 3)));
    a.session_response_created(3, "resp_1");
    let acts = a.observe(vec![812], None);
    let [
        Act::Route {
            thread_id: 812,
            body,
        },
    ] = &acts[..]
    else {
        panic!("{acts:?}");
    };
    assert_eq!(body.decisions.len(), 1);
    assert!(!body.decisions[0].approve);
    assert_eq!(body.decisions[0].reason.as_deref(), Some("declined on Kai"));
    assert!(a.observe(vec![812], None).is_empty(), "sent once");
    route_done(&mut a, 812, &["mcpr_1"], Ok(()));
    assert!(!a.waiting());

    // A session bound since (a rebind, a drop and a new bind) did not show it.
    let (mut a, _) = approvals();
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    let acts = answer(&mut a, "mcpr_1", true, Some((812, 4)));
    assert!(
        matches!(&acts[..], [Act::Route { thread_id: 812, .. }]),
        "{acts:?}"
    );

    let (mut a, _) = approvals();
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    a.requested(812, req("mcpr_2", "b", "{}"), None);
    answer(&mut a, "mcpr_1", true, Some((812, 3)));
    let acts = answer(&mut a, "mcpr_2", true, Some((812, 3)));
    assert!(
        matches!(&acts[..], [Act::Route { .. }]),
        "one call the session did not show: {acts:?}"
    );
}

/// A session's `error` with an approval code, or a failed response that carried the verdicts:
/// the thread is read again before anything is asked again.
#[test]
fn a_session_refusal_or_failure_reads_the_thread_again() {
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    answer(&mut a, "mcpr_1", true, Some((812, 3)));
    let acts = a.session_refused(3, None, refused(code::APPROVAL_MOVED_ON));
    assert_eq!(acts, vec![Act::Reread { thread_id: 812 }]);
    assert!(a.card().is_none(), "nothing asked while it is read");
    reread_done(&mut a, 812, Ok(vec![]));
    assert!(!a.waiting());
    let n = a.take_notices();
    assert!(n[0].1.contains(code::APPROVAL_MOVED_ON), "{n:?}");

    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    answer(&mut a, "mcpr_1", true, Some((812, 3)));
    a.session_response_created(3, "resp_1");
    let acts = a.session_response_done(3, "resp_1", "failed");
    assert_eq!(acts, vec![Act::Reread { thread_id: 812 }]);
    reread_done(&mut a, 812, Ok(vec![req("mcpr_1", "a", "{}")]));
    let m = a.card().unwrap();
    assert_eq!(m.call, "mcpr_1");
    assert!(m.note.as_deref().unwrap().contains("Answer again"), "{m:?}");

    // Cancelled or incomplete is no "taken" either: read again; nothing waits, so nothing is said.
    for status in ["cancelled", "incomplete"] {
        let (mut a, _) = approvals();
        a.observe(vec![812], Some(3));
        a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
        answer(&mut a, "mcpr_1", true, Some((812, 3)));
        a.session_response_created(3, "resp_1");
        let acts = a.session_response_done(3, "resp_1", status);
        assert_eq!(acts, vec![Act::Reread { thread_id: 812 }], "{status}");
        assert!(a.waiting(), "{status}: kept until the thread was read");
        reread_done(&mut a, 812, Ok(vec![]));
        assert!(!a.waiting());
        assert!(a.take_notices().is_empty(), "{status}");
    }
}

/// M1: lmgw refuses the `response.create` that carries the verdicts for a reason of its own (an
/// active response, the GPU hold, a model's error): its `error` echoes the create's `event_id`, the
/// verdicts are not left waiting for a response that never comes, the thread is read again and
/// what still waits is asked again; the next response the session creates (the owner's next turn)
/// is not taken for theirs. An `error` echoing another id changes nothing.
#[test]
fn a_refused_create_is_read_again_not_left_waiting() {
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    let Act::Session { create, .. } = answer(&mut a, "mcpr_1", true, Some((812, 3))).remove(0)
    else {
        panic!("through the session")
    };
    assert!(
        a.session_refused(3, Some("event_other"), refused("x"))
            .is_empty()
    );
    assert!(a.gates_audio(), "still waiting for its response");
    let acts = a.session_refused(
        3,
        Some(&create),
        refused("conversation_already_has_active_response"),
    );
    assert_eq!(acts, vec![Act::Reread { thread_id: 812 }]);
    assert!(a.waiting(), "the call is still known");
    // The owner's next turn: its response is not the verdicts'.
    a.session_response_created(3, "resp_turn");
    assert!(
        a.session_response_done(3, "resp_turn", "completed")
            .is_empty()
    );
    reread_done(&mut a, 812, Ok(vec![req("mcpr_1", "a", "{}")]));
    let m = a.card().unwrap();
    assert_eq!(m.call, "mcpr_1", "asked again");
    assert!(
        m.note
            .as_deref()
            .unwrap()
            .contains("conversation_already_has_active_response"),
        "{m:?}"
    );
    assert!(a.gates_audio(), "Kai's call waits again");
    // Answered again: a new create, with its own id.
    let acts = answer(&mut a, "mcpr_1", true, Some((812, 3)));
    assert!(
        matches!(&acts[..], [Act::Session { create: c, .. }] if *c != create),
        "{acts:?}"
    );
}

/// L5: a session that ends after the resumed reply started but before its end sends the verdicts
/// by the route; lmgw's `approval_decided` there means it took them in the session: taken, no
/// "not taken" notice. A first sending by the route keeps reading the thread on it.
#[test]
fn a_resend_after_the_session_ended_takes_approval_decided_as_taken() {
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    answer(&mut a, "mcpr_1", true, Some((812, 3)));
    a.session_response_created(3, "resp_1");
    let acts = a.observe(vec![812], None);
    assert!(matches!(&acts[..], [Act::Route { .. }]), "{acts:?}");
    let acts = route_done(
        &mut a,
        812,
        &["mcpr_1"],
        Err(refused(code::APPROVAL_DECIDED)),
    );
    assert!(acts.is_empty(), "{acts:?}");
    assert!(!a.waiting());
    assert!(a.take_notices().is_empty());

    let (mut a, _) = approvals();
    a.requested(812, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    let acts = route_done(
        &mut a,
        812,
        &["mcpr_1"],
        Err(refused(code::APPROVAL_DECIDED)),
    );
    assert_eq!(acts, vec![Act::Reread { thread_id: 812 }]);
}

/// L4: a call an ended session showed is Kai's own only through the conversation's thread: in
/// another thread it holds back no audio and the voice does not answer it.
#[test]
fn an_ended_sessions_call_is_not_kais_own_by_that_alone() {
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(900, req("mcpr_1", "a", "{}"), Some(3));
    assert!(a.gates_audio(), "the live session showed it");
    a.observe(vec![], None);
    assert!(!a.gates_audio(), "its session ended; not Kai's thread");
    armed(&mut a, "mcpr_1");
    assert!(!a.voice_answers());
    // A later session (4) does not make it its own either.
    a.observe(vec![812], Some(4));
    assert!(!a.gates_audio());
    // In the conversation's thread it is Kai's own whatever the session.
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(3));
    a.requested(812, req("mcpr_1", "a", "{}"), Some(3));
    a.observe(vec![812], None);
    assert!(a.gates_audio());
}

#[test]
fn a_refusal_keeps_the_calls_or_reads_the_thread_again() {
    // Beyond the device's reach: asked again, and the card says why.
    let (mut a, _) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    assert!(
        route_done(
            &mut a,
            7,
            &["mcpr_1"],
            Err(refused(code::APPROVAL_OUT_OF_SCOPE))
        )
        .is_empty()
    );
    let m = a.card().unwrap();
    assert_eq!(m.call, "mcpr_1");
    assert!(
        m.note
            .as_deref()
            .unwrap()
            .contains("Decline here, or approve in lmgw"),
        "{m:?}"
    );
    assert!(a.take_notices().is_empty());
    // A declining answer then goes.
    assert!(matches!(
        &answer(&mut a, "mcpr_1", false, None)[..],
        [Act::Route { .. }]
    ));

    // No answer at all (a timeout; lmgw may have decided): read again before asking again.
    let (mut a, _) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    let acts = route_done(
        &mut a,
        7,
        &["mcpr_1"],
        Err(Refusal::no_answer("lmgw did not answer")),
    );
    assert_eq!(acts, vec![Act::Reread { thread_id: 7 }]);
    assert!(a.card().is_none());
    // lmgw took it after all: nothing waits, nothing to say.
    reread_done(&mut a, 7, Ok(vec![]));
    assert!(!a.waiting());
    assert!(a.take_notices().is_empty());
    // Or it still waits: asked again.
    let (mut a, _) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    route_done(
        &mut a,
        7,
        &["mcpr_1"],
        Err(Refusal::no_answer("lmgw did not answer")),
    );
    reread_done(&mut a, 7, Ok(vec![req("mcpr_1", "a", "{}")]));
    assert!(
        a.card()
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .contains("Answer again")
    );

    // Decided already, moved on, gone: nothing waits any more; a notification says so.
    for c in [
        code::APPROVAL_DECIDED,
        code::APPROVAL_MOVED_ON,
        code::APPROVAL_NOT_FOUND,
        code::APPROVAL_STARTER_UNAVAILABLE,
    ] {
        let (mut a, cards) = approvals();
        a.requested(7, req("mcpr_1", "a", "{}"), None);
        answer(&mut a, "mcpr_1", true, None);
        let acts = route_done(&mut a, 7, &["mcpr_1"], Err(refused(c)));
        assert_eq!(acts, vec![Act::Reread { thread_id: 7 }], "{c}");
        reread_done(&mut a, 7, Ok(vec![]));
        assert!(!a.waiting(), "{c}");
        assert_eq!(last(&cards), Some(None));
        let n = a.take_notices();
        assert!(n[0].1.contains(c), "{n:?}");
    }

    // The thread cannot be read: the calls stay, asked again, the card says both.
    let (mut a, _) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    route_done(&mut a, 7, &["mcpr_1"], Err(refused(code::APPROVAL_DECIDED)));
    reread_done(&mut a, 7, Err(Refusal::no_answer("no network")));
    let note = a.card().unwrap().note.clone().unwrap();
    assert!(
        note.contains(code::APPROVAL_DECIDED) && note.contains("no network"),
        "{note}"
    );
}

/// A stale call spoils no batch: `approval_missing` (a call Kai never saw) and a resync read the
/// thread again, and the batch is what lmgw lists now: a call decided meanwhile goes, a call Kai
/// missed comes, the valid verdicts are not dropped with them.
#[test]
fn a_stale_batch_is_rebuilt_from_the_thread() {
    let (mut a, _) = approvals();
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    answer(&mut a, "mcpr_1", true, None);
    let acts = route_done(&mut a, 7, &["mcpr_1"], Err(refused(code::APPROVAL_MISSING)));
    assert_eq!(acts, vec![Act::Reread { thread_id: 7 }]);
    // lmgw waits on mcpr_1 and on mcpr_2, which a resync hid from Kai.
    reread_done(
        &mut a,
        7,
        Ok(vec![req("mcpr_1", "a", "{}"), req("mcpr_2", "b", "{}")]),
    );
    assert_eq!(a.card().unwrap().call, "mcpr_1");
    assert_eq!(
        a.card().unwrap().count.as_deref(),
        Some("call 1 of 2 this reply waits on")
    );
    answer(&mut a, "mcpr_1", true, None);
    let acts = answer(&mut a, "mcpr_2", false, None);
    let [Act::Route { body, .. }] = &acts[..] else {
        panic!("{acts:?}");
    };
    assert_eq!(body.decisions.len(), 2, "the whole reply now");

    // The feed's resync: every thread Kai knows calls of, and its own, read again.
    let (mut a, _) = approvals();
    a.observe(vec![812], None);
    a.requested(5, req("mcpr_5", "a", "{}"), None);
    a.requested(6, req("mcpr_6", "a", "{}"), None);
    let mut acts = a.resync();
    acts.sort_by_key(|a| match a {
        Act::Reread { thread_id } => *thread_id,
        _ => 0,
    });
    assert_eq!(
        acts,
        vec![
            Act::Reread { thread_id: 5 },
            Act::Reread { thread_id: 6 },
            Act::Reread { thread_id: 812 }
        ]
    );
    // The card stays while they are read; a second resync asks nothing more.
    assert_eq!(a.card().unwrap().call, "mcpr_5");
    assert!(a.resync().is_empty());
    reread_done(&mut a, 5, Ok(vec![]));
    reread_done(&mut a, 6, Ok(vec![req("mcpr_6", "a", "{}")]));
    reread_done(&mut a, 812, Ok(vec![req("mcpr_9", "x", "{}")]));
    assert_eq!(a.card().unwrap().call, "mcpr_6");
    assert!(a.take_notices().is_empty(), "a resync says nothing");
    a.decided("mcpr_6", true, None);
    assert_eq!(a.card().unwrap().call, "mcpr_9", "the call Kai missed");
    assert!(a.gates_audio(), "in Kai's own thread");
}

/// Only a call of Kai's own (its session showed it, or it is in the conversation's thread) holds
/// back the session's audio and takes the spoken answer, and only once the card armed for it.
/// Another thread's call is the card's alone.
#[test]
fn the_voice_answers_only_kais_own_call_once_the_card_armed() {
    let (mut a, _) = approvals();
    a.observe(vec![812], Some(1));
    // A phone's turn in another thread waits.
    a.turn_started(5, "device 'phone'", false);
    a.requested(5, req("mcpr_5", "a", "{}"), None);
    armed(&mut a, "mcpr_5");
    assert!(
        !a.gates_audio(),
        "another thread's call holds back no audio"
    );
    assert!(!a.voice_answers(), "and the talk key keeps its meaning");
    assert!(!a.press(vec![]));
    let m = a.card().unwrap();
    assert_eq!(m.hint, "Enter approves, Esc declines.");
    assert!(!m.typed.contains("voice"));
    assert!(
        a.note().unwrap().ends_with("answer on the card"),
        "{:?}",
        a.note()
    );

    // Kai's own thread: the audio is held back at once, the voice waits for the card's arming.
    let (mut a, _) = approvals();
    a.observe(vec![812], None);
    a.requested(812, req("mcpr_1", "a", "{}"), None);
    assert!(a.gates_audio());
    assert!(!a.voice_answers(), "not armed yet: a press is talk");
    assert!(!a.press(vec![]));
    armed(&mut a, "mcpr_1");
    assert!(a.voice_answers());
    // The card waits again (the keyboard came back to it): the voice waits too.
    card(
        &mut a,
        CardEvent::Unarmed {
            call: "mcpr_1".into(),
        },
        None,
    );
    assert!(!a.voice_answers());
    // An arming for a call the card no longer shows counts for nothing.
    armed(&mut a, "mcpr_old");
    assert!(!a.voice_answers());
    armed(&mut a, "mcpr_1");
    assert!(a.card().unwrap().hint.contains("talk key"));
    assert!(a.press(vec![]));
    // The conversation moved to another thread: the call is the card's alone now.
    a.cancel_recording();
    a.observe(vec![900], None);
    assert!(!a.gates_audio() && !a.voice_answers());
}

#[test]
fn the_spoken_answer_is_recorded_checked_for_speech_transcribed_and_matched() {
    let (mut a, _) = approvals();
    // Nothing waits: the press records nothing.
    assert!(!a.press(vec![]));
    a.observe(vec![7], Some(1));
    a.requested(7, req("mcpr_1", "a", "{}"), Some(1));
    armed(&mut a, "mcpr_1");
    // Near silence (a tap, a held key over a quiet room): no answer, nothing sent.
    assert!(a.press(vec![1; 10]));
    a.record(&[3; 4800]);
    assert!(a.release().is_empty());
    let note = a.card().unwrap().note.clone().unwrap();
    assert!(note.starts_with("Kai heard no speech (0 ms"), "{note}");
    assert!(note.contains("approvals.speech_ms"), "{note}");
    // Speech, after a quiet pre-roll.
    assert!(a.press(vec![1; 10]));
    assert!(a.recording());
    assert!(
        a.card()
            .unwrap()
            .busy
            .as_deref()
            .unwrap()
            .starts_with("Listening")
    );
    a.record(&spoken(300));
    let acts = a.release();
    let [
        Act::Transcribe {
            thread_id,
            call,
            wav,
        },
    ] = &acts[..]
    else {
        panic!("{acts:?}");
    };
    assert_eq!((*thread_id, call.as_str()), (7, "mcpr_1"));
    assert_eq!(wav.len(), 44 + (10 + 7200) * 2, "the pre-roll and the hold");
    assert!(!a.recording());
    assert_eq!(
        a.card().unwrap().busy.as_deref(),
        Some("Hearing the answer…")
    );
    // A second press while the first answer is heard waits.
    assert!(!a.press(vec![]));
    let heard = |a: &mut Approvals, r: Result<String, Refusal>| {
        a.on_wake(
            Wake::Done(
                Op::Hear {
                    call: "mcpr_1".into(),
                },
                Done::Heard(r),
            ),
            Some((7, 1)),
        )
    };
    let say = |a: &mut Approvals| {
        assert!(a.press(vec![]));
        a.record(&spoken(300));
        assert_eq!(a.release().len(), 1);
    };
    // Neither yes nor no: the card stays and says so, the owner's words on it. A filler word is
    // neither.
    for words in ["vielleicht später", "Okay."] {
        assert!(heard(&mut a, Ok(words.into())).is_empty());
        let m = a.card().unwrap();
        assert_eq!(m.busy, None);
        assert_eq!(
            m.note.as_deref(),
            Some(format!("Heard “{words}”: say yes or no, or click.").as_str())
        );
        say(&mut a);
    }
    // Nothing heard, and a failed transcription: the card stays too.
    assert!(heard(&mut a, Ok("  ".into())).is_empty());
    assert!(
        a.card()
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .starts_with("Kai heard nothing")
    );
    say(&mut a);
    let e = Refusal {
        status: 422,
        code: Some("asr_not_configured".into()),
        message: "no speech-to-text".into(),
    };
    assert!(heard(&mut a, Err(e)).is_empty());
    assert!(
        a.card()
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .contains("asr_not_configured")
    );
    // "Nein." declines, through the session that showed it.
    say(&mut a);
    let acts = heard(&mut a, Ok("Nein.".into()));
    assert!(
        matches!(&acts[..], [Act::Session { thread_id: 7, decisions, .. }] if !decisions[0].approve),
        "{acts:?}"
    );
    assert!(
        a.gates_audio(),
        "answered; the gate holds until lmgw created the response"
    );
    a.session_response_created(1, "resp_1");
    assert!(!a.gates_audio(), "answered");
}

#[test]
fn a_call_that_goes_while_its_answer_is_recorded_or_heard_takes_the_answer_along() {
    let (mut a, _) = approvals();
    a.observe(vec![7], None);
    a.requested(7, req("mcpr_1", "a", "{}"), None);
    armed(&mut a, "mcpr_1");
    assert!(a.press(vec![]));
    a.decided("mcpr_1", false, None);
    assert!(!a.recording());
    assert!(a.release().is_empty());
    a.requested(7, req("mcpr_2", "a", "{}"), None);
    assert!(!a.voice_answers(), "a new call arms anew");
    armed(&mut a, "mcpr_2");
    a.press(vec![]);
    a.record(&spoken(300));
    a.release();
    a.thread_gone(7);
    assert!(!a.waiting());
    let acts = a.on_wake(
        Wake::Done(
            Op::Hear {
                call: "mcpr_2".into(),
            },
            Done::Heard(Ok("ja".into())),
        ),
        None,
    );
    assert!(
        acts.is_empty(),
        "heard for a call that went: nothing is sent"
    );
    // The gateway left: everything goes, and nothing on its way is waited for.
    a.requested(7, req("mcpr_3", "a", "{}"), None);
    a.requested(8, req("mcpr_4", "a", "{}"), None);
    a.push(
        Op::Reread { thread_id: 8 },
        Box::pin(std::future::pending()),
    );
    a.forget_all();
    assert!(!a.waiting() && a.card().is_none());
    assert!(a.ops.is_empty(), "the requests on their way are dropped");
}

/// A card that cannot show (its page ended twice, no layer-shell): the owner is told to answer in
/// lmgw, the voice is not held back for it, and another thread's call still shows.
#[test]
fn a_call_the_card_cannot_show_is_left_to_lmgw() {
    let (mut a, cards) = approvals();
    a.observe(vec![812], None);
    a.requested(812, req("mcpr_1", "a", "{}"), None);
    a.requested(5, req("mcpr_5", "b", "{}"), None);
    armed(&mut a, "mcpr_1");
    assert!(a.gates_audio());
    card(
        &mut a,
        CardEvent::Lost {
            call: "mcpr_1".into(),
            why: "its page ended twice (Crashed)".into(),
        },
        None,
    );
    assert!(
        !a.gates_audio(),
        "nothing on screen says why Kai would not listen"
    );
    assert!(!a.voice_answers());
    let n = a.take_notices();
    assert!(
        n[0].1.contains("kai__a") && n[0].1.contains("answer it in lmgw"),
        "{n:?}"
    );
    assert!(
        a.note().unwrap().contains("cannot show it"),
        "{:?}",
        a.note()
    );
    // The other thread's call shows on a new card.
    assert_eq!(last(&cards), Some(Some("mcpr_5".into())));
    // Decided in lmgw: it goes.
    a.decided("mcpr_1", true, Some("the dashboard"));
    assert!(!a.note().unwrap().contains("cannot show"));
}

/// The card's texts are escaped and name who asked by kind: a label, a tool, a principal or a
/// title cannot forge a line or pass for Kai's session.
#[test]
fn names_on_the_card_are_escaped() {
    let (mut a, _) = approvals();
    a.turn_started(7, "key 'a\nb'", false);
    a.thread_title(7, "t\u{202e}x");
    a.requested(
        7,
        ApprovalRequest {
            approval_request_id: "mcpr_1".into(),
            server_label: "l".into(),
            name: "n\u{202e}m".into(),
            arguments: "{}".into(),
            call_id: None,
        },
        None,
    );
    let m = a.card().unwrap();
    assert_eq!(m.heading, "Approve \"l__n\\u202em\"?");
    assert_eq!(
        m.asked,
        "asked by a principal lmgw names \"key 'a\\nb'\" in thread \"t\\u202ex\""
    );
    // A device named as Kai's session reads as a device.
    a.turn_started(7, "device 'Kai's voice session'", false);
    a.thread_title(7, "Plans");
    a.requested(7, req("mcpr_2", "a", "{}"), None);
    a.decided("mcpr_1", false, None);
    assert_eq!(
        a.card().unwrap().asked,
        "asked by device \"Kai's voice session\" in thread 'Plans'"
    );
}

/// The call's id (lmgw since 2026-10-09) rides on the request; the card's key stays the
/// `approval_request_id`. A bound session's `mcp_approval_request` item has none, the relayed
/// `approval` frame supplies it, and a later announcement fills it in.
#[test]
fn the_cards_call_learns_the_calls_id_without_changing_its_key() {
    let (mut a, _cards) = approvals();
    let with_id = |id: &str, call: Option<&str>| ApprovalRequest {
        call_id: call.map(str::to_owned),
        ..req(id, "clipboard_set", "{}")
    };
    // The session's item: no id.
    a.requested(7, with_id("mcpr_1", None), Some(1));
    assert_eq!(a.call_id_of("mcpr_1"), None);
    // The relayed frame names it (an empty one or another approval's changes nothing).
    a.call_id_seen("mcpr_1", "");
    a.call_id_seen("mcpr_other", "call_x");
    assert_eq!(a.call_id_of("mcpr_1"), None);
    a.call_id_seen("mcpr_1", "call_1");
    assert_eq!(a.call_id_of("mcpr_1"), Some("call_1"));
    // The first id stays; the card is still keyed by the approval id (one call, not two).
    a.call_id_seen("mcpr_1", "call_other");
    a.requested(7, with_id("mcpr_1", Some("call_other")), None);
    assert_eq!(a.call_id_of("mcpr_1"), Some("call_1"));
    assert_eq!(a.card().unwrap().call, "mcpr_1");
    // The feed's announcement of a call the card knew without the id fills it in.
    a.requested(8, with_id("mcpr_2", None), Some(1));
    a.requested(8, with_id("mcpr_2", Some("call_2")), None);
    assert_eq!(a.call_id_of("mcpr_2"), Some("call_2"));
    // A call announced with its id from the start (the feed, a pending list).
    a.requested(9, with_id("mcpr_3", Some("call_3")), None);
    assert_eq!(a.call_id_of("mcpr_3"), Some("call_3"));
    a.decided("mcpr_3", true, None);
    assert_eq!(a.call_id_of("mcpr_3"), None, "gone with its call");
}
