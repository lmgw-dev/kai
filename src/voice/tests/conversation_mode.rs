//! WP9: conversation mode (ruling 23, K27) through the core, against the scripted lmgw, in paused
//! time: the tools come in as `Command::Tool` as the host sends them, the tray model, the ring's
//! input and the turn detections of the session's `session.update`s are read as lmgw and the
//! overlay get them.

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::super::link::TurnDetection;
use super::closes::next_thread;
use super::{By, Command, FeedFact, Rig, VoiceShown, rig};
use crate::core::calls::CoreCall;
use crate::host::policy::{Caller, Decision, Policy};
use crate::host::registry::{Args, Block, Call, Reply};
use crate::host::settings::{LevelSetting, Named, PolicyConfig};

/// Calls the tool the way the host does and returns its answer.
async fn tool(r: &Rig, name: &str) -> Reply {
    let (area, spec) = kai_desktop::contract::find(name).expect("a contract tool");
    let (reply, answer) = oneshot::channel();
    r.send(Command::Tool(Box::new(CoreCall {
        area: area.name,
        call: Call::new(spec, Args::default(), Caller::Own, CancellationToken::new()),
        reply,
    })));
    answer.await.expect("the core answers")
}

fn text(r: &Reply) -> String {
    match &r.blocks[..] {
        [Block::Text(t)] => t.clone(),
        other => panic!("not one text block: {other:?}"),
    }
}

fn last_detection(r: &Rig) -> Option<TurnDetection> {
    r.mock
        .socket(0, |s| s.detections().last().copied())
        .expect("a session.update")
}

const PTT: &str = "[voice]\nmode = \"push_to_talk\"\nconversation_timeout_s = 20\n";

/// Push-to-talk with listening on and a long reply playing, the tool called mid-reply: nothing
/// changes until the reply has played, then the session listens continuously, the ring stays up,
/// and the result names the timeout setting.
#[tokio::test(start_paused = true)]
async fn it_takes_effect_after_the_current_reply_and_the_ring_stays_up() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    assert_eq!(last_detection(&r), None, "push-to-talk sends no detection");
    r.rings();
    r.mock.respond("Gern, das mache ich.", 2000);
    sleep(Duration::from_millis(500)).await;
    let answer = tool(&r, "conversation_start").await;
    assert!(!answer.error, "{answer:?}");
    let said = text(&answer);
    assert!(
        said.contains("voice.conversation_timeout_s") && said.contains("20 s"),
        "{said}"
    );
    sleep(Duration::from_millis(500)).await;
    assert_eq!(last_detection(&r), None, "the reply is still playing");
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.active && i.turn),
        "{rings:?}"
    );
    sleep(Duration::from_secs(4)).await;
    assert!(
        matches!(
            last_detection(&r),
            Some(TurnDetection::ServerVad | TurnDetection::SemanticVad)
        ),
        "{:?}",
        r.mock.socket(0, |s| s.detections())
    );
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.active && i.bound && !i.turn),
        "the ring stays up between turns: {rings:?}"
    );
    let m = r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.starts_with("conversation mode:")),
        "{:?}",
        m.voice.notes
    );
    r.quit().await;
}

/// Continuous listening reaches the server: a turn spoken in the mode is answered with no key.
#[tokio::test(start_paused = true)]
async fn it_listens_continuously_until_the_silence_timeout_and_says_so() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    assert!(last_detection(&r).is_some());
    // A turn restarts the silence.
    sleep(Duration::from_secs(15)).await;
    r.mock.speak();
    sleep(Duration::from_secs(4)).await;
    sleep(Duration::from_secs(15)).await;
    assert!(
        last_detection(&r).is_some(),
        "19 s after the turn: still on"
    );
    sleep(Duration::from_secs(7)).await;
    assert_eq!(last_detection(&r), None, "the configured mode is back");
    let n = r.notice("Kai: conversation mode ended").await;
    assert!(
        n.body.contains("voice.conversation_timeout_s"),
        "{}",
        n.body
    );
    let rings = r.rings();
    assert!(rings.last().is_some_and(|i| !i.active), "{rings:?}");
    // Listening itself was on before and stays on.
    assert!(r.until(|m| m.voice.on).await.voice.on);
    r.quit().await;
}

/// 0 is no silence timeout: visible in the result, and the mode stays.
#[tokio::test(start_paused = true)]
async fn a_timeout_of_zero_never_ends_it_by_silence() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\nconversation_timeout_s = 0\n").await;
    r.listening_on().await;
    let said = text(&tool(&r, "conversation_start").await);
    assert!(said.contains("never by silence"), "{said}");
    sleep(Duration::from_secs(1800)).await;
    assert!(last_detection(&r).is_some());
    r.quit().await;
}

