//! The freedesktop Launch without the session: desktop entries, the MIME database and
//! `mimeapps.list` in temp directories under target/, and the programs (`true`, `echo`, `sh`,
//! `sleep`, a recording script) started through a fake of the user's service manager on a private
//! bus, which records each scope and ends its job as told. No GUI program is started, and the
//! session's own service manager is never asked (the real scope is `live_tests`', ignored by
//! default).
//!
//! The fake gives each scope a real cgroup v2 of its own where the test's cgroup lets it (the user
//! manager delegates the user's cgroups): a child cgroup of the test's own named as the unit, into
//! which it moves the scope's PIDs, as the manager does. So a drop's `cgroup.kill` and `KillUnit`
//! reach what the command started, `setsid` and reparented processes too; the rig kills what is
//! left in them and removes them when it ends. Where it cannot (no cgroup v2, a read-only one),
//! `KillUnit` kills the PIDs and their descendants, and `ManagerState::cgroups` stays empty. A test
//! that must not depend on the host sets `ManagerState::no_cgroups`, which makes the fake behave
//! like such a host.

mod entries;
mod mime;
mod run;
mod terminal;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::FdoLaunch;
use super::settings::LaunchConfig;
use super::spawn::{MANAGER_PATH, SYSTEMD, UserManager};
use crate::desktop::tests::bus::{PrivateBus, temp_dir};
use crate::host::registry::SessionBus;

pub const T: Duration = Duration::from_secs(5);

/// One scope the fake was asked for: its unit name and properties.
#[derive(Debug, Clone)]
pub struct Scope {
    pub unit: String,
    pub props: HashMap<String, OwnedValue>,
}

#[derive(Debug, Default)]
pub struct ManagerState {
    pub scopes: Vec<Scope>,
    /// The job's result: `done` unless a test says otherwise. `hold`: no `JobRemoved` until the
    /// test releases the job ([`Rig::release`]); `never`: none at all.
    pub result: Option<String>,
    /// Results for the next jobs, in order, before `result`.
    pub results: std::collections::VecDeque<String>,
    /// Jobs held: their id, path and unit.
    pub held: Vec<(u32, OwnedObjectPath, String)>,
    /// `KillUnit` calls: the unit, whom and the signal.
    pub kills: Vec<(String, String, i32)>,
    /// `Manager.Environment`.
    pub env: Vec<String>,
    /// The cgroups the fake made for its scopes: (unit, directory) (module doc).
    pub cgroups: Vec<(String, PathBuf)>,
    /// The fake makes no cgroup for its scopes, whatever the host allows (module doc).
    pub no_cgroups: bool,
}

/// The test's own cgroup v2 directory, if it may make child cgroups in it.
fn own_cgroup() -> Option<PathBuf> {
    let read = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = read.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let dir = Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/'));
    dir.join("cgroup.procs").exists().then_some(dir)
}

/// A cgroup for scope `unit` holding `pids` (module doc), or `None` where the test's cgroup does
/// not allow it.
fn scope_cgroup(unit: &str, pids: &[u32]) -> Option<PathBuf> {
    let dir = own_cgroup()?.join(unit);
    std::fs::create_dir(&dir).ok()?;
    for pid in pids {
        if std::fs::write(dir.join("cgroup.procs"), pid.to_string()).is_err() {
            let _ = std::fs::remove_dir(&dir);
            return None;
        }
    }
    Some(dir)
}

/// Kills what is left in `dir` and removes it, waiting (at most `T`) until it is empty.
fn remove_cgroup(dir: &Path) {
    let _ = std::fs::write(dir.join("cgroup.kill"), "1");
    let end = std::time::Instant::now() + T;
    while std::fs::remove_dir(dir).is_err() && dir.exists() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The processes below `pid` (by their parent), `pid` first.
fn descendants(pid: u32) -> Vec<u32> {
    let mut all = vec![pid];
    let mut i = 0;
    while i < all.len() {
        let parent = all[i];
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let ppid = std::fs::read_to_string(format!("/proc/{p}/stat"))
                .ok()
                .and_then(|s| {
                    s.rsplit_once(") ")
                        .and_then(|(_, r)| r.split(' ').nth(1).and_then(|x| x.parse::<u32>().ok()))
                });
            if ppid == Some(parent) && !all.contains(&p) {
                all.push(p);
            }
        }
        i += 1;
    }
    all
}

