//! A job's result spoken (WP18, ruling 22, K26; lmgw's MCP Tasks design §3.4). A result enters its
//! Chat thread as a message of role `tool`; the feed says `task.done` and a session bound to that
//! thread says `lmgw.task.done`. lmgw starts no turn for it: Kai picks the moment.
//!
//! - **Which results.** Kai's own jobs (the host's table, `crate::jobs`: their `announce`, on for a
//!   job Kai's voice session started), and every result in Kai's conversation thread or in the
//!   thread a session of Kai's is bound to (a job the voice started, whoever ran it: announced). A
//!   result says itself once, whether the feed or the session says it first (the message ids seen
//!   are kept as long as the table keeps an ended job, `jobs.keep_ended_s`).
//! - **Announce off:** a notification names the job and its status; the result waits in the
//!   thread, and the next turn there answers it.
//! - **The quiet moment** ([`Voice::jobs_quiet`]): never while the owner talks (the talk key held,
//!   speech heard and not yet answered), never while Kai speaks or a reply is open, never while a
//!   call waits for an approval, never while another client's turn runs in the thread (K16), never
//!   while the screen is locked, never while muted, never while a session is on its way; and only
//!   after `jobs.quiet_ms` of all that. The same rule holds in push-to-talk and in continuous
//!   listening (conversation mode, automatic): in push-to-talk the owner speaks only with the key
//!   held, in continuous listening the server's detector hears the speech.
//! - **Muted** (a draft of 2026-10-09): the result waits until the unmute, still
//!   owed; Kai holds no session open for it meanwhile. **Do not disturb** (`crate::dnd`, the
//!   notification server's `Inhibited`): it is not spoken but notified (the server keeps the
//!   notification quiet as it does every other).
//! - **Spoken in a bound session:** a `response.create` with no new words, tagged
//!   (`kai_job_<n>`), which lmgw runs as a continuation answering every result no reply answered
//!   yet. A result that came while a response of the thread ran is never asked for while that
//!   response runs (lmgw says it answers it): Kai waits for its `response.done`, then asks at the
//!   next quiet moment, since `lmgw.task.done` and `response.done` are not ordered (a result that
//!   entered after the turn was saved can come first); lmgw's `empty_turn` speaks nothing when
//!   nothing is owed any more. A continuation cut off (a barge-in, a cough, a stop: `cancelled` or
//!   `incomplete`) is notified: the rest is in its chat.
//! - **No session:** Kai binds its conversation thread itself when the result is there (K5's
//!   `takeover=never`, a press session that closes `voice.grace_s` after the reply); a result in
//!   another thread, a thread another client's voice holds, or a session bound elsewhere gets the
//!   notification instead (Kai never moves its conversation for a result). That bind opens no
//!   microphone: only the owner's press or listening on does.
//! - **Refusals:** `empty_turn` (nothing left to answer: a turn of another window did) ends it
//!   silently; `turn_running` waits for that turn's end (the feed's `turn.done`, or the turn
//!   leaving the feed's live state), then asks again at the next quiet moment. A turn the feed
//!   never showed: one more ask; refused again, Kai waits `jobs.quiet_ms` for the feed to show it,
//!   and otherwise gives up with a notification (the result waits in its chat). A cough is never a
//!   continuation (lmgw: only the client's own create is), so it never speaks a result; Kai never
//!   answers a cough's `empty_turn` with a create.
//! - **The screen lock** (WP18, a draft of 2026-10-09):
//!   results that arrive while locked wait, notifications included; after the unlock they are
//!   spoken (or notified) at the first quiet moment. The lock ended the session, so a spoken one
//!   binds again as above.

use std::collections::HashMap;
use std::time::Duration;

use lmgw_client::realtime::TaskDone;
use tokio::time::Instant;

use super::link::{ClientEvent, ErrorFacts};
use super::{Voice, VoiceLink};

/// The `event_id` prefix of Kai's continuation creates.
const TAG: &str = "kai_job_";

/// lmgw's refusal codes of a continuation (lmgw-client's realtime module doc).
const EMPTY_TURN: &str = "empty_turn";
const TURN_RUNNING: &str = "turn_running";

