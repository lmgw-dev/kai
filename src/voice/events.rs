//! What the socket and the server say (§5.2–§5.6), and the 20 ms tick that moves audio.
//!
//! **Audio in** (§5.3, §5.4, K16). Each tick drains the capture. While audio is wanted (listening
//! on in automatic mode, or the talk key talks) it goes out in 20 ms `input_audio_buffer.append`
//! chunks once `session.created` came, and into the buffer before that (and across a rebind).
//! Muted, and in automatic mode while another principal's turn runs in the thread (K16), the
//! chunks are silence, so an open turn ends naturally and the stream's clock keeps running. In
//! push-to-talk with the key up only the pre-roll is kept.
//!
//! **Ends.** A takeover (close 4000, `chat_thread_taken_over`) turns listening off and names who
//! took the thread; Kai does not reconnect by itself, the next press takes it back (§5.6). What a
//! close means is read by `lmgw_client::realtime::close_kind` (api §1.6), never from its words: a
//! 4003 is a revocation whose token says which (§3.3); a 4004 is the bound thread leaving this
//! device's reach, after which Kai asks for the folder's current thread and binds it again where
//! listening was on. A 1001 close is lmgw stopping or restarting (api §1.6): listening off, and
//! the tooltip and the notification say why with lmgw's reason. Any other end is a drop:
//! listening off, with a notification (K5).

use tokio::time::Instant;

use lmgw_client::realtime::ModelState;

use super::link::{
    CHAT_THREAD_BOUND, CLOSE_DROPPED, CLOSE_REVOKED, ClientEvent, CloseKind, ErrorFacts,
    RevokeKind, SHUTTING_DOWN, ServerEvent, SessionEvent, ThreadRef, close_kind,
};
use super::session::Cut;
use super::timing::{self, stage_name};
use super::{CHUNK, DEFAULT_PREROLL_MS, Intent, Voice, VoiceLink, Wake, Why};
use crate::audio::{AudioPort, RATE};
use crate::lmgw_link::KeyLoss;

/// The code of an `error` that says another client took the thread (chat-voice §8.1).
const TAKEN_OVER: &str = "chat_thread_taken_over";

/// The code of an `error` that says the bound thread is gone for this key, and of a bind's 404
/// for it (chat-voice §8.1; api L3): a 4004 follows the error, or on an older gateway the feed's
/// `thread.deleted` (review KAI-4 K4-4, K4-8).
const THREAD_NOT_FOUND: &str = "chat_thread_not_found";

/// The realtime events `lmgw_client::realtime::parse` reads by their type: one of these that comes
/// as `ServerEvent::Unknown` did not read (review KAI-3 K3-8).
const TYPED_KINDS: &[&str] = &[
    "session.created",
    "session.updated",
    "input_audio_buffer.speech_started",
    "input_audio_buffer.speech_stopped",
    "input_audio_buffer.committed",
    "input_audio_buffer.cleared",
    "conversation.item.input_audio_transcription.delta",
    "conversation.item.input_audio_transcription.completed",
    "conversation.item.input_audio_transcription.failed",
    "response.created",
    "response.output_audio.delta",
    "response.output_audio.done",
    "response.output_audio_transcript.delta",
    "response.done",
    "error",
    "lmgw.chat.frame",
    "lmgw.chat.user",
    "lmgw.chat.input",
    "lmgw.chat.reply",
    "conversation.item.truncated",
    "lmgw.model.state",
    "lmgw.response.timing",
    "lmgw.chat.thread",
    "lmgw.approval.decided",
    "lmgw.task.done",
];

