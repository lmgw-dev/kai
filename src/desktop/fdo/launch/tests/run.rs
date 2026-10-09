//! Programs started for real through the fake service manager: `run_command`'s output, status,
//! timeout and cancellation; `app_launch` and `open` with a recording script as the application;
//! the scope asked for each, and what a program inherits.

use std::time::Duration;

use kai_desktop::caps::Failure;
use kai_desktop::caps::launch::{Launch, Ran, RunBounds, Status};
use zbus::zvariant::OwnedValue;

use super::super::FdoLaunch;
use super::super::settings::LaunchConfig;
use super::{T, read_when_there, rig, script, write};

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// A command with no bound but the provider's own.
async fn run(l: &FdoLaunch, argv: &[String], cwd: Option<&str>) -> Result<Ran, Failure> {
    l.run(argv, cwd, &RunBounds::default()).await
}

fn text(v: &OwnedValue) -> String {
    String::try_from(v.try_clone().unwrap()).unwrap()
}

/// Whether process `pid` still runs (a zombie does not).
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next())
        })
        .is_some_and(|state| state != 'Z' && state != 'X')
}

async fn gone(pid: i32) {
    let end = std::time::Instant::now() + T;
    while alive(pid) {
        assert!(std::time::Instant::now() < end, "{pid} still runs");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A command's output, sizes and exit status; its scope (Kai's id with `.run`, its pid, the app
/// slice, collected when it ends, no argument in the description); the session's environment
/// without Kai's variables; the home directory as its working directory.
#[tokio::test]
async fn a_command_runs_in_its_scope_with_its_output_captured() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let ran = run(
        &r.launch,
        &argv(&[
            "sh",
            "-c",
            "echo out; echo err >&2; echo \"$RIG_MARK:${KAI_LOG:-none}\"; pwd; echo $$; exit 3",
        ]),
        None,
    )
    .await
    .unwrap();
    assert_eq!(ran.status, Status::Exited(3));
    let out = String::from_utf8(ran.stdout.head.clone()).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "out");
    assert_eq!(lines[1], "1:none", "the session's environment, no KAI_*");
    assert_eq!(lines[2], r.home().display().to_string());
    assert_eq!(ran.stdout.total, out.len() as u64);
    assert!(ran.stdout.tail.is_empty());
    assert_eq!(ran.stderr.head, b"err\n");
    assert_eq!(ran.cap, None, "no bound of the owner's own by default");
    let scopes = r.scopes();
    assert_eq!(scopes.len(), 1);
    let s = &scopes[0];
    assert!(
        s.unit.starts_with("app-dev.rootshell.kai.run-") && s.unit.ends_with(".scope"),
        "{}",
        s.unit
    );
    let pids = Vec::<u32>::try_from(s.props["PIDs"].try_clone().unwrap()).unwrap();
    assert_eq!(pids, [lines[3].parse::<u32>().unwrap()], "the shell itself");
    assert_eq!(text(&s.props["Slice"]), "app.slice");
    assert_eq!(text(&s.props["CollectMode"]), "inactive-or-failed");
    assert_eq!(text(&s.props["Description"]), "Kai: run_command");
    // A working directory from home, and `~/`.
    std::fs::create_dir_all(r.home().join("sub dir")).unwrap();
    for cwd in ["sub dir", "~/sub dir"] {
        let ran = run(&r.launch, &argv(&["pwd"]), Some(cwd)).await.unwrap();
        assert_eq!(
            String::from_utf8(ran.stdout.head).unwrap().trim(),
            r.home().join("sub dir").display().to_string()
        );
    }
}

