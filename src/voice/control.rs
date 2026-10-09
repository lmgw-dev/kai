//! The owner's side of the voice (§5.2, §5.4, §8.1, K5, K6): the talk key, listening on and off,
//! the mode, mute and a new conversation; and opening, rebinding and closing sessions.
//!
//! **The talk key (K6).** A press starts a hold, or, while Kai speaks, stops it and spends the
//! gesture. Released under `shortcuts.hold_ms` it toggles listening; held longer it commits the
//! push-to-talk turn. Repeated presses while held are ignored. A hold with listening off opens a
//! session at the press (capture buffered until `session.created`, §5.3) that closes
//! `voice.grace_s` after its reply has played (K5). In automatic mode with listening on a hold
//! needs no commit (the server's detector ends the turn); it only overrides K16's silence.
//!
//! **Listening on** is one session bound to the conversation's current thread. Kai never binds a
//! thread another client's voice session holds by itself (a rebind after a rollover,
//! `voice.on_at_start`): it stays off, says who has it and waits for a press (K5). The feed's view
//! decides first, and the bind itself asks lmgw not to take over (`&takeover=never`), so a client
//! that bound the same thread a moment earlier keeps it (409 `chat_thread_bound`, review KAI-2
//! K2-13). A press or the tray is the owner's own choice and binds anyway, taking the thread over as
//! the next press after a takeover takes it back (§5.6).
//!
//! **One bind in flight** (review KAI-2 K2-7). A session holds a slot of the key's
//! `concurrency_limit` until lmgw answered its close, so a bind waits for the last close
//! (`bind_wait`); a press, listening on or a new conversation meanwhile joins that wait instead of
//! starting a bind of its own, and the bind takes the conversation's thread as known when the close
//! is answered. A bind never replaces a session: one still there is closed first.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::conversation_mode::Conversation;
use super::link::{ClientEvent, SessionEvent, TurnDetection};
use super::session::{Cut, Session};
use super::{CurrentReq, Key, Then, Voice, VoiceLink, VoiceMode};
use crate::audio::AudioPort;

impl<L: VoiceLink> Voice<L> {
    // ------------------------------------------------------------- the owner's commands

