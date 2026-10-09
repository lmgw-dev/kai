//! WP18's live check (`scripts/live-check-jobs.sh`, `just live-check-jobs`): Kai's core with its
//! real feed, realtime adapter and hosted server against a dev lmgw on 127.0.0.1:8897 whose chat is
//! lmgw's `scripts/mock-openai.py` with `TOOL_SCRIPT` (each turn calls the live-test build's
//! `live_slow`, K26's open-ended class, then echoes the tool's result) and whose speech-to-text and
//! text-to-speech are `scripts/mock-voice-audio.py`, Kai's audio in a private PipeWire instance
//! (nothing audible). Ignored by default; it refuses to run outside that instance or against
//! another gateway.
//!
//! 1. **A bound session speaks a job** (ruling 22, K26): a push-to-talk turn in Kai's
//!    conversation calls `kailive__live_slow` (5 s) through lmgw's host link as an MCP task; the
//!    turn answers at once (`started, job <id>`); the job's result enters the thread as a `tool`
//!    row; the bound session says `lmgw.task.done`; after `jobs.quiet_ms` of quiet Kai sends one
//!    tagged `response.create` with no new words, and the continuation's reply (the mock's echo of
//!    the result) is spoken and stored after the result row.
//! 2. **No session: Kai binds by itself**: a second turn starts a 40 s job; the press session
//!    closes after `voice.grace_s` (15 s); the feed's `task.done` makes Kai bind its conversation
//!    (`takeover=never`) and speak it.
//! 3. **Announce off**: a job the owner starts in a thread of their own is notified ("Kai: a job
//!    ended"), never bound or spoken.
//! 4. **lmgw's cancel**, in that thread after its unanswered result (lmgw's chronological order,
//!    T11: the send is the model's last message, so it calls the tool again): the owner cancels a
//!    600 s job through `POST …/tasks/{task}/cancel`; lmgw sends `tasks/cancel`, Kai's job ends
//!    cancelled at once, and the thread gets its result.
//! 5. **A deleted device's job** (device.revoked): a second paired device, the phone, starts a
//!    600 s job in a thread of its own; the owner deletes its key; lmgw tells Kai, the host of the
//!    open task, and Kai ends the job.
//! 6. No device key and no tool argument in Kai's log.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use super::Notice;
use super::link::ServerEvent;
use super::lmgw_live_tests::{
    Lmgw, Mic, Report, Tap, Tapped, URL, env, is_server, log_is_clean, notice, set_up_mocks,
};
use crate::audio::live_tests::{MIC, MIC_IN, SINK, out_dir, private_or_skip, read_wav, settings};
use crate::audio::{Audio, RATE};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::{FeedLink, FeedSettings, RealtimeLink, RealtimeSettings};
use crate::pairing::DeviceKey;
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{Link, Pairing, TrayModel};

const LABEL: &str = "kailive";

/// The thread as the owner reads it.
async fn thread(lmgw: &Lmgw, id: i64) -> Value {
    lmgw.owner("GET", &format!("/chat/api/threads/{id}"), None)
        .await
}

