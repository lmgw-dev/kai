//! Kai's asynchronous jobs (WP18, ruling 22, K26): a call of a tool of K26's open-ended class
//! (`Class::Async`: `run_command`, an `async = true` routine) that lmgw makes as an MCP task (MCP
//! 2025-11-25's Tasks, lmgw's MCP Tasks design T1) answers at once with the task, and the tool runs
//! on as a job. lmgw follows it (`tasks/get`, `tasks/result`, and the status notifications Kai sends
//! at once), and its result enters the call's Chat thread later; Kai speaks it at a quiet moment
//! (`crate::voice`'s `jobs` part) or says it in a notification.
//!
//! - **The table** ([`Jobs`]) lives with the host (`crate::host::server::Shared`), so a job outlives
//!   a link that drops (T7: lmgw polls again on the next link); Kai's quit cancels what still runs,
//!   and a restarted Kai knows none of them (lmgw then reads the job as abandoned: `-32602`).
//! - **A job's ids:** the short id, four hex digits unique among the jobs Kai knows, is the one
//!   to say (the model's line, `jobs`, the tray); the task id lmgw names in `started, job <id>` is
//!   the short id with this process's random part and a counter (`ab12-3f9c1e-7`), so a task id
//!   never repeats within a run of Kai nor, but by chance, across runs (an lmgw row of an earlier
//!   Kai gets `-32602`, never a new job's status). `job_cancel`, `job_announce` and lmgw's requests
//!   take either. With every short id taken (65536 jobs known) a new job is refused, visibly.
//! - **A job ends** when its tool's handler answers (an error reply is MCP's `failed`), when it is
//!   cancelled (`tasks/cancel` from lmgw, `job_cancel`, the tray's Cancel: `cancelled`, and the
//!   handler's future is stopped), when its routine is switched off or gone, when Kai's pairing
//!   ends (a revoked key, a pairing with another lmgw), or at Kai's quit.
//! - **An ended job is kept** until its delivery is confirmed: lmgw fetched its result
//!   (`tasks/result`) and the feed or the bound session said it entered its thread (`task.done`,
//!   which is when Kai reads its `announce`). A fetch alone confirms nothing (the answer may have
//!   gone down with a half-dead link), so a result no `task.done` will confirm (lmgw's bridge, Admin
//!   Chat's thread, a delivery Kai does not know) stays fetchable, and one whose `task.done` never
//!   comes stays too: each is dropped `jobs.keep_ended_s` after its end or its latest fetch (0:
//!   kept until Kai quits), and the tray and `jobs` say until when.
//! - **`announce`** (ruling 22): on for a job Kai's own voice session started (the caller is Kai
//!   itself), off for every other caller; an async routine's `announce` sets it; `job_announce`
//!   changes it while the job runs or waits.
//! - **Who acts on a job:** the owner, lmgw on the owner's behalf and Kai itself act on any; every
//!   other caller only on the jobs it started (`job_cancel`, `job_announce`).
//! - **Nothing runs unseen** (K26): the `jobs` tool lists the table, and the tray's Jobs submenu
//!   lists every job (`tray_entries`, with Cancel and the announce switch).

pub mod settings;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lmgw_client::mcp_host::{TaskDelivery, TaskMeta};
use rmcp::model::{CallToolResult, ContentBlock, Task, TaskStatus};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::host::policy::Caller;

/// How many short ids there are (four hex digits).
const SHORT_IDS: usize = 0x10000;

/// How a job's result is delivered (lmgw's `_meta["lmgw/task"]`, T15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// It enters a Chat thread when the job ends (`thread_id`; `None`: one Kai may not know, Admin
    /// Chat).
    Thread(Option<i64>),
    /// The caller waits for it now (lmgw's bridge: `/mcp`, a temporary thread).
    Wait,
    /// lmgw said nothing Kai knows.
    Unknown,
}

impl Delivery {
    pub fn of(meta: Option<&TaskMeta>) -> Self {
        match meta {
            Some(TaskMeta {
                delivery: TaskDelivery::Thread,
                thread_id,
            }) => Delivery::Thread(*thread_id),
            Some(TaskMeta {
                delivery: TaskDelivery::Wait,
                ..
            }) => Delivery::Wait,
            _ => Delivery::Unknown,
        }
    }

    fn thread(self) -> Option<i64> {
        match self {
            Delivery::Thread(t) => t,
            _ => None,
        }
    }

