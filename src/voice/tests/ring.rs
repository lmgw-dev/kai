//! The ring's input against the owner's rule (K11, 2026-10-08), through the core: the ring is the
//! indicator that Kai is active, so `active` must hold without a gap from a press to the end of its
//! turn, and through everything continuous listening rides out (review KAI-5 R5-1, R5-8).

use std::time::Duration;

use tokio::time::sleep;

use super::super::mock::MockLink;
use super::super::{Voice, VoiceMode, VoiceSettings};
use super::{Command, LONG, RingInput, RingState, UiCmd, VoiceShown, rig, rig_with};
use crate::config::Config;

/// How often `active` rose and fell over `rings`, starting from `from`.
fn edges(from: bool, rings: &[RingInput]) -> (usize, usize) {
    let (mut rises, mut falls, mut was) = (0, 0, from);
    for i in rings {
        match (was, i.active) {
            (false, true) => rises += 1,
            (true, false) => falls += 1,
            _ => {}
        }
        was = i.active;
    }
    (rises, falls)
}

/// One stretch of activity: one rise, one fall after the reply was spoken, and every input of a
/// turn active.
fn one_stretch(rings: &[RingInput], what: &str) {
    assert_eq!(edges(false, rings), (1, 1), "{what}: {rings:?}");
    let spoke = rings
        .iter()
        .position(|i| i.state == RingState::Speaking)
        .unwrap_or_else(|| panic!("{what}: no speaking state: {rings:?}"));
    let fell = rings.iter().rposition(|i| i.active).unwrap() + 1;
    assert!(
        fell > spoke,
        "{what}: the fall comes after the reply was spoken: {rings:?}"
    );
    assert!(
        rings.iter().all(|i| !i.turn || i.active),
        "{what}: a turn is always active: {rings:?}"
    );
}

