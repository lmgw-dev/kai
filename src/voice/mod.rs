//! The voice session (§5, WP4): listening on and off, the talk key (K6), push-to-talk and
//! automatic, mute, the capture before the session exists (§5.3), barge-in, the grace close and
//! the rebind (K5), K16, takeover, drops, the hold and K21. lmgw is reached only through the
//! [`VoiceLink`] seam.
//!
//! The core owns one [`Voice`]. It polls [`Voice::next`] in its select loop and hands the wake to
//! [`Voice::handle`], forwards the owner's commands (tray, shortcuts) and the feed's facts, and
//! after each of them reads [`Voice::model`] for the tray, [`Voice::ring`] for the ring overlay, and
//! drains [`Voice::take_notices`] into desktop notifications.
//!
//! - [`session`]: one bound session (replies, the player, the truncate-first cut);
//! - [`control`]: the owner's side (the talk key, listening, mode, mute, a new conversation) and
//!   opening, rebinding and closing sessions;
//! - [`thread`]: the conversation's thread (§4): `current` at a session's start, at a connected
//!   start, when the folder's idle time passes with no turn, when the thread was deleted or
//!   archived, on `folder.current` and `resync`; the folder itself is `crate::conversation`'s;
//! - [`events`]: what the socket and the server say, and the tick that moves audio;
//! - the derived voice state, the truncate bookkeeping and the realtime events are
//!   `lmgw-client`'s (K3): `voice::Machine`, `truncate::Replies`, `realtime`;
//! - [`timing`]: the timing line and the fallbacks that answered;
//! - approvals (WP19, `crate::approvals`): while a call of Kai's own waits, no audio goes to the
//!   session, and once the card armed for it the talk key records the spoken answer locally (K17);
//!   the session's `mcp_approval_request` items and the feed's `approval.*` reach the card from
//!   here.

mod approval;
mod control;
mod conversation_mode;
mod events;
mod jobs;
pub mod link;
mod session;
mod thread;
mod timing;

#[cfg(test)]
mod approval_live_tests;
#[cfg(test)]
mod conversation_live_tests;
#[cfg(test)]
mod jobs_live_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod lmgw_live_tests;
#[cfg(test)]
mod log_tests;
#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval};

use crate::approvals::{self, Approvals};
use crate::audio::Capture;
use crate::config::Config;
use crate::lmgw_link::KeyLoss;
use crate::overlay::{RingInput, RingState};
use crate::tray::model::{VoiceModel, VoiceShown};
use link::{Current, Pending, Refusal, SessionEvent, ThreadRef};
use lmgw_client::voice::VoiceState;
use session::Session;

#[cfg(test)]
pub use link::NoVoice;
pub use link::VoiceLink;
pub use thread::Why;

/// The session's audio chunks: 20 ms of PCM16 at 24 kHz (§5.3).
pub const CHUNK: usize = 480;

/// The tray's name for a thread lmgw has not titled yet (its own word for a new thread).
pub const UNTITLED: &str = "New chat";

/// The tooltip's line while the screen is locked (the lock's own line says the rest).
const LOCK_NOTE: &str = "the screen locked: Kai stopped listening and speaking, and stays idle \
                         after the unlock until you press the talk key or start listening";

/// Push-to-talk's pre-roll when no session echo named `prefix_padding_ms` (OpenAI's default for
/// it). Logged when used.
const DEFAULT_PREROLL_MS: u32 = 300;

/// The owner's choice in the tray (`voice.mode`, §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    /// The server's detector ends turns; the microphone streams while listening is on.
    #[default]
    Automatic,
    /// `turn_detection: null`: audio goes only while the talk key is held.
    PushToTalk,
}

impl VoiceMode {
    pub const ALL: [VoiceMode; 2] = [VoiceMode::Automatic, VoiceMode::PushToTalk];

    pub fn key(self) -> &'static str {
        match self {
            VoiceMode::Automatic => "automatic",
            VoiceMode::PushToTalk => "push_to_talk",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VoiceMode::Automatic => "Automatic",
            VoiceMode::PushToTalk => "Push-to-talk",
        }
    }
}

