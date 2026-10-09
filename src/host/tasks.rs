//! K26's open-ended class over MCP 2025-11-25's Tasks (WP18; lmgw's MCP Tasks design T1, T3,
//! T7): Kai declares `capabilities.tasks {cancel, requests.tools.call}` and
//! `execution.taskSupport: "required"` on every `Class::Async` tool (`super::server::render`), and
//! answers lmgw's task-augmented `tools/call` with the task once the call is admitted.
//!
//! **Admission** is the one path every call takes (`Shared::dispatch`: K15's policy, the tool's
//! preparation, the owner's confirmation, the screen lock), bounded by lmgw's `lmgw/timeout_ms`,
//! which on a task-augmented call bounds the wait for the task (T7's contract table). A call
//! refused or cancelled before it is admitted is the request's JSON-RPC error, as lmgw shows a
//! normal call's error; nothing runs and no job exists. Once admitted, the handler runs as a job
//! of [`crate::jobs::Jobs`] (with no deadline: lmgw follows the task as long as it runs, T9) and
//! `tools/call` is answered with `CreateTaskResult` and the model's line
//! (`io.modelcontextprotocol/model-immediate-response`: where the result goes). The handler is
//! polled once before that line is written, so an async routine's own `announce` (set at its
//! run's start) is the one the line states.
//!
//! **The result's room:** a job's call gets the room its text has in the answer to `tasks/result`
//! under the link's limits at its admission (`LinkLimits::task_text_room`), as a synchronous call
//! gets the room of its own answer: `run_command` keeps its output's start and end within it and
//! says what it cut, a script routine keeps no more than it (its larger result is its error naming
//! the room), and nothing a job keeps grows past the message limit. A link whose limit shrank
//! since still answers an oversized result with the error naming the limit.
//!
//! **Following** is lmgw's: `tasks/get`, `tasks/result` (held until the job ended), `tasks/cancel`;
//! Kai also sends `notifications/tasks/status` at the job's end, so lmgw need not wait for its next
//! poll. A job Kai does not know (a restarted Kai) is `-32602`, which lmgw reads as abandoned.
//!
//! **A running job and the screen lock** (a draft of 2026-10-09): a job admitted while the screen
//! was unlocked runs on when it locks (a long command the owner left running); every call it makes
//! through Kai (a routine's `kai tool`) is refused while locked as any call is. No job delivers
//! while locked: `tasks/result` and the status notifications are held until the unlock, for every
//! caller (`tasks/get` answers as it is, so lmgw may see `completed` and still wait for the
//! result).

use std::sync::{Arc, Mutex, Weak};

