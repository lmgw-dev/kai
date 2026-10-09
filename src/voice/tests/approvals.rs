//! WP19 through the core (§11.1, K17): while a call of Kai's own waits the session gets no audio,
//! no append and no commit; once the card armed for it the talk key records the spoken answer,
//! which goes through `…/transcribe` and, matched, back as `mcp_approval_response` then
//! `response.create`; a press before the card armed is the talk key's own; an answer in neither
//! list keeps the card; a decision made elsewhere closes it; a call of another thread holds back
//! no audio and is answered on the card through the approvals route; a resync reads the waiting
//! calls again.

use std::time::Duration;

use lmgw_client::realtime::{ApprovalDecidedEvent, ApprovalRequest};
use tokio::sync::mpsc;
use tokio::time::sleep;

use super::*;
use crate::approvals::card::{CardAnswer, CardCmd, CardEvent, CardModel};
use crate::approvals::overlay::test_cards;
use crate::voice::link::ClientEvent;

pub(super) fn request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        approval_request_id: id.into(),
        server_label: "kai".into(),
        name: "clipboard_set".into(),
        arguments: r#"{"text":"hello"}"#.into(),
        call_id: None,
    }
}

/// The card's commands this test's core sent so far, kept across calls.
#[derive(Default)]
pub(super) struct Card {
    seen: Vec<CardCmd>,
}

impl Card {
    fn take(&mut self) {
        self.seen.extend(test_cards());
    }

    /// The last thing the card was told: `Some` shown, `None` hidden.
    pub(super) fn now(&mut self) -> Option<Option<(CardModel, mpsc::UnboundedSender<CardEvent>)>> {
        self.take();
        self.seen.last().map(|c| match c {
            CardCmd::Show { model, events, .. } => Some(((**model).clone(), events.clone())),
            CardCmd::Hide => None,
        })
    }

    /// What the GTK side says once the card armed for the call it shows.
    pub(super) fn arm(&mut self) -> CardModel {
        let Some(Some((m, events))) = self.now() else {
            panic!("no card shown")
        };
        events
            .send(CardEvent::Armed {
                call: m.call.clone(),
            })
            .unwrap();
        m
    }

    /// A click on the card (as the GTK side sends it once armed).
    fn click(&mut self, approve: bool) {
        let Some(Some((m, events))) = self.now() else {
            panic!("no card shown")
        };
        events
            .send(CardEvent::Answer(CardAnswer {
                call: m.call,
                approve,
            }))
            .unwrap();
    }

