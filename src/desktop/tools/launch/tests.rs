//! The launch tools over a recording fake: what each asks for and how its result reads.

use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::launch::{Captured, Ran, Started, Status};

use crate::host::limits::escaped_len;
use serde_json::{Value, json};

use super::*;
use crate::desktop::tests::FakeLaunch;
use crate::desktop::tools::Ctx;
use crate::host::registry::{Args, Block, Origin};

async fn call(fake: Arc<FakeLaunch>, name: &str, args: Value) -> (String, bool) {
    call_with(fake, name, args, &Ctx::default()).await
}

async fn call_with(fake: Arc<FakeLaunch>, name: &str, args: Value, ctx: &Ctx) -> (String, bool) {
    let services = Services {
        launch: Some(fake),
        ..Default::default()
    };
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    let r = crate::desktop::tools::run_with(&services, name, &args, ctx).await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

fn started(id: &str, name: &str) -> Started {
    Started {
        id: id.into(),
        name: name.into(),
        instances: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn open_and_app_launch_say_what_started() {
    let fake = Arc::new(FakeLaunch {
        started: Started {
            mime: Some("text/plain".into()),
            ..started("org.kde.kate", "Kate")
        },
        ..Default::default()
    });
    let (t, err) = call(fake.clone(), "open", json!({"target": "~/notes.txt"})).await;
    assert!(!err);
    assert_eq!(t, "opened with Kate (org.kde.kate) as text/plain");
    let (t, _) = call(
        fake.clone(),
        "app_launch",
        json!({"app": "kate", "urls": ["a", "b"]}),
    )
    .await;
    assert_eq!(t, "started Kate (org.kde.kate)");
    assert_eq!(
        *fake.calls.lock().unwrap(),
        [
            "open_plan ~/notes.txt",
            "open_start ~/notes.txt",
            "app_launch kate [\"a\", \"b\"]"
        ]
    );
    let many = Started {
        instances: 3,
        notes: vec!["it takes no files or URLs, so none were passed".into()],
        ..started("viewer", "Viewer\nforged line")
    };
    assert_eq!(
        started_lines(&many, false),
        "started 3 instances of \"Viewer\\nforged line\" (viewer), one per file or URL\n\
         it takes no files or URLs, so none were passed"
    );
}

/// `open` resolves its application before the owner's confirmation, whose dialog names it, and
/// the call starts that one without resolving again; a refused resolution (a launcher) asks
/// nobody.
#[tokio::test]
async fn open_names_its_application_before_the_confirmation() {
    let fake = Arc::new(FakeLaunch {
        started: Started {
            mime: Some("text/plain".into()),
            ..started("org.kde.kate", "Kate\nforged")
        },
        ..Default::default()
    });
    let services = Services {
        launch: Some(fake.clone()),
        ..Default::default()
    };
    let entries = crate::desktop::entries(&services);
    let prepare = |name: &str| {
        entries
            .iter()
            .find(|e| e.spec.name == name)
            .unwrap()
            .prepare
            .clone()
    };
    assert!(prepare("app_launch").is_none() && prepare("run_command").is_none());
    let prepare = prepare("open").expect("open resolves first");
    let spec = kai_desktop::contract::find("open").unwrap().1;
    let args = |t: &str| Args::read(spec, json!({ "target": t }).as_object().cloned()).unwrap();
    let p = prepare(&args("~/a.txt"), Origin::default()).await.unwrap();
    assert_eq!(
        p.lines,
        ["opens with: \"Kate\\nforged\" (org.kde.kate) as text/plain"]
    );
    let ctx = Ctx {
        prepared: Some(p),
        ..Default::default()
    };
    let (t, err) = call_with(fake.clone(), "open", json!({"target": "~/a.txt"}), &ctx).await;
    assert!(!err, "{t}");
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["open_plan ~/a.txt", "open_start ~/a.txt"],
        "resolved once, before the confirmation"
    );
    assert_eq!(
        prepare(&args("x.desktop"), Origin::default())
            .await
            .err()
            .as_deref(),
        Some("the fake opens no launcher")
    );
}

fn ran(status: Status, out: &[u8], err: &[u8]) -> Ran {
    Ran {
        status,
        elapsed: Duration::from_millis(230),
        stdout: Captured {
            total: out.len() as u64,
            head: out.to_vec(),
            tail: vec![],
        },
        stderr: Captured {
            total: err.len() as u64,
            head: err.to_vec(),
            tail: vec![],
        },
        cap: None,
    }
}

const SAYS: &str = "67108864 bytes, lmgw's mcp.host_max_message_mb as it announced it";

#[tokio::test]
async fn run_command_says_status_sizes_and_output() {
    let fake = Arc::new(FakeLaunch {
        ran: Some(ran(Status::Exited(0), b"hello\n", b"")),
        ..Default::default()
    });
    let (t, err) = call(
        fake.clone(),
        "run_command",
        json!({"argv": ["echo", "hello"], "cwd": "src"}),
    )
    .await;
    assert!(!err);
    assert_eq!(
        t,
        "exit 0 after 0.2 s; stdout 6 bytes; stderr none\nstdout:\nhello"
    );
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["run [\"echo\", \"hello\"] Some(\"src\") keep None stop None"]
    );
    assert_eq!(
        ran_lines(&ran(Status::Signalled(9), b"", b"oops"), None, SAYS),
        "killed by signal 9 (SIGKILL) after 0.2 s; stdout none; stderr 4 bytes\nstderr:\noops"
    );
    assert_eq!(
        ran_lines(
            &ran(
                Status::TimedOut {
                    setting: "lmgw/timeout_ms = 60000, less 0.5 s for the answer".into()
                },
                b"a\xffb",
                b"e\n"
            ),
            None,
            SAYS
        ),
        "stopped at its time limit (lmgw/timeout_ms = 60000, less 0.5 s for the answer), its \
         processes killed after 0.2 s; stdout 3 bytes; stderr 2 bytes\n\
         stdout (not all UTF-8: such bytes show as U+FFFD):\na\u{FFFD}b\nstderr:\ne"
    );
    // Kept by the provider as its start and end: what fits shows, the limit is named.
    let mut cut = ran(Status::Exited(1), b"", b"");
    cut.stdout = Captured {
        total: 3 * 1024 * 1024,
        head: b"START\n".to_vec(),
        tail: b"END\n".to_vec(),
    };
    assert_eq!(
        ran_lines(&cut, Some(64 << 20), SAYS),
        format!(
            "exit 1 after 0.2 s; stdout 3.0 MiB (3145728 bytes), its first 6 bytes and last 4 \
             bytes shown, 3.0 MiB cut between; stderr none (cut to fit the tool link's message \
             limit: {SAYS})\nstdout:\nSTART\n[3.0 MiB of stdout cut here]\nEND"
        )
    );
    // A provider's refusal is the tool's error.
    let none = Arc::new(FakeLaunch::default());
    let (t, err) = call(none, "run_command", json!({"argv": ["x"]})).await;
    assert!(err);
    assert_eq!(t, "the fake runs nothing");
}

/// The call's room and deadline reach the provider: the room as the bytes to keep, the deadline
/// less the answer's margin, named by `lmgw/timeout_ms`.
#[tokio::test]
async fn run_command_is_bounded_by_its_call() {
    let fake = Arc::new(FakeLaunch {
        ran: Some(ran(Status::Exited(0), b"", b"")),
        ..Default::default()
    });
    let ctx = Ctx {
        deadline: Some(tokio::time::Instant::now() + Duration::from_secs(60)),
        timeout_ms: Some(60_000),
        room: Some(1000),
        room_says: SAYS.into(),
        prepared: None,
    };
    call_with(fake.clone(), "run_command", json!({"argv": ["true"]}), &ctx).await;
    assert_eq!(
        *fake.calls.lock().unwrap(),
        [
            "run [\"true\"] None keep Some(1000) stop Some(\"lmgw/timeout_ms = 60000, less 0.5 s \
          for the answer\")"
        ]
    );
}

/// Output of any bytes fits the room as the JSON the link carries: invalid UTF-8 grows to U+FFFD,
/// control characters to escapes; the result says what was cut and which limit cut it.
#[test]
fn binary_output_fits_the_room_as_json() {
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i * 7919 % 256) as u8).collect();
    for room in [2_000u64, 50_000, 300_000] {
        // As the provider keeps it with the room as its bound.
        let mut keep = crate::desktop::fdo::launch::spawn::Keep::new(Some(room));
        keep.push(&bytes);
        let mut r = ran(Status::Exited(0), b"", b"");
        r.stdout = keep.captured();
        let text = ran_lines(&r, Some(room), SAYS);
        assert!(escaped_len(&text) <= room, "{room}: {}", escaped_len(&text));
        assert!(escaped_len(&text) > room * 9 / 10, "{room}: uses the room");
        assert!(text.contains("(not all UTF-8: such bytes show as U+FFFD)"));
        assert!(text.contains(&format!(
            "(cut to fit the tool link's message limit: {SAYS})"
        )));
        assert!(text.starts_with("exit 0 after 0.2 s; stdout 195.3 KiB (200000 bytes), its first"));
    }
    // The owner's own bound, where it is the smaller, is the one named.
    let mut r = ran(Status::Exited(0), &bytes, b"");
    r.cap = Some((1000, "launch.output_max_mb = 1".into()));
    let text = ran_lines(&r, Some(1 << 20), SAYS);
    assert!(
        text.contains("(cut to fit launch.output_max_mb = 1)"),
        "{text}"
    );
    // Kept whole and fitting: all of it.
    let small = ran(Status::Exited(0), b"\x00\x01ok\n", b"");
    assert_eq!(
        ran_lines(&small, Some(1000), SAYS),
        "exit 0 after 0.2 s; stdout 5 bytes; stderr none\nstdout:\n\u{0}\u{1}ok"
    );
}

