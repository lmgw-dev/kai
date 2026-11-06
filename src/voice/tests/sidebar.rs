//! Firefox's sidebar's turns through the core (kai-firefox §8.3): lmgw's feed names them as Kai's
//! own, and the core reads them as the sidebar's before the voice does, so automatic listening
//! holds back while one runs (K16) and its calls are the card's, which names the sidebar, not the
//! voice's (K17). Whose a turn is follows the turn: a resume of the sidebar's turn is the sidebar's
//! whoever decided its calls, and Kai's own turns on a thread the sidebar used are Kai's. A list of
//! running turns after the feed came back says the same to the card. Leaving the gateway ends the
//! sidebar's requests with an error it shows.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;

use super::approvals::{Card, request};
use super::{By, Command, FeedFact, LinkEvent, Rig, rig};
use crate::approvals::Requested;
use crate::approvals::overlay::test_cards;
use crate::browser::sidebar::turns::Start;
use crate::browser::sidebar::{Relay, Reply, Request};
use crate::lmgw_link::KeyLoss;
use crate::lmgw_link::proxy::Method;
use crate::lmgw_link::proxy::relay::Outbound;

const SIDEBAR_RUNS: &str = "a turn of Kai's sidebar in Firefox runs";

fn own() -> By {
    By {
        name: "device 'desktop'".into(),
        own: true,
        sidebar: false,
    }
}

/// The sidebar's side of one request: the answer's frames, and its acks (none sent: the answers
/// here stay within the window).
struct Frames {
    rx: mpsc::UnboundedReceiver<crate::browser::links::Outgoing>,
    _acks: watch::Sender<u64>,
}

/// The sidebar's `path` on `thread` through the core, as `browser.sock` hands it on; the mock
/// answers 200 and holds the stream open until the test drops it. Returns once the head came.
async fn sidebar(r: &mut Rig, thread: i64, path: &str, start: Start) -> Frames {
    let (out, rx) = mpsc::unbounded_channel();
    let (acks, acked) = watch::channel(0);
    let mut frames = Frames { rx, _acks: acks };
    let reply = Reply::detached(
        json!(1),
        out,
        acked,
        tokio_util::sync::CancellationToken::new(),
    );
    let request = Request {
        out: Outbound {
            method: Method::Post,
            path: format!("/chat/api/threads/{thread}/{path}"),
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
            stream: true,
        },
        turn: Some((thread, start)),
    };
    let before = r.mock.with(|s| s.sidebar.len());
    r.send(Command::Sidebar(Box::new(Relay { request, reply })));
    r.until_mock("the sidebar's request", |m| {
        m.with(|s| s.sidebar.len() > before)
    })
    .await;
    let head = frame(&mut frames).await;
    assert_eq!(
        (head["t"].as_str(), head["status"].as_u64()),
        (Some("res"), Some(200))
    );
    frames
}

async fn frame(frames: &mut Frames) -> Value {
    serde_json::from_str(&frames.rx.recv().await.unwrap().text).unwrap()
}

/// lmgw's streams of the sidebar's requests end; their `end`s come.
async fn streams_end(r: &Rig, frames: &mut Frames) {
    r.mock.with(|s| s.sidebar_streams.clear());
    while frame(frames).await["t"] != "end" {}
}

fn started(r: &Rig, thread_id: i64, voice: bool) {
    r.feed.fact(FeedFact::TurnStarted {
        thread_id,
        by: own(),
        voice,
    });
}

fn done(r: &Rig, thread_id: i64) {
    r.feed.fact(FeedFact::TurnDone { thread_id });
}

fn waits(r: &Rig, thread_id: i64, id: &str) {
    r.feed.fact(FeedFact::ApprovalRequested(Requested {
        thread_id,
        request: request(id),
    }));
}

fn decided(r: &Rig, id: &str) {
    r.feed.fact(FeedFact::ApprovalsWithdrawn(vec![id.into()]));
}

async fn sidebar_runs(r: &mut Rig) {
    r.until(|m| m.voice.notes.iter().any(|n| n.contains(SIDEBAR_RUNS)))
        .await;
}

async fn nothing_runs(r: &mut Rig) {
    r.until(|m| !m.voice.notes.iter().any(|n| n.contains("a turn of")))
        .await;
}

/// The card shows `call` as the sidebar's: asked by the sidebar, answered on the card (K17).
async fn the_sidebars_call(card: &mut Card, call: &str) {
    let m = card
        .until(call, |m| m.is_some_and(|m| m.call == call))
        .await;
    assert!(
        m.asked.starts_with("asked by Kai's sidebar in Firefox"),
        "{}",
        m.asked
    );
    assert!(!m.typed.contains("voice"), "{}", m.typed);
}

/// The sidebar's send in Kai's own thread: the voice holds automatic listening back as for another
/// client's turn (K16). Once the send ended, Kai's own turn there is Kai's again.
#[tokio::test(start_paused = true)]
async fn the_sidebars_turn_silences_automatic_listening_like_another_clients() {
    let mut r = rig("").await;
    r.listening_on().await;
    sleep(Duration::from_millis(300)).await;
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    started(&r, 812, false);
    sidebar_runs(&mut r).await;
    sleep(Duration::from_millis(40)).await;
    let (a0, s0) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(500)).await;
    let (a1, s1) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a1 - a0 >= 400 * 24, "the stream goes on");
    assert_eq!(
        s1 - s0,
        a1 - a0,
        "K16: only silence while the sidebar's turn runs"
    );
    done(&r, 812);
    nothing_runs(&mut r).await;
    streams_end(&r, &mut frames).await;
    // Kai's own turn there, neither marked nor a resume (a typed one), and its voice turn.
    started(&r, 812, false);
    sleep(Duration::from_millis(300)).await;
    let m = r.model.borrow().clone();
    assert!(
        !m.voice.notes.iter().any(|n| n.contains("a turn of")),
        "Kai's own turn holds nothing back: {:?}",
        m.voice.notes
    );
    done(&r, 812);
    started(&r, 812, true);
    sleep(Duration::from_millis(300)).await;
    let (a2, s2) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(300)).await;
    let (a3, s3) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a3 > a2);
    assert_eq!(s3, s2, "the microphone's audio during Kai's own turns");
    r.quit().await;
}