impl<L: VoiceLink> Voice<L> {
    pub async fn handle(&mut self, wake: Wake, audio: &impl AudioPort) {
        let now = Instant::now();
        match wake {
            Wake::Tick => {
                self.drain_capture();
                self.stuck_key(now).await;
                if let Some(s) = self.session.as_mut()
                    && let Some(w) = s.pump(now)
                {
                    tracing::warn!("voice: {w}");
                }
                self.lifecycle(now).await;
            }
            Wake::Current(r) => self.on_current(r, now).await,
            Wake::Idle => self.on_idle_wake(now),
            Wake::Session(SessionEvent::Opened) => tracing::info!("voice: the session is bound"),
            Wake::Session(SessionEvent::Refused {
                status,
                code,
                message,
            }) => self.on_refused(status, code, message),
            Wake::Session(SessionEvent::Closed { code, reason }) => self.on_closed(code, reason),
            Wake::Session(SessionEvent::Note(note)) => self.on_note(note),
            Wake::Session(SessionEvent::Server(e)) => self.on_server(e, audio, now).await,
            Wake::Session(SessionEvent::ErrorFor { event_id, error }) => {
                // WP18: a refusal of Kai's own continuation is the jobs' (`super::jobs`).
                if !self.jobs_refused(&event_id, &error) {
                    self.on_error(error, Some(&event_id))
                }
            }
            Wake::SessionGone => self.on_closed(CLOSE_DROPPED, String::new()),
            Wake::Approval(w) => self.on_approval(w).await,
        }
        self.settle();
    }

    /// `shortcuts.max_hold_s`: a hold that no release ended in that long (a release that never
    /// came, or one that came before its press) ends, and its audio is discarded: no release came,
    /// so it is by construction no deliberate turn (review KAI-2 K2-3). Push-to-talk's buffer is
    /// cleared on the server and in Kai, and nothing is committed or asked; in automatic listening
    /// the hold only ends. Logged, notified, and said in the tooltip until the next press. A guard
    /// against a stuck key, not a limit on how long the owner speaks.
    async fn stuck_key(&mut self, now: Instant) {
        let Some(k) = self.key else { return };
        if now.saturating_duration_since(k.at) < self.s.max_hold {
            return;
        }
        let secs = self.s.max_hold.as_secs();
        let manual = !k.consumed && !k.muted && self.push_to_talk();
        self.key = None;
        // A spoken answer to an approval no release ended is not sent either (K17).
        self.approvals.cancel_recording();
        self.commit_on_create = false;
        if manual {
            self.pending.clear();
            self.carry.clear();
            if let Some(s) = self.session.as_mut() {
                s.commit_pending = false;
                if s.created {
                    s.send(ClientEvent::Clear);
                }
                s.ptt_release(false);
            }
        } else if let Some(s) = self.session.as_mut() {
            s.ptt_release(false);
        }
        let said = if manual {
            format!("talk key released by the guard after {secs} s; nothing was sent")
        } else {
            format!("talk key released by the guard after {secs} s")
        };
        tracing::warn!(
            max_hold_s = secs,
            discarded = manual,
            "talk: no release came within shortcuts.max_hold_s: {said}"
        );
        self.notify(
            "Kai: the talk key was stuck",
            format!("{said} (shortcuts.max_hold_s)"),
        );
        // It stays in the tooltip until the next press.
        self.stuck = Some(format!("{said} (shortcuts.max_hold_s)"));
    }

    /// The socket's note (its outgoing queue overflowed): the tooltip's, notified once a session.
    fn on_note(&mut self, note: String) {
        tracing::warn!("voice: {note}");
        let first = self
            .session
            .as_mut()
            .is_some_and(|s| !std::mem::replace(&mut s.noted, true));
        if first {
            self.notify("Kai: voice audio dropped", note.clone());
        }
        self.note = Some(note);
    }

    // ------------------------------------------------------------- audio in

    /// Moves what the microphone captured since the last call where it belongs (module doc).
    pub(super) fn drain_capture(&mut self) {
        let Some(c) = self.capture.as_mut() else {
            return;
        };
        let mut buf = Vec::new();
        c.read(&mut buf);
        if let Some(o) = c.overflow() {
            tracing::warn!("voice: {o}");
            self.note = Some(o);
        }
        if buf.is_empty() {
            return;
        }
        // K17: the talk key records the spoken answer; while a call of Kai's own waits the session
        // gets no audio, and the pre-roll is kept for the answer.
        if self.approvals.recording() {
            self.approvals.record(&buf);
            return;
        }
        // A hold that began while such a call waited stays silent to its release (`Key::gated`).
        if self.approvals.gates_audio() || self.gated_hold() {
            self.keep_preroll(&buf);
            return;
        }
        let talking = self.talking();
        let automatic = self.continuous();
        if !(automatic || talking) {
            self.keep_preroll(&buf);
            return;
        }
        if self.muted || (automatic && !talking && self.other_turn().is_some()) {
            buf.fill(0);
        }
        match self.session.as_mut().filter(|s| s.created) {
            Some(s) => {
                self.carry.extend_from_slice(&buf);
                while self.carry.len() >= CHUNK {
                    let chunk: Vec<i16> = self.carry.drain(..CHUNK).collect();
                    s.send(ClientEvent::append_pcm16(&chunk));
                }
            }
            None => self.pending.extend_from_slice(&buf),
        }
    }

