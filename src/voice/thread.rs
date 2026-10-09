//! The conversation's thread (§4, K5): which thread of the folder the voice continues, and when
//! Kai asks lmgw for it (`POST /chat/api/folders/{id}/current`).
//!
//! - **When.** A session's start (a press, listening on: `Then::Open`), a new conversation (the
//!   owner's), and by Kai itself ([`Why`]): at a connected start once the folder is ready, when the
//!   folder's idle time passes with no turn, when its thread was deleted or archived, and after a
//!   `resync`. One request at a time: a request on its way answers for the next reason too.
//! - **The idle rollover.** Timed on the wall clock, as lmgw times it (review KAI-2 K2-8): from the
//!   thread's newest message as lmgw says it (`last_message_at`, in `current`'s answer and the feed's
//!   `thread.updated`), or a later turn Kai saw in it (the feed's `turn.started` and `turn.done` of
//!   every principal, Kai's own included, and a reply of its own session). A thread with no message
//!   is never idle: no timer until its first. When the time passes while a turn runs, the timer
//!   starts over; `current` decides either way (lmgw counts from the newest message, and a thread a
//!   turn answers is never idle), and an idle `current` that kept the thread starts it over too.
//!   tokio's timers stop while the machine sleeps, so the wall clock is looked at every
//!   [`IDLE_CHECK`] at the latest, and at once when logind says the machine resumed
//!   ([`Voice::resumed`]).
//! - **A move** (`folder.current`, or an answer naming another thread) is followed at the next quiet
//!   point (K5): a listening session rebinds unless another client's voice holds the new thread, a
//!   press session closes and the next press binds the new thread (`control::lifecycle`).
//! - **After a close** that leaves the key good (4004: the thread left this device's reach) or in
//!   doubt (a 4003 of a kind Kai does not know), Kai asks once, and the answer decides
//!   ([`Voice::ask_current_again`]): where listening was on, the answer is bound again.
//! - **Refusals.** A 401 is the key's revocation (§3.3). `not_ongoing`, `folder_no_model` and a 404
//!   (the folder is gone) are the folder's business: the core hands them to
//!   `crate::conversation`, which says the owner's steps or looks the folder up again.

use std::time::{Duration, SystemTime};

use tokio::time::Instant;

use super::link::{Current, Refusal, ThreadRef};
use super::{CurrentReq, Intent, Pending, Then, Voice, VoiceLink};
use crate::conversation::Using;
use crate::lmgw_link::KeyLoss;

/// The idle rollover's wall-clock deadline is looked at this often at the latest, so a suspend
/// (which tokio's clock does not count) delays it by at most this much where logind's resume
/// signal does not come.
pub const IDLE_CHECK: Duration = Duration::from_secs(30);

/// The wall clock. In the tests it runs on tokio's paused clock, and a suspend is simulated by
/// moving it alone ([`WallClock::skew`]).
pub struct WallClock {
    #[cfg(test)]
    base: (SystemTime, Instant),
    #[cfg(test)]
    pub skew: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl WallClock {
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            base: (SystemTime::now(), Instant::now()),
            #[cfg(test)]
            skew: Default::default(),
        }
    }

    #[cfg(not(test))]
    pub fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    #[cfg(test)]
    pub fn now(&self) -> SystemTime {
        let skew = self.skew.load(std::sync::atomic::Ordering::Relaxed);
        self.base.0
            + Instant::now().saturating_duration_since(self.base.1)
            + Duration::from_secs(skew)
    }
}

/// Unix seconds as a wall-clock time (`None` past what it can hold).
fn at_unix(secs: i64) -> Option<SystemTime> {
    let secs = u64::try_from(secs).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Why Kai asks for the conversation's thread by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A connected start, or the folder became ready (or another folder took its place).
    Start,
    /// The feed's `resync`: a move may have been missed.
    Resync,
    /// The folder's idle time passed with no turn.
    Idle,
    /// The thread was deleted, moved out or archived.
    Gone,
    /// The bound session closed with 4004: its thread left this device's reach (api §1.6).
    OutOfReach,
    /// The bound session closed with a 4003 of a kind this build does not know: the answer says
    /// whether the key still opens (§3.3).
    Revoked,
    /// The bind was refused 401 and its code did not come with the answer's head (review KAI-4
    /// K4-5): `current`'s 401, read whole, says it.
    KeyRefused,
}