    /// Whether a `task.done` will ever say the result entered a thread Kai sees: the only
    /// confirmation that lmgw got it.
    fn said_later(self) -> bool {
        matches!(self, Delivery::Thread(Some(_)))
    }
}

/// What a new job is.
#[derive(Debug, Clone)]
pub struct Start {
    pub tool: &'static str,
    /// The label lmgw lists Kai's tools under, at the call (`hello.hosts_label`): the feed's
    /// `server_label` of its `task.done`. `None` (the feed's `hello` had not come): the table's
    /// label when the `task.done` comes ([`Jobs::set_label`]).
    pub label: Option<String>,
    /// Who called (K15's caller): `announce`'s default, who may act on the job, and its words.
    pub by: Caller,
    pub delivery: Delivery,
    /// A routine's run (the `routines` area): it ends when the routine is switched off or gone.
    pub routine: bool,
}

/// A job as it started.
#[derive(Debug, Clone)]
pub struct Started {
    /// The short id, the one to say.
    pub id: String,
    /// The task id on the wire.
    pub wire: String,
    pub task: Task,
    /// Cancelled when the job ends.
    pub cancel: CancellationToken,
}

/// How a job ended.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The handler answered: a result, `failed` when it is an error.
    Answered(CallToolResult),
    /// Cancelled, by whom (`lmgw`, the tool's caller, the tray).
    Cancelled(String),
}

#[derive(Debug, Clone)]
enum State {
    Running,
    Ended {
        status: TaskStatus,
        result: CallToolResult,
        /// lmgw fetched it (`tasks/result`), at least once.
        fetched: bool,
        /// `task.done` said it entered its thread.
        said: bool,
    },
}

/// Until when an ended job is kept (`jobs.keep_ended_s`).
#[derive(Debug, Clone, Copy)]
struct Kept {
    at: tokio::time::Instant,
    wall: SystemTime,
}

#[derive(Debug, Clone)]
struct Job {
    id: String,
    wire: String,
    start: Start,
    announce: bool,
    created: SystemTime,
    updated: SystemTime,
    state: State,
    cancel: CancellationToken,
    /// Ended: dropped then (`None`: kept until Kai quits).
    kept: Option<Kept>,
}

impl Job {
    fn is(&self, key: &str) -> bool {
        self.id == key || self.wire == key
    }

    fn status(&self) -> TaskStatus {
        match &self.state {
            State::Running => TaskStatus::Working,
            State::Ended { status, .. } => status.clone(),
        }
    }

    fn task(&self) -> Task {
        let mut t = Task::new(
            self.wire.clone(),
            self.status(),
            iso(self.created),
            iso(self.updated),
        );
        // No ttl (null: Kai keeps the result as the module doc says, T9) and no poll interval of
        // Kai's own: Kai sends a status notification at every change, and lmgw's
        // `mcp.task_poll_interval_s` covers a notification lost with a link.
        if let State::Ended { status, .. } = &self.state {
            t.status_message = Some(status_word(status).to_owned());
        }
        t
    }

    fn view(&self) -> JobView {
        JobView {
            id: self.id.clone(),
            wire: self.wire.clone(),
            tool: self.start.tool,
            caller: self.start.by.describe(),
            by: self.start.by.clone(),
            announce: self.announce,
            started: self.created,
            status: status_word(&self.status()),
            running: matches!(self.state, State::Running),
            fetched: matches!(self.state, State::Ended { fetched: true, .. }),
            kept_until: self.kept.map(|k| k.wall),
            delivery: self.start.delivery,
        }
    }

    /// Ended, and lmgw confirmed it has the result: fetched, and `task.done` said it entered its
    /// thread.
    fn done_with(&self) -> bool {
        self.start.delivery.said_later()
            && matches!(
                self.state,
                State::Ended {
                    fetched: true,
                    said: true,
                    ..
                }
            )
    }
}

/// MCP's status in words.
pub fn status_word(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Working => "running",
        TaskStatus::InputRequired => "waiting for input",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        _ => "unknown",
    }
}

/// One job as the `jobs` tool and the tray show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobView {
    pub id: String,
    pub wire: String,
    pub tool: &'static str,
    /// Who started it, in words.
    pub caller: String,
    pub by: Caller,
    pub announce: bool,
    pub started: SystemTime,
    pub status: &'static str,
    pub running: bool,
    /// lmgw fetched its result.
    pub fetched: bool,
    /// Ended: dropped then (`None` while it runs, or kept until Kai quits).
    pub kept_until: Option<SystemTime>,
    pub delivery: Delivery,
}

