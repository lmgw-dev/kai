//! Starting one program in its own transient app scope on the user's service manager (K14), the
//! way Plasma's launcher does, but without its race: the child stops before `exec` until the
//! scope holds it.
//!
//! 1. Kai forks the program as its child (std's `Command`, its spawn on a blocking thread). Before
//!    `exec` the child starts a session of its own (`setsid`: a process group Kai can kill whole,
//!    no controlling terminal), marks every descriptor above 2 close-on-exec (`close_range`), sends
//!    its pid through a pipe and waits at a gate.
//! 2. Kai asks the user manager (`org.freedesktop.systemd1` on the session bus) for the scope
//!    (`StartTransientUnit`, `PIDs` the child's, `Slice=app.slice`, collected when it ends) and
//!    waits for the job's `JobRemoved`, bounded by `dbus.timeout_s`.
//! 3. Kai opens the gate and the child `exec`s; an `exec` failure comes back as std's error. A scope
//!    that failed closes the gate unopened: the child gives up (`ECANCELED`) and the scope's error
//!    is the answer.
//!
//! So the program is never in Kai's cgroup, survives Kai (nothing ties it to Kai's life: no
//! parent-death signal, a cgroup of its own), and inherits only stdin, stdout and stderr: none of
//! Kai's connections, and so none of the interfaces KWin grants Kai's binary (ruling 16).
//!
//! **Two spawns at one instant:** std's spawn learns of an `exec` failure through a pipe whose end
//! closes at `exec`. A sibling forked in the moment that end is open in Kai holds a copy of it until
//! its own `exec`, so the earlier spawn's answer then waits for the sibling's gate (at most
//! `dbus.timeout_s`, or until that call is cancelled). Only that moment: a launch held at its gate
//! holds no launch forked after it (`a_launch_held_at_its_gate_does_not_hold_another`). Closing
//! the copies in the child would need std's pipe, which the child cannot tell from the others.
//!
//! **A command's scope** ([`Scope`]) is killed whole at its time limit and when its call is
//! dropped: `KillUnit` with SIGKILL to every process in it (what left the process group with
//! `setsid` is still in the scope's cgroup), then the process group, which is all there is to
//! kill should the manager not answer. A drop kills at once, without the manager (a task spawned
//! from a drop dies with its runtime, so the command must not wait on it): it writes the scope's
//! cgroup v2 `cgroup.kill` (the user manager delegates the user's cgroups to the user), which
//! kills every process in it in one step, a `setsid` one and one reparented away from the leader
//! included, and none can fork past it. The scope's cgroup is read from the leader's
//! `/proc/<pid>/cgroup` once the scope holds it, and only one whose last part is the scope's unit
//! counts (never Kai's own). Without one (no cgroup v2, a write refused) the drop kills the process
//! group and then asks for `KillUnit`.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use kai_desktop::caps::Failure;
use kai_desktop::caps::launch::Captured;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio_util::sync::CancellationToken;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::host::registry::SessionBus;

pub const SYSTEMD: &str = "org.freedesktop.systemd1";
pub const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
pub const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// The user's service manager on the session bus.
#[derive(Clone)]
pub struct UserManager {
    pub bus: SessionBus,
    pub timeout: Duration,
}

impl UserManager {
    /// The manager's environment (`Manager.Environment`): what the session's programs get.
    pub async fn environment(&self) -> Result<Vec<String>, String> {
        let bus = self.bus.get().await?;
        let v: OwnedValue = crate::desktop::property(
            &bus,
            SYSTEMD,
            MANAGER_PATH,
            MANAGER,
            "Environment",
            self.timeout,
        )
        .await
        .map_err(|e| e.to_string())?;
        Vec::<String>::try_from(v).map_err(|e| e.to_string())
    }

