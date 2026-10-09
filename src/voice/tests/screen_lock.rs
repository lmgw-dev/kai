//! The screen lock through the core (decided by the owner 2026-10-09: Kai is not active while the
//! screen is locked), in paused time: a lock while Kai speaks, during a press, in conversation
//! mode and while the approval card shows; the talk key, listening and a new conversation doing
//! nothing while locked; and Kai idle after the unlock.

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::approvals::{Card, request};
use super::*;
use crate::core::calls::CoreCall;
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Reply};
use crate::tray::model::ICON_IDLE;

const LONG: &str = "eins zwei drei vier fünf sechs sieben acht neun zehn elf zwölf dreizehn";

impl Rig {
    /// The GTK side's lock commands since the last call (ring commands are dropped).
    fn lock_cmds(&self) -> Vec<bool> {
        let mut v = Vec::new();
        while let Ok(c) = self.ui.try_recv() {
            if let UiCmd::ScreenLocked(l) = c {
                v.push(l);
            }
        }
        v
    }

    /// The lock's watch: the shared flag, then the core told.
    async fn lock(&mut self, locked: bool) -> TrayModel {
        self.screen.set(locked);
        self.send(Command::ScreenLock);
        self.until(|m| m.voice.locked == locked).await
    }
}

/// Calls a core tool the way the host does.
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

/// Speaking: the reply stops with its truncate, the session closes, the microphone is released,
/// the ring goes, the tray says locked; after the unlock Kai is idle (listening stays off, no
/// session opens by itself).
#[tokio::test(start_paused = true)]
async fn a_lock_while_kai_speaks_stops_it_and_kai_stays_idle_after() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.respond(LONG, 3000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    sleep(Duration::from_millis(700)).await;
    let _ = r.rings();
    let m = r.lock(true).await;
    r.until_mock("the truncate", |m| m.socket(0, |s| !s.truncates.is_empty()))
        .await;
    r.until_mock("the close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    sleep(Duration::from_millis(100)).await;
    let played = r.played();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(r.played(), played, "nothing more is played");
    assert!(!m.voice.on, "listening is off");
    let view = render(&m);
    assert_eq!(view.icon, ICON_IDLE);
    assert_eq!(view.status, "screen locked: Kai is not active");
    assert!(
        view.notes
            .iter()
            .any(|n| n.starts_with("the screen is locked")),
        "{:?}",
        view.notes
    );
    let mut rings = Vec::new();
    let mut locks = Vec::new();
    while let Ok(c) = r.ui.try_recv() {
        match c {
            UiCmd::Overlay(OverlayCmd::Ring(i)) => rings.push(i),
            UiCmd::ScreenLocked(l) => locks.push(l),
            _ => {}
        }
    }
    assert!(
        rings.last().is_some_and(|i| !i.bound && !i.active),
        "the ring goes: {rings:?}"
    );
    assert_eq!(locks, [true], "the widgets were told");
    r.until(|m| m.voice.shown == VoiceShown::Off).await;
    let m = r.lock(false).await;
    assert!(
        !m.voice.on && m.voice.shown == VoiceShown::Off,
        "idle after the unlock"
    );
    assert!(
        !m.voice.notes.iter().any(|n| n.contains("screen")),
        "the lock's lines went: {:?}",
        m.voice.notes
    );
    sleep(Duration::from_secs(10)).await;
    assert_eq!(r.mock.opens(), 1, "no session opens by itself");
    assert_eq!(r.lock_cmds(), [false], "and told again");
    r.quit().await;
}

/// A press session's hold: discarded at the lock (no commit, no response), the session closed;
/// the release after it does nothing, and the talk key works again after the unlock.
#[tokio::test(start_paused = true)]
async fn a_lock_during_a_press_discards_the_hold() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\n").await;
    r.send(Command::Talk(true));
    r.created(0).await;
    sleep(Duration::from_millis(600)).await;
    r.lock(true).await;
    r.until_mock("the close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(500)).await;
    r.mock.socket(0, |s| {
        assert_eq!(s.count("input_audio_buffer.commit"), 0, "{:?}", s.kinds);
        assert_eq!(s.count("response.create"), 0, "{:?}", s.kinds);
    });
    let m = r.until(|m| m.voice.shown == VoiceShown::Off).await;
    assert!(!m.voice.on);
    r.lock(false).await;
    assert_eq!(r.mock.opens(), 1);
    r.send(Command::Talk(true));
    r.created(1).await;
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    r.until_mock("the next press's turn", |m| {
        m.socket(1, |s| s.count("response.create") == 1)
    })
    .await;
    r.quit().await;
}

/// Conversation mode ends at the lock and does not come back after the unlock; asked for while
/// locked, it is refused with the lock's words.
#[tokio::test(start_paused = true)]
async fn a_lock_ends_conversation_mode_for_good() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\nconversation_timeout_s = 0\n").await;
    r.listening_on().await;
    assert!(!tool(&r, "conversation_start").await.error);
    let m = r
        .until(|m| {
            m.voice
                .notes
                .iter()
                .any(|n| n.starts_with("conversation mode:"))
        })
        .await;
    assert!(m.voice.on);
    let m = r.lock(true).await;
    assert!(
        !m.voice
            .notes
            .iter()
            .any(|n| n.contains("conversation mode")),
        "{:?}",
        m.voice.notes
    );
    let refused = tool(&r, "conversation_start").await;
    assert!(refused.error);
    assert_eq!(text(&refused), "the screen is locked; Kai is not active");
    let m = r.lock(false).await;
    assert!(
        !m.voice.on,
        "idle: neither the mode nor listening came back"
    );
    sleep(Duration::from_secs(30)).await;
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

/// While locked the talk key, the tray's listening and a new conversation do nothing.
#[tokio::test(start_paused = true)]
async fn the_talk_key_and_listening_do_nothing_while_locked() {
    let mut r = rig("").await;
    r.lock(true).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.send(Command::ToggleListening);
    r.send(Command::SetListening(true));
    r.send(Command::NewConversation);
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.opens(), 0);
    assert_eq!(
        r.pw.lock().unwrap().captures,
        0,
        "the microphone never opened"
    );
    let m = r.until(|m| m.voice.locked).await;
    assert!(!m.voice.on && m.voice.shown == VoiceShown::Off);
    r.lock(false).await;
    r.listening_on().await;
    r.quit().await;
}

