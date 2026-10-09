//! The hosted server against a fake host that plays lmgw's side of api §5.1 to §5.5 (until lmgw's
//! M1 is in its main): the WebSocket under the device key, `initialize` and listing, calls with
//! `_meta`, `notifications/cancelled`, the closes. Test tools record what ran; a fake confirmer
//! hands the confirmations to the test.

mod calls;
mod closes;
mod limits;
mod log;
mod runs;
mod script;
mod tasks;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use kai_desktop::contract::{Kind, Level, Param, ToolSpec};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::HeaderMap;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use super::confirm::{ConfirmRequest, Confirmer};
use super::link::LinkSettings;
use super::policy::Policy;
use super::registry::{Changes, Deps, Entry, Provider, Reply, SessionBus};
use super::settings::PolicyConfig;
use super::{HostEvent, HostLink, HostStatus, ToolHost};
use crate::lmgw_link::Credentials;
use crate::pairing::DeviceKey;
use crate::pairing::tests::KEY;

pub const WAIT: Duration = Duration::from_secs(10);

const TEXT: Param = Param::new("text", Kind::Text);
pub const ECHO: ToolSpec = ToolSpec {
    name: "echo",
    description: "Answer with the text.",
    params: &[TEXT],
    ..ToolSpec::READ
};
pub const POKE: ToolSpec = ToolSpec {
    name: "poke",
    description: "Change a thing.",
    params: &[TEXT],
    ..ToolSpec::CHANGE
};
pub const HANG: ToolSpec = ToolSpec {
    name: "hang",
    description: "Run until stopped.",
    ..ToolSpec::READ
};
/// A tool that resolves something before its confirmation (a [`super::registry::Preparer`]).
pub const PREP: ToolSpec = ToolSpec {
    name: "prep",
    description: "Change a thing, resolved first.",
    params: &[TEXT],
    ..ToolSpec::CHANGE
};
/// Answers with what its call carries besides the arguments.
pub const CTX: ToolSpec = ToolSpec {
    name: "ctx",
    description: "Say the call's context.",
    ..ToolSpec::READ
};
pub const OWN_ONLY: ToolSpec = ToolSpec {
    name: "own_only",
    description: "Only Kai itself may call this.",
    level: Level::Own,
    ..ToolSpec::READ
};

/// What the test tools did, in order.
#[derive(Clone, Default)]
pub struct Ran(Arc<Mutex<Vec<String>>>);

impl Ran {
    fn push(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    pub async fn until(&self, what: &str) {
        tokio::time::timeout(WAIT, async {
            while !self.all().iter().any(|r| r == what) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("'{what}' never came: {:?}", self.all()));
    }
}

/// Logs a drop, so a test sees a running call's future go.
struct OnDrop(Ran, &'static str);
impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.push(self.1.to_owned());
    }
}

struct TestTools(Ran);

impl Provider for TestTools {
    fn entries(&self) -> Vec<Entry> {
        let ran = self.0.clone();
        let entry = |spec: &'static ToolSpec| {
            let ran = ran.clone();
            Entry {
                area: "test",
                spec,
                handler: Arc::new(move |call| {
                    let ran = ran.clone();
                    Box::pin(async move {
                        let who = call.caller.describe();
                        match call.spec.name {
                            "echo" => {
                                let text = call.args.text("text").unwrap_or_default().to_owned();
                                ran.push(format!("echo by {who}"));
                                Reply::text(format!("echo: {text}"))
                            }
                            "poke" => {
                                ran.push(format!("poke by {who}"));
                                Reply::text("poked")
                            }
                            "own_only" => {
                                ran.push(format!("own_only by {who}"));
                                Reply::text("mine")
                            }
                            "prep" => {
                                let seen = call
                                    .prepared
                                    .as_ref()
                                    .and_then(|p| p.state.downcast_ref::<String>().cloned());
                                ran.push(format!("prep by {who}"));
                                Reply::text(format!("prep ran {seen:?}"))
                            }
                            "ctx" => Reply::text(format!(
                                "room {:?}; says {}; timeout {:?}; deadline {}",
                                call.room,
                                call.room_says,
                                call.timeout_ms,
                                call.deadline.is_some()
                            )),
                            _ => {
                                let _gone = OnDrop(ran.clone(), "hang dropped");
                                let cancel = call.cancel.clone();
                                let watch = ran.clone();
                                tokio::spawn(async move {
                                    cancel.cancelled().await;
                                    watch.push("hang's token cancelled".into());
                                });
                                ran.push("hang started".into());
                                std::future::pending::<()>().await;
                                Reply::text("never")
                            }
                        }
                    })
                }),
                prepare: (spec.name == "prep").then(|| {
                    let p: super::registry::Preparer = Arc::new(|args, origin| {
                        let text = args.text("text").unwrap_or_default().to_owned();
                        Box::pin(async move {
                            if text == "name-it" && origin.approved_in_lmgw {
                                return Err("approved in lmgw: name it".to_owned());
                            }
                            if text == "refuse" {
                                return Err("refused before anyone was asked".to_owned());
                            }
                            Ok(super::registry::Prepared {
                                lines: vec![format!("resolved: {text}")],
                                state: Arc::new(format!("plan {text}")),
                                notes: Vec::new(),
                                options: Vec::new(),
                            })
                        })
                    });
                    p
                }),
            }
        };
        vec![
            entry(&ECHO),
            entry(&POKE),
            entry(&HANG),
            entry(&OWN_ONLY),
            entry(&PREP),
            entry(&CTX),
        ]
    }
}

