//! The realtime adapter (§5, K3): Kai's [`VoiceLink`] on lmgw's network API, over the device link
//! (lmgw's device link design §6.2: the device's certificate authenticates it, no request carries
//! a credential).
//!
//! - **The conversation's thread and folder** (§4): `POST /chat/api/folders/{id}/current`
//!   (`{new: true}` for a new conversation), `GET /chat/api/folders` and `POST /chat/api/folders
//!   {name}` through the device link's reqwest client, each bounded by `lmgw.timeout_s` for the
//!   answer and for its body; the answers read with `lmgw_client::requests`' readers, a refusal
//!   with its status, code and message.
//! - **A bound session**: `GET /v1/realtime?chat_thread=<id>` as a WebSocket
//!   (`lmgw_client::requests::realtime`; Kai's own binds `realtime_unless_bound`, which lmgw
//!   refuses 409 `chat_thread_bound` while another session holds the thread, review KAI-2 K2-13),
//!   opened by a task of its own. Its frames are text both ways: [`ClientEvent::to_json`] out,
//!   `lmgw_client::realtime::parse` in. A refusal before the 101 reads with `read_refusal` (the
//!   handshake's `{"error": {…}}`); the server's close frame becomes [`SessionEvent::Closed`] with
//!   its code and reason as sent (the voice reads what it means with `close_kind`: 4000 a
//!   takeover, 4003 a revocation of a kind, 4004 a thread out of reach), and an end without one is
//!   1006 with what broke it. Events sent before the 101 wait in order; a close asked for before it
//!   gives the handshake up.
//! - **A dead socket is found** (review KAI-2 K2-6): a ping every `voice.keepalive_s`; nothing
//!   heard from lmgw for two intervals, or a write it does not take in that long, ends the session
//!   as a drop (1006) that says so.
//! - **The outgoing queue is bounded** by `voice.send_queue_s` of audio (K2-6): an append beyond
//!   it is dropped, said once per overflow as [`SessionEvent::Note`] naming the key; every other
//!   event is always queued. It must hold the audio buffered before `session.created`, so it is at
//!   least `voice.connect_timeout_s`.
//! - **No other limits of its own.** The socket takes messages of any size (tungstenite's 64 MiB
//!   and 16 MiB defaults are lifted): the gateway is the owner's own, and how long a session may
//!   take to start is `voice.connect_timeout_s`, which the voice session counts from the press.
//! - **TLS** is the device link's ([`super::tls::Tls`]): the WebSocket's TLS stream is built from the
//!   same configuration as every request, and a handshake lmgw refused reads in `lmgw-client`'s
//!   words (`lmgw_client::tls::TlsRefusal`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lmgw_client::realtime::{self, ClientEvent};
use lmgw_client::requests;
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};

use super::proxy::{Forward, Limits as ProxyLimits, Proxied};
use super::tls::unreachable;
use super::{Credentials, describe};
use crate::audio::RATE;
use crate::conversation::FolderInfo;
use crate::voice::link::{
    CLOSE_DROPPED, Current, FolderPatched, Pending, ProfileList, Refusal, SessionEvent, ThreadRef,
    VoiceLink, VoiceSocket, append_samples,
};

/// The reason of Kai's own close (1000): listening off, the press session's grace over, a rebind or
/// quit.
pub(crate) const CLOSE_REASON: &str = "Kai closed the voice session";

/// What the realtime adapter needs from the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeSettings {
    /// `lmgw.timeout_s`: the answer to `current` and its body.
    pub timeout: Duration,
    /// `voice.keepalive_s`: the ping's interval; two without a word from lmgw end the session.
    pub keepalive: Duration,
    /// `voice.send_queue_s`: the audio the outgoing queue holds.
    pub send_queue: Duration,
}

impl RealtimeSettings {
    pub fn from_config(c: &crate::config::Config) -> Self {
        Self {
            timeout: c.lmgw.timeout(),
            keepalive: c.voice.keepalive(),
            send_queue: c.voice.send_queue(),
        }
    }
}

