//! Asking for the conversation's thread again (review KAI-4, K4-9's list), through the core against
//! the scripted lmgw: what a 4004 meets (a reply playing, a held talk key, a second 4004, a bind's
//! 404), what a refused bind whose body did not come means (K4-5, K4-8), and the owner's press or
//! listening on while Kai asks (K4-1, K4-2).

use std::time::Duration;

use tokio::time::sleep;

use super::super::mock::refusal;
use super::closes::{calls, connected, is_lost, next_thread, shows, summary};
use super::{Command, VoiceShown, rig};
use crate::lmgw_link::KeyLoss;

/// A press session in its grace (listening off): a hold, its reply played.
async fn press_session(r: &mut super::Rig) {
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Off && m.voice.timing.is_some())
        .await;
}

fn quiet_on(m: &crate::tray::model::TrayModel) -> bool {
    m.voice.on && m.voice.shown == VoiceShown::Quiet
}

// -------------------------------------------------------------------------- what a 4004 meets

/// A 4004 while Kai speaks: the reply stops with the session (nothing more reaches the sink), and
/// listening binds the folder's current thread.
#[tokio::test(start_paused = true)]
async fn a_4004_while_speaking_stops_the_reply_and_binds_again() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.speak();
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    // What the sink had queued plays out; nothing more comes, far short of the 1.5 s reply.
    sleep(Duration::from_secs(2)).await;
    let played = r.played();
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.played(), played, "the reply stopped with its session");
    assert!(played < 1500 * 24, "{played} samples of the reply played");
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    r.until(quiet_on).await;
    r.no_notice("Kai: voice error");
    r.quit().await;
}

/// A 4004 while the talk key is held: the hold talked into the session that ended, so its turn
/// went with it, and the tooltip says so; the release commits nothing on the new session.
#[tokio::test(start_paused = true)]
async fn a_4004_while_the_talk_key_is_held_drops_the_held_turn_and_says_so() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\n").await;
    r.listening_on().await;
    r.send(Command::Talk(true));
    r.until(|m| m.voice.shown == VoiceShown::Listening).await;
    sleep(Duration::from_millis(400)).await;
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    let m = r
        .until(|m| {
            m.voice
                .notes
                .iter()
                .any(|n| n.contains("the held turn went with it: press the talk key again"))
        })
        .await;
    assert!(m.voice.on);
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(2)).await;
    assert_eq!(
        r.mock.socket(1, |s| s.count("input_audio_buffer.commit")),
        0,
        "nothing of the dropped hold is committed"
    );
    r.no_notice("Kai: voice error");
    r.quit().await;
}

/// A 4004 before `session.created`: the same; the folder's current thread is bound.
#[tokio::test(start_paused = true)]
async fn a_4004_before_session_created_binds_the_folders_thread() {
    let mut r = rig("").await;
    r.mock
        .with(|s| s.created_after = Some(Duration::from_secs(2)));
    r.send(Command::SetListening(true));
    r.until_mock("the bind", |m| m.opens() == 1).await;
    sleep(Duration::from_millis(100)).await;
    r.mock
        .with(|s| s.created_after = Some(Duration::from_millis(200)));
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    r.until(quiet_on).await;
    r.quit().await;
}

/// Two 4004s in a row: each is lmgw's word, so each binds the folder's current thread as it is
/// then, with Kai's own bind.
#[tokio::test(start_paused = true)]
async fn two_4004s_in_a_row_each_bind_the_folders_current_thread() {
    let mut r = rig("").await;
    r.listening_on().await;
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    r.created(1).await;
    r.until(quiet_on).await;
    next_thread(&r, 901);
    r.mock.out_of_reach_socket();
    r.created(2).await;
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    assert_eq!(r.mock.socket(2, |s| s.thread), 901);
    assert!(!r.mock.socket(2, |s| s.take_over), "K5");
    let m = r.until(quiet_on).await;
    assert!(connected(&m));
    r.no_notice("Kai: voice error");
    r.no_notice("Kai: voice could not start");
    r.quit().await;
}

