//! WP18 through the core, in paused time: a job's result is spoken at a quiet moment — never while
//! the owner talks (a held key, speech heard), never while Kai speaks, never on a locked screen,
//! only after `jobs.quiet_ms` of quiet — with one tagged `response.create` and no new words; lmgw's
//! refusals `turn_running` (asked again after the running turn) and `empty_turn` (silent); with no
//! session Kai binds its conversation itself (never taking it over), and announce off, another
//! thread or a held thread is a notification. A cough is never a continuation.

use std::time::Duration;

use lmgw_client::realtime::TaskDone;
use tokio::time::sleep;

use super::*;
use crate::host::policy::Caller;
use crate::jobs::{Delivery, Start};
use crate::voice::link::ErrorFacts;

/// The default `jobs.quiet_ms`.
const QUIET: Duration = Duration::from_millis(1500);

fn done(thread: i64, message: i64) -> TaskDone {
    TaskDone {
        thread_id: thread,
        message_id: message,
        id: message,
        task_id: format!("t{message}"),
        server_label: "builds".into(),
        tool: "builds__compile".into(),
        status: "completed".into(),
        by: None,
    }
}

/// The tagged creates (`kai_job_<n>`) socket `n` got.
fn job_creates(r: &Rig, n: usize) -> Vec<String> {
    r.mock.socket(n, |s| {
        s.tagged
            .iter()
            .filter(|t| t.starts_with("kai_job_"))
            .cloned()
            .collect()
    })
}

impl Rig {
    async fn locked(&mut self, locked: bool) {
        self.screen.set(locked);
        self.send(Command::ScreenLock);
        self.until(|m| m.voice.locked == locked).await;
    }

    /// Do not disturb on or off, taken by the core before the test goes on. The tray model does
    /// not show it, and the core reads the commands and the feed's facts from separate channels in
    /// no fixed order (an unbiased select): a fact sent right after the command can be handled
    /// first, and its tick can notify the result while do not disturb is still on. In paused time
    /// the sleep returns only once every task is idle (only then does the clock move), so the core
    /// has handled the command by then.
    async fn dnd(&self, on: bool) {
        self.send(Command::Dnd(on));
        sleep(Duration::from_millis(1)).await;
    }

    fn other_turn(&self, thread: i64, running: bool) {
        let by = By {
            name: "the dashboard".into(),
            own: false,
        };
        self.feed.fact(if running {
            FeedFact::TurnStarted {
                thread_id: thread,
                by,
            }
        } else {
            FeedFact::TurnDone { thread_id: thread }
        });
    }
}

/// A bound session: nothing before `jobs.quiet_ms` of quiet, then one bare create (no commit, no
/// append of words), the reply plays, and nothing more is asked.
#[tokio::test(start_paused = true)]
async fn a_result_in_the_bound_session_is_spoken_after_the_quiet_wait() {
    let mut r = rig("").await;
    r.listening_on().await;
    let before = r.mock.socket(0, |s| s.count("input_audio_buffer.commit"));
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    // The feed says the same result: said once.
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    sleep(QUIET - Duration::from_millis(200)).await;
    assert!(job_creates(&r, 0).is_empty(), "not before jobs.quiet_ms");
    r.until_mock("the job's create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    sleep(Duration::from_secs(10)).await;
    assert_eq!(job_creates(&r, 0), ["kai_job_1"]);
    r.mock.socket(0, |s| {
        assert_eq!(s.count("response.create"), 1, "{:?}", s.kinds);
        assert_eq!(s.count("input_audio_buffer.commit"), before, "no new words");
    });
    r.no_notice("Kai: a job ended");
    r.quit().await;
}

/// Push-to-talk: not while the key is held, not while Kai speaks the held turn's reply; the
/// continuation comes `jobs.quiet_ms` after that reply, and is no commit of its own.
#[tokio::test(start_paused = true)]
async fn never_while_the_owner_talks_or_kai_speaks() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\n").await;
    r.listening_on().await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(100)).await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    sleep(Duration::from_secs(4)).await;
    assert!(job_creates(&r, 0).is_empty(), "the key is held");
    r.send(Command::Talk(false));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    // The reply is 1.5 s: the result waits through it.
    sleep(Duration::from_millis(1000)).await;
    assert!(job_creates(&r, 0).is_empty(), "Kai speaks");
    r.until(|m| m.voice.shown != VoiceShown::Speaking).await;
    let quiet_from = tokio::time::Instant::now();
    r.until_mock("the job's create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    assert!(quiet_from.elapsed() >= QUIET - Duration::from_millis(40));
    r.mock.socket(0, |s| {
        let at = s
            .kinds
            .iter()
            .rposition(|k| *k == "response.create")
            .unwrap();
        assert_ne!(
            s.kinds[at - 1],
            "input_audio_buffer.commit",
            "a bare create: {:?}",
            s.kinds
        );
        assert_eq!(
            s.count("input_audio_buffer.commit"),
            1,
            "only the held turn's"
        );
    });
    r.quit().await;
}