/// What the voice needs from the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceSettings {
    pub mode: VoiceMode,
    pub grace: Duration,
    pub connect_timeout: Duration,
    pub close_wait: Duration,
    /// `shortcuts.hold_ms`: shorter is a tap.
    pub hold: Duration,
    /// `shortcuts.max_hold_s`: a hold no release ended in this long ends (a stuck-key guard).
    pub max_hold: Duration,
    /// `audio.flush_wait_ms`.
    pub flush_wait: Duration,
    /// `voice.conversation_timeout_s` (K27): conversation mode ends after this long without speech;
    /// `None` (0): never by silence.
    pub conversation_timeout: Option<Duration>,
    /// `[approvals]` (WP19).
    pub approvals: approvals::ApprovalSettings,
    /// `jobs.quiet_ms` (WP18): how long nothing goes on before a job's result is spoken.
    pub jobs_quiet: Duration,
}

impl VoiceSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            mode: c.voice.mode,
            grace: c.voice.grace(),
            connect_timeout: c.voice.connect_timeout(),
            close_wait: c.voice.close_wait(),
            hold: c.shortcuts.hold(),
            max_hold: c.shortcuts.max_hold(),
            flush_wait: Duration::from_millis(c.audio.flush_wait_ms),
            conversation_timeout: c.voice.conversation_timeout(),
            approvals: approvals::ApprovalSettings::from_config(c),
            jobs_quiet: c.jobs.quiet(),
        }
    }
}

/// Who, in a feed event: the principal's description (api §1.7) and whether it is Kai itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct By {
    pub name: String,
    pub own: bool,
}

/// The feed's facts the voice acts on (§3.2's table). The feed raises them through the
/// `lmgw_link` seam; the core passes them on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedFact {
    /// `hello`'s and `state`'s live part: the voice sessions and the turns that run now.
    Live {
        voice: Vec<(i64, By)>,
        turns: Vec<(i64, By)>,
    },
    /// `voice.bound`.
    VoiceBound { thread_id: i64, by: By },
    /// `voice.ended`.
    VoiceEnded { thread_id: i64, by: By },
    /// `turn.started`.
    TurnStarted { thread_id: i64, by: By },
    /// `turn.done`.
    TurnDone { thread_id: i64 },
    /// `folder.current` for Kai's folder (the core passes on only its own folder's): the
    /// conversation moved to `thread_id`, or has no current thread any more (`None`: its thread
    /// was deleted, moved out or archived, reason `gone`; or the folder stopped being ongoing,
    /// `not_ongoing`).
    FolderCurrent {
        thread_id: Option<i64>,
        previous: Option<i64>,
        reason: String,
    },
    /// `thread.created` / `thread.updated`: the thread as the list shows it now, with when its
    /// newest message was written (unix seconds).
    ThreadChanged {
        thread_id: i64,
        title: String,
        archived: bool,
        last_message_at: Option<i64>,
    },
    /// `thread.deleted`, or a thread gone by the time its event was delivered.
    ThreadGone { thread_id: i64 },
    /// `resync`: the feed could not resume where it left off, so a move of the conversation may
    /// have been missed; Kai asks for its thread again (§3.2).
    Resync,
    /// The feed's open set at `caught_up` (WP19, device link §7.3): every call that waits now in
    /// the threads the device sees, at every connection of the feed; the approvals reconcile
    /// against it.
    ApprovalsOpen(Vec<approvals::Requested>),
    /// After `caught_up`, a call of a thread the device sees waits for an approval: an
    /// `approval.requested`, or a `state`'s set that gained it.
    ApprovalRequested(approvals::Requested),
    /// After `caught_up`, calls that wait no more, by id: decided, their reply or thread gone, or
    /// out of the device's reach.
    ApprovalsWithdrawn(Vec<String>),
    /// `task.done` (WP18): a job's result entered its thread.
    TaskDone(lmgw_client::realtime::TaskDone),
}

/// A desktop notification to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
}

/// What [`Voice::next`] woke for.
#[derive(Debug)]
pub enum Wake {
    Session(SessionEvent),
    /// The socket's channel ended without a close (its adapter went away).
    SessionGone,
    Current(Result<Current, Refusal>),
    Tick,
    /// The folder's idle time passed with no turn in the conversation's thread (§4).
    Idle,
    /// An answer on the approval card, or an approvals request that came back (WP19).
    Approval(approvals::Wake),
}

/// What a pending `current` request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Bind a session to the answer (a press, listening on).
    Open,
    /// A new conversation while a session is bound: rebind at the next quiet point.
    Rebind,
    /// A new conversation with no session: only remember the thread.
    Remember,
    /// Kai asks by itself (§4): rebind at the next quiet point if the conversation moved, else
    /// only remember the thread.
    Follow(Why),
}