    fn keep_preroll(&mut self, samples: &[i16]) {
        let ms = self.preroll_ms.unwrap_or(DEFAULT_PREROLL_MS);
        let keep = ms as usize * RATE as usize / 1000;
        self.preroll.extend(samples);
        while self.preroll.len() > keep {
            self.preroll.pop_front();
        }
    }

    // ------------------------------------------------------------- the socket's ends

    fn on_refused(&mut self, status: u16, code: Option<String>, message: String) {
        tracing::warn!(status, ?code, %message, "voice: the bind was refused");
        let session = self.session.take();
        crate::delegate::session_ended();
        crate::widgets::thread_apps::session_ended();
        if session.is_some() {
            self.conversation_dropped("the bind was refused");
        }
        let by_owner = session.as_ref().is_none_or(|s| s.by_owner);
        let intent = Intent {
            bind: self.listening || by_owner,
            by_owner,
        };
        // What the refusal means is its code. Where its body did not come with the answer's head
        // (`code` is none, review KAI-4 K4-5), it is the bind's documented status (chat-voice
        // §8.1's table): 404 the thread gone for this key, 409 another session holds it, 401 the
        // key refused, whose code an ordinary request reads whole.
        match (status, code.as_deref()) {
            // The thread left this device's reach between `current` and the bind: a 4004's twin
            // (review KAI-4 K4-8). Listening, the press's buffered audio and its key stay, and the
            // folder's current thread is bound in its place.
            (404, None | Some(THREAD_NOT_FOUND)) if session.is_some() => {
                return self.thread_left_reach(&message, intent, false);
            }
            (401, None) if !self.bare_401_asked => {
                tracing::info!(
                    "voice: the bind's 401 came without its code; asking current, whose 401 is \
                     read whole"
                );
                self.bare_401_asked = true;
                return self.ask_current_again(Why::KeyRefused, intent);
            }
            _ => {}
        }
        self.listening = false;
        self.key = None;
        self.pending.clear();
        self.connect_since = None;
        self.commit_on_create = false;
        // A rotated, deleted, disabled or expired key (§3.3), by the 401's code: the core says
        // "pair again", or for a disabled device "enable it" (review KAI-3 K3-6) and for an
        // expired key "move its expiry". A 401 whose code did not come even from `current` is
        // "pair again", as any 401 without a known code.
        if status == 401 {
            let loss = KeyLoss::of_code(code.as_deref().unwrap_or_default());
            self.revoked = Some((message, loss));
            return;
        }
        // Kai's own bind, and another session held the thread (K5, review KAI-2 K2-13): off until
        // a press, which takes it over. A 409 whose body did not come says no one.
        let bound_elsewhere =
            code.as_deref() == Some(CHAT_THREAD_BOUND) || (status == 409 && code.is_none());
        if !by_owner && bound_elsewhere {
            let message = if code.is_none() {
                "voice is in use on another client".to_owned()
            } else {
                message
            };
            tracing::info!(%message, "voice: the thread is bound elsewhere; staying off (K5)");
            let words = format!("{message}; Kai stays off until you press the talk key");
            self.note = Some(words.clone());
            self.notify("Kai: voice in use elsewhere", words);
            return;
        }
        let words = match code {
            Some(c) => format!("{message} ({c})"),
            None => message,
        };
        self.note = Some(format!("voice could not start: {words}"));
        self.notify("Kai: voice could not start", words);
    }

    /// A session closed: whatever the close means (`closed`), conversation mode ends with it, the
    /// same way after every kind of close (K27: a drop ends it).
    fn on_closed(&mut self, code: u16, reason: String) {
        let kind = close_kind(code, &reason);
        let had_mode = self.session.is_some() && self.conversation.is_some();
        self.closed(code, reason);
        if had_mode && self.conversation_dropped("the session closed") {
            let said = "conversation mode ended with the session";
            self.note = Some(match self.note.take() {
                Some(n) => format!("{n}; {said}"),
                None => said.to_owned(),
            });
            // The closes that notify nothing of their own say it in a notification too.
            if matches!(kind, CloseKind::OutOfReach | CloseKind::Revoked { .. }) {
                let body = format!("{said}; {}", self.configured_listening());
                self.notify("Kai: conversation mode ended", body);
            }
        }
    }