/// The program gets stdin, stdout and stderr only: a descriptor Kai left inheritable does not
/// reach it; it leads a session (and process group) of its own.
#[tokio::test]
async fn a_program_inherits_no_descriptor_and_has_its_own_session() {
    use std::os::fd::AsRawFd;
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let leak = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: fcntl on a descriptor this test owns: clear its close-on-exec flag.
    unsafe {
        libc::fcntl(leak.as_raw_fd(), libc::F_SETFD, 0);
    }
    let pidfile = r.path("pid");
    let cmd = argv(&[
        "sh",
        "-c",
        &format!("echo $$ > '{}'; exec sleep 30", pidfile.display()),
    ]);
    let run = run(&r.launch, &cmd, None);
    let pid: i32 = tokio::select! {
        _ = run => panic!("ended by itself"),
        p = async {
            let p: i32 = read_when_there(&pidfile).await.trim().parse().unwrap();
            // Until the shell exec'd sleep.
            let end = std::time::Instant::now() + T;
            while !std::fs::read_to_string(format!("/proc/{p}/comm")).is_ok_and(|c| c.trim() == "sleep") {
                assert!(std::time::Instant::now() < end);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let mut fds: Vec<i32> = std::fs::read_dir(format!("/proc/{p}/fd"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().parse().unwrap())
                .collect();
            fds.sort();
            assert_eq!(fds, [0, 1, 2]);
            // stat: pid (comm) state ppid pgrp session …
            let stat = std::fs::read_to_string(format!("/proc/{p}/stat")).unwrap();
            let rest: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
            assert_eq!((rest[2], rest[3]), (p.to_string().as_str(), p.to_string().as_str()));
            p
        } => p,
    };
    drop(leak);
    gone(pid).await;
}

/// Past `launch.command_timeout_s` every process of the scope is killed (`KillUnit`): a background
/// child, and one that left the process group (`setsid`); what it wrote so far comes back with the
/// setting named.
#[tokio::test]
async fn a_command_past_its_timeout_is_stopped_with_its_scope() {
    let Some(r) = rig(LaunchConfig {
        command_timeout_s: 1,
        ..Default::default()
    })
    .await
    else {
        return;
    };
    let pidfile = r.path("bg.pid");
    let started = std::time::Instant::now();
    let ran = run(
        &r.launch,
        &argv(&[
            "sh",
            "-c",
            &format!(
                "sleep 30 & a=$!; setsid sleep 30 & echo $a $! > '{}'; echo before; sleep 30",
                pidfile.display()
            ),
        ]),
        None,
    )
    .await
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        ran.status,
        Status::TimedOut {
            setting: "launch.command_timeout_s = 1".into()
        }
    );
    assert_eq!(ran.stdout.head, b"before\n");
    for pid in read_when_there(&pidfile).await.split_whitespace() {
        gone(pid.parse().unwrap()).await;
    }
    let kills = r.state.lock().unwrap().kills.clone();
    assert_eq!(kills.len(), 1);
    assert!(
        kills[0].0.starts_with("app-dev.rootshell.kai.run-"),
        "{kills:?}"
    );
    assert_eq!((kills[0].1.as_str(), kills[0].2), ("all", libc::SIGKILL));
}

/// The call's deadline (less the answer's margin, which the tool layer takes off) stops the
/// command and is named; one already past runs nothing; the owner's own limit, where it comes
/// first, is the one named.
#[tokio::test]
async fn a_command_stops_at_its_calls_deadline() {
    let Some(mut r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let bounds = RunBounds {
        keep: None,
        stop_at: Some((
            std::time::Instant::now() + Duration::from_millis(700),
            "lmgw/timeout_ms = 1200, less 0.5 s for the answer".into(),
        )),
    };
    let started = std::time::Instant::now();
    let ran = r
        .launch
        .run(&argv(&["sh", "-c", "echo up; sleep 30"]), None, &bounds)
        .await
        .unwrap();
    let took = started.elapsed();
    // From the limit to the answer (the scope killed, the leader reaped, the pipes read): well
    // within the answer's margin, which also covers writing the result to lmgw.
    let late = took.saturating_sub(Duration::from_millis(700));
    println!("stopped {late:?} after the limit");
    assert!(took >= Duration::from_millis(650), "{took:?}");
    assert!(
        late < crate::desktop::tools::launch::ANSWER_MARGIN / 2,
        "{late:?} after the limit"
    );
    assert_eq!(
        ran.status,
        Status::TimedOut {
            setting: "lmgw/timeout_ms = 1200, less 0.5 s for the answer".into()
        }
    );
    assert_eq!(ran.stdout.head, b"up\n");
    let past = RunBounds {
        keep: None,
        stop_at: Some((std::time::Instant::now(), "lmgw/timeout_ms = 10".into())),
    };
    let e = r
        .launch
        .run(&argv(&["true"]), None, &past)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "no time is left to run it before its limit (lmgw/timeout_ms = 10)"
    );
    r.launch.config.command_timeout_s = 1;
    let far = RunBounds {
        keep: None,
        stop_at: Some((
            std::time::Instant::now() + Duration::from_secs(30),
            "lmgw/timeout_ms = 30500, less 0.5 s for the answer".into(),
        )),
    };
    let ran = r
        .launch
        .run(&argv(&["sleep", "30"]), None, &far)
        .await
        .unwrap();
    assert_eq!(
        ran.status,
        Status::TimedOut {
            setting: "launch.command_timeout_s = 1".into()
        }
    );
}

/// lmgw's `notifications/cancelled` drops the call: every process of its scope is killed, one
/// that left the process group too: by the scope's `cgroup.kill` at the drop, without the manager
/// (where the fake made it a cgroup), else the process group and then `KillUnit`.
#[tokio::test]
async fn a_cancelled_command_is_killed_with_its_scope() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let pidfile = r.path("pids");
    let cmd = argv(&[
        "sh",
        "-c",
        &format!(
            "sleep 30 & a=$!; setsid sleep 30 & echo $$ $a $! > '{}'; sleep 30",
            pidfile.display()
        ),
    ]);
    let pids = tokio::select! {
        _ = run(&r.launch, &cmd, None) => panic!("ended by itself"),
        p = read_when_there(&pidfile) => p,
    };
    // The select dropped the call's future: the cancellation.
    for pid in pids.split_whitespace() {
        gone(pid.parse().unwrap()).await;
    }
    let kills = r.state.lock().unwrap().kills.len();
    if r.cgroups() {
        assert_eq!(
            r.state.lock().unwrap().cgroups.len(),
            1,
            "the scope's cgroup"
        );
        assert_eq!(kills, 0, "the cgroup's kill, no KillUnit");
    } else {
        assert_eq!(kills, 1, "KillUnit on the scope");
    }
}