/// Review KAI-5 R5-1: a press session's hold released while its bind waits for the last close is
/// still that press's turn (Kai commits it once the session is created), so the ring stays up from
/// the press to the end of its reply: one rise, one fall, and no view torn down in between.
#[tokio::test(start_paused = true)]
async fn a_release_before_the_session_exists_keeps_kai_active_through_the_turn() {
    let mock = MockLink::default();
    mock.with(|s| {
        s.close_lag = Duration::from_millis(800);
        s.reply = Some(("Gut.".into(), 300));
    });
    let mut r = rig_with("[voice]\ngrace_s = 0\n", mock).await;
    // A first press session closes at once after its reply; lmgw answers the close 800 ms later.
    r.send(Command::Talk(true));
    r.created(0).await;
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    r.until_mock("the press session's close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    let before = r.rings();
    assert!(
        before.last().is_some_and(|i| !i.active),
        "standby after the first press: {before:?}"
    );
    // The next press waits for that close; its release comes before its session exists.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(100)).await;
    let mut rings = r.rings();
    assert!(
        rings
            .last()
            .is_some_and(|i| !i.bound && i.active && i.turn && i.state == RingState::Thinking),
        "released, no session yet: the turn runs and waits for its answer: {rings:?}"
    );
    r.until_mock("the second session's commit", |m| {
        m.opens() > 1 && m.socket(1, |s| s.count("input_audio_buffer.commit") == 1)
    })
    .await;
    r.until_mock("the second session's close", |m| {
        m.socket(1, |s| s.closed_by_client)
    })
    .await;
    rings.extend(r.rings());
    one_stretch(&rings, "a release before the bind");
    assert!(
        rings.last().is_some_and(|i| !i.bound && !i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// Review KAI-5 R5-13: a commit waiting for its session counts as the press's turn only while that
/// session is really on its way. A bind that finds no thread gives up: it clears the wait and the
/// connect timeout's start but keeps the flag, so without that condition nothing would ever clear
/// it, and the ring would stay up in push-to-talk standby until the next press. No caller reaches
/// that bind with the flag set today; this pins the condition, not a path.
#[tokio::test(start_paused = true)]
async fn a_bind_that_gives_up_does_not_hold_kai_active() {
    let mut v = Voice::new(
        VoiceSettings::from_config(&Config::default()),
        MockLink::default(),
    );
    let now = tokio::time::Instant::now();
    // A press released while its bind waits for the last close, the thread forgotten meanwhile.
    v.bind_wait = Some(true);
    v.commit_on_create = true;
    v.connect_since = Some(now);
    let i = v.ring(now);
    assert!(
        i.active && i.turn && i.state == RingState::Thinking,
        "on its way: the press's turn waits for its answer: {i:?}"
    );
    v.bind(now, true).await;
    assert!(
        v.commit_on_create,
        "the bind with no thread keeps the flag (the case the condition covers)"
    );
    assert!(
        !v.opening() && v.connect_since.is_none(),
        "nothing is on its way, and no connect timeout runs"
    );
    let i = v.ring(now);
    assert!(!i.active && !i.turn, "standby: {i:?}");
}

/// R5-1 with the release while `current` is still on its way (a press with listening off and
/// nothing to wait for but the thread).
#[tokio::test(start_paused = true)]
async fn a_release_while_the_thread_is_asked_for_keeps_kai_active_through_the_turn() {
    let mock = MockLink::default();
    mock.with(|s| {
        s.current_lag = Duration::from_millis(700);
        s.reply = Some(("Gut.".into(), 300));
    });
    let r = rig_with("[voice]\ngrace_s = 0\n", mock).await;
    r.rings();
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(100)).await;
    let mut rings = r.rings();
    assert_eq!(r.mock.opens(), 0, "the thread is still asked for");
    assert!(
        rings.last().is_some_and(|i| !i.bound && i.active && i.turn),
        "{rings:?}"
    );
    r.until_mock("the session's close", |m| {
        m.opens() > 0 && m.socket(0, |s| s.closed_by_client)
    })
    .await;
    rings.extend(r.rings());
    one_stretch(&rings, "a release while the thread is asked for");
    r.quit().await;
}

/// R5-1 in push-to-talk mode with listening on: a press whose release beats the first session.
/// Between presses Kai is in standby, so after the turn the bound session's ring is hidden.
#[tokio::test(start_paused = true)]
async fn push_to_talk_a_release_before_the_first_session_keeps_kai_active_through_the_turn() {
    let mock = MockLink::default();
    mock.with(|s| {
        s.current_lag = Duration::from_millis(700);
        s.reply = Some(("Gut.".into(), 300));
    });
    let mut r = rig_with("[voice]\nmode = \"push_to_talk\"\n", mock).await;
    r.send(Command::ToggleListening);
    r.until(|m| m.voice.on).await;
    sleep(Duration::from_millis(50)).await;
    let mut rings = r.rings();
    assert!(
        rings.iter().all(|i| !i.active),
        "push-to-talk, listening on: standby: {rings:?}"
    );
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(100)).await;
    rings.extend(r.rings());
    assert_eq!(r.mock.opens(), 0, "the thread is still asked for");
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    rings.extend(r.rings());
    one_stretch(&rings, "push-to-talk, a release before the first session");
    assert!(
        rings.last().is_some_and(|i| i.bound && !i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// Review KAI-5 R5-8: mute mid-turn keeps the ring through the turn, then standby.
#[tokio::test(start_paused = true)]
async fn mute_mid_turn_keeps_kai_active_to_the_turns_end_then_standby() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.rings();
    r.mock.respond(LONG, 1500);
    r.mock.speak();
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.send(Command::ToggleMute);
    r.until(|m| m.voice.muted).await;
    r.until(|m| m.voice.shown != VoiceShown::Speaking).await;
    sleep(Duration::from_millis(500)).await;
    let rings = r.rings();
    assert!(rings.iter().all(|i| !i.turn || i.active), "{rings:?}");
    assert_eq!(edges(true, &rings), (0, 1), "{rings:?}");
    assert!(
        rings
            .last()
            .is_some_and(|i| !i.active && i.muted && i.bound),
        "{rings:?}"
    );
    r.quit().await;
}

/// R5-8: automatic to push-to-talk at runtime puts Kai in standby at once, and back again.
#[tokio::test(start_paused = true)]
async fn automatic_to_push_to_talk_at_runtime_is_standby_at_once() {
    let mut r = rig("").await;
    r.listening_on().await;
    assert!(r.rings().last().is_some_and(|i| i.active));
    r.send(Command::SetVoiceMode(VoiceMode::PushToTalk));
    r.until(|m| m.voice.mode == VoiceMode::PushToTalk).await;
    sleep(Duration::from_millis(100)).await;
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.bound && !i.active),
        "{rings:?}"
    );
    r.send(Command::SetVoiceMode(VoiceMode::Automatic));
    r.until(|m| m.voice.mode == VoiceMode::Automatic).await;
    sleep(Duration::from_millis(100)).await;
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.bound && i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// R5-8: an unknown 4003's one rebind keeps continuous listening active (the audio in between is
/// buffered), like a 4004.
#[tokio::test(start_paused = true)]
async fn an_unknown_4003s_rebind_keeps_continuous_listening_active() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.rings();
    r.mock.revoke_socket("key_paused: device 'kai' was paused");
    r.created(1).await;
    r.until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
        .await;
    let rings = r.rings();
    assert!(
        rings.iter().any(|i| !i.bound) && rings.iter().all(|i| i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// R5-8: continuous listening whose session never starts is active while it buffers, and in
/// standby once the connect timeout gives up.
#[tokio::test(start_paused = true)]
async fn the_connect_timeout_ends_continuous_listenings_activity() {
    let mock = MockLink::default();
    mock.with(|s| s.created_after = None);
    let mut r = rig_with("[voice]\nconnect_timeout_s = 2\n", mock).await;
    r.rings();
    r.send(Command::ToggleListening);
    sleep(Duration::from_millis(500)).await;
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| i.active && !i.turn),
        "on its way: active: {rings:?}"
    );
    r.notice("Kai: the voice session did not start").await;
    r.until(|m| !m.voice.on).await;
    let rings = r.rings();
    assert_eq!(edges(true, &rings), (0, 1), "{rings:?}");
    assert!(
        rings.last().is_some_and(|i| !i.bound && !i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// R5-8: the feed's dead-link verdict ends continuous listening's activity with its session.
#[tokio::test(start_paused = true)]
async fn a_dead_link_ends_continuous_listenings_activity() {
    let mut r = rig("").await;
    r.listening_on().await;
    assert!(r.rings().last().is_some_and(|i| i.active));
    r.feed.raise(crate::lmgw_link::LinkEvent::Down {
        note: "lmgw sent nothing for 30 s: the link is dead".into(),
        dead: true,
    });
    r.until(|m| !m.voice.on).await;
    let rings = r.rings();
    assert!(
        rings.last().is_some_and(|i| !i.bound && !i.active),
        "{rings:?}"
    );
    r.quit().await;
}

/// R5-8: a confirmed re-pair (`leave_gateway`) ends continuous listening's activity with its
/// session.
#[tokio::test(start_paused = true)]
async fn leaving_the_gateway_ends_continuous_listenings_activity() {
    let mut r = rig("").await;
    r.listening_on().await;
    assert!(r.rings().last().is_some_and(|i| i.active));
    let link = crate::pairing::tests::link(&format!(
        "v=1&url=http%3A%2F%2F127.0.0.1%3A2&name=desktop&key={}",
        crate::pairing::tests::KEY
    ));
    r.send(Command::Arrived(
        crate::pairing::PairingLink::parse(&link).unwrap(),
    ));
    let mut rings = Vec::new();
    let id = loop {
        match tokio::time::timeout(Duration::from_secs(60), r.ui.recv())
            .await
            .unwrap()
            .unwrap()
        {
            UiCmd::ConfirmPair(req) => break req.id,
            UiCmd::Overlay(crate::overlay::OverlayCmd::Ring(i)) => rings.push(i),
            _ => continue,
        }
    };
    r.send(Command::ConfirmPair { id, accept: true });
    r.until_mock("the old session's close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    r.until(|m| !m.voice.on).await;
    rings.extend(r.rings());
    assert!(
        rings.last().is_some_and(|i| !i.bound && !i.active),
        "{rings:?}"
    );
    r.quit().await;
}
