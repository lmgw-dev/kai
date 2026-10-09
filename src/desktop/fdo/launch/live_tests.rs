//! WP16a's live checks (scripts/live-check-launch.sh), ignored by default:
//!
//! - **K14 on the real user manager** (`KAI_LIVE_SCOPE=1`): a stand-in for Kai (this test binary
//!   run again as [`live_scope_helper`]) starts `sleep` as an application and as a command in real
//!   transient scopes of the session's user manager; the test kills the stand-in with SIGKILL and
//!   checks the application still runs, in its own `app-….scope` below `app.slice`, with only
//!   stdin, stdout and stderr and no `KAI_*` variable; then it stops it. A command stopped at its
//!   time limit has its whole scope killed (`KillUnit`), a `sleep` that left its process group
//!   (`setsid`) too. Nothing with a window is started; nothing else of the session is touched.
//! - **GUI applications in a nested KWin** (`KAI_LIVE_NESTED=1`, scripts/nested-kwin.sh): a fake
//!   user manager on the nested private bus (the session's manager is not on it), desktop entries
//!   and `mimeapps.list` in the nested XDG directories; `app_launch` shows a kdialog message box,
//!   `open` a text file in a kdialog text box (the default for `text/plain`); both windows are
//!   found in the nested KWin's window list, then closed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kai_desktop::caps::Capability;
use kai_desktop::caps::launch::{Launch, RunBounds, Status};

use super::entries::Entry;
use super::settings::LaunchConfig;
use super::spawn::{MANAGER_PATH, SYSTEMD, UserManager};
use super::tests::{FakeManager, ManagerState};
use super::{FdoLaunch, process_env};
use crate::host::registry::SessionBus;

const T: Duration = Duration::from_secs(10);
const HELPER: &str = "KAI_LIVE_SCOPE_HELPER";

fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(") ").and_then(|(_, r)| r.chars().next()))
        .is_some_and(|c| c != 'Z' && c != 'X')
}

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let end = Instant::now() + T;
    while !ok() {
        assert!(Instant::now() < end, "{what}: not within {} s", T.as_secs());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kills the live check's own `sleep` when dropped, however the check ends.
struct Stop(i32);

impl Drop for Stop {
    fn drop(&mut self) {
        // SAFETY: the live check's own sleep.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
        }
    }
}

/// The launch over the session's real user manager.
async fn real_launch() -> FdoLaunch {
    FdoLaunch {
        manager: UserManager {
            bus: SessionBus::connected(zbus::Connection::session().await.unwrap(), T),
            timeout: T,
        },
        config: LaunchConfig::default(),
        own_env: process_env,
    }
}

/// The stand-in for Kai: starts `sleep` as an application (its pid into `<dir>/pid`), then waits to
/// be killed.
#[tokio::test]
#[ignore = "run by live_scope_survives_kai only"]
async fn live_scope_helper() {
    let Some(dir) = std::env::var_os(HELPER).map(PathBuf::from) else {
        return;
    };
    let launch = real_launch().await;
    let s = launch.session().await;
    let entry = Entry {
        id: "dev.rootshell.kai.livecheck".into(),
        name: "Kai live check".into(),
        exec: format!(
            "sh -c \"echo \\$\\$ > '{}'; exec sleep 120\"",
            dir.join("pid").display()
        ),
        ..Default::default()
    };
    let started = launch.start(&s, &entry, &[]).await.unwrap();
    println!("helper: started {}", started.id);
    tokio::time::sleep(Duration::from_secs(120)).await;
}