/// A command that leaves a writer in the background: the answer comes when the command ends, the
/// pipe is closed behind what it wrote, and the writer ends on its broken pipe instead of filling
/// anything.
#[tokio::test]
async fn a_background_writer_neither_holds_the_answer_nor_fills_anything() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let pidfile = r.path("yes.pid");
    let bounds = RunBounds {
        keep: Some(64 * 1024),
        stop_at: None,
    };
    let started = std::time::Instant::now();
    let ran = r
        .launch
        .run(
            &argv(&[
                "sh",
                "-c",
                &format!("yes & echo $! > '{}'; echo done >&2", pidfile.display()),
            ]),
            None,
            &bounds,
        )
        .await
        .unwrap();
    assert!(
        started.elapsed() < T,
        "the answer did not wait for the writer"
    );
    assert_eq!(ran.status, Status::Exited(0));
    assert_eq!(ran.stderr.head, b"done\n");
    assert!(ran.stdout.head.len() <= 64 * 1024);
    let pid: i32 = read_when_there(&pidfile).await.trim().parse().unwrap();
    gone(pid).await;
}

/// A command that ends by itself leaves what it started in the background running.
#[tokio::test]
async fn a_finished_command_leaves_its_background_running() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let ran = run(
        &r.launch,
        &argv(&["sh", "-c", "sleep 30 > /dev/null 2>&1 & echo $!"]),
        None,
    )
    .await
    .unwrap();
    assert_eq!(ran.status, Status::Exited(0));
    let bg: i32 = String::from_utf8(ran.stdout.head)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(bg));
    // SAFETY: the test's own sleep.
    unsafe {
        libc::kill(bg, libc::SIGKILL);
    }
    gone(bg).await;
}