/// What a press says when the folder is not known yet.
const NO_FOLDER: &str = "Kai does not know its conversation's folder in lmgw yet";

/// Refusal codes that are the folder's business (§4).
fn folder_trouble(e: &Refusal) -> bool {
    matches!(e.code(), "not_ongoing" | "folder_no_model") || e.status == 404
}

impl<L: VoiceLink> Voice<L> {
    /// The conversation's folder (`crate::conversation`): its id and idle rollover, or none.
    pub fn set_folder(&mut self, using: Option<Using>) {
        let id = using.map(|u| u.id);
        if id != self.folder {
            if self.folder.is_some() {
                tracing::info!(from = ?self.folder, to = ?id, "voice: the conversation's folder changed");
                self.thread = None;
                self.last_turn = None;
            }
            self.folder = id;
        }
        let idle = using.and_then(|u| u.idle);
        if idle != self.idle {
            tracing::info!(idle_s = ?idle.map(|d| d.as_secs()), "voice: the folder's idle rollover");
            self.idle = idle;
        }
    }

    /// Kai knows the conversation's folder (remembered, or looked up).
    pub fn folder_known(&self) -> bool {
        self.folder.is_some()
    }

    /// The `current` request for the folder, or its refusal when Kai has none yet.
    pub(super) fn current_fut(&mut self, new: bool) -> Pending<Result<Current, Refusal>> {
        match self.folder {
            Some(f) => self.link.current(f, new),
            None => Box::pin(async { Err(Refusal::no_answer(NO_FOLDER)) }),
        }
    }

    /// Kai asks for the conversation's thread by itself (module doc).
    pub fn follow(&mut self, why: Why) {
        if self.folder.is_none() || !self.feed_up {
            return;
        }
        if self.current_req.is_some() {
            tracing::debug!(?why, "voice: a current request is on its way already");
            return;
        }
        tracing::info!(?why, "voice: asking for the conversation's current thread");
        self.current_req = Some(CurrentReq {
            fut: self.current_fut(false),
            then: Then::Follow(why),
            by_owner: false,
        });
        self.ensure_tick();
    }

    /// When the folder's idle time passes with no turn, on the wall clock: from the newest message
    /// or turn known, while the feed is up and the folder has an idle rollover (`None`: never, an
    /// idle past what the clock holds included, K2-21).
    fn idle_deadline(&self) -> Option<SystemTime> {
        match (self.idle, self.last_turn) {
            (Some(idle), Some(at)) if self.folder.is_some() && self.feed_up => at.checked_add(idle),
            _ => None,
        }
    }

    /// When to look at the idle rollover next: its deadline as the wall clock puts it now, and at
    /// the latest [`IDLE_CHECK`] from the last look (a suspend stops tokio's clock).
    pub(super) fn idle_at(&mut self) -> Option<Instant> {
        let Some(deadline) = self.idle_deadline() else {
            self.idle_check = None;
            return None;
        };
        let now = Instant::now();
        let left = deadline
            .duration_since(self.clock.now())
            .unwrap_or(Duration::ZERO);
        let check = *self.idle_check.get_or_insert(now + IDLE_CHECK);
        Some(now.checked_add(left).map_or(check, |at| at.min(check)))
    }

    /// The idle wake: the wall clock decides whether the time passed.
    pub(super) fn on_idle_wake(&mut self, now: Instant) {
        self.idle_check = None;
        if self.idle_deadline().is_some_and(|d| self.clock.now() >= d) {
            self.on_idle(now);
        }
    }

    /// logind says the machine resumed (K2-8): the idle rollover is looked at at once, by the wall
    /// clock, before the first turn after the resume goes into a thread lmgw already left.
    pub fn resumed(&mut self) {
        tracing::info!("voice: the machine resumed; the idle rollover is looked at now");
        self.on_idle_wake(Instant::now());
    }

    /// A turn in `thread_id` (any principal's): the idle rollover counts from now.
    pub(super) fn turn_seen(&mut self, thread_id: i64) {
        if self.thread_id() == Some(thread_id) {
            self.last_turn = Some(self.clock.now());
            self.idle_check = None;
        }
    }

    /// lmgw says when the thread's newest message was written: the rollover counts from it, or
    /// from a later turn Kai saw.
    fn message_at(&mut self, last_message_at: Option<i64>) {
        if let Some(at) = last_message_at.and_then(at_unix) {
            self.last_turn = Some(self.last_turn.map_or(at, |t| t.max(at)));
            self.idle_check = None;
        }
    }

