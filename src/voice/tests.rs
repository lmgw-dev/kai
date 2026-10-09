//! WP4's acceptance items that need no real lmgw (§15), against the scripted server of
//! [`super::mock`], through the core: the commands come in as the tray and the shortcuts send
//! them, the tray model and the ring's input are read as the tray and the overlay get them. A fake
//! PipeWire plays and records in tokio's paused time, and reports its time on that clock too: a
//! microphone that always hears a constant signal, and a playback stream that reports 300 ms of
//! delay, far more than one 10 ms cycle, so a truncate at what was played instead of what was
//! heard fails every barge-in test.
//!
//! The real-lmgw versions (TTS-driven round trips, a dashboard bind, a dashboard text turn, a dev
//! lmgw restart) run once the `lmgw-client` adapter exists.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;

use super::link::{ServerEvent, TurnDetection};
use super::mock::{MockLink, heard_text};
use super::{By, FeedFact, Notice};
use crate::audio::test_support::{self, FakeHead, FakeMic};
use crate::audio::{AudioEvent, AudioPort, Capture, EchoMode, Player};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::{Credentials, FolderEvent, LinkEvent, LmgwLink};
use crate::overlay::{OverlayCmd, RingInput, RingState};
use crate::pairing::DeviceKey;
use crate::pairing::tests::KEY;
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{
    BADGE_HELD, ICON_LISTENING, ICON_MUTED, Link, Pairing, TrayModel, VoiceShown, render,
};
use crate::ui::UiCmd;

/// WP19: no audio while a call waits for an approval, the spoken answer, the card's answer.
mod approvals;
/// The realtime closes a client tells apart (api §1.6), and the feed's `revoked` kinds (§3.3).
mod closes;
/// The conversation's folder and thread through the core (WP6).
mod conversation;
/// Conversation mode (WP9, K27): the tools, the silence timeout, the ways it ends.
mod conversation_mode;
/// WP18: a job's result spoken at a quiet moment, or notified.
mod jobs;
/// WP22: `personality_set` and the tray's Personality radio.
mod personality;
/// Asking for the thread again after a close or a refused bind, and what meets it (review KAI-4).
mod reask;
/// The ring's input against the owner's rule: active without a gap from a press to its turn's end
/// (review KAI-5).
mod ring;
/// The screen lock (the owner's decision, 2026-10-09): Kai stops and stays idle.
mod screen_lock;

/// Frames between the playback head and what the fake device presents (300 ms).
const LATENCY: u64 = 7200;
const LATENCY_MS: u64 = LATENCY * 1000 / 24_000;

/// The truncate's `audio_end_ms` is what was heard: what the sink got less the device's latency,
/// read at most one 10 ms cycle after the report it extrapolates from.
fn assert_heard(audio_end_ms: u64, played_ms: u64) {
    let heard = played_ms - LATENCY_MS;
    assert!(
        audio_end_ms + 1 >= heard && audio_end_ms <= heard + 10,
        "truncated at {audio_end_ms} ms; the sink got {played_ms} ms, so {heard} ms were heard \
         (within one cycle)"
    );
}
/// The microphone's constant sample.
const MIC: i16 = 1200;
const URL: &str = "http://127.0.0.1:1";

#[derive(Default)]
struct Pw {
    heads: Vec<FakeHead>,
    mics: Vec<FakeMic>,
    /// Non-silent samples the playback streams wrote.
    played: u64,
    captures: usize,
}

#[derive(Clone, Default)]
struct FakeAudio(Arc<Mutex<Pw>>);

impl AudioPort for FakeAudio {
    fn set_echo_mode(&self, _: EchoMode) {}
    fn set_input(&self, _: Option<String>) {}
    fn set_output(&self, _: Option<String>) {}
    fn open_playback(&self) -> Player {
        let (p, h) = test_support::playback(2000);
        self.0.lock().unwrap().heads.push(h);
        p
    }
    fn open_capture(&self) -> Capture {
        let (c, m) = test_support::capture(2000);
        let mut pw = self.0.lock().unwrap();
        pw.mics.push(m);
        pw.captures += 1;
        c
    }
    async fn shutdown(&mut self, _: Duration) {}
}

/// PipeWire's cycles: 10 ms each, the microphone and every playback stream.
async fn pipewire(pw: Arc<Mutex<Pw>>) {
    let mut iv = tokio::time::interval(Duration::from_millis(10));
    loop {
        iv.tick().await;
        let mut pw = pw.lock().unwrap();
        let mut played = 0;
        for h in &mut pw.heads {
            played += h.cycle(240, LATENCY).iter().filter(|s| **s != 0).count() as u64;
        }
        pw.played += played;
        for m in &mut pw.mics {
            m.take(&[MIC; 240]);
        }
    }
}

/// The feed, as far as the voice needs it.
#[derive(Clone, Default)]
pub(super) struct Feed(Arc<Mutex<Option<mpsc::UnboundedSender<LinkEvent>>>>);

impl LmgwLink for Feed {
    fn start(&mut self, _: Credentials, events: mpsc::UnboundedSender<LinkEvent>) {
        *self.0.lock().unwrap() = Some(events);
    }
    fn stop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

impl Feed {
    pub(super) fn raise(&self, e: LinkEvent) {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .expect("feed")
            .send(e)
            .unwrap();
    }
    fn fact(&self, f: FeedFact) {
        self.raise(LinkEvent::Fact(f));
    }
    /// `folder.current` for the rig's folder (3): the conversation moved to `thread`.
    fn moved(&self, thread: i64) {
        self.raise(LinkEvent::Folder(FolderEvent::Current {
            folder_id: 3,
            thread_id: Some(thread),
            previous: None,
            reason: "requested".into(),
        }));
    }
}

struct Rig {
    mock: MockLink,
    feed: Feed,
    pw: Arc<Mutex<Pw>>,
    model: watch::Receiver<TrayModel>,
    cmds: mpsc::UnboundedSender<Command>,
    ui: async_channel::Receiver<UiCmd>,
    notices: mpsc::UnboundedReceiver<Notice>,
    /// Open in lmgw's URLs.
    opened: mpsc::UnboundedReceiver<String>,
    audio_tx: mpsc::UnboundedSender<AudioEvent>,
    config_file: std::path::PathBuf,
    /// Moves the voice's wall clock alone, in seconds (a suspend).
    wall_skew: Arc<std::sync::atomic::AtomicU64>,
    /// The screen lock's shared flag (its watch's part in the tests).
    screen: crate::screen_lock::ScreenLock,
    /// The job table the core and the voice read (WP18; the host's in Kai).
    jobs: crate::jobs::Jobs,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<crate::core::Ended>,
    pipewire: tokio::task::JoinHandle<()>,
}

/// A paired Kai whose feed is up, with `config` added to the configuration. Unless `config` has a
/// `[conversation]` section, its folder (the mock's "Kai", 3) is remembered. When the mock's folder
/// is ready, the rig waits until the start's `current` answered (with `voice.on_at_start` the
/// session's own does; the rig does not wait for it then).
async fn rig(config: &str) -> Rig {
    rig_with(config, MockLink::default()).await
}

async fn rig_with(config: &str, mock: MockLink) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        if config.contains("[conversation]") {
            format!("[lmgw]\nurl = \"{URL}\"\n{config}")
        } else {
            format!("[lmgw]\nurl = \"{URL}\"\n[conversation]\nfolder_id = 3\n{config}")
        },
    )
    .unwrap();
    let store = Arc::new(MemoryStore::default());
    store
        .items
        .lock()
        .unwrap()
        .insert(URL.into(), ("d".into(), DeviceKey::parse(KEY).unwrap()));
    let feed = Feed::default();
    let audio = FakeAudio::default();
    let pw = audio.0.clone();
    let (audio_tx, audio_rx) = mpsc::unbounded_channel::<AudioEvent>();
    let (model_tx, model) = watch::channel(TrayModel::starting(false));
    let (ui_tx, ui) = async_channel::unbounded();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let (ntx, notices) = mpsc::unbounded_channel();
    let (otx, opened) = mpsc::unbounded_channel();
    let config_file = paths.config_file.clone();
    let mut core = Core::new(
        Config::load(&paths.config_file).unwrap(),
        paths,
        "/usr/bin/kai".into(),
        store,
        feed.clone(),
        mock.clone(),
        (audio, audio_rx),
        model_tx,
        ui_tx,
        None,
    );
    core.notify_into(ntx);
    core.open_into(otx);
    let jobs = crate::jobs::Jobs::default();
    core.set_jobs(jobs.clone());
    let wall_skew = core.wall_skew();
    let screen = core.screen_lock();
    let task = tokio::spawn(core.run(cmd_rx));
    let pipewire = tokio::spawn(pipewire(pw.clone()));
    let ready_folder = mock.with(|s| {
        s.folders
            .iter()
            .any(|f| f.readiness() == crate::conversation::Readiness::Ready)
    });
    let mut r = Rig {
        mock,
        feed,
        pw,
        model,
        cmds,
        ui,
        notices,
        opened,
        audio_tx,
        config_file,
        wall_skew,
        screen,
        jobs,
        _dir: dir,
        task,
        pipewire,
    };
    r.until(|m| matches!(m.pairing, Pairing::Paired { .. }))
        .await;
    r.feed.raise(LinkEvent::Connected {
        device: "desktop".into(),
        held: false,
        hosts_label: None,
        admin: Default::default(),
    });
    r.until(|m| {
        matches!(
            m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        )
    })
    .await;
    if ready_folder && !config.contains("on_at_start = true") {
        r.until(|m| m.voice.thread.is_some()).await;
    }
    r
}