/// The real size is always reported; beyond the bound (here the owner's `launch.output_max_mb`)
/// the start and the end are kept, the bound named; a signal is named.
#[tokio::test]
async fn output_beyond_the_bound_keeps_its_start_and_end() {
    let Some(r) = rig(LaunchConfig {
        output_max_mb: 1,
        ..Default::default()
    })
    .await
    else {
        return;
    };
    let ran = run(
        &r.launch,
        &argv(&[
            "sh",
            "-c",
            "echo START; head -c 3000000 /dev/zero | tr '\\0' x; echo; echo END; \
             echo e >&2; kill -9 $$",
        ]),
        None,
    )
    .await
    .unwrap();
    assert_eq!(ran.status, Status::Signalled(9));
    assert_eq!(ran.stdout.total, 6 + 3_000_000 + 1 + 4);
    assert_eq!(ran.stderr.total, 2);
    assert_eq!(ran.stdout.head.len(), 1024 * 1024);
    assert_eq!(ran.stdout.tail.len(), 512 * 1024 + 4);
    assert!(ran.stdout.head.starts_with(b"START\nxxx"));
    assert!(ran.stdout.tail.ends_with(b"xxx\nEND\n"));
    assert_eq!(
        ran.cap,
        Some((1024 * 1024, "launch.output_max_mb = 1".into()))
    );
    // The caller's smaller bound wins.
    let small = RunBounds {
        keep: Some(1000),
        stop_at: None,
    };
    let ran = r
        .launch
        .run(&argv(&["head", "-c", "5000", "/dev/zero"]), None, &small)
        .await
        .unwrap();
    assert_eq!(
        (
            ran.stdout.total,
            ran.stdout.head.len(),
            ran.stdout.tail.len()
        ),
        (5000, 1000, 504)
    );
}

/// Binary output end to end: kept within the room, then shown as the JSON the link carries, it
/// fits the room.
#[tokio::test]
async fn binary_output_fits_the_room_end_to_end() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let room = 20_000;
    let bounds = RunBounds {
        keep: Some(room),
        stop_at: None,
    };
    let ran = r
        .launch
        .run(
            &argv(&["head", "-c", "300000", "/dev/urandom"]),
            None,
            &bounds,
        )
        .await
        .unwrap();
    assert_eq!(ran.stdout.total, 300_000);
    let text = crate::desktop::tools::launch::ran_lines(&ran, Some(room), "20000 bytes");
    assert!(crate::host::limits::escaped_len(&text) <= room);
    assert!(text.contains("(cut to fit the tool link's message limit: 20000 bytes)"));
}

/// A scope job that never ends, and the call cancelled while Kai waits for it: the program never
/// runs, and the child waiting at its gate gives up.
#[tokio::test]
async fn a_cancel_during_the_scope_wait_runs_nothing() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    r.state.lock().unwrap().result = Some("never".into());
    let marker = r.path("ran");
    let cmd = argv(&["sh", "-c", &format!("touch '{}'", marker.display())]);
    tokio::select! {
        _ = run(&r.launch, &cmd, None) => panic!("answered without its scope"),
        () = r.until_scopes(1) => {}
    }
    // The select dropped the call while its scope's job was pending.
    let pid = Vec::<u32>::try_from(r.scopes()[0].props["PIDs"].try_clone().unwrap()).unwrap()[0];
    gone(pid as i32).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!marker.exists(), "the program ran");
}

/// Two launches at once, the first held at its gate (its scope's job pending): the second is not
/// held by it, and the first runs once its scope is there. (A spawn can wait for a sibling's gate only when
/// both fork at the same instant: module doc of `spawn`.)
#[tokio::test]
async fn a_launch_held_at_its_gate_does_not_hold_another() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    r.state.lock().unwrap().results = ["hold".to_owned()].into();
    let marker = r.path("first ran");
    let first = argv(&["sh", "-c", &format!("touch '{}'", marker.display())]);
    let held = run(&r.launch, &first, None);
    tokio::pin!(held);
    tokio::select! {
        _ = &mut held => panic!("answered without its scope"),
        () = r.until_scopes(1) => {}
    }
    let second = async {
        let ran = run(&r.launch, &argv(&["echo", "second"]), None)
            .await
            .unwrap();
        assert_eq!(ran.stdout.head, b"second\n");
    };
    tokio::select! {
        _ = &mut held => panic!("the first answered"),
        r = tokio::time::timeout(T, second) => r.expect("the second was held"),
    }
    assert!(!marker.exists());
    // Released, the first runs too.
    let unit = r.scopes()[0].unit.clone();
    let (ran, ()) = tokio::join!(held, r.release(&unit));
    assert_eq!(ran.unwrap().status, Status::Exited(0));
    assert!(marker.exists());
}