pub struct FakeManager {
    state: Arc<Mutex<ManagerState>>,
    next: u32,
}

impl FakeManager {
    pub fn new(state: Arc<Mutex<ManagerState>>) -> Self {
        Self { state, next: 0 }
    }
}

#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    async fn subscribe(&self) {}

    async fn start_transient_unit(
        &mut self,
        name: String,
        _mode: String,
        props: Vec<(String, OwnedValue)>,
        _aux: Vec<(String, Vec<(String, OwnedValue)>)>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.next += 1;
        let job = OwnedObjectPath::try_from(format!("/org/freedesktop/systemd1/job/{}", self.next))
            .unwrap();
        let props: HashMap<String, OwnedValue> = props.into_iter().collect();
        let pids = props
            .get("PIDs")
            .and_then(|v| Vec::<u32>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default();
        let result = {
            let mut s = self.state.lock().unwrap();
            let cgroup = if s.no_cgroups {
                None
            } else {
                scope_cgroup(&name, &pids)
            };
            if let Some(dir) = cgroup {
                s.cgroups.push((name.clone(), dir));
            }
            s.scopes.push(Scope {
                unit: name.clone(),
                props,
            });
            let r = s
                .results
                .pop_front()
                .or_else(|| s.result.clone())
                .unwrap_or_else(|| "done".into());
            if r == "hold" {
                s.held.push((self.next, job.clone(), name.clone()));
            }
            r
        };
        if result == "hold" || result == "never" {
            return Ok(job);
        }
        // Before the answer, as a fast manager may: the client watches before it asks.
        Self::job_removed(&emitter, self.next, job.clone(), name, result).await?;
        Ok(job)
    }

    /// Kills the scope's processes: its cgroup's (module doc), else its PIDs and everything below
    /// them, whatever their process group.
    async fn kill_unit(&self, name: String, whom: String, signal: i32) -> zbus::fdo::Result<()> {
        let pids: Vec<u32> = {
            let mut s = self.state.lock().unwrap();
            s.kills.push((name.clone(), whom, signal));
            if let Some((_, dir)) = s.cgroups.iter().find(|(u, _)| *u == name)
                && signal == libc::SIGKILL
                && std::fs::write(dir.join("cgroup.kill"), "1").is_ok()
            {
                return Ok(());
            }
            s.scopes
                .iter()
                .filter(|sc| sc.unit == name)
                .filter_map(|sc| Vec::<u32>::try_from(sc.props["PIDs"].try_clone().ok()?).ok())
                .flatten()
                .collect()
        };
        if pids.is_empty() {
            return Err(zbus::fdo::Error::Failed(format!("Unit {name} not loaded.")));
        }
        let all: Vec<u32> = pids.into_iter().flat_map(descendants).collect();
        for p in all {
            // SAFETY: kill on the test's own processes.
            unsafe {
                libc::kill(p as i32, signal);
            }
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn job_removed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        job: OwnedObjectPath,
        unit: String,
        result: String,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "Environment")]
    async fn environment(&self) -> Vec<String> {
        self.state.lock().unwrap().env.clone()
    }
}

/// A private bus with the fake manager, a home with its XDG directories, and the Launch over them.
pub struct Rig {
    _bus: PrivateBus,
    _held: zbus::Connection,
    pub state: Arc<Mutex<ManagerState>>,
    pub dir: tempfile::TempDir,
    pub launch: FdoLaunch,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let cgroups = std::mem::take(&mut self.state.lock().unwrap().cgroups);
        for (_, dir) in cgroups {
            remove_cgroup(&dir);
        }
    }
}