/// The fake confirmer: each question goes to the test.
struct Asks(mpsc::UnboundedSender<ConfirmRequest>);
impl Confirmer for Asks {
    fn ask(&self, req: ConfirmRequest) {
        let _ = self.0.send(req);
    }
}

/// lmgw's side: a listener for `/mcp/host`.
pub struct FakeHost {
    listener: TcpListener,
    pub url: String,
}

impl FakeHost {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        Self { listener, url }
    }

    async fn tcp(&self, within: Duration) -> Option<TcpStream> {
        tokio::time::timeout(within, self.listener.accept())
            .await
            .ok()
            .map(|r| r.unwrap().0)
    }

    /// The next link: the upgrade, its request's path and headers kept.
    pub async fn accept(&self) -> Conn {
        let stream = self.tcp(WAIT).await.expect("Kai linked");
        Self::upgrade(stream).await
    }

    /// The next link, taking frames of at most `max_frame` bytes (as lmgw's `mcp.host_max_frame_mb`
    /// does: a larger one fails the link) and messages of any size.
    pub async fn accept_frames_of(&self, max_frame: usize) -> Conn {
        let stream = self.tcp(WAIT).await.expect("Kai linked");
        let config = WebSocketConfig::default()
            .max_frame_size(Some(max_frame))
            .max_message_size(None);
        Self::upgrade_with(stream, Some(config)).await
    }

    /// The upgrade on a connection already accepted.
    async fn upgrade(stream: TcpStream) -> Conn {
        Self::upgrade_with(stream, None).await
    }

    #[allow(clippy::result_large_err)]
    async fn upgrade_with(stream: TcpStream, config: Option<WebSocketConfig>) -> Conn {
        let seen = Arc::new(Mutex::new((String::new(), HeaderMap::new())));
        let keep = seen.clone();
        let ws = tokio_tungstenite::accept_hdr_async_with_config(
            stream,
            move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                *keep.lock().unwrap() = (req.uri().to_string(), req.headers().clone());
                Ok(resp)
            },
            config,
        )
        .await
        .unwrap();
        let (path, headers) = seen.lock().unwrap().clone();
        Conn {
            ws,
            path,
            headers,
            next_id: 100,
        }
    }

    /// No link within `within`.
    pub async fn none_within(&self, within: Duration) {
        assert!(self.tcp(within).await.is_none(), "Kai linked again");
    }

    /// Answers the next upgrade with an HTTP refusal instead.
    pub async fn refuse(&self, status: u16, body: Value) {
        let mut stream = self.tcp(WAIT).await.expect("Kai linked");
        read_head(&mut stream).await;
        let body = body.to_string();
        let reason = match status {
            401 => "Unauthorized",
            403 => "Forbidden",
            _ => "Refused",
        };
        let resp = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    }
}

/// Reads an upgrade request's head off a raw connection (lmgw has not answered it).
async fn read_head(stream: &mut TcpStream) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    let mut head = Vec::new();
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "the request ended early");
        head.extend_from_slice(&buf[..n]);
    }
}