/// One result to say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    thread_id: i64,
    message_id: i64,
    tool: String,
    status: String,
    announce: bool,
    /// It came while this response of its thread ran (lmgw: that response answers it): Kai asks
    /// for it only after that response's end (module doc).
    during: Option<String>,
}

/// The continuation on its way.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Create {
    thread_id: i64,
    event_id: String,
    /// Its `response.created`.
    response: Option<String>,
}

/// A `turn_running` refusal Kai waits out: the turn whose end it waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TurnWait {
    thread_id: i64,
    /// The feed showed that turn: Kai waits for its `turn.done`, or for it to leave the feed's
    /// live state (a resync, the feed down). Not shown: Kai waits `jobs.quiet_ms` from `since`
    /// for the feed to show it, then gives up (module doc).
    shown: bool,
    since: Instant,
}

/// The voice's part of the jobs (module doc).
pub(super) struct Results {
    table: crate::jobs::Jobs,
    quiet: Duration,
    waiting: Vec<Waiting>,
    /// The results said, by message id, and when (pruned after `jobs.keep_ended_s`).
    seen: HashMap<i64, Instant>,
    create: Option<Create>,
    turn: Option<TurnWait>,
    /// A `turn_running` refusal whose turn the feed never showed was asked again once.
    asked_unseen: bool,
    /// Do not disturb is on (`crate::dnd`).
    dnd: bool,
    quiet_since: Option<Instant>,
    /// Kai bound this thread to speak a result (K5's `takeover=never`).
    announcing: Option<i64>,
    n: u64,
}

impl Results {
    pub(super) fn new(quiet: Duration) -> Self {
        Self {
            table: crate::jobs::Jobs::default(),
            quiet,
            waiting: Vec::new(),
            seen: HashMap::new(),
            create: None,
            turn: None,
            asked_unseen: false,
            dnd: false,
            quiet_since: None,
            announcing: None,
            n: 0,
        }
    }
}

impl<L: VoiceLink> Voice<L> {
    /// The host's job table (`Core::set_jobs`).
    pub fn set_jobs(&mut self, table: crate::jobs::Jobs) {
        self.results.table = table;
    }

    /// A result entered its thread: the feed's `task.done` (`from_session` false) or the bound
    /// session's `lmgw.task.done`.
    pub(super) fn task_done(&mut self, d: TaskDone, from_session: bool) {
        let now = Instant::now();
        let keep = self.results.table.keep();
        if !keep.is_zero() {
            self.results
                .seen
                .retain(|_, at| now.saturating_duration_since(*at) < keep);
        }
        if self.results.seen.contains_key(&d.message_id) {
            return;
        }
        let session_thread = self.session.as_ref().map(|s| s.thread.id);
        let ours = self.results.table.said(&d.server_label, &d.task_id);
        let in_conversation = from_session
            || session_thread == Some(d.thread_id)
            || self.thread_id() == Some(d.thread_id);
        let announce = match ours {
            Some(a) => a,
            None if in_conversation => true,
            // Another server's job in a thread that is not Kai's conversation: not Kai's to say.
            None => return,
        };
        self.results.seen.insert(d.message_id, now);
        // A response of its thread runs: that response answers it (module doc).
        let during = self
            .session
            .as_ref()
            .filter(|s| s.thread.id == d.thread_id)
            .and_then(|s| {
                s.machine
                    .current()
                    .filter(|r| s.machine.is_open(r))
                    .map(str::to_owned)
            });
        tracing::info!(
            thread = d.thread_id,
            message = d.message_id,
            job = %d.task_id,
            tool = %d.tool,
            status = %d.status,
            announce,
            kai_job = ours.is_some(),
            from_session,
            during = during.as_deref(),
            "voice: a job's result entered its thread"
        );
        self.results.waiting.push(Waiting {
            thread_id: d.thread_id,
            message_id: d.message_id,
            tool: d.tool,
            status: d.status,
            announce,
            during,
        });
        self.ensure_tick();
    }

    /// Do not disturb changed (`crate::dnd`).
    pub fn set_dnd(&mut self, on: bool) {
        self.results.dnd = on;
        self.jobs_wake();
    }

