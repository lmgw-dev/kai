//! The voice session's live check in a private PipeWire instance (`just live-check-voice`, which
//! runs `scripts/live-check-audio.sh` with `KAI_LIVE_ONLY=live_voice`): Kai's core with real
//! PipeWire streams, against the scripted lmgw of [`super::mock`]. Nothing is audible and the
//! session's devices are never touched. Ignored by default; it refuses to run outside the private
//! instance.
//!
//! - **A cold push-to-talk bind** (`session.created` 500 ms after the open, listening off): a TTS
//!   clip played into the test microphone from the press reaches lmgw whole and in order, the
//!   part captured before the session existed included (§5.3).
//! - **Automatic listening and a barge-in**: the reply (the same clip) plays on the test sink;
//!   `speech_started` 1.5 s in flushes it, and the truncate's `audio_end_ms` is compared with what
//!   the sink's monitor got (what was played; heard is that less the device delay, §5.5).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use super::mock::MockLink;
use super::tests::Feed;
use crate::audio::live_tests::{
    MIC, MIC_IN, Monitor, SINK, align, out_dir, private_or_skip, read_wav, settings,
};
use crate::audio::{Audio, RATE};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::LinkEvent;
use crate::pairing::DeviceKey;
use crate::pairing::tests::KEY;
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{Link, Pairing, TrayModel};

const URL: &str = "http://127.0.0.1:1";
/// When the clip starts playing into the microphone, after the press.
const SPEAKS_AFTER_MS: u64 = 150;

