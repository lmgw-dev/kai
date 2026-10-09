//! A scripted lmgw for the voice session's tests, behind the [`VoiceLink`] seam: the folders (§4:
//! the list, a create, and `current`'s refusals for a folder that is missing, not ongoing or has no
//! model), `current` and a bound `/v1/realtime?chat_thread=<id>` session, driven by the test rather
//! than by a voice detector. What it does around a cut follows lmgw's own stand-in (`scripts/realtime-mock.js`,
//! from the WP8 server code):
//!
//! - `session.created` comes `created_after` after the open (a cold bind), with the thread and its
//!   resolved turn detection; `session.update` is answered with `session.updated`, `clear` with
//!   `cleared`, a commit with `committed` and the transcript, and `response.create` with a
//!   response when `reply` is set;
//! - a response: `response.created`, audio paced at real time in 100 ms deltas with a 300 ms
//!   lead, `output_audio.done`, `response.done` (`done_lag` later: a bound response is done only
//!   once pacing's modelled playback drained, chat-voice §8.2), the reply as stored
//!   (`lmgw.chat.reply`) and `lmgw.response.timing`;
//! - a barge-in (`speech_started` while a reply plays) cancels the response at once
//!   (`turn_detected`) and its journal cuts the reply at what was *sent*; the client's truncate,
//!   a flush later, re-cuts it at what was heard (a second `lmgw.chat.reply`);
//! - a truncate of an item still being produced cancels the rest of its response
//!   (`client_cancelled`); one past the audio sent is refused (`invalid_value`);
//! - the cut keeps the reply's words by share of its audio, back to the last whole word;
//! - a takeover is `error chat_thread_taken_over` naming the binder, then close 4000; a drop is a
//!   close 1006 with no reason.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use super::link::{
    ChatReply, ClientEvent, Current, ErrorFacts, FolderInfo, FolderPatched, ModelState, Pending,
    ProfileList, Refusal, ServerEvent, SessionEvent, SessionFacts, ThreadFacts, ThreadRef,
    TurnDetection, VoiceLink, VoiceSocket,
};
use crate::audio::RATE;
use crate::lmgw_link::Credentials;

/// Audio sent ahead of real time, as lmgw's pacing does.
const LEAD_MS: u64 = 300;
/// One audio delta.
const DELTA_MS: u64 = 100;

/// A reply's state on the server.
#[derive(Debug, Clone, Default)]
pub struct MockReply {
    pub response_id: String,
    pub item_id: String,
    pub text: String,
    pub total_ms: u64,
    pub sent_ms: u64,
    pub audio_done: bool,
    pub open: bool,
    /// Why it was cancelled.
    pub cancelled: Option<String>,
}

/// One opened socket, as the server saw it.
#[derive(Debug, Default)]
pub struct MockSocket {
    pub thread: i64,
    pub tx: Option<UnboundedSender<SessionEvent>>,
    /// Every client event but the appends, in order.
    pub sent: Vec<ClientEvent>,
    /// The order in which the event kinds came (appends included), for §5.3's order.
    pub kinds: Vec<&'static str>,
    pub appended: usize,
    /// Of `appended`, the samples that were silence.
    pub silent: usize,
    /// The appended audio itself, when `MockState::keep_audio` (the live check).
    pub audio: Vec<i16>,
    pub closed_by_client: bool,
    /// The bind took the thread over (the owner's), or not (Kai's own, `&takeover=never`).
    pub take_over: bool,
    /// Sessions still holding their slot when this one opened (a close not answered yet): one
    /// bind in flight means 0 (review KAI-2 K2-7).
    pub open_beside: usize,
    pub replies: Vec<MockReply>,
    /// What the journal stored for each reply: (response id, heard content or `None` removed).
    pub stored: Vec<(String, Option<String>)>,
    /// `(item_id, audio_end_ms, sent_ms)`.
    pub truncates: Vec<(String, u64, u64)>,
    pub refused_truncates: usize,
    /// The `event_id`s of tagged events (`VoiceSocket::send_tagged`), in order.
    pub tagged: Vec<String>,
}

impl MockSocket {
    fn emit(&self, ev: ServerEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(SessionEvent::Server(ev));
        }
    }

    fn end(&mut self, code: u16, reason: &str) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(SessionEvent::Closed {
                code,
                reason: reason.to_owned(),
            });
        }
    }

    /// The turn detections sent in `session.update`s, in order.
    pub fn detections(&self) -> Vec<Option<TurnDetection>> {
        self.sent
            .iter()
            .filter_map(|e| match e {
                ClientEvent::SessionUpdate { turn_detection, .. } => Some(*turn_detection),
                _ => None,
            })
            .collect()
    }

    pub fn count(&self, kind: &str) -> usize {
        self.kinds.iter().filter(|k| **k == kind).count()
    }

    pub fn cancels(&self) -> usize {
        self.count("response.cancel")
    }
}

