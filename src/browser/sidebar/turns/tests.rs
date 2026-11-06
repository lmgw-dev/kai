use lmgw_client::requests::approvals::ApprovalRequest;

use super::*;
use crate::approvals::Requested;

fn own() -> By {
    By {
        name: "device 'desktop'".into(),
        own: true,
        sidebar: false,
    }
}

fn phone() -> By {
    By {
        name: "device 'phone'".into(),
        own: false,
        sidebar: false,
    }
}

fn started(thread_id: i64, by: By, voice: bool) -> FeedFact {
    FeedFact::TurnStarted {
        thread_id,
        by,
        voice,
    }
}

fn done(thread_id: i64) -> FeedFact {
    FeedFact::TurnDone { thread_id }
}

/// Who the voice hears start the turn.
fn by(turns: &SidebarTurns, f: FeedFact) -> By {
    match turns.fact(f) {
        Some(FeedFact::TurnStarted { by, .. }) => by,
        other => panic!("not a turn: {other:?}"),
    }
}

/// A call of `thread` waits (the feed's `approval.requested`).
fn waits(turns: &SidebarTurns, thread_id: i64, id: &str) {
    turns.fact(FeedFact::ApprovalRequested(Requested {
        thread_id,
        request: ApprovalRequest {
            approval_request_id: id.into(),
            server_label: "kai".into(),
            name: "clipboard_set".into(),
            arguments: "{}".into(),
            call_id: None,
        },
    }));
}

fn decided(turns: &SidebarTurns, id: &str) {
    turns.fact(FeedFact::ApprovalsWithdrawn(vec![id.into()]));
}

fn live(turns: &SidebarTurns, list: Vec<(i64, By, bool)>) -> Vec<(i64, By, bool)> {
    match turns.fact(FeedFact::Live {
        voice: vec![],
        turns: list,
    }) {
        Some(FeedFact::Live { turns, .. }) => turns,
        other => panic!("not live: {other:?}"),
    }
}

/// The sidebar's turn on thread 12 that ends waiting on call `mcpr_1`; its send ended.
fn a_sidebar_turn_waits(turns: &SidebarTurns) {
    let send = turns.mark(12, Start::New);
    assert_eq!(by(turns, started(12, own(), false)), By::sidebar());
    waits(turns, 12, "mcpr_1");
    assert!(turns.fact(done(12)).is_some());
    drop(send);
}

#[test]
fn a_marked_threads_next_own_turn_is_the_sidebars_and_only_that_one() {
    let turns = SidebarTurns::default();
    let mark = turns.mark(12, Start::New);
    // Another thread's own turn, and another principal's turn in the thread, are not it.
    assert_eq!(by(&turns, started(13, own(), false)), own());
    assert_eq!(by(&turns, started(12, phone(), false)), phone());
    // Kai's own voice turn is never the sidebar's.
    assert_eq!(by(&turns, started(12, own(), true)), own());
    // The own turn the request started.
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    // The mark is spent: an own turn there that is neither marked nor a resume is Kai's.
    turns.fact(done(12));
    assert_eq!(by(&turns, started(12, own(), false)), own());
    drop(mark);
}

/// L1: the feed reaches Kai on its own connection, so a short turn's stream can end before its
/// `turn.started` is read. The mark waits for it; unspent, it goes at the thread's next start or
/// end, or with a running list that does not name the thread. A refused request's goes at once.
#[test]
fn a_mark_outlives_its_request_until_the_feed_is_read() {
    let turns = SidebarTurns::default();
    drop(turns.mark(12, Start::New));
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    turns.fact(done(12));
    // Unspent, it goes at the thread's next turn.started of anyone's …
    drop(turns.mark(12, Start::New));
    assert_eq!(by(&turns, started(12, phone(), false)), phone());
    turns.fact(done(12));
    assert_eq!(by(&turns, started(12, own(), false)), own());
    turns.fact(done(12));
    // … or turn.done …
    drop(turns.mark(14, Start::New));
    turns.fact(done(14));
    assert_eq!(by(&turns, started(14, own(), false)), own());
    turns.fact(done(14));
    // … or a running list without its thread; an open request's stays.
    let open = turns.mark(16, Start::New);
    drop(turns.mark(18, Start::New));
    live(&turns, vec![]);
    assert_eq!(by(&turns, started(18, own(), false)), own());
    assert_eq!(by(&turns, started(16, own(), false)), By::sidebar());
    drop(open);
    // A request lmgw refused started no turn.
    turns.mark(20, Start::New).no_turn();
    assert_eq!(by(&turns, started(20, own(), false)), own());
}

/// H1: lmgw runs a resumed turn as its starter, whoever decided the calls. A resume of the
/// sidebar's turn is the sidebar's: verdicts from Kai's card (the withdrawal before the start or
/// after it), from the phone or the dashboard.
#[test]
fn a_resume_of_the_sidebars_turn_is_the_sidebars_whoever_decided() {
    // Kai's card (Kai's own POST …/approvals): the withdrawal comes first.
    let turns = SidebarTurns::default();
    a_sidebar_turn_waits(&turns);
    decided(&turns, "mcpr_1");
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    turns.fact(done(12));
    // The phone's verdicts: the resumed turn's start before the withdrawal.
    let turns = SidebarTurns::default();
    a_sidebar_turn_waits(&turns);
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    decided(&turns, "mcpr_1");
    turns.fact(done(12));
    // The resume ended: an own turn after it that is no resume is Kai's.
    assert_eq!(by(&turns, started(12, own(), false)), own());
}