    /// Something the jobs' wait reads changed outside an event of the session (the feed's live
    /// state, the feed's link, do not disturb): the tick runs when it is needed for it.
    pub(super) fn jobs_wake(&mut self) {
        if self.jobs_need_tick() {
            self.ensure_tick();
        }
    }

    /// Something waits to be said or notified, and the tick is needed for it (not while locked:
    /// the unlock settles again).
    pub(super) fn jobs_need_tick(&self) -> bool {
        let r = &self.results;
        if r.waiting.is_empty() || self.locked {
            return false;
        }
        // A notification to send.
        if r.dnd || r.waiting.iter().any(|w| !w.announce) {
            return true;
        }
        // Muted: the unmute settles again. A response that answers them: its end comes as an
        // event. A turn the feed shows: its end comes as one too.
        if self.muted || r.waiting.iter().all(|w| w.during.is_some()) {
            return false;
        }
        r.turn
            .is_none_or(|t| !(t.shown && self.turns_elsewhere.contains_key(&t.thread_id)))
    }

    /// A press session Kai opened, or any session, stays while a result waits to be said in it
    /// (K5's grace waits for it).
    pub(super) fn jobs_hold_session(&self) -> bool {
        let Some(tid) = self.session.as_ref().map(|s| s.thread.id) else {
            return false;
        };
        self.results
            .create
            .as_ref()
            .is_some_and(|c| c.thread_id == tid)
            || (!self.locked
                && !self.muted
                && !self.results.dnd
                && self
                    .results
                    .waiting
                    .iter()
                    .any(|w| w.thread_id == tid && w.announce))
    }

    /// Nothing goes on that a spoken result would cut into (module doc), for `thread`.
    fn jobs_quiet(&self, thread: i64) -> bool {
        if self.locked || self.muted || !self.feed_up || self.key.is_some() {
            return false;
        }
        if self.approvals.gates_audio()
            || self.approvals.recording()
            || self.approvals.waiting_in_session(self.session_no)
        {
            return false;
        }
        if self.turns_elsewhere.contains_key(&thread) {
            return false;
        }
        match self.session.as_ref() {
            Some(s) => s.created && s.quiet(),
            None => !self.opening() && self.current_req.is_none(),
        }
    }