    /// The talk key went down (§8.1).
    pub async fn talk_press(&mut self, audio: &impl AudioPort) {
        let now = Instant::now();
        if self.locked {
            tracing::info!("talk: the screen is locked; the press does nothing");
            return;
        }
        if self.key.is_some() {
            tracing::debug!("talk: a repeated press while held is ignored (K6)");
            return;
        }
        self.stuck = None;
        // K17: while the card shows a call of Kai's own and said it armed for it, the press
        // records the spoken answer. Any other press (a call of another thread, a card not armed
        // yet) is the talk key's own; while a call of Kai's waits its audio is held back anyway.
        if self.approvals.voice_answers() {
            return self.approval_press(audio, now).await;
        }
        // While Kai speaks, the press stops it and spends the gesture (K6).
        let speaking = self
            .session
            .as_ref()
            .and_then(|s| s.machine.playing().map(str::to_owned));
        if let Some(rid) = speaking {
            tracing::info!("talk: the press stops the voice (K6)");
            // A request that waits for this very reply ends with the gesture (K27); a mode that is
            // on goes on.
            if self.conversation == Some(Conversation::Requested) {
                self.conversation_by_owner("the talk key");
            }
            let wait = self.s.flush_wait;
            if let Some(s) = self.session.as_mut() {
                s.cut(&rid, Cut::Stop, wait, now).await;
            }
            self.key = Some(Key {
                at: now,
                consumed: true,
                muted: self.muted,
                gated: false,
            });
            return self.settle();
        }
        // Conversation mode ends at the press, which spends the gesture (K27).
        if self.conversation.is_some() {
            self.conversation_by_owner("the talk key");
            self.key = Some(Key {
                at: now,
                consumed: true,
                muted: self.muted,
                gated: false,
            });
            return self.settle();
        }
        if !self.feed_up {
            self.not_running();
            self.key = Some(Key {
                at: now,
                consumed: true,
                muted: self.muted,
                gated: false,
            });
            return self.settle();
        }
        // K17: a call of Kai's own waits now; the whole hold stays silent (`Key::gated`).
        let gated = self.approvals.gates_audio();
        self.key = Some(Key {
            at: now,
            consumed: false,
            muted: self.muted,
            gated,
        });
        // The owner's own press: a session it binds may be bound again after an unknown 4003.
        self.unknown_rebound = false;
        // A new hold: a commit an earlier hold left waiting for `session.created` is not this
        // one's (its audio goes with this hold instead), so it cannot cut this hold short.
        self.commit_on_create = false;
        if let Some(s) = self.session.as_mut().filter(|s| !s.created) {
            s.commit_pending = false;
        }
        if self.muted {
            tracing::info!("talk: muted, so the hold talks nothing (§5.4)");
            return self.settle();
        }
        // A session Kai bound by itself (a job's result) has no microphone yet: the press opens it.
        if self.session.is_some() {
            self.open_capture(audio);
        }
        // A push-to-talk turn starts: the session's buffer is cleared, the pre-roll goes first.
        // Not while a call of Kai's own waits (K17): this press is talk (the card has not armed
        // for the voice), but nothing of it reaches the session, and its release commits nothing.
        if gated {
            tracing::info!("talk: a call of Kai's waits for an approval; this hold sends nothing");
        } else if self.push_to_talk() {
            if self.preroll_ms.is_none() && !self.preroll.is_empty() {
                tracing::info!(
                    "talk: the pre-roll is {} ms (no session named prefix_padding_ms yet; \
                     OpenAI's default)",
                    super::DEFAULT_PREROLL_MS
                );
            }
            let preroll: Vec<i16> = self.preroll.drain(..).collect();
            self.carry.clear();
            match self.session.as_mut().filter(|s| s.created) {
                Some(s) => {
                    s.grace_at = None;
                    // A turn the server holds that was never committed goes (thinking stops).
                    if let Some(rid) = s.machine.current().map(str::to_owned) {
                        s.cut(&rid, Cut::Stop, self.s.flush_wait, now).await;
                    }
                    s.send(ClientEvent::Clear);
                    s.ptt_press();
                    for c in preroll.chunks(super::CHUNK) {
                        s.send(ClientEvent::append_pcm16(c));
                    }
                }
                None => {
                    self.pending.clear();
                    self.pending.extend(preroll);
                }
            }
        }
        if self.session.is_none() {
            if self.current_req.is_none() && self.bind_wait.is_some() {
                // A bind waits for the last close: the press joins it (K2-7).
                tracing::info!("talk: the press binds once the last session's close is answered");
                self.bind_wait = Some(true);
                self.open_capture(audio);
                self.connect_since.get_or_insert(now);
            } else {
                // A press while Kai asks for the thread (a new conversation, a re-ask after a 4004
                // or a bind's 404, its own follow) binds the answer as soon as it is known, as the
                // owner's bind (review KAI-4 K4-1); with nothing on its way it asks.
                if self.current_req.is_some() {
                    tracing::info!(
                        "talk: the press binds the conversation's thread once it is known"
                    );
                }
                self.begin_connect(audio, now, true);
            }
        }
        self.settle();
    }

    /// The talk key came up (§8.1).
    pub async fn talk_release(&mut self, audio: &impl AudioPort) {
        if self.key.is_none() {
            return;
        }
        // What the microphone heard up to the release belongs to the hold.
        if self.talking() {
            self.drain_capture();
        }
        let key = self.key.take().expect("checked above");
        if self.approvals.recording() {
            return self.approval_release();
        }
        if key.consumed {
            return self.settle();
        }
        let held = key.at.elapsed();
        if held < self.s.hold {
            tracing::info!(
                held_ms = held.as_millis() as u64,
                "talk: a tap toggles listening"
            );
            // The tap's audio is no turn in push-to-talk.
            let ptt = self.push_to_talk_for(true);
            if ptt {
                self.pending.clear();
                self.carry.clear();
            }
            if let Some(s) = self.session.as_mut().filter(|s| s.created) {
                if ptt {
                    s.send(ClientEvent::Clear);
                }
                s.ptt_release(false);
            }
            return self.toggle_listening(audio).await;
        }
        if key.muted {
            tracing::info!("talk: a hold while muted is ignored (§5.4)");
            return self.settle();
        }
        if !self.push_to_talk() {
            // Automatic: the server's detector ends the turn.
            return self.settle();
        }
        if key.gated || self.approvals.gates_audio() {
            // A hold while a call of Kai's waits (it began before the call came, or before the
            // card armed), or one that began while it waited and outlived the wait: no commit
            // (K17); its audio never went, so a commit would send a clipped turn.
            tracing::info!("talk: a call waits for an approval; the hold's turn is not committed");
            self.pending.clear();
            self.carry.clear();
            if let Some(s) = self.session.as_mut().filter(|s| s.created) {
                s.send(ClientEvent::Clear);
                s.ptt_release(false);
            }
            return self.settle();
        }
        tracing::info!(
            held_ms = held.as_millis() as u64,
            "talk: commit the push-to-talk turn"
        );
        let tail: Vec<i16> = std::mem::take(&mut self.carry);
        match self.session.as_mut().filter(|s| s.created) {
            Some(s) => {
                if !tail.is_empty() {
                    s.send(ClientEvent::append_pcm16(&tail));
                }
                s.send(ClientEvent::Commit);
                s.send(ClientEvent::ResponseCreate);
                s.ptt_release(true);
            }
            None => {
                self.pending.extend(tail);
                match self.session.as_mut() {
                    Some(s) => s.commit_pending = true,
                    None => self.commit_on_create = true,
                }
            }
        }
        self.settle();
    }

