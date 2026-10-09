//! K26's open-ended class over MCP Tasks (WP18, `super::super::tasks`): what Kai declares, a
//! task-augmented call admitted and answered with its task, lmgw following it (`tasks/get`,
//! `tasks/result`, the status notification, `tasks/cancel`), refusals before admission, a link
//! that drops, a fetch whose answer was lost, the result's room, a routine switched off, and the
//! screen lock.

use std::sync::Arc;

use kai_desktop::contract::{Class, ToolSpec};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::*;
use crate::host::registry::{Entry, Provider, Reply};

/// Answers once the test releases it.
const SLOW: ToolSpec = ToolSpec {
    name: "slow",
    description: "Answer when released.",
    params: &[TEXT],
    class: Class::Async,
    ..ToolSpec::READ
};
/// Fails when released.
const SLOW_FAIL: ToolSpec = ToolSpec {
    name: "slow_fail",
    description: "Fail when released.",
    class: Class::Async,
    ..ToolSpec::READ
};
/// Answers with exactly its call's room in bytes (`room None`: a short text).
const FILL: ToolSpec = ToolSpec {
    name: "fill",
    description: "Fill the room.",
    class: Class::Async,
    ..ToolSpec::READ
};
/// Sets its job's announce first thing (as an async routine's own `announce` does), then waits.
const ANNOUNCER: ToolSpec = ToolSpec {
    name: "announcer",
    description: "Announce itself.",
    class: Class::Async,
    ..ToolSpec::READ
};
/// State-changing: confirmed for an untrusted caller.
const SLOW_POKE: ToolSpec = ToolSpec {
    name: "slow_poke",
    description: "Change a thing, slowly.",
    class: Class::Async,
    ..ToolSpec::CHANGE
};

/// The tools; `slow_poke` (a routine here) only while `.2` is on.
struct AsyncTools(Ran, Arc<Notify>, Arc<std::sync::atomic::AtomicBool>);

impl Provider for AsyncTools {
    fn entries(&self) -> Vec<Entry> {
        let poke = self.2.load(std::sync::atomic::Ordering::SeqCst);
        [&SLOW, &SLOW_FAIL, &SLOW_POKE, &FILL, &ANNOUNCER]
            .into_iter()
            .filter(|spec| poke || spec.name != "slow_poke")
            .map(|spec| {
                let (ran, go) = (self.0.clone(), self.1.clone());
                Entry {
                    area: if spec.name == "slow_poke" {
                        kai_desktop::contract::routines::AREA.name
                    } else {
                        "test"
                    },
                    spec,
                    handler: Arc::new(move |call| {
                        let (ran, go) = (ran.clone(), go.clone());
                        Box::pin(async move {
                            if call.spec.name == "announcer" {
                                crate::jobs::set_announce(true);
                            }
                            if call.spec.name == "fill" {
                                ran.push(format!("fill room {:?}", call.room));
                                return Reply::text(match call.room {
                                    Some(n) => "a".repeat(n as usize),
                                    None => "no room".to_owned(),
                                });
                            }
                            let cancel = call.cancel.clone();
                            let watch = ran.clone();
                            tokio::spawn(async move {
                                cancel.cancelled().await;
                                watch.push(format!("{}'s token cancelled", call.spec.name));
                            });
                            ran.push(format!(
                                "{} started; deadline {}",
                                call.spec.name,
                                call.deadline.is_some()
                            ));
                            go.notified().await;
                            let text = call.args.text("text").unwrap_or_default().to_owned();
                            match call.spec.name {
                                "slow_fail" => Reply::error("it broke"),
                                name => Reply::text(format!("{name}: {text}")),
                            }
                        })
                    }),
                    prepare: None,
                }
            })
            .collect()
    }
}

struct TaskRig {
    r: Rig,
    c: Conn,
    go: Arc<Notify>,
    /// `slow_poke`, the routine, is offered.
    poke: Arc<std::sync::atomic::AtomicBool>,
    next: i64,
}

async fn task_rig() -> TaskRig {
    task_rig_with(None).await
}

