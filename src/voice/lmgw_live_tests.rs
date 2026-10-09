//! The voice session against a real lmgw (§15 WP4, the real-lmgw half): Kai's core with real
//! PipeWire streams in the private instance, the real feed and the realtime adapter
//! (`lmgw_link::RealtimeLink`), and a dev lmgw on 127.0.0.1:8897 whose chat, speech-to-text and
//! text-to-speech are mocks: no model, no GPU. `scripts/live-check-voice-lmgw.sh`
//! (`just live-check-voice-lmgw`) prepares it and runs it; ignored by default, and it refuses to
//! run outside the private PipeWire instance or against any other gateway.
//!
//! The test starts and stops the dev lmgw itself (a restart is one of the checks), through the
//! launcher the script wrote. The owner's side goes through the dev lmgw's `/api` and Chat API with
//! the owner bearer from its log line; "the dashboard" is that bearer. The device key lives in
//! memory only (no wallet). Speech is a TTS clip played into the private test microphone; the
//! replies' audio is a TTS clip too, played on the private test sink, whose monitor is the ground
//! truth of what was played.
//!
//! The checks, each with its numbers in `report.txt`:
//! 1. push-to-talk round trips: a press with listening off (a cold bind, the capture buffered until
//!    `session.created`), then push-to-talk mode with listening on;
//! 2. an automatic round trip (the thread's `semantic_vad`);
//! 3. a barge-in about 1.5 s into a reply leaves the heard text in the thread;
//! 4. a dashboard text turn silences automatic listening until it is done (K16);
//! 5. a dashboard bind takes over: the notification names the dashboard, Kai does not reconnect;
//! 6. a dev lmgw restart turns listening off with a notification that says lmgw is stopping or
//!    restarting (its 1001 close, not a lost connection), and Kai does not reconnect.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::Notice;
use super::link::{
    ClientEvent, Current, FolderInfo, Pending, Refusal, ServerEvent, SessionEvent, VoiceLink,
    VoiceSocket,
};
use crate::audio::live_tests::{
    MIC, MIC_IN, Monitor, SINK, align, out_dir, private_or_skip, read_wav, settings,
};
use crate::audio::test_support::monotonic_ns;
use crate::audio::{Audio, Player, RATE};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::{Credentials, FeedLink, FeedSettings, RealtimeLink, RealtimeSettings};
use crate::pairing::DeviceKey;
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{Link, Pairing, TrayModel, VoiceShown};
use crate::voice::VoiceMode;

pub(super) const URL: &str = "http://127.0.0.1:8897";
pub(super) const ASR_TEXT: &str =
    "Wie wird das Wetter morgen in Berlin? Und was ziehe ich am besten an?";

pub(super) fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} (set by scripts/live-check-voice-lmgw.sh)"))
}

// ------------------------------------------------------------------------ the dev lmgw

/// The dev lmgw: started and stopped through the script's launcher; its owner bearer.
pub(super) struct Lmgw {
    run: String,
    log: PathBuf,
    pid_file: PathBuf,
    child: Option<tokio::process::Child>,
    pub(super) token: String,
    http: reqwest::Client,
}

impl Lmgw {
    pub(super) fn new() -> Self {
        Self {
            run: env("KAI_LIVE_LMGW_RUN"),
            log: PathBuf::from(env("KAI_LIVE_LMGW_LOG")),
            pid_file: PathBuf::from(env("KAI_LIVE_LMGW_PID")),
            child: None,
            token: String::new(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    /// Starts it and waits until it answers; returns how long that took.
    pub(super) async fn start(&mut self) -> Duration {
        let t0 = Instant::now();
        let mut child = tokio::process::Command::new("bash")
            .arg(&self.run)
            .kill_on_drop(true)
            .spawn()
            .expect("the launcher starts");
        std::fs::write(&self.pid_file, child.id().unwrap().to_string()).unwrap();
        loop {
            assert!(
                t0.elapsed() < Duration::from_secs(60),
                "the dev lmgw did not answer in 60 s"
            );
            if let Ok(Some(st)) = child.try_wait() {
                panic!("the dev lmgw exited: {st} (see {})", self.log.display());
            }
            let r = self
                .http
                .get(format!("{URL}/v1/models"))
                .timeout(Duration::from_secs(1))
                .send()
                .await;
            if r.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.child = Some(child);
        let log = std::fs::read_to_string(&self.log).unwrap();
        let at = log
            .rfind("lmgw-owner-")
            .expect("the owner bearer in the dev lmgw's log");
        self.token = log[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        t0.elapsed()
    }

    /// SIGINT (its graceful quit) now; the task says how it ended (killed after 15 s).
    pub(super) fn interrupt(&mut self) -> tokio::task::JoinHandle<String> {
        let mut child = self.child.take().expect("the dev lmgw runs");
        let pid = child.id().expect("running") as i32;
        // SAFETY: a signal to the dev lmgw this test started.
        unsafe { libc::kill(pid, libc::SIGINT) };
        let t0 = Instant::now();
        tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
                Ok(st) => format!(
                    "quit {} ms after SIGINT ({})",
                    t0.elapsed().as_millis(),
                    st.unwrap()
                ),
                Err(_) => {
                    child.kill().await.unwrap();
                    "did not quit within 15 s of SIGINT; killed".to_owned()
                }
            }
        })
    }

    pub(super) async fn stop(&mut self) -> String {
        if self.child.is_none() {
            return "was not running".into();
        }
        self.interrupt().await.unwrap()
    }

    pub(super) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        bearer: &str,
    ) -> (u16, String) {
        let mut rb = match method {
            "GET" => self.http.get(format!("{URL}{path}")),
            _ => self.http.post(format!("{URL}{path}")),
        };
        rb = rb.bearer_auth(bearer).timeout(Duration::from_secs(60));
        if let Some(b) = body {
            rb = rb
                .header("content-type", "application/json")
                .body(b.to_string());
        }
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.text().await.unwrap())
    }