/// A resume of Kai's own voice turn stays Kai's, also when the sidebar sent the verdicts; a new
/// message of the sidebar's while calls wait is the sidebar's (lmgw declines the calls).
#[test]
fn a_resume_of_kais_own_turn_stays_kais_and_a_new_message_is_the_senders() {
    let turns = SidebarTurns::default();
    assert_eq!(by(&turns, started(12, own(), true)), own());
    waits(&turns, 12, "mcpr_1");
    turns.fact(done(12));
    let verdicts = turns.mark(12, Start::Resume);
    decided(&turns, "mcpr_1");
    assert_eq!(by(&turns, started(12, own(), false)), own());
    drop(verdicts);
    waits(&turns, 12, "mcpr_2");
    turns.fact(done(12));
    let send = turns.mark(12, Start::New);
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    decided(&turns, "mcpr_2");
    drop(send);
}

/// A resume of a turn Kai never saw start (Kai started after it): the sidebar's when the sidebar
/// sent the verdicts, else Kai's.
#[test]
fn a_resume_of_a_turn_kai_never_saw_is_read_by_who_sent_the_verdicts() {
    let turns = SidebarTurns::default();
    waits(&turns, 12, "mcpr_1");
    waits(&turns, 14, "mcpr_2");
    let verdicts = turns.mark(12, Start::Resume);
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    drop(verdicts);
    assert_eq!(by(&turns, started(14, own(), false)), own());
}

/// After the sidebar used a thread: Kai's own turn there that is neither marked nor a resume (a
/// typed one), and Kai's own voice turn, are Kai's.
#[test]
fn kais_own_turns_on_a_thread_the_sidebar_used_are_kais() {
    let turns = SidebarTurns::default();
    a_sidebar_turn_waits(&turns);
    decided(&turns, "mcpr_1");
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    turns.fact(done(12));
    assert_eq!(by(&turns, started(12, own(), false)), own(), "typed");
    turns.fact(done(12));
    assert_eq!(by(&turns, started(12, own(), true)), own(), "spoken");
    // The voice turn's calls decided elsewhere: its resume is Kai's.
    waits(&turns, 12, "mcpr_3");
    turns.fact(done(12));
    decided(&turns, "mcpr_3");
    assert_eq!(by(&turns, started(12, own(), false)), own());
}

/// L2: a turn that starts cancels the thread's running one, whose turn.done can come after the
/// new turn.started. That one is not heard: the thread's running turn stays as it was read.
#[test]
fn a_cancelled_turns_done_after_its_successors_start_is_not_heard() {
    let turns = SidebarTurns::default();
    assert_eq!(by(&turns, started(12, own(), true)), own());
    let send = turns.mark(12, Start::New);
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    assert_eq!(turns.fact(done(12)), None, "the voice turn's, cancelled");
    assert_eq!(turns.fact(done(12)), Some(done(12)), "the sidebar's");
    drop(send);
    // The other order: the cancelled turn's done first.
    assert_eq!(by(&turns, started(12, phone(), false)), phone());
    assert_eq!(turns.fact(done(12)), Some(done(12)));
    assert_eq!(by(&turns, started(12, phone(), false)), phone());
    assert_eq!(turns.fact(done(12)), Some(done(12)));
}

#[test]
fn the_running_list_says_which_turns_are_the_sidebars() {
    let turns = SidebarTurns::default();
    let _m = turns.mark(12, Start::New);
    assert_eq!(by(&turns, started(12, own(), false)), By::sidebar());
    // The feed came back while the turn ran: its list names it as Kai's own.
    let _n = turns.mark(20, Start::New);
    waits(&turns, 22, "mcpr_1");
    let _v = turns.mark(22, Start::Resume);
    assert_eq!(
        live(
            &turns,
            vec![
                (12, own(), false),
                (20, own(), false),
                (22, own(), false),
                (30, own(), false),
                (32, own(), true),
            ]
        ),
        vec![
            (12, By::sidebar(), false),
            (20, By::sidebar(), false),
            (22, By::sidebar(), false),
            (30, own(), false),
            (32, own(), true),
        ],
        "12 runs, 20 began while the feed was down, so did 22's resume, 30 and 32 are Kai's"
    );
    // A list read again keeps what it read; a done ends it.
    assert_eq!(
        live(&turns, vec![(12, own(), false)]),
        vec![(12, By::sidebar(), false)]
    );
    assert_eq!(turns.fact(done(12)), Some(done(12)));
    assert_eq!(
        live(&turns, vec![(12, own(), false)]),
        vec![(12, own(), false)]
    );
}

#[test]
fn a_deleted_thread_and_a_left_gateway_take_what_was_read() {
    let turns = SidebarTurns::default();
    a_sidebar_turn_waits(&turns);
    turns.fact(FeedFact::ThreadGone { thread_id: 12 });
    assert_eq!(by(&turns, started(12, own(), false)), own());
    let turns = SidebarTurns::default();
    a_sidebar_turn_waits(&turns);
    let _open = turns.mark(14, Start::New);
    turns.forget();
    assert_eq!(by(&turns, started(12, own(), false)), own());
    assert_eq!(by(&turns, started(14, own(), false)), own());
}