/// Kai's realtime client (module doc).
pub struct RealtimeLink {
    settings: RealtimeSettings,
    creds: Option<Credentials>,
}

impl RealtimeLink {
    pub fn new(settings: RealtimeSettings) -> Self {
        Self {
            settings,
            creds: None,
        }
    }

    /// Sends `req` and reads its answer with `read`, both bounded by `lmgw.timeout_s`; `what` names
    /// the request in the errors.
    fn call<T: Send + 'static>(
        &self,
        req: requests::Request,
        what: &'static str,
        read: fn(u16, &str) -> Result<T, requests::ApiError>,
    ) -> Pending<Result<T, Refusal>> {
        let Some(creds) = self.creds.clone() else {
            return Box::pin(async { Err(Refusal::no_answer("Kai is not paired")) });
        };
        let client = creds.http();
        let timeout = self.settings.timeout;
        Box::pin(async move {
            let secs = timeout.as_secs();
            let url = req.url(&creds.url());
            let mut rb = match req.method {
                requests::Method::Get => client.get(url),
                requests::Method::Post => client.post(url),
            };
            for h in &req.headers {
                let (name, value) = super::header(h).map_err(Refusal::no_answer)?;
                rb = rb.header(name, value);
            }
            if let Some(body) = req.body {
                rb = rb.body(body);
            }
            let resp = match tokio::time::timeout(timeout, rb.send()).await {
                Err(_) => {
                    return Err(Refusal::no_answer(format!(
                        "lmgw did not answer the request for {what} within {secs} s \
                         (lmgw.timeout_s)"
                    )));
                }
                Ok(Err(e)) => return Err(Refusal::no_answer(unreachable(&e))),
                Ok(Ok(r)) => r,
            };
            let status = resp.status().as_u16();
            let body = match tokio::time::timeout(timeout, resp.text()).await {
                Err(_) => {
                    return Err(Refusal::no_answer(format!(
                        "lmgw did not finish its answer about {what} within {secs} s \
                         (lmgw.timeout_s)"
                    )));
                }
                Ok(Err(e)) => {
                    return Err(Refusal::no_answer(format!(
                        "lmgw's answer broke off: {}",
                        describe(&e)
                    )));
                }
                Ok(Ok(b)) => b,
            };
            read(status, &body).map_err(|e| Refusal {
                status,
                code: Some(e.code),
                message: e.message,
            })
        })
    }

    /// The handshake's request (the device link's WebSocket takes `wss:` only).
    fn handshake(
        &mut self,
        creds: &Credentials,
        thread: i64,
        take_over: bool,
    ) -> Result<tungstenite::handshake::client::Request, String> {
        let req = if take_over {
            requests::realtime(thread)
        } else {
            requests::realtime_unless_bound(thread)
        };
        let url = req
            .ws_url(&creds.url())
            .map_err(|e| format!("{} ({})", e.message, e.code))?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| format!("the voice session's address '{url}' does not work: {e}"))?;
        for h in &req.headers {
            let (name, value) = super::header(h)?;
            request.headers_mut().insert(name, value);
        }
        Ok(request)
    }
}

/// Sends `req` with `bytes` as its body when given (else the request's own), and waits
/// `lmgw.timeout_s` for the answer's head; `what` names it in the errors.
async fn send_head(
    creds: &Credentials,
    req: requests::Request,
    bytes: Option<Vec<u8>>,
    timeout: Duration,
    what: &str,
) -> Result<reqwest::Response, Refusal> {
    let client = creds.http();
    let url = req.url(&creds.url());
    let mut rb = match req.method {
        requests::Method::Get => client.get(url),
        requests::Method::Post => client.post(url),
    };
    for h in &req.headers {
        let (name, value) = super::header(h).map_err(Refusal::no_answer)?;
        rb = rb.header(name, value);
    }
    if let Some(b) = bytes {
        rb = rb.body(b);
    } else if let Some(body) = req.body {
        rb = rb.body(body);
    }
    match tokio::time::timeout(timeout, rb.send()).await {
        Err(_) => Err(Refusal::no_answer(format!(
            "lmgw did not answer the request for {what} within {} s (lmgw.timeout_s)",
            timeout.as_secs()
        ))),
        Ok(Err(e)) => Err(Refusal::no_answer(unreachable(&e))),
        Ok(Ok(r)) => Ok(r),
    }
}