impl Rig {
    fn send(&self, c: Command) {
        self.cmds.send(c).unwrap();
    }

    async fn until(&mut self, f: impl FnMut(&TrayModel) -> bool) -> TrayModel {
        tokio::time::timeout(Duration::from_secs(60), self.model.wait_for(f))
            .await
            .expect("the tray model to get there")
            .unwrap()
            .clone()
    }

    /// Waits (in paused time) until `f` holds of the mock.
    async fn until_mock(&self, what: &str, f: impl Fn(&MockLink) -> bool) {
        for _ in 0..6000 {
            if f(&self.mock) {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("the mock never got to: {what}");
    }

    async fn created(&mut self, n: usize) {
        self.until_mock("a session", |m| m.opens() > n).await;
        self.until_mock("session.update", |m| {
            m.socket(n, |s| s.count("session.update") > 0)
        })
        .await;
    }

    async fn notice(&mut self, summary: &str) -> Notice {
        loop {
            let n = tokio::time::timeout(Duration::from_secs(60), self.notices.recv())
                .await
                .unwrap_or_else(|_| panic!("no notification '{summary}'"))
                .unwrap();
            if n.summary == summary {
                return n;
            }
        }
    }

    fn no_notice(&mut self, summary: &str) {
        while let Ok(n) = self.notices.try_recv() {
            assert_ne!(n.summary, summary, "{n:?}");
        }
    }

    /// What the ring was told since the last call.
    fn rings(&self) -> Vec<RingInput> {
        let mut v = Vec::new();
        while let Ok(c) = self.ui.try_recv() {
            if let UiCmd::Overlay(OverlayCmd::Ring(i)) = c {
                v.push(i);
            }
        }
        v
    }

    fn played(&self) -> u64 {
        self.pw.lock().unwrap().played
    }

    async fn listening_on(&mut self) {
        self.send(Command::ToggleListening);
        self.created(0).await;
        self.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    }

    async fn quit(self) -> MockLink {
        self.send(Command::Quit);
        self.task.await.unwrap();
        self.pipewire.abort();
        self.mock
    }
}

// -------------------------------------------------------------------------- round trips

#[tokio::test(start_paused = true)]
async fn push_to_talk_round_trip_with_listening_off() {
    let mut r = rig("[voice]\ngrace_s = 2\n").await;
    assert_eq!(r.mock.opens(), 0, "K4: voice is off at login");
    r.send(Command::Talk(true));
    // Held for 600 ms; the session is created 200 ms after the open: the first part is buffered.
    let m = r.until(|m| m.voice.shown == VoiceShown::Listening).await;
    assert_eq!(render(&m).icon, ICON_LISTENING);
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until_mock("commit and response.create", |m| {
        m.opens() == 1 && m.socket(0, |s| s.count("response.create") == 1)
    })
    .await;
    r.mock.socket(0, |s| {
        assert_eq!(s.thread, 812);
        // §5.3: session.update first, then the buffer in order, then the commit and the ask.
        assert_eq!(s.kinds[0], "session.update");
        assert_eq!(
            s.detections(),
            vec![None],
            "a hold with listening off: manual turns"
        );
        let commit = s
            .kinds
            .iter()
            .position(|k| *k == "input_audio_buffer.commit");
        let ask = s.kinds.iter().position(|k| *k == "response.create");
        let last_append = s
            .kinds
            .iter()
            .rposition(|k| *k == "input_audio_buffer.append");
        assert!(last_append < commit && commit < ask, "{:?}", s.kinds);
        // 600 ms held: all of it, the last moment before the release included, none of it lost
        // while the session was coming up (the fake microphone delivers 10 ms per cycle).
        let ms = s.appended as u64 * 1000 / 24_000;
        assert!((590..=610).contains(&ms), "{ms} ms appended");
        assert_eq!(s.silent, 0);
    });
    // The reply plays, then the session is quiet.
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    let m = r
        .until(|m| m.voice.shown == VoiceShown::Off && m.voice.timing.is_some())
        .await;
    assert!(r.played() >= 1500 * 24, "the reply was played");
    assert_eq!(
        m.voice.timing.as_deref(),
        Some(
            "first audio 800 ms (turn end 400, ASR 30, first token 200, clause 100, audio 40), \
             chat mock/chat"
        )
    );
    assert_eq!(render(&m).title, "Kai: Kai");
    let rings = r.rings();
    assert!(
        rings
            .iter()
            .any(|i| i.bound && i.turn && i.state == RingState::Listening)
    );
    assert!(
        rings
            .iter()
            .any(|i| i.turn && i.state == RingState::Thinking)
    );
    assert!(
        rings
            .iter()
            .any(|i| i.turn && i.state == RingState::Speaking)
    );
    let last = rings.last().unwrap();
    assert!(last.bound && !last.turn, "after the reply: bound, no turn");
    assert!(
        rings.iter().all(|i| i.active == i.turn),
        "a press session is active through its turn and in standby after it: {rings:?}"
    );
    // K5: the press session closes voice.grace_s after its reply has played.
    assert!(!r.mock.socket(0, |s| s.closed_by_client));
    sleep(Duration::from_millis(2200)).await;
    assert!(r.mock.socket(0, |s| s.closed_by_client), "the grace close");
    assert!(
        !r.rings().last().unwrap().bound,
        "the ring's view goes with it"
    );
    assert_eq!(r.pw.lock().unwrap().captures, 1);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn automatic_round_trip() {
    let mut r = rig("").await;
    r.listening_on().await;
    // The thread resolves server_vad; the session runs it.
    r.mock.socket(0, |s| {
        assert_eq!(s.detections(), vec![Some(TurnDetection::ServerVad)])
    });
    let ring = r.rings();
    assert!(
        ring.last().is_some_and(|i| i.bound && i.active && !i.turn),
        "quiet continuous listening is active, so the ring shows listening (K11): {ring:?}"
    );
    // The microphone streams.
    sleep(Duration::from_millis(500)).await;
    let (appended, silent) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(appended >= 400 * 24, "{appended}");
    assert_eq!(silent, 0);
    r.mock.speak();
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    let m = r
        .until(|m| m.voice.shown == VoiceShown::Quiet && m.voice.timing.is_some())
        .await;
    assert!(m.voice.on);
    assert_eq!(render(&m).icon, ICON_LISTENING);
    // The ring's input is queued, so every step shows (the tray's watch keeps only the latest).
    let rings = r.rings();
    let turn: Vec<RingState> = rings.iter().filter(|i| i.turn).map(|i| i.state).collect();
    assert_eq!(
        turn.first(),
        Some(&RingState::Listening),
        "speech_started starts the turn: {rings:?}"
    );
    assert!(turn.contains(&RingState::Speaking));
    assert!(
        rings.iter().all(|i| i.active),
        "continuous listening is active through the turn: {rings:?}"
    );
    assert!(
        rings.last().is_some_and(|i| i.bound && i.active && !i.turn),
        "the turn's end goes back to listening"
    );
    // Listening off: the session closes, the microphone is released.
    r.send(Command::SetListening(false));
    r.until(|m| !m.voice.on && m.voice.shown == VoiceShown::Off)
        .await;
    r.until_mock("the close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    let last = r.rings().last().unwrap().clone();
    assert!(!last.bound && !last.active);
    r.quit().await;
}

/// A tool turn is one audio item (chat-voice §8.5): its spoken preamble speaks, and once the
/// listener heard all of it while the tool runs, the voice is silent and the state is `thinking`
/// (`lmgw-client`'s machine: a `tool` frame's `start`, and the reply starved in Kai's player)
/// until the answer's audio comes.
#[tokio::test(start_paused = true)]
async fn a_tool_running_after_its_spoken_preamble_shows_thinking() {
    let mut r = rig("").await;
    r.listening_on().await;
    let delta = |ms: u64| ServerEvent::AudioDelta {
        response_id: "resp_t".into(),
        item_id: "item_t".into(),
        pcm: lmgw_client::base64::pcm16_le_bytes(&vec![3000i16; (ms * 24) as usize]),
    };
    let frame = |event: &str| ServerEvent::ChatFrame {
        response_id: "resp_t".into(),
        event: "tool".into(),
        data: json!({"event": event, "name": "kai__weather"}),
    };
    r.mock.emit(ServerEvent::ResponseCreated {
        response_id: "resp_t".into(),
    });
    r.mock.emit(delta(300));
    r.mock.emit(frame("start"));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    // The preamble heard to its end (300 ms of audio behind 300 ms of latency), the tool still
    // running: thinking.
    r.until(|m| m.voice.shown == VoiceShown::Thinking).await;
    r.mock.emit(frame("result"));
    r.mock.emit(delta(400));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.mock.emit(ServerEvent::AudioDone {
        response_id: "resp_t".into(),
    });
    r.mock.emit(ServerEvent::ResponseDone {
        response_id: "resp_t".into(),
        status: "completed".into(),
        reason: None,
    });
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_thread_that_resolves_push_to_talk_listens_with_semantic_vad() {
    let mock = MockLink::default();
    mock.with(|s| s.resolved = None);
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.mock.socket(0, |s| {
        assert_eq!(s.detections(), vec![Some(TurnDetection::SemanticVad)])
    });
    // Push-to-talk mode, from the tray: null, and nothing flows until the key.
    r.send(Command::SetVoiceMode(super::VoiceMode::PushToTalk));
    r.until_mock("the update", |m| m.socket(0, |s| s.detections().len() == 2))
        .await;
    r.mock.socket(0, |s| assert_eq!(s.detections()[1], None));
    let before = r.mock.socket(0, |s| s.appended);
    sleep(Duration::from_millis(500)).await;
    assert_eq!(
        r.mock.socket(0, |s| s.appended),
        before,
        "nothing without the key"
    );
    let m = r
        .until(|m| m.voice.mode == super::VoiceMode::PushToTalk)
        .await;
    assert_eq!(render(&m).status, "listening on: hold the talk key to talk");
    // A hold: clear, the pre-roll (300 ms, the session's prefix_padding_ms), live audio, commit.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    r.until_mock("the commit", |m| {
        m.socket(0, |s| s.count("response.create") == 1)
    })
    .await;
    r.mock.socket(0, |s| {
        let clear = s
            .kinds
            .iter()
            .position(|k| *k == "input_audio_buffer.clear");
        let commit = s
            .kinds
            .iter()
            .position(|k| *k == "input_audio_buffer.commit");
        assert!(clear.is_some() && clear < commit, "{:?}", s.kinds);
        let ms = (s.appended - before) as u64 * 1000 / 24_000;
        assert!((760..=860).contains(&ms), "pre-roll + hold: {ms} ms");
    });
    assert!(
        std::fs::read_to_string(r._dir.path().join("kai/config.toml"))
            .unwrap()
            .contains("mode = \"push_to_talk\""),
        "the tray's choice is saved as voice.mode"
    );
    r.quit().await;
}

/// The owner's decision, 2026-10-08: with push-to-talk Kai listens only while the talk key is
/// held, so the ring shows from a press through its turn's end and nothing between presses, even
/// with listening on (the tray's listening icon still says listening is on).
#[tokio::test(start_paused = true)]
async fn push_to_talk_shows_the_ring_only_from_a_press_to_its_turns_end() {
    let mut r = rig("[voice]\nmode = \"push_to_talk\"\n").await;
    r.listening_on().await;
    let rings = r.rings();
    assert!(
        rings
            .last()
            .is_some_and(|i| i.bound && !i.turn && !i.active),
        "listening on in push-to-talk: bound, in standby, nothing shown: {rings:?}"
    );
    for _ in 0..2 {
        r.send(Command::Talk(true));
        sleep(Duration::from_millis(500)).await;
        r.send(Command::Talk(false));
        r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
        r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
        let rings = r.rings();
        assert!(
            rings
                .iter()
                .any(|i| i.turn && i.state == RingState::Listening),
            "the press shows the ring: {rings:?}"
        );
        assert!(
            rings
                .iter()
                .any(|i| i.turn && i.state == RingState::Speaking)
        );
        assert!(
            rings.iter().all(|i| i.active == i.turn),
            "push-to-talk is active from the press through its turn, in standby between presses: \
             {rings:?}"
        );
        assert!(
            rings.last().is_some_and(|i| i.bound && !i.turn),
            "between presses: no turn, so the ring eases out and hides"
        );
    }
    r.quit().await;
}

// -------------------------------------------------------------------------- barge-in

const LONG: &str = "eins zwei drei vier fünf sechs sieben acht neun zehn elf zwölf dreizehn";

#[tokio::test(start_paused = true)]
async fn a_barge_in_leaves_the_heard_text() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.respond(LONG, 3000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    sleep(Duration::from_millis(1000)).await;
    r.mock.speech_started();
    r.until_mock("the truncate", |m| m.socket(0, |s| !s.truncates.is_empty()))
        .await;
    let (item, ms, sent) = r.mock.socket(0, |s| s.truncates[0].clone());
    assert_eq!(item, "item_a1");
    // The flush stopped the sink: what it got is what was played.
    assert_heard(ms, r.played() * 1000 / 24_000);
    assert!(sent > ms + 200, "more was sent ({sent} ms) than heard");
    r.mock.socket(0, |s| {
        assert_eq!(
            s.cancels(),
            0,
            "a barge-in's response the server cancels itself"
        );
        assert_eq!(s.refused_truncates, 0);
    });
    // The thread keeps exactly what was heard.
    let stored = r.mock.stored(0)["resp_1"].clone().unwrap();
    assert_eq!(stored, heard_text(LONG, 3000, ms));
    assert!(!stored.is_empty() && LONG.starts_with(&stored) && stored.len() < LONG.len());
    // The voice stopped: nothing more is played.
    sleep(Duration::from_millis(100)).await;
    let played = r.played();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(r.played(), played, "the flush stopped the audio");
    let m = r.until(|m| m.voice.shown == VoiceShown::Listening).await;
    assert!(m.voice.on);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_talk_press_over_the_voice_stops_it_and_spends_the_gesture() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.respond(LONG, 3000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    sleep(Duration::from_millis(700)).await;
    r.send(Command::Talk(true));
    r.until_mock("the truncate", |m| m.socket(0, |s| !s.truncates.is_empty()))
        .await;
    sleep(Duration::from_millis(100)).await;
    // A tap meant to interrupt: listening stays on (K6).
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(300)).await;
    let m = r.until(|m| m.voice.shown != VoiceShown::Speaking).await;
    assert!(m.voice.on, "the stopping press consumed the gesture");
    let played_ms = r.played() * 1000 / 24_000;
    r.mock.socket(0, |s| {
        let (_, ms, _) = s.truncates[0];
        assert_heard(ms, played_ms);
        // Truncate first; the truncate of an item still being produced ended the response.
        assert_eq!(s.cancels(), 0);
        let stored = &s.stored.last().unwrap().1;
        assert_eq!(stored.as_deref(), Some(heard_text(LONG, 3000, ms).as_str()));
    });
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_stop_after_the_audio_was_complete_also_cancels() {
    let mock = MockLink::default();
    // The response stays open while its modelled playback drains (chat-voice §8.2).
    mock.with(|s| s.done_lag = Duration::from_secs(2));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    // A short reply: its item is complete 300 ms in (the lead), and it still plays.
    r.mock.respond("Gern, sofort.", 600);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until_mock("the audio's done", |m| {
        m.socket(0, |s| s.replies[0].audio_done)
    })
    .await;
    sleep(Duration::from_millis(50)).await;
    r.send(Command::Talk(true));
    r.until_mock("the cancel", |m| m.socket(0, |s| s.cancels() == 1))
        .await;
    r.mock.socket(0, |s| {
        let t = s
            .kinds
            .iter()
            .position(|k| *k == "conversation.item.truncate");
        let c = s.kinds.iter().position(|k| *k == "response.cancel");
        assert!(t.is_some() && t < c, "truncate first: {:?}", s.kinds);
    });
    r.send(Command::Talk(false));
    r.quit().await;
}

// -------------------------------------------------------------------------- others in the thread

#[tokio::test(start_paused = true)]
async fn a_takeover_names_who_took_it_and_kai_does_not_reconnect() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.take_over("device 'phone'");
    let n = r.notice("Kai: voice taken over").await;
    assert!(n.body.contains("device 'phone'"), "{n:?}");
    let m = r
        .until(|m| !m.voice.on && m.voice.shown == VoiceShown::Off)
        .await;
    assert!(
        m.voice.notes.iter().any(|l| l.contains("device 'phone'")),
        "{:?}",
        m.voice.notes
    );
    sleep(Duration::from_secs(10)).await;
    assert_eq!(r.mock.opens(), 1, "no reconnect by itself");
    // The next press takes the thread back.
    r.send(Command::Talk(true));
    r.until_mock("a second session", |m| m.opens() == 2).await;
    r.send(Command::Talk(false));
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn another_clients_turn_silences_automatic_listening_until_it_is_done() {
    let mut r = rig("").await;
    r.listening_on().await;
    sleep(Duration::from_millis(300)).await;
    let by = |name: &str, own| By {
        name: name.into(),
        own,
    };
    // Kai's own turn changes nothing.
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 812,
        by: by("device 'desktop'", true),
    });
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 812,
        by: by("the dashboard", false),
    });
    let m = r
        .until(|m| m.voice.notes.iter().any(|n| n.contains("the dashboard")))
        .await;
    assert!(m.voice.on);
    assert!(
        !r.rings().last().unwrap().active,
        "K16's silence puts continuous listening in standby, so the ring eases out"
    );
    sleep(Duration::from_millis(40)).await;
    let (a0, s0) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(500)).await;
    let (a1, s1) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a1 - a0 >= 400 * 24, "the stream goes on");
    assert_eq!(
        s1 - s0,
        a1 - a0,
        "K16: only silence while the dashboard's turn runs"
    );
    // A talk press still wins.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    let (a2, s2) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a2 - a1 - (s2 - s1) >= 300 * 24, "the press's audio is real");
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(100)).await;
    let (a3, s3) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(300)).await;
    let (a4, s4) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert_eq!(s4 - s3, a4 - a3, "silence again after the release");
    r.mock.socket(0, |s| {
        assert_eq!(
            s.count("input_audio_buffer.commit"),
            0,
            "automatic: no commit"
        )
    });
    // Until it is done.
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    r.until(|m| !m.voice.notes.iter().any(|n| n.contains("the dashboard")))
        .await;
    assert!(
        r.rings().last().unwrap().active,
        "active again once the dashboard's turn is done"
    );
    sleep(Duration::from_millis(40)).await;
    let (a5, s5) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(300)).await;
    let (a6, s6) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a6 > a5);
    assert_eq!(s6, s5, "the microphone's audio again");
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn kai_by_itself_never_binds_a_thread_another_clients_voice_holds() {
    let phone = By {
        name: "device 'phone'".into(),
        own: false,
    };
    // voice.on_at_start, and `hello`'s live part says the phone holds the thread (Kai's own entries
    // do not count).
    let mut r = rig("[voice]\non_at_start = true\n").await;
    r.feed.fact(FeedFact::Live {
        voice: vec![
            (812, phone.clone()),
            (
                700,
                By {
                    name: "device 'desktop'".into(),
                    own: true,
                },
            ),
        ],
        turns: vec![],
    });
    let n = r.notice("Kai: voice in use elsewhere").await;
    assert!(n.body.contains("device 'phone'"), "{n:?}");
    let m = r.until(|m| m.voice.shown == VoiceShown::Off).await;
    assert!(!m.voice.on);
    assert!(
        m.voice
            .notes
            .contains(&"voice in use on device 'phone'".to_owned())
    );
    assert_eq!(r.mock.opens(), 0, "K5: Kai stays off");
    // The owner's press takes the thread over (§5.6).
    r.send(Command::ToggleListening);
    r.created(0).await;
    r.mock.socket(0, |s| assert_eq!(s.thread, 812));
    // The phone's session ended with the takeover.
    r.feed.fact(FeedFact::VoiceEnded {
        thread_id: 812,
        by: phone,
    });
    let m = r.until(|m| m.voice.notes.is_empty()).await;
    assert!(m.voice.on);
    // A rollover onto a thread another client's voice holds: Kai turns listening off instead of
    // rebinding, and waits for a press (K5).
    r.feed.fact(FeedFact::VoiceBound {
        thread_id: 813,
        by: By {
            name: "the dashboard".into(),
            own: false,
        },
    });
    r.feed.moved(813);
    let n = r.notice("Kai: voice in use elsewhere").await;
    assert!(n.body.contains("the dashboard"));
    let m = r.until(|m| !m.voice.on).await;
    assert!(
        m.voice
            .notes
            .contains(&"voice in use on the dashboard".to_owned()),
        "{:?}",
        m.voice.notes
    );
    assert!(r.mock.socket(0, |s| s.closed_by_client));
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_rollover_rebinds_at_the_next_quiet_point_and_buffers_between() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.rings();
    r.mock.respond(LONG, 1500);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    // The folder's current thread moves while Kai speaks.
    r.mock.with(|s| {
        s.thread = super::link::ThreadRef {
            id: 900,
            title: "Kai (2)".into(),
        }
    });
    r.feed.moved(900);
    let m = r
        .until(|m| m.voice.notes.iter().any(|n| n.contains("rebinds")))
        .await;
    assert_eq!(m.voice.shown, VoiceShown::Speaking);
    sleep(Duration::from_millis(300)).await;
    assert_eq!(r.mock.opens(), 1, "not while the reply plays");
    r.created(1).await;
    r.mock.socket(0, |s| {
        assert!(s.closed_by_client);
        assert!(
            s.truncates.is_empty(),
            "nothing was cut: the reply had played"
        );
    });
    r.mock.socket(1, |s| {
        assert_eq!(s.thread, 900);
        assert_eq!(s.kinds[0], "session.update");
    });
    sleep(Duration::from_millis(200)).await;
    let m = r
        .until(|m| m.voice.thread.as_deref() == Some("Kai (2)"))
        .await;
    assert!(m.voice.on);
    assert!(
        r.mock.socket(1, |s| s.appended) > 0,
        "audio flows into the new one"
    );
    // The audio between the sessions is buffered and sent: continuous listening stays active
    // throughout, so the ring stays up in between.
    let rings = r.rings();
    assert!(
        rings.iter().any(|i| !i.bound) && rings.iter().all(|i| i.active),
        "{rings:?}"
    );
    // A new conversation from the menu: current {new: true}, then the rebind.
    r.send(Command::NewConversation);
    r.created(2).await;
    // The start's, the session's, the new conversation's.
    assert_eq!(
        r.mock.with(|s| s.current_calls.clone()),
        [false, false, true]
    );
    r.mock.socket(2, |s| assert_eq!(s.thread, 901));
    r.quit().await;
}