/// The folder every rig starts with: "Kai", ready (a model, ongoing with no idle rollover), keeping
/// its history (never archived).
pub fn kai_folder() -> FolderInfo {
    FolderInfo {
        id: 3,
        name: "Kai".into(),
        model: Some("kai-chat".into()),
        idle_minutes: Some(0),
        archive_days: Some(0),
        purge_days: None,
        tool_labels: Vec::new(),
        profile_id: None,
    }
}

/// A refusal as lmgw words it.
pub fn refusal(status: u16, code: &str, message: &str) -> Refusal {
    Refusal {
        status,
        code: Some(code.into()),
        message: message.into(),
    }
}

#[derive(Debug)]
pub struct MockState {
    /// What `current` answers.
    pub thread: ThreadRef,
    /// Whether the next `current` answer started its thread (a new conversation always does).
    pub rolled_over: bool,
    /// The thread's newest message, unix seconds, as `current` answers it (`None`: none, or a
    /// gateway that does not say).
    pub last_message_at: Option<i64>,
    /// `current` calls (`new`).
    pub current_calls: Vec<bool>,
    /// The folder each `current` call asked.
    pub current_folders: Vec<i64>,
    /// `current` fails with this.
    pub current_error: Option<Refusal>,
    /// What `current` answers first, one each, before `thread`: a thread that goes before Kai's
    /// bind to it arrives (the bind's 404), or one that came back into reach.
    pub answers_first: Vec<ThreadRef>,
    /// How long `current` takes (an HTTP round trip).
    pub current_lag: Duration,
    /// The next `current` calls, one each: `Some` fails with it, `None` answers as usual.
    pub current_script: Vec<Option<Refusal>>,
    /// The next binds are refused with these, one each, before the 101: (status, code, message);
    /// a code of `None` is a body that did not come with the answer's head (review KAI-4 K4-5).
    pub refuse_opens: Vec<(u16, Option<String>, String)>,
    /// The folders the device sees; `current` on one that is missing, not ongoing or without a
    /// model is refused as lmgw refuses it.
    pub folders: Vec<FolderInfo>,
    /// `GET /chat/api/folders` calls.
    pub folder_lists: usize,
    /// The names `POST /chat/api/folders` was sent.
    pub created: Vec<String>,
    /// The folder list fails with this.
    pub folders_error: Option<Refusal>,
    /// What `GET /chat/api/profiles` answers (WP22), and how it fails.
    pub profiles: ProfileList,
    pub profiles_error: Option<Refusal>,
    /// `GET /chat/api/profiles` calls.
    pub profile_lists: usize,
    /// The folder profile writes, `(folder, profile)`, and how they fail.
    pub profile_sets: Vec<(i64, Option<i64>)>,
    pub profile_set_error: Option<Refusal>,
    /// A folder profile write answers this long after it was sent.
    pub profile_set_delay: Duration,
    /// `session.created` comes this long after the open; `None`: never.
    pub created_after: Option<Duration>,
    /// The thread's resolved turn detection (`None`: push-to-talk).
    pub resolved: Option<String>,
    /// A reply to every `response.create`: its text and audio length.
    pub reply: Option<(String, u64)>,
    /// `response.done` comes this long after the audio's `done`.
    pub done_lag: Duration,
    /// Keep the appended audio (`MockSocket::audio`).
    pub keep_audio: bool,
    /// The close is answered this long after the client's close (the journal drains first).
    pub close_lag: Duration,
    /// Threads another client's session holds: a bind that does not take over is refused 409
    /// `chat_thread_bound` naming it (K2-13).
    pub bound_elsewhere: HashMap<i64, String>,
    pub sockets: Vec<MockSocket>,
    pub creds: Option<String>,
    /// `POST …/approvals` (WP19): the thread and the verdicts, and how it fails.
    pub approvals_sent: Vec<(i64, lmgw_client::requests::approvals::ApprovalsRequest)>,
    pub approvals_error: Option<Refusal>,
    /// `POST …/transcribe`: what the next ones answer, one each (then "ja"), and what was sent:
    /// the thread and the WAV's length in bytes.
    pub transcripts: Vec<Result<String, Refusal>>,
    pub transcribed: Vec<(i64, usize)>,
    /// `GET /chat/api/threads/{id}`'s waiting calls by thread (none listed: none wait), and the
    /// threads read.
    pub pending: HashMap<i64, Vec<lmgw_client::requests::approvals::ApprovalRequest>>,
    pub pending_read: Vec<i64>,
    /// The next tagged `response.create`s are refused with these codes, one each: an `error`
    /// echoing the create's `event_id` and no response (lmgw's refusal of a create, WP19's M1).
    pub refuse_creates: Vec<String>,
    next: u64,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            thread: ThreadRef {
                id: 812,
                title: "Kai".into(),
            },
            rolled_over: false,
            last_message_at: None,
            current_calls: Vec::new(),
            current_folders: Vec::new(),
            current_error: None,
            answers_first: Vec::new(),
            current_lag: Duration::from_millis(20),
            current_script: Vec::new(),
            refuse_opens: Vec::new(),
            folders: vec![kai_folder()],
            folder_lists: 0,
            created: Vec::new(),
            folders_error: None,
            profiles: ProfileList::default(),
            profiles_error: None,
            profile_lists: 0,
            profile_sets: Vec::new(),
            profile_set_error: None,
            profile_set_delay: Duration::ZERO,
            created_after: Some(Duration::from_millis(200)),
            resolved: Some("server_vad".into()),
            reply: Some((
                "Es ist halb neun, und draußen regnet es leicht.".into(),
                1500,
            )),
            done_lag: Duration::ZERO,
            keep_audio: false,
            close_lag: Duration::ZERO,
            bound_elsewhere: HashMap::new(),
            sockets: Vec::new(),
            creds: None,
            approvals_sent: Vec::new(),
            approvals_error: None,
            refuse_creates: Vec::new(),
            transcripts: Vec::new(),
            transcribed: Vec::new(),
            pending: HashMap::new(),
            pending_read: Vec::new(),
            next: 0,
        }
    }
}