struct CurrentReq {
    fut: Pending<Result<Current, Refusal>>,
    then: Then,
    /// The owner asked (a press, the tray), not Kai by itself (K5).
    by_owner: bool,
}

/// What an ask for the conversation's thread is for, kept when Kai asks again (after a 4004, a
/// bind's 404, a 401 whose code did not come): whether its answer is bound, and whether the owner
/// asked (review KAI-4 K4-1). Merged, a bind and the owner's ask win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Intent {
    bind: bool,
    by_owner: bool,
}

impl Intent {
    fn of(req: &CurrentReq) -> Self {
        Self {
            bind: req.then == Then::Open,
            by_owner: req.by_owner,
        }
    }

    fn merge(self, other: Self) -> Self {
        Self {
            bind: self.bind || other.bind,
            by_owner: self.by_owner || other.by_owner,
        }
    }
}

/// The talk key, from its press to its release (K6).
#[derive(Debug, Clone, Copy)]
struct Key {
    at: Instant,
    /// The press stopped Kai speaking: the gesture is spent.
    consumed: bool,
    /// Muted at the press: a hold talks nothing (§5.4); a tap still toggles.
    muted: bool,
    /// A call of Kai's own waited at the press (K17): the whole hold sends the session nothing and
    /// commits nothing, also once the gate lifts while it is held (a clipped turn otherwise).
    gated: bool,
}

pub struct Voice<L> {
    s: VoiceSettings,
    link: L,
    /// The owner's choices.
    listening: bool,
    mode: VoiceMode,
    muted: bool,
    /// K27's override of the owner's choices, for the rest of the session.
    conversation: Option<conversation_mode::Conversation>,
    key: Option<Key>,
    /// What the session runs with (`EchoStatus::half_duplex`).
    half_duplex: bool,
    /// The feed: up, held, and what other clients do.
    feed_up: bool,
    feed_note: Option<String>,
    feed_held: bool,
    voice_elsewhere: HashMap<i64, String>,
    turns_elsewhere: HashMap<i64, String>,
    /// PipeWire is reachable (the audio thread's link).
    audio_up: bool,
    session: Option<Session>,
    current_req: Option<CurrentReq>,
    /// A rebind waiting for a quiet point (K5), and whether the owner asked (a new conversation).
    rebind: Option<(ThreadRef, bool)>,
    /// A bind waiting for the last session's close to be answered (one slot per session; one bind
    /// in flight, review KAI-2 K2-7): it binds the conversation's thread as known then, `thread`.
    /// Whether the owner asked (`by_owner`, K5).
    bind_wait: Option<bool>,
    /// Sessions closed and waiting for lmgw's answer to the close.
    closing: Vec<JoinHandle<()>>,
    capture: Option<Capture>,
    /// Captured audio not sent yet (§5.3): before `session.created`, and across a rebind.
    pending: Vec<i16>,
    /// When the wait for the session began (`voice.connect_timeout_s` counts from here).
    connect_since: Option<Instant>,
    /// A push-to-talk release came while the thread was still being asked for: the session commits
    /// once its buffer went.
    commit_on_create: bool,
    /// Push-to-talk's pre-roll: the last `prefix_padding_ms` of the microphone.
    preroll: VecDeque<i16>,
    preroll_ms: Option<u32>,
    /// What did not fill a 20 ms chunk yet.
    carry: Vec<i16>,
    tick: Option<Interval>,
    /// The conversation's thread as last known: the tooltip's title, the thread a rebind goes to.
    thread: Option<ThreadRef>,
    /// The conversation's folder (§4, from `crate::conversation`): `current` asks it.
    folder: Option<i64>,
    /// Its idle rollover (`ongoing.idle_minutes`; `None`: only on request).
    idle: Option<Duration>,
    /// The newest message or turn Kai knows of in `thread`, on the wall clock (`None`: none, so it
    /// may be a new, empty thread, which is never idle). The idle rollover is timed from it, as lmgw
    /// times it, on the wall clock: tokio's timers do not count a suspend (review KAI-2 K2-8).
    last_turn: Option<std::time::SystemTime>,
    /// When the idle rollover is looked at next at the latest (every [`thread::IDLE_CHECK`]).
    idle_check: Option<Instant>,
    /// The wall clock (the tests run it on tokio's paused clock).
    clock: thread::WallClock,
    /// A `current` refused for the folder's sake (409 `not_ongoing`, `folder_no_model`, 404),
    /// and whether the owner asked: the core's conversation takes it.
    folder_refusal: Option<(Refusal, bool)>,
    /// A bind was refused 401 without its code (the body did not come with the head, review KAI-4
    /// K4-5) and Kai asked `current` for it; a second such 401 before a session is created is
    /// "pair again", as any 401 without a known code.
    bare_401_asked: bool,
    /// The session Kai bound again after a 4003 of a kind it does not know (review KAI-4 K4-7):
    /// another such close turns listening off instead of binding once more. The owner's press or
    /// listening on clears it.
    unknown_rebound: bool,
    /// A 4003 of a kind Kai does not know: lmgw's sentence, which the core says once per reason,
    /// with the feed's `revoked` of the same kind (review KAI-4 K4-7).
    revoked_unknown: Option<String>,
    timing_line: Option<String>,
    /// `lmgw.response.timing` not handed to the ring yet.
    timing_new: Option<lmgw_client::realtime::VoiceTiming>,
    /// Numbers hold periods: every hold change starts one (K21, §5.6).
    period: u64,
    notified: HashSet<(u64, String, String)>,
    notices: Vec<Notice>,
    /// A 4003 close of a kind Kai knows, or a 401 on `current` or the bind (§3.3): the core
    /// revokes the pairing.
    revoked: Option<(String, KeyLoss)>,
    /// The last thing to say about the voice (a drop, a takeover, an error), for the tooltip.
    note: Option<String>,
    /// `shortcuts.max_hold_s` ended the last hold: said until the next press.
    stuck: Option<String>,
    /// `error` events by code and message: repeats are counted in the log (§5.6).
    errors: HashMap<(Option<String>, String), u64>,
    /// The calls that wait for an approval, the card, the spoken answer (WP19).
    approvals: Approvals,
    /// Counts the sessions bound, so a call a session showed is answered through that session
    /// only (`approvals`).
    session_no: u64,
    /// The screen is locked (`crate::screen_lock`): Kai is not active, the talk key and listening
    /// on do nothing.
    locked: bool,
    /// Jobs' results to speak (WP18, `jobs`).
    results: jobs::Results,
}

