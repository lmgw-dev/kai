//! The seam where the realtime client plugs in (§5, WP4).
//!
//! The voice session talks to lmgw only through [`VoiceLink`]: it asks for the conversation's
//! current thread (§4, `POST /chat/api/folders/{id}/current`) and opens a session bound to it
//! (`GET /v1/realtime?chat_thread=<id>`, chat-voice §8.1). The conversation's folder (§4,
//! `crate::conversation`) is looked up and created through it too (`GET` and `POST
//! /chat/api/folders`), so the device link's one HTTP client serves both. The events are `lmgw-client`'s own
//! (K3, api §4.1): [`ClientEvent`] is what the client owns in a bound session, built to its text
//! frame by `ClientEvent::to_json`, and [`ServerEvent`] is what the session acts on, parsed from a
//! text frame by `lmgw_client::realtime::parse`. So the adapter that implements this trait,
//! `lmgw_link::RealtimeLink`, only moves text frames over a WebSocket; the tests run the scripted
//! server in `voice::mock`, and the core's tests that do not drive the voice run [`NoVoice`].
//!
//! - **Client events** ([`ClientEvent`]): turn detection and `half_duplex` in `session.update`,
//!   the input buffer (`ClientEvent::append_pcm16` makes an append of samples), the response, the
//!   truncate.
//! - **Server events** ([`ServerEvent`]): only those a client acts on, typed; every other `type`,
//!   and a typed one whose fields do not read, arrives as `ServerEvent::Unknown` with its `type`
//!   and data (the enum is `#[non_exhaustive]`). Audio deltas carry PCM16 little-endian bytes
//!   (`lmgw_client::base64::pcm16_samples` makes samples of them).
//! - **The socket's life** ([`SessionEvent`]): the handshake's answer, every server event in order,
//!   then exactly one [`SessionEvent::Closed`] (or [`SessionEvent::Refused`] before the 101). Each
//!   [`VoiceLink::open`] gets its own channel, so events of an older socket never reach a newer
//!   session.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

pub use crate::conversation::FolderInfo;
use crate::lmgw_link::Credentials;
use crate::lmgw_link::proxy::{Forward, Limits as ProxyLimits, Proxied};
pub use lmgw_client::requests::{FolderPatched, ProfileList};

pub use lmgw_client::realtime::{
    CLOSE_REVOKED, ClientEvent, CloseKind, ErrorFacts, RevokeKind, SHUTTING_DOWN, ServerEvent,
    TurnDetection, close_kind,
};
#[cfg(test)]
pub use lmgw_client::realtime::{ChatReply, ModelState, SessionFacts, ThreadFacts};

/// A boxed future the session polls in its select loop (cancel-safe: it stays stored).
pub type Pending<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A request lmgw refused, or one that got no answer: the HTTP status (`0` when none came), the
/// refusal's code (`folder_no_model`, `not_ongoing`, `not_found`…) and its message, or what
/// happened instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl Refusal {
    /// No answer from lmgw: why.
    pub fn no_answer(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            code: None,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        self.code.as_deref().unwrap_or_default()
    }

    /// A 401: the pairing opens nothing now (§3.3): `device_key_unknown` (re-paired or deleted),
    /// `device_disabled`, `key_expired`, `device_certificate_expired`
    /// (`lmgw_client::requests::key_refused`), or any other 401.
    pub fn key_refused(&self) -> bool {
        self.status == 401
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(c) => write!(f, "{} ({c})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// `current`'s answer: the conversation's thread, whether this call started it (a new thread has
/// no message yet, so it is never idle), and when its newest message was written (unix seconds,
/// lmgw's `last_message_at`; `None` for a thread with none). The idle rollover counts from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    pub thread: ThreadRef,
    pub rolled_over: bool,
    pub last_message_at: Option<i64>,
}

/// A Chat thread: what `current` answers, and `session.lmgw.resolved.chat_thread`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRef {
    pub id: i64,
    pub title: String,
}

impl From<lmgw_client::realtime::ThreadFacts> for ThreadRef {
    fn from(t: lmgw_client::realtime::ThreadFacts) -> Self {
        Self {
            id: t.id,
            title: t.title,
        }
    }
}

