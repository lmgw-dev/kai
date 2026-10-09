//! The Kai conversation against a real lmgw (§15 WP6): Kai's core with the real feed, the realtime
//! adapter and real PipeWire streams in the private instance, against a dev lmgw on 127.0.0.1:8897
//! whose chat, speech-to-text and text-to-speech are mocks (no model, no GPU), as in
//! `lmgw_live_tests` (whose harness this reuses). `scripts/live-check-conversation.sh`
//! (`just live-check-conversation`) prepares it and runs it; ignored by default, and it refuses to
//! run outside the private PipeWire instance or against any other gateway.
//!
//! The owner's side goes through the dev lmgw's `/api` and Chat API with the owner bearer from its
//! log line. Kai's device key and a second device's live in memory only (no wallet). Kai's
//! configuration starts without `conversation.folder_id`: a first run.
//!
//! The checks, each with its numbers in `report.txt`:
//! 1. a first run: no folder "Kai" on the gateway, so Kai creates a plain one and says the owner's
//!    steps (a notification, the tooltip); a device's write of the folder's retention is refused
//!    (why Kai never sends it); a press is refused `not_ongoing` and says the steps again;
//! 2. the owner sets the model, marks it ongoing with one idle minute and a year's retention: Kai
//!    recovers on `folder.updated` (the steps gone, the id remembered, `current` asked);
//! 3. Open in lmgw names the thread's dashboard page (no browser is opened);
//! 4. a turn, then the folder's idle time with no turn: Kai asks `current`, lmgw rolls over, and
//!    Kai rebinds at the quiet point;
//! 5. a new conversation from the tray: `current {new: true}`, then the rebind; then (5b) that
//!    thread leaves the device's reach (api §1.6): the owner attaches lmgw's admin tools to it,
//!    lmgw closes Kai's session with 4004, and Kai asks for the folder's current thread and binds
//!    it with its own bind (never a takeover), with no "pair again";
//! 6. K5: a second device key rolls the conversation over after the idle time and binds the new
//!    thread first, while Kai's talk key is held; Kai finishes its turn in the old thread and stays
//!    off, naming the other device.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::Notice;
use super::link::{ServerEvent, SessionEvent};
use super::lmgw_live_tests::{
    Lmgw, Mic, Report, Tap, Tapped, URL, device_key, env, is_server, ms, notice, set_up_mocks,
    shown, until,
};
use crate::audio::live_tests::{MIC, MIC_IN, SINK, out_dir, private_or_skip, read_wav, settings};
use crate::audio::{Audio, RATE};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::{FeedLink, FeedSettings, RealtimeLink, RealtimeSettings};
use crate::pairing::DeviceKey;
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{Link, MenuEntry, Pairing, TrayModel, VoiceShown, render};

const STEPS: &str = "Kai: set up the folder 'Kai' in lmgw";

