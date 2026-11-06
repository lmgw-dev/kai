//! Kai's local sockets in its runtime folder (`$XDG_RUNTIME_DIR/kai`): the CLI's `cli.sock`
//! (`crate::cli`) and Kai for Firefox's `browser.sock` (`crate::browser`). Each is bound in the
//! folder, which Kai makes private at its start (`crate::paths::private_dir`, mode 0700), with mode
//! 0600 itself, removed when its server ends, and each reads the process that connected through its
//! pidfd.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixSocket, UnixStream};

/// Removes the socket when the server ends (Kai quits).
pub struct Unlink(pub PathBuf);

impl Drop for Unlink {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The socket at `path`: its folder made where it is missing (mode 0700), an old socket of an
/// earlier Kai removed (one Kai runs per session), mode 0600.
///
/// The mode is set between `bind` and `listen`: `bind` makes the file with the umask's mode, and
/// until `listen` no process can connect to it, so the moment it has that mode lets nobody in. A
/// umask set around `bind` would be the process's for that moment, every other thread's file
/// creations included.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if !m.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "something that is not a socket is in its place",
            ));
        }
        std::fs::remove_file(path)?;
    }
    let socket = UnixSocket::new_stream()?;
    socket.bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    // u32::MAX goes to listen(2) as -1, the kernel's own maximum (net.core.somaxconn), what
    // UnixListener::bind asks for.
    socket.listen(u32::MAX)
}

/// The peer of `stream` as a process: a pidfd from `SO_PEERPIDFD` (Linux 6.5 and later); a kernel
/// without it says `ENOPROTOOPT`.
fn peer_pidfd(stream: &UnixStream) -> std::io::Result<OwnedFd> {
    let mut fd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: SO_PEERPIDFD writes one int (a new file descriptor) to the pointer, which lives on
    // this frame, and its length to `len`.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERPIDFD,
            (&raw mut fd).cast(),
            &raw mut len,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if fd < 0 {
        return Err(std::io::Error::other("SO_PEERPIDFD gave no descriptor"));
    }
    // SAFETY: on success the kernel made `fd` for us; nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The pid a pidfd names now (`Pid:` in its fdinfo); `None` once the process is gone.
fn pidfd_pid(pidfd: &OwnedFd) -> Option<i32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd())).ok()?;
    info.lines()
        .find_map(|l| l.strip_prefix("Pid:"))
        .and_then(|p| p.trim().parse::<i32>().ok())
        .filter(|p| *p > 0)
}

/// The process of `pidfd` has not exited (a pidfd polls readable once it has).
fn pidfd_alive(pidfd: &OwnedFd) -> bool {
    let mut p = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd on this frame, no wait.
    let r = unsafe { libc::poll(&raw mut p, 1, 0) };
    r == 0
}

/// The process that connected, as its socket names it, taken while the connection is at hand
/// and read later ([`PeerProcess::read`], which may run off the async runtime).
#[derive(Debug)]
pub enum PeerProcess {
    /// Its pidfd: a read through it is bound to that process.
    Pidfd(OwnedFd),
    /// `SO_PEERCRED`'s pid, on a kernel without `SO_PEERPIDFD` (`ENOPROTOOPT`): a process that
    /// exits at once could hand it to another in the moment before the read.
    Pid(i32),
    /// Unidentifiable: the pidfd failed for another reason (the peer gone, no descriptor left),
    /// where the pid could already be another process's, or the kernel gave no pid.
    Unknown,
}

/// The process that connected to `stream` (`cred_pid`: `SO_PEERCRED`'s pid). Only a kernel without
/// `SO_PEERPIDFD` falls back to the pid; any other failure of the pidfd leaves the peer unknown.
pub fn peer_process(stream: &UnixStream, cred_pid: Option<i32>) -> PeerProcess {
    match peer_pidfd(stream) {
        Ok(pidfd) => PeerProcess::Pidfd(pidfd),
        Err(e) if e.raw_os_error() == Some(libc::ENOPROTOOPT) => {
            cred_pid.map_or(PeerProcess::Unknown, PeerProcess::Pid)
        }
        Err(e) => {
            tracing::debug!("the peer's pidfd cannot be read ({e}); the peer is not identified");
            PeerProcess::Unknown
        }
    }
}