#[derive(Clone, Default)]
pub struct MockLink {
    pub st: Arc<Mutex<MockState>>,
}

struct Socket {
    st: Arc<Mutex<MockState>>,
    n: usize,
}

/// The reply's words heard at `ms` of `total_ms`: by share, back to the last whole word.
pub fn heard_text(text: &str, total_ms: u64, ms: u64) -> String {
    if ms >= total_ms {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = (chars.len() as u64 * ms / total_ms.max(1)) as usize;
    let cut: String = chars[..n].iter().collect();
    if n < chars.len() && chars[n] != ' ' {
        match cut.rfind(' ') {
            Some(i) => cut[..i].trim_end().to_owned(),
            None => String::new(),
        }
    } else {
        cut.trim_end().to_owned()
    }
}

fn facts(st: &MockState, detection: Option<Option<TurnDetection>>) -> SessionFacts {
    let td = match detection {
        Some(d) => d.map(|d| d.key().to_owned()),
        None => st.resolved.clone(),
    };
    SessionFacts {
        prefix_padding_ms: td.as_ref().map(|_| 300),
        turn_detection: td,
        half_duplex: Some(false),
        thread: Some(ThreadFacts {
            id: st.thread.id,
            title: st.thread.title.clone(),
            admin_tools: false,
        }),
        chat: Some("mock/chat".into()),
        asr: Some("mock/asr".into()),
        tts: Some("mock/tts".into()),
        voice: Some("mock-voice".into()),
    }
}

impl MockLink {
    pub fn with<R>(&self, f: impl FnOnce(&mut MockState) -> R) -> R {
        f(&mut self.st.lock().unwrap())
    }

    pub fn opens(&self) -> usize {
        self.with(|s| s.sockets.len())
    }

    /// Reads socket `n` (0-based, in open order).
    pub fn socket<R>(&self, n: usize, f: impl FnOnce(&MockSocket) -> R) -> R {
        self.with(|s| f(&s.sockets[n]))
    }

    fn last(&self) -> usize {
        self.opens().checked_sub(1).expect("a socket was opened")
    }

    pub fn emit(&self, ev: ServerEvent) {
        let n = self.last();
        self.with(|s| s.sockets[n].emit(ev));
    }

    /// The server heard speech start: on a playing reply that is a barge-in, which cancels it.
    pub fn speech_started(&self) {
        let n = self.last();
        self.with(|s| {
            let sock = &mut s.sockets[n];
            sock.emit(ServerEvent::SpeechStarted);
            if let Some(r) = sock.replies.iter_mut().find(|r| r.open) {
                r.open = false;
                r.cancelled = Some("turn_detected".into());
                let (rid, heard) = (
                    r.response_id.clone(),
                    heard_text(&r.text, r.total_ms, r.sent_ms),
                );
                sock.stored.push((rid.clone(), Some(heard)));
                sock.emit(ServerEvent::ResponseDone {
                    response_id: rid,
                    status: "cancelled".into(),
                    reason: Some("turn_detected".into()),
                });
            }
        });
    }

    /// A whole spoken turn as the server detects it: speech, its end, the commit and transcript,
    /// then a reply.
    pub fn speak(&self) {
        self.speech_started();
        self.emit(ServerEvent::SpeechStopped);
        self.emit(ServerEvent::Committed);
        self.emit(ServerEvent::Transcript("Wie spät ist es?".into()));
        let reply = self.with(|s| s.reply.clone());
        if let Some((text, ms)) = reply {
            self.respond(&text, ms);
        }
    }

    /// A response with `ms` of audio, paced at real time (module doc).
    pub fn respond(&self, text: &str, ms: u64) {
        self.respond_with(text, ms, None);
    }

    /// A response whose audio is `pcm` (24 kHz), paced at real time.
    #[allow(dead_code)] // the live check
    pub fn respond_pcm(&self, text: &str, pcm: Vec<i16>) {
        let ms = pcm.len() as u64 * 1000 / RATE as u64;
        self.respond_with(text, ms, Some(Arc::new(pcm)));
    }

    fn respond_with(&self, text: &str, ms: u64, audio: Option<Arc<Vec<i16>>>) {
        let n = self.last();
        let (rid, item, tx) = self.with(|s| {
            s.next += 1;
            let rid = format!("resp_{}", s.next);
            let item = format!("item_a{}", s.next);
            let sock = &mut s.sockets[n];
            sock.replies.push(MockReply {
                response_id: rid.clone(),
                item_id: item.clone(),
                text: text.to_owned(),
                total_ms: ms,
                open: true,
                ..MockReply::default()
            });
            sock.emit(ServerEvent::ResponseCreated {
                response_id: rid.clone(),
            });
            (rid, item, sock.tx.clone())
        });
        let Some(_) = tx else { return };
        let st = self.st.clone();
        tokio::spawn(async move {
            let t0 = tokio::time::Instant::now();
            loop {
                let (go_on, sent) = {
                    let mut s = st.lock().unwrap();
                    let sock = &mut s.sockets[n];
                    let Some(r) = sock.replies.iter_mut().find(|r| r.response_id == rid) else {
                        return;
                    };
                    if r.cancelled.is_some() || sock.tx.is_none() {
                        return;
                    }
                    if r.sent_ms >= r.total_ms {
                        (false, r.sent_ms)
                    } else {
                        let len = DELTA_MS.min(r.total_ms - r.sent_ms);
                        let samples = (len * RATE as u64 / 1000) as usize;
                        let from = (r.sent_ms * RATE as u64 / 1000) as usize;
                        r.sent_ms += len;
                        let at = r.sent_ms;
                        let pcm = match &audio {
                            Some(a) => a[from.min(a.len())..(from + samples).min(a.len())].to_vec(),
                            None => vec![3000i16; samples],
                        };
                        sock.emit(ServerEvent::AudioDelta {
                            response_id: rid.clone(),
                            item_id: item.clone(),
                            pcm: lmgw_client::base64::pcm16_le_bytes(&pcm),
                        });
                        (true, at)
                    }
                };
                if !go_on {
                    break;
                }
                // Paced: the audio runs LEAD_MS ahead of real time.
                let due = t0 + Duration::from_millis(sent.saturating_sub(LEAD_MS));
                tokio::time::sleep_until(due).await;
            }
            let lag = {
                let mut s = st.lock().unwrap();
                let lag = s.done_lag;
                let sock = &mut s.sockets[n];
                let Some(r) = sock.replies.iter_mut().find(|r| r.response_id == rid) else {
                    return;
                };
                if r.cancelled.is_some() {
                    return;
                }
                r.audio_done = true;
                sock.emit(ServerEvent::AudioDone {
                    response_id: rid.clone(),
                });
                lag
            };
            if !lag.is_zero() {
                tokio::time::sleep(lag).await;
            }
            let mut s = st.lock().unwrap();
            let sock = &mut s.sockets[n];
            let Some(r) = sock.replies.iter_mut().find(|r| r.response_id == rid) else {
                return;
            };
            if r.cancelled.is_some() || sock.tx.is_none() {
                return;
            }
            r.open = false;
            let (total, text) = (r.total_ms, r.text.clone());
            sock.emit(ServerEvent::ResponseDone {
                response_id: rid.clone(),
                status: "completed".into(),
                reason: None,
            });
            sock.stored.push((rid.clone(), Some(text.clone())));
            sock.emit(ServerEvent::ChatReply(ChatReply {
                message_id: 900,
                content: Some(text),
                ..ChatReply::default()
            }));
            sock.emit(ServerEvent::Timing(json!({
                "response_id": rid, "message_id": 900, "end_of_turn_ms": 400, "asr_ms": 30,
                "first_token_ms": 200, "first_clause_ms": 100, "first_audio_ms": 40,
                "total_ms": total, "to_first_audio_ms": 800, "cold": [],
                "models": {"asr": {"alias": "mock/asr", "answered_by": null},
                           "chat": {"alias": "mock/chat", "answered_by": null},
                           "tts": {"alias": "mock/tts", "answered_by": null}}
            })));
        });
    }

    /// Another client binds the thread (chat-voice §8.1, api §1.7: the message names it).
    pub fn take_over(&self, by: &str) {
        let n = self.last();
        let msg = format!("voice mode moved to {by}");
        self.with(|s| {
            let sock = &mut s.sockets[n];
            sock.emit(ServerEvent::Error(ErrorFacts {
                kind: "invalid_request_error".into(),
                code: Some("chat_thread_taken_over".into()),
                message: msg.clone(),
            }));
            sock.end(4000, &msg);
        });
    }

    /// The socket drops (the network, or a lmgw killed): no close frame.
    pub fn drop_socket(&self) {
        let n = self.last();
        self.with(|s| s.sockets[n].end(1006, ""));
    }

    /// The device's key is revoked: lmgw closes the session with 4003 and its reason, its kind's
    /// token first (§3.3, api §1.6: "device_disabled: device 'kai' was disabled").
    pub fn revoke_socket(&self, reason: &str) {
        let n = self.last();
        self.with(|s| s.sockets[n].end(lmgw_client::realtime::CLOSE_REVOKED, reason));
    }

    /// The bound thread left the device's reach (api §1.6: the owner attached lmgw's admin tools
    /// to it): lmgw says `error chat_thread_not_found`, then closes the session with 4004 and its
    /// neutral reason, as its `realtime/thread/hooks.rs` does. What `current` answers next is the
    /// test's to set.
    pub fn out_of_reach_socket(&self) {
        let n = self.last();
        self.with(|s| {
            let thread = s.sockets[n].thread;
            s.sockets[n].emit(ServerEvent::Error(ErrorFacts {
                kind: "invalid_request_error".into(),
                code: Some("chat_thread_not_found".into()),
                message: format!(
                    "chat thread {thread} is out of reach for this key, and this session closes"
                ),
            }));
            let reason = format!("chat thread {thread} is out of reach for this key");
            s.sockets[n].end(lmgw_client::realtime::CLOSE_OUT_OF_REACH, &reason)
        });
    }

    /// lmgw stops or restarts: it closes the session with 1001 and its reason (api §1.6).
    pub fn going_away(&self) {
        let n = self.last();
        self.with(|s| {
            s.sockets[n].end(
                lmgw_client::realtime::CLOSE_GOING_AWAY,
                lmgw_client::realtime::SHUTTING_DOWN,
            )
        });
    }

    pub fn model_state(&self, stage: &str, alias: &str, state: &str, answered_by: Option<&str>) {
        self.emit(ServerEvent::ModelState(ModelState {
            stage: stage.into(),
            alias: alias.into(),
            state: state.into(),
            answered_by: answered_by.map(Into::into),
            cause: (state == "held").then(|| "gpu_hold".into()),
            ..ModelState::default()
        }));
    }
}

impl VoiceLink for MockLink {
    fn set_credentials(&mut self, creds: Option<Credentials>) {
        self.with(|s| s.creds = creds.map(|c| c.url));
    }

    fn current(&mut self, folder: i64, new: bool) -> Pending<Result<Current, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            // An HTTP round trip.
            let lag = st.lock().unwrap().current_lag;
            tokio::time::sleep(lag).await;
            let mut s = st.lock().unwrap();
            s.current_calls.push(new);
            s.current_folders.push(folder);
            if !s.current_script.is_empty()
                && let Some(e) = s.current_script.remove(0)
            {
                return Err(e);
            }
            if let Some(e) = s.current_error.clone() {
                return Err(e);
            }
            // lmgw's order: unknown, not ongoing, no model.
            let Some(f) = s.folders.iter().find(|f| f.id == folder) else {
                return Err(refusal(404, "not_found", "folder not found"));
            };
            if f.idle_minutes.is_none() {
                return Err(refusal(
                    409,
                    "not_ongoing",
                    &format!(
                        "folder '{}' is not an ongoing conversation — mark it one in its settings",
                        f.name
                    ),
                ));
            }
            if f.model.is_none() {
                return Err(refusal(
                    409,
                    "folder_no_model",
                    &format!(
                        "folder '{}' names no model: set the folder's model in its defaults",
                        f.name
                    ),
                ));
            }
            if !new && !s.answers_first.is_empty() {
                return Ok(Current {
                    thread: s.answers_first.remove(0),
                    rolled_over: false,
                    last_message_at: None,
                });
            }
            if new {
                s.thread = ThreadRef {
                    id: s.thread.id + 1,
                    title: "New conversation".into(),
                };
                s.rolled_over = true;
            }
            let rolled_over = std::mem::take(&mut s.rolled_over);
            Ok(Current {
                thread: s.thread.clone(),
                rolled_over,
                last_message_at: if rolled_over { None } else { s.last_message_at },
            })
        })
    }

    fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut s = st.lock().unwrap();
            s.folder_lists += 1;
            match s.folders_error.clone() {
                Some(e) => Err(e),
                None => Ok(s.folders.clone()),
            }
        })
    }

    fn profiles(&mut self) -> Pending<Result<ProfileList, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            let mut s = st.lock().unwrap();
            s.profile_lists += 1;
            match s.profiles_error.clone() {
                Some(e) => Err(e),
                None => Ok(s.profiles.clone()),
            }
        })
    }

    fn set_folder_profile(
        &mut self,
        folder: i64,
        profile: Option<i64>,
    ) -> Pending<Result<FolderPatched, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            let delay = st.lock().unwrap().profile_set_delay;
            tokio::time::sleep(delay).await;
            let mut s = st.lock().unwrap();
            s.profile_sets.push((folder, profile));
            if let Some(e) = s.profile_set_error.clone() {
                return Err(e);
            }
            let Some(f) = s.folders.iter_mut().find(|f| f.id == folder) else {
                return Err(refusal(404, "not_found", "folder not found"));
            };
            f.profile_id = profile;
            let mut patched = FolderPatched::default();
            patched.folder.id = folder;
            patched.folder.defaults.profile_id = profile;
            patched.applied = Some(lmgw_client::requests::AppliedToCurrent {
                thread_id: s.thread.id,
                fields: vec!["profile_id".into()],
            });
            Ok(patched)
        })
    }

    fn approvals(
        &mut self,
        thread: i64,
        body: &lmgw_client::requests::approvals::ApprovalsRequest,
    ) -> Pending<Result<(), Refusal>> {
        let (st, body) = (self.st.clone(), body.clone());
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut s = st.lock().unwrap();
            s.approvals_sent.push((thread, body));
            match s.approvals_error.clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })
    }

    fn pending_approvals(
        &mut self,
        thread: i64,
    ) -> Pending<Result<Vec<lmgw_client::requests::approvals::ApprovalRequest>, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut s = st.lock().unwrap();
            s.pending_read.push(thread);
            Ok(s.pending.get(&thread).cloned().unwrap_or_default())
        })
    }

    fn transcribe(&mut self, thread: i64, wav: Vec<u8>) -> Pending<Result<String, Refusal>> {
        let st = self.st.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut s = st.lock().unwrap();
            s.transcribed.push((thread, wav.len()));
            if s.transcripts.is_empty() {
                Ok("Ja.".into())
            } else {
                s.transcripts.remove(0)
            }
        })
    }

    fn create_folder(&mut self, name: &str) -> Pending<Result<FolderInfo, Refusal>> {
        let st = self.st.clone();
        let name = name.to_owned();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut s = st.lock().unwrap();
            s.created.push(name.clone());
            let f = FolderInfo {
                id: s.folders.iter().map(|f| f.id).max().unwrap_or(0) + 1,
                name,
                ..FolderInfo::default()
            };
            s.folders.push(f.clone());
            Ok(f)
        })
    }

    fn open(
        &mut self,
        thread: i64,
        take_over: bool,
        events: UnboundedSender<SessionEvent>,
    ) -> Box<dyn VoiceSocket> {
        let (n, after, refused, refusal) = self.with(|s| {
            let open_beside = s.sockets.iter().filter(|k| k.tx.is_some()).count();
            let refused = (!take_over)
                .then(|| s.bound_elsewhere.get(&thread).cloned())
                .flatten();
            let refusal = (!s.refuse_opens.is_empty()).then(|| s.refuse_opens.remove(0));
            s.sockets.push(MockSocket {
                thread,
                tx: Some(events),
                take_over,
                open_beside,
                ..MockSocket::default()
            });
            (s.sockets.len() - 1, s.created_after, refused, refusal)
        });
        if let Some((status, code, message)) = refusal {
            self.with(|s| {
                if let Some(tx) = s.sockets[n].tx.take() {
                    let _ = tx.send(SessionEvent::Refused {
                        status,
                        code,
                        message,
                    });
                }
            });
            return Box::new(Socket {
                st: self.st.clone(),
                n,
            });
        }
        if let Some(by) = refused {
            // lmgw's answer to a bind that never takes over while another session holds the thread.
            self.with(|s| {
                if let Some(tx) = s.sockets[n].tx.take() {
                    let _ = tx.send(SessionEvent::Refused {
                        status: 409,
                        code: Some(super::link::CHAT_THREAD_BOUND.into()),
                        message: format!("voice is in use on {by}"),
                    });
                }
            });
            return Box::new(Socket {
                st: self.st.clone(),
                n,
            });
        }
        let st = self.st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            {
                let s = st.lock().unwrap();
                if let Some(tx) = &s.sockets[n].tx {
                    let _ = tx.send(SessionEvent::Opened);
                }
            }
            let Some(after) = after else { return };
            tokio::time::sleep(after).await;
            let s = st.lock().unwrap();
            let f = facts(&s, None);
            s.sockets[n].emit(ServerEvent::SessionCreated(f));
        });
        Box::new(Socket {
            st: self.st.clone(),
            n,
        })
    }
}