    pub(super) async fn until(
        &mut self,
        what: &str,
        f: impl Fn(Option<&CardModel>) -> bool,
    ) -> CardModel {
        for _ in 0..6000 {
            match self.now() {
                Some(Some((m, _))) if f(Some(&m)) => return m,
                Some(None) if f(None) => return CardModel::default_for_tests(),
                _ => {}
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("the card never got to: {what}");
    }
}

impl CardModel {
    fn default_for_tests() -> Self {
        CardModel {
            call: String::new(),
            heading: String::new(),
            asked: String::new(),
            args: Vec::new(),
            count: None,
            note: None,
            busy: None,
            hint: String::new(),
            typed: String::new(),
        }
    }
}

fn kinds_since(r: &Rig, from: usize) -> Vec<&'static str> {
    r.mock.socket(0, |s| s.kinds[from..].to_vec())
}

#[tokio::test(start_paused = true)]
async fn no_audio_while_a_call_waits_and_the_spoken_yes_goes_back_through_the_session() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    sleep(Duration::from_millis(300)).await;
    let before = r.mock.socket(0, |s| s.appended);
    assert!(before > 0, "automatic listening streams");
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    let m = card.until("the card", |m| m.is_some()).await;
    assert_eq!(m.heading, "Approve kai__clipboard_set?");
    assert_eq!(m.args, vec!["text: hello".to_owned()]);
    let m = r
        .until(|m| {
            m.voice
                .notes
                .iter()
                .any(|n| n.contains("an approval waits"))
        })
        .await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("kai__clipboard_set"))
    );
    let from = r.mock.socket(0, |s| s.kinds.len());
    let appended = r.mock.socket(0, |s| s.appended);
    // K17: nothing goes to the session while the call waits.
    sleep(Duration::from_millis(800)).await;
    // The card armed (its GTK side says so): the talk key records the answer now.
    card.arm();
    sleep(Duration::from_millis(20)).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    let busy = card
        .until("listening", |m| m.is_some_and(|m| m.busy.is_some()))
        .await;
    assert!(busy.busy.unwrap().starts_with("Listening"));
    r.send(Command::Talk(false));
    r.until_mock("the approval's answer and response.create", |m| {
        m.socket(0, |s| s.count("response.create") > 0)
    })
    .await;
    let kinds = kinds_since(&r, from);
    let answer = kinds
        .iter()
        .position(|k| *k == "conversation.item.create")
        .expect("an mcp_approval_response");
    let create = kinds.iter().position(|k| *k == "response.create").unwrap();
    assert!(answer < create, "{kinds:?}");
    assert!(
        !kinds[..create].contains(&"input_audio_buffer.append")
            && !kinds[..create].contains(&"input_audio_buffer.commit"),
        "no append and no commit while the call waited: {kinds:?}"
    );
    r.mock.socket(0, |s| {
        let sent = s
            .sent
            .iter()
            .find(|e| matches!(e, ClientEvent::ApprovalResponse { .. }))
            .unwrap();
        assert_eq!(
            sent,
            &ClientEvent::ApprovalResponse {
                approval_request_id: "mcpr_1".into(),
                approve: true,
                reason: None,
            }
        );
    });
    // The hold went to …/transcribe of the call's thread: 600 ms and the pre-roll, as WAV.
    let (thread, bytes) = r.mock.with(|s| s.transcribed[0]);
    assert_eq!(thread, 812);
    let ms = (bytes - 44) as u64 / 48;
    assert!(
        (600..=950).contains(&ms),
        "{ms} ms recorded (the hold, 300 ms of pre-roll, a tick)"
    );
    assert!(
        r.mock.with(|s| s.approvals_sent.is_empty()),
        "not the route"
    );
    card.until("the card goes", |m| m.is_none()).await;
    // The audio flows again.
    sleep(Duration::from_millis(300)).await;
    assert!(r.mock.socket(0, |s| s.appended) > appended);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn an_answer_in_neither_list_keeps_the_card_and_a_decision_elsewhere_closes_it() {
    let _ = test_cards();
    let mut card = Card::default();
    let mock = MockLink::default();
    mock.with(|s| s.transcripts = vec![Ok("vielleicht".into())]);
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    card.arm();
    sleep(Duration::from_millis(20)).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    let m = card
        .until("the note", |m| m.is_some_and(|m| m.note.is_some()))
        .await;
    assert_eq!(
        m.note.as_deref(),
        Some("Heard “vielleicht”: say yes or no, or click.")
    );
    assert_eq!(r.mock.socket(0, |s| s.count("conversation.item.create")), 0);
    assert_eq!(
        r.mock.socket(0, |s| s.count("input_audio_buffer.commit")),
        0
    );
    let appended = r.mock.socket(0, |s| s.appended);
    r.mock
        .emit(ServerEvent::ApprovalDecided(ApprovalDecidedEvent {
            approval_request_id: "mcpr_1".into(),
            approve: false,
            by: Some("the dashboard".into()),
        }));
    card.until("the card goes", |m| m.is_none()).await;
    sleep(Duration::from_millis(300)).await;
    assert!(r.mock.socket(0, |s| s.appended) > appended, "audio again");
    assert_eq!(r.mock.socket(0, |s| s.count("response.create")), 0);
    r.quit().await;
}

/// A call of another thread from the feed, no session: the card's click goes through the route,
/// and the talk key keeps its own meaning (it binds Kai's session; nothing is transcribed).
#[tokio::test(start_paused = true)]
async fn a_feed_call_is_answered_on_the_card_through_the_route() {
    let _ = test_cards();
    let mut card = Card::default();
    let r = rig("").await;
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 5,
        by: By {
            name: "device 'phone'".into(),
            own: false,
        },
    });
    r.feed
        .fact(FeedFact::ApprovalRequested(crate::approvals::Requested {
            thread_id: 5,
            request: request("mcpr_7"),
        }));
    let m = card.until("the card", |m| m.is_some()).await;
    assert_eq!(m.asked, "asked by device 'phone' in thread 5");
    assert_eq!(m.hint, "Enter approves, Esc declines.", "no voice for it");
    card.arm();
    // A press is the talk key's own: it binds Kai's session, and records no answer.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    r.until_mock("Kai's session", |m| m.opens() == 1).await;
    assert!(
        r.mock.with(|s| s.transcribed.is_empty()),
        "no spoken answer"
    );
    // The click.
    card.click(true);
    r.until_mock("the route", |m| m.with(|s| !s.approvals_sent.is_empty()))
        .await;
    let (thread, body) = r.mock.with(|s| s.approvals_sent[0].clone());
    assert_eq!(thread, 5);
    assert_eq!(body.decisions.len(), 1);
    assert!(body.decisions[0].approve && !body.speak);
    card.until("the card goes", |m| m.is_none()).await;
    r.quit().await;
}