/// §3.2: a `resync` may have hidden a `folder.current`, so Kai asks for its thread again; a moved
/// conversation is rebound at the next quiet point, an unmoved one changes nothing.
#[tokio::test(start_paused = true)]
async fn a_resync_asks_for_the_thread_again_and_rebinds_if_it_moved() {
    let mut r = rig("").await;
    r.listening_on().await;
    assert_eq!(r.mock.with(|s| s.current_calls.clone()), [false, false]);
    r.feed.fact(FeedFact::Resync);
    r.until_mock("a third current", |m| {
        m.with(|s| s.current_calls.len()) == 3
    })
    .await;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(r.mock.opens(), 1, "the conversation did not move");
    r.mock.with(|s| {
        s.thread = super::link::ThreadRef {
            id: 900,
            title: "Kai (2)".into(),
        }
    });
    r.feed.fact(FeedFact::Resync);
    r.created(1).await;
    r.mock.socket(1, |s| assert_eq!(s.thread, 900));
    assert_eq!(
        r.mock.with(|s| s.current_calls.clone()),
        [false, false, false, false]
    );
    r.quit().await;
}

// -------------------------------------------------------------------------- ends

#[tokio::test(start_paused = true)]
async fn a_drop_turns_listening_off_with_a_notification() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.drop_socket();
    let n = r.notice("Kai: voice stopped").await;
    assert!(n.body.contains("lmgw went away"), "{n:?}");
    let m = r
        .until(|m| !m.voice.on && m.voice.shown == VoiceShown::Off)
        .await;
    assert!(m.voice.notes.iter().any(|l| l.contains("lmgw went away")));
    let last = r.rings().last().unwrap().clone();
    assert!(!last.bound && !last.active);
    sleep(Duration::from_secs(10)).await;
    assert_eq!(r.mock.opens(), 1, "Q7: off after a drop; no reconnect");
    r.quit().await;
}