/// One link, lmgw's end.
pub struct Conn {
    ws: WebSocketStream<TcpStream>,
    pub path: String,
    pub headers: HeaderMap,
    next_id: i64,
}

impl Conn {
    pub async fn send(&mut self, v: Value) {
        self.ws
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn send_text(&mut self, t: &str) {
        self.ws
            .send(Message::Text(t.to_owned().into()))
            .await
            .unwrap();
    }

    /// The next JSON-RPC message from Kai (pings and pongs skipped).
    pub async fn recv(&mut self) -> Value {
        loop {
            let m = tokio::time::timeout(WAIT, self.ws.next())
                .await
                .expect("Kai sent nothing")
                .expect("the link ended")
                .unwrap();
            match m {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Close(f) => panic!("Kai closed the link: {f:?}"),
                _ => {}
            }
        }
    }

    /// The answer to request `id`; notifications before it are kept in `seen`.
    pub async fn answer(&mut self, id: i64, seen: &mut Vec<Value>) -> Value {
        loop {
            let m = self.recv().await;
            if m.get("id") == Some(&json!(id)) {
                return m;
            }
            seen.push(m);
        }
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        self.answer(id, &mut Vec::new()).await
    }

    /// lmgw's opening (api §5.1): `initialize`, the answer, `notifications/initialized`.
    pub async fn initialize(&mut self) -> Value {
        self.initialize_with(None).await
    }

    /// The opening with the link's limits announced (`_meta["lmgw/host_limits"]`).
    pub async fn initialize_with(&mut self, limits: Option<Value>) -> Value {
        let mut params = json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "lmgw", "version": "fake"}
        });
        if let Some(l) = limits {
            params["_meta"] = json!({ "lmgw/host_limits": l });
        }
        let answer = self.request("initialize", params).await;
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        answer
    }

    pub async fn list(&mut self) -> Vec<Value> {
        let a = self.request("tools/list", json!({})).await;
        a["result"]["tools"].as_array().unwrap().clone()
    }

    /// Sends a `tools/call` with lmgw's `_meta` (api §5.5) and returns its id.
    pub async fn call(&mut self, name: &str, args: Value, caller: Value, timeout_ms: u64) -> i64 {
        self.call_with(name, args, caller, Value::Null, timeout_ms)
            .await
    }

    /// [`Self::call`] carrying `lmgw/approval`: `approved` by the owner.
    pub async fn call_approved(
        &mut self,
        name: &str,
        args: Value,
        caller: Value,
        timeout_ms: u64,
    ) -> i64 {
        let approval = json!({"decision": "approved", "by": owner()});
        self.call_with(name, args, caller, approval, timeout_ms)
            .await
    }

    async fn call_with(
        &mut self,
        name: &str,
        args: Value,
        caller: Value,
        approval: Value,
        timeout_ms: u64,
    ) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "name": name,
                "arguments": args,
                "_meta": {"lmgw/caller": caller, "lmgw/approval": approval, "lmgw/timeout_ms": timeout_ms}
            }
        }))
        .await;
        id
    }

    pub async fn cancel(&mut self, id: i64, reason: &str) {
        self.send(json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": id, "reason": reason}
        }))
        .await;
    }

    pub async fn close(&mut self, code: u16, reason: &str) {
        self.ws
            .send(Message::Close(Some(CloseFrame {
                code: code.into(),
                reason: reason.to_owned().into(),
            })))
            .await
            .unwrap();
        // Kai's answer to the close.
        while let Ok(Some(Ok(m))) = tokio::time::timeout(WAIT, self.ws.next()).await {
            if matches!(m, Message::Close(_)) {
                break;
            }
        }
    }

    /// Kai closed the link: its close frame's code and reason.
    pub async fn closed_by_kai(&mut self) -> (u16, String) {
        loop {
            match tokio::time::timeout(WAIT, self.ws.next()).await.unwrap() {
                Some(Ok(Message::Close(Some(f)))) => return (f.code.into(), f.reason.to_string()),
                Some(Ok(_)) => {}
                other => panic!("no close frame: {other:?}"),
            }
        }
    }
}

pub fn owner() -> Value {
    json!({"kind": "owner", "name": "owner"})
}