/// The approval card hides at the lock (its call keeps waiting, the tooltip says where) and shows
/// again after the unlock; a press while locked records no spoken answer.
#[tokio::test(start_paused = true)]
async fn the_approval_card_hides_while_locked_and_shows_again_after() {
    let _ = crate::approvals::overlay::test_cards();
    let mut card = Card::default();
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::ApprovalRequest(request("mcpr_1")));
    let shown = card.until("the card", |m| m.is_some()).await;
    card.arm();
    let m = r.lock(true).await;
    card.until("hidden", |m| m.is_none()).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("the card shows again once the screen is unlocked")),
        "{:?}",
        m.voice.notes
    );
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(500)).await;
    assert!(
        matches!(card.now(), Some(None)),
        "still hidden, nothing recorded"
    );
    r.lock(false).await;
    let again = card.until("shown again", |m| m.is_some()).await;
    assert_eq!(again.call, shown.call, "the same call still waits");
    r.quit().await;
}

/// A burst (lock, unlock, lock) queued before the core gets to it: the core reads the shared flag
/// for each and never writes it, so the flag stays locked throughout (the host refuses all along)
/// and the core acts once, on the lock.
#[tokio::test(start_paused = true)]
async fn a_burst_of_lock_changes_leaves_the_flag_to_its_watch() {
    let mut r = rig("").await;
    let _ = r.lock_cmds();
    for locked in [true, false, true] {
        r.screen.set(locked);
        r.send(Command::ScreenLock);
    }
    for _ in 0..200 {
        assert!(r.screen.is_locked(), "only the watch writes the flag");
        tokio::task::yield_now().await;
    }
    let m = r.until(|m| m.voice.locked).await;
    assert!(m.voice.locked);
    assert!(r.screen.is_locked());
    assert_eq!(r.lock_cmds(), [true], "acted once, on the lock");
    // A command with no change of the flag does nothing.
    r.send(Command::ScreenLock);
    sleep(Duration::from_millis(50)).await;
    assert!(r.lock_cmds().is_empty());
    let m = r.lock(false).await;
    assert!(!m.voice.locked);
    assert_eq!(r.lock_cmds(), [false]);
}