/// Muted time counts toward the silence timeout (the owner's decision, 2026-10-09): muted
/// throughout, the mode ends at `voice.conversation_timeout_s` as silence would; a mute does not
/// end it by itself.
#[tokio::test(start_paused = true)]
async fn muted_time_counts_toward_the_timeout() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    r.send(Command::SetMuted(true));
    sleep(Duration::from_secs(10)).await;
    assert!(last_detection(&r).is_some(), "muted for 10 s: still on");
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.muted && !i.active),
        "muted is standby: {rings:?}"
    );
    sleep(Duration::from_secs(11)).await;
    assert_eq!(
        last_detection(&r),
        None,
        "20 s of silence, all of it muted: the configured mode is back"
    );
    let n = r.notice("Kai: conversation mode ended").await;
    assert!(
        n.body.contains("voice.conversation_timeout_s"),
        "{}",
        n.body
    );
    r.quit().await;
}

/// The talk key ends it: the press spends the gesture, so nothing is committed; listening is as
/// configured.
#[tokio::test(start_paused = true)]
async fn the_talk_key_ends_it() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    assert!(last_detection(&r).is_some());
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(1)).await;
    assert_eq!(last_detection(&r), None);
    assert_eq!(
        r.mock.socket(0, |s| s.count("input_audio_buffer.commit")),
        0
    );
    assert!(r.until(|m| m.voice.on).await.voice.on);
    r.quit().await;
}

/// The tray's toggle ends the mode and does not switch listening off.
#[tokio::test(start_paused = true)]
async fn the_tray_ends_it() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    r.send(Command::ToggleListening);
    sleep(Duration::from_secs(1)).await;
    assert_eq!(last_detection(&r), None);
    assert!(r.until(|m| m.voice.on).await.voice.on, "listening stays on");
    r.quit().await;
}

/// `conversation_end` ends it and names what returns; ending twice is harmless.
#[tokio::test(start_paused = true)]
async fn the_tool_ends_it() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    let said = text(&tool(&r, "conversation_end").await);
    assert!(said.contains("push-to-talk"), "{said}");
    sleep(Duration::from_secs(1)).await;
    assert_eq!(last_detection(&r), None);
    assert_eq!(
        text(&tool(&r, "conversation_end").await),
        "Conversation mode was not on."
    );
    r.quit().await;
}

/// K16 still holds back: another principal's turn makes the mode send silence (the ring is down)
/// until it is done.
#[tokio::test(start_paused = true)]
async fn another_principals_turn_still_holds_it_back() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    r.rings();
    let thread = r.mock.with(|s| s.thread.id);
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: thread,
        by: By {
            name: "key:script".into(),
            own: false,
        },
    });
    sleep(Duration::from_secs(1)).await;
    let m = r
        .until(|m| m.voice.notes.iter().any(|n| n.contains("sends silence")))
        .await;
    assert!(m.voice.notes.iter().any(|n| n.contains("sends silence")));
    assert!(r.rings().last().is_some_and(|i| !i.active));
    // The wait is no silence: the mode outlives the timeout while the turn runs.
    sleep(Duration::from_secs(60)).await;
    assert!(last_detection(&r).is_some());
    r.feed.fact(FeedFact::TurnDone { thread_id: thread });
    sleep(Duration::from_secs(1)).await;
    assert!(r.rings().last().is_some_and(|i| i.active));
    r.quit().await;
}

/// The session's end restores the configured mode: a press session stays open in the mode past its
/// grace, closes after the grace once the mode ends, and the next session starts as configured.
#[tokio::test(start_paused = true)]
async fn the_sessions_end_restores_the_configured_mode() {
    let mut r =
        rig("[voice]\nmode = \"push_to_talk\"\ngrace_s = 2\nconversation_timeout_s = 0\n").await;
    r.send(Command::Talk(true));
    r.created(0).await;
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(3)).await;
    // The reply played; the grace would close the session, but the mode asked for it first.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.mock.respond("Gut.", 500);
    let said = tool(&r, "conversation_start").await;
    assert!(!said.error, "{said:?}");
    sleep(Duration::from_secs(10)).await;
    assert!(
        !r.mock.socket(0, |s| s.closed_by_client),
        "the mode keeps the session open"
    );
    assert!(last_detection(&r).is_some());
    tool(&r, "conversation_end").await;
    r.until_mock("the grace close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    // A session of the same listening mode afterwards starts as configured.
    r.send(Command::Talk(true));
    r.created(1).await;
    assert_eq!(
        r.mock
            .socket(1, |s| s.detections().last().copied().flatten()),
        None
    );
    r.quit().await;
}