/// K4-8: the thread left reach between `current` and the bind: the bind's 404 is a 4004's twin.
/// Listening stays on and the next answer is bound, with or without the body's code (K4-5).
#[tokio::test(start_paused = true)]
async fn a_404_on_the_rebind_asks_again_and_binds_the_next_answer() {
    for code in [Some("chat_thread_not_found"), None] {
        let mut r = rig("").await;
        r.listening_on().await;
        // lmgw's first answer after the 4004 is thread 900, whose bind is refused; then 901.
        r.mock.with(|s| {
            s.answers_first.push(super::super::link::ThreadRef {
                id: 900,
                title: "New chat".into(),
            });
            s.refuse_opens.push((
                404,
                code.map(Into::into),
                "there is no chat thread 900".into(),
            ));
        });
        next_thread(&r, 901);
        r.mock.out_of_reach_socket();
        r.created(2).await;
        assert_eq!(r.mock.socket(1, |s| s.thread), 900, "{code:?}");
        assert_eq!(r.mock.socket(2, |s| s.thread), 901, "{code:?}");
        assert!(!r.mock.socket(2, |s| s.take_over), "{code:?}: K5");
        r.until(quiet_on).await;
        r.no_notice("Kai: voice could not start");
        r.quit().await;
    }
}

/// K4-8 for the owner's press: its bind refused 404, the press binds the next answer as the
/// owner's, and its hold goes there whole (its buffered audio and its release kept).
#[tokio::test(start_paused = true)]
async fn a_404_on_a_presss_bind_binds_the_next_answer_with_the_hold() {
    let mut r = rig("").await;
    r.mock.with(|s| {
        s.answers_first.push(super::super::link::ThreadRef {
            id: 900,
            title: "New chat".into(),
        });
        s.refuse_opens.push((404, None, "HTTP 404".into()));
    });
    next_thread(&r, 901);
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until_mock("the press's turn on the next thread", |m| {
        m.opens() == 2 && m.socket(1, |s| s.count("response.create") == 1)
    })
    .await;
    r.mock.socket(1, |s| {
        assert_eq!(s.thread, 901);
        assert!(s.take_over, "the owner's press");
        let ms = s.appended as u64 * 1000 / 24_000;
        assert!(ms >= 590, "{ms} ms of the hold went up");
    });
    r.no_notice("Kai: voice could not start");
    r.quit().await;
}

// -------------------------------------------------------------------------- refusals by status

/// K4-5: a bind's 401 whose body did not come: Kai asks `current`, whose 401 is read whole, and
/// its code decides: a disabled device waits, its key kept.
#[tokio::test(start_paused = true)]
async fn a_bind_refused_401_without_its_code_asks_current_whose_code_decides() {
    for (code, loss) in [
        ("device_disabled", KeyLoss::Disabled),
        ("key_expired", KeyLoss::Expired),
        ("device_key_unknown", KeyLoss::PairAgain),
    ] {
        let words = format!("lmgw's {code} sentence");
        let mut r = rig("").await;
        let asked = calls(&r);
        r.mock.with(|s| {
            s.refuse_opens.push((401, None, "HTTP 401".into()));
            // The listening's own `current` answers; the next one is refused.
            s.current_script = vec![None, Some(refusal(401, code, &words))];
        });
        r.send(Command::SetListening(true));
        let n = r.notice(summary(loss)).await;
        assert!(n.body.contains(&words), "{code}: {n:?}");
        let m = r.until(is_lost).await;
        shows(&m, loss, &words);
        assert_eq!(calls(&r), asked + 2, "{code}: one ask for the code");
        assert_eq!(r.mock.opens(), 1, "{code}");
        r.quit().await;
    }
}

/// A second 401 without its code, after `current` answered: "pair again", as any 401 without a
/// known code; Kai never asks again and again.
#[tokio::test(start_paused = true)]
async fn a_second_401_without_its_code_pairs_again() {
    let mut r = rig("").await;
    r.mock.with(|s| {
        s.refuse_opens = vec![(401, None, "HTTP 401".into()); 2];
    });
    r.send(Command::SetListening(true));
    let n = r.notice("Kai: pair again").await;
    assert!(n.body.contains("HTTP 401"), "{n:?}");
    r.until(is_lost).await;
    assert_eq!(r.mock.opens(), 2);
    r.quit().await;
}