    /// As the owner (the dashboard); a JSON answer.
    pub(super) async fn owner(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, text) = self.call(method, path, body, &self.token).await;
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {text}"
        );
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    }

    pub(super) async fn op(&self, name: &str, body: Value) -> Value {
        let v = self
            .owner("POST", &format!("/api/op/{name}"), Some(body))
            .await;
        assert_eq!(v["ok"], true, "{name}: {v}");
        v
    }

    /// A thread's messages as the owner reads them: (id, role, content, voice).
    pub(super) async fn messages(&self, thread: i64) -> Vec<(i64, String, String, Value)> {
        let v = self
            .owner("GET", &format!("/chat/api/threads/{thread}"), None)
            .await;
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["id"].as_i64().unwrap(),
                    m["role"].as_str().unwrap().to_owned(),
                    m["content"].as_str().unwrap_or_default().to_owned(),
                    m["voice"].clone(),
                )
            })
            .collect()
    }
}

/// The mock upstreams, the aliases and the Chat's voice settings; Kai's folder (ongoing, its
/// model the mock's) and Kai's device key. Returns (folder id, device key).
async fn set_up(lmgw: &Lmgw) -> (i64, String) {
    set_up_mocks(lmgw).await;
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
    (folder, device_key(lmgw, "kai-live").await)
}