/// Listening off ends it with the session.
#[tokio::test(start_paused = true)]
async fn listening_off_ends_the_mode_with_the_session() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    r.send(Command::SetListening(false));
    r.until_mock("the close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    r.send(Command::ToggleListening);
    r.created(1).await;
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        r.mock
            .socket(1, |s| s.detections().last().copied().flatten()),
        None,
        "the new session is push-to-talk again"
    );
    r.quit().await;
}

/// No session, no mode.
#[tokio::test(start_paused = true)]
async fn it_needs_a_voice_session() {
    let r = rig(PTT).await;
    let answer = tool(&r, "conversation_start").await;
    assert!(answer.error, "{answer:?}");
    assert!(text(&answer).contains("voice session"), "{}", text(&answer));
    r.quit().await;
}

// ------------------------------------------------------------------------- drops end it (K27)

/// What the owner is told when the mode ended with its session or a rebind.
async fn ended_visibly(r: &mut Rig) {
    let n = r.notice("Kai: conversation mode ended").await;
    assert!(n.body.contains("push-to-talk"), "{}", n.body);
    let m = r.until(|m| m.voice.on).await;
    assert!(
        !m.voice
            .notes
            .iter()
            .any(|n| n.starts_with("conversation mode:")),
        "{:?}",
        m.voice.notes
    );
}

/// A 4004 with listening on: Kai binds the folder's current thread again, and the mode is not in
/// the new session: it ended with the old one, as a press session's drop ends it.
#[tokio::test(start_paused = true)]
async fn a_4004_ends_the_mode_when_it_is_on() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    assert!(last_detection(&r).is_some());
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    sleep(Duration::from_secs(3)).await;
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    assert_eq!(
        r.mock
            .socket(1, |s| s.detections().last().copied().flatten()),
        None,
        "the new session is as configured"
    );
    let m = r.until(|m| m.voice.on).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("conversation mode ended")),
        "{:?}",
        m.voice.notes
    );
    r.notice("Kai: conversation mode ended").await;
    // Never restarted by itself, and the silence timeout has nothing to end.
    sleep(Duration::from_secs(30)).await;
    assert_eq!(
        r.mock
            .socket(1, |s| s.detections().last().copied().flatten()),
        None
    );
    r.quit().await;
}

/// The same for a request that waits for the reply the tool answered in: the 4004 ends it, and the
/// next session does not start in the mode after all.
#[tokio::test(start_paused = true)]
async fn a_4004_ends_a_request_that_waits_for_the_reply() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    r.mock.respond("Gern, das mache ich.", 3000);
    sleep(Duration::from_millis(500)).await;
    tool(&r, "conversation_start").await;
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    sleep(Duration::from_secs(5)).await;
    assert_eq!(
        r.mock
            .socket(1, |s| s.detections().last().copied().flatten()),
        None
    );
    r.notice("Kai: conversation mode ended").await;
    r.quit().await;
}

/// A K5 rebind (a new conversation) at the quiet point ends the mode on, and a request not yet in
/// effect, visibly, through the same end: the configured mode is back on the new session.
#[tokio::test(start_paused = true)]
async fn a_rebind_ends_the_mode_and_a_pending_request_visibly() {
    for wait_for_reply in [false, true] {
        let mut r = rig(PTT).await;
        r.listening_on().await;
        if wait_for_reply {
            r.mock.respond("Gern, das mache ich.", 3000);
            sleep(Duration::from_millis(500)).await;
        }
        tool(&r, "conversation_start").await;
        sleep(Duration::from_secs(1)).await;
        r.send(Command::NewConversation);
        r.created(1).await;
        sleep(Duration::from_secs(5)).await;
        assert_eq!(
            r.mock
                .socket(1, |s| s.detections().last().copied().flatten()),
            None,
            "wait_for_reply {wait_for_reply}"
        );
        ended_visibly(&mut r).await;
        r.quit().await;
    }
}

/// A dropped session (lmgw went away): the mode goes with it, and listening is off as for any drop.
#[tokio::test(start_paused = true)]
async fn a_dead_link_ends_the_mode_with_the_session() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    r.mock.drop_socket();
    r.notice("Kai: voice stopped").await;
    let m = r.until(|m| !m.voice.on).await;
    assert!(
        !m.voice
            .notes
            .iter()
            .any(|n| n.starts_with("conversation mode:")),
        "{:?}",
        m.voice.notes
    );
    r.quit().await;
}

