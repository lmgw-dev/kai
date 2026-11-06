//! The kai-firefox side of the acceptance runs: `tests/accept/launch.mjs` in the kai-firefox
//! checkout, started with `node`, read as one JSON object per line and ended by closing its stdin
//! (it ends Firefox, the hosts Firefox left and the fixture site, each by PID with its start time
//! checked). The launcher itself is a child this process holds and has not reaped, so its PID
//! names it until `wait` returns.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// How long the launcher may take to start Firefox and the extension, and to stop them.
const START: Duration = Duration::from_secs(120);
const STOP: Duration = Duration::from_secs(60);

pub struct Launcher {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    /// What it printed once the user's tab showed the start page.
    pub ready: Value,
}

impl Launcher {
    /// Starts the launcher of the kai-firefox checkout `firefox` for Kai's `socket`, with Kai's
    /// `host` (`kai-browser-host`), and waits until it is ready.
    pub async fn start(firefox: &Path, socket: &Path, host: &Path) -> Result<Self, String> {
        let mut child = Command::new("node")
            .arg("tests/accept/launch.mjs")
            .arg("--socket")
            .arg(socket)
            .current_dir(firefox)
            .env("KAI_BROWSER_HOST", host)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("node tests/accept/launch.mjs in {}: {e}", firefox.display()))?;
        let stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().expect("piped")).lines();
        let mut l = Self {
            child,
            stdin,
            lines,
            ready: Value::Null,
        };
        let first = l.next("ready", START).await?;
        l.ready = first;
        Ok(l)
    }

    /// The next line that carries `key` (`failed` ends the wait with its words).
    async fn next(&mut self, key: &str, within: Duration) -> Result<Value, String> {
        let end = tokio::time::Instant::now() + within;
        loop {
            let line = tokio::time::timeout_at(end, self.lines.next_line())
                .await
                .map_err(|_| format!("the launcher said no {key} within {within:?}"))?
                .map_err(|e| format!("the launcher's stdout: {e}"))?
                .ok_or_else(|| format!("the launcher ended before it said {key}"))?;
            let v: Value = serde_json::from_str(&line)
                .map_err(|e| format!("the launcher said {line:?}, no JSON: {e}"))?;
            if let Some(f) = v.get("failed") {
                return Err(format!("the launcher failed: {f}"));
            }
            if let Some(x) = v.get(key) {
                return Ok(x.clone());
            }
        }
    }

    /// Sends a command and returns its answer (the line carrying `key`).
    async fn command(&mut self, cmd: &str, key: &str) -> Result<Value, String> {
        let stdin = self.stdin.as_mut().ok_or("the launcher was stopped")?;
        let line = format!("{}\n", json!({ "cmd": cmd }));
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("the launcher's stdin: {e}"))?;
        self.next(key, Duration::from_secs(20)).await
    }

    /// The fixture site's requests so far, with the bodies of writes.
    pub async fn site_log(&mut self) -> Result<Vec<Value>, String> {
        match self.command("site_log", "site_log").await? {
            Value::Array(a) => Ok(a),
            other => Err(format!("the site's log is {other}")),
        }
    }

    /// Stops it (module doc): what it reports (`left`: processes still naming the run's profile).
    /// A launcher that does not end in time is killed: it is our unreaped child.
    pub async fn stop(mut self) -> Result<Value, String> {
        drop(self.stdin.take());
        let said = self.next("stopped", STOP).await;
        match tokio::time::timeout(STOP, self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }
        said
    }
}