impl<L: VoiceLink> Voice<L> {
    pub fn new(s: VoiceSettings, link: L) -> Self {
        let jobs_quiet = s.jobs_quiet;
        let approvals = Approvals::new(
            s.approvals.clone(),
            Box::new(crate::approvals::overlay::send),
        );
        Self {
            mode: s.mode,
            s,
            link,
            listening: false,
            muted: false,
            conversation: None,
            key: None,
            half_duplex: false,
            feed_up: false,
            feed_note: None,
            feed_held: false,
            voice_elsewhere: HashMap::new(),
            turns_elsewhere: HashMap::new(),
            audio_up: true,
            session: None,
            current_req: None,
            rebind: None,
            bind_wait: None,
            closing: Vec::new(),
            capture: None,
            pending: Vec::new(),
            connect_since: None,
            commit_on_create: false,
            preroll: VecDeque::new(),
            preroll_ms: None,
            carry: Vec::new(),
            tick: None,
            thread: None,
            folder: None,
            idle: None,
            last_turn: None,
            idle_check: None,
            clock: thread::WallClock::new(),
            folder_refusal: None,
            bare_401_asked: false,
            unknown_rebound: false,
            revoked_unknown: None,
            timing_line: None,
            timing_new: None,
            period: 0,
            notified: HashSet::new(),
            notices: Vec::new(),
            revoked: None,
            note: None,
            stuck: None,
            errors: HashMap::new(),
            approvals,
            session_no: 0,
            locked: false,
            results: jobs::Results::new(jobs_quiet),
        }
    }