pub fn key(name: &str) -> Value {
    json!({"kind": "key", "name": name})
}

/// The result's text blocks, joined, and whether it is an error.
pub fn text_of(answer: &Value) -> (String, bool) {
    let r = &answer["result"];
    let text = r["content"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    (text, r["isError"].as_bool().unwrap_or(false))
}

/// What the providers are built from, with nothing behind it (no core, no GTK, a bus never
/// connected).
pub fn deps() -> Deps {
    Deps {
        cmds: mpsc::unbounded_channel().0,
        ui: async_channel::unbounded().0,
        bus: SessionBus::lazy(Duration::from_secs(1)),
        config: Arc::default(),
        creds: watch::channel(None).1,
        policy: Arc::default(),
        changes: Changes::new().0,
        screen: crate::screen_lock::ScreenLock::default(),
        a11y: crate::desktop::fdo::atspi::switch::A11ySwitch::inert(),
        jobs: crate::jobs::Jobs::default(),
    }
}

pub struct Rig {
    pub host: HostLink,
    /// The credentials the providers see (`Deps::creds`).
    pub creds: watch::Receiver<Option<Credentials>>,
    pub events: mpsc::UnboundedReceiver<HostEvent>,
    pub asks: mpsc::UnboundedReceiver<ConfirmRequest>,
    pub ran: Ran,
    pub fake: FakeHost,
    /// The screen lock the host refuses by.
    pub screen: crate::screen_lock::ScreenLock,
}

pub fn settings(retry_max_s: u64) -> LinkSettings {
    LinkSettings {
        retry_max: Duration::from_secs(retry_max_s),
        timeout: Duration::from_secs(5),
        keepalive: Duration::from_secs(15),
        fallback: super::limits::LinkLimits::fallback(64, 16),
    }
}

/// A started host against a fresh fake lmgw.
pub async fn rig_with(policy: PolicyConfig, settings: LinkSettings) -> Rig {
    rig_with_providers(policy, settings, Vec::new()).await
}

/// [`rig_with`], and `more` providers after the test tools.
pub async fn rig_with_providers(
    policy: PolicyConfig,
    settings: LinkSettings,
    more: Vec<Box<dyn Provider>>,
) -> Rig {
    let fake = FakeHost::new().await;
    let ran = Ran::default();
    let (asks_tx, asks) = mpsc::unbounded_channel();
    let (_changes, changed) = Changes::new();
    let (creds_tx, creds) = watch::channel(None);
    let screen = crate::screen_lock::ScreenLock::default();
    let mut host = HostLink::new(
        settings,
        Arc::new(std::sync::RwLock::new(Policy::new(&policy))),
        std::iter::once(Box::new(TestTools(ran.clone())) as Box<dyn Provider>)
            .chain(more)
            .collect(),
        changed,
        Arc::new(Asks(asks_tx)),
        creds_tx,
        screen.clone(),
        crate::jobs::Jobs::default(),
    );
    let (tx, events) = mpsc::unbounded_channel();
    host.start(
        Credentials {
            url: fake.url.clone(),
            key: DeviceKey::parse(KEY).unwrap(),
        },
        tx,
    );
    Rig {
        host,
        creds,
        events,
        asks,
        ran,
        fake,
        screen,
    }
}

pub async fn rig() -> Rig {
    rig_with(PolicyConfig::default(), settings(1)).await
}

impl Rig {
    /// The next status the predicate takes (statuses before it skipped).
    pub async fn status(&mut self, want: impl Fn(&HostStatus) -> bool) -> HostStatus {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await.expect("the host's events ended") {
                    HostEvent::Status(s) if want(&s) => return s,
                    _ => {}
                }
            }
        })
        .await
        .expect("the status never came")
    }

    pub async fn event(&mut self) -> HostEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("no host event")
            .expect("the host's events ended")
    }

    /// A link up: accepted, initialized, `Linked`.
    pub async fn linked(&mut self) -> Conn {
        let mut c = self.fake.accept().await;
        c.initialize().await;
        self.status(|s| matches!(s, HostStatus::Linked { .. }))
            .await;
        c
    }

    pub async fn ask(&mut self) -> ConfirmRequest {
        tokio::time::timeout(WAIT, self.asks.recv())
            .await
            .expect("no confirmation was asked")
            .unwrap()
    }
}