/// A device key named `name` for the dev lmgw.
pub(super) async fn device_key(lmgw: &Lmgw, name: &str) -> String {
    lmgw.op(
        "key_create",
        json!({"kind": "device", "name": name, "url": URL}),
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The mock upstreams, the aliases `kai-chat`, `kai-asr` and `kai-tts`, and the Chat's voice
/// settings.
pub(super) async fn set_up_mocks(lmgw: &Lmgw) {
    let chat = env("KAI_LIVE_MOCK_CHAT");
    let audio = env("KAI_LIVE_MOCK_AUDIO");
    lmgw.op(
        "upstream_set",
        json!({"action": "create", "name": "mock-chat", "protocol": "llama_cpp",
            "base_url": format!("http://127.0.0.1:{chat}/v1")}),
    )
    .await;
    lmgw.op(
        "model_set",
        json!({"action": "create", "alias": "kai-chat", "upstream": "mock-chat",
            "upstream_model": "triage"}),
    )
    .await;
    let up = lmgw
        .op(
            "upstream_set",
            json!({"action": "create", "name": "mock-audio", "protocol": "openai",
                "kind": "audio_cpp", "base_url": format!("http://127.0.0.1:{audio}/v1")}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    for (alias, task, endpoint, model) in [
        ("kai-asr", "asr", "/v1/audio/transcriptions", "mock-asr"),
        ("kai-tts", "tts", "/v1/audio/speech", "mock-tts"),
    ] {
        lmgw.op(
            "alias_set",
            json!({"action": "create", "alias": alias, "upstream_id": up, "upstream_model": model,
                "capabilities_override": {"capabilities": {"task": task, "endpoints": [endpoint],
                "source": "owner"}}}),
        )
        .await;
    }
    lmgw.op(
        "settings_set",
        json!({"chat_stt_alias": "kai-asr", "chat_tts_alias": "kai-tts", "chat_voice": "alba"}),
    )
    .await;
}

// ------------------------------------------------------------------------ the tap

/// A client event as sent: when, on which socket, its kind; an append's level.
#[derive(Debug, Clone)]
pub(super) struct Sent {
    pub(super) socket: usize,
    pub(super) at: Instant,
    pub(super) kind: &'static str,
    /// An append's peak sample (0: silence).
    pub(super) peak: i32,
    pub(super) samples: usize,
    pub(super) event: Option<ClientEvent>,
    /// The `event_id` it was tagged with (`VoiceSocket::send_tagged`).
    pub(super) tag: Option<String>,
}

/// A session event as Kai got it (an audio delta's samples dropped, their count kept).
#[derive(Debug, Clone)]
pub(super) struct Got {
    pub(super) at: Instant,
    pub(super) event: SessionEvent,
}

/// One `current` call: when it answered, its folder and `new`, and the thread and `rolled_over`,
/// or the refusal.
pub(super) type CurrentCall = (Instant, i64, bool, Result<(i64, bool), String>);

#[derive(Default)]
pub(super) struct TapState {
    pub(super) opens: Vec<(Instant, i64)>,
    /// Each open's `take_over` (Kai's own binds never take over, K2-13).
    pub(super) take_overs: Vec<bool>,
    sent: Vec<Sent>,
    pub(super) got: Vec<Got>,
    pub(super) currents: Vec<CurrentCall>,
    /// `…/transcribe` requests (WP19's spoken answers): when, and the thread.
    pub(super) transcribes: Vec<(Instant, i64)>,
    /// Each assistant item's first second of audio as lmgw sent it (lmgw trims a clause's
    /// leading silence, so the item's audio, not the TTS clip, places it on the sink).
    heads: std::collections::HashMap<String, Vec<i16>>,
}

/// The real link with every event both ways recorded.
#[derive(Clone, Default)]
pub(super) struct Tap(Arc<Mutex<TapState>>);

impl Tap {
    pub(super) fn with<R>(&self, f: impl FnOnce(&TapState) -> R) -> R {
        f(&self.0.lock().unwrap())
    }

    pub(super) fn opens(&self) -> usize {
        self.with(|t| t.opens.len())
    }

    pub(super) fn got_after(&self, since: Instant, f: impl Fn(&Got) -> bool) -> Option<Got> {
        self.with(|t| t.got.iter().find(|g| g.at >= since && f(g)).cloned())
    }

    pub(super) fn sent_after(&self, since: Instant, f: impl Fn(&Sent) -> bool) -> Option<Sent> {
        self.with(|t| t.sent.iter().find(|s| s.at >= since && f(s)).cloned())
    }

    pub(super) async fn until_got(
        &self,
        since: Instant,
        secs: u64,
        what: &str,
        f: impl Fn(&Got) -> bool,
    ) -> Got {
        let t0 = Instant::now();
        loop {
            if let Some(g) = self.got_after(since, &f) {
                return g;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "never got: {what} (waited {secs} s)"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn until_sent(
        &self,
        since: Instant,
        secs: u64,
        what: &str,
        f: impl Fn(&Sent) -> bool,
    ) -> Sent {
        let t0 = Instant::now();
        loop {
            if let Some(s) = self.sent_after(since, &f) {
                return s;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "never sent: {what} (waited {secs} s)"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The appends sent in [from, to): how many, how many carried sound, the loudest.
    fn appends(&self, from: Instant, to: Instant) -> (usize, usize, i32) {
        self.with(|t| {
            let a: Vec<&Sent> = t
                .sent
                .iter()
                .filter(|s| s.kind == "append" && s.at >= from && s.at < to)
                .collect();
            let loud = a.iter().filter(|s| s.peak > 0).count();
            (a.len(), loud, a.iter().map(|s| s.peak).max().unwrap_or(0))
        })
    }
}

fn kind(e: &ClientEvent) -> &'static str {
    match e {
        ClientEvent::SessionUpdate { .. } => "session.update",
        ClientEvent::Append { .. } => "append",
        ClientEvent::Commit => "commit",
        ClientEvent::Clear => "clear",
        ClientEvent::ResponseCreate => "response.create",
        ClientEvent::ResponseCancel { .. } => "response.cancel",
        ClientEvent::Truncate { .. } => "truncate",
        ClientEvent::ApprovalResponse { .. } => "approval",
    }
}

pub(super) struct Tapped<L> {
    pub(super) link: L,
    pub(super) tap: Tap,
}

struct TapSocket {
    n: usize,
    inner: Box<dyn VoiceSocket>,
    tap: Tap,
}

impl VoiceSocket for TapSocket {
    fn send_tagged(&mut self, event: ClientEvent, event_id: String) {
        self.tap.0.lock().unwrap().sent.push(Sent {
            socket: self.n,
            at: Instant::now(),
            kind: kind(&event),
            peak: 0,
            samples: 0,
            event: Some(event.clone()),
            tag: Some(event_id.clone()),
        });
        self.inner.send_tagged(event, event_id);
    }

    fn send(&mut self, event: ClientEvent) {
        let (peak, samples) = match &event {
            ClientEvent::Append { audio } => {
                let bytes = lmgw_client::base64::decode(audio).unwrap_or_default();
                let pcm = lmgw_client::base64::pcm16_samples(&bytes);
                (
                    pcm.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0),
                    pcm.len(),
                )
            }
            _ => (0, 0),
        };
        let k = kind(&event);
        self.tap.0.lock().unwrap().sent.push(Sent {
            socket: self.n,
            at: Instant::now(),
            kind: k,
            peak,
            samples,
            event: (k != "append").then(|| event.clone()),
            tag: None,
        });
        self.inner.send(event);
    }

    fn close(&mut self) {
        self.tap.0.lock().unwrap().sent.push(Sent {
            socket: self.n,
            at: Instant::now(),
            kind: "close",
            peak: 0,
            samples: 0,
            event: None,
            tag: None,
        });
        self.inner.close();
    }
}

impl<L: VoiceLink> VoiceLink for Tapped<L> {
    fn set_credentials(&mut self, creds: Option<Credentials>) {
        self.link.set_credentials(creds);
    }

    fn current(&mut self, folder: i64, new: bool) -> Pending<Result<Current, Refusal>> {
        let answer = self.link.current(folder, new);
        let tap = self.tap.clone();
        Box::pin(async move {
            let r = answer.await;
            tap.0.lock().unwrap().currents.push((
                Instant::now(),
                folder,
                new,
                r.as_ref()
                    .map(|c| (c.thread.id, c.rolled_over))
                    .map_err(|e| e.to_string()),
            ));
            r
        })
    }

    fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>> {
        self.link.folders()
    }

    fn create_folder(&mut self, name: &str) -> Pending<Result<FolderInfo, Refusal>> {
        self.link.create_folder(name)
    }

    fn profiles(&mut self) -> Pending<Result<crate::voice::link::ProfileList, Refusal>> {
        self.link.profiles()
    }

    fn set_folder_profile(
        &mut self,
        folder: i64,
        profile: Option<i64>,
    ) -> Pending<Result<crate::voice::link::FolderPatched, Refusal>> {
        self.link.set_folder_profile(folder, profile)
    }

    fn approvals(
        &mut self,
        thread: i64,
        body: &lmgw_client::requests::approvals::ApprovalsRequest,
    ) -> Pending<Result<(), Refusal>> {
        self.link.approvals(thread, body)
    }

    fn pending_approvals(
        &mut self,
        thread: i64,
    ) -> Pending<Result<Vec<lmgw_client::requests::approvals::ApprovalRequest>, Refusal>> {
        self.link.pending_approvals(thread)
    }

    fn transcribe(&mut self, thread: i64, wav: Vec<u8>) -> Pending<Result<String, Refusal>> {
        self.tap
            .0
            .lock()
            .unwrap()
            .transcribes
            .push((Instant::now(), thread));
        self.link.transcribe(thread, wav)
    }

    fn open(
        &mut self,
        thread: i64,
        take_over: bool,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Box<dyn VoiceSocket> {
        let n = {
            let mut t = self.tap.0.lock().unwrap();
            t.opens.push((Instant::now(), thread));
            t.take_overs.push(take_over);
            t.opens.len() - 1
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<SessionEvent>();
        let inner = self.link.open(thread, take_over, tx);
        let tap = self.tap.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let mut t = tap.0.lock().unwrap();
                let kept = match &ev {
                    SessionEvent::Server(ServerEvent::AudioDelta {
                        response_id,
                        item_id,
                        pcm,
                    }) => {
                        let head = t.heads.entry(item_id.clone()).or_default();
                        if head.len() < RATE as usize {
                            head.extend(lmgw_client::base64::pcm16_samples(pcm));
                        }
                        SessionEvent::Server(ServerEvent::AudioDelta {
                            response_id: response_id.clone(),
                            item_id: item_id.clone(),
                            pcm: Vec::new(),
                        })
                    }
                    other => other.clone(),
                };
                t.got.push(Got {
                    at: Instant::now(),
                    event: kept,
                });
                drop(t);
                if events.send(ev).is_err() {
                    break;
                }
            }
        });
        Box::new(TapSocket {
            n,
            inner,
            tap: self.tap.clone(),
        })
    }
}

pub(super) fn is_server(g: &Got, f: impl Fn(&ServerEvent) -> bool) -> bool {
    matches!(&g.event, SessionEvent::Server(e) if f(e))
}

// ------------------------------------------------------------------------ the microphone

/// Plays clips into the test microphone (its loopback's sink), pumped by a task of its own.
pub(super) struct Mic {
    player: Arc<Mutex<Player>>,
    n: u32,
    pump: tokio::task::JoinHandle<()>,
}

impl Mic {
    pub(super) fn new(audio: &Audio) -> Self {
        let player = Arc::new(Mutex::new(audio.open_playback()));
        let p = player.clone();
        let pump = tokio::spawn(async move {
            loop {
                p.lock().unwrap().pump();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        Self { player, n: 0, pump }
    }

    /// Starts `clip` now; returns when it ends (the loopback's own delay not included).
    pub(super) fn say(&mut self, clip: &[i16]) -> Instant {
        self.n += 1;
        let id = format!("said-{}", self.n);
        let mut p = self.player.lock().unwrap();
        p.push(&id, clip);
        p.end(&id);
        Instant::now() + Duration::from_secs_f64(clip.len() as f64 / RATE as f64)
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

// ------------------------------------------------------------------------ waiting

pub(super) async fn until(
    model: &mut watch::Receiver<TrayModel>,
    secs: u64,
    what: &str,
    f: impl FnMut(&TrayModel) -> bool,
) -> TrayModel {
    tokio::time::timeout(Duration::from_secs(secs), model.wait_for(f))
        .await
        .unwrap_or_else(|_| panic!("the tray never showed: {what} (waited {secs} s)"))
        .unwrap()
        .clone()
}

/// The next notification with `summary` (the others are kept for the report).
pub(super) async fn notice(
    rx: &mut mpsc::UnboundedReceiver<Notice>,
    seen: &mut Vec<(Instant, Notice)>,
    summary: &str,
    secs: u64,
) -> (Instant, Notice) {
    if let Some(i) = seen.iter().position(|(_, n)| n.summary == summary) {
        return seen.remove(i);
    }
    let t0 = Instant::now();
    loop {
        let left = Duration::from_secs(secs).saturating_sub(t0.elapsed());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(n)) if n.summary == summary => return (Instant::now(), n),
            Ok(Some(n)) => seen.push((Instant::now(), n)),
            _ => panic!("no notification '{summary}' within {secs} s"),
        }
    }
}

pub(super) fn shown(m: &TrayModel) -> VoiceShown {
    m.voice.shown
}

/// `a - b` in ms, negative when `a` came first.
fn signed_ms(a: Instant, b: Instant) -> i64 {
    if a >= b {
        (a - b).as_millis() as i64
    } else {
        -((b - a).as_millis() as i64)
    }
}

/// Review KAI-2 K2-1: Kai's log (at whatever `KAI_LOG` the script set, `trace` included) carries
/// none of `keys` and no audio. Returns the report's line.
pub(super) fn log_is_clean(log: &std::path::Path, keys: &[&str]) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    for k in keys {
        let hex = k.trim_start_matches("lmgw-device-");
        assert!(!text.contains(hex), "a device key is in {}", log.display());
    }
    assert!(
        !text.contains("\"audio\":\""),
        "an append's audio is in {}",
        log.display()
    );
    format!(
        "Kai's log ({} lines, KAI_LOG={}): no device key, no audio",
        text.lines().count(),
        std::env::var("KAI_LOG").unwrap_or_else(|_| "info".into())
    )
}

pub(super) fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// When the sink first played sound after `after_ns` (CLOCK_MONOTONIC), from the monitor's cycles.
fn first_sound(cycles: &[(i64, Vec<f32>)], after_ns: i64) -> Option<i64> {
    cycles
        .iter()
        .filter(|(t, _)| *t >= after_ns)
        .find_map(|(t, v)| {
            v.iter()
                .position(|s| s.abs() > 1e-3)
                .map(|i| t + i as i64 * 1_000_000_000 / 48_000)
        })
}

pub(super) struct Report(pub(super) String);

impl Report {
    pub(super) fn line(&mut self, s: String) {
        eprintln!("{s}");
        self.0.push_str(&s);
        self.0.push('\n');
    }
}

// ------------------------------------------------------------------------ the check

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/live-check-voice-lmgw.sh (a private PipeWire instance and a dev lmgw)"]
async fn lmgw_voice_live() {
    if !private_or_skip() {
        return;
    }
    assert_eq!(
        std::env::var("KAI_LIVE_LMGW_URL").as_deref(),
        Ok(URL),
        "only the dev lmgw on 127.0.0.1:8897"
    );
    // Kai's log beside the report (KAI_LOG, default info); it carries no key (the core never logs one).
    let log = std::fs::File::create(out_dir().join("kai.log")).unwrap();
    // Kai's own subscriber, its log cap included (review KAI-2 K2-1).
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "info".into()),
        Mutex::new(log),
        false,
    ));
    let clip = read_wav(&env("KAI_LIVE_CLIP"));
    let clip_ms = clip.len() as u64 * 1000 / RATE as u64;
    let mut r = Report(String::new());
    let dir = tempfile::tempdir().unwrap();

    let mut lmgw = Lmgw::new();
    let boot = lmgw.start().await;
    let (folder, key) = set_up(&lmgw).await;
    r.line(format!(
        "dev lmgw up in {} ms; mocks: chat (echo), ASR (\"{ASR_TEXT}\"), TTS (the clip, {clip_ms} ms per clause); folder {folder}",
        ms(boot)
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

    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        format!(
            "[lmgw]\nurl = \"{URL}\"\nretry_max_s = 2\n[audio]\ninput = \"{MIC}\"\noutput = \"{SINK}\"\n\
             [conversation]\nfolder_id = {folder}\n[voice]\ngrace_s = 2\n[shortcuts]\nbind = false\n"
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
    let (ntx, mut notices) = mpsc::unbounded_channel();
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
    let task = tokio::spawn(core.run(cmd_rx));
    until(
        &mut model,
        20,
        "the feed up, the test microphone listed",
        |m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up { .. },
                    ..
                }
            ) && m.audio.devices.inputs.iter().any(|d| d.name == MIC)
        },
    )
    .await;
    r.line("the feed is up: device 'kai-live' on the dev lmgw".into());

    // ---- 1a. push-to-talk, listening off: a press opens the session (§5.3, K5's grace)
    let monitor = Monitor::start(SINK);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let spoken_end = mic.say(&clip);
    tokio::time::sleep_until((spoken_end + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let release = Instant::now();
    let release_ns = monotonic_ns();
    let opened = tap
        .until_got(press, 10, "the 101", |g| g.event == SessionEvent::Opened)
        .await;
    let created = tap
        .until_got(press, 10, "session.created", |g| {
            is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
        })
        .await;
    let thread = match &created.event {
        SessionEvent::Server(ServerEvent::SessionCreated(f)) => {
            f.thread.clone().expect("bound to a thread").id
        }
        _ => unreachable!(),
    };
    let first_delta = tap
        .until_got(release, 20, "the reply's audio", |g| {
            is_server(g, |e| matches!(e, ServerEvent::AudioDelta { .. }))
        })
        .await;
    let reply = tap
        .until_got(release, 30, "the reply as stored", |g| {
            is_server(g, |e| matches!(e, ServerEvent::ChatReply(_)))
        })
        .await;
    until(&mut model, 30, "the reply played", |m| {
        shown(m) == VoiceShown::Off && m.voice.timing.is_some()
    })
    .await;
    let closed = tap
        .until_sent(reply.at, 15, "the grace close", |s| s.kind == "close")
        .await;
    let cycles = monitor.finish();
    let heard_ns = first_sound(&cycles, release_ns).expect("the reply reached the sink");
    let audio_sent = tap.with(|t| {
        t.sent
            .iter()
            .filter(|s| s.socket == 0 && s.kind == "append")
            .map(|s| s.samples)
            .sum::<usize>()
    }) as u64
        * 1000
        / RATE as u64;
    let msgs = lmgw.messages(thread).await;
    let user = msgs
        .iter()
        .rev()
        .find(|m| m.1 == "user")
        .expect("the user's turn");
    let assistant = msgs
        .iter()
        .rev()
        .find(|m| m.1 == "assistant")
        .expect("the reply");
    let timing = model.borrow().voice.timing.clone().unwrap_or_default();
    r.line(format!(
        "1a push-to-talk, listening off (a cold bind from the press): 101 {} ms and session.created {} ms after the press; \
         {audio_sent} ms of audio went up for a hold of {} ms; release → first reply delta {} ms, → first sound on the sink {} ms; \
         the grace close {} ms after the reply was stored (voice.grace_s = 2); thread {thread}: user \"{}\", reply \"{}\"; \
         tooltip timing: {timing}",
        ms(opened.at - press),
        ms(created.at - press),
        ms(release - press),
        ms(first_delta.at - release),
        (heard_ns - release_ns) / 1_000_000,
        ms(closed.at - reply.at),
        user.2,
        assistant.2,
    ));
    assert_eq!(user.2, ASR_TEXT);
    assert_eq!(assistant.2, format!("Echo: {ASR_TEXT}"));
    assert!(
        audio_sent + 100 >= ms(release - press) as u64,
        "the audio from the press went up whole"
    );

    // ---- 1b. push-to-talk mode, listening on
    cmds.send(Command::SetVoiceMode(VoiceMode::PushToTalk))
        .unwrap();
    cmds.send(Command::SetListening(true)).unwrap();
    let on = Instant::now();
    tap.until_got(on, 10, "session.created (push-to-talk mode)", |g| {
        is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
    })
    .await;
    until(&mut model, 10, "quiet listening", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    let monitor = Monitor::start(SINK);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let spoken_end = mic.say(&clip);
    tokio::time::sleep_until((spoken_end + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let release = Instant::now();
    let release_ns = monotonic_ns();
    let commit = tap
        .until_sent(press, 5, "the commit", |s| s.kind == "commit")
        .await;
    let first_delta = tap
        .until_got(release, 20, "the reply's audio", |g| {
            is_server(g, |e| matches!(e, ServerEvent::AudioDelta { .. }))
        })
        .await;
    let reply = tap
        .until_got(release, 30, "the reply as stored", |g| {
            is_server(g, |e| matches!(e, ServerEvent::ChatReply(_)))
        })
        .await;
    until(&mut model, 30, "the reply played", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    let cycles = monitor.finish();
    let heard_ns = first_sound(&cycles, release_ns).expect("the reply reached the sink");
    let (n, loud, _) = tap.appends(press, release + Duration::from_millis(100));
    let cleared = tap.sent_after(press, |s| s.kind == "clear").is_some();
    r.line(format!(
        "1b push-to-talk mode, listening on: press → clear {cleared}, {n} appends ({loud} with sound), commit and response.create \
         {} ms after the release; release → first reply delta {} ms, → first sound on the sink {} ms; reply stored {} ms after the release",
        ms(commit.at - release),
        ms(first_delta.at - release),
        (heard_ns - release_ns) / 1_000_000,
        ms(reply.at - release),
    ));
    assert!(cleared && loud > 0);

    // ---- 2. automatic: the server's detector ends the turn
    cmds.send(Command::SetVoiceMode(VoiceMode::Automatic))
        .unwrap();
    let auto = Instant::now();
    let update = tap
        .until_sent(auto, 5, "the session.update for automatic", |s| {
            s.kind == "session.update"
        })
        .await;
    let detection = match update.event {
        Some(ClientEvent::SessionUpdate { turn_detection, .. }) => turn_detection.map(|d| d.key()),
        _ => None,
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let monitor = Monitor::start(SINK);
    let said = Instant::now();
    let spoken_end = mic.say(&clip);
    let spoken_end_ns = monotonic_ns() + clip_ms as i64 * 1_000_000;
    let started = tap
        .until_got(said, 10, "speech_started", |g| {
            is_server(g, |e| matches!(e, ServerEvent::SpeechStarted))
        })
        .await;
    let stopped = tap
        .until_got(said, 20, "speech_stopped", |g| {
            is_server(g, |e| matches!(e, ServerEvent::SpeechStopped))
        })
        .await;
    let first_delta = tap
        .until_got(said, 30, "the reply's audio", |g| {
            is_server(g, |e| matches!(e, ServerEvent::AudioDelta { .. }))
        })
        .await;
    let reply = tap
        .until_got(said, 40, "the reply as stored", |g| {
            is_server(g, |e| matches!(e, ServerEvent::ChatReply(_)))
        })
        .await;
    until(&mut model, 30, "the reply played", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    let cycles = monitor.finish();
    let heard_ns = first_sound(&cycles, spoken_end_ns).expect("the reply reached the sink");
    let timing = model.borrow().voice.timing.clone().unwrap_or_default();
    r.line(format!(
        "2 automatic ({}): speech_started {} ms after the clip began, speech_stopped {} ms after it ended; end of speech → first reply delta {} ms, \
         → first sound on the sink {} ms; reply stored {} ms after the clip ended; tooltip timing: {timing}",
        detection.unwrap_or("none"),
        ms(started.at - said),
        stopped.at.saturating_duration_since(spoken_end).as_millis(),
        first_delta.at.saturating_duration_since(spoken_end).as_millis(),
        (heard_ns - spoken_end_ns) / 1_000_000,
        reply.at.saturating_duration_since(spoken_end).as_millis(),
    ));
    assert!(detection.is_some(), "automatic sends a detector");

    // ---- 3. a barge-in about 1.5 s into the reply (§5.5)
    let monitor = Monitor::start(SINK);
    let said = Instant::now();
    mic.say(&clip);
    until(&mut model, 30, "Kai speaking", |m| {
        shown(m) == VoiceShown::Speaking
    })
    .await;
    let speaking = Instant::now();
    let reply_rid = tap
        .with(|t| {
            t.got.iter().rev().find_map(|g| match &g.event {
                SessionEvent::Server(ServerEvent::ResponseCreated { response_id })
                    if g.at >= said =>
                {
                    Some(response_id.clone())
                }
                _ => None,
            })
        })
        .expect("the reply's response");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let barge = Instant::now();
    mic.say(&clip);
    let truncate = tap
        .until_sent(barge, 10, "the truncate", |s| s.kind == "truncate")
        .await;
    let (item, audio_end_ms) = match &truncate.event {
        Some(ClientEvent::Truncate {
            item_id,
            audio_end_ms,
            ..
        }) => (item_id.clone(), *audio_end_ms),
        _ => unreachable!(),
    };
    let speech = tap
        .until_got(barge, 10, "speech_started", |g| {
            is_server(g, |e| matches!(e, ServerEvent::SpeechStarted))
        })
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cycles = monitor.finish();
    let mon: Vec<f32> = cycles.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let head = tap
        .with(|t| t.heads.get(&item).cloned())
        .expect("the item's audio");
    let m0 = align(&head, &mon);
    let end = mon
        .iter()
        .rposition(|s| s.abs() > 1e-4)
        .expect("the reply reached the sink") as i64;
    let on_sink_ms = (end - m0) * 1000 / 48_000;
    let cut = tap
        .until_got(barge, 10, "the reply as stored after the cut", |g| {
            g.at >= truncate.at
                && is_server(
                    g,
                    |e| matches!(e, ServerEvent::ChatReply(c) if c.unheard.is_some()),
                )
        })
        .await;
    // A barge-in's reply may be stored twice: at what was sent, then at the truncate's heard.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cuts: Vec<(Instant, lmgw_client::realtime::ChatReply)> = tap.with(|t| {
        t.got
            .iter()
            .filter(|g| g.at >= barge)
            .filter_map(|g| match &g.event {
                SessionEvent::Server(ServerEvent::ChatReply(c)) if c.unheard.is_some() => {
                    Some((g.at, c.clone()))
                }
                _ => None,
            })
            .collect()
    });
    let stored = cuts.last().expect("the cut reply").1.clone();
    let stores: Vec<String> = cuts
        .iter()
        .map(|(at, c)| {
            format!(
                "{:+} ms: \"{}\"",
                signed_ms(*at, truncate.at),
                c.content.clone().unwrap_or_default()
            )
        })
        .collect();
    // The barge-in's own words become the next turn; its reply plays out.
    tap.until_got(barge, 40, "the next reply as stored", |g| {
        g.at > cut.at
            && is_server(
                g,
                |e| matches!(e, ServerEvent::ChatReply(c) if c.unheard.is_none()),
            )
    })
    .await;
    until(&mut model, 30, "the next reply played", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    let msgs = lmgw.messages(thread).await;
    let in_thread = msgs
        .iter()
        .find(|m| m.0 == stored.message_id)
        .expect("the cut reply in the thread");
    let cancels = tap.with(|t| {
        t.sent
            .iter()
            .filter(|s| {
                s.at >= barge
                    && s.at < truncate.at + Duration::from_millis(200)
                    && s.kind == "response.cancel"
            })
            .count()
    });
    r.line(format!(
        "3 barge-in: the second clip began {} ms into Kai's speaking; speech_started {} ms after it, the truncate {} ms after it \
         (response {reply_rid}, item {item}): audio_end_ms {audio_end_ms} (heard), the sink got {on_sink_ms} ms of the reply (played); \
         response.cancel sent: {cancels}; stored after the barge-in: {stores:?}; the thread keeps \"{}\" (message {}), unheard \"{}\"",
        ms(barge - speaking),
        ms(speech.at - barge),
        ms(truncate.at - barge),
        in_thread.2,
        stored.message_id,
        stored.unheard.clone().unwrap_or_default(),
    ));
    assert_eq!(
        Some(in_thread.2.clone()),
        stored.content,
        "the thread keeps what was heard"
    );
    assert!(
        format!("Echo: {ASR_TEXT}").starts_with(in_thread.2.trim_end()),
        "the heard text is the reply's start"
    );
    assert!(
        !stored.unheard.clone().unwrap_or_default().is_empty(),
        "something was not heard"
    );
    assert!(
        on_sink_ms + 5 >= audio_end_ms as i64 && on_sink_ms <= audio_end_ms as i64 + 150,
        "heard = played less the device delay"
    );

    // ---- 4. a dashboard text turn silences automatic listening until it is done (K16)
    let words: Vec<String> = (1..=150).map(|i| format!("Wort{i}")).collect();
    let content = words.join(" ");
    let sending = Instant::now();
    let send = {
        let http = lmgw.http.clone();
        let token = lmgw.token.clone();
        let content = content.clone();
        tokio::spawn(async move {
            let r = http
                .post(format!("{URL}/chat/api/threads/{thread}/send"))
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(json!({"content": content}).to_string())
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .unwrap();
            let status = r.status().as_u16();
            let body = r.text().await.unwrap();
            (Instant::now(), status, body)
        })
    };
    let k16 = until(&mut model, 10, "K16's note", |m| {
        m.voice
            .notes
            .iter()
            .any(|n| n.contains("a turn of the dashboard runs"))
    })
    .await;
    let k16_on = Instant::now();
    let note = k16
        .voice
        .notes
        .iter()
        .find(|n| n.contains("a turn of"))
        .cloned()
        .unwrap();
    let spoken_end = mic.say(&clip);
    until(&mut model, 30, "K16's note gone", |m| {
        !m.voice
            .notes
            .iter()
            .any(|n| n.contains("a turn of the dashboard runs"))
    })
    .await;
    let k16_off = Instant::now();
    let (done_at, status, body) = send.await.unwrap();
    let (n, loud, peak) = tap.appends(k16_on, k16_off);
    let responses = tap.with(|t| {
        t.got
            .iter()
            .filter(|g| {
                g.at >= k16_on
                    && g.at < k16_off
                    && is_server(g, |e| matches!(e, ServerEvent::ResponseCreated { .. }))
            })
            .count()
    });
    let speech_during = tap.with(|t| {
        t.got
            .iter()
            .filter(|g| {
                g.at >= k16_on
                    && g.at < k16_off
                    && is_server(g, |e| matches!(e, ServerEvent::SpeechStarted))
            })
            .count()
    });
    r.line(format!(
        "4 K16: the dashboard's send took {} ms (HTTP {status}); Kai's note came {} ms after the send began and went {} ms after the send's \
         stream ended; the clip ({clip_ms} ms) played into the microphone from the note on, ending {} before the note went; \
         meanwhile {n} appends, {loud} with sound (peak {peak}), speech_started {speech_during} times, responses {responses}; note: \"{note}\"",
        ms(done_at - sending),
        ms(k16_on - sending),
        k16_off.saturating_duration_since(done_at).as_millis(),
        if spoken_end < k16_off { format!("{} ms", ms(k16_off - spoken_end)) } else { format!("-{} ms (it ran past)", ms(spoken_end - k16_off)) },
    ));
    assert!(
        n > 50 && loud == 0,
        "silence while the dashboard's turn runs"
    );
    assert_eq!(speech_during, 0);
    assert!(
        spoken_end < k16_off,
        "the whole clip fell into the dashboard's turn"
    );
    assert!(
        body.contains("event: done"),
        "the dashboard's turn finished: {}",
        &body[body.len().saturating_sub(300)..]
    );
    let msgs = lmgw.messages(thread).await;
    assert_eq!(
        msgs.last().map(|m| m.2.clone()),
        Some(format!("Echo: {content}")),
        "the dashboard's reply was saved"
    );
    // After it, the microphone counts again.
    let after = Instant::now();
    mic.say(&clip);
    let started = tap
        .until_got(
            after,
            10,
            "speech_started after the dashboard's turn",
            |g| is_server(g, |e| matches!(e, ServerEvent::SpeechStarted)),
        )
        .await;
    let (n2, loud2, _) = tap.appends(after, started.at);
    tap.until_got(after, 40, "that reply as stored", |g| {
        is_server(g, |e| matches!(e, ServerEvent::ChatReply(_)))
    })
    .await;
    until(&mut model, 30, "that reply played", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    r.line(format!(
        "4 after the dashboard's turn: {n2} appends before speech_started, {loud2} with sound; speech_started {} ms after the clip began",
        ms(started.at - after)
    ));
    assert!(loud2 > 0);

    // ---- 5. a dashboard bind takes over (§5.6)
    let opens = tap.opens();
    let mut req = format!("ws://127.0.0.1:8897/v1/realtime?chat_thread={thread}")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", lmgw.token).parse().unwrap(),
    );
    let bind = Instant::now();
    let (mut dashboard, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("the dashboard binds");
    let closed = tap
        .until_got(bind, 10, "Kai's session closed", |g| {
            matches!(g.event, SessionEvent::Closed { .. })
        })
        .await;
    let (taken_at, taken) = notice(&mut notices, &mut seen, "Kai: voice taken over", 10).await;
    let m = until(&mut model, 5, "listening off", |m| !m.voice.on).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reopened = tap.opens() - opens;
    r.line(format!(
        "5 takeover: Kai's session closed {} ms after the dashboard's handshake ({:?}); notified {} ms after it: \"{}\"; \
         tooltip note: {:?}; sessions opened in the 5 s after: {reopened}",
        ms(closed.at - bind),
        closed.event,
        ms(taken_at - bind),
        taken.body,
        m.voice.notes.iter().find(|n| n.contains("take it back")),
    ));
    assert!(
        taken.body.contains("the dashboard"),
        "the notification names who took it"
    );
    assert_eq!(reopened, 0, "Kai does not reconnect by itself");
    dashboard.send(Message::Close(None)).await.unwrap();
    while let Some(Ok(_)) = dashboard.next().await {}

    // ---- 6. a dev lmgw restart turns listening off with a notification (K5)
    cmds.send(Command::SetListening(true)).unwrap();
    let on = Instant::now();
    tap.until_got(on, 10, "session.created again", |g| {
        is_server(g, |e| matches!(e, ServerEvent::SessionCreated(_)))
    })
    .await;
    until(&mut model, 10, "quiet listening", |m| {
        shown(m) == VoiceShown::Quiet
    })
    .await;
    let opens = tap.opens();
    let stopping = Instant::now();
    // The stop runs beside the wait: lmgw may take a while to quit.
    let stop = lmgw.interrupt();
    let (stopped_at, stopped) = notice(&mut notices, &mut seen, "Kai: lmgw is stopping", 30).await;
    let closed = tap
        .until_got(stopping, 5, "the session's end", |g| {
            matches!(g.event, SessionEvent::Closed { .. })
        })
        .await;
    let m = until(&mut model, 5, "listening off", |m| !m.voice.on).await;
    let how = stop.await.unwrap();
    let boot = lmgw.start().await;
    let started = Instant::now();
    let up = until(&mut model, 30, "the feed up again", |m| {
        matches!(
            &m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        )
    })
    .await;
    let back = Instant::now();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reopened = tap.opens() - opens;
    let still_off = !model.borrow().voice.on;
    r.line(format!(
        "6 restart: SIGINT at 0; Kai's session ended at {} ms ({:?}); notified at {} ms: \"{}\"; listening off: {}; the dev lmgw {how}; \
         started again in {} ms, the feed up {} ms after that; sessions opened in the 5 s after: {reopened}, listening still off: {still_off}; tooltip: {:?}",
        ms(closed.at - stopping),
        closed.event,
        ms(stopped_at - stopping),
        stopped.body,
        !m.voice.on,
        ms(boot),
        ms(back - started),
        up.voice.notes,
    ));
    assert!(
        matches!(&closed.event, SessionEvent::Closed { code, .. } if *code == lmgw_client::realtime::CLOSE_GOING_AWAY),
        "lmgw's stop closes the session with 1001: {:?}",
        closed.event
    );
    assert!(
        stopped.body.contains(super::link::SHUTTING_DOWN),
        "{}",
        stopped.body
    );
    assert!(
        m.voice
            .notes
            .iter()
            .any(|n| n.contains(super::link::SHUTTING_DOWN)),
        "the tooltip says it: {:?}",
        m.voice.notes
    );
    assert_eq!(reopened, 0, "no reconnect by itself after lmgw's stop");
    assert!(still_off);

    // ---- the end: a clean quit, the dev lmgw stopped
    for (at, n) in seen.drain(..) {
        r.line(format!(
            "notification (+{} ms): {} | {}",
            ms(at - press),
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
    r.line(log_is_clean(&out_dir().join("kai.log"), &[&key]));
    std::fs::write(out_dir().join("report.txt"), &r.0).unwrap();
}