/// The streams share the room by need: a small one shows whole, the other gets the rest; the
/// shown ends never overlap.
#[test]
fn the_room_is_shared_by_need() {
    assert_eq!(budgets(10, 5, 20), (10, 5));
    assert_eq!(budgets(100, 5, 20), (15, 5));
    assert_eq!(budgets(5, 100, 20), (5, 15));
    assert_eq!(budgets(100, 100, 21), (11, 10));
    let mut r = ran(Status::Exited(0), "a".repeat(5000).as_bytes(), b"short\n");
    r.stdout.head = [b"HEAD".as_slice(), &[b'a'; 4992], b"TAIL"].concat();
    let text = ran_lines(&r, Some(1500), SAYS);
    assert!(escaped_len(&text) <= 1500);
    assert!(text.contains("\nstderr:\nshort"), "{text}");
    assert!(
        text.contains("stdout:\nHEAD") && text.contains("TAIL\nstderr"),
        "{text}"
    );
}

#[test]
fn sizes_read_as_a_person_reads_them() {
    assert_eq!(size(0), "0 bytes");
    assert_eq!(size(1023), "1023 bytes");
    assert_eq!(size(1536), "1.5 KiB");
    assert_eq!(size(8 * 1024 * 1024), "8.0 MiB");
    assert_eq!(size(3 << 30), "3.0 GiB");
}