    /// A transient scope `unit` holding `pid` (module doc, step 2).
    pub async fn start_scope(
        &self,
        unit: &str,
        description: &str,
        pid: u32,
    ) -> Result<(), Failure> {
        let secs = self.timeout.as_secs();
        let bus = self.bus.get().await.map_err(Failure::Failed)?;
        let work = async {
            // The job's end is watched before the job exists, so it cannot be missed.
            let owner = zbus::fdo::DBusProxy::new(&bus)
                .await?
                .get_name_owner(zbus::names::BusName::try_from(SYSTEMD)?)
                .await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(owner.as_str())?
                .interface(MANAGER)?
                .member("JobRemoved")?
                .path(MANAGER_PATH)?
                .build()
                .to_owned();
            let mut jobs = zbus::MessageStream::for_match_rule(rule, &bus, None).await?;
            // The manager sends job signals to subscribers; a second subscription of one
            // connection is refused, which changes nothing.
            let _ = bus
                .call_method(Some(SYSTEMD), MANAGER_PATH, Some(MANAGER), "Subscribe", &())
                .await;
            let props: Vec<(&str, Value)> = vec![
                ("Description", Value::from(description)),
                ("PIDs", Value::from(vec![pid])),
                ("Slice", Value::from("app.slice")),
                ("CollectMode", Value::from("inactive-or-failed")),
            ];
            let aux: Vec<(&str, Vec<(&str, Value)>)> = Vec::new();
            let reply = bus
                .call_method(
                    Some(SYSTEMD),
                    MANAGER_PATH,
                    Some(MANAGER),
                    "StartTransientUnit",
                    &(unit, "fail", props, aux),
                )
                .await?;
            let job: OwnedObjectPath = reply.body().deserialize()?;
            while let Some(msg) = jobs.next().await {
                let Ok(msg) = msg else { continue };
                if let Ok((_, path, _, result)) = msg
                    .body()
                    .deserialize::<(u32, OwnedObjectPath, String, String)>()
                    && path == job
                {
                    return Ok::<_, zbus::Error>(Some(result));
                }
            }
            Ok(None)
        };
        match tokio::time::timeout(self.timeout, work).await {
            Ok(Ok(Some(r))) if r == "done" => Ok(()),
            Ok(Ok(Some(r))) => Err(Failure::Failed(format!(
                "the user's service manager could not start the program's scope ({r})"
            ))),
            Ok(Ok(None)) => Err(Failure::Failed(
                "the session bus closed before the program's scope started".into(),
            )),
            Ok(Err(e)) => Err(Failure::Failed(format!(
                "the user's service manager refused the program's scope: {e}"
            ))),
            Err(_) => Err(Failure::Failed(format!(
                "the user's service manager did not start the program's scope within {secs} s \
                 (dbus.timeout_s)"
            ))),
        }
    }
}

impl UserManager {
    /// SIGKILL to every process of `unit` (`KillUnit`, whom `all`): what left the command's process
    /// group (`setsid`, a daemon) is still in its scope's cgroup.
    pub async fn kill_unit(&self, unit: &str) -> Result<(), String> {
        let bus = self.bus.get().await?;
        let body = (unit, "all", libc::SIGKILL);
        let call = bus.call_method(
            Some(SYSTEMD),
            MANAGER_PATH,
            Some(MANAGER),
            "KillUnit",
            &body,
        );
        match tokio::time::timeout(self.timeout, call).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!(
                "no answer within {} s (dbus.timeout_s)",
                self.timeout.as_secs()
            )),
        }
    }
}