#[tokio::test]
#[ignore = "live: real transient scopes on the session's user manager (just live-check-launch)"]
async fn live_scope_survives_kai() {
    if std::env::var("KAI_LIVE_SCOPE").as_deref() != Ok("1") {
        println!("skipped: KAI_LIVE_SCOPE is not 1");
        return;
    }
    let dir = crate::desktop::tests::bus::temp_dir();
    // A command first: its own scope, its output.
    let launch = real_launch().await;
    let ran = launch
        .run(
            &["sh".into(), "-c".into(), "cat /proc/self/cgroup".into()],
            None,
            &Default::default(),
        )
        .await
        .unwrap();
    let cgroup = String::from_utf8_lossy(&ran.stdout.head).into_owned();
    println!(
        "command: {:?} in {}",
        ran.status,
        cgroup
            .lines()
            .find(|l| l.starts_with("0::"))
            .unwrap_or_default()
    );
    assert_eq!(ran.status, Status::Exited(0));
    assert!(
        cgroup.contains("/app.slice/app-dev.rootshell.kai.run-"),
        "{cgroup}"
    );
    // A command stopped at its limit: KillUnit kills what left its process group too.
    let escaped = dir.path().join("escaped");
    let bounds = RunBounds {
        keep: None,
        stop_at: Some((
            Instant::now() + Duration::from_secs(1),
            "the live check's limit".into(),
        )),
    };
    let ran = launch
        .run(
            &[
                "sh".into(),
                "-c".into(),
                format!(
                    "setsid sleep 60 & echo $! > '{}'; sleep 60",
                    escaped.display()
                ),
            ],
            None,
            &bounds,
        )
        .await
        .unwrap();
    assert_eq!(
        ran.status,
        Status::TimedOut {
            setting: "the live check's limit".into()
        }
    );
    let gone_pid: i32 = std::fs::read_to_string(&escaped)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _stop_escaped = Stop(gone_pid);
    wait_for("the escaped sleep's end", || !alive(gone_pid));
    println!("command: stopped at its limit, the sleep that left its process group killed too");
    // An application from a stand-in for Kai, which is then killed.
    let helper = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "desktop::fdo::launch::live_tests::live_scope_helper",
            "--nocapture",
        ])
        .env(HELPER, dir.path())
        .env("KAI_LIVE_MARK", "1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    struct Reap(std::process::Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut helper = Reap(helper);
    let pidfile = dir.path().join("pid");
    wait_for("the application's pid", || {
        std::fs::read_to_string(&pidfile).is_ok_and(|t| t.ends_with('\n'))
    });
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _stop = Stop(pid);
    wait_for("sleep", || {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "sleep")
    });
    // SIGKILL to the stand-in, as `kill -9` of Kai.
    helper.0.kill().unwrap();
    helper.0.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(pid), "the application died with the stand-in");
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
    println!(
        "application: alive after kill -9 of its starter, in {}",
        cgroup
            .lines()
            .find(|l| l.starts_with("0::"))
            .unwrap_or_default()
    );
    assert!(
        cgroup.contains("/app.slice/app-dev.rootshell.kai.livecheck-"),
        "{cgroup}"
    );
    let mut fds: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fds.sort();
    assert_eq!(fds, ["0", "1", "2"]);
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    assert!(
        !environ.split(|b| *b == 0).any(|v| v.starts_with(b"KAI_")),
        "a KAI_ variable reached it"
    );
    println!("application: fds 0 1 2, no KAI_ variable; stopping it");
    drop(_stop);
    wait_for("the application's end", || !alive(pid));
    println!("live scope check: ok");
}