    /// The tray's left click, its Start/Stop item, or a tap of the talk key.
    pub async fn toggle_listening(&mut self, audio: &impl AudioPort) {
        if self.locked {
            tracing::info!("voice: the screen is locked; listening stays off");
            return;
        }
        // In conversation mode the tray's toggle ends the mode and nothing else (K27).
        if self.conversation.is_some() {
            return self.conversation_by_owner("the tray");
        }
        let on = !self.listening;
        self.set_listening(on, audio).await;
    }

    pub async fn set_listening(&mut self, on: bool, audio: &impl AudioPort) {
        self.listen(on, true, audio).await;
    }

    /// `voice.on_at_start` (K4): listening on at the feed's first `hello`, Kai's own doing, so it
    /// never takes a thread another client's voice holds (K5).
    pub async fn listen_at_start(&mut self, audio: &impl AudioPort) {
        self.listen(true, false, audio).await;
    }

    async fn listen(&mut self, on: bool, by_owner: bool, audio: &impl AudioPort) {
        if on == self.listening {
            return self.settle();
        }
        if on && self.locked {
            tracing::info!("voice: the screen is locked; listening stays off");
            return self.settle();
        }
        if on && !self.feed_up {
            self.not_running();
            return self.settle();
        }
        self.listening = on;
        tracing::info!(on, mode = self.mode.key(), "voice: listening");
        if on {
            self.note = None;
            self.commit_on_create = false;
            if by_owner {
                self.unknown_rebound = false;
            }
            if let Some(s) = self.session.as_mut() {
                s.grace_at = None;
                s.commit_pending = false;
            }
            if let Some(owner) = self.bind_wait.as_mut() {
                // A bind waits for the last close: listening on joins it (K2-7).
                *owner |= by_owner;
                self.open_capture(audio);
                self.connect_since.get_or_insert(Instant::now());
            } else if self.session.is_none() {
                // What is really on its way decides, never a wait that only looks like opening
                // (review KAI-4 K4-2): a request on its way (a re-ask after a 4004, Kai's own
                // follow) answers for it and binds; with none, Kai asks now. Either way the
                // microphone opens and the connect timeout runs.
                self.begin_connect(audio, Instant::now(), by_owner);
            } else {
                // A session Kai bound by itself (a job's result) has no microphone yet.
                self.open_capture(audio);
            }
            self.update_session();
        } else {
            self.end_session().await;
        }
        self.settle();
    }

    /// The tray's radio (§7 item 3).
    pub fn set_mode(&mut self, mode: VoiceMode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        tracing::info!(mode = mode.key(), "voice: mode");
        if self.listening
            && let Some(s) = self.session.as_mut().filter(|s| s.created)
            && mode == VoiceMode::PushToTalk
            && !self.key.is_some_and(|k| !k.consumed && !k.muted)
        {
            // What the server holds of an unfinished turn goes; nothing flows until the key.
            s.machine.turn_failed();
            s.send(ClientEvent::Clear);
        }
        self.update_session();
        self.settle();
    }