impl VoiceSocket for Socket {
    fn send_tagged(&mut self, event: ClientEvent, event_id: String) {
        let refuse = {
            let mut s = self.st.lock().unwrap();
            s.sockets[self.n].tagged.push(event_id.clone());
            (matches!(event, ClientEvent::ResponseCreate) && !s.refuse_creates.is_empty())
                .then(|| s.refuse_creates.remove(0))
        };
        let Some(code) = refuse else {
            return self.send(event);
        };
        let mut s = self.st.lock().unwrap();
        let sock = &mut s.sockets[self.n];
        sock.kinds.push("response.create");
        sock.sent.push(event);
        if let Some(tx) = &sock.tx {
            let _ = tx.send(SessionEvent::ErrorFor {
                event_id,
                error: ErrorFacts {
                    kind: "invalid_request_error".into(),
                    message: format!("{code} (the mock)"),
                    code: Some(code),
                },
            });
        }
    }

    fn send(&mut self, event: ClientEvent) {
        let respond = {
            let mut s = self.st.lock().unwrap();
            let reply = s.reply.clone();
            let keep = s.keep_audio;
            let f_update = |s: &MockState, d| facts(s, Some(d));
            let kind = match &event {
                ClientEvent::SessionUpdate { .. } => "session.update",
                ClientEvent::Append { .. } => "input_audio_buffer.append",
                ClientEvent::Commit => "input_audio_buffer.commit",
                ClientEvent::Clear => "input_audio_buffer.clear",
                ClientEvent::ResponseCreate => "response.create",
                ClientEvent::ResponseCancel { .. } => "response.cancel",
                ClientEvent::Truncate { .. } => "conversation.item.truncate",
                ClientEvent::ApprovalResponse { .. } => "conversation.item.create",
            };
            let facts_now = match &event {
                ClientEvent::SessionUpdate { turn_detection, .. } => {
                    Some(f_update(&s, *turn_detection))
                }
                _ => None,
            };
            let sock = &mut s.sockets[self.n];
            sock.kinds.push(kind);
            let mut respond = None;
            match &event {
                ClientEvent::Append { audio } => {
                    let pcm = lmgw_client::base64::pcm16_samples(
                        &lmgw_client::base64::decode(audio).expect("an append is base64"),
                    );
                    sock.appended += pcm.len();
                    sock.silent += pcm.iter().filter(|x| **x == 0).count();
                    if keep {
                        sock.audio.extend_from_slice(&pcm);
                    }
                }
                ClientEvent::SessionUpdate { .. } => {
                    sock.sent.push(event.clone());
                    sock.emit(ServerEvent::SessionUpdated(facts_now.expect("built above")));
                }
                ClientEvent::Clear => {
                    sock.sent.push(event.clone());
                    sock.emit(ServerEvent::Cleared);
                }
                ClientEvent::Commit => {
                    sock.sent.push(event.clone());
                    sock.emit(ServerEvent::Committed);
                    sock.emit(ServerEvent::Transcript("Wie spät ist es?".into()));
                }
                ClientEvent::ResponseCreate => {
                    sock.sent.push(event.clone());
                    respond = reply;
                }
                ClientEvent::ResponseCancel { response_id } => {
                    sock.sent.push(event.clone());
                    let r = sock.replies.iter_mut().find(|r| {
                        r.open && response_id.as_ref().is_none_or(|id| *id == r.response_id)
                    });
                    match r {
                        Some(r) => {
                            r.open = false;
                            r.cancelled = Some("client_cancelled".into());
                            let rid = r.response_id.clone();
                            sock.emit(ServerEvent::ResponseDone {
                                response_id: rid,
                                status: "cancelled".into(),
                                reason: Some("client_cancelled".into()),
                            });
                        }
                        None => sock.emit(ServerEvent::Error(ErrorFacts {
                            kind: "invalid_request_error".into(),
                            code: Some("response_cancel_not_active".into()),
                            message: "no active response".into(),
                        })),
                    }
                }
                ClientEvent::Truncate {
                    item_id,
                    audio_end_ms,
                    ..
                } => {
                    sock.sent.push(event.clone());
                    let found = sock.replies.iter().position(|r| r.item_id == *item_id);
                    match found {
                        Some(i) if *audio_end_ms <= sock.replies[i].sent_ms => {
                            let r = sock.replies[i].clone();
                            sock.truncates
                                .push((item_id.clone(), *audio_end_ms, r.sent_ms));
                            sock.emit(ServerEvent::Truncated {
                                item_id: item_id.clone(),
                                audio_end_ms: *audio_end_ms,
                            });
                            // An item still being produced stops its response.
                            if r.open && !r.audio_done {
                                let rr = &mut sock.replies[i];
                                rr.open = false;
                                rr.cancelled = Some("client_cancelled".into());
                                sock.emit(ServerEvent::ResponseDone {
                                    response_id: r.response_id.clone(),
                                    status: "cancelled".into(),
                                    reason: Some("client_cancelled".into()),
                                });
                            }
                            let heard = heard_text(&r.text, r.total_ms, *audio_end_ms);
                            let stored = (!heard.is_empty()).then_some(heard.clone());
                            sock.stored.retain(|(id, _)| *id != r.response_id);
                            sock.stored.push((r.response_id.clone(), stored.clone()));
                            sock.emit(ServerEvent::ChatReply(ChatReply {
                                message_id: 900,
                                content: stored,
                                unheard: Some(r.text[heard.len()..].trim().to_owned()),
                                removed: heard.is_empty(),
                                ..ChatReply::default()
                            }));
                        }
                        _ => {
                            sock.refused_truncates += 1;
                            sock.emit(ServerEvent::Error(ErrorFacts {
                                kind: "invalid_request_error".into(),
                                code: Some("invalid_value".into()),
                                message: format!("audio_end_ms {audio_end_ms} is beyond the audio"),
                            }));
                        }
                    }
                }
                ClientEvent::ApprovalResponse { .. } => sock.sent.push(event.clone()),
            }
            respond
        };
        if let Some((text, ms)) = respond {
            MockLink {
                st: self.st.clone(),
            }
            .respond_on(self.n, &text, ms);
        }
    }

    fn close(&mut self) {
        let mut s = self.st.lock().unwrap();
        let lag = s.close_lag;
        let sock = &mut s.sockets[self.n];
        sock.closed_by_client = true;
        sock.kinds.push("close");
        // The journal drains, then the close is answered.
        if lag.is_zero() {
            sock.end(1000, "");
        } else {
            let (st, n) = (self.st.clone(), self.n);
            tokio::spawn(async move {
                tokio::time::sleep(lag).await;
                st.lock().unwrap().sockets[n].end(1000, "");
            });
        }
    }
}

impl MockLink {
    /// [`MockLink::respond`] on socket `n` (an older socket's response, or the newest's).
    fn respond_on(&self, n: usize, text: &str, ms: u64) {
        let last = self.last();
        if n == last {
            self.respond(text, ms);
        }
    }

    /// The replies as stored on socket `n`.
    pub fn stored(&self, n: usize) -> HashMap<String, Option<String>> {
        self.socket(n, |s| s.stored.iter().cloned().collect())
    }
}