/// The life of one opened socket, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The handshake was answered with 101: the session is bound; `session.created` follows.
    Opened,
    /// Refused before the 101: the status, `code` and `message`, as
    /// `lmgw_client::requests::read_refusal` reads the answer's body (the handshake's
    /// `{"error": {code, message}}` and the Chat API's flat `{code, message}` alike). `code` is
    /// none where the body carried none: none was sent, or it did not come with the answer's head
    /// (review KAI-4 K4-5); the status, which always came, then decides. Status 0: no answer.
    Refused {
        status: u16,
        code: Option<String>,
        message: String,
    },
    Server(ServerEvent),
    /// An `error` that echoes the `event_id` of a client event Kai sent with one
    /// ([`VoiceSocket::send_tagged`]): lmgw refused that event. Every other `error` comes as
    /// `Server(ServerEvent::Error)`.
    ErrorFor {
        event_id: String,
        error: ErrorFacts,
    },
    /// Something to say about the socket that does not end it: the outgoing queue overflowed and
    /// audio is dropped (`voice.send_queue_s`, review KAI-2 K2-6).
    Note(String),
    /// The socket ended: the close code and reason (1006 when it dropped without a close frame).
    /// Always the last event.
    Closed {
        code: u16,
        reason: String,
    },
}

/// A drop: the socket ended without a close frame.
pub const CLOSE_DROPPED: u16 = 1006;

/// The refusal of a bind that does not take over while another session holds the thread
/// (`lmgw_client::requests::realtime_unless_bound`, review KAI-2 K2-13).
pub const CHAT_THREAD_BOUND: &str = lmgw_client::requests::code::CHAT_THREAD_BOUND;

/// A client event's `type`, for the log.
pub fn event_kind(event: &ClientEvent) -> &'static str {
    event.kind()
}

/// The samples an append carries (PCM16, from its base64's length); 0 for any other event.
pub fn append_samples(event: &ClientEvent) -> usize {
    match event {
        ClientEvent::Append { audio } => audio.trim_end_matches('=').len() * 3 / 4 / 2,
        _ => 0,
    }
}

/// One opened session's sending side.
pub trait VoiceSocket: Send {
    /// Sends `event` in order after everything sent before. Events sent before the 101 wait. The
    /// queue holds at most `voice.send_queue_s` of audio: an append beyond it is dropped (said once
    /// as [`SessionEvent::Note`]); every other event is always queued.
    fn send(&mut self, event: ClientEvent);
    /// [`Self::send`] with `event_id` on the event, so an `error` about it echoes the id
    /// ([`SessionEvent::ErrorFor`]). A socket that cannot tag sends it plain.
    fn send_tagged(&mut self, event: ClientEvent, event_id: String) {
        let _ = event_id;
        self.send(event);
    }
    /// A normal close (1000), after everything sent before. [`SessionEvent::Closed`] follows once
    /// lmgw answered it (it answers after its journal drained, chat-voice §8.6) or the socket ended.
    fn close(&mut self);
}

