//! The sidebar's turns (kai-firefox design §8.3). The sidebar is Kai (decision 9): lmgw records its
//! turns as Kai's own, their tool calls reach Kai as `Caller::Own`, and in hands-free mode they
//! count as the owner's act, since the owner typed them. Nothing here changes that. Kai tells the
//! sidebar's turns apart only for two things of its voice's, before the voice reads the feed:
//!
//! - **K16:** automatic listening holds back while a turn of the sidebar's runs, as for another
//!   principal's turn, so the room cannot cancel a typed turn.
//! - **K17:** the approval card names the sidebar as who asked, and the calls of a turn of the
//!   sidebar's are not "mine" for the voice: the talk key does not answer them and they hold back no
//!   audio, also in the conversation's thread.
//!
//! The voice reads such a turn as [`By::sidebar`] (`own: false`, `sidebar: true`). Whose a turn is
//! follows the turn, not the request that carried it:
//!
//! - **A new turn** (`send`, `continue`, `answer`, a message's `edit` and `regenerate`,
//!   [`Start::New`]): the request **marks** its thread before it goes to lmgw ([`Mark`]). The
//!   thread's next own `turn.started` that is no voice turn takes the mark: that turn is the
//!   sidebar's. The mark stays while the request is open, and once the request ended it waits for
//!   the core to read the thread's next `turn.started` or `turn.done` (or a running list without
//!   the thread): lmgw's feed reaches Kai on a connection of its own, and a short turn's stream can
//!   end before its `turn.started` is read. A request lmgw refused (its head no success) started no
//!   turn, since lmgw refuses before a turn begins: its mark goes at once.
//! - **A resume:** lmgw runs a turn whose calls were decided as the principal that started it,
//!   whoever decided them (Kai's card, the sidebar, the phone, the dashboard). So an own turn that
//!   is no voice turn, while the thread's last reply waits on calls or had calls decided since the
//!   thread's last `turn.started`, inherits the reading of the thread's last turn. One whose turn
//!   Kai never saw start (Kai started after it) is the sidebar's when the sidebar sent the verdicts
//!   (`approvals` marks its thread as [`Start::Resume`]), else Kai's.
//! - **Anything else** own is Kai's: its voice turns (`voice: true`), which never take a mark, and
//!   an own turn that is neither marked nor a resume.
//!
//! A `hello`'s or `state`'s list of running turns is read the same way: a turn Kai saw start keeps
//! its reading, one that began while the feed was down is read as it starts.
//!
//! **One running turn a thread** (lmgw's `chat_live`): a turn that starts cancels the thread's
//! running one, whose `turn.done` (it carries no turn's id) can come after the new `turn.started`.
//! So Kai counts each thread's turns started and not done, and a `turn.done` that leaves one running
//! is the cancelled turn's: the voice does not hear it, and the running turn stays as it was read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::voice::{By, FeedFact};

/// How a turn route starts its turn (`super::admit`, module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// A new turn of the thread: the sidebar's.
    New,
    /// Verdicts on the thread's waiting calls: lmgw resumes the turn they belong to, which stays
    /// whose it was.
    Resume,
}

/// The sidebar's turns (module doc). Cheap to clone.
#[derive(Clone, Default)]
pub struct SidebarTurns(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    next: u64,
    /// The turn routes' marks, oldest first.
    marks: Vec<Marked>,
    threads: HashMap<i64, Thread>,
    /// The calls that wait now, by id: their thread (the feed's approval facts).
    waiting: HashMap<String, i64>,
}

struct Marked {
    id: u64,
    thread: i64,
    start: Start,
    /// Its request is still open.
    open: bool,
}

/// What Kai read of a thread's turns.
#[derive(Default)]
struct Thread {
    /// Its turns started and not done.
    running: u32,
    /// Whether its last turn (the running one, or the one whose reply waits) is the sidebar's;
    /// `None`: Kai saw none start.
    last: Option<bool>,
    /// Calls of it stopped waiting (decided, declined) since its last `turn.started`.
    decided: bool,
}

/// A thread marked by a turn route's request (module doc).
pub struct Mark {
    turns: SidebarTurns,
    id: u64,
}

impl Mark {
    /// lmgw refused the request: it started no turn, so the mark goes now.
    pub fn no_turn(self) {
        self.turns.lock().marks.retain(|m| m.id != self.id);
    }
}

impl Drop for Mark {
    /// The request ended: an unspent mark waits for the core's reading of the feed (module doc).
    fn drop(&mut self) {
        if let Some(m) = self.turns.lock().marks.iter_mut().find(|m| m.id == self.id) {
            m.open = false;
        }
    }
}

/// How the sidebar's turns are named on the approval card and in the voice's notes.
pub const NAME: &str = "Kai's sidebar in Firefox";

impl SidebarTurns {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Marks `thread` for a request on a turn route (module doc).
    pub fn mark(&self, thread: i64, start: Start) -> Mark {
        let mut st = self.lock();
        st.next += 1;
        let id = st.next;
        st.marks.push(Marked {
            id,
            thread,
            start,
            open: true,
        });
        Mark {
            turns: self.clone(),
            id,
        }
    }

    /// The gateway was left (a re-pair, a revocation): nothing read of its threads applies.
    pub fn forget(&self) {
        let mut st = self.lock();
        st.marks.clear();
        st.threads.clear();
        st.waiting.clear();
    }

