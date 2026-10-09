//! Conversation mode (ruling 23, K27): a temporary override for the rest of the voice session. Kai
//! listens as in continuous listening (K11), whatever the configured mode, until the mode ends; the
//! configured mode is back then.
//!
//! - **Start** (`conversation_start`, Kai's own session): the request waits for the current reply
//!   ([`Conversation::Requested`]) and takes effect at the first quiet point after it
//!   ([`Conversation::Active`]); the server's turn detection is switched on then.
//! - **End:** `conversation_end`, the talk key (the press ends it and spends the gesture, as a press
//!   that stops the voice does, K6; a press that stops a reply ends a request that waits for that
//!   reply, and a mode already on goes on), the tray (its listening toggle), the silence timeout
//!   (`voice.conversation_timeout_s`, 0: none; PipeWire being down is silence, and so is muted
//!   time: the timeout runs on while muted, the owner's decision of 2026-10-09), the screen lock
//!   (`crate::screen_lock`), or the session's end.
//! - **A drop ends it** (K5), whichever way the session went: a close of any kind (a 4004 that
//!   Kai binds again after included), the feed's dead-link verdict, a connect timeout, and a K5
//!   rebind to another thread (a new conversation, the folder's rollover). The configured mode is
//!   back, and the tooltip says so (the rebind and the drops that notify nothing of their own
//!   also send a notification). The mode is never carried to the next session.
//! - **K16** holds back as in continuous listening, and counts as activity: the timeout runs only
//!   while nothing happens.
//!
//! The ring stays up (the ring means Kai is active, [`Voice::standby`]); the tray shows the quiet
//! listening icon and says so in its tooltip.

use std::time::Duration;

use tokio::time::Instant;

use super::{Voice, VoiceLink, VoiceMode};
use lmgw_client::voice::VoiceState;

/// Where conversation mode is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Conversation {
    /// Asked for; takes effect after the current reply.
    Requested,
    /// On. `active_at` is the last time anything happened (the silence timeout counts from it).
    Active { active_at: Instant },
}

impl<L: VoiceLink> Voice<L> {
    /// Conversation mode has taken effect: the microphone streams to the server's turn detection.
    pub(super) fn conversation_on(&self) -> bool {
        matches!(self.conversation, Some(Conversation::Active { .. }))
    }

    /// Continuous listening (K11): conversation mode, or listening on in automatic mode.
    pub(super) fn continuous(&self) -> bool {
        self.conversation_on() || (self.listening && self.mode == VoiceMode::Automatic)
    }

    /// Words for the silence timeout, for the tool's result and the tooltip.
    fn conversation_timeout_words(&self) -> String {
        match self.s.conversation_timeout {
            Some(t) => format!(
                "after {} s without speech (voice.conversation_timeout_s)",
                t.as_secs()
            ),
            None => "never by silence (voice.conversation_timeout_s = 0)".to_owned(),
        }
    }

    /// `conversation_start`: the tool's answer, or why it cannot.
    pub fn conversation_start(&mut self) -> Result<String, String> {
        if self.locked {
            return Err(crate::screen_lock::REFUSAL.to_owned());
        }
        if self.session.is_none() {
            return Err("conversation mode needs Kai's voice session, and none is open".to_owned());
        }
        let ends = format!(
            "it ends by conversation_end, the talk key, the tray, or {}",
            self.conversation_timeout_words()
        );
        if self.conversation.is_some() {
            return Ok(format!("Conversation mode is already on; {ends}."));
        }
        tracing::info!("voice: conversation mode requested; it starts after the reply (K27)");
        self.conversation = Some(Conversation::Requested);
        self.ensure_tick();
        Ok(format!(
            "Conversation mode is on from the end of this reply: Kai listens without the talk \
             key; {ends}."
        ))
    }

    /// `conversation_end`: the tool's answer.
    pub fn conversation_end(&mut self) -> String {
        if self.conversation.is_none() {
            return "Conversation mode was not on.".to_owned();
        }
        self.conversation_over("conversation_end");
        format!("Conversation mode ended; {}.", self.configured_listening())
    }