/// K15 from the annotations: `open` and `app_launch` are asked for every caller by default,
/// `run_command` too, even for the owner; an approval in the thread decides them (K15 as built);
/// each may be set to allow (ruling 28).
#[test]
fn the_policy_follows_the_launch_annotations() {
    use crate::host::policy::{Caller, Decision, Policy};
    use crate::host::settings::{PolicyConfig, ToolState};
    let policy = Policy::new(&PolicyConfig::default());
    for spec in kai_desktop::contract::launch::AREA.tools {
        assert!(
            matches!(
                policy.decide(spec, &Caller::Owner, None),
                Decision::Confirm { .. }
            ),
            "{}",
            spec.name
        );
        assert_eq!(
            policy.decide(spec, &Caller::Gateway, Some(&Caller::Owner)),
            Decision::Allow
        );
        let allowed = Policy::new(&PolicyConfig {
            tools: [(spec.name.to_owned(), ToolState::Allow)].into(),
            ..Default::default()
        });
        assert_eq!(
            allowed.decide(spec, &Caller::Unknown(String::new()), None),
            Decision::Allow
        );
    }
    let run = kai_desktop::contract::find("run_command").unwrap().1;
    assert_eq!(
        policy.decide(run, &Caller::Owner, None),
        Decision::Confirm {
            why: "confirmed by default (K24)"
        }
    );
    assert!(run.hints.destructive);
    assert_eq!(run.class, kai_desktop::contract::Class::Async);
}