/// A call of another thread (a phone's turn) holds back none of Kai's audio: automatic listening
/// streams on while its card shows, and a press is a press, not an answer.
#[tokio::test(start_paused = true)]
async fn a_call_of_another_thread_holds_back_no_audio() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    r.feed
        .fact(FeedFact::ApprovalRequested(crate::approvals::Requested {
            thread_id: 5,
            request: request("mcpr_7"),
        }));
    card.until("the card", |m| m.is_some()).await;
    card.arm();
    let before = r.mock.socket(0, |s| s.appended);
    sleep(Duration::from_millis(500)).await;
    let after = r.mock.socket(0, |s| s.appended);
    assert!(
        after - before >= 400 * 24,
        "the microphone streams on: {} samples",
        after - before
    );
    let m = r.until(|m| !m.voice.notes.is_empty()).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("answer on the card") && !n.contains("talk key")),
        "{:?}",
        m.voice.notes
    );
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(300)).await;
    assert!(
        r.mock.with(|s| s.transcribed.is_empty()),
        "no spoken answer"
    );
    assert!(card.now().unwrap().is_some(), "the card stays");
    r.quit().await;
}

/// In Kai's own thread a press before the card armed is the talk key's own: no answer is recorded
/// or transcribed, and no audio reaches the session (the call still waits).
#[tokio::test(start_paused = true)]
async fn a_press_before_the_card_armed_is_talk_not_an_answer() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    sleep(Duration::from_millis(100)).await;
    let appended = r.mock.socket(0, |s| s.appended);
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    let m = card.now().unwrap().unwrap().0;
    assert_eq!(m.busy, None, "not listening for an answer");
    // The card arms while the key is held: this press stays talk.
    card.arm();
    sleep(Duration::from_millis(100)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(300)).await;
    assert!(
        r.mock.with(|s| s.transcribed.is_empty()),
        "nothing transcribed"
    );
    assert_eq!(
        r.mock.socket(0, |s| s.appended),
        appended,
        "no audio while Kai's call waits"
    );
    assert_eq!(r.mock.socket(0, |s| s.count("conversation.item.create")), 0);
    // Pressed again, now that it is armed: the answer.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    r.until_mock("the approval's answer", |m| {
        m.socket(0, |s| s.count("conversation.item.create") > 0)
    })
    .await;
    assert_eq!(r.mock.with(|s| s.transcribed.len()), 1);
    r.quit().await;

    // Push-to-talk: the press before the card armed sends no pre-roll, no clear and no commit.
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("[voice]
mode = \"push_to_talk\"\n")
    .await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    sleep(Duration::from_millis(100)).await;
    let (from, appended) = r.mock.socket(0, |s| (s.kinds.len(), s.appended));
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(300)).await;
    let kinds = kinds_since(&r, from);
    assert_eq!(r.mock.socket(0, |s| s.appended), appended, "{kinds:?}");
    assert!(
        !kinds.contains(&"input_audio_buffer.commit") && !kinds.contains(&"response.create"),
        "{kinds:?}"
    );
    assert!(r.mock.with(|s| s.transcribed.is_empty()));
    r.quit().await;
}

/// The feed's resync reads the waiting calls of the threads Kai knows calls of again: one decided
/// meanwhile goes, one Kai missed comes.
#[tokio::test(start_paused = true)]
async fn a_resync_reads_the_waiting_calls_again() {
    let _ = test_cards();
    let mut card = Card::default();
    let r = rig("").await;
    r.feed
        .fact(FeedFact::ApprovalRequested(crate::approvals::Requested {
            thread_id: 5,
            request: request("mcpr_7"),
        }));
    card.until("the card", |m| m.is_some_and(|m| m.call == "mcpr_7"))
        .await;
    r.mock
        .with(|s| s.pending.insert(5, vec![request("mcpr_8")]));
    r.feed.fact(FeedFact::Resync);
    card.until("the call lmgw lists now", |m| {
        m.is_some_and(|m| m.call == "mcpr_8")
    })
    .await;
    assert!(r.mock.with(|s| s.pending_read.contains(&5)));
    r.quit().await;
}

/// A press session waits for its call's answer instead of closing after its grace (K5).
#[tokio::test(start_paused = true)]
async fn a_press_session_stays_while_its_call_waits() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("[voice]\ngrace_s = 1\n").await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    sleep(Duration::from_secs(5)).await;
    assert!(!r.mock.socket(0, |s| s.closed_by_client), "the grace waits");
    card.click(false);
    r.until_mock("the answer", |m| {
        m.socket(0, |s| {
            s.sent.iter().any(|e| {
                matches!(e, ClientEvent::ApprovalResponse { approve: false, reason: Some(r), .. } if r == "declined on Kai")
            })
        })
    })
    .await;
    r.until_mock("the grace close after the resumed reply", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    r.quit().await;
}

