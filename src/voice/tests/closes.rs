//! The realtime closes a client tells apart (api §1.6, lmgw 7d6d18c) and the feed's `revoked`
//! kinds (§3.3), through the core against the scripted lmgw. What a close means is its code and,
//! for a 4003, the token its reason starts with (`lmgw_client::realtime::close_kind`); what a
//! `revoked` event means is its `kind`. Kai never reads either from lmgw's words.

use std::time::Duration;

use tokio::time::sleep;

use super::super::link::{Refusal, ThreadRef};
use super::super::mock::refusal;
use super::{Command, FeedFact, Link, LinkEvent, Pairing, VoiceShown, rig};
use crate::lmgw_link::KeyLoss;
use crate::tray::model::{MenuEntry, TrayModel, render};

pub(super) fn calls(r: &super::Rig) -> usize {
    r.mock.with(|s| s.current_calls.len())
}

/// The mock's conversation is `id` at its next `current`, a thread new to it.
pub(super) fn next_thread(r: &super::Rig, id: i64) {
    r.mock.with(|s| {
        s.thread = ThreadRef {
            id,
            title: "New chat".into(),
        };
        s.rolled_over = true;
    });
}

pub(super) fn connected(m: &TrayModel) -> bool {
    matches!(
        m.pairing,
        Pairing::Paired {
            link: Link::Up { .. },
            ..
        }
    )
}

/// What a key that opens nothing now is shown as: the waiting states keep the key and offer Try
/// again, pair again offers only Pair and Quit.
pub(super) fn shows(m: &TrayModel, loss: KeyLoss, words: &str) {
    let expected = match loss {
        KeyLoss::Disabled => Pairing::Disabled {
            reason: words.into(),
        },
        KeyLoss::Expired => Pairing::Expired {
            reason: words.into(),
        },
        KeyLoss::PairAgain => Pairing::Revoked {
            reason: words.into(),
        },
    };
    assert_eq!(m.pairing, expected);
    let menu = render(m).menu;
    match loss {
        KeyLoss::PairAgain => assert_eq!(
            menu,
            [
                MenuEntry::PairWithLmgw,
                MenuEntry::Settings,
                MenuEntry::CheckForUpdates,
                MenuEntry::Quit
            ]
        ),
        _ => assert_eq!(
            menu,
            [
                MenuEntry::TryAgain,
                MenuEntry::PairWithLmgw,
                MenuEntry::Settings,
                MenuEntry::CheckForUpdates,
                MenuEntry::Quit
            ]
        ),
    }
}

pub(super) fn summary(loss: KeyLoss) -> &'static str {
    match loss {
        KeyLoss::Disabled => "Kai: this device is disabled",
        KeyLoss::Expired => "Kai: this device's key expired",
        KeyLoss::PairAgain => "Kai: pair again",
    }
}

pub(super) fn is_lost(m: &TrayModel) -> bool {
    matches!(
        m.pairing,
        Pairing::Revoked { .. } | Pairing::Disabled { .. } | Pairing::Expired { .. }
    )
}

// -------------------------------------------------------------------------- 4003

/// A 4003's token says what it asks: a disabled device is enabled again and an expired key's
/// expiry moved (both wait, the key kept, Try again), a rotated or deleted key and another kind
/// of key's revocation are paired again. lmgw's sentence is shown without the token, and Kai asks
/// nothing more.
#[tokio::test(start_paused = true)]
async fn a_4003_close_says_what_its_kind_asks() {
    for (reason, loss, words) in [
        (
            "device_disabled: device 'kai' was disabled",
            KeyLoss::Disabled,
            "device 'kai' was disabled",
        ),
        (
            "key_expired: device 'kai' expired",
            KeyLoss::Expired,
            "device 'kai' expired",
        ),
        (
            "key_unknown: device 'kai' was rotated — pair it again",
            KeyLoss::PairAgain,
            "device 'kai' was rotated — pair it again",
        ),
        (
            "revoked: key 'kai' was disabled",
            KeyLoss::PairAgain,
            "key 'kai' was disabled",
        ),
    ] {
        let mut r = rig("").await;
        r.listening_on().await;
        let asked = calls(&r);
        r.mock.revoke_socket(reason);
        let n = r.notice(summary(loss)).await;
        let token = reason.split_once(':').unwrap().0;
        assert!(
            n.body.contains(words) && !n.body.contains(&format!("{token}:")),
            "{reason}: {n:?}"
        );
        let m = r.until(is_lost).await;
        shows(&m, loss, words);
        assert!(!m.voice.on, "{reason}");
        let ring = r.rings().last().unwrap().clone();
        assert!(!ring.bound && !ring.active, "{reason}: the ring goes");
        sleep(Duration::from_secs(5)).await;
        assert_eq!(calls(&r), asked, "{reason}: nothing more is asked");
        assert_eq!(r.mock.opens(), 1, "{reason}: no bind again");
        r.quit().await;
    }
}