    /// Middle click, the menu's check item and the `mute` shortcut (§5.4).
    pub fn set_muted(&mut self, muted: bool) {
        if muted != self.muted {
            self.muted = muted;
            tracing::info!(muted, "voice: mute");
        }
        self.settle();
    }

    pub fn toggle_mute(&mut self) {
        self.set_muted(!self.muted);
    }

    /// The `new-conversation` shortcut and menu item (§8.1): `current {new: true}`, then rebind.
    pub fn new_conversation(&mut self) {
        if self.locked {
            tracing::info!("voice: the screen is locked; no new conversation");
            return;
        }
        if !self.feed_up {
            self.not_running();
            return;
        }
        let then = match (&self.session, &self.current_req) {
            (Some(_), _) => Then::Rebind,
            (None, Some(req)) if req.then == Then::Open => Then::Open,
            // A bind waits for the last close: it binds the new conversation (K2-7).
            (None, None) if self.bind_wait.is_some() => Then::Open,
            (None, _) => Then::Remember,
        };
        tracing::info!(?then, "voice: a new conversation");
        self.current_req = Some(CurrentReq {
            fut: self.current_fut(true),
            then,
            by_owner: true,
        });
        self.ensure_tick();
    }

    // ------------------------------------------------------------- sessions

    /// The automatic mode's detector, or `None` (push-to-talk, and a press session, K5).
    pub(super) fn detection(&self) -> Option<TurnDetection> {
        if self.continuous() {
            let resolved = self.session.as_ref().and_then(|s| s.resolved.as_deref());
            Some(TurnDetection::for_automatic(resolved))
        } else {
            None
        }
    }

    /// The session runs manual turns: push-to-talk mode, or a hold with listening off.
    pub(super) fn push_to_talk(&self) -> bool {
        self.push_to_talk_for(self.listening)
    }

    fn push_to_talk_for(&self, listening: bool) -> bool {
        !(self.conversation_on() || (listening && self.mode == VoiceMode::Automatic))
    }

    pub(super) fn update_session(&mut self) {
        let detection = self.detection();
        let half_duplex = self.half_duplex;
        if let Some(s) = self.session.as_mut() {
            s.update(detection, half_duplex);
        }
    }

    fn not_running(&mut self) {
        let why = self
            .feed_note
            .clone()
            .unwrap_or_else(|| "the feed is not connected".to_owned());
        tracing::info!(%why, "voice: lmgw is not running");
        self.note = Some(format!("lmgw is not running ({why})"));
        self.notify("Kai: lmgw is not running", why);
    }

    /// The press or listening on (§5.3): capture starts into the buffer, and Kai asks for the
    /// current thread, then binds it. A request already on its way (Kai's own, a new
    /// conversation's, a re-ask after a 4004) answers for it: its thread is bound, as the owner's
    /// where `by_owner` (never taken back from an ask the owner made). The connect timeout counts
    /// from the first of them.
    fn begin_connect(&mut self, audio: &impl AudioPort, now: Instant, by_owner: bool) {
        self.open_capture(audio);
        self.connect_since.get_or_insert(now);
        match self.current_req.as_mut() {
            Some(req) => {
                req.then = Then::Open;
                req.by_owner |= by_owner;
            }
            None => {
                self.current_req = Some(CurrentReq {
                    fut: self.current_fut(false),
                    then: Then::Open,
                    by_owner,
                })
            }
        }
        self.ensure_tick();
    }

    pub(super) fn open_capture(&mut self, audio: &impl AudioPort) {
        if self.capture.is_none() {
            tracing::info!("voice: the microphone opens");
            self.capture = Some(audio.open_capture());
        }
    }