/// What the voice session and its conversation's folder need of lmgw.
pub trait VoiceLink: Send + 'static {
    /// The key and URL for what follows (`None`: not paired; every call then fails).
    fn set_credentials(&mut self, creds: Option<Credentials>);
    /// The conversation's current thread (§4): `POST /chat/api/folders/{folder}/current`, `{new:
    /// true}` for a new conversation. A refusal carries the server's code (`folder_no_model`,
    /// `not_ongoing`, `not_found`) and message.
    fn current(&mut self, folder: i64, new: bool) -> Pending<Result<Current, Refusal>>;
    /// Every Chat folder the device sees (`GET /chat/api/folders`, §4's lookup).
    fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>>;
    /// Creates a plain folder named `name` (`POST /chat/api/folders {name}`): no model, no
    /// retention, not ongoing (§4; the owner sets those on the dashboard).
    fn create_folder(&mut self, name: &str) -> Pending<Result<FolderInfo, Refusal>>;
    /// Every personality profile and the one new threads start with (`GET /chat/api/profiles`,
    /// WP22). A link without the route answers that it has none.
    fn profiles(&mut self) -> Pending<Result<ProfileList, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer("this link reads no profiles")) })
    }
    /// The folder's conversation talks with profile `profile` from now on (`None`: the default),
    /// its current thread included (`POST /chat/api/folders/{folder}` with `defaults_patch`,
    /// WP22).
    fn set_folder_profile(
        &mut self,
        _folder: i64,
        _profile: Option<i64>,
    ) -> Pending<Result<FolderPatched, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer("this link sets no profiles")) })
    }
    /// One request of the configuration window's Personality page, sent to lmgw with the device
    /// key (`crate::lmgw_link::proxy`, WP21b): lmgw's answer for the page, or Kai's refusal in
    /// lmgw's error shape. A link without it says so.
    fn proxy(&mut self, _forward: Forward, _limits: ProxyLimits) -> Pending<Proxied> {
        Box::pin(async { Proxied::refused(502, "no_proxy", "this link forwards nothing to lmgw") })
    }
    /// `POST /chat/api/threads/{thread}/approvals` with the verdicts of every call the thread's
    /// last reply waits on (WP19, api §6.3). `Ok` once lmgw took them (its answer's head); the
    /// resumed turn's stream is read to its end beside, since a request that stopped reading would
    /// stop the turn. A refusal carries lmgw's code (`approval_out_of_scope`, `approval_decided`…).
    fn approvals(
        &mut self,
        _thread: i64,
        _body: &lmgw_client::requests::approvals::ApprovalsRequest,
    ) -> Pending<Result<(), Refusal>> {
        Box::pin(async { Err(Refusal::no_answer("this link decides no approvals")) })
    }
    /// The calls the thread's last reply waits on now (`GET /chat/api/threads/{thread}`, its
    /// messages' `pending_approvals`, WP19): what a resync or a refused answer reads again.
    fn pending_approvals(
        &mut self,
        _thread: i64,
    ) -> Pending<Result<Vec<lmgw_client::requests::approvals::ApprovalRequest>, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer("this link reads no threads")) })
    }
    /// `POST /chat/api/threads/{thread}/transcribe` with a WAV recording (K17): its words, written
    /// nowhere.
    fn transcribe(&mut self, _thread: i64, _wav: Vec<u8>) -> Pending<Result<String, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer("this link transcribes nothing")) })
    }
    /// Opens `/v1/realtime?chat_thread=<thread>` over the device link. `take_over`: the owner's
    /// own bind (a press, the tray), which takes the thread from another session; Kai's own binds
    /// (a rebind, `voice.on_at_start`) never do, and are refused 409 [`CHAT_THREAD_BOUND`] while
    /// another session holds it (`&takeover=never`, K5, review KAI-2 K2-13). Returns at once; the
    /// handshake's answer and everything after it arrive on `events`.
    fn open(
        &mut self,
        thread: i64,
        take_over: bool,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Box<dyn VoiceSocket>;
}

/// A link with no realtime client, for the core's tests that do not drive the voice: it says so.
#[cfg(test)]
#[derive(Default)]
pub struct NoVoice;

#[cfg(test)]
const NO_VOICE_NOTE: &str = "this test's Kai has no realtime client";

#[cfg(test)]
struct NoSocket;

#[cfg(test)]
impl VoiceSocket for NoSocket {
    fn send(&mut self, _event: ClientEvent) {}
    fn close(&mut self) {}
}

#[cfg(test)]
impl VoiceLink for NoVoice {
    fn set_credentials(&mut self, _creds: Option<Credentials>) {}

    fn current(&mut self, _folder: i64, _new: bool) -> Pending<Result<Current, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer(NO_VOICE_NOTE)) })
    }

    fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer(NO_VOICE_NOTE)) })
    }

    fn create_folder(&mut self, _name: &str) -> Pending<Result<FolderInfo, Refusal>> {
        Box::pin(async { Err(Refusal::no_answer(NO_VOICE_NOTE)) })
    }

    fn open(
        &mut self,
        _thread: i64,
        _take_over: bool,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Box<dyn VoiceSocket> {
        let _ = events.send(SessionEvent::Refused {
            status: 0,
            code: None,
            message: NO_VOICE_NOTE.to_owned(),
        });
        Box::new(NoSocket)
    }
}