    fn closed(&mut self, code: u16, reason: String) {
        let Some(mut s) = self.session.take() else {
            return;
        };
        crate::delegate::session_ended();
        crate::widgets::thread_apps::session_ended();
        // Whatever plays stops with the session.
        s.player = None;
        let created = s.created;
        let taken_message = s
            .last_error
            .as_ref()
            .filter(|e| e.code.as_deref() == Some(TAKEN_OVER))
            .map(|e| e.message.clone());
        let was_listening = self.listening;
        // A hold that talked when the session ended: its turn went with the session.
        let held = self.talking();
        // Kai's own follow, or the owner's new conversation, on its way: the owner's ask is kept.
        let asked_by_owner = self.current_req.as_ref().is_some_and(|r| r.by_owner);
        self.listening = false;
        self.key = None;
        self.pending.clear();
        self.carry.clear();
        self.connect_since = None;
        self.commit_on_create = false;
        self.current_req = None;
        self.rebind = None;
        self.bind_wait = None;
        // What the close means is lmgw's code and, for a 4003, the token its reason starts with
        // (api §1.6: `lmgw_client::realtime::close_kind`), never its words.
        let kind = close_kind(code, &reason);
        if kind == CloseKind::TakenOver || taken_message.is_some() {
            let who = taken_message
                .or_else(|| (!reason.is_empty()).then(|| reason.clone()))
                .unwrap_or_else(|| "voice mode moved to another client".to_owned());
            tracing::info!(code, %who, "voice: taken over; not reconnecting (§5.6)");
            self.note = Some(format!("{who}; press the talk key to take it back"));
            self.notify(
                "Kai: voice taken over",
                format!("{who}; press the talk key to take it back"),
            );
            return;
        }
        match kind {
            CloseKind::Revoked { kind } => {
                self.revoked_close(kind, &reason, was_listening, asked_by_owner)
            }
            CloseKind::OutOfReach => {
                self.listening = was_listening;
                let intent = Intent {
                    bind: was_listening,
                    by_owner: asked_by_owner,
                };
                tracing::info!(
                    %reason,
                    listening = was_listening,
                    held,
                    "voice: the thread left this device's reach (close 4004); asking for the \
                     folder's current thread"
                );
                self.thread_left_reach(&reason, intent, held)
            }
            CloseKind::ShuttingDown => {
                // lmgw's stop or restart (api §1.6): it closes every session with 1001 and says so
                // ("lmgw is stopping or restarting"); it is not a lost connection. Off, as after
                // any end that is not Kai's own (K5): the next press binds again once lmgw is
                // back.
                let what = if reason.trim().is_empty() {
                    SHUTTING_DOWN.to_owned()
                } else {
                    reason.clone()
                };
                tracing::info!(code, %reason, created, "voice: lmgw is stopping or restarting; listening is off (K5)");
                if created {
                    self.note = Some(format!("voice stopped: {what}"));
                    self.notify(
                        "Kai: lmgw is stopping",
                        format!(
                            "voice stopped: {what}; press the talk key to start again once it is back"
                        ),
                    );
                } else {
                    self.note = Some(format!("voice could not start: {what}"));
                    self.notify(
                        "Kai: lmgw is stopping",
                        format!("voice could not start: {what}"),
                    );
                }
            }
            _ => {
                let why = if reason.is_empty() {
                    format!("close {code}")
                } else {
                    format!("{reason}, close {code}")
                };
                tracing::warn!(code, %reason, created, "voice: the session dropped; listening is off (K5)");
                if created {
                    self.note = Some(format!("voice stopped: lmgw went away ({why})"));
                    self.notify(
                        "Kai: voice stopped",
                        format!(
                            "voice stopped: lmgw went away ({why}); press the talk key to start again"
                        ),
                    );
                } else {
                    self.note = Some(format!("voice could not start ({why})"));
                    self.notify("Kai: voice could not start", why);
                }
            }
        }
    }