#[tokio::test]
async fn a_program_that_cannot_start_is_said_without_echoing_it() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let say = |e: Failure| e.to_string();
    let e = say(run(&r.launch, &argv(&["kai-s3cr3t-no-such"]), None)
        .await
        .unwrap_err());
    assert_eq!(e, "the program is not on PATH");
    let e = say(run(&r.launch, &argv(&["./s3cr3t"]), None)
        .await
        .unwrap_err());
    assert_eq!(e, "the program does not exist or is not executable");
    let e = say(run(&r.launch, &argv(&[]), None).await.unwrap_err());
    assert_eq!(e, "argv names no program");
    let e = say(run(&r.launch, &argv(&["true"]), Some("s3cr3t-dir"))
        .await
        .unwrap_err());
    assert_eq!(e, "cwd: no such folder");
    assert!(r.scopes().is_empty(), "no scope for what never started");
    // A relative program runs from the working directory.
    script(&r.home().join("bin/hello"), "echo hello");
    let ran = run(&r.launch, &argv(&["bin/hello"]), None).await.unwrap();
    assert_eq!(ran.stdout.head, b"hello\n");
}

/// A scope the manager does not start: the program never runs, and the error says so.
#[tokio::test]
async fn a_failed_scope_never_runs_the_program() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    r.state.lock().unwrap().result = Some("failed".into());
    let marker = r.path("ran");
    let e = run(
        &r.launch,
        &argv(&["sh", "-c", &format!("touch '{}'", marker.display())]),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "the user's service manager could not start the program's scope (failed)"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!marker.exists());
}

/// Without the manager's environment the program gets Kai's own, without Kai's variables.
#[test]
fn the_fallback_environment_drops_kais_variables() {
    let own = super::rig_own_env();
    let env = super::super::program_env(None, &own);
    assert!(env.iter().all(|(k, _)| !k.starts_with("KAI_")));
    assert!(env.iter().any(|(k, _)| k == "PATH"));
    let session = vec!["HOME=/h".to_owned(), "PATH=/s/bin".to_owned()];
    let env = super::super::program_env(Some(&session), &own);
    assert!(
        env.contains(&("PATH".into(), "/s/bin".into())),
        "the session's wins"
    );
    assert!(
        env.contains(&("WAYLAND_DISPLAY".into(), "kai-tests-none".into())),
        "what a program needs, from Kai's where the session lacks it"
    );
    assert!(env.iter().all(|(k, _)| k != "KAI_LOG"));
}

/// The rig's recording application: `<id>.desktop` whose program writes its arguments, one per
/// line, and then `$RIG_MARK`, into `<root>/<id>.args`.
fn recorder(r: &super::Rig, id: &str, exec_args: &str, extra: &str) -> std::path::PathBuf {
    let out = r.path(&format!("{id}.args"));
    let prog = r.path(&format!("bin/{id}"));
    script(
        &prog,
        &format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{0}.tmp'; echo \"mark $RIG_MARK $PWD\" >> \
             '{0}.tmp'; mv '{0}.tmp' '{0}'",
            out.display()
        ),
    );
    write(
        &r.path(&format!("data/applications/{id}.desktop")),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Rec {id}\nName[de]=Rek {id}\nExec={} \
             {exec_args}\n{extra}\n",
            prog.display()
        ),
    );
    out
}

