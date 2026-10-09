//! The job table (WP18): ids (short and on the wire, refused when every short id is taken), the
//! announce default and its changes, who may act on a job, the end, `tasks/result`'s wait, when a
//! job is forgotten (a confirmed delivery, `jobs.keep_ended_s`), the label known late, a routine
//! switched off, the listing and the tray.

use std::time::{Duration, UNIX_EPOCH};

use rmcp::model::{CallToolResult, ContentBlock, TaskStatus};

use super::*;
use crate::host::settings::Named;

fn start(own: bool, delivery: Delivery) -> Start {
    Start {
        tool: "run_command",
        label: Some("kai".into()),
        by: if own { Caller::Own } else { Caller::Owner },
        delivery,
        routine: false,
    }
}

fn device(name: &str) -> Caller {
    Caller::Named(Named {
        kind: "device".into(),
        name: name.into(),
    })
}

fn ok(text: &str) -> Outcome {
    Outcome::Answered(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[test]
fn iso_is_utc_with_milliseconds() {
    assert_eq!(iso(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    let t = UNIX_EPOCH + Duration::from_millis(1_791_504_000_123);
    assert_eq!(iso(t), "2026-10-09T00:00:00.123Z");
    // A leap day.
    let t = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723);
    assert_eq!(iso(t), "2000-02-29T01:02:03.000Z");
}

/// Short ids are four hex digits unique among the jobs Kai knows; the task id is the short id with
/// the process's part and a counter, never repeated in a run; either names the job.
#[test]
fn ids_are_short_to_say_and_unique_on_the_wire() {
    let jobs = Jobs::default();
    let mut shorts = std::collections::HashSet::new();
    let mut wires = std::collections::HashSet::new();
    for _ in 0..500 {
        let s = jobs.start(start(false, Delivery::Wait)).unwrap();
        assert_eq!(s.id.len(), 4);
        assert!(s.id.chars().all(|c| c.is_ascii_hexdigit()), "{}", s.id);
        assert_eq!(s.task.task_id, s.wire);
        let parts: Vec<&str> = s.wire.split('-').collect();
        assert_eq!(parts.len(), 3, "{}", s.wire);
        assert_eq!(parts[0], s.id);
        assert_eq!(parts[1], *PROCESS);
        assert!(shorts.insert(s.id.clone()));
        assert!(wires.insert(s.wire.clone()));
        assert_eq!(jobs.task(&s.id).unwrap().task_id, s.wire);
        assert_eq!(jobs.view(&s.wire).unwrap().id, s.id);
    }
}

fn filler(id: String) -> Job {
    let now = SystemTime::now();
    Job {
        wire: format!("{id}-x-0"),
        id,
        start: start(false, Delivery::Wait),
        announce: false,
        created: now,
        updated: now,
        state: State::Running,
        cancel: CancellationToken::new(),
        kept: None,
    }
}

/// With every short id taken a new job is refused, visibly and at once: never a search that does
/// not end. The last free one is found.
#[test]
fn every_short_id_taken_is_a_refusal_not_a_hang() {
    let all: Vec<Job> = (0..SHORT_IDS).map(|i| filler(format!("{i:04x}"))).collect();
    assert_eq!(fresh_id(&all), None);
    let mut one_free = all.clone();
    let gone = one_free.remove(0xbeef);
    assert_eq!(fresh_id(&one_free).as_deref(), Some(gone.id.as_str()));
    let jobs = Jobs::default();
    *jobs.lock() = all;
    let started = std::time::Instant::now();
    let r = jobs.start(start(false, Delivery::Wait));
    assert_eq!(r.err(), Some(Refusal::Full));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(Refusal::Full.words("x").contains("jobs.keep_ended_s"));
}

#[test]
fn announce_is_on_for_kais_own_session_and_off_otherwise() {
    let jobs = Jobs::default();
    let own = jobs
        .start(start(true, Delivery::Thread(Some(1))))
        .unwrap()
        .id;
    let other = jobs
        .start(start(false, Delivery::Thread(Some(1))))
        .unwrap()
        .id;
    let list = jobs.list();
    assert!(list.iter().find(|j| j.id == own).unwrap().announce);
    assert!(!list.iter().find(|j| j.id == other).unwrap().announce);
    jobs.set_announce(&other, true).unwrap();
    assert!(jobs.list().iter().find(|j| j.id == other).unwrap().announce);
    assert_eq!(jobs.set_announce("zzzz", true), Err(Refusal::Unknown));
}

/// The owner's callers act on any job; another caller only on its own; one lmgw did not name on
/// none.
#[test]
fn only_the_owner_or_the_starter_acts_on_a_job() {
    let jobs = Jobs::default();
    let phone = jobs
        .start(Start {
            by: device("phone"),
            ..start(false, Delivery::Thread(Some(1)))
        })
        .unwrap()
        .id;
    let laptop = device("laptop");
    assert_eq!(
        jobs.cancel_as(&phone, "x", &laptop).err(),
        Some(Refusal::NotYours("device 'phone'".into()))
    );
    assert_eq!(
        jobs.set_announce_as(&phone, true, &laptop),
        Err(Refusal::NotYours("device 'phone'".into()))
    );
    assert!(
        Refusal::NotYours("device 'phone'".into())
            .words(&phone)
            .contains("only the owner or its starter")
    );
    assert!(
        jobs.set_announce_as(&phone, true, &Caller::Unknown(String::new()))
            .is_err()
    );
    jobs.set_announce_as(&phone, true, &device("phone"))
        .unwrap();
    for owner in [Caller::Owner, Caller::Gateway, Caller::Own] {
        jobs.set_announce_as(&phone, false, &owner).unwrap();
    }
    jobs.cancel_as(&phone, "the phone", &device("phone"))
        .unwrap();
    assert_eq!(jobs.list()[0].status, "cancelled");
}

/// An async routine's `announce` reaches its job from inside the handler.
#[tokio::test]
async fn a_handler_sets_its_jobs_announce() {
    let jobs = Jobs::default();
    let id = jobs
        .start(start(true, Delivery::Thread(Some(1))))
        .unwrap()
        .id;
    scope(jobs.clone(), id.clone(), async { set_announce(false) }).await;
    assert!(!jobs.list()[0].announce);
    // Outside a job it does nothing.
    set_announce(true);
    assert!(!jobs.list()[0].announce);
}

#[tokio::test]
async fn the_result_waits_for_the_end_and_a_cancel_wins_over_a_late_answer() {
    let jobs = Jobs::default();
    let s = jobs.start(start(false, Delivery::Wait)).unwrap();
    let waiting = tokio::spawn({
        let jobs = jobs.clone();
        let wire = s.wire.clone();
        async move { jobs.result(&wire).await }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    jobs.end(&s.id, ok("done"));
    assert!(
        s.cancel.is_cancelled(),
        "the end stops what the handler spawned"
    );
    let r = waiting.await.unwrap().unwrap();
    assert_eq!(r.content[0].as_text().unwrap().text, "done");

    let s = jobs.start(start(false, Delivery::Wait)).unwrap();
    let t = jobs.cancel(&s.id, "the owner (the tray)").unwrap();
    assert_eq!(t.status, TaskStatus::Cancelled);
    assert!(s.cancel.is_cancelled());
    jobs.end(&s.id, ok("too late"));
    let r = jobs.result(&s.wire).await.unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(
        r.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("cancelled by the owner (the tray)")
    );
    assert_eq!(jobs.cancel("zzzz", "x").err(), Some(Refusal::Unknown));
}

/// A result that enters a thread Kai sees is kept until both lmgw fetched it and `task.done` said
/// it entered, in either order: the fetch alone confirms nothing.
#[tokio::test]
async fn a_thread_job_goes_once_fetched_and_said() {
    let jobs = Jobs::default();
    let a = jobs.start(start(true, Delivery::Thread(Some(7)))).unwrap();
    jobs.end(&a.id, ok("x"));
    jobs.result(&a.wire).await.unwrap();
    assert_eq!(jobs.list().len(), 1, "fetched, not said yet");
    // A second fetch (the first answer was lost with its link) still has it.
    assert!(jobs.fetch(&a.wire).is_some());
    assert_eq!(jobs.said("kai", &a.wire), Some(true));
    assert!(jobs.list().is_empty());

    let b = jobs.start(start(false, Delivery::Thread(Some(7)))).unwrap();
    jobs.end(&b.id, ok("y"));
    assert_eq!(jobs.said("kai", &b.wire), Some(false));
    assert_eq!(jobs.list().len(), 1, "said, not fetched yet");
    assert_eq!(
        jobs.cancel(&b.id, "x").err(),
        Some(Refusal::Ended("completed")),
        "an ended job is not cancelled"
    );
    jobs.result(&b.wire).await.unwrap();
    assert!(jobs.list().is_empty());
}

/// A result no `task.done` will confirm (lmgw's bridge, Admin Chat's thread, a delivery Kai does
/// not know) stays fetchable after its fetch, and goes `jobs.keep_ended_s` after the latest one.
#[tokio::test(start_paused = true)]
async fn an_unconfirmed_result_is_kept_for_jobs_keep_ended_s() {
    let jobs = Jobs::default();
    jobs.set_keep(Duration::from_secs(600));
    let mut ids = Vec::new();
    for d in [Delivery::Wait, Delivery::Thread(None), Delivery::Unknown] {
        let s = jobs.start(start(false, d)).unwrap();
        jobs.end(&s.id, ok("r"));
        jobs.result(&s.wire).await.unwrap();
        ids.push(s.wire);
    }
    tokio::time::sleep(Duration::from_secs(300)).await;
    // lmgw asks again (its first answer went down with a half-dead link): still there.
    assert!(jobs.fetch(&ids[0]).is_some());
    assert_eq!(jobs.list().len(), 3);
    let v = jobs.view(&ids[1]).unwrap();
    assert!(v.fetched && v.kept_until.is_some(), "{v:?}");
    assert!(listing(&jobs).contains("lmgw has its result; kept until"));
    tokio::time::sleep(Duration::from_secs(301)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        jobs.list()
            .iter()
            .map(|v| v.wire.clone())
            .collect::<Vec<_>>(),
        [ids[0].clone()],
        "the latest fetch kept the first longer"
    );
    tokio::time::sleep(Duration::from_secs(300)).await;
    tokio::task::yield_now().await;
    assert!(jobs.list().is_empty());
    // A thread job whose `task.done` never comes goes too.
    let s = jobs.start(start(false, Delivery::Thread(Some(4)))).unwrap();
    jobs.end(&s.id, ok("r"));
    tokio::time::sleep(Duration::from_secs(601)).await;
    tokio::task::yield_now().await;
    assert!(jobs.list().is_empty());
    // 0: kept until Kai quits.
    jobs.set_keep(Duration::ZERO);
    let s = jobs.start(start(false, Delivery::Wait)).unwrap();
    jobs.end(&s.id, ok("r"));
    jobs.result(&s.wire).await.unwrap();
    tokio::time::sleep(Duration::from_secs(86_400 * 30)).await;
    assert_eq!(jobs.list().len(), 1);
    assert!(listing(&jobs).contains("kept until Kai quits (jobs.keep_ended_s = 0)"));
}

/// A job called before the feed's `hello` named the label: its `task.done` is read with the label
/// the table knows by then.
#[test]
fn a_label_unknown_at_the_call_is_read_at_delivery() {
    let jobs = Jobs::default();
    let s = jobs
        .start(Start {
            label: None,
            ..start(true, Delivery::Thread(Some(3)))
        })
        .unwrap();
    jobs.end(&s.id, ok("x"));
    assert_eq!(jobs.said("kailive", &s.wire), None, "no label known yet");
    jobs.set_label(Some("kailive".into()));
    assert_eq!(jobs.said("other", &s.wire), None);
    assert_eq!(jobs.said("kailive", &s.wire), Some(true));
}

/// A routine switched off (or gone) ends its running jobs; other tools' jobs and ended ones stay.
#[test]
fn a_routine_off_ends_its_jobs() {
    let jobs = Jobs::default();
    let routine = |tool| Start {
        tool,
        routine: true,
        ..start(false, Delivery::Thread(Some(1)))
    };
    let backup = jobs.start(routine("backup")).unwrap();
    let sync = jobs.start(routine("sync")).unwrap();
    let cmd = jobs.start(start(false, Delivery::Thread(Some(1)))).unwrap();
    jobs.routines_gone(&["sync", "routine_create"].into_iter().collect());
    assert!(backup.cancel.is_cancelled());
    assert!(!sync.cancel.is_cancelled() && !cmd.cancel.is_cancelled());
    let v = jobs.view(&backup.id).unwrap();
    assert_eq!(v.status, "cancelled");
    // run_command is not a routine: never ended for not being one.
    jobs.routines_gone(&std::collections::HashSet::new());
    assert!(sync.cancel.is_cancelled());
    assert!(!cmd.cancel.is_cancelled());
}

/// The pairing's end and the quit cancel every running job, and `sent` waits for the status
/// notifications on their way.
#[tokio::test]
async fn cancel_all_tells_lmgw_before_it_returns() {
    let jobs = Jobs::default();
    let told = Arc::new(Mutex::new(Vec::new()));
    jobs.set_notifier(Arc::new({
        let told = told.clone();
        move |t: Task| {
            let told = told.clone();
            Some(tokio::spawn(async move {
                tokio::task::yield_now().await;
                told.lock().unwrap().push((t.task_id, t.status));
            }))
        }
    }));
    let a = jobs.start(start(false, Delivery::Thread(Some(1)))).unwrap();
    let b = jobs.start(start(false, Delivery::Wait)).unwrap();
    jobs.quit();
    jobs.sent(Duration::from_secs(5)).await;
    let mut told = told.lock().unwrap().clone();
    told.sort_by(|x, y| x.0.cmp(&y.0));
    let mut want = vec![
        (a.wire.clone(), TaskStatus::Cancelled),
        (b.wire.clone(), TaskStatus::Cancelled),
    ];
    want.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(told, want);
    let r = jobs.fetch(&a.wire).unwrap();
    assert!(r.content[0].as_text().unwrap().text.contains("Kai's quit"));
}

#[test]
fn the_listing_says_each_job() {
    let jobs = Jobs::default();
    assert_eq!(listing(&jobs), "No jobs: nothing runs.");
    let a = jobs
        .start(start(true, Delivery::Thread(Some(1))))
        .unwrap()
        .id;
    let b = jobs
        .start(start(false, Delivery::Thread(Some(1))))
        .unwrap()
        .id;
    jobs.end(
        &b,
        Outcome::Answered(CallToolResult::error(vec![ContentBlock::text("no")])),
    );
    let text = listing(&jobs);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with(&format!("job {a} (run_command): running since ")),
        "{text}"
    );
    assert!(
        lines[0].ends_with("started by Kai itself; its result is spoken"),
        "{text}"
    );
    assert!(
        lines[1].starts_with(&format!(
            "job {b} (run_command): failed, its result waits for lmgw; kept until "
        )),
        "{text}"
    );
    assert!(
        lines[1].ends_with(" (jobs.keep_ended_s); started by the owner; its result is notified"),
        "{text}"
    );
}

/// The published table is the tray's: its entries follow it, and the tray's Cancel and switch
/// act on it.
#[tokio::test]
async fn the_tray_follows_the_published_table() {
    let jobs = Jobs::default();
    jobs.publish();
    let mut tray = tray_entries();
    let id = jobs
        .start(start(false, Delivery::Thread(Some(2))))
        .unwrap()
        .id;
    let e = tray.borrow_and_update().clone();
    assert_eq!(e.len(), 1);
    assert!(e[0].running && !e[0].announce);
    assert!(
        e[0].label
            .starts_with(&format!("run_command · job {id} · since ")),
        "{e:?}"
    );
    tray_announce(&id, true);
    assert!(tray.borrow_and_update()[0].announce);
    tray_cancel(&id);
    let e = tray.borrow_and_update().clone();
    assert!(!e[0].running);
    assert!(
        e[0].label
            .contains("cancelled, its result waits for lmgw; kept until"),
        "{e:?}"
    );
}

/// The feed's `device.revoked` cancels the running jobs that device started and tells lmgw; the
/// owner's, Kai's own, another device's (a name that only starts alike), a key's and an ended job
/// stay; a device with no jobs is a no-op. A job being listed (the table read) while it arrives is
/// no obstacle, and a second event for the same device finds nothing left.
#[tokio::test]
async fn a_deleted_device_ends_the_jobs_it_started_and_no_others() {
    let jobs = Jobs::default();
    let told = Arc::new(Mutex::new(Vec::new()));
    jobs.set_notifier(Arc::new({
        let told = told.clone();
        move |t: Task| {
            told.lock().unwrap().push((t.task_id, t.status));
            None
        }
    }));
    let by = |c: Caller| {
        jobs.start(Start {
            by: c,
            ..start(false, Delivery::Thread(Some(1)))
        })
        .unwrap()
    };
    let key = Caller::Named(Named {
        kind: "key".into(),
        name: "phone".into(),
    });
    let p1 = by(device("phone"));
    let p2 = by(device("phone"));
    let done = by(device("phone"));
    jobs.end(&done.id, ok("fine"));
    let tablet = by(device("tablet"));
    let phone2 = by(device("phone2"));
    let keyed = by(key);
    let owner = by(Caller::Owner);
    let own = by(Caller::Own);
    told.lock().unwrap().clear();

    let held = jobs.list();
    assert_eq!(jobs.device_revoked("watch"), 0);
    assert!(told.lock().unwrap().is_empty());
    assert_eq!(jobs.device_revoked("phone"), 2);
    drop(held);
    assert_eq!(jobs.device_revoked("phone"), 0);

    for s in [&p1, &p2] {
        assert!(s.cancel.is_cancelled());
        assert_eq!(jobs.view(&s.id).unwrap().status, "cancelled");
        let r = jobs.fetch(&s.wire).unwrap();
        assert!(r.content[0].as_text().unwrap().text.contains("'phone'"));
    }
    for s in [&tablet, &phone2, &keyed, &owner, &own] {
        assert!(!s.cancel.is_cancelled(), "{}", s.id);
        assert_eq!(jobs.view(&s.id).unwrap().status, "running");
    }
    assert_eq!(jobs.view(&done.id).unwrap().status, "completed");
    let mut told = told.lock().unwrap().clone();
    told.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = vec![
        (p1.wire.clone(), TaskStatus::Cancelled),
        (p2.wire.clone(), TaskStatus::Cancelled),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(told, want);
}