    /// A 4003 close (§3.3): the key was revoked, and the token its reason starts with says how
    /// (api §1.6). A kind Kai knows goes to the core with lmgw's sentence, as the feed's `revoked`
    /// does: a disabled device and an expired key wait for the owner with the key kept, a rotated
    /// or deleted key and another kind of key's revocation are paired again. A kind this build
    /// does not know, or none (an older gateway's), goes to the core with lmgw's sentence, which
    /// it says once per reason, as lmgw's guidance asks; Kai asks for the conversation's thread
    /// once, and the answer decides (a 401 is that refusal's, any other answer means the key still
    /// opens). Listening binds the answer once: another such close in a row turns listening off,
    /// as any end that is not Kai's own does (K5), so a gateway that ends every session so is not
    /// met again and again (review KAI-4 K4-7); the owner's press starts again.
    fn revoked_close(
        &mut self,
        kind: RevokeKind,
        reason: &str,
        listening: bool,
        asked_by_owner: bool,
    ) {
        let (_, sentence) = RevokeKind::split_close_reason(reason);
        let words = match (sentence.trim(), reason.trim()) {
            ("", "") => format!("close {CLOSE_REVOKED}"),
            ("", whole) => whole.to_owned(),
            (sentence, _) => sentence.to_owned(),
        };
        if let Some(loss) = KeyLoss::of_kind(&kind) {
            tracing::warn!(%kind, %reason, ?loss, "voice: the key was revoked (close 4003)");
            self.revoked = Some((words, loss));
            return;
        }
        let again = std::mem::take(&mut self.unknown_rebound);
        tracing::warn!(
            kind = %kind,
            %reason,
            again,
            "voice: a revocation of a kind this build does not know (close 4003); Kai asks lmgw \
             once whether the key still opens"
        );
        self.revoked_unknown = Some(words.clone());
        let bind = listening && !again;
        if listening && again {
            let said = format!(
                "lmgw ended the voice session again ({words}); press the talk key to start again"
            );
            self.note = Some(format!("voice stopped: {said}"));
            self.notify("Kai: voice stopped", said);
        }
        self.unknown_rebound = bind;
        self.listening = bind;
        self.ask_current_again(
            Why::Revoked,
            Intent {
                bind,
                by_owner: asked_by_owner,
            },
        );
    }

    /// The bound thread left this device's reach: a 4004 close (api §1.6: the owner attached
    /// lmgw's admin tools to it, or what the device may see narrowed), or a bind's 404 for it
    /// (review KAI-4 K4-8). The key is still good, so this is no revocation: the thread is
    /// forgotten and Kai asks for the folder's current thread, which `intent` binds (Kai's own
    /// bind unless the owner asked: K5, never a takeover). A hold that talked into the session
    /// (`held`) went with it, and the note says so. A refusal of `current` says itself as any does,
    /// a 401 as the key's (`thread::current_refused`).
    fn thread_left_reach(&mut self, reason: &str, intent: Intent, held: bool) {
        self.thread = None;
        self.last_turn = None;
        let mut note = format!(
            "the last thread left this device's reach (lmgw: {reason}); Kai follows the folder's \
             current thread"
        );
        if held {
            note.push_str("; the held turn went with it: press the talk key again");
        }
        self.note = Some(note);
        self.ask_current_again(Why::OutOfReach, intent);
    }

    // ------------------------------------------------------------- server events