/// K4-5: Kai's own bind refused 409 without its code is another client's voice (K2-13): off until
/// a press.
#[tokio::test(start_paused = true)]
async fn a_409_without_its_code_on_kais_own_bind_stays_off() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_opens.push((409, None, "HTTP 409".into())));
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    let n = r.notice("Kai: voice in use elsewhere").await;
    assert!(
        n.body
            .contains("voice is in use on another client; Kai stays off until you press"),
        "{n:?}"
    );
    let m = r.until(|m| !m.voice.on).await;
    assert!(connected(&m));
    r.quit().await;
}

// -------------------------------------------------------------------------- the owner meanwhile

/// K4-1: a press while Kai asks again after a press session's 4004 binds the answer as the
/// owner's, and its turn goes there.
#[tokio::test(start_paused = true)]
async fn a_press_during_a_reask_binds_the_answer_as_the_owners() {
    let mut r = rig("[voice]\ngrace_s = 30\n").await;
    press_session(&mut r).await;
    r.mock.with(|s| s.current_lag = Duration::from_secs(2));
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    sleep(Duration::from_millis(300)).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until_mock("the press's turn", |m| {
        m.opens() == 2 && m.socket(1, |s| s.count("response.create") == 1)
    })
    .await;
    r.mock.socket(1, |s| {
        assert_eq!(s.thread, 900);
        assert!(s.take_over, "the owner's press");
    });
    r.quit().await;
}

/// K4-2: listening on while Kai asks again binds the answer, with the microphone open and its
/// audio going up.
#[tokio::test(start_paused = true)]
async fn listening_on_during_a_reask_binds_with_the_microphone() {
    let mut r = rig("[voice]\ngrace_s = 30\n").await;
    press_session(&mut r).await;
    r.mock.with(|s| s.current_lag = Duration::from_secs(2));
    next_thread(&r, 900);
    r.mock.out_of_reach_socket();
    sleep(Duration::from_millis(300)).await;
    let captures = r.pw.lock().unwrap().captures;
    r.send(Command::SetListening(true));
    r.created(1).await;
    r.until(quiet_on).await;
    sleep(Duration::from_secs(1)).await;
    assert!(r.pw.lock().unwrap().captures > captures, "a capture opened");
    assert!(r.mock.socket(1, |s| s.appended) > 0, "audio went up");
    assert_eq!(r.mock.socket(1, |s| s.thread), 900);
    r.quit().await;
}

// -------------------------------------------------------------------------- unknown kinds

/// K4-7: a 4003 of a kind Kai does not know binds again once; another in a row turns listening
/// off (the owner's press starts again). lmgw's sentence is said once, the feed's `revoked` of the
/// same reason included.
#[tokio::test(start_paused = true)]
async fn an_unknown_4003_binds_again_once_and_its_sentence_is_said_once() {
    let mut r = rig("").await;
    r.listening_on().await;
    let reason = "key_paused: device 'kai' was paused";
    r.mock.revoke_socket(reason);
    let n = r.notice("Kai: lmgw ended the connection").await;
    assert!(n.body.contains("device 'kai' was paused"), "{n:?}");
    r.created(1).await;
    r.until(quiet_on).await;
    r.feed.raise(super::LinkEvent::Revoked {
        reason: "device 'kai' was paused".into(),
        loss: None,
    });
    // The feed's own try after its backoff connects again.
    r.feed.raise(super::LinkEvent::Connected {
        device: "desktop".into(),
        held: false,
        hosts_label: None,
        admin: Default::default(),
    });
    r.until(connected).await;
    r.mock.revoke_socket(reason);
    let n = r.notice("Kai: voice stopped").await;
    assert!(
        n.body
            .contains("lmgw ended the voice session again (device 'kai' was paused)"),
        "{n:?}"
    );
    let m = r.until(|m| !m.voice.on).await;
    assert!(!is_lost(&m));
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.opens(), 2, "no third bind by itself");
    r.no_notice("Kai: lmgw ended the connection");
    // The owner's press binds again.
    r.send(Command::ToggleListening);
    r.created(2).await;
    r.quit().await;
}