/// A 4003 of a kind this build does not know, or with no token (an older gateway's): lmgw's
/// guidance is to say its sentence; Kai does, once, and asks `current` once. The key still
/// opening, listening binds again (Kai's own bind, never a takeover, K5), and the pairing stays.
#[tokio::test(start_paused = true)]
async fn a_4003_of_an_unknown_kind_is_said_and_current_asked_once() {
    for reason in [
        "key_paused: device 'kai' was paused",
        // No token: its words are not read, though they name a disable.
        "device 'kai' was disabled",
    ] {
        let words = reason.split_once(": ").map_or(reason, |(_, w)| w);
        let mut r = rig("").await;
        r.listening_on().await;
        let asked = calls(&r);
        r.mock.revoke_socket(reason);
        let n = r.notice("Kai: lmgw ended the connection").await;
        assert!(
            n.body.contains(words) && n.body.contains("still opens"),
            "{reason}: {n:?}"
        );
        r.created(1).await;
        assert_eq!(calls(&r), asked + 1, "{reason}: asked once");
        assert!(
            !r.mock.socket(1, |s| s.take_over),
            "{reason}: Kai's own bind never takes over (K5)"
        );
        let m = r
            .until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
            .await;
        assert!(connected(&m), "{reason}: {:?}", m.pairing);
        for s in [
            "Kai: pair again",
            "Kai: this device is disabled",
            "Kai: lmgw ended the connection",
            "Kai: voice stopped",
        ] {
            r.no_notice(s);
        }
        r.quit().await;
    }
}

/// The answer to that one question decides: a 401 by its code (`device_key_unknown` pairs again,
/// `device_disabled` and `key_expired` wait), and no new session opens.
#[tokio::test(start_paused = true)]
async fn a_4003_of_an_unknown_kind_follows_currents_401() {
    for (code, loss) in [
        ("device_key_unknown", KeyLoss::PairAgain),
        ("device_disabled", KeyLoss::Disabled),
        ("key_expired", KeyLoss::Expired),
    ] {
        let words = format!("lmgw's {code} sentence");
        let mut r = rig("").await;
        r.listening_on().await;
        r.mock
            .with(|s| s.current_error = Some(refusal(401, code, &words)));
        r.mock.revoke_socket("key_paused: device 'kai' was paused");
        r.notice("Kai: lmgw ended the connection").await;
        let n = r.notice(summary(loss)).await;
        assert!(n.body.contains(&words), "{code}: {n:?}");
        let m = r.until(is_lost).await;
        shows(&m, loss, &words);
        assert_eq!(r.mock.opens(), 1, "{code}");
        r.quit().await;
    }
}

// -------------------------------------------------------------------------- 4004

/// A 4004 is the bound thread leaving this device's reach, no revocation: Kai never says pair
/// again; it asks for the folder's current thread and, listening, binds it (its own bind, never a
/// takeover, K5). The tooltip says why the thread changed.
#[tokio::test(start_paused = true)]
async fn a_4004_close_asks_current_again_and_listening_binds_it() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.rings();
    let asked = calls(&r);
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    assert_eq!(calls(&r), asked + 1, "current asked once");
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    assert!(
        !r.mock.socket(1, |s| s.take_over),
        "Kai's own bind never takes over (K5)"
    );
    let m = r
        .until(|m| {
            m.voice.on
                && m.voice.shown == VoiceShown::Quiet
                && m.voice.thread.as_deref() == Some("New chat")
        })
        .await;
    assert!(connected(&m), "{:?}", m.pairing);
    // Listening stays on and the audio in between goes to the new session: Kai stays active, the
    // ring stays up.
    let rings = r.rings();
    assert!(
        rings.iter().any(|i| !i.bound) && rings.iter().all(|i| i.active),
        "{rings:?}"
    );
    assert!(
        m.voice.notes.iter().any(|n| n
            == "the last thread left this device's reach (lmgw: chat thread 812 is out of reach \
                for this key); Kai follows the folder's current thread"),
        "{:?}",
        m.voice.notes
    );
    // lmgw's `error chat_thread_not_found` before the 4004 is no notification (review KAI-4
    // K4-4): the 4004 follows, and its note says what happened.
    for s in [
        "Kai: pair again",
        "Kai: this device is disabled",
        "Kai: voice stopped",
        "Kai: voice error",
    ] {
        r.no_notice(s);
    }
    r.quit().await;
}