/// §3.3, review KAI-3 K3-6: a 4003 close for a disabled device says to enable it (the key stays),
/// one for a rotated key says "pair again". Since lmgw 7d6d18c the close's token says which (api
/// §1.6), and lmgw's sentence is shown without it; every kind: `closes`.
#[tokio::test(start_paused = true)]
async fn a_4003_close_for_a_disabled_device_asks_to_enable_it() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .revoke_socket("device_disabled: device 'kai' was disabled");
    let n = r.notice("Kai: this device is disabled").await;
    assert!(
        n.body.contains("device 'kai' was disabled") && !n.body.contains("device_disabled:"),
        "{n:?}"
    );
    let m = r
        .until(|m| matches!(m.pairing, Pairing::Disabled { .. }))
        .await;
    assert!(!m.voice.on);
    assert_eq!(
        crate::tray::model::render(&m).menu[0],
        crate::tray::model::MenuEntry::TryAgain
    );
    r.quit().await;

    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .revoke_socket("key_unknown: device 'kai' was rotated — pair it again");
    r.notice("Kai: pair again").await;
    r.until(|m| matches!(m.pairing, Pairing::Revoked { .. }))
        .await;
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn lmgw_stopping_says_so_rather_than_a_lost_connection() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.going_away();
    let n = r.notice("Kai: lmgw is stopping").await;
    assert!(
        n.body
            .contains("voice stopped: lmgw is stopping or restarting")
            && !n.body.contains("went away"),
        "{n:?}"
    );
    let m = r
        .until(|m| !m.voice.on && m.voice.shown == VoiceShown::Off)
        .await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|l| l == "voice stopped: lmgw is stopping or restarting"),
        "{:?}",
        m.voice.notes
    );
    sleep(Duration::from_secs(10)).await;
    assert_eq!(
        r.mock.opens(),
        1,
        "off after lmgw's stop; no reconnect by itself"
    );
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_press_while_the_feed_is_down_says_lmgw_is_not_running() {
    let mut r = rig("").await;
    r.feed.raise(LinkEvent::Down {
        note: "lmgw is not reachable; retry in 4 s".into(),
        dead: false,
    });
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
    r.send(Command::Talk(true));
    let n = r.notice("Kai: lmgw is not running").await;
    assert!(n.body.contains("retry in 4 s"));
    r.send(Command::Talk(false));
    sleep(Duration::from_secs(1)).await;
    assert_eq!(r.mock.opens(), 0);
    assert_eq!(
        r.mock.with(|s| s.current_calls.clone()),
        [false],
        "only the start's: Kai buffers nothing"
    );
    assert_eq!(r.pw.lock().unwrap().captures, 0);
    let m = r.until(|m| !m.voice.on).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.starts_with("lmgw is not running"))
    );
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_buffer_is_bounded_by_the_connect_timeout() {
    let mock = MockLink::default();
    mock.with(|s| s.created_after = None);
    let mut r = rig_with("[voice]\nconnect_timeout_s = 2\n", mock).await;
    r.send(Command::Talk(true));
    let n = r.notice("Kai: the voice session did not start").await;
    assert!(
        n.body.contains("within 2 s (voice.connect_timeout_s)")
            && n.body.contains("s of audio were not sent"),
        "{n:?}"
    );
    assert!(r.mock.socket(0, |s| s.closed_by_client && s.appended == 0));
    r.send(Command::Talk(false));
    let m = r.until(|m| m.voice.shown == VoiceShown::Off).await;
    assert!(!m.voice.on);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn quit_truncates_the_playing_reply_before_the_close() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.respond(LONG, 3000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    sleep(Duration::from_millis(800)).await;
    let pw = r.pw.clone();
    let mock = r.quit().await;
    let played_ms = pw.lock().unwrap().played * 1000 / 24_000;
    mock.socket(0, |s| {
        let t = s
            .kinds
            .iter()
            .position(|k| *k == "conversation.item.truncate");
        let c = s.kinds.iter().position(|k| *k == "close");
        assert!(t.is_some() && t < c, "{:?}", s.kinds);
        let (_, ms, _) = s.truncates[0];
        assert_heard(ms, played_ms);
    });
}

// -------------------------------------------------------------------------- mute, taps, hold

#[tokio::test(start_paused = true)]
async fn mute_sends_silence_and_ignores_push_to_talk_presses() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.send(Command::ToggleMute);
    let m = r.until(|m| m.voice.muted).await;
    assert_eq!(render(&m).icon, ICON_MUTED);
    let ring = r.rings().last().unwrap().clone();
    assert!(
        ring.bound && !ring.active,
        "muted: standby, the ring eases out"
    );
    sleep(Duration::from_millis(40)).await;
    let (a0, s0) = r.mock.socket(0, |s| (s.appended, s.silent));
    sleep(Duration::from_millis(400)).await;
    let (a1, s1) = r.mock.socket(0, |s| (s.appended, s.silent));
    assert!(a1 > a0);
    assert_eq!(s1 - s0, a1 - a0, "muted: silence flows");
    r.send(Command::SetVoiceMode(super::VoiceMode::PushToTalk));
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(200)).await;
    r.mock.socket(0, |s| {
        assert_eq!(
            s.count("input_audio_buffer.commit"),
            0,
            "the hold was ignored"
        )
    });
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("presses are ignored"))
    );
    r.send(Command::SetMuted(false));
    r.until(|m| !m.voice.muted).await;
    assert!(
        !r.rings().last().unwrap().active,
        "push-to-talk is in standby between presses"
    );
    r.send(Command::SetVoiceMode(super::VoiceMode::Automatic));
    r.until(|m| m.voice.mode == super::VoiceMode::Automatic)
        .await;
    assert!(
        r.rings().last().unwrap().active,
        "unmuted and automatic: active again"
    );
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn taps_toggle_listening() {
    let mut r = rig("").await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(100)).await;
    r.send(Command::Talk(false));
    let m = r
        .until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
        .await;
    assert_eq!(m.voice.thread.as_deref(), Some("Kai"));
    r.created(0).await;
    sleep(Duration::from_secs(40)).await;
    assert!(
        !r.mock.socket(0, |s| s.closed_by_client),
        "a listening session has no grace close"
    );
    // A repeated press while held is ignored (K6); the release taps listening off.
    r.send(Command::Talk(true));
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(50)).await;
    r.send(Command::Talk(false));
    r.until(|m| !m.voice.on).await;
    r.until_mock("the close", |m| m.socket(0, |s| s.closed_by_client))
        .await;
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_tap_whose_session_comes_up_while_the_key_is_down_ends_in_quiet_listening() {
    // The session is created about 230 ms after the press; the tap lasts 280 ms.
    let mut r = rig("").await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(280)).await;
    r.until_mock("created while held", |m| {
        m.opens() == 1 && m.socket(0, |s| s.count("session.update") >= 1)
    })
    .await;
    r.send(Command::Talk(false));
    let m = r
        .until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
        .await;
    assert_eq!(render(&m).status, "listening on");
    assert!(
        r.rings()
            .last()
            .is_some_and(|i| i.bound && i.active && !i.turn)
    );
    r.mock.socket(0, |s| {
        assert_eq!(
            s.detections(),
            vec![None, Some(TurnDetection::ServerVad)],
            "manual for the press, then automatic once the tap turned listening on"
        );
        assert_eq!(s.count("input_audio_buffer.commit"), 0);
    });
    r.quit().await;
}