    async fn on_server(&mut self, e: ServerEvent, audio: &impl AudioPort, now: Instant) {
        let talking = self.talking();
        let gated_hold = self.gated_hold();
        let push_to_talk = self.push_to_talk();
        let flush_wait = self.s.flush_wait;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        match e {
            ServerEvent::SessionCreated(f) => {
                s.created = true;
                self.bare_401_asked = false;
                if let Some(t) = f.thread {
                    let t = ThreadRef::from(t);
                    s.thread = t.clone();
                    self.thread = Some(t);
                }
                s.resolved = f.turn_detection.clone();
                if let Some(ms) = f.prefix_padding_ms {
                    self.preroll_ms = Some(ms);
                }
                let buffered = self.pending.len();
                tracing::info!(
                    thread = s.thread.id,
                    resolved = ?f.turn_detection,
                    buffered_ms = buffered as u64 * 1000 / RATE as u64,
                    chat = ?f.chat, asr = ?f.asr, tts = ?f.tts,
                    "voice: session.created"
                );
                self.connect_since = None;
                self.errors.clear();
                // §5.3: session.update, the buffer in order, then live chunks.
                self.update_session();
                let s = self.session.as_mut().expect("checked above");
                let mut pending = std::mem::take(&mut self.pending);
                let tail = pending.len() - pending.len() % CHUNK;
                for c in pending[..tail].chunks(CHUNK) {
                    s.send(ClientEvent::append_pcm16(c));
                }
                self.carry = pending.split_off(tail);
                if talking && push_to_talk && !gated_hold {
                    s.ptt_press();
                }
                if s.commit_pending {
                    s.commit_pending = false;
                    if !self.carry.is_empty() {
                        s.send(ClientEvent::append_pcm16(&std::mem::take(&mut self.carry)));
                    }
                    s.send(ClientEvent::Commit);
                    s.send(ClientEvent::ResponseCreate);
                    s.machine.ptt_up(true);
                }
            }
            ServerEvent::SessionUpdated(f) => {
                // Only the answer to the last update is the session's word (`Updates`): an
                // older echo would undo a newer choice.
                let settled = s.updates.echo();
                if settled && let Some(ms) = f.prefix_padding_ms {
                    self.preroll_ms = Some(ms);
                }
                tracing::debug!(turn_detection = ?f.turn_detection, settled, "voice: session.updated");
            }
            ServerEvent::SpeechStarted => {
                if let Some(rid) = s.machine.playing().map(str::to_owned) {
                    tracing::info!(response_id = %rid, "voice: barge-in");
                    s.cut(&rid, Cut::BargeIn, flush_wait, now).await;
                }
                s.machine.speech_started();
            }
            ServerEvent::SpeechStopped => s.machine.speech_stopped(),
            ServerEvent::Committed => s.machine.committed(),
            ServerEvent::Cleared => {
                // Push-to-talk's own clear (the press) is followed by its turn.
                if !talking {
                    s.machine.cleared();
                }
            }
            ServerEvent::TranscriptDelta(_) | ServerEvent::SpokenDelta { .. } => {}
            // A tool turn (chat-voice §8.5): `thinking` while its tool runs after the preamble.
            ServerEvent::ChatFrame {
                event,
                data,
                response_id,
            } => {
                // MCP Apps views of the turn's calls (`crate::widgets::thread_apps`).
                crate::widgets::thread_apps::frame(&response_id, &event, &data);
                // The relayed `approval` frame carries the call's id, which the turn's
                // `mcp_approval_request` item (OpenAI's shape) does not: the card learns it.
                if event == "tool"
                    && data["event"] == "approval"
                    && let (Some(id), Some(call)) = (
                        data["approval_request_id"].as_str(),
                        data["call_id"].as_str(),
                    )
                {
                    self.approvals.call_id_seen(id, call);
                }
                match event.as_str() {
                    "tool" => match data["event"].as_str().unwrap_or_default() {
                        "start" | "ready" => s.machine.set_tool(true),
                        "result" => s.machine.set_tool(false),
                        _ => {}
                    },
                    "done" => s.machine.set_tool(false),
                    _ => {}
                }
            }
            ServerEvent::ChatUser {
                message_id,
                content,
                response_id,
                ..
            } => {
                tracing::debug!(
                    message_id,
                    chars = content.chars().count(),
                    "voice: the turn's user message"
                );
                // K25: a delegate Kai's own session calls gets these words verbatim.
                crate::delegate::heard(&content, response_id.as_deref());
            }
            ServerEvent::ChatInput { input, why, .. } => {
                tracing::debug!(%input, ?why, "voice: how the turn reaches the chat model")
            }
            ServerEvent::Transcript(t) => {
                tracing::debug!(chars = t.chars().count(), "voice: the turn's transcript");
            }
            ServerEvent::TranscriptFailed(err) => {
                s.machine.turn_failed();
                self.on_error(err, None);
            }
            ServerEvent::ResponseCreated { response_id } => {
                // WP19: the first response after verdicts this session carried is theirs.
                self.approvals
                    .session_response_created(self.session_no, &response_id);
                // K25: the words kept so far are a past turn's (the user message of a heard turn
                // races its reply).
                crate::delegate::response_created(&response_id);
                s.last_error = None;
                s.response_created(&response_id, now, || audio.open_playback());
                self.note = None;
                self.jobs_response_created(&response_id);
            }
            ServerEvent::AudioDelta {
                response_id,
                item_id,
                pcm,
            } => {
                let pcm = lmgw_client::base64::pcm16_samples(&pcm);
                s.audio(&response_id, &item_id, &pcm, now, || audio.open_playback())
            }
            ServerEvent::AudioDone { response_id } => {
                s.replies.audio_done(&response_id);
                s.end_audio(&response_id);
            }
            ServerEvent::ResponseDone {
                response_id,
                status,
                reason,
            } => {
                tracing::info!(%response_id, %status, ?reason, "voice: response.done");
                let tid = s.thread.id;
                s.response_done(&response_id);
                // WP19: the response that carried verdicts ended: lmgw took them if it completed.
                let acts =
                    self.approvals
                        .session_response_done(self.session_no, &response_id, &status);
                if status == "failed" {
                    s.machine.turn_failed();
                    if s.last_error.is_none()
                        && let Some(r) = reason
                    {
                        self.note = Some(format!("the reply failed: {r}"));
                    }
                }
                // A turn of Kai's own in the conversation's thread (the feed says it too).
                self.turn_seen(tid);
                self.approval_acts(acts);
                self.jobs_response_done(&response_id, &status);
            }
            ServerEvent::Truncated {
                item_id,
                audio_end_ms,
            } => tracing::debug!(%item_id, audio_end_ms, "voice: truncated"),
            ServerEvent::Error(err) => self.on_error(err, None),
            ServerEvent::ChatReply(r) => tracing::info!(
                message_id = r.message_id,
                removed = r.removed,
                heard_chars = r.content.map(|c| c.chars().count()),
                unheard_chars = r.unheard.map(|c| c.chars().count()),
                skipped = r.skipped,
                "voice: the reply as stored"
            ),
            ServerEvent::ModelState(m) => self.on_model_state(m),
            ServerEvent::Timing(v) => self.on_timing(*v),
            ServerEvent::Thread(t) => {
                let t = ThreadRef::from(t);
                s.thread = t.clone();
                self.thread = Some(t);
            }
            // WP19: a call of the session's turn waits; the card shows it, and the verdict goes
            // back through this session while it is the bound one.
            ServerEvent::ApprovalRequest(req) => {
                let thread = s.thread.id;
                self.approvals.requested(thread, req, Some(self.session_no));
            }
            // A call of the session's turn was decided (by another client too): the card lets
            // it go, and its view, if one is shown, is told (`crate::widgets::thread_apps`).
            ServerEvent::ApprovalDecided(d) => {
                // The card knows the call's id (before it lets the call go).
                let call_id = self
                    .approvals
                    .call_id_of(&d.approval_request_id)
                    .map(str::to_owned);
                self.approvals
                    .decided(&d.approval_request_id, d.approve, d.by.as_deref());
                crate::widgets::thread_apps::approval_decided(
                    &d.approval_request_id,
                    call_id.as_deref(),
                    d.approve,
                    d.by.as_deref(),
                );
            }
            // WP18: a job's result entered the session's thread.
            ServerEvent::TaskDone(d) => self.task_done(d, true),
            // Its `type` only: an event a newer gateway sends may carry audio or the owner's
            // words (review KAI-2 K2-1). One `lmgw-client` reads by its type but whose fields did
            // not read is warned about with its size (review KAI-3 K3-8); every other kind (the
            // OpenAI-shaped events Kai skips, a newer gateway's) stays at trace.
            ServerEvent::Unknown { kind, data } => {
                if TYPED_KINDS.contains(&kind.as_str()) {
                    tracing::warn!(
                        %kind,
                        bytes = data.to_string().len(),
                        "voice: an event whose fields do not read; Kai skips it"
                    )
                } else {
                    tracing::trace!(%kind, "voice: an event Kai does not act on")
                }
            }
            _ => tracing::trace!("voice: an event Kai does not act on"),
        }
    }