/// The answer's body, within `lmgw.timeout_s`.
async fn read_body(
    resp: reqwest::Response,
    timeout: Duration,
    what: &str,
) -> Result<String, Refusal> {
    match tokio::time::timeout(timeout, resp.text()).await {
        Err(_) => Err(Refusal::no_answer(format!(
            "lmgw did not finish its answer about {what} within {} s (lmgw.timeout_s)",
            timeout.as_secs()
        ))),
        Ok(Err(e)) => Err(Refusal::no_answer(format!(
            "lmgw's answer broke off: {}",
            describe(&e)
        ))),
        Ok(Ok(b)) => Ok(b),
    }
}

impl VoiceLink for RealtimeLink {
    fn set_credentials(&mut self, creds: Option<Credentials>) {
        self.creds = creds;
    }

    fn current(&mut self, folder: i64, new: bool) -> Pending<Result<Current, Refusal>> {
        let answer = self.call(
            requests::current(folder, new),
            "the conversation's thread",
            requests::read_current,
        );
        Box::pin(async move {
            match answer.await {
                Ok(c) => {
                    tracing::info!(
                        folder,
                        new,
                        thread = c.thread.row.id,
                        rolled_over = c.rolled_over,
                        reason = ?c.reason,
                        note = ?c.note,
                        "voice: the conversation's current thread"
                    );
                    Ok(Current {
                        last_message_at: c.thread.row.last_message_at,
                        thread: ThreadRef {
                            id: c.thread.row.id,
                            title: c.thread.row.title,
                        },
                        rolled_over: c.rolled_over,
                    })
                }
                Err(e) => {
                    tracing::warn!(folder, status = e.status, code = ?e.code, message = %e.message, "voice: current refused");
                    Err(e)
                }
            }
        })
    }

    fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>> {
        let answer = self.call(requests::folders(), "the folders", requests::read_folders);
        Box::pin(async move {
            answer
                .await
                .map(|l| l.folders.iter().map(FolderInfo::from).collect())
        })
    }

    fn create_folder(&mut self, name: &str) -> Pending<Result<FolderInfo, Refusal>> {
        // Only the name: the model, ongoing and the retention are the owner's (§4).
        let body = requests::FolderCreate {
            name: name.to_owned(),
            ..Default::default()
        };
        let answer = self.call(
            requests::create_folder(&body),
            "a new folder",
            requests::read_folder,
        );
        Box::pin(async move { answer.await.map(|f| FolderInfo::from(&f)) })
    }

    fn profiles(&mut self) -> Pending<Result<ProfileList, Refusal>> {
        self.call(
            requests::profiles(),
            "the personality profiles",
            requests::read_profiles,
        )
    }

    fn set_folder_profile(
        &mut self,
        folder: i64,
        profile: Option<i64>,
    ) -> Pending<Result<FolderPatched, Refusal>> {
        self.call(
            requests::set_folder_profile(folder, profile),
            "the folder's personality",
            requests::read_folder_patched,
        )
    }