/// Continuous listening: speech heard restarts the wait; a cough (speech, a commit, a response
/// lmgw refuses `empty_turn`) is no continuation, and Kai's create still waits for the quiet.
#[tokio::test(start_paused = true)]
async fn speech_and_a_cough_restart_the_wait() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    sleep(Duration::from_millis(1000)).await;
    // A cough: the server hears it and refuses the automatic response (no words).
    r.mock.emit(ServerEvent::SpeechStarted);
    sleep(Duration::from_millis(300)).await;
    r.mock.emit(ServerEvent::SpeechStopped);
    r.mock.emit(ServerEvent::Committed);
    r.mock.emit(ServerEvent::ResponseCreated {
        response_id: "resp_cough".into(),
    });
    r.mock.emit(ServerEvent::Error(ErrorFacts {
        kind: "invalid_request_error".into(),
        code: Some("empty_turn".into()),
        message: "nothing to answer".into(),
    }));
    r.mock.emit(ServerEvent::ResponseDone {
        response_id: "resp_cough".into(),
        status: "failed".into(),
        reason: Some("nothing to answer".into()),
    });
    let cough_end = tokio::time::Instant::now();
    sleep(Duration::from_millis(1000)).await;
    assert!(
        job_creates(&r, 0).is_empty(),
        "the cough restarted the wait"
    );
    r.until_mock("the job's create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    assert!(cough_end.elapsed() >= QUIET - Duration::from_millis(40));
    sleep(Duration::from_secs(5)).await;
    assert_eq!(job_creates(&r, 0).len(), 1);
    r.quit().await;
}

/// `turn_running` with the turn on the feed: no create while it runs; after its `turn.done` the
/// next quiet moment asks again.
#[tokio::test(start_paused = true)]
async fn turn_running_asks_again_after_the_running_turn() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["turn_running".into()]);
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("the first create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.other_turn(812, true);
    sleep(Duration::from_secs(20)).await;
    assert_eq!(job_creates(&r, 0), ["kai_job_1"], "not while the turn runs");
    let m = r.until(|_| true).await;
    assert!(
        !m.voice.notes.iter().any(|n| n.contains("turn_running")),
        "no voice error: {:?}",
        m.voice.notes
    );
    r.other_turn(812, false);
    r.until_mock("the second create", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 2
        })
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.quit().await;
}

/// `turn_running` whose turn the feed never showed: one more ask at the next quiet moment; refused
/// again, Kai waits `jobs.quiet_ms` for the feed to show it and then gives up with a notification
/// (never a wait that nothing ends: the session and the microphone are not held for it).
#[tokio::test(start_paused = true)]
async fn turn_running_unseen_asks_once_more_then_gives_up() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["turn_running".into(), "turn_running".into()]);
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("two creates", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 2
        })
    })
    .await;
    let n = r.notice("Kai: a job ended").await;
    assert!(n.body.contains("lmgw says a turn runs there"), "{n:?}");
    sleep(Duration::from_secs(20)).await;
    assert_eq!(job_creates(&r, 0).len(), 2, "given up: no third ask");
    r.quit().await;
}

