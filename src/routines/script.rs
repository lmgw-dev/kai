//! A script routine's run (K28): the executable in its own transient app scope on the user's
//! service manager (K14, as `run_command`'s: `crate::desktop::fdo::launch::spawn`), named
//! `app-dev.rootshell.kai.routine.<name>-<random>.scope`, so a `kai tool` it runs is known by its
//! cgroup as the routine's (`crate::cli`).
//!
//! - **Arguments:** the routine's parameters in their declared order, each as text (an integer as
//!   digits, a boolean as `true`/`false`); an omitted optional one is an empty argument.
//! - **Environment:** the session's (the user manager's, as every program Kai starts gets it), with
//!   `KAI_CLI_SOCKET` (where `kai tool` and `kai model` reach this Kai), `KAI_ROUTINE` (its name),
//!   and Kai's own binary's folder first on `PATH` when `kai` there is not this one (a dev build).
//!   No secret: the device's key never leaves Kai; the script's model calls go through `kai model`.
//! - **Working directory:** the routine's folder. **stdin:** empty.
//! - **stdout** is the result (K22: compact; never cut: a result larger than the call's room is the
//!   call's error naming the room: lmgw's message limit for a tool call, a notification's for the
//!   tray and a key, `crate::notify::BODY_ROOM`). For the CLI (`Call::out`) it is streamed to the
//!   client as it comes and not kept. Nothing keeps more than the call's room. **stderr** goes to
//!   the journal under `kai-routine-<name>` (`journalctl --user -t kai-routine-<name>`), not into
//!   Kai's log.
//! - **The scope is Kai's:** its unit name is in [`LiveScopes`] from before the start until the run
//!   ends, and only then does a `kai tool` from inside it count as the routine (`crate::cli`).
//! - **The exit code** decides: 0 is done, anything else failed (the result says the code).
//! - **Its time:** lmgw's wait less `run_command`'s answer margin; a call with no wait (the CLI,
//!   the tray, a key) runs until it ends or its caller goes. A cancelled or timed-out call kills
//!   the whole scope.

use std::collections::HashSet;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt};

use tokio_util::sync::CancellationToken;

use super::def::Routine;
use crate::desktop::fdo::launch::program_env;
use crate::desktop::fdo::launch::spawn::{self, Keep, Program, UserManager};
use crate::desktop::tools::launch::ANSWER_MARGIN;
use crate::host::registry::{Args, Call, Out, Reply};

/// The unit names of the routine scopes Kai started whose run has not ended (module doc).
#[derive(Clone, Default)]
pub struct LiveScopes(Arc<Mutex<HashSet<String>>>);

impl LiveScopes {
    /// `unit` is one of Kai's routine scopes now.
    pub fn holds(&self, unit: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(unit)
    }

    /// Marks `unit` as Kai's until the mark is dropped (the run's end, however it ends).
    pub fn mark(&self, unit: &str) -> ScopeMark {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(unit.to_owned());
        ScopeMark(self.clone(), unit.to_owned())
    }
}

/// A unit's place in [`LiveScopes`] (`LiveScopes::mark`).
pub struct ScopeMark(LiveScopes, String);

impl Drop for ScopeMark {
    fn drop(&mut self) {
        self.0
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
    }
}

/// What a script's run needs of Kai.
#[derive(Clone)]
pub struct Env {
    pub manager: UserManager,
    /// Kai's own environment (the process's; the tests give theirs).
    pub own_env: fn() -> Vec<(String, String)>,
    /// Where Kai serves the CLI (`KAI_CLI_SOCKET`).
    pub socket: PathBuf,
    /// The folder of Kai's own binary, for `PATH`.
    pub exe_dir: Option<PathBuf>,
    /// The routine scopes running now (module doc), shared with the CLI's server.
    pub scopes: LiveScopes,
}

/// The unit a routine's script runs in: `app-<APP_ID>.routine.<name>-<random>.scope`.
pub fn unit(name: &str) -> String {
    spawn::scope_unit(&format!("{}.routine.{name}", crate::APP_ID))
}

/// The prefix of a routine's scope's name, up to the routine's name.
pub fn unit_prefix() -> String {
    format!("app-{}.routine.", spawn::escape_unit(crate::APP_ID))
}