/// A unit name's escaping (`systemd-escape`): only `[A-Za-z0-9:_.]` stay, a leading `.` and every
/// other byte become `\xNN`, so the app id survives the name (`-` separates its parts).
pub fn escape_unit(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b == b':' || b == b'_' || (b == b'.' && i > 0) {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// `app-<app id>-<random>.scope` (the systemd desktop-application naming the portals read an app
/// id from); the random part has no `-`.
pub fn scope_unit(app_id: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_default();
    format!(
        "app-{}-{:x}{:x}{:x}.scope",
        escape_unit(app_id),
        std::process::id(),
        nanos & 0xffff_ffff,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// One program to start.
pub struct Program {
    /// The executable, resolved.
    pub path: PathBuf,
    /// The arguments, `argv[0]` as written.
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// The whole environment it gets.
    pub env: Vec<(String, String)>,
    pub unit: String,
    /// The scope's description: never an argument (the journal logs it).
    pub description: String,
    pub stdout: Stdio,
    pub stderr: Stdio,
    /// Its scope is killed when the caller goes away (`run_command`): the [`Scope`] the spawn
    /// returns is armed, from the moment the scope holds the program.
    pub kill_on_drop: bool,
}

/// `close_range(2)`'s flag: mark close-on-exec instead of closing.
const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;

/// The child's side of the gate (module doc, step 1). Runs between `fork` and `exec`: only
/// async-signal-safe calls, no allocation. `opener` is Kai's end of the gate, which the fork
/// copied: closed here, so the child holds no writer of its own gate.
fn child_gate(info: RawFd, gate: RawFd, opener: RawFd) -> std::io::Result<()> {
    // SAFETY: close, setsid, syscall, getpid, write and read are async-signal-safe; the buffers
    // are on this stack frame.
    unsafe {
        libc::close(opener);
        if libc::setsid() < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Every descriptor of Kai's above stderr closes at exec (std's own error pipe already
        // does). A kernel without close_range (before 5.11) leaves them as they are: Kai opens
        // its own close-on-exec anyway.
        libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint,
            libc::c_uint::MAX,
            CLOSE_RANGE_CLOEXEC,
        );
        let pid = (libc::getpid() as u32).to_ne_bytes();
        if libc::write(info, pid.as_ptr().cast(), pid.len()) != pid.len() as isize {
            return Err(std::io::Error::last_os_error());
        }
        let mut byte = 0u8;
        loop {
            let n = libc::read(gate, (&raw mut byte).cast(), 1);
            if n == 1 && byte == 1 {
                return Ok(());
            }
            if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(std::io::Error::from_raw_os_error(libc::ECANCELED));
        }
    }
}

/// Starts `p` in its scope (module doc). The child is returned once it `exec`ed, with its scope
/// (armed where `p.kill_on_drop` says).
pub async fn spawn(
    manager: &UserManager,
    p: Program,
) -> Result<(tokio::process::Child, Scope), Failure> {
    let (mut info_r, info_w) =
        std::io::pipe().map_err(|e| Failure::Failed(format!("cannot make a pipe: {e}")))?;
    let (gate_r, gate_w) =
        std::io::pipe().map_err(|e| Failure::Failed(format!("cannot make a pipe: {e}")))?;
    let opener_fd = gate_w.as_raw_fd();
    let mut gate = Gate(Some(gate_w));
    let mut cmd = tokio::process::Command::new(&p.path);
    cmd.arg0(p.argv.first().map(String::as_str).unwrap_or_default())
        .args(p.argv.iter().skip(1))
        .current_dir(&p.cwd)
        .env_clear()
        .envs(p.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(p.stdout)
        .stderr(p.stderr);
    let (info_fd, gate_fd) = (info_w.as_raw_fd(), gate_r.as_raw_fd());
    // SAFETY: child_gate is async-signal-safe (its doc).
    unsafe {
        cmd.pre_exec(move || child_gate(info_fd, gate_fd, opener_fd));
    }
    // std's spawn returns once the child exec'ed or failed, so it waits for the gate: on a
    // blocking thread, while this task opens the gate.
    let mut spawner = tokio::task::spawn_blocking(move || {
        let r = cmd.spawn();
        drop(info_w);
        drop(gate_r);
        r
    });
    let reader = tokio::task::spawn_blocking(move || {
        let mut b = [0u8; 4];
        info_r
            .read_exact(&mut b)
            .ok()
            .map(|()| u32::from_ne_bytes(b))
    });
    // The pid, or the spawn's early end: a child that failed before the gate (its working
    // directory, say), whose pipe end a sibling forked meanwhile may still hold.
    let (pid, early) = tokio::select! {
        pid = reader => (pid.ok().flatten(), None),
        spawned = &mut spawner => (None, Some(spawned)),
    };
    let scoped = match pid {
        Some(pid) => manager.start_scope(&p.unit, &p.description, pid).await,
        None => Ok(()),
    };
    // From here a dropped call kills the scope (a `run_command` cancelled while it `exec`s).
    let armed = p.kill_on_drop && pid.is_some() && scoped.is_ok();
    let mut scope = Scope {
        manager: manager.clone(),
        unit: p.unit.clone(),
        pgid: pid.map(|p| p as i32).unwrap_or(0),
        // The child waits at the gate, in its scope now.
        cgroup: pid.filter(|_| armed).and_then(|pid| {
            let read = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
            let cg = scope_cgroup(&read, &p.unit);
            if cg.is_none() {
                tracing::info!(unit = %p.unit, "launch: the scope's cgroup is not Kai's to kill (no cgroup v2 path naming it); a drop kills the process group");
            }
            cg
        }),
        armed,
    };
    gate.answer(scoped.is_ok());
    let spawned = match early {
        Some(s) => s,
        None => spawner.await,
    }
    .map_err(|e| Failure::Failed(format!("the spawn did not finish: {e}")))?;
    match (spawned, scoped) {
        (Ok(child), Ok(())) => Ok((child, scope)),
        (Ok(mut child), Err(e)) => {
            let _ = child.start_kill();
            Err(e)
        }
        (Err(_), Err(e)) => Err(e),
        (Err(e), Ok(())) => {
            // It never ran: nothing to kill.
            scope.disarm();
            Err(Failure::Refused(spawn_error(&e)))
        }
    }
}

/// Kai's end of the gate: it says go (1) or give up (0), and gives up when dropped unanswered (a
/// cancelled call). An explicit answer, not the pipe's end: a sibling child forked meanwhile may
/// hold a copy of this end until its own `exec`, so the end of the pipe can come late.
struct Gate(Option<std::io::PipeWriter>);

impl Gate {
    fn answer(&mut self, go: bool) {
        if let Some(mut w) = self.0.take() {
            let _ = w.write_all(&[u8::from(go)]);
        }
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.answer(false);
    }
}

fn spawn_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::ENOENT) => "the program or the working directory does not exist".into(),
        Some(libc::EACCES) => "the program is not executable or the working directory not \
                               accessible"
            .into(),
        Some(libc::ENOEXEC) => "the program is not in a format this system runs".into(),
        _ => format!("the program could not start: {e}"),
    }
}

/// A command's scope, which Kai kills (module doc) at its time limit and when it is dropped armed
/// (the call cancelled).
pub struct Scope {
    manager: UserManager,
    unit: String,
    pgid: i32,
    /// The scope's cgroup directory (module doc), when Kai found it.
    cgroup: Option<PathBuf>,
    armed: bool,
}

impl Scope {
    fn kill_group(pgid: i32) {
        if pgid > 1 {
            // SAFETY: kill with a negative pid > 1 signals that process group only.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }

    /// Kills everything in the scope, once.
    pub async fn kill(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if let Err(e) = self.manager.kill_unit(&self.unit).await {
            tracing::warn!(unit = %self.unit, "launch: KillUnit failed ({e}); the process group is killed");
        }
        Self::kill_group(self.pgid);
    }

    /// The command ended by itself: what it left running in the background stays.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        tracing::info!(unit = %self.unit, "launch: run_command was cancelled; its scope is killed");
        // Here and now, not in a task: a task spawned from a drop is cancelled when the runtime
        // shuts down (Kai quitting mid run), and the command would outlive it. The scope's cgroup
        // first: every process in it, in one step (module doc).
        if let Some(cg) = &self.cgroup {
            match std::fs::write(cg.join("cgroup.kill"), "1") {
                Ok(()) => return,
                // The scope's cgroup went: everything in it ended already.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(e) => tracing::warn!(
                    unit = %self.unit,
                    "launch: the scope's cgroup.kill was refused ({e}); the process group is killed"
                ),
            }
        }
        // The leader started its own session, so its pid is the group's id.
        Self::kill_group(self.pgid);
        // Then `KillUnit` for what left the group (`setsid`, a daemon): still in the scope's
        // cgroup. Best effort; the group is dead already.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let (manager, unit) = (self.manager.clone(), self.unit.clone());
            rt.spawn(async move {
                if let Err(e) = manager.kill_unit(&unit).await {
                    tracing::warn!(%unit, "launch: KillUnit failed ({e}); only the process group was killed");
                }
            });
        }
    }
}