/// The second unseen `turn_running`, and the feed shows the turn soon after: Kai waits for its
/// `turn.done`, then asks again.
#[tokio::test(start_paused = true)]
async fn turn_running_shown_late_is_waited_for() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["turn_running".into(), "turn_running".into()]);
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("two creates", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 2
        })
    })
    .await;
    r.other_turn(812, true);
    sleep(Duration::from_secs(20)).await;
    assert_eq!(job_creates(&r, 0).len(), 2, "the turn runs");
    r.no_notice("Kai: a job ended");
    r.other_turn(812, false);
    r.until_mock("the third create", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 3
        })
    })
    .await;
    r.quit().await;
}

/// A turn the feed showed that leaves its live state without a `turn.done` (a resync's `state`,
/// a feed that dropped) ends the wait: Kai asks again at the next quiet moment rather than wait for
/// an event that will not come (the session and the microphone held meanwhile).
#[tokio::test(start_paused = true)]
async fn a_turn_wait_ends_when_the_turn_leaves_the_live_state() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["turn_running".into(), "turn_running".into()]);
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("two creates", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 2
        })
    })
    .await;
    // The feed shows the turn: Kai waits for its end.
    r.other_turn(812, true);
    sleep(Duration::from_secs(20)).await;
    assert_eq!(job_creates(&r, 0).len(), 2);
    // The feed's live state no longer has it (its turn.done was lost).
    r.feed.fact(FeedFact::Live {
        voice: Vec::new(),
        turns: Vec::new(),
    });
    r.until_mock("the third create", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 3
        })
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.no_notice("Kai: a job ended");
    r.quit().await;
}

/// Muted: the result waits (still owed), nothing is spoken or notified; after the unmute it is
/// spoken at the next quiet moment.
#[tokio::test(start_paused = true)]
async fn muted_the_result_waits_for_the_unmute() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.send(Command::SetMuted(true));
    r.until(|m| m.voice.muted).await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    sleep(Duration::from_secs(30)).await;
    assert!(job_creates(&r, 0).is_empty(), "muted");
    r.no_notice("Kai: a job ended");
    r.send(Command::SetMuted(false));
    let unmuted = tokio::time::Instant::now();
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    assert!(unmuted.elapsed() >= QUIET - Duration::from_millis(40));
    r.quit().await;
}

/// Do not disturb: not spoken, notified (the notification server keeps it quiet), and no bind.
#[tokio::test(start_paused = true)]
async fn do_not_disturb_notifies_instead_of_speaking() {
    let mut r = rig("").await;
    r.dnd(true).await;
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    let n = r.notice("Kai: a job ended").await;
    assert!(n.body.contains("do not disturb is on"), "{n:?}");
    sleep(Duration::from_secs(10)).await;
    assert_eq!(r.mock.opens(), 0, "no bind for it");
    // Off again: the next result is spoken.
    r.dnd(false).await;
    r.feed.fact(FeedFact::TaskDone(done(812, 42)));
    r.until_mock("a bind", |m| m.opens() == 1).await;
    r.quit().await;
}

/// A result that comes while a response of its thread runs is not asked for while it runs (lmgw:
/// that response answers it); after its `response.done` Kai asks at the next quiet moment, and
/// lmgw's `empty_turn` (it was answered) ends it silently.
#[tokio::test(start_paused = true)]
async fn a_result_during_a_response_waits_for_its_end() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["empty_turn".into()]);
    r.mock.respond("Ein Satz.", 4000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    sleep(Duration::from_millis(2500)).await;
    assert!(job_creates(&r, 0).is_empty(), "not while the response runs");
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    let quiet_from = tokio::time::Instant::now();
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    assert!(quiet_from.elapsed() >= QUIET - Duration::from_millis(40));
    sleep(Duration::from_secs(10)).await;
    assert_eq!(job_creates(&r, 0).len(), 1);
    r.no_notice("Kai: a job ended");
    r.quit().await;
}