/// The owner's folder `name`, as the owner lists it.
async fn owners_folder(lmgw: &Lmgw, name: &str) -> Option<Value> {
    let v = lmgw.owner("GET", "/chat/api/folders", None).await;
    v["folders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .cloned()
}

/// A push-to-talk turn with listening on: the clip spoken into the microphone while the key is
/// held. Returns when the reply was stored (lmgw.chat.reply) and has played.
async fn press_turn(
    cmds: &mpsc::UnboundedSender<Command>,
    mic: &mut Mic,
    clip: &[i16],
    tap: &Tap,
    model: &mut watch::Receiver<TrayModel>,
) -> Instant {
    let press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let end = mic.say(clip);
    tokio::time::sleep_until((end + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let reply = tap
        .until_got(press, 60, "the reply as stored", |g| {
            is_server(g, |e| matches!(e, ServerEvent::ChatReply(_)))
        })
        .await;
    until(model, 60, "the reply played", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    reply.at
}

fn last_current(tap: &Tap) -> super::lmgw_live_tests::CurrentCall {
    tap.with(|t| t.currents.last().cloned().expect("a current call"))
}

/// The first `current` Kai asked whose answer came at or after `since` and that `f` accepts, waited
/// for up to 15 s. The feed's `folder.current` may tell Kai the thread before its own answer
/// arrives (live against lmgw a50141d: 11 ms after the ask, ahead of the answer), so a step that
/// reads Kai's answer waits for it, not only for the tray.
async fn current_since(
    tap: &Tap,
    since: Instant,
    what: &str,
    f: impl Fn(&super::lmgw_live_tests::CurrentCall) -> bool,
) -> super::lmgw_live_tests::CurrentCall {
    let start = Instant::now();
    loop {
        if let Some(c) = tap.with(|t| t.currents.iter().find(|c| c.0 >= since && f(c)).cloned()) {
            return c;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "no current: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/live-check-conversation.sh (a private PipeWire instance and a dev lmgw)"]
async fn lmgw_conversation_live() {
    if !private_or_skip() {
        return;
    }
    assert_eq!(
        std::env::var("KAI_LIVE_LMGW_URL").as_deref(),
        Ok(URL),
        "only the dev lmgw on 127.0.0.1:8897"
    );
    let log_path = out_dir().join("kai.log");
    let log = std::fs::File::create(&log_path).unwrap();
    // Kai's own subscriber, its log cap included (review KAI-2 K2-1).
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "info".into()),
        Mutex::new(log),
        false,
    ));
    let clip = read_wav(&env("KAI_LIVE_CLIP"));
    let mut r = Report(String::new());
    let dir = tempfile::tempdir().unwrap();

    let mut lmgw = Lmgw::new();
    let boot = lmgw.start().await;
    set_up_mocks(&lmgw).await;
    let key = device_key(&lmgw, "kai-live").await;
    let key2 = device_key(&lmgw, "kai-live-2").await;
    assert!(
        owners_folder(&lmgw, "Kai").await.is_none(),
        "a fresh gateway"
    );
    r.line(format!(
        "dev lmgw up in {} ms (data dir fresh: no folder 'Kai'); mocks: chat (echo), ASR, TTS (the clip, {} ms per clause)",
        ms(boot),
        clip.len() as u64 * 1000 / RATE as u64
    ));

    // Kai: audio on the test microphone and sink; a second handle plays into the microphone.
    let mut s = settings(dir.path());
    s.input = Some(MIC.into());
    s.output = Some(SINK.into());
    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let audio = Audio::start(s, audio_tx).unwrap();
    let mut sb = settings(&dir.path().join("feeder"));
    sb.output = Some(MIC_IN.into());
    let (btx, _brx) = mpsc::unbounded_channel();
    let mut mic_audio = Audio::start(sb, btx).unwrap();
    let mut mic = Mic::new(&mic_audio);

    // A first run: no conversation.folder_id.
    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        format!(
            "[lmgw]\nurl = \"{URL}\"\nretry_max_s = 2\n[audio]\ninput = \"{MIC}\"\noutput = \"{SINK}\"\n\
             [voice]\nmode = \"push_to_talk\"\ngrace_s = 2\n[shortcuts]\nbind = false\n"
        ),
    )
    .unwrap();
    let config_file = paths.config_file.clone();
    let remembered = || Config::load(&config_file).unwrap().conversation.folder_id;
    let config = Config::load(&paths.config_file).unwrap();
    let store = Arc::new(MemoryStore::default());
    store.items.lock().unwrap().insert(
        URL.into(),
        ("kai-live".into(), DeviceKey::parse(&key).unwrap()),
    );
    let feed = FeedLink::new(FeedSettings::from_config(&config)).unwrap();
    let tap = Tap::default();
    let voice_link = Tapped {
        link: RealtimeLink::new(RealtimeSettings::from_config(&config), feed.client()),
        tap: tap.clone(),
    };
    let (model_tx, mut model) = watch::channel(TrayModel::starting(false));
    let (ui_tx, _ui) = async_channel::unbounded();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let (ntx, mut notices) = mpsc::unbounded_channel();
    let (otx, mut opened) = mpsc::unbounded_channel();
    let mut seen: Vec<(Instant, Notice)> = Vec::new();
    let mut core = Core::new(
        config,
        paths,
        "/usr/bin/kai".into(),
        store,
        feed,
        voice_link,
        (audio, audio_rx),
        model_tx,
        ui_tx,
        None,
    );
    core.notify_into(ntx);
    core.open_into(otx);
    let t0 = Instant::now();
    let task = tokio::spawn(core.run(cmd_rx));

    // ---- 1. a first run: Kai creates its folder and says the owner's steps (§4)
    until(&mut model, 20, "the feed up", |m| {
        matches!(
            &m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        ) && m.audio.devices.inputs.iter().any(|d| d.name == MIC)
    })
    .await;
    let up = Instant::now();
    let (told_at, told) = notice(&mut notices, &mut seen, STEPS, 20).await;
    let created = owners_folder(&lmgw, "Kai")
        .await
        .expect("Kai created its folder");
    let folder = created["id"].as_i64().unwrap();
    let m = until(&mut model, 10, "the steps in the tooltip", |m| {
        m.conversation
            .iter()
            .any(|n| n.starts_with("voice waits for its folder"))
    })
    .await;
    let tooltip = render(&m)
        .notes
        .into_iter()
        .find(|n| n.starts_with("voice waits"))
        .unwrap();
    r.line(format!(
        "1 first run: feed up {} ms after the start; folder {folder} created (as the owner reads it: model {}, ongoing {}, archive_days {}, purge_days {}); \
         notified {} ms after the feed was up: \"{}\" | \"{}\"; tooltip: \"{tooltip}\"; conversation.folder_id: {:?}",
        ms(up - t0),
        created["defaults"]["model_alias"],
        created["ongoing"],
        created["archive_days"],
        created["purge_days"],
        ms(told_at.saturating_duration_since(up)),
        told.summary,
        told.body,
        remembered(),
    ));
    assert!(created["defaults"]["model_alias"].is_null() && created["ongoing"].is_null());
    assert!(created["archive_days"].is_null() && created["purge_days"].is_null());
    assert!(
        told.body
            .contains("model and mark it as an ongoing conversation")
    );
    assert_eq!(remembered(), None);
    // Open in lmgw while the folder waits: its settings form (lmgw's deep link), served.
    cmds.send(Command::OpenInLmgw).unwrap();
    let settings_url = tokio::time::timeout(Duration::from_secs(5), opened.recv())
        .await
        .unwrap()
        .unwrap();
    let settings_status = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&settings_url)
        .bearer_auth(&lmgw.token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    r.line(format!(
        "1 Open in lmgw while the folder waits would open {settings_url}; the dev lmgw serves it: \
         HTTP {settings_status}"
    ));
    assert_eq!(
        settings_url,
        format!("{URL}/chat?folder={folder}&settings=1")
    );
    assert_eq!(settings_status, 200);
    // Why Kai never sends the folder's retention: a device's write of it is refused.
    let (status, text) = lmgw
        .call(
            "POST",
            &format!("/chat/api/folders/{folder}"),
            Some(json!({"purge_days": 365})),
            &key,
        )
        .await;
    r.line(format!(
        "1 a device's retention write: HTTP {status} {text}"
    ));
    assert_eq!(status, 403);
    // A press: current is refused (not ongoing); the steps again, no session.
    let press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    cmds.send(Command::Talk(false)).unwrap();
    let (again_at, _) = notice(&mut notices, &mut seen, STEPS, 10).await;
    let refused = last_current(&tap);
    r.line(format!(
        "1 a press: current answered {:?}; the steps notified again {} ms after the press; sessions opened: {}",
        refused.3,
        ms(again_at.saturating_duration_since(press)),
        tap.opens()
    ));
    assert!(refused.3.as_ref().unwrap_err().contains("not_ongoing"));
    assert_eq!(tap.opens(), 0);

    // ---- 2. the owner sets the model, ongoing (one idle minute) and a year's retention
    let patch = Instant::now();
    lmgw.owner(
        "POST",
        &format!("/chat/api/folders/{folder}"),
        Some(json!({"defaults": {"model_alias": "kai-chat"},
            "ongoing": {"idle_minutes": 1}, "purge_days": 365})),
    )
    .await;
    let m = until(&mut model, 15, "recovered: a thread, no steps", |m| {
        m.voice.thread.is_some() && m.conversation.is_empty()
    })
    .await;
    let recovered = Instant::now();
    let first = current_since(&tap, patch, "Kai's own after the patch", |c| c.3.is_ok()).await;
    let (t1, rolled) = first.3.clone().expect("current answered");
    let view = render(&m);
    r.line(format!(
        "2 recovery on folder.updated: the thread known {} ms after the owner's patch (current answered {} ms after it: thread {t1}, rolled over {rolled}); \
         conversation.folder_id = {:?}; tray title \"{}\", menu {:?}",
        ms(recovered - patch),
        ms(first.0.saturating_duration_since(patch)),
        remembered(),
        view.title,
        &view.menu[..6]
    ));
    assert_eq!(remembered(), Some(folder));
    assert!(rolled);
    assert_eq!(view.title, "Kai: New chat");
    assert!(view.menu.contains(&MenuEntry::Continue {
        title: "New chat".into()
    }));
    assert!(view.menu.contains(&MenuEntry::OpenInLmgw));
    assert!(!view.notes.iter().any(|n| n.contains("voice waits")));

    // ---- 3. Open in lmgw: the thread's dashboard page (no browser here)
    cmds.send(Command::OpenInLmgw).unwrap();
    let url = tokio::time::timeout(Duration::from_secs(5), opened.recv())
        .await
        .unwrap()
        .unwrap();
    let page = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&url)
        .bearer_auth(&lmgw.token)
        .send()
        .await
        .unwrap();
    let page_status = page.status().as_u16();
    let page_type = page
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_owned());
    r.line(format!(
        "3 Open in lmgw would open {url}; the dev lmgw serves it: HTTP {page_status} {page_type:?}"
    ));
    assert_eq!(url, format!("{URL}/chat?t={t1}"));
    assert_eq!(page_status, 200);

    // ---- 4. a turn, then the idle rollover (one idle minute) and the rebind at the quiet point
    cmds.send(Command::SetListening(true)).unwrap();
    tap.until_got(patch, 15, "session.created", |g| {
        is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
    })
    .await;
    until(&mut model, 10, "quiet listening", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    assert_eq!(tap.with(|t| t.opens[0].1), t1);
    let a1 = press_turn(&cmds, &mut mic, &clip, &tap, &mut model).await;
    let before = tap.with(|t| t.currents.len());
    let reopened = loop {
        assert!(
            a1.elapsed() < Duration::from_secs(150),
            "no rollover within 150 s of the turn"
        );
        if tap.opens() >= 2 {
            break tap.with(|t| t.opens[1]);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let roll = tap.with(|t| t.currents[before..].to_vec());
    let created2 = tap
        .until_got(reopened.0, 10, "session.created on the new thread", |g| {
            is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
        })
        .await;
    let t2 = reopened.1;
    let m = until(&mut model, 10, "listening on in the new thread", |m| {
        m.voice.on && shown(m) == VoiceShown::Quiet
    })
    .await;
    let current_now =
        owners_folder(&lmgw, "Kai").await.unwrap()["ongoing"]["current_thread_id"].clone();
    let kai_log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let idle_lines = kai_log
        .lines()
        .filter(|l| l.contains("rolled_over=true") && l.contains("reason=Some(Idle)"))
        .count();
    r.line(format!(
        "4 idle rollover: the reply stored at 0; Kai's current calls after it: {:?} (ms after the reply: {:?}); \
         the rebind's open {} ms, its session.created {} ms after the reply: thread {t1} → {t2}; lmgw's current thread now {current_now}; \
         Kai's log: {idle_lines} current answer(s) with reason idle; tooltip title \"{}\"",
        roll.iter().map(|c| (c.2, c.3.clone())).collect::<Vec<_>>(),
        roll.iter().map(|c| ms(c.0.saturating_duration_since(a1))).collect::<Vec<_>>(),
        ms(reopened.0 - a1),
        ms(created2.at - a1),
        render(&m).title,
    ));
    assert_ne!(t2, t1);
    assert_eq!(current_now, json!(t2));
    assert!(roll.iter().any(|c| c.3 == Ok((t2, true))));
    assert!(ms(reopened.0 - a1) >= 59_000, "not before the idle minute");
    assert!(idle_lines >= 1);

    // ---- 5. a new conversation from the tray: current {new: true}, then the rebind
    press_turn(&cmds, &mut mic, &clip, &tap, &mut model).await;
    let asked = Instant::now();
    cmds.send(Command::NewConversation).unwrap();
    let reopened = loop {
        assert!(asked.elapsed() < Duration::from_secs(15), "no rebind");
        if tap.opens() >= 3 {
            break tap.with(|t| t.opens[2]);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let new = current_since(&tap, asked, "current {new: true}", |c| c.2).await;
    let t3 = reopened.1;
    r.line(format!(
        "5 new conversation: current {{new: true}} answered {} ms after the item ({:?}), the rebind's open {} ms after it: thread {t2} → {t3}",
        ms(new.0.saturating_duration_since(asked)),
        new.3,
        ms(reopened.0 - asked)
    ));
    assert!(new.2 && new.3 == Ok((t3, true)));
    tap.until_got(reopened.0, 10, "session.created on it", |g| {
        is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
    })
    .await;
    until(&mut model, 10, "quiet listening", |m| {
        m.voice.on && shown(m) == VoiceShown::Quiet
    })
    .await;

    // ---- 5b. the thread leaves the device's reach (api §1.6): the owner attaches lmgw's admin
    // tools to it; lmgw closes Kai's session with 4004, Kai asks for the folder's current thread
    // and binds it with its own bind, never "pair again".
    let opens = tap.opens();
    let asked = tap.with(|t| t.currents.len());
    let attach = Instant::now();
    lmgw.owner(
        "POST",
        &format!("/chat/api/threads/{t3}/settings"),
        Some(json!({"mcp_tools": [{"server_label": "lmgw"}]})),
    )
    .await;
    let closed = tap
        .until_got(attach, 10, "the session's close", |g| {
            matches!(g.event, SessionEvent::Closed { .. })
        })
        .await;
    let SessionEvent::Closed { code, reason } = closed.event.clone() else {
        unreachable!()
    };
    // Kai binds the folder's current thread again. lmgw closes with 4004 only once the attach is
    // live (api §1.6, lmgw a50141d), so every `current` answered after the close names another
    // thread, and Kai binds the first answer at once: one bind. The wait below is for that bind;
    // a bind to t3 would be found by the assertions after it.
    let (binds, (reopen_at, t5)) = loop {
        assert!(
            attach.elapsed() < Duration::from_secs(15),
            "no bind to another thread"
        );
        let binds: Vec<((Instant, i64), bool)> = tap.with(|t| {
            t.opens[opens..]
                .iter()
                .copied()
                .zip(t.take_overs[opens..].iter().copied())
                .collect()
        });
        if let Some(b) = binds.iter().find(|((_, t), _)| *t != t3) {
            break (binds.clone(), b.0);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let created5 = tap
        .until_got(
            reopen_at,
            10,
            "session.created on the folder's current thread",
            |g| is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_))),
        )
        .await;
    let m = until(
        &mut model,
        10,
        "listening on in the folder's current thread",
        |m| m.voice.on && shown(m) == VoiceShown::Quiet,
    )
    .await;
    // Settled: nothing more is bound.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let binds_now = tap.opens() - opens;
    let closes: Vec<(u16, String)> = tap.with(|t| {
        t.got
            .iter()
            .filter(|g| g.at >= attach)
            .filter_map(|g| match &g.event {
                SessionEvent::Closed { code, reason } => Some((*code, reason.clone())),
                _ => None,
            })
            .collect()
    });
    let after_close: Vec<_> = tap.with(|t| {
        t.currents[asked..]
            .iter()
            .filter(|c| c.0 >= closed.at)
            .cloned()
            .collect()
    });
    let (t3_status, _) = lmgw
        .call("GET", &format!("/chat/api/threads/{t3}"), None, &key)
        .await;
    let kai_log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let out_of_reach_lines = kai_log
        .lines()
        .filter(|l| l.contains("voice: the thread left this device's reach (close 4004)"))
        .count();
    while let Ok(n) = notices.try_recv() {
        seen.push((Instant::now(), n));
    }
    let since: Vec<String> = seen
        .iter()
        .filter(|(at, _)| *at >= attach)
        .map(|(_, n)| format!("{} | {}", n.summary, n.body))
        .collect();
    let lost: Vec<_> = seen
        .iter()
        .filter(|(at, n)| {
            *at >= attach
                && [
                    "Kai: pair again",
                    "Kai: this device is disabled",
                    "Kai: this device's key expired",
                ]
                .contains(&n.summary.as_str())
        })
        .map(|(_, n)| n.summary.clone())
        .collect();
    r.line(format!(
        "5b out of reach: the owner attached lmgw's admin tools to {t3}; Kai's session closed {} ms later \
         ({code}, \"{reason}\"); the closes since: {closes:?}; Kai's current calls after the first close: {:?} \
         ({:?} ms after it); Kai's binds since (thread, take_over): {:?}; the bind to {t5} {} ms after the \
         first close, its session.created {} ms after it; binds after settling: {binds_now}; {t3} for Kai's \
         key now: HTTP {t3_status}; Kai's log: {out_of_reach_lines} 4004 line(s); pairing still connected: {}; \
         notifications since the attach: {since:?}; tooltip notes: {:?}",
        ms(closed.at - attach),
        after_close.iter().map(|c| c.3.clone()).collect::<Vec<_>>(),
        after_close
            .iter()
            .map(|c| ms(c.0.saturating_duration_since(closed.at)))
            .collect::<Vec<_>>(),
        binds.iter().map(|((_, t), o)| (*t, *o)).collect::<Vec<_>>(),
        ms(reopen_at.saturating_duration_since(closed.at)),
        ms(created5.at.saturating_duration_since(closed.at)),
        matches!(
            m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        ),
        m.voice.notes,
    ));
    assert_eq!(
        code,
        lmgw_client::realtime::CLOSE_OUT_OF_REACH,
        "the close: {reason}"
    );
    assert_eq!(
        lmgw_client::realtime::close_kind(code, &reason),
        lmgw_client::realtime::CloseKind::OutOfReach
    );
    // lmgw's 4004s, and at most Kai's own normal close of a session it left at a quiet point.
    let out_of_reach = closes
        .iter()
        .filter(|(c, _)| *c == lmgw_client::realtime::CLOSE_OUT_OF_REACH)
        .count();
    assert!(
        closes
            .iter()
            .all(|(c, _)| *c == lmgw_client::realtime::CLOSE_OUT_OF_REACH || *c == 1000),
        "only 4004s and Kai's own closes: {closes:?}"
    );
    assert_eq!(
        out_of_reach_lines, out_of_reach,
        "Kai read every 4004 as one"
    );
    assert_eq!(out_of_reach, 1, "one 4004, after the attach: {closes:?}");
    assert!(
        after_close
            .iter()
            .all(|c| c.3.as_ref().is_ok_and(|(t, _)| *t == t5)),
        "every current answered after the close names {t5}, never {t3}: {after_close:?}"
    );
    assert_eq!(
        binds.len(),
        1,
        "Kai bound current's answer at once, and only it: {binds:?}"
    );
    assert!(
        after_close
            .iter()
            .any(|c| c.3.as_ref().is_ok_and(|(t, _)| *t == t5)),
        "Kai asked current after the close, and bound its answer"
    );
    assert_ne!(t5, t3);
    assert!(
        binds.iter().all(|(_, took)| !took),
        "Kai's own binds never take over (K5): {binds:?}"
    );
    assert_eq!(
        binds_now,
        binds.iter().position(|((_, t), _)| *t == t5).unwrap() + 1,
        "nothing bound after {t5}"
    );
    assert_eq!(t3_status, 404, "{t3} is out of Kai's reach");
    assert!(matches!(
        m.pairing,
        Pairing::Paired {
            link: Link::Up { .. },
            ..
        }
    ));
    assert!(lost.is_empty(), "never pair again for a 4004: {lost:?}");
    // lmgw's `error chat_thread_not_found` before its 4004 is no notification (review KAI-4 K4-4).
    assert!(
        !since.iter().any(|n| n.starts_with("Kai: voice error |")),
        "no voice error for a 4004: {since:?}"
    );
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("left this device's reach")),
        "{:?}",
        m.voice.notes
    );
    // Step 6 runs in the thread Kai bound again.
    let t3 = t5;

    // ---- 6. K5: another device rolls over after the idle time and binds the new thread first
    let a3 = press_turn(&cmds, &mut mic, &clip, &tap, &mut model).await;
    let opens = tap.opens();
    tokio::time::sleep_until((a3 + Duration::from_secs(50)).into()).await;
    // Kai's talk key is down (not a quiet point) across the idle minute.
    let hold = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    mic.say(&clip);
    tokio::time::sleep_until((a3 + Duration::from_secs(64)).into()).await;
    let (status, text) = lmgw
        .call(
            "POST",
            &format!("/chat/api/folders/{folder}/current"),
            Some(json!({})),
            &key2,
        )
        .await;
    let rolled2: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let roll2_at = Instant::now();
    assert_eq!(status, 200, "{text}");
    let t4 = rolled2["thread"]["id"].as_i64().unwrap();
    let mut req = format!("ws://127.0.0.1:8897/v1/realtime?chat_thread={t4}")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {key2}").parse().unwrap());
    let (mut other, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("the second device binds");
    loop {
        match tokio::time::timeout(Duration::from_secs(10), other.next()).await {
            Ok(Some(Ok(Message::Text(t)))) if t.contains("\"session.created\"") => break,
            Ok(Some(Ok(_))) => {}
            other => panic!("the second device's session: {other:?}"),
        }
    }
    let bound2 = Instant::now();
    let m = until(&mut model, 10, "Kai sees the other device's voice", |m| {
        m.voice
            .notes
            .iter()
            .any(|n| n == "voice in use on device 'kai-live-2'")
    })
    .await;
    let pending_note = m.voice.notes.clone();
    let release = Instant::now();
    cmds.send(Command::Talk(false)).unwrap();
    let (stayed_at, stayed) =
        notice(&mut notices, &mut seen, "Kai: voice in use elsewhere", 60).await;
    let m = until(&mut model, 10, "listening off", |m| !m.voice.on).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let new_opens = tap.opens() - opens;
    let still = tokio::time::timeout(Duration::from_millis(500), other.next()).await;
    let other_open = !matches!(still, Ok(Some(Ok(Message::Close(_)))) | Ok(None));
    let t3_msgs = lmgw.messages(t3).await;
    r.line(format!(
        "6 K5: the other device's current {} ms after Kai's last reply in {t3} (the key held since {} ms): HTTP {status}, thread {t4}, rolled over {}, reason {}; \
         it bound {t4} {} ms later; Kai's notes then: {pending_note:?}; the release at {} ms; \"{}\" notified {} ms after the release: \"{}\"; \
         Kai's sessions opened since: {new_opens}; listening off: {}; the other device's session still open: {other_open}; \
         messages in {t3}: {} (the held turn's included); tooltip: {:?}",
        ms(roll2_at - a3),
        ms(hold - a3),
        rolled2["rolled_over"],
        rolled2["reason"],
        ms(bound2 - roll2_at),
        ms(release - a3),
        stayed.summary,
        ms(stayed_at.saturating_duration_since(release)),
        stayed.body,
        !m.voice.on,
        t3_msgs.len(),
        m.voice.notes,
    ));
    assert_eq!(rolled2["rolled_over"], true);
    assert_eq!(rolled2["reason"], "idle");
    assert_ne!(t4, t3);
    assert!(
        stayed.body.contains("device 'kai-live-2'"),
        "{}",
        stayed.body
    );
    assert_eq!(
        new_opens, 0,
        "K5: Kai never binds the thread the other device holds"
    );
    assert!(!m.voice.on);
    assert!(other_open, "Kai took nothing over");
    assert!(t3_msgs.len() >= 4, "two turns in {t3}");
    other.send(Message::Close(None)).await.unwrap();
    while let Some(Ok(_)) = other.next().await {}

    // ---- the end: a clean quit, the dev lmgw stopped
    for (at, n) in seen.drain(..) {
        r.line(format!(
            "notification (+{} ms): {} | {}",
            ms(at - t0),
            n.summary,
            n.body
        ));
    }
    while let Ok(n) = notices.try_recv() {
        r.line(format!("notification: {} | {}", n.summary, n.body));
    }
    cmds.send(Command::Quit).unwrap();
    task.await.unwrap();
    drop(mic);
    mic_audio.shutdown(Duration::from_secs(4)).await;
    let how = lmgw.stop().await;
    r.line(format!("Kai quit; the dev lmgw {how}"));
    r.line(super::lmgw_live_tests::log_is_clean(
        &log_path,
        &[&key, &key2],
    ));
    std::fs::write(out_dir().join("report.txt"), &r.0).unwrap();
}
