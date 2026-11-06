//! Kai's speech without a model (kai-firefox §7.2, §7.3; `crate::voice::speak`) through the core,
//! in paused time, against the mock's text-to-speech (lmgw's WAV framing, read by Kai's own WAV
//! reader) and the fake PipeWire: Kai's own voice, asked once per run, its words in its reply
//! language; one request per sentence; the quiet moment jobs wait for, with its hold-off, so a
//! pause between a turn's tool rounds starts nothing; do not disturb, muted, another client's turn
//! and PipeWire down hold it with the reason told and nothing made; the screen lock drops what
//! comes; the sender's gate checked again when it plays; Kai's reply first and the paragraph heard
//! again after it; the owner's press and speech cutting it, the speech's echo never forming a
//! turn; a failure's words and status, and a notice that cannot be spoken notified instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use super::super::VoiceMode;
use super::*;
use crate::voice::link::Refusal;
use crate::voice::speak::{
    Dropped, Failure, Gate, KaiVoice, Kind, Lang, Said, SpeechVoice, Spoken, Text, Utterance, Words,
};

type Replies = mpsc::UnboundedReceiver<(u64, Said)>;

/// The default `browser.speech_quiet_ms`.
const HOLD: Duration = Duration::from_millis(1500);

fn utterance(
    kind: Kind,
    text: &str,
    tag: u64,
    tx: &mpsc::UnboundedSender<(u64, Said)>,
) -> Utterance {
    Utterance {
        kind,
        text: Text::Page(text.to_owned()),
        voice: None,
        gate: None,
        cancel: CancellationToken::new(),
        tag,
        reply: Some(tx.clone()),
    }
}

/// An announcement in Kai's words (`DE …` in German) whose gate says Firefox's focus.
fn announcement(
    text: &str,
    focused: bool,
    tag: u64,
    tx: &mpsc::UnboundedSender<(u64, Said)>,
) -> Utterance {
    Utterance {
        kind: Kind::Announcement { urgent: false },
        text: Text::Kai(Words::new(text, format!("DE {text}"))),
        gate: Some(Arc::new(move || Ok(focused))),
        ..utterance(Kind::Reading, "", tag, tx)
    }
}

/// A gate the test turns: 0 focused, 1 not focused, 2 too old.
fn turned(state: &Arc<AtomicU8>) -> Gate {
    let s = state.clone();
    Arc::new(move || match s.load(Ordering::SeqCst) {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Dropped::Stale(60)),
    })
}

/// The next answer that is not a [`Said::Waiting`].
async fn said(rx: &mut Replies) -> (u64, Said) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(120), rx.recv())
            .await
            .expect("an answer of the speech")
            .expect("the speech is there");
        if !matches!(got.1, Said::Waiting(_)) {
            return got;
        }
    }
}

/// The next answer, which is a [`Said::Waiting`]: its reason.
async fn waits(rx: &mut Replies) -> String {
    let got = tokio::time::timeout(Duration::from_secs(120), rx.recv())
        .await
        .expect("an answer of the speech")
        .expect("the speech is there");
    match got.1 {
        Said::Waiting(why) => why,
        other => panic!("not waiting: {other:?}"),
    }
}

impl Rig {
    fn say(&self, u: Utterance) {
        self.send(Command::Speak(Box::new(u)));
    }

    fn speeches(&self) -> Vec<String> {
        self.mock
            .with(|s| s.speeches.iter().map(|(_, t)| t.clone()).collect())
    }
}

