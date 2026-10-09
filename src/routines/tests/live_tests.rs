//! WP20's live check (scripts/live-check-routines.sh), ignored by default (`KAI_LIVE_ROUTINES=1`):
//! a routine's script in a real transient scope of the session's user manager calls the test's
//! `echo` through the real `kai` binary (`KAI_BIN`, which the script finds first on its `PATH`) and
//! this test's CLI socket under target/ (never the running Kai's); the real cgroup names the
//! caller `routine:live_probe`. Then the same binary from outside any routine scope is the caller
//! `cli`, and so is the same binary in a scope of the routine's name that Kai did not start
//! (`systemd-run --user --scope --unit=app-dev.rootshell.kai.routine.live_probe-forged…`); the
//! routine run from the terminal streams its script's stdout through `kai tool`. Nothing with a
//! window starts, every scope is gone when its process ends, and the session's running Kai is not
//! touched.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::Map;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::cli::server::{Backend, proc_cgroup};
use crate::desktop::fdo::launch::spawn::UserManager;
use crate::host::policy::Caller;
use crate::host::settings::{LevelSetting, ToolState};

fn routine_scopes() -> String {
    let out = std::process::Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--type=scope",
            "--all",
            "--no-legend",
            "app-dev.rootshell.kai.routine.*",
        ])
        .output()
        .expect("systemctl");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
#[ignore = "live: scripts/live-check-routines.sh"]
async fn live_routine_in_a_real_scope() {
    if std::env::var_os("KAI_LIVE_ROUTINES").is_none() {
        eprintln!("skipped: KAI_LIVE_ROUTINES is not set");
        return;
    }
    let kai = PathBuf::from(std::env::var("KAI_BIN").expect("KAI_BIN names the kai binary"));
    assert!(kai.is_file(), "{}", kai.display());
    let home = Home::new();
    home.routine(
        "live_probe",
        "description = \"Call echo through kai tool.\"\nkind = \"script\"\nread_only = true\n",
        Some("kai tool echo --text \"from $KAI_ROUTINE\"\ngrep -o 'app-[^/]*scope' /proc/self/cgroup >&2"),
    );
    home.switch_on("live_probe");
    // What its routine.toml asks (read_only: unasked for trusted callers; no level: any), granted
    // as the switch-on dialog's box would (K28): the owner runs it unasked, the terminal is asked.
    let (mut rig, _) = rig_with(
        home,
        PolicyConfig {
            trusted_callers: vec!["routine:live_probe".into()],
            tools: [("live_probe".to_owned(), ToolState::AllowTrusted)].into(),
            levels: [("live_probe".to_owned(), LevelSetting::Any)].into(),
            ..Default::default()
        },
    );
    let bus = zbus::Connection::session().await.expect("the session bus");
    rig.run_with_kai(
        Some(UserManager {
            bus: SessionBus::connected(bus, T),
            timeout: T,
        }),
        kai.parent().map(|p| p.to_path_buf()),
    );
    let server = tokio::spawn(crate::cli::server::serve(
        rig.home.socket(),
        Backend {
            local: rig.local.clone(),
            creds: rig.creds.subscribe(),
            client: None,
            cgroup_of: proc_cgroup,
            scopes: rig.scopes.clone(),
        },
    ));
    let end = Instant::now() + T;
    while !rig.home.socket().exists() {
        assert!(Instant::now() < end, "no socket");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let entry = rig.local.entry_any("live_probe").expect("listed");
    let r = rig
        .local
        .call(
            &entry,
            Some(Map::new()),
            &Caller::Owner,
            CancellationToken::new(),
        )
        .await;
    println!(
        "live: the routine answered {:?} (error: {})",
        text(&r),
        r.error
    );
    assert!(!r.error, "{}", text(&r));
    assert_eq!(text(&r), "echo: from live_probe");
    assert_eq!(
        rig.ran.all(),
        ["echo by routine 'live_probe': from live_probe"]
    );
    println!("live: echo ran as {:?}", rig.ran.all());
    // The terminal: the same binary, not in a routine's scope, is `cli` (untrusted: asked).
    let socket = rig.home.socket();
    let kai_bin = kai.clone();
    let terminal = tokio::task::spawn_blocking(move || {
        std::process::Command::new(kai_bin)
            .args(["tool", "echo", "--text", "from the terminal"])
            .env("KAI_CLI_SOCKET", socket)
            .env_remove("KAI_ROUTINE")
            .output()
            .expect("kai tool")
    });
    let q = rig.ask().await;
    println!("live: the terminal's call asks as {:?}", q.caller);
    assert_eq!(q.caller, "the command line (kai tool)");
    q.reply.send(true).unwrap();
    let out = terminal.await.unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "echo: from the terminal\n"
    );
    // A scope of the routine's name that Kai did not start (anyone's systemd-run) is `cli`.
    let unit = format!(
        "{}live_probe-forged{}.scope",
        crate::routines::unit_prefix(),
        std::process::id()
    );
    let socket = rig.home.socket();
    let kai_bin = kai.clone();
    let forged = tokio::task::spawn_blocking(move || {
        std::process::Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect"])
            .arg(format!("--unit={unit}"))
            .arg(kai_bin)
            .args(["tool", "echo", "--text", "forged"])
            .env("KAI_CLI_SOCKET", socket)
            .env_remove("KAI_ROUTINE")
            .output()
            .expect("systemd-run")
    });
    let q = rig.ask().await;
    println!("live: a forged routine scope asks as {:?}", q.caller);
    assert_eq!(q.caller, "the command line (kai tool)");
    q.reply.send(true).unwrap();
    let out = forged.await.unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "echo: forged\n");
    assert_eq!(
        rig.ran.all().last().map(String::as_str),
        Some("echo by the command line (kai tool): forged")
    );
    // The routine itself from the terminal: its script's stdout streams through `kai tool`.
    let socket = rig.home.socket();
    let kai_bin = kai.clone();
    let terminal = tokio::task::spawn_blocking(move || {
        std::process::Command::new(kai_bin)
            .args(["tool", "live_probe"])
            .env("KAI_CLI_SOCKET", socket)
            .env_remove("KAI_ROUTINE")
            .output()
            .expect("kai tool")
    });
    let q = rig.ask().await;
    assert_eq!(q.caller, "the command line (kai tool)");
    q.reply.send(true).unwrap();
    let out = terminal.await.unwrap();
    assert!(out.status.success(), "{out:?}");
    println!(
        "live: kai tool live_probe printed {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "echo: from live_probe\n"
    );
    server.abort();
    let end = Instant::now() + T;
    loop {
        let left = routine_scopes();
        if left.is_empty() {
            break;
        }
        assert!(Instant::now() < end, "routine scopes left: {left}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    println!("live: no routine scope is left");
}