/// The shortcuts' edges as the core gets them, through [`crate::shortcuts::Edges`].
fn deliver(r: &Rig, edges: &mut crate::shortcuts::Edges, id: &str, pressed: bool, ts: u64) {
    let e = crate::shortcuts::Edge {
        id: id.into(),
        pressed,
        ts,
    };
    if let Some((id, pressed)) = edges.edge(e)
        && let Some(c) = crate::shortcuts::command(&id, pressed)
    {
        r.send(c);
    }
}

#[tokio::test(start_paused = true)]
async fn a_release_delivered_before_its_press_leaves_no_key_down() {
    // K-2: the portal's release (t = 1070 ms) is handled before its press (t = 1000 ms).
    let mut r = rig("").await;
    let mut edges = crate::shortcuts::Edges::default();
    deliver(&r, &mut edges, "talk", false, 1070);
    deliver(&r, &mut edges, "talk", true, 1000);
    sleep(Duration::from_secs(2)).await;
    let m = r.model.borrow().clone();
    assert_eq!(
        m.voice.shown,
        VoiceShown::Off,
        "no stuck key: nothing listens"
    );
    assert!(!m.voice.on);
    assert_eq!(r.mock.opens(), 0, "no press session opened");
    assert_eq!(
        r.pw.lock().unwrap().captures,
        0,
        "the microphone stays closed"
    );
    // The key works as before: a tap turns listening on.
    deliver(&r, &mut edges, "talk", true, 5000);
    sleep(Duration::from_millis(70)).await;
    deliver(&r, &mut edges, "talk", false, 5070);
    r.until(|m| m.voice.on && m.voice.shown == VoiceShown::Quiet)
        .await;
    r.quit().await;
}