/// An announcement in Kai's own voice (its conversation thread's), with no session: nothing before
/// the quiet moment's hold-off, then the speech is asked of lmgw, plays, and is heard; Kai's voice
/// is read once for the run; its words in Kai's reply language, German for German.
#[tokio::test(start_paused = true)]
async fn an_announcement_is_spoken_in_kais_voice_and_words_without_a_model() {
    let r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let t0 = Instant::now();
    r.say(announcement("Loaded: Checkout.", true, 7, &tx));
    r.say(announcement("The page says: saved", true, 8, &tx));
    assert_eq!(said(&mut rx).await, (7, Said::Started));
    assert!(
        t0.elapsed() >= HOLD,
        "after the hold-off: {:?}",
        t0.elapsed()
    );
    assert_eq!(said(&mut rx).await, (7, Said::Done(Spoken::Heard)));
    assert_eq!(said(&mut rx).await, (8, Said::Started));
    assert_eq!(said(&mut rx).await, (8, Said::Done(Spoken::Heard)));
    assert!(r.played() > 0, "the fake PipeWire played it");
    r.mock.with(|s| {
        assert_eq!(s.voices_read, [812], "Kai's voice, once for the run");
        let kai = SpeechVoice {
            model: "kai-tts".into(),
            voice: Some("alba".into()),
            ..Default::default()
        };
        assert_eq!(
            s.speeches,
            [
                (kai.clone(), "Loaded: Checkout.".to_owned()),
                (kai, "The page says: saved".to_owned()),
            ]
        );
        assert!(s.sockets.is_empty(), "no session, no model turn");
    });
    // The next run asks for the voice again (a voice changed on the dashboard is heard), and
    // says Kai's words in its reply language.
    r.mock.with(|s| {
        s.kai_voice = Ok(KaiVoice {
            voice: SpeechVoice {
                model: "other-tts".into(),
                ..Default::default()
            },
            lang: Lang::De,
        })
    });
    r.say(Utterance {
        kind: Kind::Notice,
        text: Text::Kai(Words::new("Reading stopped.", "Vorlesen beendet.")),
        ..utterance(Kind::Notice, "", 9, &tx)
    });
    assert_eq!(said(&mut rx).await, (9, Said::Started));
    r.mock.with(|s| {
        assert_eq!(s.voices_read, [812, 812]);
        let (v, text) = s.speeches.last().unwrap();
        assert_eq!(
            (v.model.as_str(), text.as_str()),
            ("other-tts", "Vorlesen beendet.")
        );
    });
    r.quit().await;
}

/// A paragraph is made one sentence per request (`browser.speech_timeout_s` bounds each), and
/// plays as one, from its first sentence's start to its last's end.
#[tokio::test(start_paused = true)]
async fn a_paragraph_is_made_sentence_by_sentence() {
    let r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    r.say(utterance(
        Kind::Reading,
        "We left at dawn. Then north!\nThe ridge was cold.",
        1,
        &tx,
    ));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    assert_eq!(said(&mut rx).await, (1, Said::Done(Spoken::Heard)));
    assert_eq!(
        r.speeches(),
        ["We left at dawn.", "Then north!", "The ridge was cold."]
    );
    r.quit().await;
}

/// §7.2: an announcement whose Firefox lacks the focused window is not made, unless conversation
/// mode is on; a reading's paragraph has no such gate. The gate runs again when it plays: one made
/// while Firefox had the focus is dropped unheard when it lost it by its turn, and one that grew
/// too old meanwhile is dropped without being made.
#[tokio::test(start_paused = true)]
async fn the_gate_is_checked_when_made_and_when_played() {
    let mut r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    r.say(announcement("The page says: hi", false, 1, &tx));
    assert_eq!(
        said(&mut rx).await,
        (1, Said::Done(Spoken::Dropped(Dropped::NotFocused)))
    );
    r.say(utterance(Kind::Reading, "A paragraph.", 2, &tx));
    assert_eq!(said(&mut rx).await, (2, Said::Started));
    assert_eq!(said(&mut rx).await, (2, Said::Done(Spoken::Heard)));
    assert_eq!(r.speeches(), ["A paragraph."], "nothing made for the first");

    // Behind a long one: made while focused, dropped when Firefox lost the focus by its turn.
    let long = "word ".repeat(100);
    r.say(utterance(Kind::Reading, &long, 3, &tx));
    assert_eq!(said(&mut rx).await, (3, Said::Started));
    let focus = Arc::new(AtomicU8::new(0));
    r.say(Utterance {
        kind: Kind::Announcement { urgent: true },
        text: Text::Kai(Words::new("The page says: made.", "DE")),
        gate: Some(turned(&focus)),
        ..utterance(Kind::Reading, "", 4, &tx)
    });
    r.until_mock("the second made", |m| m.with(|s| s.speeches.len() == 3))
        .await;
    focus.store(1, Ordering::SeqCst);
    assert_eq!(said(&mut rx).await, (3, Said::Done(Spoken::Heard)));
    assert_eq!(
        said(&mut rx).await,
        (4, Said::Done(Spoken::Dropped(Dropped::NotFocused))),
        "made, never played"
    );

    // While Kai's reply plays it waits, unmade; too old by its turn, it is dropped unmade. The
    // positive control behind it plays.
    r.listening_on().await;
    r.mock.respond("Es ist halb neun.", 1500);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    let age = Arc::new(AtomicU8::new(0));
    r.say(Utterance {
        gate: Some(turned(&age)),
        ..announcement("The page says: old.", true, 5, &tx)
    });
    r.say(announcement("The page says: fresh.", true, 6, &tx));
    sleep(Duration::from_millis(500)).await;
    age.store(2, Ordering::SeqCst);
    assert_eq!(
        said(&mut rx).await,
        (5, Said::Done(Spoken::Dropped(Dropped::Stale(60))))
    );
    assert_eq!(said(&mut rx).await, (6, Said::Started));
    assert!(
        !r.speeches().iter().any(|t| t.contains("old")),
        "{:?}",
        r.speeches()
    );
    r.quit().await;
}