    /// `error` (§5.6): shown verbatim and logged; repeats are counted in the log. `event_id`: the
    /// client event it echoes (`SessionEvent::ErrorFor`).
    fn on_error(&mut self, e: ErrorFacts, event_id: Option<&str>) {
        let n = {
            let n = self
                .errors
                .entry((e.code.clone(), e.message.clone()))
                .or_insert(0);
            *n += 1;
            *n
        };
        tracing::warn!(kind = %e.kind, code = ?e.code, message = %e.message, event_id, times = n, "voice: error");
        if let Some(s) = self.session.as_mut() {
            s.last_error = Some(e.clone());
        }
        // WP19: lmgw refused the verdicts this session carried: the `response.create` that
        // carries them, by its echoed id, whatever the code (M1), or an approval code
        // (`approval_decided`, …).
        let approval_code = e
            .code
            .as_deref()
            .is_some_and(|c| c.starts_with("approval_"));
        if event_id.is_some() || approval_code {
            let refusal = super::link::Refusal {
                status: 0,
                code: e.code.clone(),
                message: e.message.clone(),
            };
            let create = if approval_code { None } else { event_id };
            let acts = self
                .approvals
                .session_refused(self.session_no, create, refusal);
            self.approval_acts(acts);
        }
        let words = match &e.code {
            Some(c) => format!("{c}: {}", e.message),
            None => e.message.clone(),
        };
        match e.code.as_deref() {
            // The close follows and says it.
            Some(TAKEN_OVER) => {}
            // The thread is gone for this key: the 4004 follows (on an older gateway, for a
            // deletion, the feed's `thread.deleted`), and Kai acts on it and says it there. Logged,
            // counted and in the tooltip, never a notification (review KAI-4 K4-4).
            Some(THREAD_NOT_FOUND) => self.note = Some(format!("voice error: {words}")),
            Some("gpu_hold") => {
                self.hold_notice("", "", &e.message);
            }
            // The verdicts the session carried: the approvals say it on the card (above).
            Some(c) if c.starts_with("approval_") => {
                self.note = Some(format!("voice error: {words}"))
            }
            _ if event_id.is_some() => self.note = Some(format!("voice error: {words}")),
            _ => {
                self.note = Some(format!("voice error: {words}"));
                if n == 1 {
                    self.notify("Kai: voice error", words);
                }
            }
        }
    }