/// Review KAI-2 K2-3: a hold no release ended in `shortcuts.max_hold_s` is no deliberate turn: its
/// audio is discarded (the server's buffer cleared), nothing is committed or asked, and the owner
/// is told.
#[tokio::test(start_paused = true)]
async fn a_hold_with_no_release_is_discarded_at_max_hold() {
    let mut r = rig("[shortcuts]\nmax_hold_s = 2\n[voice]\ngrace_s = 60\n").await;
    r.send(Command::Talk(true));
    r.created(0).await;
    sleep(Duration::from_millis(1500)).await;
    assert!(r.mock.socket(0, |s| s.appended) > 0, "the hold streams");
    let n = r.notice("Kai: the talk key was stuck").await;
    assert_eq!(
        n.body,
        "talk key released by the guard after 2 s; nothing was sent (shortcuts.max_hold_s)"
    );
    let m = r
        .until(|m| {
            m.voice
                .notes
                .iter()
                .any(|n| n.contains("shortcuts.max_hold_s"))
        })
        .await;
    assert!(
        m.voice.notes.iter().any(|n| n.contains("nothing was sent")),
        "{:?}",
        m.voice.notes
    );
    assert_ne!(m.voice.shown, VoiceShown::Listening, "the hold ended");
    r.mock.socket(0, |s| {
        let clears = s
            .kinds
            .iter()
            .rposition(|k| *k == "input_audio_buffer.clear");
        let last_append = s
            .kinds
            .iter()
            .rposition(|k| *k == "input_audio_buffer.append");
        assert!(
            clears > last_append,
            "the server's buffer is cleared after the hold: {:?}",
            s.kinds
        );
    });
    // The real release, if it ever comes, changes nothing; the press session closes at its grace.
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(500)).await;
    r.mock.socket(0, |s| {
        assert_eq!(s.count("input_audio_buffer.commit"), 0, "{:?}", s.kinds);
        assert_eq!(s.count("response.create"), 0, "{:?}", s.kinds);
    });
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_next_bind_waits_until_the_last_close_is_answered() {
    // lmgw answers a close once its journal drained; until then the session holds its slot of
    // the key's concurrency_limit (api §1.3).
    let mock = MockLink::default();
    mock.with(|s| s.close_lag = Duration::from_millis(800));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.send(Command::SetListening(false));
    r.until(|m| !m.voice.on).await;
    r.send(Command::SetListening(true));
    let m = r.until(|m| m.voice.shown == VoiceShown::Connecting).await;
    assert!(m.voice.on);
    sleep(Duration::from_millis(500)).await;
    assert_eq!(r.mock.opens(), 1, "not while the last close is unanswered");
    r.created(1).await;
    // The audio captured meanwhile went into the new session.
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    assert!(r.mock.socket(1, |s| s.appended) >= 500 * 24);
    r.quit().await;
}

/// Review KAI-2 K2-7: one bind in flight. Holds while the last session's close is unanswered
/// join one waiting bind, timed so the close is answered between a press and its `current`: the
/// old code then bound twice, the second over the first.
#[tokio::test(start_paused = true)]
async fn presses_during_a_pending_close_bind_once() {
    let mock = MockLink::default();
    mock.with(|s| {
        s.close_lag = Duration::from_millis(800);
        s.reply = Some(("Gut.".into(), 300));
    });
    let mut r = rig_with("[voice]\ngrace_s = 0\n", mock).await;
    // A press session: its reply plays, then it closes at once (grace 0); lmgw answers the close
    // 800 ms later.
    r.send(Command::Talk(true));
    r.created(0).await;
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    r.until_mock("the press session's close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    // A hold, its release, and the next press just before the close is answered.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    sleep(Duration::from_millis(380)).await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    r.until_mock("the second session's commit", |m| {
        m.opens() > 1 && m.socket(1, |s| s.count("input_audio_buffer.commit") == 1)
    })
    .await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.mock.opens(), 2, "one bind for both holds");
    r.mock.socket(1, |s| {
        assert_eq!(s.open_beside, 0, "no session opened beside another");
        assert!(s.take_over, "the owner's press takes over");
    });
    assert_eq!(
        r.mock.with(|s| s.current_calls.clone()),
        [false, false, false],
        "the start's, the first press's, the second press's own: the third press joined the wait"
    );
    r.quit().await;
}