/// Never over Kai's reply: what comes while it plays waits for its end and the hold-off; a reply
/// that begins while Kai speaks by itself takes the playback, and the paragraph is heard again
/// after it, whole, made once.
#[tokio::test(start_paused = true)]
async fn kais_reply_goes_first_and_the_paragraph_is_heard_again() {
    let mut r = rig("").await;
    r.listening_on().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let t0 = Instant::now();
    r.mock.respond("Es ist halb neun.", 1500);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.say(utterance(Kind::Reading, "We left at dawn.", 1, &tx));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    assert!(
        t0.elapsed() >= Duration::from_millis(1500) + HOLD,
        "after the reply's audio and the hold-off: {:?}",
        t0.elapsed()
    );
    // 16 characters at 20 ms: 320 ms of speech. Kai's reply comes in its middle.
    sleep(Duration::from_millis(100)).await;
    r.mock.respond("Noch etwas.", 500);
    assert_eq!(
        said(&mut rx).await,
        (1, Said::Started),
        "heard again after the reply"
    );
    assert_eq!(said(&mut rx).await, (1, Said::Done(Spoken::Heard)));
    assert_eq!(r.speeches().len(), 1, "made once");
    r.quit().await;
}

/// A pause between a turn's tool rounds is no quiet moment: nothing starts (or is made) in it, so
/// the next round's reply never takes back a paragraph's beginning; after the last round and the
/// hold-off it plays once.
#[tokio::test(start_paused = true)]
async fn a_pause_between_tool_rounds_starts_nothing() {
    let mut r = rig("").await;
    r.listening_on().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    r.mock.respond("Ich schaue nach.", 400);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.say(utterance(Kind::Reading, "We left at dawn.", 1, &tx));
    r.until(|m| m.voice.shown != VoiceShown::Speaking).await;
    // The tool round: 600 ms of quiet, then the next round's reply.
    sleep(Duration::from_millis(600)).await;
    assert!(r.speeches().is_empty(), "nothing made in the pause");
    r.mock.respond("Gefunden.", 400);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    let replied = Instant::now();
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    assert!(replied.elapsed() >= HOLD, "{:?}", replied.elapsed());
    assert_eq!(said(&mut rx).await, (1, Said::Done(Spoken::Heard)));
    assert_eq!(r.speeches().len(), 1);
    r.quit().await;
}

/// The quiet moment is the jobs' (`jobs_quiet`): do not disturb, muted and another client's turn
/// in Kai's conversation (K16) each hold it, with the reason told and nothing made, and it plays
/// once the gate lifts.
#[tokio::test(start_paused = true)]
async fn do_not_disturb_muted_and_another_turn_hold_it() {
    let r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    r.send(Command::Dnd(true));
    sleep(Duration::from_millis(1)).await;
    r.say(utterance(Kind::Reading, "One.", 1, &tx));
    assert_eq!(waits(&mut rx).await, "do not disturb is on");
    sleep(Duration::from_secs(5)).await;
    assert!(r.speeches().is_empty(), "nothing made");
    r.send(Command::Dnd(false));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    assert_eq!(said(&mut rx).await, (1, Said::Done(Spoken::Heard)));

    r.send(Command::ToggleMute);
    sleep(Duration::from_millis(1)).await;
    r.say(utterance(Kind::Reading, "Two.", 2, &tx));
    assert_eq!(waits(&mut rx).await, "Kai is muted");
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.speeches().len(), 1, "nothing made while muted");
    r.send(Command::ToggleMute);
    assert_eq!(said(&mut rx).await, (2, Said::Started));
    assert_eq!(said(&mut rx).await, (2, Said::Done(Spoken::Heard)));

    let by = By {
        name: "the dashboard".into(),
        own: false,
        sidebar: false,
    };
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 812,
        by,
        voice: false,
    });
    sleep(Duration::from_millis(1)).await;
    r.say(utterance(Kind::Reading, "Three.", 3, &tx));
    assert_eq!(
        waits(&mut rx).await,
        "another client's turn runs in Kai's conversation"
    );
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.speeches().len(), 2, "nothing made during the other turn");
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    assert_eq!(said(&mut rx).await, (3, Said::Started));
    r.quit().await;
}