    pub fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }

    /// The tests move the wall clock alone (a suspend) through this.
    #[cfg(test)]
    pub fn wall_skew(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.clock.skew.clone()
    }

    /// The next thing to handle. Cancel-safe: every source keeps its state between calls.
    pub async fn next(&mut self) -> Wake {
        let idle_at = self.idle_at();
        let Self {
            session,
            current_req,
            tick,
            approvals,
            ..
        } = self;
        tokio::select! {
            ev = async {
                match session.as_mut() {
                    Some(s) => s.events.recv().await,
                    None => std::future::pending().await,
                }
            } => match ev {
                Some(e) => Wake::Session(e),
                None => Wake::SessionGone,
            },
            r = async {
                match current_req.as_mut() {
                    Some(req) => req.fut.as_mut().await,
                    None => std::future::pending().await,
                }
            } => Wake::Current(r),
            _ = async {
                match tick.as_mut() {
                    Some(t) => { t.tick().await; }
                    None => std::future::pending().await,
                }
            } => Wake::Tick,
            () = async {
                match idle_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => Wake::Idle,
            w = approvals.next() => Wake::Approval(w),
        }
    }

    // ------------------------------------------------------------- what the core reads

    pub fn take_notices(&mut self) -> Vec<Notice> {
        for (summary, body) in self.approvals.take_notices() {
            self.notify(summary, body);
        }
        std::mem::take(&mut self.notices)
    }

    /// A 4003 close of a kind Kai knows, or a 401 on `current` or the bind: the key was revoked
    /// (§3.3); the server's reason, and what it asks of the owner.
    pub fn take_revoked(&mut self) -> Option<(String, KeyLoss)> {
        self.revoked.take()
    }

    /// A 4003 close of a kind Kai does not know: lmgw's sentence, for the core to say once
    /// (review KAI-4 K4-7).
    pub fn take_revoked_unknown(&mut self) -> Option<String> {
        self.revoked_unknown.take()
    }

    /// A `current` refused for the folder's sake, and whether the owner asked (§4).
    pub fn take_folder_refusal(&mut self) -> Option<(Refusal, bool)> {
        self.folder_refusal.take()
    }

    /// The conversation's thread as last known (Open in lmgw, §4).
    pub fn thread_id(&self) -> Option<i64> {
        self.thread.as_ref().map(|t| t.id)
    }

    /// A new `lmgw.response.timing` for the ring's `setTiming`.
    pub fn take_timing(&mut self) -> Option<lmgw_client::realtime::VoiceTiming> {
        self.timing_new.take()
    }

    fn state(&self, now: Instant) -> Option<VoiceState> {
        self.session
            .as_ref()
            .filter(|s| s.created)
            .map(|s| s.state(now))
    }

    /// The talk key is held and talks (not spent on a stop, not muted).
    fn talking(&self) -> bool {
        self.key.is_some_and(|k| !k.consumed && !k.muted)
    }

    /// The talk key is held in a hold that began while a call of Kai's own waited (`Key::gated`).
    fn gated_hold(&self) -> bool {
        self.key.is_some_and(|k| k.gated)
    }

    /// Another principal's turn runs in the session's thread (K16).
    fn other_turn(&self) -> Option<&str> {
        let tid = self.session.as_ref().map(|s| s.thread.id)?;
        self.turns_elsewhere.get(&tid).map(String::as_str)
    }

    /// Continuous listening (automatic mode with listening on) actively listens: what the
    /// microphone hears reaches lmgw, live, or from the buffer a session on its way gets at
    /// `session.created` (§5.3: the first bind, a rebind, the bind after a 4004). Not while muted,
    /// sending K16's silence, or with PipeWire down.
    fn listens_continuously(&self) -> bool {
        self.continuous()
            && !self.muted
            && self.audio_up
            && self.other_turn().is_none()
            && (self.session.is_some() || self.opening())
    }

    /// Kai is in standby (K11): not actively listening, thinking or talking, so the ring is hidden;
    /// active otherwise, and the ring shows. `turn` is whether a turn runs; a turn is always
    /// active.
    /// - Continuous listening is active the whole time it is not muted
    ///   ([`Self::listens_continuously`]).
    /// - Push-to-talk (the owner's decision, 2026-10-08) and a press session are in standby
    ///   between presses, active from a press through the end of its turn.
    /// - Muted, and with no session and none on its way, Kai is in standby. The GPU hold changes
    ///   nothing here.
    /// - A hotword mode, when it comes, adds one fact that this reads: the word fired, set at the
    ///   word and cleared at its turn's start, or by a timeout when no speech follows the word.
    ///   The turn then keeps Kai active to its end.
    fn standby(&self, turn: bool) -> bool {
        !(turn || self.listens_continuously())
    }

    /// The ring's input (§9.4, the WP5 seam): `bound` while a session exists, `active` while Kai
    /// is not in standby ([`Self::standby`]), `turn` from a talk press or `speech_started` to the
    /// end of the reply's playback.
    ///
    /// A push-to-talk release that came before its session exists is still that press's turn: the
    /// session commits it once created (review KAI-5 R5-1). It counts only while that session is
    /// really on its way, so a bind that gave up cannot hold the ring up.
    pub fn ring(&self, now: Instant) -> RingInput {
        let state = self.state(now).unwrap_or_default();
        let talking = self.talking();
        let commit_pending = self.session.as_ref().is_some_and(|s| s.commit_pending)
            || (self.commit_on_create && self.opening());
        let ring_state = match state {
            _ if talking => RingState::Listening,
            VoiceState::Idle if commit_pending => RingState::Thinking,
            VoiceState::Idle => RingState::Idle,
            VoiceState::Listening => RingState::Listening,
            VoiceState::Thinking => RingState::Thinking,
            VoiceState::Speaking => RingState::Speaking,
            VoiceState::Interrupted => RingState::Interrupted,
        };
        let turn = talking || commit_pending || state != VoiceState::Idle;
        RingInput {
            bound: self.session.is_some(),
            active: !self.standby(turn),
            turn,
            state: ring_state,
            muted: self.muted,
            loading: self
                .session
                .as_ref()
                .and_then(|s| s.loading.values().next().cloned()),
            held: self.feed_held || self.session.as_ref().is_some_and(|s| !s.held.is_empty()),
        }
    }

    /// The tray's voice part (§5.1, §7).
    pub fn model(&self, now: Instant) -> VoiceModel {
        let shown = match self.state(now) {
            _ if self.talking() => VoiceShown::Listening,
            Some(VoiceState::Listening) => VoiceShown::Listening,
            Some(VoiceState::Thinking) => VoiceShown::Thinking,
            Some(VoiceState::Speaking) => VoiceShown::Speaking,
            Some(VoiceState::Interrupted) => VoiceShown::Interrupted,
            Some(VoiceState::Idle) if self.session.as_ref().is_some_and(|s| s.commit_pending) => {
                VoiceShown::Thinking
            }
            Some(VoiceState::Idle) if self.listening || self.conversation_on() => VoiceShown::Quiet,
            Some(VoiceState::Idle) => VoiceShown::Off,
            None if self.session.is_some() || self.opening() => VoiceShown::Connecting,
            None => VoiceShown::Off,
        };
        let mut notes = Vec::new();
        if let Some(by) = self.other_turn()
            && self.continuous()
        {
            notes.push(format!(
                "a turn of {by} runs in this conversation: automatic listening sends silence \
                 until it is done (the talk key still talks)"
            ));
        }
        if let Some(by) = self
            .thread
            .as_ref()
            .and_then(|t| self.voice_elsewhere.get(&t.id))
        {
            notes.push(format!("voice in use on {by}"));
        }
        if self.locked {
            notes.push(
                "the screen is locked: Kai does not listen, speak or run tools until it is \
                 unlocked"
                    .to_owned(),
            );
        }
        if self.muted {
            notes.push("muted: push-to-talk presses are ignored".to_owned());
        }
        if self.rebind.is_some() {
            notes.push("the conversation moved: Kai rebinds at the next quiet point".to_owned());
        }
        if shown == VoiceShown::Connecting {
            notes.push(format!(
                "connecting: audio is buffered for up to {} s (voice.connect_timeout_s)",
                self.s.connect_timeout.as_secs()
            ));
        }
        notes.extend(self.conversation_note());
        notes.extend(self.approvals.note());
        if let Some(n) = &self.note {
            notes.push(n.clone());
        }
        if let Some(n) = &self.stuck {
            notes.push(n.clone());
        }
        VoiceModel {
            locked: self.locked,
            on: self.listening || self.conversation.is_some(),
            mode: self.mode,
            muted: self.muted,
            shown,
            // An untitled thread is named as the dashboard's button names a new one.
            thread: self.thread.as_ref().map(|t| {
                if t.title.trim().is_empty() {
                    UNTITLED.to_owned()
                } else {
                    t.title.clone()
                }
            }),
            timing: self.timing_line.clone(),
            held: self.session.as_ref().is_some_and(|s| !s.held.is_empty()),
            notes,
        }
    }

    // ------------------------------------------------------------- facts from the core

    /// The feed is up (`hello` came) or down (`note`).
    pub fn feed(&mut self, up: bool, note: Option<String>) {
        self.feed_up = up;
        self.feed_note = note;
        if !up {
            // Live state is `hello`'s again once it is back (api §2.3).
            self.voice_elsewhere.clear();
            self.turns_elsewhere.clear();
        }
        self.jobs_wake();
    }

    /// The feed's `hold`: every change starts a hold period (K21, §5.6).
    pub fn hold(&mut self, active: bool) {
        if active != self.feed_held {
            self.period += 1;
            self.feed_held = active;
            if !active && let Some(s) = self.session.as_mut() {
                s.held.clear();
            }
        }
    }

    pub fn fact(&mut self, fact: FeedFact) {
        match fact {
            FeedFact::Live { voice, turns } => {
                self.voice_elsewhere = voice
                    .into_iter()
                    .filter(|(_, by)| !by.own)
                    .map(|(t, by)| (t, by.name))
                    .collect();
                self.turns_elsewhere = turns
                    .into_iter()
                    .filter(|(_, by)| !by.own)
                    .map(|(t, by)| (t, by.name))
                    .collect();
                self.jobs_wake();
            }
            FeedFact::VoiceBound { thread_id, by } if !by.own => {
                tracing::info!(thread_id, by = %by.name, "voice bound elsewhere");
                self.voice_elsewhere.insert(thread_id, by.name);
            }
            FeedFact::VoiceEnded { thread_id, by } if !by.own => {
                self.voice_elsewhere.remove(&thread_id);
            }
            FeedFact::VoiceBound { .. } | FeedFact::VoiceEnded { .. } => {}
            FeedFact::TurnStarted { thread_id, by } => {
                self.turn_seen(thread_id);
                self.approvals.turn_started(thread_id, &by.name, by.own);
                if by.own {
                    self.turns_elsewhere.remove(&thread_id);
                } else {
                    tracing::info!(thread_id, by = %by.name, "a turn of another principal runs");
                    self.turns_elsewhere.insert(thread_id, by.name);
                }
            }
            FeedFact::TurnDone { thread_id } => {
                self.turn_seen(thread_id);
                self.turns_elsewhere.remove(&thread_id);
                self.jobs_turn_done(thread_id);
            }
            FeedFact::TaskDone(d) => self.task_done(d, false),
            FeedFact::Resync => {
                // What the feed did not deliver may have changed any thread's waiting calls.
                let acts = self.approvals.resync();
                self.approval_acts(acts);
                // Only with a conversation to follow.
                if self.session.is_some() || self.thread.is_some() {
                    tracing::info!("voice: the feed resynced; asking for the current thread");
                    self.follow(Why::Resync);
                }
            }
            FeedFact::FolderCurrent {
                thread_id,
                previous,
                reason,
            } => self.folder_current(thread_id, previous, &reason),
            FeedFact::ThreadChanged {
                thread_id,
                title,
                archived,
                last_message_at,
            } => {
                self.approvals.thread_title(thread_id, &title);
                self.thread_changed(thread_id, title, archived, last_message_at)
            }
            FeedFact::ApprovalsOpen(set) => self.approvals.open_set(set),
            FeedFact::ApprovalRequested(r) => {
                self.approvals.requested(r.thread_id, r.request, None);
            }
            FeedFact::ApprovalsWithdrawn(ids) => self.approvals.withdrawn(&ids),
            FeedFact::ThreadGone { thread_id } => {
                self.approvals.thread_gone(thread_id);
                if self.thread_id() == Some(thread_id) {
                    tracing::info!(thread_id, "voice: the conversation's thread was deleted");
                    self.follow(Why::Gone);
                }
            }
        }
    }

    /// The screen locked or unlocked (`crate::screen_lock`, the owner's decision of 2026-10-09).
    /// Locked: conversation mode ends, a held talk key's hold ends with its audio discarded (no
    /// commit), listening goes off and the session ends as at quit (a playing reply is stopped with
    /// its truncate, a response on its way goes with the session), the capture closes, and the
    /// approval card hides (its calls stay waiting). Unlocked: the card shows again what still
    /// waits; nothing else resumes, Kai is idle.
    pub async fn screen_lock(&mut self, locked: bool) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        self.approvals.set_locked(locked);
        if locked {
            let was = (
                self.listening,
                self.conversation.is_some(),
                self.key.is_some(),
                self.session.is_some() || self.opening(),
            );
            tracing::info!(
                listening = was.0,
                conversation = was.1,
                key = was.2,
                session = was.3,
                "voice: the screen locked; Kai stops listening and speaking"
            );
            self.conversation_dropped("the screen locked");
            self.key = None;
            self.listening = false;
            self.end_session().await;
            self.note = Some(LOCK_NOTE.to_owned());
        } else {
            tracing::info!("voice: the screen unlocked; Kai is idle");
            if self.note.as_deref() == Some(LOCK_NOTE) {
                self.note = None;
            }
        }
        self.settle();
    }

    /// Conversation mode has taken effect (the accessibility switch follows it).
    pub fn conversation_active(&self) -> bool {
        self.conversation_on()
    }

    /// The echo mode's half duplex (§6.2) changed: a bound session is told.
    pub fn set_half_duplex(&mut self, on: bool) {
        self.half_duplex = on;
        self.update_session();
    }

    /// PipeWire's link: when it is back, a capture that closed with it is opened again.
    pub fn audio_link(&mut self, up: bool, audio: &impl crate::audio::AudioPort) {
        self.audio_up = up;
        if let Some(s) = self.session.as_mut() {
            s.audio_up = up;
        }
        if up && self.capture.as_ref().is_some_and(Capture::closed) {
            tracing::info!("voice: PipeWire is back; the capture opens again");
            self.capture = Some(audio.open_capture());
        }
    }

    fn notify(&mut self, summary: impl Into<String>, body: impl Into<String>) {
        let n = Notice {
            summary: summary.into(),
            body: body.into(),
        };
        tracing::info!(summary = %n.summary, body = %n.body, "voice: notify");
        self.notices.push(n);
    }

    /// Runs the 20 ms tick while anything needs it; stops it otherwise (no wakeups at idle).
    fn ensure_tick(&mut self) {
        if self.tick.is_none() {
            let mut t = tokio::time::interval(Duration::from_millis(20));
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            self.tick = Some(t);
        }
    }

    /// A session is being opened: its thread is asked for, or its bind waits for the last close.
    /// What starts a session never reads this; it looks at what is really on its way (review KAI-4
    /// K4-2).
    fn opening(&self) -> bool {
        self.bind_wait.is_some()
            || self
                .current_req
                .as_ref()
                .is_some_and(|r| r.then == Then::Open)
    }

    fn settle(&mut self) {
        self.conversation_settle();
        // WP19: Kai's own threads and the live session, as the approvals read them; verdicts a
        // session carried that ended before lmgw took them go by the route.
        let own: Vec<i64> = self
            .session
            .as_ref()
            .map(|s| s.thread.id)
            .into_iter()
            .chain(self.thread_id())
            .collect();
        let live = self
            .session
            .as_ref()
            .filter(|s| s.created)
            .map(|_| self.session_no);
        let acts = self.approvals.observe(own, live);
        self.approval_acts(acts);
        let needs_capture = self.listening
            || self.session.is_some()
            || self.opening()
            || self.talking()
            || self.approvals.recording();
        if !needs_capture && self.capture.take().is_some() {
            tracing::info!("voice: the microphone is released");
            self.pending.clear();
            self.preroll.clear();
            self.carry.clear();
        }
        // A held key keeps the tick for `shortcuts.max_hold_s`.
        let needs_tick = self.session.is_some()
            || self.current_req.is_some()
            || self.capture.is_some()
            || self.key.is_some()
            || self.jobs_need_tick();
        if !needs_tick {
            self.tick = None;
        } else {
            self.ensure_tick();
        }
        self.closing.retain(|t| !t.is_finished());
    }

    /// §2's quit, the voice's part: the playing reply gets its truncate, the session closes, and
    /// Kai waits for lmgw to take the close at most `voice.close_wait_s` (each close's own bound).
    pub async fn quit(&mut self) {
        self.listening = false;
        self.key = None;
        self.current_req = None;
        self.bind_wait = None;
        if let Some(s) = self.session.take() {
            let t = self.close(s).await;
            self.closing.push(t);
        }
        for t in self.closing.drain(..) {
            let _ = t.await;
        }
        self.capture = None;
        self.tick = None;
    }
}