/// The cgroup v2 directory of scope `unit` from a process's `/proc/<pid>/cgroup` (`read`): its
/// unified line (`0::<path>`) under `/sys/fs/cgroup`, only when the path's last part is `unit` (the
/// process is in that scope, so the directory is the scope's and never Kai's own).
fn scope_cgroup(read: &str, unit: &str) -> Option<PathBuf> {
    let path = read.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let rel = path.strip_prefix('/')?;
    let dir = std::path::Path::new(rel);
    if unit.is_empty()
        || dir.file_name()? != unit
        || dir
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(std::path::Path::new("/sys/fs/cgroup").join(dir))
}

/// What Kai keeps of one output stream: its first `keep` bytes, its last `keep / 2` after those,
/// and its real size (`None`: all of it). In memory and bounded, so a runaway command fills neither
/// RAM nor a disk.
pub struct Keep {
    head: Vec<u8>,
    head_max: Option<usize>,
    tail: VecDeque<u8>,
    tail_max: usize,
    total: u64,
}

impl Keep {
    pub fn new(keep: Option<u64>) -> Self {
        let head_max = keep.map(|k| usize::try_from(k).unwrap_or(usize::MAX));
        Self {
            head: Vec::new(),
            head_max,
            tail: VecDeque::new(),
            tail_max: head_max.map(|h| h / 2 + 4).unwrap_or(0),
            total: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let room = self
            .head_max
            .map_or(bytes.len(), |m| m.saturating_sub(self.head.len()));
        let (to_head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(to_head);
        if rest.is_empty() {
            return;
        }
        let rest = &rest[rest.len().saturating_sub(self.tail_max)..];
        self.tail.extend(rest);
        let over = self.tail.len().saturating_sub(self.tail_max);
        self.tail.drain(..over);
    }

    pub fn captured(self) -> Captured {
        Captured {
            total: self.total,
            head: self.head,
            tail: self.tail.into(),
        }
    }
}

/// The bytes waiting in the pipe `fd` (`FIONREAD`).
pub fn waiting(fd: RawFd) -> usize {
    let mut n: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int to the pointer, which lives on this frame.
    let r = unsafe { libc::ioctl(fd, libc::FIONREAD, &raw mut n) };
    if r < 0 { 0 } else { n.max(0) as usize }
}

/// Reads one of a command's pipes into `keep` until its end, or until `leader_gone`: then what
/// waits in the pipe at that moment (all the leader wrote) is read and the pipe closed, so what the
/// command left in the background gets EPIPE instead of holding the answer or filling anything.
pub async fn read_pipe<R: AsyncRead + AsRawFd + Unpin>(
    mut pipe: R,
    mut keep: Keep,
    leader_gone: CancellationToken,
) -> Keep {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        tokio::select! {
            biased;
            () = leader_gone.cancelled() => break,
            r = pipe.read(&mut buf) => match r {
                Ok(0) | Err(_) => return keep,
                Ok(n) => keep.push(&buf[..n]),
            },
        }
    }
    let mut left = waiting(pipe.as_raw_fd());
    while left > 0 {
        let want = left.min(buf.len());
        match pipe.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                keep.push(&buf[..n]);
                left -= n;
            }
        }
    }
    keep
}