use rmcp::model::{
    CallToolRequestParams, CancelTaskResult, CreateTaskResult, ErrorCode, GetTaskPayloadResult,
    GetTaskResult, JsonObject, Meta, Notification, ServerNotification, TaskRequestsCapability,
    TaskStatusNotificationParam, TasksCapability, ToolsTaskCapability,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::registry::{Call, Entry, Reply};
use super::server::{Bounds, Server, Shared, Stop};
use crate::jobs::{Delivery, Jobs, Outcome, Start};

/// The JSON-RPC code of a task-augmented call refused before it ran (K15, the owner, the tool's
/// own check, the screen lock): an application error, its message what the model reads.
const REFUSED: ErrorCode = ErrorCode(-32001);

/// MCP's key for a task's immediate words to the model (lmgw's T5 puts them under its line).
const IMMEDIATE: &str = "io.modelcontextprotocol/model-immediate-response";

/// What Kai declares as a receiver (lmgw's contract table: `cancel` and `requests.tools.call`; no
/// `list`, which lmgw does not use).
pub fn capability() -> TasksCapability {
    let mut c = TasksCapability::default();
    c.cancel = Some(JsonObject::new());
    let mut requests = TaskRequestsCapability::default();
    let mut tools = ToolsTaskCapability::default();
    tools.call = Some(JsonObject::new());
    requests.tools = Some(tools);
    c.requests = Some(requests);
    c
}

/// Sends each job's status change to lmgw over the current link (none while it is down: lmgw
/// polls again on the next link, T7), after the unlock while the screen is locked.
pub fn notifier(shared: Weak<Shared>) -> crate::jobs::Notifier {
    Arc::new(move |task| {
        let rt = tokio::runtime::Handle::try_current().ok()?;
        let shared = shared.clone();
        Some(rt.spawn(async move {
            let Some(screen) = shared.upgrade().map(|s| s.screen.clone()) else {
                return;
            };
            if screen.is_locked() {
                tracing::info!(job = %task.task_id, "host: a task's status waits for the unlock");
            }
            screen.unlocked().await;
            let Some(peer) = shared
                .upgrade()
                .and_then(|s| s.peer.lock().expect("never poisoned").get())
            else {
                return;
            };
            let id = task.task_id.clone();
            let n = ServerNotification::TaskStatusNotification(Notification::new(
                TaskStatusNotificationParam::new(task),
            ));
            if let Err(e) = peer.send_notification(n).await {
                tracing::debug!(job = %id, "host: a task status did not go out: {e}");
            }
        }))
    })
}

/// A task-augmented `tools/call` (module doc).
pub async fn enqueue(
    server: &Server,
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
) -> Result<CreateTaskResult, McpError> {
    let a = match server.arrive(request, &context) {
        Ok(a) => a,
        Err(Ok(refusal)) => return Err(McpError::new(REFUSED, text_of(&refusal), None)),
        Err(Err(e)) => return Err(e),
    };
    let shared = server.shared.clone();
    let spec = a.entry.spec;
    let tool = spec.name;
    // Admission: the dispatch runs to the handler, which hands the admitted call over here.
    let (admitted_tx, admitted) = oneshot::channel::<Call>();
    let admitted_tx = Arc::new(Mutex::new(Some(admitted_tx)));
    let original = a.entry.clone();
    let gate = Entry {
        handler: Arc::new(move |call| {
            let tx = admitted_tx.lock().unwrap_or_else(|e| e.into_inner()).take();
            Box::pin(async move {
                if let Some(tx) = tx {
                    let _ = tx.send(call);
                }
                Reply::text("admitted; it runs as a job")
            })
        }),
        ..a.entry.clone()
    };
    let admission_ct = CancellationToken::new();
    let limits = a.limits;
    let bounds = Bounds {
        cancel: admission_ct.clone(),
        deadline: a.deadline,
        timeout_ms: a.meta.timeout_ms,
        room: None,
        room_says: limits.message_says(),
        out: None,
    };
    let caller = a.caller.clone();
    let approver = a.approver.clone();
    let args = a.args;
    let dispatching = {
        let shared = shared.clone();
        tokio::spawn(async move {
            shared
                .dispatch(&gate, args, &caller, approver.as_ref(), bounds)
                .await
        })
    };
    let call = tokio::select! {
        c = admitted => c,
        () = context.ct.cancelled() => {
            admission_ct.cancel();
            tracing::info!(tool, "host: lmgw cancelled the task's call before it was admitted");
            return Err(McpError::new(
                ErrorCode(-32800),
                format!("the call of {tool} was cancelled"),
                None,
            ));
        }
    };
    let Ok(mut call) = call else {
        // Not admitted: the dispatch's own answer says why.
        return Err(match dispatching.await {
            Ok(Err(Stop::Refused(why))) => McpError::new(REFUSED, why, None),
            Ok(Err(Stop::Cancelled(_))) => McpError::new(
                ErrorCode(-32800),
                format!("the call of {tool} was cancelled"),
                None,
            ),
            Ok(Ok(_)) | Err(_) => {
                McpError::internal_error(format!("Kai could not start {tool} as a job"), None)
            }
        });
    };
    let label = shared.label.read().expect("never poisoned").clone();
    let delivery = Delivery::of(a.meta.task.as_ref());
    let jobs = shared.jobs.clone();
    let started = match jobs.start(Start {
        tool,
        label,
        by: a.caller.clone(),
        delivery,
        routine: original.area == kai_desktop::contract::routines::AREA.name,
    }) {
        Ok(s) => s,
        Err(refusal) => {
            // Admitted, and not run: no job can be told apart.
            call.cancel.cancel();
            return Err(McpError::new(REFUSED, refusal.words("for this call"), None));
        }
    };
    let (id, cancel) = (started.id.clone(), started.cancel.clone());
    // The job runs open-ended: no deadline of lmgw's, its own cancellation, and the room its text
    // has in `tasks/result`'s answer (module doc).
    let limits = *server.limits.read().expect("never poisoned");
    call.cancel = cancel.clone();
    call.deadline = None;
    call.timeout_ms = None;
    call.room = limits.task_text_room(&started.wire);
    call.room_says = limits.message_says();
    let mut handler: Running = Box::pin(crate::jobs::scope(
        jobs.clone(),
        id.clone(),
        (original.handler)(call),
    ));
    // One poll before the model's line: an async routine sets its own announce first thing.
    match futures_util::FutureExt::now_or_never(&mut handler) {
        Some(reply) => {
            tracing::info!(job = %id, tool, error = reply.error, "host: the job answered at once");
            jobs.end(&id, Outcome::Answered(super::server::result(reply)));
        }
        None => {
            tokio::spawn(run(jobs.clone(), id.clone(), tool, handler, cancel));
        }
    }
    let words = immediate(&jobs, &id, tool, delivery);
    let mut meta = Meta::new();
    meta.insert(IMMEDIATE.into(), serde_json::Value::String(words));
    Ok(CreateTaskResult::new(started.task).with_meta(meta))
}

/// A job's handler, in its job's scope.
type Running = std::pin::Pin<Box<dyn std::future::Future<Output = super::registry::Reply> + Send>>;

/// Runs the admitted call's handler as job `id` until it answers or the job is cancelled.
async fn run(
    jobs: Jobs,
    id: String,
    tool: &'static str,
    handler: Running,
    cancel: CancellationToken,
) {
    tokio::select! {
        reply = handler => {
            tracing::info!(job = %id, tool, error = reply.error, "host: the job answered");
            jobs.end(&id, Outcome::Answered(super::server::result(reply)));
        }
        // `Jobs::cancel` and the quit end the job themselves.
        () = cancel.cancelled() => tracing::info!(job = %id, tool, "host: the job was stopped"),
    }
}

/// The model's line beside `started, job <id>`: where the result goes.
fn immediate(jobs: &Jobs, id: &str, tool: &str, delivery: Delivery) -> String {
    let announce = jobs
        .list()
        .into_iter()
        .find(|j| j.id == id)
        .is_some_and(|j| j.announce);
    match delivery {
        Delivery::Thread(_) if announce => format!(
            "{tool} runs as job {id}; its result comes into this chat when it is done, and Kai \
             speaks it then: there is nothing to wait for"
        ),
        Delivery::Thread(_) => format!(
            "{tool} runs as job {id}; its result comes into this chat when it is done, and Kai \
             notifies the owner: there is nothing to wait for"
        ),
        _ => format!("{tool} runs as job {id}"),
    }
}

/// `tasks/get`.
pub fn get(shared: &Shared, id: &str) -> Result<GetTaskResult, McpError> {
    shared
        .jobs
        .task(id)
        .map(GetTaskResult::new)
        .ok_or_else(|| unknown(id))
}

/// `tasks/result`: held until the job ended and the screen is unlocked (module doc); within the
/// link's message limit as any result.
pub async fn result(
    server: &Server,
    id: &str,
    context: &RequestContext<RoleServer>,
) -> Result<GetTaskPayloadResult, McpError> {
    let jobs = server.shared.jobs.clone();
    let screen = server.shared.screen.clone();
    let waited = async {
        loop {
            if screen.is_locked() {
                tracing::info!(job = %id, "host: a task's result waits for the unlock");
            }
            screen.unlocked().await;
            tokio::select! {
                biased;
                () = screen.locked() => continue,
                known = jobs.ended(id) => if !known {
                    return None;
                },
            }
            if !screen.is_locked() {
                return jobs.fetch(id);
            }
        }
    };
    let r = tokio::select! {
        r = waited => r.ok_or_else(|| unknown(id))?,
        () = context.ct.cancelled() => return Err(McpError::new(
            ErrorCode(-32800),
            format!("the wait for job {id}'s result was cancelled"),
            None,
        )),
    };
    let r = super::limits::related(r, id);
    let limits = *server.limits.read().expect("never poisoned");
    let r = super::server::within(&format!("job {id}"), r, &context.id, &limits);
    Ok(GetTaskPayloadResult::new(
        serde_json::to_value(r).unwrap_or_default(),
    ))
}

/// `tasks/cancel`: lmgw's cancel (the owner's or a device's in a thread, the thread deleted).
pub fn cancel(shared: &Shared, id: &str) -> Result<CancelTaskResult, McpError> {
    match shared.jobs.cancel(id, "lmgw") {
        Ok(task) => Ok(CancelTaskResult::new(task)),
        Err(r) => Err(McpError::invalid_params(r.words(id), None)),
    }
}

fn unknown(id: &str) -> McpError {
    McpError::invalid_params(format!("Kai has no job {id}"), None)
}

fn text_of(r: &rmcp::model::CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}