/// A continuation cut off (a barge-in) is notified: the rest is in its chat.
#[tokio::test(start_paused = true)]
async fn a_cut_off_continuation_is_notified() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.mock.with(|s| s.reply = None);
    r.mock.speech_started();
    let n = r.notice("Kai: a job ended").await;
    assert!(n.body.contains("cut off; it is in its chat"), "{n:?}");
    r.mock.emit(ServerEvent::SpeechStopped);
    sleep(Duration::from_secs(10)).await;
    assert_eq!(job_creates(&r, 0).len(), 1, "not asked again");
    r.quit().await;
}

/// Push-to-talk: Kai's own bind for a result opens no microphone (no capture, no indicator); the
/// owner's press in that session opens it and talks.
#[tokio::test(start_paused = true)]
async fn kais_own_bind_opens_no_microphone() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\ngrace_s = 30\n").await;
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    r.until_mock("the create", |m| {
        m.opens() == 1 && m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown != VoiceShown::Speaking).await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(
        r.mock.opens(),
        1,
        "the session is still there (grace_s = 30)"
    );
    assert_eq!(
        r.pw.lock().unwrap().captures,
        0,
        "no microphone for Kai's own bind"
    );
    let appended = r.mock.socket(0, |s| s.count("input_audio_buffer.append"));
    assert_eq!(appended, 0);
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    assert_eq!(
        r.pw.lock().unwrap().captures,
        1,
        "the owner's press opens it"
    );
    r.send(Command::Talk(false));
    r.until_mock("the press's commit", |m| {
        m.socket(0, |s| s.count("input_audio_buffer.commit") == 1)
    })
    .await;
    assert!(r.mock.socket(0, |s| s.count("input_audio_buffer.append")) > 0);
    r.quit().await;
}

/// Kai's quit (the production path, `Core::quit`) cancels what runs, and lmgw is told before the
/// link closes.
#[tokio::test(start_paused = true)]
async fn the_quit_cancels_running_jobs_and_tells_lmgw() {
    let r = rig("").await;
    let told = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    r.jobs.set_notifier(std::sync::Arc::new({
        let told = told.clone();
        move |t: rmcp::model::Task| {
            let told = told.clone();
            Some(tokio::spawn(async move {
                sleep(Duration::from_millis(50)).await;
                told.lock().unwrap().push(t.status);
            }))
        }
    }));
    let started = r
        .jobs
        .start(Start {
            tool: "run_command",
            label: Some("kailive".into()),
            by: Caller::Owner,
            routine: false,
            delivery: Delivery::Thread(Some(812)),
        })
        .unwrap();
    let jobs = r.jobs.clone();
    r.quit().await;
    assert!(started.cancel.is_cancelled());
    assert_eq!(jobs.list()[0].status, "cancelled");
    assert_eq!(
        *told.lock().unwrap(),
        [rmcp::model::TaskStatus::Cancelled],
        "told before the quit returned"
    );
}

/// The results said are remembered as long as the table keeps an ended job: one said again later
/// than `jobs.keep_ended_s` is a new one.
#[tokio::test(start_paused = true)]
async fn the_results_said_are_kept_with_the_table() {
    let mut r = rig("[jobs]\nkeep_ended_s = 60\n").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    // The feed says it again soon: once.
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    sleep(Duration::from_secs(10)).await;
    assert_eq!(job_creates(&r, 0).len(), 1);
    sleep(Duration::from_secs(60)).await;
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    r.until_mock("a second create", |m| {
        m.socket(0, |s| {
            s.tagged
                .iter()
                .filter(|t| t.starts_with("kai_job_"))
                .count()
                == 2
        })
    })
    .await;
    r.quit().await;
}

