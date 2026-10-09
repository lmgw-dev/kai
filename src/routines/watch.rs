//! The routines' directory and Kai's configuration, watched with inotify (K28: a change sends
//! `notifications/tools/list_changed`). Each burst of events is one rescan once the files have been
//! quiet for [`QUIET`]: a script saved in several writes, or an agent writing a whole routine, is
//! read once, whole. The rescan reads every routine again and lists the tools again only when
//! something offered changed (the host compares the list, K22).
//!
//! Watched: the routines' directory (a routine added, removed or renamed), each routine's own
//! folder (its `routine.toml` and script, not deeper: a routine that is its own git repository
//! does not rescan on every `git` operation), and the configuration's folder (the switches,
//! `routines.enabled`, written by the operations, the tray or by hand).

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// How long the files must be quiet before a rescan.
pub const QUIET: Duration = Duration::from_millis(150);

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR;

/// One inotify instance.
pub struct Watch {
    fd: AsyncFd<OwnedFd>,
}

impl Watch {
    pub fn new() -> std::io::Result<Self> {
        // SAFETY: inotify_init1 returns a new descriptor or -1.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a descriptor we own from here.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self {
            fd: AsyncFd::with_interest(fd, Interest::READABLE)?,
        })
    }

    /// Watches the folder `dir` (again: a folder watched already keeps its watch).
    pub fn add(&self, dir: &Path) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: a valid descriptor and a NUL-terminated path.
        let wd =
            unsafe { libc::inotify_add_watch(self.fd.get_ref().as_raw_fd(), path.as_ptr(), MASK) };
        if wd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits for the next burst of events and its end ([`QUIET`]).
    pub async fn burst(&self) -> std::io::Result<()> {
        self.events().await?;
        while tokio::time::timeout(QUIET, self.events()).await.is_ok() {}
        Ok(())
    }

    /// Waits until events come, and reads them all (only that something changed matters).
    async fn events(&self) -> std::io::Result<()> {
        let mut buf = [0u8; 8192];
        loop {
            let mut ready = self.fd.readable().await?;
            let read = ready.try_io(|fd| {
                // SAFETY: reads into a buffer of its length.
                let n = unsafe {
                    libc::read(fd.get_ref().as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
                };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match read {
                Ok(Ok(0)) => continue,
                Ok(Ok(_)) => {
                    // Drain what else is queued now.
                    loop {
                        // SAFETY: as above.
                        let n = unsafe {
                            libc::read(
                                self.fd.get_ref().as_raw_fd(),
                                buf.as_mut_ptr().cast(),
                                buf.len(),
                            )
                        };
                        if n <= 0 {
                            break;
                        }
                    }
                    return Ok(());
                }
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }
}