    /// `lmgw.model.state` (chat-voice §4.3): loading for the ring, the hold for the badge.
    fn on_model_state(&mut self, m: ModelState) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        tracing::info!(stage = %m.stage, alias = %m.alias, state = %m.state, "voice: model state");
        match m.state.as_str() {
            "loading" => {
                s.loading.insert(m.stage.clone(), m.alias.clone());
            }
            "held" => {
                s.loading.remove(&m.stage);
                s.held.insert(m.stage.clone(), m.alias.clone());
                let why = m
                    .message
                    .clone()
                    .unwrap_or_else(|| m.cause.clone().unwrap_or_else(|| "gpu_hold".to_owned()));
                self.hold_notice(&m.stage, &m.alias, &why);
            }
            other => {
                s.loading.remove(&m.stage);
                s.held.remove(&m.stage);
                if other == "failed" {
                    let msg = m.message.unwrap_or_default();
                    self.note = Some(format!(
                        "the {} model '{}' failed: {msg}",
                        stage_name(&m.stage),
                        m.alias
                    ));
                }
            }
        }
    }

    /// §5.6: one notification per hold period names the stage whose alias has no fallback.
    fn hold_notice(&mut self, stage: &str, alias: &str, why: &str) {
        if !self
            .notified
            .insert((self.period, "held".to_owned(), stage.to_owned()))
        {
            return;
        }
        let body = if stage.is_empty() {
            format!("local voice is paused: {why}")
        } else {
            format!(
                "the {} model '{alias}' has no fallback, so local voice is paused ({why})",
                stage_name(stage)
            )
        };
        self.notify("Kai: GPU hold", body);
    }

    /// `lmgw.response.timing`: the tooltip's line, the ring's `setTiming`, and K21: the first
    /// time in a hold period that a stage is answered by a fallback (another fallback alias for
    /// the same stage in that period is in the timing line, not notified again).
    fn on_timing(&mut self, v: lmgw_client::realtime::VoiceTiming) {
        self.timing_line = timing::line(&v);
        for f in timing::fallbacks(&v) {
            if self
                .notified
                .insert((self.period, "fallback".to_owned(), f.stage.clone()))
            {
                self.notify(
                    "Kai: a fallback answered",
                    format!(
                        "the {} alias '{}' was answered by '{}' (its configured fallback)",
                        stage_name(&f.stage),
                        f.alias,
                        f.answered_by
                    ),
                );
            }
        }
        self.timing_new = Some(v);
    }
}