/// A journal stream (what `systemd-cat -t <identifier>` makes) for a launched app's stdout and
/// stderr, so its messages are its own in the journal and not Kai's; `None` without journald. The
/// identifier is the stream's header line: control and format characters are left out of it.
pub fn journal_stream(identifier: &str) -> Option<OwnedFd> {
    let identifier = crate::host::confirm::plain(identifier);
    let mut s = std::os::unix::net::UnixStream::connect("/run/systemd/journal/stdout").ok()?;
    // identifier, unit id, priority (info), level prefix, to syslog, to kmsg, to console.
    s.write_all(format!("{identifier}\n\n6\n1\n0\n0\n0\n").as_bytes())
        .ok()?;
    s.shutdown(std::net::Shutdown::Read).ok()?;
    Some(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scope's cgroup counts only when its path names the scope: never Kai's own, a v1 line or
    /// a path that climbs.
    #[test]
    fn the_scopes_cgroup_is_the_one_that_names_it() {
        let unit = "app-dev.rootshell.kai.run-1a2b.scope";
        let read = format!(
            "1:net_cls:/\n0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n"
        );
        assert_eq!(
            scope_cgroup(&read, unit),
            Some(PathBuf::from(format!(
                "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}"
            )))
        );
        let kais = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-kai.scope\n";
        assert_eq!(scope_cgroup(kais, unit), None, "Kai's own cgroup");
        assert_eq!(
            scope_cgroup(&format!("1:name=systemd:/{unit}\n"), unit),
            None
        );
        assert_eq!(scope_cgroup(&format!("0::/a/../{unit}\n"), unit), None);
        assert_eq!(scope_cgroup("0::/\n", unit), None);
        assert_eq!(scope_cgroup(&format!("0::/{unit}\n"), ""), None);
    }

    #[test]
    fn unit_names_keep_the_app_id_whole() {
        assert_eq!(escape_unit("org.kde.dolphin"), "org.kde.dolphin");
        assert_eq!(escape_unit("kde-open"), "kde\\x2dopen");
        assert_eq!(escape_unit(".x y/é"), "\\x2ex\\x20y\\x2f\\xc3\\xa9");
        let u = scope_unit("org.kde.kate");
        let rand = u
            .strip_prefix("app-org.kde.kate-")
            .unwrap()
            .strip_suffix(".scope")
            .unwrap();
        assert!(!rand.is_empty() && !rand.contains('-'), "{u}");
        assert_ne!(scope_unit("a"), scope_unit("a"));
    }

    #[test]
    fn a_stream_keeps_its_start_and_its_end_within_the_bound() {
        let mut k = Keep::new(Some(10));
        k.push(b"0123456");
        k.push(b"789abcdefghij");
        k.push(b"klmnop");
        let c = k.captured();
        assert_eq!(c.total, 26);
        assert_eq!(c.head, b"0123456789");
        assert_eq!(c.tail, b"hijklmnop", "keep / 2 + 4 of the end");
        let mut all = Keep::new(None);
        all.push(b"abc");
        all.push(b"def");
        let c = all.captured();
        assert_eq!(
            (c.head.as_slice(), c.tail.len(), c.cut()),
            (&b"abcdef"[..], 0, 0)
        );
        let mut small = Keep::new(Some(10));
        small.push(b"0123456789ab");
        let c = small.captured();
        assert_eq!(
            (c.tail.as_slice(), c.cut()),
            (&b"ab"[..], 0),
            "nothing lost in between"
        );
    }
}