/// A 4004 while no listening session ran (a press session in its grace): the thread is only
/// asked for and remembered; the next press binds it.
#[tokio::test(start_paused = true)]
async fn a_4004_close_of_a_press_session_remembers_the_folders_thread() {
    let mut r = rig("[voice]\ngrace_s = 30\n").await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Off && m.voice.timing.is_some())
        .await;
    let asked = calls(&r);
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    let m = r
        .until(|m| m.voice.thread.as_deref() == Some("New chat"))
        .await;
    assert!(!m.voice.on && connected(&m));
    assert_eq!(calls(&r), asked + 1);
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.opens(), 1, "no bind until a press");
    r.send(Command::Talk(true));
    r.created(1).await;
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    r.send(Command::Talk(false));
    r.quit().await;
}

/// If `current` then refuses, that refusal decides: a 401 by its code (pair again only where the
/// code says so), anything else as `current`'s refusals are said; the key is kept where it waits.
#[tokio::test(start_paused = true)]
async fn a_4004_close_whose_current_is_refused_follows_the_refusal() {
    for (code, loss) in [
        ("key_expired", KeyLoss::Expired),
        ("device_disabled", KeyLoss::Disabled),
        ("device_key_unknown", KeyLoss::PairAgain),
    ] {
        let words = format!("lmgw's {code} sentence");
        let mut r = rig("").await;
        r.listening_on().await;
        r.mock
            .with(|s| s.current_error = Some(refusal(401, code, &words)));
        r.mock.out_of_reach_socket();
        let n = r.notice(summary(loss)).await;
        assert!(n.body.contains(&words), "{code}: {n:?}");
        let m = r.until(is_lost).await;
        shows(&m, loss, &words);
        assert_eq!(r.mock.opens(), 1, "{code}");
        r.quit().await;
    }
    // No answer: listening goes off with lmgw's trouble said; the pairing stays.
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.with(|s| {
        s.current_error = Some(Refusal::no_answer(
            "lmgw is not reachable: Connection refused",
        ))
    });
    r.mock.out_of_reach_socket();
    let n = r.notice("Kai: voice could not start").await;
    assert!(n.body.contains("Connection refused"), "{n:?}");
    let m = r.until(|m| !m.voice.on).await;
    assert!(connected(&m), "{:?}", m.pairing);
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

/// lmgw closes with 4004 only once the change is live (api §1.6, lmgw a50141d), so the `current`
/// Kai asks on the close is lmgw's word: it is bound at once, with no word from the feed awaited,
/// even when it names the thread that just closed (it came back into reach meanwhile).
#[tokio::test(start_paused = true)]
async fn a_4004_binds_currents_answer_at_once_whatever_thread_it_names() {
    let mut r = rig("").await;
    r.listening_on().await;
    let asked = calls(&r);
    let closed = tokio::time::Instant::now();
    r.mock.out_of_reach_socket();
    r.until_mock("the bind", |m| m.opens() == 2).await;
    let after = closed.elapsed();
    assert!(
        after < Duration::from_millis(200),
        "bound with current's answer ({:?} of current's lag): {after:?}",
        r.mock.with(|s| s.current_lag)
    );
    assert_eq!(calls(&r), asked + 1, "one ask");
    assert_eq!(r.mock.socket(1, |s| s.thread), 812);
    assert!(!r.mock.socket(1, |s| s.take_over), "K5");
    r.until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
        .await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.mock.opens(), 2, "nothing bound after it");
    r.no_notice("Kai: the voice session did not start");
    r.quit().await;
}

/// A deleted bound thread (lmgw a50141d): the session hears `error chat_thread_not_found` and the
/// 4004, the feed `thread.deleted`, over two connections in no fixed order. Either way Kai binds
/// the folder's current thread once, with its own bind, and says nothing but the 4004's note.
#[tokio::test(start_paused = true)]
async fn a_deleted_threads_4004_and_the_feeds_thread_deleted_bind_once_in_either_order() {
    for feed_first in [true, false] {
        let mut r = rig("").await;
        r.listening_on().await;
        next_thread(&r, 900);
        if feed_first {
            r.feed.fact(FeedFact::ThreadGone { thread_id: 812 });
            // The follow it asks is still on its way when the close comes.
            sleep(Duration::from_millis(5)).await;
            r.mock.out_of_reach_socket();
        } else {
            r.mock.out_of_reach_socket();
            r.feed.fact(FeedFact::ThreadGone { thread_id: 812 });
        }
        r.created(1).await;
        assert_eq!(
            r.mock.socket(1, |s| s.thread),
            900,
            "feed first: {feed_first}"
        );
        assert!(
            !r.mock.socket(1, |s| s.take_over),
            "feed first: {feed_first}: K5"
        );
        let m = r
            .until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
            .await;
        assert!(connected(&m), "feed first: {feed_first}");
        sleep(Duration::from_secs(2)).await;
        assert_eq!(r.mock.opens(), 2, "feed first: {feed_first}: one bind");
        for s in [
            "Kai: voice error",
            "Kai: voice could not start",
            "Kai: voice stopped",
        ] {
            r.no_notice(s);
        }
        r.quit().await;
    }
}