/// `app_launch`: resolved by name, started in `app-<id>-….scope` in the home directory with the
/// session's environment, its files passed as the field code says; one process per file where it
/// takes one.
#[tokio::test]
async fn an_application_starts_in_its_scope_with_its_files() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let out = recorder(&r, "org.example.rec", "--open %U", "");
    let started = r
        .launch
        .app_launch(
            "rek org.example.rec",
            &argv(&["notes.txt", "https://example.com/x"]),
        )
        .await
        .unwrap();
    assert_eq!(started.id, "org.example.rec");
    assert_eq!(started.name, "Rek org.example.rec");
    assert_eq!(started.instances, 1);
    let rec = read_when_there(&out).await;
    assert_eq!(
        rec,
        format!(
            "--open\nfile://{}/notes.txt\nhttps://example.com/x\nmark 1 {}\n",
            r.home().display(),
            r.home().display()
        )
    );
    let scopes = r.scopes();
    assert!(
        scopes[0].unit.starts_with("app-org.example.rec-"),
        "{}",
        scopes[0].unit
    );
    assert_eq!(text(&scopes[0].props["Description"]), "Rek org.example.rec");

    let one = recorder(&r, "one", "%f", "Path=/");
    let started = r
        .launch
        .app_launch("one", &argv(&["/a", "/b"]))
        .await
        .unwrap();
    assert_eq!(started.instances, 2);
    assert_eq!(r.scopes().len(), 3, "a scope per process");
    let rec = read_when_there(&one).await;
    assert!(rec.ends_with("mark 1 /\n"), "its Path: {rec}");

    let e = r.launch.app_launch("rek", &[]).await.unwrap_err();
    assert!(matches!(e, Failure::Refused(_)), "{e:?}");
    assert!(e.to_string().starts_with("2 applications match"), "{e}");
}

/// An application that asks for a terminal starts in `launch.terminal`; with none named, or one
/// not installed, the error names the setting.
#[tokio::test]
async fn a_terminal_application_starts_in_the_configured_terminal() {
    let Some(mut r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let term_out = recorder(&r, "term", "", "");
    recorder(&r, "tui", "-x", "Terminal=true");
    r.launch.config.terminal = Some(vec![r.path("bin/term").display().to_string(), "-e".into()]);
    r.launch.app_launch("tui", &[]).await.unwrap();
    let rec = read_when_there(&term_out).await;
    let lines: Vec<&str> = rec.lines().collect();
    assert_eq!(lines[0], "-e", "{rec}");
    assert!(lines[1].ends_with("bin/tui"), "{rec}");
    assert_eq!(lines[2], "-x");
    r.launch.config.terminal = Some(vec!["kai-no-such-terminal".into()]);
    let e = r
        .launch
        .app_launch("tui", &[])
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "Rek tui runs in a terminal, and the terminal launch.terminal names is not installed"
    );
    r.launch.config.terminal = Some(vec![]);
    let e = r
        .launch
        .app_launch("tui", &[])
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "Rek tui runs in a terminal, and launch.terminal names none"
    );
}

/// `launch.terminal` unset: the application starts in the terminal Plasma's kdeglobals names (in
/// the session's XDG_CONFIG_HOME), followed by `-e`, as KIO starts it.
#[tokio::test]
async fn a_terminal_application_starts_in_plasmas_terminal_when_none_is_set() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let term_out = recorder(&r, "term", "", "");
    recorder(&r, "tui", "-x", "Terminal=true");
    write(
        &r.path("config/kdeglobals"),
        &format!(
            "[General]\nTerminalApplication={} --flag\n",
            r.path("bin/term").display()
        ),
    );
    r.launch.app_launch("tui", &[]).await.unwrap();
    let rec = read_when_there(&term_out).await;
    let lines: Vec<&str> = rec.lines().collect();
    assert_eq!(&lines[..2], ["--flag", "-e"], "{rec}");
    assert!(lines[2].ends_with("bin/tui"), "{rec}");
    assert_eq!(lines[3], "-x");
}

/// `open`: the type from the name, the default from `mimeapps.list`, the file passed; a URL by
/// its scheme; a missing file and a type nobody opens are said.
#[tokio::test]
async fn open_starts_the_default_application_for_the_type() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    write(&r.path("sys/mime/globs2"), "50:text/x-log:*.log\n");
    let text_out = recorder(&r, "org.example.editor", "%f", "MimeType=text/plain;");
    let web_out = recorder(&r, "org.example.browser", "%u", "");
    write(
        &r.path("config/kde-mimeapps.list"),
        "[Default Applications]\nx-scheme-handler/https=org.example.browser.desktop\n",
    );
    write(&r.home().join("today.log"), "a log\n");
    let started = r.launch.open("today.log").await.unwrap();
    assert_eq!(started.id, "org.example.editor");
    assert_eq!(started.mime.as_deref(), Some("text/x-log"));
    assert_eq!(
        started.notes,
        ["no default for text/x-log: the one for text/plain"]
    );
    let rec = read_when_there(&text_out).await;
    assert!(
        rec.starts_with(&format!("{}\n", r.home().join("today.log").display())),
        "{rec}"
    );
    let started = r.launch.open("HTTPS://example.com/").await.unwrap();
    assert_eq!(started.mime.as_deref(), Some("x-scheme-handler/https"));
    assert_eq!(
        read_when_there(&web_out).await.lines().next(),
        Some("https://example.com/")
    );
    let e = r.launch.open("s3cr3t.txt").await.unwrap_err().to_string();
    assert_eq!(
        e,
        "no such file or folder (a URL needs its scheme, https://…)"
    );
    std::fs::write(r.home().join("blob"), [0u8, 1]).unwrap();
    let e = r.launch.open("blob").await.unwrap_err().to_string();
    assert!(
        e.starts_with("no application opens application/octet-stream here"),
        "{e}"
    );
}