    /// The feed's `fact` as the voice reads it (module doc); `None`: the voice does not hear it (a
    /// cancelled turn's `turn.done` while the turn that replaced it runs).
    pub fn fact(&self, fact: FeedFact) -> Option<FeedFact> {
        let mut st = self.lock();
        match fact {
            FeedFact::TurnStarted {
                thread_id,
                by,
                voice,
            } => {
                let by = if st.started(thread_id, by.own && !voice) {
                    tracing::info!(thread_id, "a turn of the sidebar's runs");
                    By::sidebar()
                } else {
                    by
                };
                Some(FeedFact::TurnStarted {
                    thread_id,
                    by,
                    voice,
                })
            }
            FeedFact::TurnDone { thread_id } => {
                if st.done(thread_id) {
                    return Some(FeedFact::TurnDone { thread_id });
                }
                tracing::debug!(
                    thread_id,
                    "a cancelled turn's turn.done while the turn that replaced it runs"
                );
                None
            }
            FeedFact::Live { voice, turns } => Some(FeedFact::Live {
                voice,
                turns: st.live(turns),
            }),
            FeedFact::ApprovalsOpen(set) => {
                let now: HashMap<String, i64> = set
                    .iter()
                    .map(|r| (r.request.approval_request_id.clone(), r.thread_id))
                    .collect();
                let gone: Vec<i64> = st
                    .waiting
                    .iter()
                    .filter(|(id, _)| !now.contains_key(*id))
                    .map(|(_, t)| *t)
                    .collect();
                for t in gone {
                    st.threads.entry(t).or_default().decided = true;
                }
                st.waiting = now;
                Some(FeedFact::ApprovalsOpen(set))
            }
            FeedFact::ApprovalRequested(r) => {
                st.waiting
                    .insert(r.request.approval_request_id.clone(), r.thread_id);
                Some(FeedFact::ApprovalRequested(r))
            }
            FeedFact::ApprovalsWithdrawn(ids) => {
                for id in &ids {
                    if let Some(t) = st.waiting.remove(id) {
                        st.threads.entry(t).or_default().decided = true;
                    }
                }
                Some(FeedFact::ApprovalsWithdrawn(ids))
            }
            FeedFact::ThreadGone { thread_id } => {
                st.threads.remove(&thread_id);
                st.waiting.retain(|_, t| *t != thread_id);
                st.marks.retain(|m| m.thread != thread_id);
                Some(FeedFact::ThreadGone { thread_id })
            }
            other => Some(other),
        }
    }
}

impl State {
    /// Takes the oldest mark of `thread` for a `start`.
    fn take(&mut self, thread: i64, start: Start) -> bool {
        match self
            .marks
            .iter()
            .position(|m| m.thread == thread && m.start == start)
        {
            Some(i) => {
                self.marks.remove(i);
                true
            }
            None => false,
        }
    }

    /// Whether an own turn of `thread` that is no voice turn and begins now is the sidebar's
    /// (module doc).
    fn sidebars(&mut self, thread: i64) -> bool {
        if self.take(thread, Start::New) {
            return true;
        }
        let verdicts = self.take(thread, Start::Resume);
        let known = self.threads.get(&thread);
        let resumes = verdicts
            || known.is_some_and(|t| t.decided)
            || self.waiting.values().any(|t| *t == thread);
        if !resumes {
            return false;
        }
        let read = known.and_then(|t| t.last).unwrap_or(verdicts);
        tracing::info!(
            thread_id = thread,
            sidebar = read,
            "a resumed turn, read as the turn it resumes"
        );
        read
    }

    /// A `turn.started` of `thread`; `own_text`: an own turn that is no voice turn. Whether it is
    /// the sidebar's.
    fn started(&mut self, thread: i64, own_text: bool) -> bool {
        let sidebar = own_text && self.sidebars(thread);
        // The thread's next start was read: what a request that ended left unspent goes.
        self.marks.retain(|m| m.thread != thread || m.open);
        // The calls that waited were decided, or declined by a new message.
        self.waiting.retain(|_, t| *t != thread);
        let t = self.threads.entry(thread).or_default();
        t.running += 1;
        t.last = Some(sidebar);
        t.decided = false;
        sidebar
    }

    /// A `turn.done` of `thread`: whether it ends the thread's running turn, not a cancelled one
    /// (module doc).
    fn done(&mut self, thread: i64) -> bool {
        self.marks.retain(|m| m.thread != thread || m.open);
        let t = self.threads.entry(thread).or_default();
        let replaced = t.running > 1;
        t.running = t.running.saturating_sub(1);
        !replaced
    }

    /// A `hello`'s or `state`'s running turns (module doc).
    fn live(&mut self, turns: Vec<(i64, By, bool)>) -> Vec<(i64, By, bool)> {
        let seen: HashMap<i64, u32> = self.threads.iter().map(|(t, s)| (*t, s.running)).collect();
        let mut listed: HashMap<i64, u32> = HashMap::new();
        let turns = turns
            .into_iter()
            .map(|(t, by, voice)| {
                let n = listed.entry(t).or_default();
                *n += 1;
                let known = seen.get(&t).copied().unwrap_or(0) >= *n;
                let own_text = by.own && !voice;
                let sidebar = if known {
                    own_text && self.threads.get(&t).and_then(|s| s.last) == Some(true)
                } else {
                    let sidebar = own_text && self.sidebars(t);
                    let s = self.threads.entry(t).or_default();
                    s.last = Some(sidebar);
                    s.decided = false;
                    sidebar
                };
                if sidebar {
                    (t, By::sidebar(), voice)
                } else {
                    (t, by, voice)
                }
            })
            .collect();
        for (t, s) in &mut self.threads {
            s.running = listed.get(t).copied().unwrap_or(0);
        }
        for (t, n) in &listed {
            self.threads.entry(*t).or_default().running = *n;
        }
        // A request that ended, of a thread that runs nothing: its turn ended unread, or it had
        // none.
        self.marks
            .retain(|m| m.open || listed.contains_key(&m.thread));
        turns
    }
}

#[cfg(test)]
mod tests;