/// `empty_turn`: another window's turn answered it; nothing is spoken, said or asked again.
#[tokio::test(start_paused = true)]
async fn empty_turn_ends_it_silently() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .with(|s| s.refuse_creates = vec!["empty_turn".into()]);
    r.mock.emit(ServerEvent::TaskDone(done(812, 41)));
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    sleep(Duration::from_secs(10)).await;
    assert_eq!(job_creates(&r, 0).len(), 1);
    let m = r.until(|_| true).await;
    assert!(
        !m.voice.notes.iter().any(|n| n.contains("empty_turn")),
        "{:?}",
        m.voice.notes
    );
    r.no_notice("Kai: voice error");
    r.no_notice("Kai: a job ended");
    r.quit().await;
}

/// No session: Kai binds its conversation's thread itself (not taking it over), speaks the result,
/// and the press session closes after its grace.
#[tokio::test(start_paused = true)]
async fn with_no_session_kai_binds_to_speak_it() {
    let mut r = rig("[voice]\ngrace_s = 2\n").await;
    assert_eq!(r.mock.opens(), 0);
    r.feed.fact(FeedFact::TaskDone(done(812, 41)));
    r.until_mock("a bind", |m| m.opens() == 1).await;
    r.mock.socket(0, |s| {
        assert_eq!(s.thread, 812);
        assert!(!s.take_over, "takeover=never (K5)");
    });
    r.until_mock("the create", |m| {
        m.socket(0, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until_mock("the grace close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    assert_eq!(job_creates(&r, 0).len(), 1);
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

/// A result in another thread, or in a thread another client's voice holds, is notified.
#[tokio::test(start_paused = true)]
async fn another_thread_or_a_held_one_is_notified() {
    let mut r = rig("").await;
    // A Kai job (its announce on) whose result entered a thread that is not the conversation.
    r.jobs
        .start(Start {
            tool: "run_command",
            label: Some("kailive".into()),
            by: Caller::Own,
            routine: false,
            delivery: Delivery::Thread(Some(900)),
        })
        .unwrap();
    let id = r.jobs.list()[0].wire.clone();
    r.feed.fact(FeedFact::TaskDone(TaskDone {
        server_label: "kailive".into(),
        task_id: id,
        tool: "kailive__run_command".into(),
        ..done(900, 51)
    }));
    let n = r.notice("Kai: a job ended").await;
    assert_eq!(
        n.body,
        "kailive__run_command completed: it waits in its chat"
    );
    // The conversation's thread, held by another client's voice.
    r.feed.fact(FeedFact::VoiceBound {
        thread_id: 812,
        by: By {
            name: "the phone".into(),
            own: false,
        },
    });
    r.feed.fact(FeedFact::TaskDone(done(812, 52)));
    let n = r.notice("Kai: a job ended").await;
    assert!(n.body.contains("the voice is in use on the phone"), "{n:?}");
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.opens(), 0, "Kai never takes it over for a result");
    r.quit().await;
}

/// Announce off (a job another caller started): a notification, no bind and no create.
#[tokio::test(start_paused = true)]
async fn announce_off_is_a_notification() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.jobs
        .start(Start {
            tool: "run_command",
            label: Some("kailive".into()),
            by: Caller::Owner,
            routine: false,
            delivery: Delivery::Thread(Some(812)),
        })
        .unwrap();
    let id = r.jobs.list()[0].wire.clone();
    r.mock.emit(ServerEvent::TaskDone(TaskDone {
        server_label: "kailive".into(),
        task_id: id,
        tool: "kailive__run_command".into(),
        status: "failed".into(),
        ..done(812, 61)
    }));
    let n = r.notice("Kai: a job ended").await;
    assert_eq!(n.body, "kailive__run_command failed: it waits in its chat");
    sleep(Duration::from_secs(5)).await;
    assert!(job_creates(&r, 0).is_empty());
    r.quit().await;
}

/// Locked: a result that arrives waits, its notification too; after the unlock it is spoken at the
/// first quiet moment (the lock ended the session, so Kai binds again).
#[tokio::test(start_paused = true)]
async fn results_wait_for_the_unlock() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.locked(true).await;
    r.until_mock("the lock's close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    r.feed.fact(FeedFact::TaskDone(done(812, 71)));
    r.jobs
        .start(Start {
            tool: "run_command",
            label: Some("kailive".into()),
            by: Caller::Owner,
            routine: false,
            delivery: Delivery::Thread(Some(812)),
        })
        .unwrap();
    let id = r.jobs.list()[0].wire.clone();
    r.feed.fact(FeedFact::TaskDone(TaskDone {
        server_label: "kailive".into(),
        task_id: id,
        ..done(812, 72)
    }));
    sleep(Duration::from_secs(30)).await;
    assert_eq!(r.mock.opens(), 1, "nothing binds while locked");
    r.no_notice("Kai: a job ended");
    r.locked(false).await;
    let n = r.notice("Kai: a job ended").await;
    assert!(n.body.contains("it waits in its chat"), "{n:?}");
    r.until_mock("a bind after the unlock", |m| m.opens() == 2)
        .await;
    r.until_mock("the create", |m| {
        m.socket(1, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.quit().await;
}

/// A lock while a result waits for its quiet moment: it is not spoken while locked.
#[tokio::test(start_paused = true)]
async fn a_lock_during_the_wait_holds_it() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::TaskDone(done(812, 81)));
    sleep(Duration::from_millis(500)).await;
    r.locked(true).await;
    sleep(Duration::from_secs(30)).await;
    assert!(job_creates(&r, 0).is_empty());
    assert_eq!(r.mock.opens(), 1);
    r.locked(false).await;
    r.until_mock("a bind after the unlock", |m| m.opens() == 2)
        .await;
    r.until_mock("the create", |m| {
        m.socket(1, |s| s.tagged.iter().any(|t| t.starts_with("kai_job_")))
    })
    .await;
    r.quit().await;
}

/// The jobs tools through the core: `jobs` lists, `job_announce` switches, `job_cancel` cancels.
#[tokio::test(start_paused = true)]
async fn the_job_tools_act_on_the_table() {
    use crate::core::calls::CoreCall;
    use crate::host::registry::{Args, Block, Call};
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    let r = rig("").await;
    let started = r.jobs.start(Start {
        tool: "run_command",
        label: Some("kailive".into()),
        by: Caller::Owner,
        routine: false,
        delivery: Delivery::Thread(Some(812)),
    });
    let crate::jobs::Started { id, cancel, .. } = started.unwrap();
    let call = |name: &str, args: serde_json::Value| {
        let (area, spec) = kai_desktop::contract::find(name).unwrap();
        let args = Args::read(spec, args.as_object().cloned()).unwrap();
        let (reply, answer) = oneshot::channel();
        r.send(Command::Tool(Box::new(CoreCall {
            area: area.name,
            call: Call::new(spec, args, Caller::Owner, CancellationToken::new()),
            reply,
        })));
        answer
    };
    let text = |reply: crate::host::registry::Reply| match &reply.blocks[..] {
        [Block::Text(t)] => (t.clone(), reply.error),
        other => panic!("{other:?}"),
    };
    let (t, _) = text(call("jobs", json!({})).await.unwrap());
    assert!(
        t.starts_with(&format!("job {id} (run_command): running since")),
        "{t}"
    );
    assert!(t.ends_with("its result is notified"), "{t}");
    let (t, e) = text(
        call("job_announce", json!({"id": id, "on": true}))
            .await
            .unwrap(),
    );
    assert!(!e && t.contains("Kai speaks job"), "{t}");
    assert!(r.jobs.list()[0].announce);
    let (t, e) = text(call("job_cancel", json!({"id": id})).await.unwrap());
    assert!(!e && t.contains("cancelled"), "{t}");
    assert!(cancel.is_cancelled());
    let (t, e) = text(call("job_cancel", json!({"id": id})).await.unwrap());
    assert!(e && t.contains("already ended (cancelled)"), "{t}");
    let (t, e) = text(call("job_cancel", json!({"id": "zzzz"})).await.unwrap());
    assert!(e && t == "Kai has no job zzzz", "{t}");
    r.quit().await;
}