    /// What the configured mode is, in words.
    pub(super) fn configured_listening(&self) -> &'static str {
        match (self.listening, self.mode) {
            (true, VoiceMode::Automatic) => "Kai keeps listening (automatic mode)",
            (true, VoiceMode::PushToTalk) => "Kai listens to the talk key (push-to-talk mode)",
            (false, _) => "Kai listens to the talk key and closes the session after the grace",
        }
    }

    /// Ends the mode (or its request) and returns the session to the configured mode.
    pub(super) fn conversation_over(&mut self, why: &str) {
        if self.conversation.take().is_some() {
            tracing::info!(
                why,
                "voice: conversation mode ends; the configured mode returns"
            );
            self.update_session();
            self.settle();
        }
    }

    /// The session is gone and none is on its way: nothing is left for the mode to be on.
    pub(super) fn conversation_settle(&mut self) {
        if self.session.is_none() && !self.opening() {
            self.conversation_dropped("the session ended");
        }
    }

    /// The mode ends because its session did (a close, listening off, a dead link, a connect
    /// timeout). The session is gone, so there is nothing to tell it; the caller settles. `true`:
    /// a mode was on, or asked for.
    pub(super) fn conversation_dropped(&mut self, why: &str) -> bool {
        if self.conversation.take().is_none() {
            return false;
        }
        tracing::info!(
            why,
            "voice: the session ended, and conversation mode with it; the configured mode returns"
        );
        true
    }

    /// Ends the mode (or its request) with a reason the owner is told: the tooltip's note and a
    /// notification.
    pub(super) fn conversation_cut(&mut self, why: &str, words: &str) {
        if self.conversation.is_none() {
            return;
        }
        self.conversation_over(why);
        let body = format!("{words}; {}", self.configured_listening());
        self.note = Some(format!("conversation mode ended: {body}"));
        self.notify("Kai: conversation mode ended", body);
    }

    /// The tick's part (lifecycle): a request takes effect at the first quiet point; the silence
    /// timeout. `session` is created.
    pub(super) fn conversation_tick(&mut self, now: Instant) {
        let Some(conversation) = self.conversation else {
            return;
        };
        let Some(s) = self.session.as_ref().filter(|s| s.created) else {
            return;
        };
        let quiet = s.quiet() && !self.talking();
        match conversation {
            Conversation::Requested if quiet => {
                tracing::info!("voice: conversation mode starts (K27)");
                self.conversation = Some(Conversation::Active { active_at: now });
                self.update_session();
                self.ensure_tick();
            }
            Conversation::Requested => {}
            Conversation::Active { active_at } => {
                // Anything going on, or K16's other turn, is activity. Muted time is silence: the
                // timeout runs on while muted (the owner's decision, 2026-10-09).
                let busy = !quiet
                    || self.state(now).is_some_and(|st| st != VoiceState::Idle)
                    || self.other_turn().is_some();
                if busy {
                    self.conversation = Some(Conversation::Active { active_at: now });
                    return;
                }
                let Some(limit) = self.s.conversation_timeout else {
                    return;
                };
                if now.saturating_duration_since(active_at) >= limit {
                    self.conversation_timed_out(limit);
                }
            }
        }
    }

    fn conversation_timed_out(&mut self, limit: Duration) {
        let secs = limit.as_secs();
        self.conversation_cut(
            "silence",
            &format!("no speech for {secs} s (voice.conversation_timeout_s)"),
        );
    }

    /// The owner ended it with a key or the tray.
    pub(super) fn conversation_by_owner(&mut self, how: &str) {
        self.conversation_over(how);
    }

    /// The tooltip's line.
    pub(super) fn conversation_note(&self) -> Option<String> {
        match self.conversation? {
            Conversation::Requested => Some("conversation mode starts after this reply".to_owned()),
            Conversation::Active { .. } => Some(format!(
                "conversation mode: Kai listens without the talk key; it ends by the talk key, \
                 the tray, or {}",
                self.conversation_timeout_words()
            )),
        }
    }
}