/// H1: the sidebar's turn waits on a call; the owner approves it on Kai's card, which posts the
/// verdict through the route (lmgw resumes the turn as its starter, Kai). The resumed turn is the
/// sidebar's: automatic listening holds back again, and its next call is the card's, named as the
/// sidebar's.
#[tokio::test(start_paused = true)]
async fn a_resume_approved_on_kais_card_stays_the_sidebars() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    started(&r, 812, false);
    sidebar_runs(&mut r).await;
    waits(&r, 812, "mcpr_1");
    done(&r, 812);
    streams_end(&r, &mut frames).await;
    nothing_runs(&mut r).await;
    the_sidebars_call(&mut card, "mcpr_1").await;
    card.arm();
    card.click(true);
    r.until_mock("the route", |m| m.with(|s| !s.approvals_sent.is_empty()))
        .await;
    assert_eq!(r.mock.with(|s| s.approvals_sent[0].0), 812);
    // lmgw's feed: the call decided, and the resumed turn, named as Kai's own.
    decided(&r, "mcpr_1");
    started(&r, 812, false);
    sidebar_runs(&mut r).await;
    waits(&r, 812, "mcpr_2");
    the_sidebars_call(&mut card, "mcpr_2").await;
    done(&r, 812);
    nothing_runs(&mut r).await;
    r.quit().await;
}

/// H1: the sidebar's turn's call approved elsewhere (the phone, the dashboard): the resumed turn,
/// which lmgw names as Kai's own, is the sidebar's, also when its start comes before the call's
/// withdrawal. Kai's own voice turn on the thread afterwards is Kai's.
#[tokio::test(start_paused = true)]
async fn a_resume_approved_elsewhere_stays_the_sidebars() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    started(&r, 812, false);
    waits(&r, 812, "mcpr_1");
    done(&r, 812);
    streams_end(&r, &mut frames).await;
    the_sidebars_call(&mut card, "mcpr_1").await;
    started(&r, 812, false);
    decided(&r, "mcpr_1");
    sidebar_runs(&mut r).await;
    card.until("the card goes", |m| m.is_none()).await;
    waits(&r, 812, "mcpr_2");
    the_sidebars_call(&mut card, "mcpr_2").await;
    done(&r, 812);
    nothing_runs(&mut r).await;
    decided(&r, "mcpr_2");
    started(&r, 812, false);
    sidebar_runs(&mut r).await;
    done(&r, 812);
    nothing_runs(&mut r).await;
    // Kai's own voice turn: Kai's, and so is its call.
    started(&r, 812, true);
    waits(&r, 812, "mcpr_3");
    let m = card
        .until("Kai's call", |m| m.is_some_and(|m| m.call == "mcpr_3"))
        .await;
    assert!(
        m.asked.starts_with("asked by Kai's voice session"),
        "{}",
        m.asked
    );
    r.quit().await;
}

/// L2: the sidebar's send cancels Kai's running voice turn, whose `turn.done` comes after the
/// sidebar's `turn.started`. It does not end the sidebar's turn for the voice.
#[tokio::test(start_paused = true)]
async fn a_cancelled_turns_late_done_keeps_the_sidebars_turn_running() {
    let mut r = rig("").await;
    r.listening_on().await;
    started(&r, 812, true);
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    started(&r, 812, false);
    sidebar_runs(&mut r).await;
    done(&r, 812);
    sleep(Duration::from_millis(300)).await;
    assert!(
        r.model
            .borrow()
            .voice
            .notes
            .iter()
            .any(|n| n.contains(SIDEBAR_RUNS)),
        "the voice turn's done ends nothing"
    );
    done(&r, 812);
    nothing_runs(&mut r).await;
    streams_end(&r, &mut frames).await;
    r.quit().await;
}

/// M1: the feed came back while the sidebar's turn ran; `hello`'s list of running turns names it
/// as Kai's own. The card reads it as the sidebar's, as at a `turn.started`.
#[tokio::test(start_paused = true)]
async fn the_running_list_after_the_feed_came_back_reaches_the_card() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    r.feed.fact(FeedFact::Live {
        voice: vec![],
        turns: vec![(812, own(), false)],
    });
    sidebar_runs(&mut r).await;
    waits(&r, 812, "mcpr_1");
    the_sidebars_call(&mut card, "mcpr_1").await;
    streams_end(&r, &mut frames).await;
    r.quit().await;
}

/// L7: Kai leaves the gateway (here its key was revoked): the sidebar's request on its way there
/// ends with an `end` that says so, and the request's mark goes with what Kai read.
#[tokio::test(start_paused = true)]
async fn leaving_the_gateway_ends_the_sidebars_requests_with_an_error() {
    let mut r = rig("").await;
    let mut frames = sidebar(&mut r, 812, "send", Start::New).await;
    r.feed.raise(LinkEvent::Revoked {
        reason: "device 'kai' expired".into(),
        loss: Some(KeyLoss::Expired),
    });
    let end = frame(&mut frames).await;
    assert_eq!(
        (end["t"].as_str(), end["seq"].as_u64()),
        (Some("end"), Some(0))
    );
    assert_eq!(end["error"]["code"], "gateway_left", "{end}");
    r.quit().await;
}