    /// The idle time passed (§4): Kai asks `current`, which rolls over; not while a turn runs.
    pub(super) fn on_idle(&mut self, _now: Instant) {
        let busy = self.key.is_some()
            || self.session.as_ref().is_some_and(|s| !s.quiet())
            || self
                .thread_id()
                .is_some_and(|t| self.turns_elsewhere.contains_key(&t));
        if busy {
            tracing::debug!("voice: the idle time passed during a turn; it starts over");
            self.last_turn = Some(self.clock.now());
            return;
        }
        tracing::info!(
            idle_s = self.idle.map(|d| d.as_secs()),
            "voice: the folder's idle time passed with no turn"
        );
        self.last_turn = None;
        self.follow(Why::Idle);
    }

    /// `folder.current` for Kai's folder (§3.2's table).
    pub(super) fn folder_current(
        &mut self,
        thread_id: Option<i64>,
        previous: Option<i64>,
        reason: &str,
    ) {
        match thread_id {
            Some(t) if self.thread_id() != Some(t) => {
                tracing::info!(
                    thread = t,
                    ?previous,
                    reason,
                    "voice: the conversation moved"
                );
                let to = ThreadRef {
                    id: t,
                    title: String::new(),
                };
                self.moved_to(to);
            }
            Some(_) => {}
            // The folder stopped being ongoing: `folder.updated` tells the conversation.
            None if reason == "not_ongoing" => {}
            None => {
                let ours = self.thread_id();
                // A device's feed names no previous thread out of its reach.
                if ours.is_some() && (previous.is_none() || previous == ours) {
                    tracing::info!(?previous, reason, "voice: the conversation's thread ended");
                    self.follow(Why::Gone);
                }
            }
        }
    }

    /// The conversation's thread is now `to`, a thread new to it: a bound session on another
    /// thread rebinds at the next quiet point (K5).
    fn moved_to(&mut self, to: ThreadRef) {
        // A thread that just became current has no message yet.
        self.last_turn = None;
        if let Some(s) = &self.session
            && s.thread.id != to.id
        {
            tracing::info!(thread = to.id, "voice: rebind at the next quiet point");
            self.rebind = Some((to.clone(), false));
            self.ensure_tick();
        }
        self.thread = Some(to);
    }

    /// `thread.updated` (or `created`): the title of Kai's thread and its newest message's time;
    /// an archived one is gone (§4).
    pub(super) fn thread_changed(
        &mut self,
        thread_id: i64,
        title: String,
        archived: bool,
        last_message_at: Option<i64>,
    ) {
        if self.thread_id() != Some(thread_id) {
            return;
        }
        if archived {
            tracing::info!(thread_id, "voice: the conversation's thread was archived");
            return self.follow(Why::Gone);
        }
        self.message_at(last_message_at);
        if let Some(t) = self.thread.as_mut() {
            t.title = title.clone();
        }
        if let Some(s) = self.session.as_mut().filter(|s| s.thread.id == thread_id) {
            s.thread.title = title;
        }
    }

    /// `current`'s answer, for what asked it.
    pub(super) async fn on_current(&mut self, r: Result<Current, Refusal>, now: Instant) {
        let Some(req) = self.current_req.take() else {
            return;
        };
        match r {
            Ok(c) => self.got_current(c, req, now).await,
            Err(e) => self.current_refused(e, req),
        }
    }

    async fn got_current(&mut self, c: Current, req: CurrentReq, now: Instant) {
        let known = self.thread_id() == Some(c.thread.id);
        if c.rolled_over || !known {
            // A new thread has no message, so it is never idle until its first turn; another
            // thread's turns Kai saw are not this one's.
            self.last_turn = None;
        }
        self.idle_check = None;
        self.message_at(c.last_message_at);
        if req.then == Then::Follow(Why::Idle) && !c.rolled_over {
            // lmgw kept the thread (a message Kai did not see, or a turn running): the timer starts
            // over from the answer, so it never asks again at once.
            let now = self.clock.now();
            self.last_turn = Some(self.last_turn.map_or(now, |t| t.max(now)));
        }
        let t = c.thread;
        match req.then {
            Then::Open => {
                self.thread = Some(t);
                self.bind(now, req.by_owner).await;
            }
            Then::Rebind | Then::Remember | Then::Follow(_) => {
                if !known {
                    tracing::info!(
                        thread = t.id,
                        rolled_over = c.rolled_over,
                        then = ?req.then,
                        "voice: the conversation continues in this thread"
                    );
                }
                match &self.session {
                    Some(s) if s.thread.id != t.id => {
                        // A new conversation is the owner's own; Kai's own follow is not (K5).
                        let by_owner = req.then == Then::Rebind && req.by_owner;
                        self.rebind = Some((t.clone(), by_owner));
                        self.ensure_tick();
                    }
                    _ => {}
                }
                self.thread = Some(t);
            }
        }
    }