    fn approvals(
        &mut self,
        thread: i64,
        body: &requests::approvals::ApprovalsRequest,
    ) -> Pending<Result<(), Refusal>> {
        let req = requests::approvals(thread, body);
        let Some(creds) = self.creds.clone() else {
            return Box::pin(async { Err(Refusal::no_answer("Kai is not paired")) });
        };
        let timeout = self.settings.timeout;
        Box::pin(async move {
            let resp = send_head(&creds, req, None, timeout, "the approval").await?;
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                let body = read_body(resp, timeout, "the approval").await?;
                let e = requests::read_refusal(status, &body);
                return Err(Refusal {
                    status,
                    code: Some(e.code),
                    message: e.message,
                });
            }
            // The resumed turn streams to this request: read to its end beside (a request that
            // stopped reading would stop the turn). Its length is the model's, so no bound of
            // Kai's own; only the frames' sizes are logged, never their content.
            tokio::spawn(async move {
                let mut resp = resp;
                let mut bytes = 0usize;
                let ended = loop {
                    match resp.chunk().await {
                        Ok(Some(chunk)) => bytes += chunk.len(),
                        Ok(None) => break None,
                        Err(e) => break Some(describe(&e)),
                    }
                };
                match ended {
                    None => {
                        tracing::info!(thread, bytes, "approvals: the resumed turn's stream ended")
                    }
                    Some(e) => tracing::warn!(
                        thread,
                        bytes,
                        "approvals: the resumed turn's stream broke off: {e}"
                    ),
                }
            });
            Ok(())
        })
    }

    fn pending_approvals(
        &mut self,
        thread: i64,
    ) -> Pending<Result<Vec<requests::approvals::ApprovalRequest>, Refusal>> {
        // lmgw-client has no request for one thread yet: its shape, as the Chat's page reads it.
        let req = requests::Request {
            method: requests::Method::Get,
            path: format!("/chat/api/threads/{thread}"),
            headers: Vec::new(),
            body: None,
        };
        self.call(req, "the thread's waiting calls", read_pending_approvals)
    }

    fn transcribe(&mut self, thread: i64, wav: Vec<u8>) -> Pending<Result<String, Refusal>> {
        let req = requests::transcribe(thread, "audio/wav");
        let Some(creds) = self.creds.clone() else {
            return Box::pin(async { Err(Refusal::no_answer("Kai is not paired")) });
        };
        let timeout = self.settings.timeout;
        Box::pin(async move {
            let what = "the spoken answer";
            let resp = send_head(&creds, req, Some(wav), timeout, what).await?;
            let status = resp.status().as_u16();
            let body = read_body(resp, timeout, what).await?;
            requests::read_transcription(status, &body)
                .map(|d| d.text)
                .map_err(|e| Refusal {
                    status,
                    code: Some(e.code),
                    message: e.message,
                })
        })
    }

    fn proxy(&mut self, forward: Forward, limits: ProxyLimits) -> Pending<Proxied> {
        let Some(creds) = self.creds.clone() else {
            return Box::pin(async {
                Proxied::refused(503, "not_paired", "Kai is not paired with lmgw")
            });
        };
        Box::pin(async move { super::proxy::send(&creds, forward, limits).await })
    }

    fn open(
        &mut self,
        thread: i64,
        take_over: bool,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Box<dyn VoiceSocket> {
        let (tx, rx) = mpsc::unbounded_channel();
        let closing = Arc::new(Notify::new());
        let queued = Arc::new(AtomicUsize::new(0));
        let socket = Socket {
            tx,
            closing: closing.clone(),
            closed: false,
            queued: queued.clone(),
            max_samples: (self.settings.send_queue.as_millis() as usize)
                .saturating_mul(RATE as usize)
                / 1000,
            limit: self.settings.send_queue,
            dropped: 0,
            events: events.downgrade(),
        };
        let prepared = match self.creds.clone() {
            None => Err("Kai is not paired".to_owned()),
            Some(creds) => self
                .handshake(&creds, thread, take_over)
                .map(|r| (r, creds.tls)),
        };
        match prepared {
            Err(message) => {
                let _ = events.send(SessionEvent::Refused {
                    status: 0,
                    code: None,
                    message,
                });
            }
            Ok((request, tls)) => {
                tracing::info!(thread, take_over, url = %request.uri(), "voice: opening the session");
                let (keepalive, timeout) = (self.settings.keepalive, self.settings.timeout);
                tokio::spawn(run(
                    request,
                    Link {
                        tls,
                        keepalive,
                        timeout,
                    },
                    rx,
                    closing,
                    queued,
                    events,
                ));
            }
        }
        Box::new(socket)
    }
}

/// What the session's task sends.
enum Out {
    /// The event, and the `event_id` it carries (`VoiceSocket::send_tagged`).
    Event(ClientEvent, Option<String>),
    Close,
}