/// The arguments of the call, in the routine's declared order (module doc).
pub fn argv(path: &Path, routine: &Routine, args: &Args) -> Vec<String> {
    let mut argv = vec![path.display().to_string()];
    for p in routine.spec.params {
        argv.push(
            args.text(p.name)
                .map(str::to_owned)
                .or_else(|| args.integer(p.name).map(|i| i.to_string()))
                .or_else(|| args.number(p.name).map(|n| n.to_string()))
                .or_else(|| args.boolean(p.name).map(|b| b.to_string()))
                .unwrap_or_default(),
        );
    }
    argv
}

/// The script's environment (module doc): `session` is the user manager's (`None`: unknown).
pub fn environment(
    session: Option<&[String]>,
    own: &[(String, String)],
    name: &str,
    socket: &Path,
    exe_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = program_env(session, own);
    env.retain(|(k, _)| k != "KAI_CLI_SOCKET" && k != "KAI_ROUTINE");
    if let Some(dir) = exe_dir {
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let first_kai = path
            .split(':')
            .filter(|p| !p.is_empty())
            .map(|p| Path::new(p).join("kai"))
            .find(|k| k.is_file());
        if first_kai.as_deref() != Some(dir.join("kai").as_path()) {
            let joined = if path.is_empty() {
                dir.display().to_string()
            } else {
                format!("{}:{path}", dir.display())
            };
            env.retain(|(k, _)| k != "PATH");
            env.push(("PATH".into(), joined));
        }
    }
    env.push(("KAI_CLI_SOCKET".into(), socket.display().to_string()));
    env.push(("KAI_ROUTINE".into(), name.to_owned()));
    env
}