impl PeerProcess {
    /// What `read` finds in `/proc` for the process. With a pidfd the read is bound to that
    /// process: its pid is read from the pidfd, then `read` runs, then the pidfd says the process
    /// is still there, so the pid was not another's. `None`: unknown.
    pub fn read<T>(&self, read: impl FnOnce(i32) -> Option<T>) -> Option<T> {
        match self {
            Self::Pidfd(pidfd) => {
                let pid = pidfd_pid(pidfd)?;
                let found = read(pid)?;
                pidfd_alive(pidfd).then_some(found)
            }
            Self::Pid(pid) => read(*pid),
            Self::Unknown => None,
        }
    }
}

/// What `read` finds in `/proc` for the process that connected ([`peer_process`], then
/// [`PeerProcess::read`]). `None`: unknown.
pub fn peer_read<T>(
    stream: &UnixStream,
    cred_pid: Option<i32>,
    read: impl FnOnce(i32) -> Option<T>,
) -> Option<T> {
    peer_process(stream, cred_pid).read(read)
}

/// `/proc/<pid>/cgroup`.
pub fn proc_cgroup(pid: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()
}

/// A temp folder for a test's sockets with a short path: a socket's path has at most 107 bytes
/// (`sun_path`), which a folder under a worktree's `target/` can come close to. In the session's
/// runtime folder (`$XDG_RUNTIME_DIR`), else the system's temp folder; it holds sockets and small
/// files only.
#[cfg(test)]
pub fn short_temp_dir() -> tempfile::TempDir {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix("kai-test-")
        .tempdir_in(base)
        .unwrap()
}

/// The cgroup paths of a `/proc/<pid>/cgroup` text, one per hierarchy line
/// (`<id>:<controllers>:<path>`).
pub fn cgroup_paths(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| line.splitn(3, ':').nth(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peer's pidfd names the process that connected and says whether it is still there.
    #[tokio::test]
    async fn the_peer_is_read_through_its_pidfd() {
        let (a, _b) = UnixStream::pair().unwrap();
        let pidfd = match peer_pidfd(&a) {
            Ok(fd) => fd,
            Err(e) if e.raw_os_error() == Some(libc::ENOPROTOOPT) => {
                eprintln!("skipped: this kernel has no SO_PEERPIDFD (Linux 6.5)");
                return;
            }
            Err(e) => panic!("SO_PEERPIDFD: {e}"),
        };
        assert_eq!(pidfd_pid(&pidfd), Some(std::process::id() as i32));
        assert!(pidfd_alive(&pidfd));
        let cgroup = |pid: i32| Some(format!("cgroup of {pid}"));
        assert_eq!(
            peer_read(&a, None, cgroup),
            Some(format!("cgroup of {}", std::process::id()))
        );
        // A process that is gone: its pidfd says so.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        // SAFETY: pidfd_open on our own child's pid, which is not reaped until `wait`.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0) };
        assert!(fd >= 0);
        // SAFETY: the syscall made the descriptor for us.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        child.wait().unwrap();
        assert!(!pidfd_alive(&fd));
        assert_eq!(pidfd_pid(&fd), None);
        let gone = PeerProcess::Pidfd(fd);
        assert_eq!(gone.read(cgroup), None, "a read for a process that is gone");
        // Without a pidfd: the pid on a kernel without SO_PEERPIDFD, else nothing.
        assert_eq!(PeerProcess::Pid(7).read(cgroup), Some("cgroup of 7".into()));
        assert_eq!(PeerProcess::Unknown.read(cgroup), None);
    }

    /// The socket is 0600 from the moment it can be connected to, in a folder made 0700; an old
    /// socket in its place is replaced, anything else is not.
    #[tokio::test]
    async fn the_socket_is_private() {
        let tmp = short_temp_dir();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let path = tmp.path().join("run/kai/x.sock");
        let listener = bind(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let _c = UnixStream::connect(&path).await.unwrap();
        let _ = listener.accept().await.unwrap();
        drop(listener);
        let _again = bind(&path).unwrap();
        let file = tmp.path().join("run/kai/file");
        std::fs::write(&file, "").unwrap();
        assert!(bind(&file).is_err());
    }

    #[test]
    fn a_cgroup_file_has_a_path_per_line() {
        let text = "1:net_cls:/\n0::/user.slice/app.slice/app-x.scope\n";
        assert_eq!(
            cgroup_paths(text).collect::<Vec<_>>(),
            ["/", "/user.slice/app.slice/app-x.scope"]
        );
    }
}