/// `open` never starts a program or a launcher, however it is named: a desktop entry, a script or
/// a binary without an extension; the error points at `run_command`. A colon in a file's name is
/// no URL scheme.
#[tokio::test]
async fn open_refuses_programs_and_launchers() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    write(
        &r.path("sys/mime/globs2"),
        "50:application/x-desktop:*.desktop\n50:text/x-log:*.log\n",
    );
    recorder(
        &r,
        "org.example.any",
        "%f",
        "MimeType=application/x-desktop;application/x-shellscript;application/x-executable;text/plain;",
    );
    write(
        &r.home().join("evil.desktop"),
        "[Desktop Entry]\nType=Application\nExec=sh -c 'touch pwned'\n",
    );
    write(&r.home().join("tool"), "#!/bin/sh\ntouch pwned\n");
    std::fs::copy("/bin/true", r.home().join("binary")).unwrap();
    for (name, kind) in [
        (
            "evil.desktop",
            "a desktop entry (a launcher) (application/x-desktop)",
        ),
        ("tool", "a shell script (application/x-shellscript)"),
        ("binary", "a program (application/x-executable)"),
    ] {
        let e = r.launch.open_plan(name).await.unwrap_err().to_string();
        assert_eq!(
            e,
            format!(
                "this is {kind}: open does not start programs or launchers; run it with \
                 run_command, which the owner confirms"
            )
        );
    }
    assert!(r.scopes().is_empty());
    // `notes:x.txt` is a file in the home directory, opened as one.
    write(&r.home().join("notes:x.txt"), "a note\n");
    let o = r.launch.open_plan("notes:x.txt").await.unwrap();
    assert_eq!(
        (o.id.as_str(), o.mime.as_str()),
        ("org.example.any", "text/plain")
    );
}

/// A desktop entry's `Path` that is not a folder is said, not replaced by the home directory.
#[tokio::test]
async fn an_entry_whose_path_is_no_folder_is_said() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    recorder(&r, "lost", "", "Path=/kai-no-such-dir");
    let e = r
        .launch
        .app_launch("lost", &[])
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "Rek lost's desktop entry names a working directory (Path) that is not a folder here"
    );
    assert!(r.scopes().is_empty());
}

/// An entry that runs a shell with the file inside its command (`sh -c "… %f"`) gets the file
/// as one word, whatever its name holds: a name built to add a command adds none.
#[tokio::test]
async fn a_file_name_cannot_add_a_command_to_an_entrys_shell() {
    let Some(r) = rig(LaunchConfig::default()).await else {
        return;
    };
    let out = r.path("shelled.args");
    let prog = r.path("bin/shelled");
    script(
        &prog,
        &format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{0}.tmp'; mv '{0}.tmp' '{0}'",
            out.display()
        ),
    );
    write(
        &r.path("data/applications/shelled.desktop"),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Shelled\nExec=sh -c \"{} %f\"\n",
            prog.display()
        ),
    );
    let pwned = r.home().join("pwned");
    // It runs in the home directory: `touch pwned` would make `pwned` there.
    let name = "x;touch pwned;'\"$(touch pwned)\".mp4";
    write(&r.home().join(name), "");
    r.launch
        .app_launch("shelled", &[r.home().join(name).display().to_string()])
        .await
        .unwrap();
    let rec = read_when_there(&out).await;
    assert_eq!(rec, format!("{}\n", r.home().join(name).display()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pwned.exists(), "the name added a command");
}