/// Review KAI-2 K2-7: a new conversation while a bind waits for the last close binds the new
/// thread, not the old one.
#[tokio::test(start_paused = true)]
async fn a_new_conversation_during_a_pending_close_binds_the_new_thread() {
    let mock = MockLink::default();
    mock.with(|s| s.close_lag = Duration::from_millis(800));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.send(Command::SetListening(false));
    r.until(|m| !m.voice.on).await;
    r.send(Command::SetListening(true));
    sleep(Duration::from_millis(100)).await;
    assert_eq!(r.mock.opens(), 1, "the bind waits for the close");
    r.send(Command::NewConversation);
    r.created(1).await;
    r.mock.socket(1, |s| {
        assert_eq!(s.thread, 813, "the new conversation's thread");
        assert_eq!(s.open_beside, 0);
    });
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.mock.opens(), 2);
    let m = r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    assert_eq!(m.voice.thread.as_deref(), Some("New conversation"));
    r.quit().await;
}

/// Review KAI-2 K2-13: the feed has not said yet that another client bound the thread Kai rebinds
/// to; Kai's own bind does not take over, lmgw refuses it, and Kai stays off until a press, which
/// takes the thread.
#[tokio::test(start_paused = true)]
async fn kais_own_rebind_never_takes_over_a_thread_bound_a_moment_ago() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock
        .socket(0, |s| assert!(s.take_over, "the owner's toggle"));
    r.mock.with(|s| {
        s.bound_elsewhere.insert(900, "device 'phone'".into());
        s.thread = super::link::ThreadRef {
            id: 900,
            title: "Kai (2)".into(),
        };
    });
    r.feed.moved(900);
    let n = r.notice("Kai: voice in use elsewhere").await;
    assert_eq!(
        n.body,
        "voice is in use on device 'phone'; Kai stays off until you press the talk key"
    );
    let m = r
        .until(|m| !m.voice.on && m.voice.shown == VoiceShown::Off)
        .await;
    assert!(
        m.voice.notes.iter().any(|n| n.contains("device 'phone'")),
        "{:?}",
        m.voice.notes
    );
    r.mock.socket(1, |s| {
        assert_eq!(s.thread, 900);
        assert!(!s.take_over, "Kai's own rebind: takeover=never");
    });
    // The owner's press takes it.
    r.send(Command::ToggleListening);
    r.created(2).await;
    r.mock
        .socket(2, |s| assert!(s.thread == 900 && s.take_over));
    r.quit().await;
}