    fn current_refused(&mut self, e: Refusal, req: CurrentReq) {
        if req.then == Then::Open {
            self.listening = false;
            self.key = None;
            self.pending.clear();
            self.connect_since = None;
            self.commit_on_create = false;
        }
        if e.key_refused() {
            tracing::warn!("voice: current answered 401: {e}");
            let loss = KeyLoss::of_code(e.code());
            self.revoked = Some((e.message, loss));
            return;
        }
        if folder_trouble(&e) {
            tracing::warn!(then = ?req.then, "voice: current refused for the folder's sake: {e}");
            self.folder_refusal = Some((e, req.by_owner));
            return;
        }
        let e = e.to_string();
        match req.then {
            Then::Open => {
                tracing::warn!("voice: no current thread: {e}");
                self.note = Some(format!("voice could not start: {e}"));
                self.notify("Kai: voice could not start", e);
            }
            Then::Follow(Why::Resync) => {
                tracing::warn!("voice: after the feed's resync, no current thread: {e}");
                self.note = Some(format!(
                    "after the feed's resync Kai could not ask for its thread: {e}"
                ));
            }
            Then::Follow(why) => {
                tracing::warn!(?why, "voice: no current thread: {e}");
                self.note = Some(format!(
                    "Kai could not ask lmgw for its conversation's thread: {e}"
                ));
            }
            Then::Rebind | Then::Remember => {
                tracing::warn!("voice: no new conversation: {e}");
                self.note = Some(format!("no new conversation: {e}"));
                self.notify("Kai: no new conversation", e);
            }
        }
    }

    /// After a close that leaves the key good (4004) or in doubt (a 4003 of a kind Kai does not
    /// know), a bind's 404 or a 401 whose code did not come: Kai asks for the conversation's thread
    /// again, and the answer decides (§3.3). lmgw closes with 4004 only once the change is live
    /// (api §1.6, lmgw a50141d), so `current` already sees it: the answer is lmgw's word, taken as
    /// it comes, whatever thread it names. `intent` says what the ask is for, merged with a request
    /// still on its way, which this one replaces (review KAI-4 K4-1): where it binds, the answer is
    /// bound at once (by Kai itself unless the owner asked: K5, never a takeover then); else it is
    /// only remembered, and the next press binds it. A press or listening on meanwhile turns it
    /// into the owner's bind (`control`). The connect timeout counts from the first ask, never from
    /// a re-ask (review KAI-4 K4-6). A refusal is said as any `current`'s is: a 401 is the key's
    /// (with its code's loss), the folder's refusals go to the conversation.
    pub(super) fn ask_current_again(&mut self, why: Why, intent: Intent) {
        let intent = match self.current_req.take() {
            Some(on_its_way) => intent.merge(Intent::of(&on_its_way)),
            None => intent,
        };
        let then = if intent.bind {
            self.connect_since.get_or_insert(Instant::now());
            Then::Open
        } else {
            Then::Follow(why)
        };
        tracing::info!(
            ?why,
            ?then,
            by_owner = intent.by_owner,
            "voice: asking for the conversation's current thread again"
        );
        self.current_req = Some(CurrentReq {
            fut: self.current_fut(false),
            then,
            by_owner: intent.by_owner,
        });
        self.ensure_tick();
    }

    /// The conversation's folder is gone (§3.2's `folder.deleted`): the session ends as at quit,
    /// listening goes off, and the thread is forgotten; `note` says why.
    pub async fn leave_conversation(&mut self, note: String) {
        let had = self.session.is_some() || self.listening || self.opening();
        self.listening = false;
        self.key = None;
        self.end_session().await;
        self.thread = None;
        self.last_turn = None;
        if had {
            self.note = Some(format!("voice stopped: {note}"));
            self.notify("Kai: voice stopped", note);
        }
        self.settle();
    }
}