    /// The tick's part (module doc).
    pub(super) async fn jobs_tick(&mut self, now: Instant) {
        // A continuation whose session went (a close, the lock) is gone with it; what it was to
        // answer is asked again.
        if let Some(c) = &self.results.create
            && self
                .session
                .as_ref()
                .is_none_or(|s| s.thread.id != c.thread_id)
        {
            tracing::info!(
                thread = c.thread_id,
                "voice: the session went before the job's result was spoken"
            );
            self.results.create = None;
        }
        // A result whose response no longer runs (it ended, its session went) is owed again.
        let open: Option<(i64, Vec<String>)> = self.session.as_ref().map(|s| {
            let open = self
                .results
                .waiting
                .iter()
                .filter_map(|w| w.during.clone())
                .filter(|r| s.machine.is_open(r))
                .collect();
            (s.thread.id, open)
        });
        for w in &mut self.results.waiting {
            if let Some(r) = &w.during
                && !open
                    .as_ref()
                    .is_some_and(|(tid, open)| *tid == w.thread_id && open.contains(r))
            {
                tracing::info!(
                    thread = w.thread_id,
                    response = %r,
                    "voice: the response a job's result came during went; it is asked for"
                );
                w.during = None;
            }
        }
        if self.results.waiting.is_empty() || self.locked {
            self.results.quiet_since = None;
            return;
        }
        // Results not to be spoken: their notification, now that the screen is unlocked.
        let (quiet_ones, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.results.waiting)
            .into_iter()
            .partition(|w| !w.announce);
        self.results.waiting = rest;
        for w in quiet_ones {
            self.job_notice(&w, "it waits in its chat");
        }
        // Do not disturb: notified instead of spoken.
        if self.results.dnd && self.results.create.is_none() {
            for w in std::mem::take(&mut self.results.waiting) {
                self.job_notice(
                    &w,
                    "it waits in its chat; do not disturb is on, so Kai does not speak it",
                );
            }
            self.results.turn = None;
        }
        if self.results.waiting.is_empty() || self.results.create.is_some() {
            return;
        }
        let Some(target) = self
            .results
            .waiting
            .iter()
            .find(|w| w.during.is_none())
            .map(|w| w.thread_id)
        else {
            return;
        };
        if let Some(t) = self.results.turn {
            if t.thread_id != target {
                self.results.turn = None;
            } else if self.turns_elsewhere.contains_key(&target) {
                // The feed shows the turn: Kai waits for its end.
                self.results.turn = Some(TurnWait { shown: true, ..t });
                return;
            } else if t.shown {
                tracing::info!(
                    thread = target,
                    "voice: the running turn left the feed's live state; the jobs' results are \
                     asked for again at a quiet moment"
                );
                self.results.turn = None;
                self.results.asked_unseen = false;
                self.results.quiet_since = None;
            } else if now.saturating_duration_since(t.since) < self.results.quiet {
                return;
            } else {
                tracing::info!(
                    thread = target,
                    "voice: lmgw says a turn runs that the feed never showed; giving up"
                );
                self.results.turn = None;
                self.results.asked_unseen = false;
                self.jobs_not_spoken(
                    target,
                    "it waits in its chat; Kai could not speak it, lmgw says a turn runs there \
                     that Kai does not see",
                );
                return;
            }
        }
        if !self.jobs_quiet(target) {
            self.results.quiet_since = None;
            return;
        }
        let since = *self.results.quiet_since.get_or_insert(now);
        if now.saturating_duration_since(since) < self.results.quiet {
            return;
        }
        self.results.quiet_since = None;
        self.results.turn = None;
        let bound = self.session.as_ref().map(|s| s.thread.id);
        match bound {
            Some(tid) if tid == target => {
                self.results.n += 1;
                let event_id = format!("{TAG}{}", self.results.n);
                tracing::info!(thread = tid, %event_id, "voice: speaking the jobs' results (response.create, no new words)");
                if let Some(s) = self.session.as_mut() {
                    s.grace_at = None;
                    s.send_tagged(ClientEvent::ResponseCreate, event_id.clone());
                }
                self.results.announcing = None;
                self.results.create = Some(Create {
                    thread_id: tid,
                    event_id,
                    response: None,
                });
            }
            Some(_) => self.jobs_not_spoken(
                target,
                "it waits in its chat; Kai's voice is in another conversation",
            ),
            None if self.results.announcing == Some(target) => {
                // Kai bound the thread for it, and the session did not come (a refusal, a drop).
                self.results.announcing = None;
                self.jobs_not_spoken(target, "it waits in its chat; Kai's voice could not start");
            }
            None if self.thread_id() == Some(target) => {
                if let Some(by) = self.voice_elsewhere.get(&target).cloned() {
                    self.jobs_not_spoken(
                        target,
                        &format!("it waits in its chat; the voice is in use on {by}"),
                    );
                    return;
                }
                tracing::info!(
                    thread = target,
                    "voice: binding the conversation to speak a job's result (ruling 22)"
                );
                self.results.announcing = Some(target);
                self.bind(now, false).await;
            }
            None => self.jobs_not_spoken(target, "it waits in its chat"),
        }
    }

    /// Every waiting result of `thread` is notified instead of spoken.
    fn jobs_not_spoken(&mut self, thread: i64, why: &str) {
        let (these, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.results.waiting)
            .into_iter()
            .partition(|w| w.thread_id == thread);
        self.results.waiting = rest;
        for w in these {
            self.job_notice(&w, why);
        }
    }

    fn job_notice(&mut self, w: &Waiting, why: &str) {
        self.notify(
            "Kai: a job ended",
            format!("{} {}: {why}", w.tool, w.status),
        );
    }

    /// The results of the continuation's thread are answered (or need no answer any more).
    fn jobs_answered(&mut self, thread: i64) {
        self.results.waiting.retain(|w| w.thread_id != thread);
        self.results.create = None;
        self.results.asked_unseen = false;
    }