/// A press while the reply the request waits for plays: it stops the voice (K6) and, being the talk
/// key, ends the request; a mode that is already on goes on after the press stops a reply.
#[tokio::test(start_paused = true)]
async fn a_press_during_the_reply_ends_a_request_but_not_a_mode_that_is_on() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    r.mock.respond("Gern, das mache ich.", 4000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    tool(&r, "conversation_start").await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(300)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(5)).await;
    assert_eq!(last_detection(&r), None, "the request ended with the press");
    assert_eq!(
        r.mock.socket(0, |s| s.count("input_audio_buffer.commit")),
        0,
        "the press stopped the voice and was spent"
    );
    // Now on; a reply plays and the press stops it, the mode stays.
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    assert!(last_detection(&r).is_some());
    r.mock.respond("Noch eine Antwort.", 4000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(300)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(2)).await;
    assert!(last_detection(&r).is_some(), "a mode that is on goes on");
    r.quit().await;
}

/// PipeWire being down is silence for the timeout: nothing can be heard, so the mode does not stay
/// on for ever.
#[tokio::test(start_paused = true)]
async fn pipewire_down_counts_as_silence_for_the_timeout() {
    let mut r = rig(PTT).await;
    r.listening_on().await;
    tool(&r, "conversation_start").await;
    sleep(Duration::from_secs(1)).await;
    assert!(last_detection(&r).is_some());
    {
        let pw = r.pw.lock().unwrap();
        pw.mics[0].close();
    }
    r.audio_tx
        .send(super::AudioEvent::Link(Some(
            "PipeWire is not reachable".into(),
        )))
        .unwrap();
    sleep(Duration::from_secs(15)).await;
    assert!(
        last_detection(&r).is_some(),
        "15 s: still inside the timeout"
    );
    sleep(Duration::from_secs(10)).await;
    assert_eq!(
        last_detection(&r),
        None,
        "the timeout ran with PipeWire down"
    );
    r.notice("Kai: conversation mode ended").await;
    r.quit().await;
}

// ------------------------------------------------------------------------------------ policy

fn named(kind: &str, name: &str) -> Caller {
    Caller::Named(Named {
        kind: kind.into(),
        name: name.into(),
    })
}

fn policy(level: Option<LevelSetting>) -> Policy {
    let mut c = PolicyConfig {
        trusted_callers: vec!["key:laptop".into()],
        ..Default::default()
    };
    if let Some(l) = level {
        for t in ["conversation_start", "conversation_end"] {
            c.levels.insert(t.into(), l);
        }
    }
    c.validate().unwrap();
    Policy::new(&c)
}

/// K27 and ruling 28: every other caller is denied outright by default, not asked; Kai's own
/// session runs it without a dialog; the owner can widen the level.
#[test]
fn every_other_caller_is_denied_until_the_owner_widens_the_level() {
    let callers = [
        ("owner", Caller::Owner),
        ("gateway", Caller::Gateway),
        ("trusted key", named("key", "laptop")),
        ("key", named("key", "script")),
        ("device", named("device", "phone")),
        ("unknown", Caller::Unknown(String::new())),
    ];
    for tool in ["conversation_start", "conversation_end"] {
        let (_, spec) = kai_desktop::contract::find(tool).unwrap();
        let p = policy(None);
        assert!(
            matches!(p.decide(spec, &Caller::Own, None), Decision::Allow),
            "{tool}"
        );
        for (who, c) in &callers {
            assert!(
                matches!(p.decide(spec, c, None), Decision::Deny(_)),
                "{tool}: {who}"
            );
        }
        let p = policy(Some(LevelSetting::Owner));
        for (who, c) in &callers {
            let owner = matches!(c, Caller::Owner | Caller::Gateway);
            assert_eq!(
                matches!(p.decide(spec, c, None), Decision::Allow),
                owner,
                "{tool}: {who} at owner"
            );
        }
        let p = policy(Some(LevelSetting::Any));
        for (who, c) in &callers {
            assert!(
                !matches!(p.decide(spec, c, None), Decision::Deny(_)),
                "{tool}: {who} at any"
            );
        }
        // Widened to any, an untrusted caller is asked, not denied (the overlay default, PD4).
        assert!(matches!(
            p.decide(spec, &named("key", "script"), None),
            Decision::Confirm { .. }
        ));
    }
}