/// M1: lmgw refuses the `response.create` that carries the verdicts (another response is active):
/// its `error` echoes the create's `event_id`, so the verdicts are not left waiting for a response
/// that never comes. The thread is read again, the card asks again with lmgw's words, no audio goes
/// meanwhile, and the next answer goes with a create of its own, which lmgw takes.
#[tokio::test(start_paused = true)]
async fn a_refused_create_asks_again_instead_of_waiting_forever() {
    let _ = test_cards();
    let mut card = Card::default();
    let mock = MockLink::default();
    mock.with(|s| {
        s.refuse_creates = vec!["conversation_already_has_active_response".into()];
        s.pending.insert(812, vec![request("mcpr_1")]);
    });
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    card.arm();
    card.click(true);
    let m = card
        .until("asked again", |m| m.is_some_and(|m| m.note.is_some()))
        .await;
    assert!(
        m.note
            .as_deref()
            .unwrap()
            .contains("conversation_already_has_active_response"),
        "{m:?}"
    );
    assert!(r.mock.with(|s| s.pending_read.contains(&812)));
    r.no_notice("Kai: voice error");
    let appended = r.mock.socket(0, |s| s.appended);
    sleep(Duration::from_millis(300)).await;
    assert_eq!(
        r.mock.socket(0, |s| s.appended),
        appended,
        "Kai's call waits again: no audio"
    );
    // Answered again: a create with its own id, which lmgw takes this time.
    card.arm();
    card.click(true);
    r.until_mock("the second create", |m| {
        m.socket(0, |s| s.tagged.len() == 2)
    })
    .await;
    r.mock.socket(0, |s| assert_ne!(s.tagged[0], s.tagged[1]));
    card.until("the card goes", |m| m.is_none()).await;
    r.until_mock("the audio again", |m| {
        m.socket(0, |s| s.appended > appended)
    })
    .await;
    assert!(
        r.mock.with(|s| s.approvals_sent.is_empty()),
        "not the route"
    );
    r.quit().await;
}

/// M2: a push-to-talk hold that began while Kai's call waited stays silent to its release, also
/// when the call is decided while it is held: nothing of it reaches the session and it commits
/// nothing (its first part was never sent, so a commit would be a clipped turn).
#[tokio::test(start_paused = true)]
async fn a_hold_that_began_gated_stays_silent_when_the_gate_lifts() {
    let _ = test_cards();
    let mut card = Card::default();
    let mut r = rig("[voice]
mode = \"push_to_talk\"\n")
    .await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    let (from, appended) = r.mock.socket(0, |s| (s.kinds.len(), s.appended));
    // Pressed before the card armed: the talk key's own press, gated.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(300)).await;
    r.mock
        .emit(ServerEvent::ApprovalDecided(ApprovalDecidedEvent {
            approval_request_id: "mcpr_1".into(),
            approve: true,
            by: Some("the dashboard".into()),
        }));
    card.until("the card goes", |m| m.is_none()).await;
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(300)).await;
    let kinds = kinds_since(&r, from);
    assert_eq!(r.mock.socket(0, |s| s.appended), appended, "{kinds:?}");
    assert!(
        !kinds.contains(&"input_audio_buffer.commit") && !kinds.contains(&"response.create"),
        "{kinds:?}"
    );
    // The next hold talks as ever.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    r.until_mock("the next turn's commit", |m| {
        m.socket(0, |s| s.count("input_audio_buffer.commit") == 1)
    })
    .await;
    assert!(r.mock.socket(0, |s| s.appended) > appended);
    r.quit().await;
}

/// M2: verdicts sent through the session hold the gate until lmgw created their response, so a
/// turn the owner commits in between is not taken for it: no audio goes before `response.created`.
#[tokio::test(start_paused = true)]
async fn the_gate_holds_until_the_verdicts_response_was_created() {
    let _ = test_cards();
    let mut card = Card::default();
    let mock = MockLink::default();
    // No reply to a create: lmgw has not answered it yet.
    mock.with(|s| s.reply = None);
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    card.until("the card", |m| m.is_some()).await;
    card.arm();
    card.click(true);
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.count("response.create") == 1)
    })
    .await;
    card.until("the card goes", |m| m.is_none()).await;
    let appended = r.mock.socket(0, |s| s.appended);
    sleep(Duration::from_millis(500)).await;
    assert_eq!(r.mock.socket(0, |s| s.appended), appended, "gated still");
    r.mock.emit(ServerEvent::ResponseCreated {
        response_id: "resp_9".into(),
    });
    r.until_mock("the audio again", |m| {
        m.socket(0, |s| s.appended > appended)
    })
    .await;
    r.mock.emit(ServerEvent::ResponseDone {
        response_id: "resp_9".into(),
        status: "completed".into(),
        reason: None,
    });
    r.quit().await;
}