impl Rig {
    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.state.lock().unwrap().scopes.clone()
    }

    /// Waits (at most `T`) until the fake was asked for `n` scopes.
    pub async fn until_scopes(&self, n: usize) {
        let end = std::time::Instant::now() + T;
        while self.scopes().len() < n {
            assert!(
                std::time::Instant::now() < end,
                "only {} scopes",
                self.scopes().len()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Ends the held job of `unit` as `done`.
    pub async fn release(&self, unit: &str) {
        let (id, job) = {
            let mut s = self.state.lock().unwrap();
            let i = s.held.iter().position(|(_, _, u)| u == unit).expect("held");
            let (id, job, _) = s.held.remove(i);
            (id, job)
        };
        let iface = self
            ._held
            .object_server()
            .interface::<_, FakeManager>(MANAGER_PATH)
            .await
            .unwrap();
        FakeManager::job_removed(iface.signal_emitter(), id, job, unit.into(), "done".into())
            .await
            .unwrap();
    }
}

/// The session environment the fake hands out: the temp home and XDG directories (`data` the
/// user's, `sys` the system's, `config`), the test's PATH from Kai's own, KDE as the desktop,
/// German, and `RIG_MARK=1` (so a test sees the session's environment, not Kai's).
pub async fn rig(config: LaunchConfig) -> Option<Rig> {
    let bus = PrivateBus::start()?;
    let dir = temp_dir();
    for d in ["home", "data/applications", "config", "sys/applications"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    let p = |s: &str| dir.path().join(s).display().to_string();
    let state = Arc::new(Mutex::new(ManagerState {
        env: vec![
            format!("HOME={}", p("home")),
            format!("XDG_DATA_HOME={}", p("data")),
            format!("XDG_DATA_DIRS={}", p("sys")),
            format!("XDG_CONFIG_HOME={}", p("config")),
            format!("XDG_CONFIG_DIRS={}", p("sysconfig")),
            "XDG_CURRENT_DESKTOP=KDE".into(),
            "LANG=de_DE.UTF-8".into(),
            "RIG_MARK=1".into(),
        ],
        ..Default::default()
    }));
    let held = bus.connect().await;
    held.object_server()
        .at(
            MANAGER_PATH,
            FakeManager {
                state: state.clone(),
                next: 0,
            },
        )
        .await
        .unwrap();
    held.request_name(SYSTEMD).await.unwrap();
    let launch = FdoLaunch {
        manager: UserManager {
            bus: SessionBus::connected(bus.connect().await, T),
            timeout: T,
        },
        config,
        own_env: rig_own_env,
    };
    Some(Rig {
        _bus: bus,
        _held: held,
        state,
        dir,
        launch,
    })
}

/// Kai's own environment in the tests: the process's PATH, and a Kai variable that must never
/// reach a program.
pub fn rig_own_env() -> Vec<(String, String)> {
    vec![
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        ("KAI_LOG".into(), "debug".into()),
        ("WAYLAND_DISPLAY".into(), "kai-tests-none".into()),
    ]
}

/// Writes `text` to `path`, making its directory.
pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An executable shell script at `path`. It is written by `install` from a draft, so the file is
/// never open for writing in this process, whose children wait at the gate before `exec` holding
/// its descriptors ("text file busy" for another test's script otherwise).
pub fn script(path: &Path, body: &str) {
    let draft = path.with_extension("draft");
    write(&draft, &format!("#!/bin/sh\n{body}\n"));
    let ok = std::process::Command::new("install")
        .arg("-m755")
        .arg(&draft)
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "install {}", path.display());
}

/// Waits (at most `T`) until `path` holds a whole line, and reads it.
pub async fn read_when_there(path: &Path) -> String {
    let end = std::time::Instant::now() + T;
    loop {
        if let Ok(t) = std::fs::read_to_string(path)
            && t.ends_with('\n')
        {
            return t;
        }
        assert!(
            std::time::Instant::now() < end,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