/// PipeWire down: nothing is made, the reason is told, and a notice is notified instead of
/// waiting; back up, what waits plays.
#[tokio::test(start_paused = true)]
async fn nothing_is_made_while_pipewire_is_down() {
    let mut r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    r.audio_tx
        .send(AudioEvent::Link(Some("PipeWire is not reachable".into())))
        .unwrap();
    sleep(Duration::from_millis(1)).await;
    r.say(utterance(Kind::Reading, "One.", 1, &tx));
    assert_eq!(waits(&mut rx).await, "PipeWire is not reachable");
    r.say(Utterance {
        kind: Kind::Notice,
        text: Text::Kai(Words::new("Reading paused: speech failed.", "DE")),
        ..utterance(Kind::Notice, "", 2, &tx)
    });
    assert_eq!(said(&mut rx).await, (2, Said::Done(Spoken::Notified)));
    let n = r.notice("Kai").await;
    assert!(
        n.body.starts_with("Reading paused: speech failed."),
        "{n:?}"
    );
    sleep(Duration::from_secs(5)).await;
    assert!(r.speeches().is_empty(), "nothing made");
    r.audio_tx.send(AudioEvent::Link(None)).unwrap();
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    r.quit().await;
}

/// The owner talking cuts it: a talk press stops it and is spent (no session, K6), the reading's
/// paragraph waiting behind is cut too.
#[tokio::test(start_paused = true)]
async fn the_owners_press_cuts_it_and_is_spent() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\n").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let long = "word ".repeat(100);
    r.say(announcement(&long, true, 1, &tx));
    r.say(utterance(Kind::Reading, "Then north.", 2, &tx));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    r.send(Command::Talk(true));
    let mut got = vec![said(&mut rx).await, said(&mut rx).await];
    got.sort_by_key(|(t, _)| *t);
    assert_eq!(
        got,
        [
            (
                1,
                Said::Done(Spoken::Cut("you pressed the talk key".into()))
            ),
            (
                2,
                Said::Done(Spoken::Cut("you pressed the talk key".into()))
            ),
        ]
    );
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(500)).await;
    assert_eq!(r.mock.opens(), 0, "the press was spent on the cut");
    r.until(|m| m.voice.mode == VoiceMode::PushToTalk).await;
    r.quit().await;
}

/// Its echo never becomes the owner's turn (Kai's design WP24): in continuous listening, speech
/// heard while it plays cuts it and is spent on the cut, the input buffer cleared, and the session
/// hears silence for `browser.speech_echo_guard_ms` after it stops, so neither what the
/// microphone carried nor the echo's tail forms a turn. The positive control: speech heard with
/// nothing of Kai's own playing or just played is the owner's turn as ever, nothing cleared.
#[tokio::test(start_paused = true)]
async fn its_echo_never_forms_a_turn() {
    let mut r = rig("").await;
    r.listening_on().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let clears = |r: &Rig| r.mock.socket(0, |s| s.count("input_audio_buffer.clear"));

    // The positive control first: no speech of Kai's own anywhere near.
    r.mock.speech_started();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(clears(&r), 0, "the owner's own turn is kept");
    r.mock.emit(ServerEvent::SpeechStopped);
    r.mock.emit(ServerEvent::Committed);
    r.mock.respond("Ja.", 300);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;

    // While it plays, the microphone reaches lmgw (a barge-in by voice works).
    let long = "word ".repeat(100);
    r.say(utterance(Kind::Reading, &long, 1, &tx));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    sleep(Duration::from_millis(200)).await;
    let (sent, silent) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(200)).await;
    let (sent2, silent2) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(sent2 > sent && silent2 == silent, "the microphone is live");

    r.mock.speech_started();
    assert_eq!(
        said(&mut rx).await,
        (1, Said::Done(Spoken::Cut("you spoke".into())))
    );
    r.until_mock("the clear", |m| {
        m.socket(0, |s| s.count("input_audio_buffer.clear") == 1)
    })
    .await;
    // The guard: silence for 500 ms after the cut, then the microphone again.
    let (a0, s0) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(400)).await;
    let (a1, s1) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a1 > a0 && s1 - s0 == a1 - a0, "silence within the guard");
    sleep(Duration::from_millis(400)).await;
    let (a2, s2) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a2 - a1 > s2 - s1, "the microphone after it");
    r.until(|m| m.voice.shown != VoiceShown::Listening).await;

    // Speech heard long after it stopped is the owner's own: nothing cleared.
    sleep(Duration::from_secs(2)).await;
    r.mock.speech_started();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(clears(&r), 1);
    r.quit().await;
}