/// Runs the script `path` of `routine` for `call` (module doc).
pub async fn run(env: &Env, routine: &Routine, path: &Path, call: &Call) -> Reply {
    let name = routine.name.as_str();
    let session = match env.manager.environment().await {
        Ok(list) => Some(list),
        Err(e) => {
            tracing::warn!(
                routine = name,
                "routines: the session's environment is unknown ({e}); the script gets Kai's \
                 own without Kai's variables"
            );
            None
        }
    };
    let vars = environment(
        session.as_deref(),
        &(env.own_env)(),
        name,
        &env.socket,
        env.exe_dir.as_deref(),
    );
    let stop_at = call.deadline.map(|d| {
        let at = d.into_std();
        (
            at.checked_sub(ANSWER_MARGIN).unwrap_or(at),
            format!(
                "lmgw/timeout_ms = {}, less {:.1} s for the answer",
                call.timeout_ms.unwrap_or_default(),
                ANSWER_MARGIN.as_secs_f64()
            ),
        )
    });
    if let Some((at, says)) = &stop_at
        && *at <= Instant::now()
    {
        return Reply::error(format!(
            "no time is left to run {name} before its limit ({says})"
        ));
    }
    let journal = spawn::journal_stream(&format!("kai-routine-{name}"));
    let unit = unit(name);
    let _mark = env.scopes.mark(&unit);
    let started = Instant::now();
    let spawned = spawn::spawn(
        &env.manager,
        Program {
            path: path.to_owned(),
            argv: argv(path, routine, &call.args),
            cwd: routine.dir.clone(),
            env: vars,
            unit: unit.clone(),
            description: format!("Kai routine {name}"),
            stdout: Stdio::piped(),
            stderr: journal.map(Stdio::from).unwrap_or(Stdio::null()),
            kill_on_drop: true,
        },
    )
    .await;
    let (mut child, mut scope) = match spawned {
        Ok(s) => s,
        Err(e) => return Reply::error(format!("{name} did not start: {e}")),
    };
    tracing::info!(routine = name, %unit, "routines: a script started");
    let Some(out) = child.stdout.take() else {
        scope.kill().await;
        return Reply::error(format!("{name}: the script's output pipe is missing"));
    };
    let leader_gone = CancellationToken::new();
    let reading = async {
        match &call.out {
            Some(tx) => {
                stream_pipe(out, tx, leader_gone.clone()).await;
                None
            }
            None => Some(spawn::read_pipe(out, Keep::new(call.room), leader_gone.clone()).await),
        }
    };
    let waiting = async {
        let waited = match &stop_at {
            Some((at, _)) => {
                tokio::time::timeout_at(tokio::time::Instant::from_std(*at), child.wait())
                    .await
                    .ok()
            }
            None => Some(child.wait().await),
        };
        let ended = match waited {
            Some(Ok(st)) => {
                scope.disarm();
                use std::os::unix::process::ExitStatusExt;
                Ok(match (st.code(), st.signal()) {
                    (Some(c), _) => Ended::Exited(c),
                    (None, Some(sig)) => Ended::Signalled(sig),
                    (None, None) => Ended::Exited(-1),
                })
            }
            Some(Err(e)) => {
                scope.kill().await;
                Err(format!("cannot wait for {name}: {e}"))
            }
            None => {
                scope.kill().await;
                let _ = child.wait().await;
                Ok(Ended::TimedOut(
                    stop_at.as_ref().map(|(_, s)| s.clone()).unwrap_or_default(),
                ))
            }
        };
        leader_gone.cancel();
        ended
    };
    let (ended, kept) = tokio::join!(waiting, reading);
    let Some(kept) = kept else {
        // Streamed: the client has the output; the answer says only how it ended.
        tracing::info!(
            routine = name,
            %unit,
            ended = ?ended.as_ref().map(Ended::word),
            ms = started.elapsed().as_millis() as u64,
            "routines: a script ended (its stdout streamed)"
        );
        return match ended {
            Ok(Ended::Exited(0)) => Reply::text(""),
            Ok(other) => Reply::error(format!("failed: {name} {}", other.word())),
            Err(e) => Reply::error(e),
        };
    };
    let captured = kept.captured();
    tracing::info!(
        routine = name,
        %unit,
        ended = ?ended.as_ref().map(Ended::word),
        stdout_bytes = captured.total,
        ms = started.elapsed().as_millis() as u64,
        "routines: a script ended"
    );
    let ended = match ended {
        Ok(e) => e,
        Err(e) => return Reply::error(e),
    };
    if let Some(room) = call.room
        && captured.total > room
    {
        return Reply::error(format!(
            "{name} wrote {} bytes to stdout, more than its result may take ({}); it {}",
            captured.total,
            call.room_says,
            ended.word()
        ));
    }
    let text = String::from_utf8_lossy(&captured.head);
    let text = text.trim_end();
    match ended {
        Ended::Exited(0) if text.is_empty() => Reply::text("done"),
        Ended::Exited(0) => Reply::text(text),
        other => {
            let why = other.word();
            Reply::error(if text.is_empty() {
                format!("failed: {name} {why}")
            } else {
                format!("{text}\nfailed: {name} {why}")
            })
        }
    }
}

/// Streams the pipe to `tx` until its end, or until `leader_gone` (then what waits in the pipe at
/// that moment, as `spawn::read_pipe` does). A receiver that is gone takes nothing more; the pipe
/// is still read to its end, so the script never blocks on it.
async fn stream_pipe<R: AsyncRead + AsRawFd + Unpin>(
    mut pipe: R,
    tx: &Out,
    leader_gone: CancellationToken,
) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut open = true;
    loop {
        tokio::select! {
            biased;
            () = leader_gone.cancelled() => break,
            r = pipe.read(&mut buf) => match r {
                Ok(0) | Err(_) => return,
                Ok(n) => forward(tx, &mut open, &buf[..n]).await,
            },
        }
    }
    let mut left = spawn::waiting(pipe.as_raw_fd());
    while left > 0 {
        let want = left.min(buf.len());
        match pipe.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                forward(tx, &mut open, &buf[..n]).await;
                left -= n;
            }
        }
    }
}

async fn forward(tx: &Out, open: &mut bool, chunk: &[u8]) {
    if *open && tx.send(chunk.to_vec()).await.is_err() {
        *open = false;
    }
}

/// How a script ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Exited(i32),
    Signalled(i32),
    TimedOut(String),
}

impl Ended {
    fn word(&self) -> String {
        match self {
            Ended::Exited(0) => "exited with 0".into(),
            Ended::Exited(c) => format!("exited with {c}"),
            Ended::Signalled(s) => format!("was killed by signal {s}"),
            Ended::TimedOut(says) => format!("was stopped at its time limit ({says})"),
        }
    }
}