/// One session's sending side (the task holds the socket).
struct Socket {
    tx: mpsc::UnboundedSender<Out>,
    /// Gives up a handshake not answered yet.
    closing: Arc<Notify>,
    closed: bool,
    /// Audio samples queued and not written yet (the task counts them down), and the bound.
    queued: Arc<AtomicUsize>,
    max_samples: usize,
    /// `voice.send_queue_s`, for the note.
    limit: Duration,
    /// Samples dropped in the current overflow (one note per overflow).
    dropped: usize,
    /// For the note; weak, so the channel still ends with the task (`Closed` is the last event).
    events: mpsc::WeakUnboundedSender<SessionEvent>,
}

impl VoiceSocket for Socket {
    fn send(&mut self, event: ClientEvent) {
        self.queue(event, None);
    }

    fn send_tagged(&mut self, event: ClientEvent, event_id: String) {
        self.queue(event, Some(event_id));
    }

    fn close(&mut self) {
        if !std::mem::replace(&mut self.closed, true) {
            let _ = self.tx.send(Out::Close);
            self.closing.notify_one();
        }
    }
}

impl Socket {
    /// Queues `event` (with `event_id` when tagged), audio within `voice.send_queue_s`.
    fn queue(&mut self, event: ClientEvent, event_id: Option<String>) {
        if self.closed {
            return;
        }
        let n = append_samples(&event);
        if n > 0 {
            if self.queued.load(Ordering::Acquire) + n > self.max_samples {
                if self.dropped == 0 {
                    let note = format!(
                        "lmgw does not take the voice session's audio: {} s are queued \
                         (voice.send_queue_s); the microphone's audio is dropped until it does",
                        self.limit.as_secs()
                    );
                    tracing::warn!("voice: {note}");
                    if let Some(events) = self.events.upgrade() {
                        let _ = events.send(SessionEvent::Note(note));
                    }
                }
                self.dropped += n;
                return;
            }
            if self.dropped > 0 {
                tracing::warn!(
                    dropped_ms = self.dropped as u64 * 1000 / RATE as u64,
                    "voice: the outgoing queue has room again; this much audio was dropped"
                );
                self.dropped = 0;
            }
            self.queued.fetch_add(n, Ordering::AcqRel);
        }
        let _ = self.tx.send(Out::Event(event, event_id));
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // A session dropped without its close still ends the socket.
        self.close();
    }
}

/// What a session's task connects with: the device link's TLS, `voice.keepalive_s` (module doc),
/// and `lmgw.timeout_s` for a refusal's body after its head.
struct Link {
    tls: super::tls::Tls,
    keepalive: Duration,
    timeout: Duration,
}