    /// `response.created`: the first after Kai's continuation create is its response.
    pub(super) fn jobs_response_created(&mut self, response_id: &str) {
        if let Some(c) = self.results.create.as_mut()
            && c.response.is_none()
        {
            c.response = Some(response_id.to_owned());
        }
    }

    /// `response.done`. A result that came while this response ran is asked for now (lmgw says
    /// that response answers it, but `lmgw.task.done` and `response.done` are not ordered: a result
    /// that entered after the turn was saved can come before its `response.done`; lmgw's
    /// `empty_turn` then speaks nothing). The continuation's answered the results it was to
    /// answer (a refusal came as an `error` before it, [`Self::jobs_refused`]; a barge-in's words
    /// are a turn that answers them too); cut off (`cancelled`, `incomplete`), it is notified.
    pub(super) fn jobs_response_done(&mut self, response_id: &str, status: &str) {
        for w in &mut self.results.waiting {
            if w.during.as_deref() == Some(response_id) {
                tracing::info!(
                    thread = w.thread_id,
                    message = w.message_id,
                    %response_id,
                    status,
                    "voice: the response a job's result came during ended; it is asked for"
                );
                w.during = None;
            }
        }
        let Some(c) = self.results.create.clone() else {
            return;
        };
        if c.response.as_deref() != Some(response_id) {
            return;
        }
        tracing::info!(thread = c.thread_id, %response_id, status, "voice: the jobs' results were answered");
        if matches!(status, "cancelled" | "incomplete") {
            let cut: Vec<Waiting> = self
                .results
                .waiting
                .iter()
                .filter(|w| w.thread_id == c.thread_id)
                .cloned()
                .collect();
            for w in cut {
                self.job_notice(&w, "Kai's reply to it was cut off; it is in its chat");
            }
        }
        self.jobs_answered(c.thread_id);
    }

    /// An `error` echoing `event_id`: one of Kai's continuations was refused (`true`), handled
    /// here rather than as a voice error.
    pub(super) fn jobs_refused(&mut self, event_id: &str, e: &ErrorFacts) -> bool {
        if !event_id.starts_with(TAG) {
            return false;
        }
        let Some(c) = self
            .results
            .create
            .clone()
            .filter(|c| c.event_id == event_id)
        else {
            tracing::debug!(%event_id, code = ?e.code, "voice: a refusal of an earlier job create");
            return true;
        };
        self.results.create = None;
        match e.code.as_deref() {
            Some(EMPTY_TURN) => {
                tracing::info!(
                    thread = c.thread_id,
                    "voice: nothing left to answer (empty_turn): the jobs' results were answered"
                );
                self.jobs_answered(c.thread_id);
            }
            Some(TURN_RUNNING) => {
                let shown = self.turns_elsewhere.contains_key(&c.thread_id);
                let again = !shown && !std::mem::replace(&mut self.results.asked_unseen, true);
                tracing::info!(
                    thread = c.thread_id,
                    shown,
                    again,
                    "voice: a turn runs in the thread (turn_running); asking again after it"
                );
                // A turn the feed never showed: one more ask at the next quiet moment.
                self.results.turn = (!again).then_some(TurnWait {
                    thread_id: c.thread_id,
                    shown,
                    since: Instant::now(),
                });
            }
            _ => {
                let words = match &e.code {
                    Some(code) => format!("{code}: {}", e.message),
                    None => e.message.clone(),
                };
                tracing::warn!(thread = c.thread_id, %words, "voice: the jobs' results could not be spoken");
                self.jobs_not_spoken(
                    c.thread_id,
                    &format!("Kai could not speak it ({words}); it waits in its chat"),
                );
            }
        }
        true
    }

    /// A turn in `thread` ended (the feed's `turn.done`): a `turn_running` wait is over.
    pub(super) fn jobs_turn_done(&mut self, thread: i64) {
        if self.results.turn.is_some_and(|t| t.thread_id == thread) {
            tracing::info!(
                thread,
                "voice: the running turn ended; the jobs' results are asked for again at a quiet moment"
            );
            self.results.turn = None;
            self.results.asked_unseen = false;
            self.results.quiet_since = None;
            self.ensure_tick();
        }
    }
}