/// Why a job could not be started, cancelled or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Kai knows no job of that id (never, or it ended and was dropped).
    Unknown,
    /// It has ended; its status.
    Ended(&'static str),
    /// Every short id is taken (Kai knows 65536 jobs).
    Full,
    /// The caller is not the owner's and did not start it; who did.
    NotYours(String),
}

impl Refusal {
    pub fn words(&self, id: &str) -> String {
        match self {
            Refusal::Unknown => format!("Kai has no job {id}"),
            Refusal::Ended(status) => format!("job {id} has already ended ({status})"),
            Refusal::Full => format!(
                "Kai knows {SHORT_IDS} jobs, so every job id is taken; cancel running ones (the \
                 jobs tool, the tray's Jobs) or wait until ended ones are dropped \
                 (jobs.keep_ended_s)"
            ),
            Refusal::NotYours(by) => {
                format!("job {id} was started by {by}; only the owner or its starter may act on it")
            }
        }
    }
}

/// Whether `actor` may act on a job `starter` started (module doc): the owner's callers on any,
/// a named caller on its own; a caller lmgw did not name is nobody's, so on none.
pub fn may_act(actor: &Caller, starter: &Caller) -> bool {
    match actor {
        Caller::Owner | Caller::Gateway | Caller::Own => true,
        Caller::Named(_) => actor == starter,
        Caller::Unknown(_) => false,
    }
}

/// Sends a job's status to lmgw (`notifications/tasks/status`), when a link is up: the sending,
/// for [`Jobs::sent`].
pub type Notifier = Arc<dyn Fn(Task) -> Option<JoinHandle<()>> + Send + Sync>;

/// `jobs.keep_ended_s`'s default (`settings`).
pub const KEEP_ENDED_DEFAULT: Duration = Duration::from_secs(86_400);

struct Inner {
    jobs: Mutex<Vec<Job>>,
    changed: watch::Sender<u64>,
    notifier: Mutex<Option<Notifier>>,
    sending: Mutex<Vec<JoinHandle<()>>>,
    /// `jobs.keep_ended_s` (zero: until Kai quits).
    keep: Mutex<Duration>,
    /// The label lmgw lists Kai's tools under now (`hello.hosts_label`).
    label: Mutex<Option<String>>,
}

/// The job table (module doc). Cloning shares it.
#[derive(Clone)]
pub struct Jobs(Arc<Inner>);

impl Default for Jobs {
    fn default() -> Self {
        Self(Arc::new(Inner {
            jobs: Mutex::new(Vec::new()),
            changed: watch::channel(0).0,
            notifier: Mutex::new(None),
            sending: Mutex::new(Vec::new()),
            keep: Mutex::new(KEEP_ENDED_DEFAULT),
            label: Mutex::new(None),
        }))
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// This process's part of every task id (module doc): six random hex digits.
static PROCESS: LazyLock<String> = LazyLock::new(|| format!("{:06x}", random() & 0xff_ffff));
/// The counter of task ids in this process.
static SERIAL: AtomicU64 = AtomicU64::new(0);

fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.write_u32(std::process::id());
    h.finish()
}

impl Jobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Job>> {
        locked(&self.0.jobs)
    }

    fn touched(&self, task: Option<Task>) {
        self.0.changed.send_modify(|n| *n += 1);
        publish(self);
        if let Some(t) = task {
            let notifier = locked(&self.0.notifier).clone();
            if let Some(handle) = notifier.and_then(|n| n(t)) {
                let mut sending = locked(&self.0.sending);
                sending.retain(|h| !h.is_finished());
                sending.push(handle);
            }
        }
    }

    /// Where status changes go (the host sets it).
    pub fn set_notifier(&self, n: Notifier) {
        *locked(&self.0.notifier) = Some(n);
    }

    /// `jobs.keep_ended_s` (zero: until Kai quits); applies to what ends or is fetched from now.
    pub fn set_keep(&self, keep: Duration) {
        *locked(&self.0.keep) = keep;
    }

    pub fn keep(&self) -> Duration {
        *locked(&self.0.keep)
    }

    /// The label lmgw lists Kai's tools under now (the feed's `hello`).
    pub fn set_label(&self, label: Option<String>) {
        *locked(&self.0.label) = label;
    }