/// [`task_rig`] whose lmgw announces `limits` in `initialize`.
async fn task_rig_with(limits: Option<Value>) -> TaskRig {
    let go = Arc::new(Notify::new());
    let ran = Ran::default();
    let poke = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut r = rig_with_providers(
        PolicyConfig::default(),
        settings(1),
        vec![Box::new(AsyncTools(ran.clone(), go.clone(), poke.clone()))],
    )
    .await;
    r.ran = ran;
    let mut c = r.fake.accept().await;
    c.initialize_with(limits).await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    // The feed's hello: Kai's own device and the label its tools are listed under.
    r.host.feed_up("kai-test", Some("kailive"));
    TaskRig {
        r,
        c,
        go,
        poke,
        next: 9000,
    }
}

impl TaskRig {
    /// A task-augmented `tools/call` (`task: {}` and `lmgw/task`), its id.
    async fn call(&mut self, name: &str, args: Value, caller: Value, thread: Option<i64>) -> i64 {
        self.next += 1;
        let id = self.next;
        self.c
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {
                    "name": name,
                    "arguments": args,
                    "task": {},
                    "_meta": {
                        "lmgw/caller": caller,
                        "lmgw/timeout_ms": 60_000,
                        "lmgw/task": {"delivery": "thread", "thread_id": thread}
                    }
                }
            }))
            .await;
        id
    }

    async fn rpc(&mut self, method: &str, task: &str) -> Value {
        self.c.request(method, json!({"taskId": task})).await
    }
}

/// `initialize` declares Kai a task receiver, and the async class is listed `required`.
#[tokio::test]
async fn kai_declares_tasks_and_the_async_class_requires_them() {
    let go = Arc::new(Notify::new());
    let r = rig_with_providers(
        PolicyConfig::default(),
        settings(1),
        vec![Box::new(AsyncTools(
            Ran::default(),
            go,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        ))],
    )
    .await;
    let mut c = r.fake.accept().await;
    let init = c.initialize().await;
    let tasks = &init["result"]["capabilities"]["tasks"];
    assert_eq!(tasks["requests"]["tools"]["call"], json!({}), "{init}");
    assert_eq!(tasks["cancel"], json!({}), "{init}");
    let tools = c.list().await;
    let support = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["execution"]["taskSupport"].clone())
            .unwrap()
    };
    assert_eq!(support("slow"), json!("required"));
    assert_eq!(
        support("echo"),
        Value::Null,
        "a synchronous tool has no execution"
    );
}

/// The whole life: the task at once with the model's line, `working`, the release, the status
/// notification, `completed`, the result; then the job is forgotten once `task.done` said it.
#[tokio::test]
async fn a_task_answers_at_once_and_its_result_follows() {
    let mut t = task_rig().await;
    let id = t
        .call("slow", json!({"text": "hi"}), owner(), Some(812))
        .await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let task = &a["result"]["task"];
    assert_eq!(task["status"], "working", "{a}");
    assert!(task["ttl"].is_null(), "{a}");
    let job = task["taskId"].as_str().unwrap().to_owned();
    // The task id: the short id to say, this run's part, a counter.
    let short = job.split('-').next().unwrap().to_owned();
    assert_eq!(short.len(), 4, "{job}");
    assert_eq!(job.split('-').count(), 3, "{job}");
    let line = a["result"]["_meta"]["io.modelcontextprotocol/model-immediate-response"]
        .as_str()
        .unwrap();
    assert!(line.contains(&format!("job {short}")), "{line}");
    assert!(line.contains("notifies the owner"), "{line}");
    // The job runs open-ended: no deadline of lmgw's.
    t.r.ran.until("slow started; deadline false").await;
    let jobs = t.r.host.jobs();
    assert_eq!(jobs.thread_of(&job), Some(812));
    let view = &jobs.list()[0];
    assert!(view.running && !view.announce, "{view:?}");
    assert_eq!(view.caller, "the owner");
    let g = t.rpc("tasks/get", &job).await;
    assert_eq!(g["result"]["status"], "working", "{g}");
    // tasks/result is held until the job ended.
    t.next += 1;
    let held = t.next;
    t.c.send(
        json!({"jsonrpc": "2.0", "id": held, "method": "tasks/result", "params": {"taskId": job}}),
    )
    .await;
    t.go.notify_one();
    let mut seen = Vec::new();
    let res = t.c.answer(held, &mut seen).await;
    assert_eq!(text_of(&res).0, "slow: hi", "{res}");
    assert_eq!(
        res["result"]["_meta"]["io.modelcontextprotocol/related-task"]["taskId"],
        json!(job)
    );
    let note = seen
        .iter()
        .find(|m| m["method"] == "notifications/tasks/status")
        .unwrap_or_else(|| panic!("no status notification: {seen:?}"));
    assert_eq!(note["params"]["status"], "completed", "{note}");
    assert_eq!(note["params"]["taskId"], json!(job));
    let g = t.rpc("tasks/get", &job).await;
    assert_eq!(g["result"]["status"], "completed", "{g}");
    // `task.done` said it entered its thread: lmgw has the result too, so the job goes.
    assert_eq!(
        jobs.said("other", &job),
        None,
        "another label's task is not Kai's"
    );
    assert_eq!(jobs.said("kailive", &job), Some(false));
    assert!(jobs.list().is_empty(), "{:?}", jobs.list());
}