/// Review KAI-2 K2-6: the feed's dead-link verdict ends the bound session as a drop (K5); a feed
/// that is merely down (lmgw closed it) leaves the session to its own socket.
#[tokio::test(start_paused = true)]
async fn the_feeds_dead_link_verdict_ends_the_session() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.feed.raise(LinkEvent::Down {
        note: "lmgw closed the feed; next try in 1 s".into(),
        dead: false,
    });
    sleep(Duration::from_millis(500)).await;
    assert!(!r.mock.socket(0, |s| s.closed_by_client));
    r.feed.raise(LinkEvent::Connected {
        device: "desktop".into(),
        held: false,
        hosts_label: None,
        admin: Default::default(),
    });
    r.feed.raise(LinkEvent::Down {
        note: "lmgw sent nothing for 30 s (two hello.keepalive_s intervals): the link is dead"
            .into(),
        dead: true,
    });
    let n = r.notice("Kai: voice stopped").await;
    assert!(
        n.body.contains("the link to lmgw is dead") && n.body.contains("press the talk key"),
        "{n:?}"
    );
    let m = r.until(|m| !m.voice.on).await;
    assert!(
        m.voice
            .notes
            .iter()
            .any(|l| l.contains("the link to lmgw is dead"))
    );
    assert!(r.mock.socket(0, |s| s.closed_by_client));
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_hold_badge_and_one_notification_per_hold_period() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.feed.raise(LinkEvent::Hold { active: true });
    r.mock.model_state("tts", "mock/tts", "held", None);
    let n = r.notice("Kai: GPU hold").await;
    assert!(
        n.body.contains("TTS model 'mock/tts' has no fallback"),
        "{n:?}"
    );
    let m = r.until(|m| m.voice.held).await;
    assert_eq!(render(&m).badge, Some(BADGE_HELD));
    assert!(
        r.rings().iter().any(|i| i.held && i.active),
        "the hold changes nothing: continuous listening stays active"
    );
    r.mock.model_state("tts", "mock/tts", "held", None);
    // K21: a fallback that answered is said once per hold period.
    let timing = json!({"to_first_audio_ms": 900,
        "models": {"asr": {"alias": "mock/asr", "answered_by": "cloud/asr"},
                   "chat": {"alias": "mock/chat"}, "tts": {"alias": "mock/tts"}}});
    r.mock.emit(ServerEvent::Timing(timing.clone()));
    let n = r.notice("Kai: a fallback answered").await;
    assert_eq!(
        n.body,
        "the ASR alias 'mock/asr' was answered by 'cloud/asr' (its configured fallback)"
    );
    let m = r
        .until(|m| {
            m.voice
                .timing
                .as_deref()
                .is_some_and(|t| t.contains("cloud/asr"))
        })
        .await;
    assert!(render(&m).notes[0].contains("ASR answered by cloud/asr"));
    r.mock.emit(ServerEvent::Timing(timing.clone()));
    // K21 is per stage: another fallback alias for the ASR in the same period says nothing new.
    r.mock
        .emit(ServerEvent::Timing(json!({"to_first_audio_ms": 900,
        "models": {"asr": {"alias": "mock/asr", "answered_by": "cloud/asr-2"}}})));
    sleep(Duration::from_millis(100)).await;
    r.no_notice("Kai: GPU hold");
    r.no_notice("Kai: a fallback answered");
    // The hold ends: a new period, so the next fallback is said again.
    r.feed.raise(LinkEvent::Hold { active: false });
    let m = r.until(|m| !m.voice.held).await;
    assert_eq!(render(&m).badge, None);
    r.mock.emit(ServerEvent::Timing(timing));
    r.notice("Kai: a fallback answered").await;
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn after_a_pipewire_restart_the_streams_open_again() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.respond(LONG, 3000);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    sleep(Duration::from_millis(800)).await;
    // PipeWire goes away: both streams read closed.
    {
        let pw = r.pw.lock().unwrap();
        pw.mics[0].close();
        pw.heads[0].close();
    }
    let played = r.played();
    r.audio_tx
        .send(AudioEvent::Link(Some("PipeWire is not reachable".into())))
        .unwrap();
    // The reply cannot play on: speaking ends, and the thread keeps what was heard (K-11).
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    assert!(
        !r.rings().last().unwrap().active,
        "with PipeWire down Kai hears nothing: standby"
    );
    r.until_mock("the truncate", |m| m.socket(0, |s| !s.truncates.is_empty()))
        .await;
    let (item, ms, sent) = r.mock.socket(0, |s| s.truncates[0].clone());
    assert_eq!(item, "item_a1");
    // The stream is gone: the last report's heard, with no extrapolation. That report came at
    // the start of the last cycle, so it is what the sink got less the latency and that cycle.
    let heard = played * 1000 / 24_000 - LATENCY_MS;
    assert!(
        ms <= heard && ms + 11 >= heard,
        "truncated at {ms} ms; the sink got {} ms, so {heard} ms were heard",
        played * 1000 / 24_000
    );
    assert!(ms >= 300 && sent > ms, "{ms} ms heard, {sent} ms sent");
    assert_eq!(
        r.mock.stored(0)["resp_1"].clone().unwrap(),
        heard_text(LONG, 3000, ms)
    );
    // Back: the capture opens again and audio flows; the next reply opens a new player.
    r.audio_tx.send(AudioEvent::Link(None)).unwrap();
    r.until_mock("audio again", |_| r.pw.lock().unwrap().captures == 2)
        .await;
    assert!(r.rings().last().unwrap().active, "active again");
    sleep(Duration::from_millis(40)).await;
    let a0 = r.mock.socket(0, |s| s.appended);
    sleep(Duration::from_millis(300)).await;
    assert!(r.mock.socket(0, |s| s.appended) > a0 + 200 * 24);
    r.mock.respond("Wieder da.", 500);
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    assert_eq!(r.pw.lock().unwrap().heads.len(), 2);
    r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_confirmed_re_pair_ends_the_session_bound_on_the_old_gateway() {
    // A press session waiting out its grace, with listening off: a re-pair must end it too.
    let mut r = rig("[voice]\ngrace_s = 600\n").await;
    r.feed.fact(FeedFact::VoiceBound {
        thread_id: 812,
        by: By {
            name: "device 'phone'".into(),
            own: false,
        },
    });
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.send(Command::Talk(false));
    r.until(|m| m.voice.shown == VoiceShown::Speaking).await;
    r.until(|m| m.voice.shown == VoiceShown::Off && m.voice.thread.is_some())
        .await;
    let m = r.until(|m| m.voice.thread.is_some()).await;
    assert!(
        m.voice.notes.iter().any(|n| n.contains("phone")),
        "{:?}",
        m.voice.notes
    );
    assert!(!r.mock.socket(0, |s| s.closed_by_client), "in its grace");
    assert_eq!(r.mock.with(|s| s.creds.clone()).as_deref(), Some(URL));
    // A link for another gateway arrives and the owner confirms it.
    let link = crate::pairing::tests::link(&format!(
        "v=1&url=http%3A%2F%2F127.0.0.1%3A2&name=desktop&key={KEY}"
    ));
    r.send(Command::Arrived(
        crate::pairing::PairingLink::parse(&link).unwrap(),
    ));
    let id = loop {
        match tokio::time::timeout(Duration::from_secs(60), r.ui.recv())
            .await
            .unwrap()
            .unwrap()
        {
            UiCmd::ConfirmPair(req) => break req.id,
            _ => continue,
        }
    };
    assert!(
        !r.mock.socket(0, |s| s.closed_by_client),
        "not before the answer"
    );
    r.send(Command::ConfirmPair { id, accept: true });
    r.until_mock("the old session's close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    let m = r
        .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == "http://127.0.0.1:2"))
        .await;
    assert!(!m.voice.on);
    assert_eq!(
        m.voice.thread, None,
        "the old gateway's thread is forgotten"
    );
    assert_eq!(
        Config::load(&r.config_file).unwrap().conversation.folder_id,
        None,
        "and its folder (§4)"
    );
    assert!(
        !m.voice.notes.iter().any(|n| n.contains("phone")),
        "and what its other clients did: {:?}",
        m.voice.notes
    );
    assert_eq!(
        r.mock.with(|s| s.creds.clone()).as_deref(),
        Some("http://127.0.0.1:2"),
        "the voice link now holds the new gateway's credentials"
    );
    assert_eq!(
        r.mock.opens(),
        1,
        "no session opens by itself on the new one"
    );
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_reply_while_pipewire_is_down_opens_no_stream_and_keeps_nothing_unheard() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.pw.lock().unwrap().mics[0].close();
    r.audio_tx
        .send(AudioEvent::Link(Some("PipeWire is not reachable".into())))
        .unwrap();
    r.until(|m| m.audio.link.is_some()).await;
    r.mock.respond(LONG, 3000);
    r.until_mock("the truncate", |m| m.socket(0, |s| !s.truncates.is_empty()))
        .await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(
        r.pw.lock().unwrap().heads.len(),
        0,
        "no playback stream was opened"
    );
    r.mock.socket(0, |s| {
        assert_eq!(s.truncates.len(), 1, "one truncate, not one per delta");
        assert_eq!(
            (s.truncates[0].0.as_str(), s.truncates[0].1),
            ("item_a1", 0)
        );
        assert_eq!(s.refused_truncates, 0);
    });
    assert_eq!(
        r.mock.stored(0)["resp_1"],
        None,
        "nothing was heard, nothing is kept"
    );
    let m = r.until(|m| m.voice.shown == VoiceShown::Quiet).await;
    assert!(m.voice.on);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_press_during_new_conversation_binds_the_new_thread_and_commits_once() {
    // K-10: with no session, "New conversation" asks for the thread; a press while it asks is not
    // swallowed, and its release's commit is not carried into the next hold.
    let mut r = rig("[voice]\ngrace_s = 60\n").await;
    r.send(Command::NewConversation);
    r.send(Command::Talk(true));
    let m = r.until(|m| m.voice.shown == VoiceShown::Listening).await;
    assert!(!m.voice.on);
    sleep(Duration::from_millis(600)).await;
    r.send(Command::Talk(false));
    r.until_mock("the commit", |m| {
        m.opens() == 1 && m.socket(0, |s| s.count("response.create") == 1)
    })
    .await;
    assert_eq!(
        r.mock.with(|s| s.current_calls.clone()),
        [false, true],
        "after the start's, one request: the new conversation's"
    );
    r.mock.socket(0, |s| {
        assert_eq!(s.thread, 813, "the press bound the new thread");
        let ms = s.appended as u64 * 1000 / 24_000;
        assert!((590..=610).contains(&ms), "the whole hold: {ms} ms");
        assert_eq!(s.count("input_audio_buffer.commit"), 1);
    });
    // The reply plays; the next hold is one turn, committed at its own release only.
    r.until(|m| m.voice.shown == VoiceShown::Off && m.voice.timing.is_some())
        .await;
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(500)).await;
    r.mock
        .socket(0, |s| assert_eq!(s.count("input_audio_buffer.commit"), 1));
    r.send(Command::Talk(false));
    r.until_mock("the second commit", |m| {
        m.socket(0, |s| s.count("input_audio_buffer.commit") == 2)
    })
    .await;
    assert_eq!(r.mock.opens(), 1);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn on_at_start_opens_a_session_when_the_feed_comes_up() {
    let mut r = rig("[voice]\non_at_start = true\n").await;
    r.created(0).await;
    r.until(|m| m.voice.on).await;
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn an_error_is_shown_verbatim_and_a_refusal_ends_the_session() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.emit(ServerEvent::Error(super::link::ErrorFacts {
        kind: "invalid_request_error".into(),
        code: Some("context_length_exceeded".into()),
        message: "this conversation no longer fits".into(),
    }));
    let n = r.notice("Kai: voice error").await;
    assert_eq!(
        n.body,
        "context_length_exceeded: this conversation no longer fits"
    );
    r.until(|m| m.voice.notes.iter().any(|n| n.contains("no longer fits")))
        .await;
    r.send(Command::SetListening(false));
    r.until(|m| !m.voice.on).await;
    // A refused bind shows the server's message.
    r.mock.with(|s| {
        s.current_error = Some(super::mock::refusal(
            403,
            "key_scope",
            "this key may not use the alias 'kai-chat'",
        ))
    });
    r.send(Command::SetListening(true));
    let n = r.notice("Kai: voice could not start").await;
    assert_eq!(
        n.body,
        "this key may not use the alias 'kai-chat' (key_scope)"
    );
    r.until(|m| !m.voice.on).await;
    r.quit().await;
}

/// Review KAI-3 K3-8: a known event whose fields do not read is warned about by its name and
/// size; an event Kai does not act on stays at trace.
#[tokio::test(start_paused = true)]
async fn a_known_realtime_event_that_does_not_read_is_warned_about() {
    let out = crate::logging::test_support::Capture::default();
    let _log =
        tracing::subscriber::set_default(crate::logging::subscriber("info", out.clone(), false));
    let mut r = rig("").await;
    r.listening_on().await;
    let secret = "the owner's words";
    for kind in ["lmgw.model.state", "response.output_item.added"] {
        r.mock.emit(ServerEvent::Unknown {
            kind: kind.into(),
            data: json!({"type": kind, "text": secret}),
        });
    }
    sleep(Duration::from_millis(100)).await;
    r.quit().await;
    let text = out.text();
    assert!(
        text.lines().any(|l| l.contains("WARN")
            && l.contains("kind=lmgw.model.state")
            && l.contains("bytes=")),
        "{text}"
    );
    assert!(!text.contains("response.output_item.added"), "{text}");
    assert!(!text.contains(secret), "{text}");
}