/// The nested KWin's directory, or why this is not one (the WP14 live check's rules).
fn nested() -> Result<PathBuf, String> {
    if std::env::var("KAI_LIVE_NESTED").as_deref() != Ok("1") {
        return Err("KAI_LIVE_NESTED is not 1".into());
    }
    let log = std::env::var_os("KAI_NESTED_KWIN_LOG").ok_or("no KAI_NESTED_KWIN_LOG")?;
    let dir = Path::new(&log).parent().ok_or("no directory")?.to_owned();
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    if Path::new(&display) != dir.join("run/kai-nested") {
        return Err("WAYLAND_DISPLAY is not the nested KWin's".into());
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let private = std::fs::read_to_string(dir.join("bus")).unwrap_or_default();
    if bus.is_empty() || bus != private.trim() {
        return Err("not the nested KWin's private bus".into());
    }
    Ok(dir)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
#[ignore = "live: GUI applications in a nested KWin (just live-check-launch)"]
async fn live_launch_in_nested_kwin() {
    if let Err(e) = nested() {
        println!("skipped: {e}");
        return;
    }
    let env_of = |k: &str| PathBuf::from(std::env::var(k).unwrap());
    let data = env_of("XDG_DATA_HOME");
    let config = env_of("XDG_CONFIG_HOME");
    let home = env_of("HOME");
    write(
        &data.join("applications/kai-live-msg.desktop"),
        "[Desktop Entry]\nType=Application\nName=Kai Live Message\n\
         Exec=kdialog --title \"Kai live launch\" --msgbox \"started by app_launch\"\n",
    );
    write(
        &data.join("applications/kai-live-text.desktop"),
        "[Desktop Entry]\nType=Application\nName=Kai Live Text\n\
         Exec=kdialog --title \"Kai live open\" --textbox %f\nMimeType=text/plain;\n",
    );
    write(
        &config.join("mimeapps.list"),
        "[Default Applications]\ntext/plain=kai-live-text.desktop\n",
    );
    write(&home.join("kai-live.txt"), "opened by open\n");
    // The fake user manager on the nested bus, handing out the nested session's environment.
    let mut env: Vec<String> = std::env::vars()
        .filter(|(k, _)| !k.starts_with("KAI_"))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    env.push("QT_QPA_PLATFORM=wayland".into());
    env.push("XDG_CURRENT_DESKTOP=KDE".into());
    let state = Arc::new(Mutex::new(ManagerState {
        env,
        ..Default::default()
    }));
    let held = zbus::Connection::session().await.unwrap();
    held.object_server()
        .at(MANAGER_PATH, FakeManager::new(state.clone()))
        .await
        .unwrap();
    held.request_name(SYSTEMD).await.unwrap();
    let launch = FdoLaunch {
        manager: UserManager {
            bus: SessionBus::connected(zbus::Connection::session().await.unwrap(), T),
            timeout: T,
        },
        config: LaunchConfig::default(),
        own_env: process_env,
    };
    // The nested KWin's windows, through WP14's provider.
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    let desktop_env = crate::desktop::Env {
        current_desktop: "KDE".into(),
        session: Some(SessionBus::lazy(T)),
        system: Some(crate::desktop::SystemBus::new(Some(bus), T)),
        wayland: true,
        timeout: T,
        runtime_dir: env_of("XDG_RUNTIME_DIR").join("kai"),
        ..crate::desktop::Env::inert()
    };
    let desktop = crate::desktop::DesktopTools::new(
        desktop_env,
        Default::default(),
        crate::host::registry::Changes::new().0,
        false,
    );
    desktop.probe_now().await;
    let windows = desktop
        .chosen()
        .into_iter()
        .find(|c| c.capability == Capability::Windows)
        .and_then(|c| match c.service {
            Some(crate::desktop::probe::Service::Windows(w)) => Some(w),
            _ => None,
        })
        .expect("the nested KWin's windows");
    struct Kill(Vec<u32>);
    impl Drop for Kill {
        fn drop(&mut self) {
            for p in &self.0 {
                // SAFETY: the live check's own kdialogs.
                unsafe {
                    libc::kill(*p as i32, libc::SIGTERM);
                }
            }
        }
    }
    let mut kill = Kill(Vec::new());
    for (what, title) in [("app_launch", "Kai live launch"), ("open", "Kai live open")] {
        let started = match what {
            "app_launch" => launch.app_launch("kai live message", &[]).await,
            _ => launch.open("kai-live.txt").await,
        }
        .unwrap();
        println!(
            "{what}: started {} ({}) as {:?}",
            started.name, started.id, started.mime
        );
        let end = Instant::now() + T;
        let w = loop {
            let snap = windows.snapshot().await.unwrap();
            if let Some(w) = snap.windows.into_iter().find(|w| w.title.contains(title)) {
                break w;
            }
            assert!(Instant::now() < end, "{what}: no window titled {title:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        println!(
            "{what}: window {} {:?} (app {}), pid {}",
            w.id, w.title, w.app, w.pid
        );
        kill.0.push(w.pid);
    }
    let scopes = state.lock().unwrap().scopes.clone();
    for s in &scopes {
        println!("scope {}", s.unit);
    }
    assert!(scopes[0].unit.starts_with("app-kai\\x2dlive\\x2dmsg-"));
    assert!(scopes[1].unit.starts_with("app-kai\\x2dlive\\x2dtext-"));
    drop(kill);
    println!("live launch check: ok");
}