/// A speech lmgw cannot make ends that utterance with lmgw's words and status (a 4xx is a
/// refusal of that text alone), and the next one is made; a notice whose speech fails is notified.
#[tokio::test(start_paused = true)]
async fn a_failed_speech_ends_with_its_words_and_a_notice_is_notified() {
    let mut r = rig("").await;
    r.mock.with(|s| {
        s.speech_errors = vec![
            Refusal {
                status: 503,
                code: Some("gpu_hold".into()),
                message: "the GPU is held".into(),
            },
            Refusal {
                status: 422,
                code: Some("empty_input".into()),
                message: "input is empty".into(),
            },
        ]
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    r.say(utterance(Kind::Reading, "One.", 1, &tx));
    r.say(utterance(Kind::Reading, "Two.", 2, &tx));
    r.say(utterance(Kind::Reading, "Three.", 3, &tx));
    let (t, s) = said(&mut rx).await;
    assert_eq!(t, 1);
    let Said::Done(Spoken::Failed(Failure { status, why })) = s else {
        panic!("{s:?}");
    };
    assert_eq!(status, Some(503));
    assert!(why.contains("the GPU is held (gpu_hold)"), "{why}");
    let (t, s) = said(&mut rx).await;
    let Said::Done(Spoken::Failed(f)) = s else {
        panic!("{s:?}");
    };
    assert!(t == 2 && f.refused(), "{f:?}");
    assert_eq!(said(&mut rx).await, (3, Said::Started));
    assert_eq!(said(&mut rx).await, (3, Said::Done(Spoken::Heard)));
    r.mock.with(|s| {
        s.speech_errors = vec![Refusal::no_answer(
            "lmgw did not make the speech within 60 s (browser.speech_timeout_s)",
        )]
    });
    r.say(Utterance {
        kind: Kind::Notice,
        text: Text::Kai(Words::new("Reading paused: speech failed.", "DE")),
        ..utterance(Kind::Notice, "", 4, &tx)
    });
    let (t, s) = said(&mut rx).await;
    assert!(
        t == 4 && matches!(s, Said::Done(Spoken::Failed(_))),
        "{s:?}"
    );
    let n = r.notice("Kai").await;
    assert!(n.body.contains("Kai could not speak it"), "{n:?}");
    r.quit().await;
}

/// The screen lock (K32): what plays stops, a reading is cut, an announcement dropped; what comes
/// while locked is dropped at once, nothing made.
#[tokio::test(start_paused = true)]
async fn the_screen_lock_stops_it_and_drops_what_comes() {
    let mut r = rig("").await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let long = "word ".repeat(100);
    r.say(utterance(Kind::Reading, &long, 1, &tx));
    assert_eq!(said(&mut rx).await, (1, Said::Started));
    r.say(announcement("Loaded.", true, 2, &tx));
    r.screen.set(true);
    r.send(Command::ScreenLock);
    r.until(|m| m.voice.locked).await;
    let mut got = vec![said(&mut rx).await, said(&mut rx).await];
    got.sort_by_key(|(t, _)| *t);
    assert_eq!(
        got,
        [
            (1, Said::Done(Spoken::Cut("the screen locked".into()))),
            (2, Said::Done(Spoken::Dropped(Dropped::Locked))),
        ]
    );
    let made = r.speeches().len();
    r.say(announcement("The page says: while locked", true, 3, &tx));
    assert_eq!(
        said(&mut rx).await,
        (3, Said::Done(Spoken::Dropped(Dropped::Locked)))
    );
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.speeches().len(), made, "nothing made while locked");
    r.quit().await;
}