/// Waits until thread `id` satisfies `f`; the thread then.
async fn until_thread(
    lmgw: &Lmgw,
    id: i64,
    secs: u64,
    what: &str,
    f: impl Fn(&Value) -> bool,
) -> Value {
    let t0 = Instant::now();
    loop {
        let t = thread(lmgw, id).await;
        if f(&t) {
            return t;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "thread {id} never got to: {what} (waited {secs} s): {t}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The thread's messages: (role, content).
fn messages(t: &Value) -> Vec<(String, String)> {
    t["messages"]
        .as_array()
        .map(|m| {
            m.iter()
                .map(|m| {
                    (
                        m["role"].as_str().unwrap_or_default().to_owned(),
                        m["content"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `tool` row whose content holds `marker`, and an assistant reply after it holding it too (the
/// mock's echo of the result).
fn answered(t: &Value, marker: &str) -> bool {
    let m = messages(t);
    let Some(at) = m
        .iter()
        .position(|(role, c)| role == "tool" && c.contains(marker))
    else {
        return false;
    };
    m[at + 1..]
        .iter()
        .any(|(role, c)| role == "assistant" && c.contains(marker))
}

/// The thread's settings: Kai's tools.
async fn with_kai_tools(lmgw: &Lmgw, id: i64) {
    lmgw.owner(
        "POST",
        &format!("/chat/api/threads/{id}/settings"),
        Some(json!({"mcp_tools": [{"server_label": LABEL}]})),
    )
    .await;
}

async fn press_turn(cmds: &mpsc::UnboundedSender<Command>, mic: &mut Mic, clip: &[i16]) {
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let said = mic.say(clip);
    tokio::time::sleep_until((said + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
}

/// The tagged `response.create`s Kai sent for jobs since `since`.
fn job_creates(tap: &Tap, since: Instant) -> Vec<(Instant, String)> {
    let mut out = Vec::new();
    let mut from = since;
    while let Some(s) = tap.sent_after(from, |s| {
        s.kind == "response.create" && s.tag.as_deref().is_some_and(|t| t.starts_with("kai_job_"))
    }) {
        out.push((s.at, s.tag.clone().unwrap_or_default()));
        from = s.at + Duration::from_nanos(1);
    }
    out
}

/// The `response.done` of the first response created after `since`, waited for: its status.
async fn response_done_after(tap: &Tap, since: Instant, secs: u64) -> String {
    let created = tap
        .until_got(since, secs, "response.created", |g| {
            is_server(g, |e| matches!(e, ServerEvent::ResponseCreated { .. }))
        })
        .await;
    let rid = match &created.event {
        super::link::SessionEvent::Server(ServerEvent::ResponseCreated { response_id }) => {
            response_id.clone()
        }
        _ => unreachable!(),
    };
    let done = tap
        .until_got(created.at, secs, "its response.done", |g| {
            is_server(g, |e| {
                matches!(e, ServerEvent::ResponseDone { response_id, .. } if *response_id == rid)
            })
        })
        .await;
    match done.event {
        super::link::SessionEvent::Server(ServerEvent::ResponseDone { status, .. }) => status,
        _ => unreachable!(),
    }
}

/// Kai's log lines holding `what`.
fn logged(log: &std::path::Path, what: &str) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(what))
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/live-check-jobs.sh (a private PipeWire instance and a dev lmgw)"]
async fn live_jobs() {
    if !private_or_skip() {
        return;
    }
    assert_eq!(
        std::env::var("KAI_LIVE_LMGW_URL").as_deref(),
        Ok(URL),
        "only the dev lmgw on 127.0.0.1:8897"
    );
    let out: PathBuf = out_dir();
    let log_path = out.join("kai.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "trace".into()),
        Mutex::new(log),
        false,
    ));
    let mut r = Report(String::new());
    let clip = read_wav(&env("KAI_LIVE_CLIP"));
    let clip = &clip[..clip.len().min(RATE as usize * 2)];
    let marks: Vec<String> = env("KAI_LIVE_JOB_MARKS")
        .split(',')
        .map(str::to_owned)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut lmgw = Lmgw::new();
    let boot = lmgw.start().await;
    set_up_mocks(&lmgw).await;
    let folder = lmgw
        .owner(
            "POST",
            "/chat/api/folders",
            Some(
                json!({"name": "Kai", "defaults": {"model_alias": "kai-chat"},
                "ongoing": {"idle_minutes": 0}}),
            ),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    let key = lmgw
        .op(
            "key_create",
            json!({"kind": "device", "name": "kai-live", "url": URL, "hosts_label": LABEL}),
        )
        .await["key"]
        .as_str()
        .unwrap()
        .to_owned();
    r.line(format!(
        "dev lmgw up in {} ms; device key kai-live (hosts {LABEL}); folder {folder}",
        boot.as_millis()
    ));

    // Kai: the core with the hosted server, its audio in the private PipeWire instance.
    let mut s = settings(dir.path());
    s.input = Some(MIC.into());
    s.output = Some(SINK.into());
    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let audio = Audio::start(s, audio_tx).unwrap();
    let mut sb = settings(&dir.path().join("feeder"));
    sb.output = Some(MIC_IN.into());
    let (btx, _brx) = mpsc::unbounded_channel();
    let mic_audio = Audio::start(sb, btx).unwrap();
    let mut mic = Mic::new(&mic_audio);
    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        format!(
            "[lmgw]\nurl = \"{URL}\"\nretry_max_s = 2\n[audio]\ninput = \"{MIC}\"\noutput = \"{SINK}\"\n\
             [conversation]\nfolder_id = {folder}\n[voice]\ngrace_s = 15\nmode = \"push_to_talk\"\n\
             [shortcuts]\nbind = false\n[jobs]\nquiet_ms = 1500\n\
             [policy]\ntrusted_callers = [\"device:phone\"]\n[policy.tools]\nlive_slow = \"allow\"\n"
        ),
    )
    .unwrap();
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
    let screen = crate::screen_lock::ScreenLock::default();
    let (host, policy_notes) = crate::hosted_server(
        &config,
        cmds.clone(),
        ui_tx.clone(),
        crate::host::registry::SessionBus::lazy(config.dbus.timeout()),
        screen.clone(),
        crate::desktop::fdo::atspi::switch::A11ySwitch::inert(),
    );
    let jobs = host.jobs();
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
    let (ntx, mut notices) = mpsc::unbounded_channel::<Notice>();
    core.notify_into(ntx);
    core.set_jobs(jobs.clone());
    core.set_host(Box::new(host), policy_notes);
    core.set_desktop_state(
        screen.clone(),
        crate::desktop::fdo::atspi::switch::A11ySwitch::inert(),
    );
    let task = tokio::spawn(core.run(cmd_rx));
    let linked = |m: &TrayModel| {
        matches!(
            &m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        ) && m
            .tools
            .iter()
            .any(|t| t.contains(&format!("linked as {LABEL}")))
    };
    tokio::time::timeout(Duration::from_secs(30), model.wait_for(linked))
        .await
        .expect("the feed up and the tools linked")
        .unwrap();
    // Kai's conversation's thread, which `current` names at the connected start.
    let t0 = Instant::now();
    let conv = loop {
        if let Some(id) = tap.with(|t| t.currents.last().and_then(|c| c.3.clone().ok())) {
            break id.0;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "no current");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    with_kai_tools(&lmgw, conv).await;
    r.line(format!(
        "the feed is up, the tools are linked as {LABEL}; Kai's conversation is thread {conv}, \
         with Kai's tools"
    ));
    let mut seen_notices = Vec::new();

    // ---- 1. a bound session speaks a job
    let opens = tap.opens();
    let press = Instant::now();
    press_turn(&cmds, &mut mic, clip).await;
    until_thread(&lmgw, conv, 30, "the turn's started, job", |t| {
        t.to_string().contains("started, job ")
    })
    .await;
    let job1 = logged(&log_path, "jobs: started")
        .last()
        .cloned()
        .expect("Kai's job started");
    assert!(job1.contains("announce=true"), "{job1}");
    let task_done = tap
        .until_got(press, 60, "lmgw.task.done", |g| {
            is_server(g, |e| matches!(e, ServerEvent::TaskDone(_)))
        })
        .await;
    let creates = loop {
        let c = job_creates(&tap, press);
        if !c.is_empty() {
            break c;
        }
        assert!(
            task_done.at.elapsed() < Duration::from_secs(30),
            "no job create after lmgw.task.done"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let waited = creates[0].0.duration_since(task_done.at);
    let commits_between = tap
        .sent_after(task_done.at, |s| s.kind == "commit" && s.at < creates[0].0)
        .is_some();
    assert!(!commits_between, "a bare create: no commit before it");
    assert!(
        waited >= Duration::from_millis(1400),
        "the quiet wait: {waited:?}"
    );
    let status = response_done_after(&tap, creates[0].0, 60).await;
    assert_eq!(
        status, "completed",
        "the continuation's reply played to its end"
    );
    let t = until_thread(&lmgw, conv, 60, "the continuation's reply", |t| {
        answered(t, &marks[0])
    })
    .await;
    let rows = messages(&t);
    // Its audio heard to the end before the next press.
    tokio::time::sleep(Duration::from_secs(3)).await;
    r.line(format!(
        "1 a press turn called {LABEL}__live_slow (5 s) as a job ({}); lmgw.task.done came, Kai's \
         {} went {} ms later with no new words, and the continuation's reply holds the result \
         after its tool row ({} messages in the thread)",
        job1.split("jobs: started")
            .nth(1)
            .unwrap_or_default()
            .trim(),
        creates[0].1,
        waited.as_millis(),
        rows.len()
    ));
    assert_eq!(tap.opens(), opens + 1, "one session");

    // ---- 2. no session: Kai binds by itself
    let press2 = Instant::now();
    press_turn(&cmds, &mut mic, clip).await;
    let t0 = Instant::now();
    while logged(&log_path, "jobs: started").len() < 2 {
        assert!(t0.elapsed() < Duration::from_secs(30), "no second job");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let closed = tap
        .until_sent(press2, 60, "the grace close", |s| s.kind == "close")
        .await;
    let opens2 = tap.opens();
    r.line(format!(
        "2 a 40 s job started; the press session closed after its grace ({} ms after the press)",
        closed.at.duration_since(press2).as_millis()
    ));
    let t0 = Instant::now();
    while tap.opens() == opens2 {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "Kai never bound for the result"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let bound_at = Instant::now();
    let take_over = tap.with(|t| t.take_overs.last().copied());
    assert_eq!(take_over, Some(false), "takeover=never (K5)");
    let t = until_thread(&lmgw, conv, 60, "the announced reply", |t| {
        answered(t, &marks[1])
    })
    .await;
    let c2 = job_creates(&tap, bound_at);
    assert_eq!(c2.len(), 1, "one create: {c2:?}");
    assert_eq!(response_done_after(&tap, c2[0].0, 60).await, "completed");
    r.line(format!(
        "2 the result entered with no session: Kai bound thread {conv} itself (takeover=never), \
         sent {} {} ms after the bind, and the reply holds the result ({} messages)",
        c2[0].1,
        c2[0].0.duration_since(bound_at).as_millis(),
        messages(&t).len()
    ));

    // ---- 3. announce off: the owner's job is notified
    let own_thread = lmgw
        .owner(
            "POST",
            "/chat/api/threads",
            Some(json!({"model_alias": "kai-chat"})),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    with_kai_tools(&lmgw, own_thread).await;
    let opens3 = tap.opens();
    let (st, _) = lmgw
        .call(
            "POST",
            &format!("/chat/api/threads/{own_thread}/send"),
            Some(json!({"content": "run the slow one"})),
            &lmgw.token,
        )
        .await;
    assert_eq!(st, 200);
    until_thread(&lmgw, own_thread, 60, "the owner's job result", |t| {
        messages(t)
            .iter()
            .any(|(role, c)| role == "tool" && c.contains(&marks[2]))
    })
    .await;
    let (_, n) = notice(&mut notices, &mut seen_notices, "Kai: a job ended", 30).await;
    assert!(n.body.contains("live_slow completed"), "{n:?}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(tap.opens(), opens3, "no bind for an announce-off job");
    r.line(format!(
        "3 the owner's job (announce off) entered thread {own_thread}: notified (\"{}\"), no bind",
        n.body
    ));

    // ---- 4. lmgw's cancel, in the same thread: its newest row is 3's result, which nothing
    // answered (announce off). lmgw renders a late result where it was stored (T11, chronological
    // order), so this send is the history's last message and the mock calls the tool again; an
    // lmgw that placed the owed result after the send would have the mock echo it instead, and no
    // task would start.
    let (st, _) = lmgw
        .call(
            "POST",
            &format!("/chat/api/threads/{own_thread}/send"),
            Some(json!({"content": "run the long one"})),
            &lmgw.token,
        )
        .await;
    assert_eq!(st, 200);
    let running = |t: &Value| {
        t["tasks"]
            .as_array()
            .and_then(|a| a.iter().find(|k| k["status"] == "working"))
            .and_then(|k| k["id"].as_i64())
    };
    let t = until_thread(&lmgw, own_thread, 30, "the open task", |t| {
        running(t).is_some()
    })
    .await;
    let task_id = running(&t).unwrap();
    r.line(format!(
        "4 a send after an unanswered result in the same thread (T11, chronological): the model \
         was asked with the send last and called the tool again ({} messages)",
        messages(&t).len()
    ));
    assert!(jobs.list().iter().any(|j| j.running), "Kai's job runs");
    let cancel_at = Instant::now();
    let answer = lmgw
        .owner(
            "POST",
            &format!("/chat/api/threads/{own_thread}/tasks/{task_id}/cancel"),
            Some(json!({})),
        )
        .await;
    let t = until_thread(&lmgw, own_thread, 30, "the cancelled result", |t| {
        messages(t)
            .iter()
            .any(|(role, c)| role == "tool" && c.contains("cancelled"))
    })
    .await;
    assert!(
        !jobs.list().iter().any(|j| j.running),
        "Kai's job ended: {:?}",
        jobs.list()
    );
    assert!(
        logged(&log_path, "jobs: ended")
            .iter()
            .any(|l| l.contains("cancelled")),
        "Kai's log has the cancelled job"
    );
    r.line(format!(
        "4 the owner's cancel in lmgw ({answer}) reached Kai as tasks/cancel: the job ended \
         cancelled, its result entered the thread {} ms after the cancel ({} messages)",
        cancel_at.elapsed().as_millis(),
        messages(&t).len()
    ));

    // ---- 5. a deleted device's job: the phone (a second paired device) runs a 600 s job in a
    // thread of its own; the owner deletes the phone's key; lmgw tells Kai, the host of the open
    // task, `device.revoked`, and Kai ends the job. Kai's own key stays.
    let phone = lmgw
        .op(
            "key_create",
            json!({"kind": "device", "name": "phone", "url": URL,
                "tool_scope_mode": "allow", "tool_scope_patterns": format!("{LABEL}__*")}),
        )
        .await;
    let phone_key = phone["key"].as_str().unwrap().to_owned();
    let phone_id = phone["id"].as_i64().unwrap();
    let (st, text) = lmgw
        .call(
            "POST",
            "/chat/api/threads",
            Some(json!({"model_alias": "kai-chat"})),
            &phone_key,
        )
        .await;
    assert_eq!(st, 200, "the phone creates a thread: {text}");
    let phone_thread = serde_json::from_str::<Value>(&text).unwrap()["id"]
        .as_i64()
        .unwrap();
    // The thread's tools: the phone's tool scope allows Kai's label, the owner sets them.
    with_kai_tools(&lmgw, phone_thread).await;
    let (st, text) = lmgw
        .call(
            "POST",
            &format!("/chat/api/threads/{phone_thread}/send"),
            Some(json!({"content": "run the long one"})),
            &phone_key,
        )
        .await;
    assert_eq!(st, 200, "the phone sends: {text}");
    until_thread(&lmgw, phone_thread, 30, "the phone's open task", |t| {
        running(t).is_some()
    })
    .await;
    assert!(
        jobs.list().iter().any(|j| j.running),
        "Kai's job for the phone runs"
    );
    let delete_at = Instant::now();
    lmgw.op("key_delete", json!({"id": phone_id})).await;
    let t0 = Instant::now();
    while jobs.list().iter().any(|j| j.running) {
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "device.revoked did not end the phone's job: {:?}",
            jobs.list()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        logged(&log_path, "the device that started it was deleted")
            .iter()
            .any(|l| l.contains("phone")),
        "Kai's log has the deleted device's job"
    );
    r.line(format!(
        "5 the phone's key deleted in lmgw: device.revoked reached Kai, the host of its open \
         task, and its job ended cancelled {} ms later",
        delete_at.elapsed().as_millis()
    ));

    // ---- 6. the log and the quit
    cmds.send(Command::Quit).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(20), task).await;
    lmgw.stop().await;
    let mut line = log_is_clean(&log_path, &[&key, &phone_key]);
    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    for m in &marks {
        assert!(
            !text.contains(m.as_str()),
            "a tool argument is in Kai's log"
        );
    }
    line.push_str("; no tool argument");
    r.line(format!("6 {line}"));
    std::fs::write(out.join("report.txt"), &r.0).unwrap();
}