    /// Binds the conversation's thread, `self.thread` (one bind in flight, module doc). Kai's own
    /// binds (`by_owner` false: a rebind, `voice.on_at_start`) never take a thread another client's
    /// voice session holds (K5): the feed's view first, then lmgw's refusal of a bind that does
    /// not take over (K2-13).
    pub(super) async fn bind(&mut self, now: Instant, by_owner: bool) {
        let Some(thread) = self.thread.clone() else {
            tracing::warn!("voice: no thread to bind");
            self.bind_wait = None;
            self.connect_since = None;
            return;
        };
        if let Some(by) = self.voice_elsewhere.get(&thread.id).cloned() {
            if by_owner {
                tracing::info!(thread = thread.id, %by, "voice: the owner's press takes the thread over");
            } else {
                tracing::info!(thread = thread.id, %by, "voice: the thread is bound elsewhere; staying off (K5)");
                self.bind_wait = None;
                self.listening = false;
                self.key = None;
                self.pending.clear();
                self.connect_since = None;
                self.commit_on_create = false;
                self.notify(
                    "Kai: voice in use elsewhere",
                    format!("voice in use on {by}; Kai stays off until you press the talk key"),
                );
                return;
            }
        }
        // Never two sessions: one still there is closed first, as at quit (K2-7).
        if let Some(old) = self.session.take() {
            debug_assert!(false, "a bind while a session is bound");
            tracing::warn!(
                from = old.thread.id,
                "voice: a bind while a session is bound; that one closes first"
            );
            let t = self.close(old).await;
            self.closing.push(t);
        }
        // A session holds a slot of the key's concurrency_limit until lmgw answered its close
        // (api §1.3): the next bind waits for the last close, at most `voice.close_wait_s`.
        if self.closing.iter().any(|t| !t.is_finished()) {
            tracing::info!(
                thread = thread.id,
                "voice: binding once the last session's close is answered"
            );
            self.connect_since.get_or_insert(now);
            let owner = self.bind_wait.unwrap_or(false) || by_owner;
            self.bind_wait = Some(owner);
            self.ensure_tick();
            return;
        }
        self.bind_wait = None;
        tracing::info!(thread = thread.id, take_over = by_owner, "voice: binding");
        let (tx, rx) = mpsc::unbounded_channel();
        let socket = self.link.open(thread.id, by_owner, tx);
        let mut s = Session::new(socket, rx, thread, now);
        self.session_no += 1;
        s.by_owner = by_owner;
        s.commit_pending = std::mem::take(&mut self.commit_on_create);
        s.audio_up = self.audio_up;
        self.connect_since.get_or_insert(now);
        self.session = Some(s);
        self.ensure_tick();
    }

    /// The gateway changes (a confirmed re-pair) or its key was revoked: the session bound there
    /// ends as at quit (the playing reply's truncate, then the close), whether listening is on or a
    /// press session waits out its grace; listening goes off, and what Kai knew of that gateway's
    /// thread and of its other clients is forgotten.
    pub async fn leave_gateway(&mut self) {
        tracing::info!("voice: leaving the gateway; its session ends");
        self.listening = false;
        self.key = None;
        self.end_session().await;
        self.thread = None;
        self.last_turn = None;
        self.voice_elsewhere.clear();
        self.turns_elsewhere.clear();
        self.approvals.forget_all();
        self.settle();
    }

    /// The feed's dead-link verdict (review KAI-2 K2-6): the bound session shares that path, so it
    /// ends as a drop would (K5): closed as at quit, listening off, a notification. Nothing to do
    /// with no session and none on its way.
    pub async fn link_dead(&mut self, note: String) {
        if self.session.is_none() && !self.listening && !self.opening() {
            return;
        }
        tracing::warn!(%note, "voice: the feed found the link dead; the session ends (K5)");
        self.listening = false;
        self.key = None;
        self.end_session().await;
        self.note = Some(format!("voice stopped: the link to lmgw is dead ({note})"));
        self.notify(
            "Kai: voice stopped",
            format!(
                "voice stopped: the link to lmgw is dead ({note}); press the talk key to start again"
            ),
        );
        self.settle();
    }

    /// Ends the session as at quit, and releases what it held (listening off, grace over, a drop).
    pub(super) async fn end_session(&mut self) {
        self.current_req = None;
        self.bare_401_asked = false;
        self.rebind = None;
        self.bind_wait = None;
        self.connect_since = None;
        self.commit_on_create = false;
        self.pending.clear();
        self.conversation_dropped("the session was ended");
        if let Some(s) = self.session.take() {
            let t = self.close(s).await;
            self.closing.push(t);
        }
    }