/// The session's task: the handshake, then its frames both ways until the socket ends; exactly
/// one `Closed` (or `Refused`) at the end.
async fn run(
    request: tungstenite::handshake::client::Request,
    link: Link,
    mut out: mpsc::UnboundedReceiver<Out>,
    closing: Arc<Notify>,
    queued: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<SessionEvent>,
) {
    let keepalive = link.keepalive;
    let config = WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None);
    let connect = link
        .tls
        .websocket(request, Some(config), true, link.timeout);
    let connected = tokio::select! {
        r = connect => r,
        () = closing.notified() => {
            tracing::info!("voice: closed before lmgw answered the handshake");
            let _ = events.send(SessionEvent::Closed {
                code: CloseCode::Normal.into(),
                reason: "closed before lmgw answered the handshake".into(),
            });
            return;
        }
    };
    let ws = match connected {
        Ok((ws, _)) => ws,
        Err(tungstenite::Error::Http(resp)) => {
            let status = resp.status().as_u16();
            let body = refusal_body(&resp);
            let e = requests::read_refusal(status, &body);
            // The body was read to its end (`tls::ws`, review KAI-4 K4-5); one that never came, or
            // a body without lmgw's error, carries no code (`read_refusal` says `http_<status>`
            // then), and the voice decides by the status, which always came.
            let code = (e.code != format!("http_{status}")).then_some(e.code);
            let _ = events.send(SessionEvent::Refused {
                status,
                code,
                message: e.message,
            });
            return;
        }
        Err(e) => {
            let _ = events.send(SessionEvent::Refused {
                status: 0,
                code: None,
                message: unreachable(&e),
            });
            return;
        }
    };
    if events.send(SessionEvent::Opened).is_err() {
        return;
    }
    let (mut sink, mut stream) = ws.split();
    // The server's close frame: the socket's end says it.
    let mut close_frame: Option<(u16, String)> = None;
    let mut sent_close = false;
    // K2-6: a ping every interval; two intervals without a word from lmgw, or a write it does not
    // take in that long, and the link is dead.
    let dead_after = keepalive.saturating_mul(2);
    let secs = dead_after.as_secs();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + keepalive, keepalive);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    let unwritten = || {
        format!(
            "lmgw took no data from the voice session for {secs} s (two voice.keepalive_s intervals): the link is dead"
        )
    };
    let broke = loop {
        tokio::select! {
            o = out.recv(), if !sent_close => match o {
                Some(Out::Event(e, event_id)) => {
                    let n = append_samples(&e);
                    let sent = tokio::time::timeout(
                        dead_after,
                        sink.send(Message::Text(frame(&e, event_id.as_deref()).into())),
                    )
                    .await;
                    queued.fetch_sub(n, Ordering::AcqRel);
                    match sent {
                        Err(_) => break Some(unwritten()),
                        Ok(Err(err)) => break Some(describe(&err)),
                        Ok(Ok(())) => {}
                    }
                }
                Some(Out::Close) | None => {
                    sent_close = true;
                    // A normal close (1000) that says whose it is; lmgw answers it once its journal
                    // drained (chat-voice §8.6).
                    let frame = CloseFrame {
                        code: CloseCode::Normal,
                        reason: CLOSE_REASON.into(),
                    };
                    match tokio::time::timeout(dead_after, sink.send(Message::Close(Some(frame)))).await {
                        Err(_) => break Some(unwritten()),
                        Ok(Err(err)) => break Some(describe(&err)),
                        Ok(Ok(())) => {}
                    }
                }
            },
            () = events.closed() => break None,
            _ = ping.tick() => {
                if heard.elapsed() >= dead_after {
                    break Some(format!(
                        "lmgw answered nothing on the voice session for {secs} s (two \
                         voice.keepalive_s intervals): the link is dead"
                    ));
                }
                if !sent_close {
                    match tokio::time::timeout(dead_after, sink.send(Message::Ping(Vec::new().into()))).await {
                        Err(_) => break Some(unwritten()),
                        Ok(Err(err)) => break Some(describe(&err)),
                        Ok(Ok(())) => {}
                    }
                }
            },
            m = stream.next() => {
                if matches!(m, Some(Ok(_))) {
                    heard = tokio::time::Instant::now();
                }
                match m {
                Some(Ok(Message::Text(text))) => match realtime::parse(&text) {
                    Some(ev) => {
                        if events.send(session_event(ev, &text)).is_err() {
                            // Nobody listens any more: the session is gone.
                            break None;
                        }
                    }
                    None => tracing::warn!(chars = text.len(), "voice: a frame that is no event, skipped"),
                },
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(f) => (u16::from(f.code), f.reason.as_str().to_owned()),
                        None => (CloseCode::Status.into(), String::new()),
                    };
                    tracing::info!(code, %reason, "voice: lmgw's close frame");
                    // The close handshake is complete from here: lmgw ends the connection next
                    // (tungstenite queued the answer to a close lmgw began; it goes out below).
                    close_frame = Some((code, reason));
                    break None;
                }
                Some(Ok(Message::Binary(b))) => {
                    tracing::debug!(bytes = b.len(), "voice: a binary frame, skipped");
                }
                Some(Ok(_)) => {}
                Some(Err(tungstenite::Error::ConnectionClosed)) | None => break None,
                Some(Err(e)) => break Some(describe(&e)),
                }
            },
        }
    };
    let (code, reason) = match (close_frame, broke) {
        (Some(c), _) => c,
        (None, Some(why)) => (CLOSE_DROPPED, why),
        (None, None) => (CLOSE_DROPPED, String::new()),
    };
    let _ = events.send(SessionEvent::Closed { code, reason });
    // The answer to a close lmgw began, best effort: the session is over either way.
    let _ = sink.flush().await;
}