    /// Changes of the table (the tray, the tests).
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    /// A new running job. Refused with every short id taken.
    pub fn start(&self, start: Start) -> Result<Started, Refusal> {
        let cancel = CancellationToken::new();
        let now = SystemTime::now();
        let mut jobs = self.lock();
        let Some(id) = fresh_id(&jobs) else {
            tracing::warn!(
                known = jobs.len(),
                tool = start.tool,
                "jobs: refused: every short id is taken"
            );
            return Err(Refusal::Full);
        };
        let wire = format!(
            "{id}-{}-{}",
            *PROCESS,
            SERIAL.fetch_add(1, Ordering::Relaxed) + 1
        );
        let job = Job {
            id: id.clone(),
            wire: wire.clone(),
            announce: start.by == Caller::Own,
            start,
            created: now,
            updated: now,
            state: State::Running,
            cancel: cancel.clone(),
            kept: None,
        };
        tracing::info!(
            job = %id,
            task = %wire,
            tool = job.start.tool,
            caller = %job.start.by.describe(),
            announce = job.announce,
            delivery = ?job.start.delivery,
            "jobs: started"
        );
        let task = job.task();
        jobs.push(job);
        drop(jobs);
        self.touched(None);
        Ok(Started {
            id,
            wire,
            task,
            cancel,
        })
    }

    /// Sets `key`'s announce (`job_announce`, an async routine's own `announce`, the tray).
    pub fn set_announce(&self, key: &str, on: bool) -> Result<(), Refusal> {
        let mut jobs = self.lock();
        let job = jobs
            .iter_mut()
            .find(|j| j.is(key))
            .ok_or(Refusal::Unknown)?;
        if job.announce == on {
            return Ok(());
        }
        job.announce = on;
        tracing::info!(job = %job.id, on, "jobs: announce");
        drop(jobs);
        self.touched(None);
        Ok(())
    }