// -------------------------------------------------------------------------- the feed

/// The feed's `revoked` with an expired key's kind waits as a disabled device does: the key kept,
/// Try again offered; Try again starts the link with the stored key, and while the key is still
/// expired the next 401 `key_expired` shows the same state.
#[tokio::test(start_paused = true)]
async fn the_feeds_expired_key_waits_and_try_again_tries_the_key() {
    let mut r = rig("").await;
    let words = "device 'kai' expired";
    r.feed.raise(LinkEvent::Revoked {
        reason: words.into(),
        loss: Some(KeyLoss::Expired),
    });
    let n = r.notice("Kai: this device's key expired").await;
    assert!(
        n.body.contains(words) && n.body.contains("Try again"),
        "{n:?}"
    );
    let m = r.until(is_lost).await;
    shows(&m, KeyLoss::Expired, words);
    // A tray click meanwhile says what to do.
    r.send(Command::ToggleListening);
    let n = r.notice("Kai: this device's key expired").await;
    assert!(n.body.contains("move its expiry"), "{n:?}");
    // Try again: the stored key opens the feed once more.
    r.send(Command::TryAgain);
    r.until(|m| {
        matches!(
            m.pairing,
            Pairing::Paired {
                link: Link::Down { .. },
                ..
            }
        )
    })
    .await;
    r.feed.raise(LinkEvent::Revoked {
        reason: "device 'kai' expired on 2026-10-07".into(),
        loss: Some(KeyLoss::Expired),
    });
    let m = r.until(is_lost).await;
    shows(&m, KeyLoss::Expired, "device 'kai' expired on 2026-10-07");
    r.quit().await;
}

/// The feed's `revoked` of a kind this build does not know, or none: lmgw's sentence is said, and
/// Kai asks `current` once; a 401 decides by its code.
#[tokio::test(start_paused = true)]
async fn the_feeds_revoked_of_an_unknown_kind_asks_once_and_a_401_decides() {
    let mut r = rig("").await;
    let asked = calls(&r);
    r.mock.with(|s| {
        s.current_error = Some(refusal(
            401,
            "device_disabled",
            "device 'kai' is disabled — enable it on Usage → Keys",
        ))
    });
    r.feed.raise(LinkEvent::Revoked {
        reason: "device 'kai' was paused".into(),
        loss: None,
    });
    let n = r.notice("Kai: lmgw ended the connection").await;
    assert!(n.body.contains("device 'kai' was paused"), "{n:?}");
    let m = r.until(is_lost).await;
    shows(
        &m,
        KeyLoss::Disabled,
        "device 'kai' is disabled — enable it on Usage → Keys",
    );
    assert_eq!(calls(&r), asked + 1, "asked once");
    r.quit().await;
}

/// When that question's answer is no 401, the key still opens: the core starts nothing itself; the
/// feed's own try after its backoff (review KAI-4 K4-7) connects, and the pairing never changed.
/// The same sentence met again is not said again.
#[tokio::test(start_paused = true)]
async fn the_feeds_revoked_of_an_unknown_kind_whose_key_still_opens_leaves_the_retry_to_the_feed() {
    let mut r = rig("").await;
    let asked = calls(&r);
    for _ in 0..2 {
        r.feed.raise(LinkEvent::Revoked {
            reason: "device 'kai' was paused".into(),
            loss: None,
        });
        r.feed.raise(LinkEvent::Down {
            note: "lmgw ended the feed: device 'kai' was paused; next try in 1 s".into(),
            dead: false,
        });
        let m = r
            .until(|m| {
                matches!(
                    &m.pairing,
                    Pairing::Paired { link: Link::Down { note }, .. } if note.contains("next try in")
                )
            })
            .await;
        assert!(!is_lost(&m));
        r.feed.raise(LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: None,
            admin: Default::default(),
        });
        r.until(connected).await;
    }
    sleep(Duration::from_secs(1)).await;
    assert_eq!(calls(&r), asked + 2, "asked once each time");
    r.notice("Kai: lmgw ended the connection").await;
    r.no_notice("Kai: lmgw ended the connection");
    r.no_notice("Kai: pair again");
    r.quit().await;
}