/// A handshake refusal's body as text. tungstenite hands the body over as it came off the wire,
/// so a chunked one (`transfer-encoding: chunked`, as lmgw sends its bind's 404 and 409) is joined
/// here; without that, its code read as `http_<status>` and Kai could not tell a thread bound
/// elsewhere (K2-13) or out of reach from any other refusal. A body that does not read as chunks
/// is kept as it came. It is only what came in the read that completed the head: a body that came
/// later is missing or cut, and the voice then decides by the status (review KAI-4 K4-5).
pub(crate) fn refusal_body(resp: &tungstenite::http::Response<Option<Vec<u8>>>) -> String {
    let raw = resp.body().as_deref().unwrap_or_default();
    let chunked = resp
        .headers()
        .get_all(tungstenite::http::header::TRANSFER_ENCODING)
        .iter()
        .any(|v| {
            v.to_str()
                .is_ok_and(|v| v.to_ascii_lowercase().contains("chunked"))
        });
    let joined = if chunked { dechunk(raw) } else { None };
    String::from_utf8_lossy(joined.as_deref().unwrap_or(raw)).into_owned()
}
/// The text frame of `e`, with `event_id` when the voice tagged it (an `error` about it echoes the
/// id, api §1: OpenAI's `error.event_id`).
fn frame(e: &ClientEvent, event_id: Option<&str>) -> String {
    let text = e.to_json();
    let Some(id) = event_id else { return text };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(mut o)) => {
            o.insert("event_id".into(), id.into());
            serde_json::Value::Object(o).to_string()
        }
        _ => text,
    }
}

/// A parsed server event as the voice gets it: an `error` echoing a client event's id
/// (`error.event_id`) is `ErrorFor`, every other one `Server`.
fn session_event(ev: realtime::ServerEvent, text: &str) -> SessionEvent {
    if let realtime::ServerEvent::Error(error) = ev {
        let id = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v["error"]["event_id"].as_str().map(str::to_owned))
            .filter(|id| !id.is_empty());
        return match id {
            Some(event_id) => SessionEvent::ErrorFor { event_id, error },
            None => SessionEvent::Server(realtime::ServerEvent::Error(error)),
        };
    }
    SessionEvent::Server(ev)
}

/// The data of an HTTP/1.1 chunked body: each chunk's hex size (extensions after `;` ignored), its
/// bytes, up to the empty last chunk or the end of what came. `None` when it does not read as
/// chunks.
fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    while !rest.is_empty() {
        let end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&rest[..end]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[end + 2..];
        if size == 0 {
            break;
        }
        let data = rest.get(..size)?;
        out.extend_from_slice(data);
        rest = &rest[size..];
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
    }
    Some(out)
}

#[cfg(test)]
mod tests;

/// `GET /chat/api/threads/{id}`'s answer, as far as WP19 reads it: every call its messages'
/// `pending_approvals` list (lmgw lists only the calls a reply still waits on).
pub(super) fn read_pending_approvals(
    status: u16,
    body: &str,
) -> Result<Vec<requests::approvals::ApprovalRequest>, requests::ApiError> {
    #[derive(serde::Deserialize)]
    struct Message {
        #[serde(default)]
        pending_approvals: Option<Vec<requests::approvals::ApprovalRequest>>,
    }
    #[derive(serde::Deserialize)]
    struct Thread {
        #[serde(default)]
        messages: Vec<Message>,
    }
    if !(200..300).contains(&status) {
        return Err(requests::read_refusal(status, body));
    }
    serde_json::from_str::<Thread>(body)
        .map(|t| {
            t.messages
                .into_iter()
                .flat_map(|m| m.pending_approvals.unwrap_or_default())
                .collect()
        })
        .map_err(|e| requests::ApiError {
            code: "unreadable".into(),
            message: format!("the thread does not read: {e}"),
            ..Default::default()
        })
}