    /// Arms `job`'s retention from now (`jobs.keep_ended_s`).
    fn keep_from_now(&self, job: &mut Job) {
        let keep = self.keep();
        if keep.is_zero() {
            job.kept = None;
            return;
        }
        let at = tokio::time::Instant::now() + keep;
        job.kept = Some(Kept {
            at,
            wall: SystemTime::now() + keep,
        });
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let table = Arc::downgrade(&self.0);
            rt.spawn(async move {
                tokio::time::sleep_until(at).await;
                if let Some(inner) = Weak::upgrade(&table) {
                    Jobs(inner).prune();
                }
            });
        }
    }

    /// Drops the ended jobs whose retention passed (module doc).
    pub fn prune(&self) {
        let now = tokio::time::Instant::now();
        let mut jobs = self.lock();
        let before = jobs.len();
        jobs.retain(|j| {
            let gone =
                matches!(j.state, State::Ended { .. }) && j.kept.is_some_and(|k| k.at <= now);
            if gone {
                tracing::info!(
                    job = %j.id,
                    fetched = matches!(j.state, State::Ended { fetched: true, .. }),
                    "jobs: dropped after jobs.keep_ended_s with no confirmed delivery"
                );
            }
            !gone
        });
        let pruned = jobs.len() != before;
        drop(jobs);
        if pruned {
            self.touched(None);
        }
    }

    /// `key` ended (its runner). A job that ended already (a cancel came first) stays as it ended.
    pub fn end(&self, key: &str, outcome: Outcome) {
        let mut jobs = self.lock();
        let Some(i) = jobs.iter().position(|j| j.is(key)) else {
            return;
        };
        if !matches!(jobs[i].state, State::Running) {
            return;
        }
        let (status, result) = match outcome {
            Outcome::Answered(r) if r.is_error == Some(true) => (TaskStatus::Failed, r),
            Outcome::Answered(r) => (TaskStatus::Completed, r),
            Outcome::Cancelled(by) => (
                TaskStatus::Cancelled,
                CallToolResult::error(vec![ContentBlock::text(format!(
                    "the job was cancelled by {by}"
                ))]),
            ),
        };
        let mut job = jobs[i].clone();
        tracing::info!(
            job = %job.id,
            tool = job.start.tool,
            status = status_word(&status),
            secs = job.created.elapsed().unwrap_or_default().as_secs(),
            "jobs: ended"
        );
        job.cancel.cancel();
        job.updated = SystemTime::now();
        job.state = State::Ended {
            status,
            result,
            fetched: false,
            said: false,
        };
        self.keep_from_now(&mut job);
        let task = job.task();
        jobs[i] = job;
        drop(jobs);
        self.touched(Some(task));
    }

    /// Cancels `key` (`tasks/cancel`, `job_cancel`, the tray): it ends `cancelled` at once and its
    /// handler is stopped. The task as it is now.
    pub fn cancel(&self, key: &str, by: &str) -> Result<Task, Refusal> {
        self.cancel_as(key, by, &Caller::Owner)
    }

    /// [`Self::cancel`] for `actor`, who may act only on what [`may_act`] lets it.
    pub fn cancel_as(&self, key: &str, by: &str, actor: &Caller) -> Result<Task, Refusal> {
        {
            let jobs = self.lock();
            let job = jobs.iter().find(|j| j.is(key)).ok_or(Refusal::Unknown)?;
            if !may_act(actor, &job.start.by) {
                return Err(Refusal::NotYours(job.start.by.describe()));
            }
            if let State::Ended { status, .. } = &job.state {
                return Err(Refusal::Ended(status_word(status)));
            }
        }
        self.end(key, Outcome::Cancelled(by.to_owned()));
        self.task(key).ok_or(Refusal::Unknown)
    }

    /// [`Self::set_announce`] for `actor` ([`may_act`]).
    pub fn set_announce_as(&self, key: &str, on: bool, actor: &Caller) -> Result<(), Refusal> {
        {
            let jobs = self.lock();
            let job = jobs.iter().find(|j| j.is(key)).ok_or(Refusal::Unknown)?;
            if !may_act(actor, &job.start.by) {
                return Err(Refusal::NotYours(job.start.by.describe()));
            }
        }
        self.set_announce(key, on)
    }

    /// One job (`job_cancel`'s and `job_announce`'s dialog).
    pub fn view(&self, key: &str) -> Option<JobView> {
        self.lock().iter().find(|j| j.is(key)).map(Job::view)
    }

    /// `tasks/get`.
    pub fn task(&self, key: &str) -> Option<Task> {
        self.lock().iter().find(|j| j.is(key)).map(Job::task)
    }

    /// Waits until `key` ended (`false`: Kai does not know it, or it was dropped meanwhile).
    pub async fn ended(&self, key: &str) -> bool {
        let mut changes = self.changes();
        loop {
            match self.lock().iter().find(|j| j.is(key)) {
                None => return false,
                Some(j) if matches!(j.state, State::Ended { .. }) => return true,
                Some(_) => {}
            }
            if changes.changed().await.is_err() {
                return false;
            }
        }
    }

    /// `tasks/result`, once [`Self::ended`]: the result, `None` for a job Kai does not know or
    /// one still running. The job stays until its delivery is confirmed (module doc); each fetch
    /// keeps it `jobs.keep_ended_s` longer.
    pub fn fetch(&self, key: &str) -> Option<CallToolResult> {
        let mut jobs = self.lock();
        let i = jobs.iter().position(|j| j.is(key))?;
        let mut job = jobs[i].clone();
        let State::Ended {
            result, fetched, ..
        } = &mut job.state
        else {
            return None;
        };
        *fetched = true;
        let r = result.clone();
        if job.done_with() {
            tracing::info!(job = %job.id, "jobs: lmgw has the result and it entered its thread");
            jobs.remove(i);
        } else {
            self.keep_from_now(&mut job);
            jobs[i] = job;
        }
        drop(jobs);
        self.touched(None);
        Some(r)
    }

    /// [`Self::ended`], then [`Self::fetch`].
    #[cfg(test)]
    pub async fn result(&self, key: &str) -> Option<CallToolResult> {
        if !self.ended(key).await {
            return None;
        }
        self.fetch(key)
    }

    /// A `task.done` (the feed's, or the bound session's) for `task_id` under `label`: whether Kai
    /// is to speak it, when it is one of Kai's jobs (`None`: not Kai's, or one Kai no longer
    /// knows). A job whose label was not known at its call is read with the label now. The job is
    /// forgotten once lmgw fetched its result too.
    pub fn said(&self, label: &str, task_id: &str) -> Option<bool> {
        let now_label = locked(&self.0.label).clone();
        let mut jobs = self.lock();
        let i = jobs.iter().position(|j| {
            j.wire == task_id && j.start.label.as_deref().or(now_label.as_deref()) == Some(label)
        })?;
        let announce = jobs[i].announce;
        if let State::Ended { said, .. } = &mut jobs[i].state {
            *said = true;
        }
        if jobs[i].done_with() {
            tracing::info!(job = %jobs[i].id, "jobs: lmgw has the result and it entered its thread");
            jobs.remove(i);
        }
        drop(jobs);
        self.touched(None);
        Some(announce)
    }

    /// Every job Kai knows, oldest first.
    pub fn list(&self) -> Vec<JobView> {
        self.lock().iter().map(Job::view).collect()
    }

    /// Kai quits: every running job is cancelled (its handler stopped; a command's scope ends).
    pub fn quit(&self) {
        self.cancel_all("Kai's quit");
    }

    /// Every running job is cancelled by `by` (Kai's quit, the pairing's end).
    pub fn cancel_all(&self, by: &str) {
        let running: Vec<String> = self
            .lock()
            .iter()
            .filter(|j| matches!(j.state, State::Running))
            .map(|j| j.id.clone())
            .collect();
        for id in running {
            self.end(&id, Outcome::Cancelled(by.to_owned()));
        }
    }

    /// The running jobs of a routine no longer `offered` (switched off, its folder gone) are
    /// cancelled.
    pub fn routines_gone(&self, offered: &HashSet<&str>) {
        let gone: Vec<(String, &'static str)> = self
            .lock()
            .iter()
            .filter(|j| {
                j.start.routine
                    && matches!(j.state, State::Running)
                    && !offered.contains(j.start.tool)
            })
            .map(|j| (j.id.clone(), j.start.tool))
            .collect();
        for (id, tool) in gone {
            tracing::info!(job = %id, routine = tool, "jobs: its routine is off or gone");
            self.end(
                &id,
                Outcome::Cancelled(format!(
                    "Kai: the routine {tool} was switched off or its folder is gone"
                )),
            );
        }
    }

    /// The device `name` was deleted in lmgw (the feed's `device.revoked`): every running job it
    /// started is cancelled, as lmgw's cancel would have; a job any other caller started, a
    /// finished one, and a device with no jobs are left as they are. How many were cancelled.
    pub fn device_revoked(&self, name: &str) -> usize {
        let gone: Vec<(String, &'static str)> = self
            .lock()
            .iter()
            .filter(|j| {
                matches!(j.state, State::Running)
                    && matches!(&j.start.by, Caller::Named(n) if n.kind == "device" && n.name == name)
            })
            .map(|j| (j.id.clone(), j.start.tool))
            .collect();
        for (id, tool) in &gone {
            tracing::info!(job = %id, tool, device = name, "jobs: the device that started it was deleted");
            self.end(
                id,
                Outcome::Cancelled(format!(
                    "Kai: the device '{name}' that started it was deleted"
                )),
            );
        }
        gone.len()
    }

    /// The status notifications on their way go out, all within `bound` (Kai's quit: lmgw sees
    /// the jobs cancelled before the link closes).
    pub async fn sent(&self, bound: Duration) {
        let sending = std::mem::take(&mut *locked(&self.0.sending));
        if sending.is_empty() {
            return;
        }
        let n = sending.len();
        if tokio::time::timeout(bound, futures_util::future::join_all(sending))
            .await
            .is_err()
        {
            tracing::info!(
                n,
                "jobs: the status notifications did not all go out within lmgw.timeout_s; lmgw \
                 reads those jobs as abandoned"
            );
        }
    }

    /// The thread `key`'s result enters (for the tests and the log).
    #[allow(dead_code)]
    pub fn thread_of(&self, key: &str) -> Option<i64> {
        self.lock()
            .iter()
            .find(|j| j.is(key))
            .and_then(|j| j.start.delivery.thread())
    }
}

/// Four hex digits no job Kai knows has (module doc); `None` with every one taken. One pass over
/// the table and at most one over the ids, from a random start.
fn fresh_id(jobs: &[Job]) -> Option<String> {
    if jobs.len() >= SHORT_IDS {
        return None;
    }
    let taken: HashSet<&str> = jobs.iter().map(|j| j.id.as_str()).collect();
    let from = random() as usize;
    (0..SHORT_IDS)
        .map(|i| format!("{:04x}", (from + i) % SHORT_IDS))
        .find(|id| !taken.contains(id.as_str()))
}

/// ISO 8601 in UTC, as MCP's `createdAt` and `lastUpdatedAt` carry it.
pub fn iso(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

// ------------------------------------------------------------- the tray (K26: nothing unseen)

/// One job in the tray's Jobs submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayJob {
    pub id: String,
    pub label: String,
    pub running: bool,
    pub announce: bool,
}

static TRAY: LazyLock<watch::Sender<Vec<TrayJob>>> = LazyLock::new(|| watch::channel(Vec::new()).0);
static PUBLISHED: Mutex<Option<Weak<Inner>>> = Mutex::new(None);

impl Jobs {
    /// This table is the one the tray shows and acts on (`main`'s).
    pub fn publish(&self) {
        *locked(&PUBLISHED) = Some(Arc::downgrade(&self.0));
        publish(self);
    }
}

fn published() -> Option<Jobs> {
    locked(&PUBLISHED)
        .as_ref()
        .and_then(Weak::upgrade)
        .map(Jobs)
}

fn publish(jobs: &Jobs) {
    let ours = published().is_some_and(|p| Arc::ptr_eq(&p.0, &jobs.0));
    if !ours {
        return;
    }
    let entries = jobs
        .list()
        .into_iter()
        .map(|v| TrayJob {
            label: tray_label(&v),
            id: v.id,
            running: v.running,
            announce: v.announce,
        })
        .collect();
    TRAY.send_if_modified(|old| {
        let changed = *old != entries;
        *old = entries;
        changed
    });
}

/// Where an ended job's result is, and until when Kai keeps it (`jobs.keep_ended_s`).
fn ended_words(v: &JobView) -> String {
    let where_ = if v.fetched {
        "lmgw has its result"
    } else {
        "its result waits for lmgw"
    };
    let until = match v.kept_until {
        Some(t) => format!("kept until {} (jobs.keep_ended_s)", local_hm(t)),
        None => "kept until Kai quits (jobs.keep_ended_s = 0)".to_owned(),
    };
    format!("{}, {where_}; {until}", v.status)
}

fn tray_label(v: &JobView) -> String {
    let state = if v.running {
        format!("since {}", local_hm(v.started))
    } else {
        ended_words(v)
    };
    format!("{} · job {} · {state} · {}", v.tool, v.id, v.caller)
}

/// `HH:MM` on the local clock (libc's localtime_r).
fn local_hm(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which lives on this stack frame.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if ok {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        iso(t)
    }
}

/// The tray's view of the published table's jobs.
pub fn tray_entries() -> watch::Receiver<Vec<TrayJob>> {
    TRAY.subscribe()
}

/// The tray's Cancel (the owner's own).
pub fn tray_cancel(id: &str) {
    match published().map(|j| j.cancel(id, "the owner (the tray)")) {
        Some(Ok(_)) => {}
        Some(Err(r)) => tracing::info!("jobs: the tray's cancel: {}", r.words(id)),
        None => tracing::warn!("jobs: no job table yet; the tray's cancel is dropped"),
    }
}

/// The tray's announce switch.
pub fn tray_announce(id: &str, on: bool) {
    if let Some(Err(r)) = published().map(|j| j.set_announce(id, on)) {
        tracing::info!("jobs: the tray's announce: {}", r.words(id));
    }
}

/// The `jobs` tool's answer: one line per job.
pub fn listing(jobs: &Jobs) -> String {
    let list = jobs.list();
    if list.is_empty() {
        return "No jobs: nothing runs.".to_owned();
    }
    list.iter()
        .map(|v| {
            let state = if v.running {
                format!("running since {}", local_hm(v.started))
            } else {
                ended_words(v)
            };
            let speak = if v.announce {
                "its result is spoken"
            } else {
                "its result is notified"
            };
            format!(
                "job {} ({}): {state}; started by {}; {speak}",
                v.id, v.tool, v.caller
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------- the running job, for its handler

tokio::task_local! {
    static CURRENT: (Jobs, String);
}

/// Runs `fut` (a job's handler) as job `id`, so the handler can reach its job ([`set_announce`]).
pub async fn scope<F: std::future::Future>(jobs: Jobs, id: String, fut: F) -> F::Output {
    CURRENT.scope((jobs, id), fut).await
}

/// Sets the announce of the job the calling handler runs as (an async routine's own `announce`);
/// nothing when it runs as no job (a synchronous call: Kai's own CLI, a delegate's step).
pub fn set_announce(on: bool) {
    let _ = CURRENT.try_with(|(jobs, id)| jobs.set_announce(id, on));
}