    /// Closes `s` as at quit (§2): the playing reply's truncate, then the close; a task waits for
    /// lmgw's answer at most `voice.close_wait_s`.
    pub(super) async fn close(&mut self, mut s: Session) -> JoinHandle<()> {
        crate::delegate::session_ended();
        s.leave(self.s.flush_wait, Instant::now()).await;
        s.socket.close();
        // After the leave: nothing it brought lingers among the calls followed.
        crate::widgets::thread_apps::session_ended();
        let wait = self.s.close_wait;
        let tid = s.thread.id;
        tokio::spawn(async move {
            let answered = tokio::time::timeout(wait, async {
                while let Some(ev) = s.events.recv().await {
                    if let SessionEvent::Closed { code, reason } = ev {
                        return Some((code, reason));
                    }
                }
                None
            })
            .await;
            match answered {
                Ok(Some((code, reason))) => {
                    tracing::info!(thread = tid, code, %reason, "voice: the session closed")
                }
                Ok(None) => tracing::info!(thread = tid, "voice: the session's socket is gone"),
                Err(_) => tracing::warn!(
                    thread = tid,
                    "lmgw did not answer the voice session's close within {} s \
                     (voice.close_wait_s)",
                    wait.as_secs()
                ),
            }
            drop(s);
        })
    }

    /// The tick's part for the session's life: the connect timeout (§5.3), the grace close (K5)
    /// and a rebind at a quiet point (K5).
    pub(super) async fn lifecycle(&mut self, now: Instant) {
        // The bind that waited for the last close; a `current` request on its way binds with its
        // answer instead (K2-7).
        if self.session.is_none()
            && self.current_req.is_none()
            && self.closing.iter().all(|t| t.is_finished())
            && let Some(by_owner) = self.bind_wait.take()
        {
            self.bind(now, by_owner).await;
        }
        if let Some(since) = self.connect_since {
            let created = self.session.as_ref().is_some_and(|s| s.created);
            if created {
                self.connect_since = None;
            } else if now.saturating_duration_since(since) >= self.s.connect_timeout {
                let lost = self.pending.len() as f64 / crate::audio::RATE as f64;
                let secs = self.s.connect_timeout.as_secs();
                tracing::warn!(
                    secs,
                    lost_s = lost,
                    "voice: no session within voice.connect_timeout_s; the buffer is dropped"
                );
                self.notify(
                    "Kai: the voice session did not start",
                    format!(
                        "lmgw did not start the voice session within {secs} s \
                         (voice.connect_timeout_s); {lost:.1} s of audio were not sent"
                    ),
                );
                self.note = Some(format!(
                    "the last voice session did not start within {secs} s (voice.connect_timeout_s)"
                ));
                self.listening = false;
                self.key = None;
                self.end_session().await;
                return;
            }
        }
        self.conversation_tick(now);
        // WP18: a job's result at a quiet moment (`super::jobs`).
        self.jobs_tick(now).await;
        let key_down = self.key.is_some();
        // A call the session showed waits for its answer: the session stays for it (WP19).
        let approval = self.approvals.waiting_in_session(self.session_no);
        // A job's result waits to be spoken in it: the session stays for it (WP18).
        let job = self.jobs_hold_session();
        let Some(s) = self.session.as_mut().filter(|s| s.created) else {
            return;
        };
        // K5's grace: a press session closes `voice.grace_s` after its reply has played (a grace
        // past what the clock can hold never ends, K2-21).
        if !self.listening
            && !key_down
            && !approval
            && !job
            && s.quiet()
            && self.conversation.is_none()
        {
            if s.grace_at.is_none() {
                s.grace_at = now.checked_add(self.s.grace);
            }
            if s.grace_at.is_some_and(|at| now >= at) {
                tracing::info!(
                    grace_s = self.s.grace.as_secs(),
                    "voice: the press session's grace is over; it closes"
                );
                self.end_session().await;
                return;
            }
        } else if let Some(s) = self.session.as_mut() {
            s.grace_at = None;
        }
        // K5's rebind at a quiet point: no open response, nothing playing, no held key.
        let quiet = self.session.as_ref().is_some_and(Session::quiet) && !key_down;
        if quiet && let Some((thread, by_owner)) = self.rebind.take() {
            // The mode does not move to the next session: it ends, visibly (K27).
            self.conversation_cut("rebind", "the conversation moved to another thread");
            let old = self.session.take().expect("checked above");
            tracing::info!(
                from = old.thread.id,
                to = thread.id,
                "voice: rebinding (K5)"
            );
            let t = self.close(old).await;
            self.closing.push(t);
            self.thread = Some(thread);
            if self.listening {
                self.bind(now, by_owner).await;
            }
            // Else a press session's thread moved: the next press binds the new one.
        }
    }
}