/// A job Kai's own voice session started is announced (ruling 22), and the model's line says so.
#[tokio::test]
async fn a_job_of_kais_own_session_is_announced() {
    let mut t = task_rig().await;
    let own = json!({"kind": "device", "name": "kai-test"});
    let id = t.call("slow", json!({"text": "v"}), own, Some(9)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let line = a["result"]["_meta"]["io.modelcontextprotocol/model-immediate-response"]
        .as_str()
        .unwrap_or_else(|| panic!("{a}"))
        .to_owned();
    assert!(line.contains("Kai speaks it"), "{line}");
    let jobs = t.r.host.jobs();
    let v = jobs.list().remove(0);
    assert!(v.announce, "{v:?}");
    assert_eq!(v.caller, "Kai itself");
}

/// The model's line states the announce a routine sets at its run's start, not the caller's
/// default.
#[tokio::test]
async fn the_models_line_reads_the_routines_own_announce() {
    let mut t = task_rig().await;
    let id = t.call("announcer", json!({}), owner(), Some(9)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let line = a["result"]["_meta"]["io.modelcontextprotocol/model-immediate-response"]
        .as_str()
        .unwrap_or_else(|| panic!("{a}"))
        .to_owned();
    assert!(line.contains("Kai speaks it"), "{line}");
    assert!(t.r.host.jobs().list()[0].announce);
}

/// A failed job (an error reply) is MCP's `failed`, its result `isError`.
#[tokio::test]
async fn an_error_reply_is_a_failed_task() {
    let mut t = task_rig().await;
    let id = t.call("slow_fail", json!({}), owner(), None).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow_fail started; deadline false").await;
    t.go.notify_one();
    let mut seen = Vec::new();
    while !seen
        .iter()
        .any(|m: &Value| m["method"] == "notifications/tasks/status")
    {
        seen.push(t.c.recv().await);
    }
    let g = t.rpc("tasks/get", &job).await;
    assert_eq!(g["result"]["status"], "failed", "{g}");
    let res = t.rpc("tasks/result", &job).await;
    assert_eq!(text_of(&res), ("it broke".to_owned(), true), "{res}");
    // No thread will say it entered, so nothing confirms lmgw got it: a second fetch (the first
    // answer lost with a half-dead link) still has it, until jobs.keep_ended_s.
    let v = t.r.host.jobs().view(&job).unwrap();
    assert!(v.fetched && v.kept_until.is_some(), "{v:?}");
    let again = t.rpc("tasks/result", &job).await;
    assert_eq!(text_of(&again), ("it broke".to_owned(), true), "{again}");
}

/// A job's call has the room its text has in `tasks/result`'s answer under the link's limits: a
/// result that fills it reaches lmgw whole, not as the message limit's error.
#[tokio::test]
async fn a_jobs_result_fits_the_tasks_result_answer() {
    let mut t = task_rig_with(Some(
        json!({"max_message_bytes": 20_000, "max_frame_bytes": 0}),
    ))
    .await;
    let id = t.call("fill", json!({}), owner(), Some(6)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    let room =
        t.r.ran
            .all()
            .iter()
            .find_map(|l| {
                l.strip_prefix("fill room Some(")
                    .map(|n| n.trim_end_matches(')').to_owned())
            })
            .unwrap_or_else(|| panic!("{:?}", t.r.ran.all()))
            .parse::<usize>()
            .unwrap();
    assert!(room > 19_000 && room < 20_000, "{room}");
    let res = t.rpc("tasks/result", &job).await;
    let (text, error) = text_of(&res);
    assert!(!error, "{}", &text[..text.len().min(300)]);
    assert_eq!(text.len(), room);
    assert!(
        serde_json::to_string(&res).unwrap().len() <= 20_000,
        "the answer is within the message limit"
    );
}

/// A routine switched off (its entry no longer offered) ends its running job; a K15 policy change
/// that unlists a tool ends none.
#[tokio::test]
async fn a_routine_switched_off_ends_its_job_and_a_policy_change_does_not() {
    let mut t = task_rig().await;
    // slow_poke stands for a routine (the routines area) here.
    let id = t.call("slow_poke", json!({}), owner(), Some(1)).await;
    let q = t.r.ask().await;
    q.reply.send(true).unwrap();
    let a = t.c.answer(id, &mut Vec::new()).await;
    let routine_job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow_poke started; deadline false").await;
    let id = t.call("slow", json!({"text": "x"}), owner(), Some(1)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let other = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow started; deadline false").await;
    let jobs = t.r.host.jobs();
    // The policy turns `slow` off: unlisted, its job runs on.
    let mut policy = PolicyConfig::default();
    policy
        .tools
        .insert("slow".into(), crate::host::settings::ToolState::Off);
    t.r.host
        .set_policy(crate::host::policy::Policy::new(&policy));
    assert!(jobs.view(&other).unwrap().running);
    assert!(jobs.view(&routine_job).unwrap().running);
    // The routine is switched off: its provider no longer offers it, and the relist (any change,
    // here the policy's again) sees that.
    t.poke.store(false, std::sync::atomic::Ordering::SeqCst);
    t.r.host
        .set_policy(crate::host::policy::Policy::new(&policy));
    t.r.ran.until("slow_poke's token cancelled").await;
    let g = t.rpc("tasks/get", &routine_job).await;
    assert_eq!(g["result"]["status"], "cancelled", "{g}");
    assert!(jobs.view(&other).unwrap().running);
}

/// `tasks/cancel`: `cancelled` at once, the handler's token cancelled; a second cancel and an
/// unknown id are `-32602` (lmgw reads the unknown one as abandoned).
#[tokio::test]
async fn tasks_cancel_stops_the_job() {
    let mut t = task_rig().await;
    let id = t.call("slow", json!({"text": "x"}), owner(), Some(1)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow started; deadline false").await;
    let c = t.rpc("tasks/cancel", &job).await;
    assert_eq!(c["result"]["status"], "cancelled", "{c}");
    t.r.ran.until("slow's token cancelled").await;
    let res = t.rpc("tasks/result", &job).await;
    let (text, error) = text_of(&res);
    assert!(error && text.contains("cancelled by lmgw"), "{res}");
    let again = t.rpc("tasks/cancel", &job).await;
    assert_eq!(again["error"]["code"], -32602, "{again}");
    for method in ["tasks/get", "tasks/result", "tasks/cancel"] {
        let a = t.rpc(method, "zzzz").await;
        assert_eq!(a["error"]["code"], -32602, "{method}: {a}");
    }
}

/// A call refused before it is admitted (the owner denies it) is the request's error, and no job
/// exists.
#[tokio::test]
async fn a_refused_task_call_is_an_error_and_no_job() {
    let mut t = task_rig().await;
    let id = t.call("slow_poke", json!({}), key("script"), Some(1)).await;
    let q = t.r.ask().await;
    q.reply.send(false).unwrap();
    let a = t.c.answer(id, &mut Vec::new()).await;
    assert_eq!(a["error"]["code"], -32001, "{a}");
    assert!(
        a["error"]["message"]
            .as_str()
            .unwrap()
            .contains("the owner denied"),
        "{a}"
    );
    assert!(t.r.host.jobs().list().is_empty());
    assert!(t.r.ran.all().is_empty(), "{:?}", t.r.ran.all());
}

/// lmgw's cancel of the call while the owner is asked: the dialog closes, nothing runs.
#[tokio::test]
async fn a_task_call_cancelled_before_admission_never_runs() {
    let mut t = task_rig().await;
    let id = t.call("slow_poke", json!({}), key("script"), Some(1)).await;
    let mut q = t.r.ask().await;
    t.c.cancel(id, "the turn was cancelled").await;
    tokio::time::timeout(WAIT, q.reply.closed()).await.unwrap();
    let a = t.c.answer(id, &mut Vec::new()).await;
    assert!(a.get("error").is_some(), "{a}");
    assert!(t.r.host.jobs().list().is_empty());
}

/// A link that drops ends nothing (lmgw's T7): the next link reads the job, and its result.
#[tokio::test]
async fn a_job_outlives_its_link() {
    let mut t = task_rig().await;
    let id = t
        .call("slow", json!({"text": "on"}), owner(), Some(3))
        .await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow started; deadline false").await;
    t.c.close(1001, "lmgw restarts").await;
    let mut c = t.r.fake.accept().await;
    c.initialize().await;
    let g = c.request("tasks/get", json!({"taskId": job})).await;
    assert_eq!(g["result"]["status"], "working", "{g}");
    assert_eq!(
        t.r.ran.all(),
        ["slow started; deadline false"],
        "the drop stopped nothing"
    );
    t.go.notify_one();
    let res = c.request("tasks/result", json!({"taskId": job})).await;
    assert_eq!(text_of(&res).0, "slow: on", "{res}");
}

/// The screen lock: a new task call is refused; a job admitted before runs on, and delivers
/// nothing while locked: its `tasks/result` and its status notification wait for the unlock
/// (see the module doc of `crate::host::tasks`).
#[tokio::test]
async fn the_lock_refuses_new_jobs_and_holds_what_running_ones_deliver() {
    let mut t = task_rig().await;
    let id = t
        .call("slow", json!({"text": "kept"}), owner(), Some(4))
        .await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow started; deadline false").await;
    t.r.screen.set(true);
    let id = t
        .call("slow", json!({"text": "new"}), owner(), Some(4))
        .await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    assert_eq!(a["error"]["code"], -32001, "{a}");
    assert!(
        a["error"]["message"]
            .as_str()
            .unwrap()
            .contains(crate::screen_lock::REFUSAL),
        "{a}"
    );
    t.next += 1;
    let held = t.next;
    t.c.send(
        json!({"jsonrpc": "2.0", "id": held, "method": "tasks/result", "params": {"taskId": job}}),
    )
    .await;
    t.go.notify_one();
    // It ran on and ended; nothing of it reaches lmgw while locked.
    t.r.ran.until("slow's token cancelled").await;
    assert!(!t.r.host.jobs().view(&job).unwrap().running);
    let quiet = tokio::time::timeout(Duration::from_millis(500), t.c.recv()).await;
    assert!(quiet.is_err(), "sent while locked: {quiet:?}");
    let g = t.rpc("tasks/get", &job).await;
    assert_eq!(
        g["result"]["status"], "completed",
        "tasks/get says it as it is: {g}"
    );
    t.r.screen.set(false);
    let mut seen = Vec::new();
    let res = t.c.answer(held, &mut seen).await;
    assert_eq!(text_of(&res).0, "slow: kept", "{res}");
    while !seen
        .iter()
        .any(|m: &Value| m["method"] == "notifications/tasks/status")
    {
        seen.push(t.c.recv().await);
    }
}

/// Kai's quit cancels what runs; lmgw reads it `cancelled`.
#[tokio::test]
async fn the_quit_cancels_running_jobs() {
    let mut t = task_rig().await;
    let id = t.call("slow", json!({"text": "x"}), owner(), Some(5)).await;
    let a = t.c.answer(id, &mut Vec::new()).await;
    let job = a["result"]["task"]["taskId"].as_str().unwrap().to_owned();
    t.r.ran.until("slow started; deadline false").await;
    t.r.host.jobs().quit();
    t.r.ran.until("slow's token cancelled").await;
    let g = t.rpc("tasks/get", &job).await;
    assert_eq!(g["result"]["status"], "cancelled", "{g}");
}
