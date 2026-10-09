//! The clipboard's write side through `wl-copy` (wl-clipboard) on a Wayland session. The text goes
//! over its standard input, never its arguments (a process list shows those), and `wl-copy` forks
//! to hold the selection, so Kai waits only for its exit status. The program is found at the probe
//! and kept as a path.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::{Failure, Fut};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct WlCopy {
    pub program: PathBuf,
    pub timeout: Duration,
}

impl Clipboard for WlCopy {
    fn set<'a>(&'a self, text: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let mut child = Command::new(&self.program)
                .args(["--type", "text/plain;charset=utf-8"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| Failure::Failed(format!("cannot run wl-copy: {e}")))?;
            let mut stdin = child.stdin.take().expect("piped");
            let run = async {
                stdin.write_all(text.as_bytes()).await?;
                drop(stdin);
                child.wait().await
            };
            match tokio::time::timeout(self.timeout, run).await {
                Ok(Ok(status)) if status.success() => Ok(()),
                Ok(Ok(status)) => Err(Failure::Failed(format!(
                    "wl-copy failed ({status}); the compositor may not allow clipboard access"
                ))),
                Ok(Err(e)) => Err(Failure::Failed(format!("wl-copy failed: {e}"))),
                Err(_) => Err(Failure::Failed(format!(
                    "wl-copy did not finish within {} s (dbus.timeout_s)",
                    self.timeout.as_secs()
                ))),
            }
        })
    }
}