/// Where `clip`'s onset lies in `got` (both 24 kHz), and the normalized correlation of the first
/// half second there.
fn locate(clip: &[i16], got: &[i16]) -> Option<(i64, f64)> {
    let peak = clip.iter().map(|&s| (s as i32).abs()).max()? as f64;
    let thr = peak * 0.1;
    let k_on = clip.iter().position(|&s| (s as f64).abs() > thr)? as i64;
    let g_on = got.iter().position(|&s| (s as f64).abs() > thr)? as i64;
    let win = 12_000.min(clip.len() - k_on as usize);
    let l0 = g_on - k_on;
    let mut best = (f64::MIN, l0);
    for l in l0 - 400..=l0 + 400 {
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        for (k, &x) in clip.iter().enumerate().skip(k_on as usize).take(win) {
            let j = k as i64 + l;
            let y = if j >= 0 && (j as usize) < got.len() {
                got[j as usize] as f64
            } else {
                0.0
            };
            let x = x as f64;
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        let c = xy / (xx.sqrt() * yy.sqrt()).max(1.0);
        if c > best.0 {
            best = (c, l);
        }
    }
    Some((best.1, best.0))
}

async fn until(model: &mut watch::Receiver<TrayModel>, f: impl FnMut(&TrayModel) -> bool) {
    tokio::time::timeout(Duration::from_secs(20), model.wait_for(f))
        .await
        .expect("the tray model to get there")
        .unwrap();
}

async fn until_mock(mock: &MockLink, what: &str, f: impl Fn(&MockLink) -> bool) {
    let t0 = Instant::now();
    while !f(mock) {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "the mock never got to: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the private PipeWire instance of scripts/live-check-audio.sh"]
async fn live_voice_push_to_talk_and_barge_in() {
    if !private_or_skip() {
        return;
    }
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    let mut report = String::new();
    let dir = tempfile::tempdir().unwrap();

    // Kai's audio on the test microphone and the default test sink.
    let mut s = settings(dir.path());
    s.input = Some(MIC.into());
    s.output = Some(SINK.into());
    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let audio = Audio::start(s, audio_tx).unwrap();
    // A second audio handle plays into the microphone's loopback.
    let mut sb = settings(&dir.path().join("feeder"));
    sb.output = Some(MIC_IN.into());
    let (btx, _brx) = mpsc::unbounded_channel();
    let mut feeder_audio = Audio::start(sb, btx).unwrap();

    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        format!(
            "[lmgw]\nurl = \"{URL}\"\n[audio]\ninput = \"{MIC}\"\noutput = \"{SINK}\"\n\
             [voice]\ngrace_s = 1\n"
        ),
    )
    .unwrap();
    let store = Arc::new(MemoryStore::default());
    store
        .items
        .lock()
        .unwrap()
        .insert(URL.into(), ("d".into(), DeviceKey::parse(KEY).unwrap()));
    let mock = MockLink::default();
    mock.with(|s| {
        s.keep_audio = true;
        // The push-to-talk turn's answer; the barge-in's reply is the clip itself.
        s.reply = Some(("Gern.".into(), 400));
        s.created_after = Some(Duration::from_millis(500));
    });
    let feed = Feed::default();
    let (model_tx, mut model) = watch::channel(TrayModel::starting(false));
    let (ui_tx, _ui) = async_channel::unbounded();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let (ntx, mut notices) = mpsc::unbounded_channel();
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
    let task = tokio::spawn(core.run(cmd_rx));
    until(&mut model, |m| matches!(m.pairing, Pairing::Paired { .. })).await;
    feed.raise(LinkEvent::Connected {
        device: "desktop".into(),
        held: false,
        hosts_label: None,
        admin: Default::default(),
    });
    until(&mut model, |m| {
        matches!(
            m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        ) && m.audio.devices.inputs.iter().any(|d| d.name == MIC)
    })
    .await;

    // ---- a cold push-to-talk bind with listening off (§5.3)
    let mut feeder = feeder_audio.open_playback();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let said = &clip[..(RATE as usize * 2).min(clip.len())];
    let press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    // The owner starts speaking a moment after the press.
    tokio::time::sleep(Duration::from_millis(SPEAKS_AFTER_MS)).await;
    feeder.push("said", said);
    while press.elapsed() < Duration::from_millis(2400) {
        feeder.pump();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cmds.send(Command::Talk(false)).unwrap();
    let held_ms = press.elapsed().as_millis() as u64;
    until_mock(&mock, "the commit", |m| {
        m.opens() == 1 && m.socket(0, |s| s.count("response.create") == 1)
    })
    .await;
    let (kinds, got) = mock.socket(0, |s| (s.kinds.clone(), s.audio.clone()));
    let got_ms = got.len() as u64 * 1000 / RATE as u64;
    let (lag, corr) = locate(said, &got).expect("the clip reached lmgw");
    let lag_ms = lag * 1000 / RATE as i64;
    let line = format!(
        "push-to-talk, cold bind (session.created 500 ms after the open): held {held_ms} ms, \
         appended {got_ms} ms; the clip, played into the microphone {SPEAKS_AFTER_MS} ms after \
         the press, starts {lag_ms} ms into what lmgw got (the capture ran about {} ms after the \
         press, the loopback's own delay included), correlation {corr:.3} over its first 0.5 s; \
         first event {:?}\n",
        SPEAKS_AFTER_MS as i64 - lag_ms,
        kinds[0]
    );
    eprint!("{line}");
    report.push_str(&line);
    assert_eq!(
        kinds[0], "session.update",
        "§5.3: the update before the buffer"
    );
    assert!(
        got_ms + 60 >= held_ms && got_ms <= held_ms + 60,
        "every captured sample from the press went out"
    );
    assert!(corr > 0.8, "the clip arrived intact");
    assert!(
        lag > 0 && lag_ms < SPEAKS_AFTER_MS as i64 + 100,
        "the clip's start was not lost"
    );
    drop(feeder);

    // ---- automatic listening, the reply on the sink, a barge-in 1.5 s in (§5.5)
    until_mock(&mock, "the grace close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    let line = format!(
        "the press session closed {} ms after the press (its reply of 400 ms, then \
         voice.grace_s = 1)\n",
        press.elapsed().as_millis()
    );
    eprint!("{line}");
    report.push_str(&line);
    let monitor = Monitor::start(SINK);
    cmds.send(Command::SetListening(true)).unwrap();
    until_mock(&mock, "session.created", |m| {
        m.opens() == 2 && m.socket(1, |s| s.count("session.update") > 0)
    })
    .await;
    mock.respond_pcm("eins zwei drei vier fünf sechs sieben acht", clip.clone());
    until(&mut model, |m| {
        m.voice.shown == crate::tray::model::VoiceShown::Speaking
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    mock.speech_started();
    until_mock(&mock, "the truncate", |m| {
        m.socket(1, |s| !s.truncates.is_empty())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cycles = monitor.finish();
    let mon: Vec<f32> = cycles.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let (_, truncated_ms, sent_ms) = mock.socket(1, |s| s.truncates[0].clone());
    let m0 = align(&clip, &mon);
    let end = mon
        .iter()
        .rposition(|s| s.abs() > 1e-4)
        .expect("the reply reached the sink") as i64;
    let on_sink_ms = (end - m0) * 1000 / 48_000;
    let cancels = mock.socket(1, |s| s.cancels());
    let line = format!(
        "barge-in 1.5 s into the reply: truncated at {truncated_ms} ms (heard), the sink got \
         {on_sink_ms} ms of it (played), {sent_ms} ms were sent; response.cancel sent: {cancels}\n"
    );
    eprint!("{line}");
    report.push_str(&line);
    assert!((1300..1700).contains(&truncated_ms), "{truncated_ms}");
    assert!(
        on_sink_ms + 5 >= truncated_ms as i64 && on_sink_ms <= truncated_ms as i64 + 150,
        "heard = played less the device delay"
    );
    assert_eq!(
        cancels, 0,
        "a barge-in's response the server cancels itself"
    );
    while let Ok(n) = notices.try_recv() {
        report.push_str(&format!("notification: {} | {}\n", n.summary, n.body));
    }
    std::fs::write(out_dir().join("voice-report.txt"), &report).unwrap();
    cmds.send(Command::Quit).unwrap();
    task.await.unwrap();
    feeder_audio.shutdown(Duration::from_secs(4)).await;
}
