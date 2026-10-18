//! `kai tool` and `kai model` (K28): Kai as a command for scripts, the routines' first, over the
//! local socket `$XDG_RUNTIME_DIR/kai/cli.sock` (mode 0600, the peer's uid checked; Kai opens no
//! network listener, §11.1).
//!
//! - **The wire:** one JSON line from `kai`, a [`Request`] (within the host link's message limit as
//!   Kai knows it, `crate::host::limits`: a larger one is refused naming the limit), and from Kai
//!   zero or more [`Stdout`] lines (a script routine's stdout as it comes, base64, never kept by
//!   Kai) and then one [`Answer`]. The client keeps its side open while it waits: a client that goes
//!   (Ctrl+C, its script killed) cancels the call, and a routine's script it started is killed with
//!   it.
//! - **The caller** (K15, K28): the peer's cgroup (`/proc/<pid>/cgroup`). A process in a routine's
//!   scope (`app-dev.rootshell.kai.routine.<name>-<random>.scope`) is `routine:<name>` only while
//!   that scope is one Kai made for a run that has not ended (`crate::routines::LiveScopes`); a
//!   scope of that name made by anyone else (`systemd-run --user --scope --unit=…`), or Kai's after
//!   its run, is `cli`, as is any other process (the owner's terminal, any program of the user).
//!   The cgroup is read for the process that connected: through its pidfd (`SO_PEERPIDFD`, Linux
//!   6.5 and later), checked to be alive after the read, so a pid reused in between is not read;
//!   on an older kernel `SO_PEERCRED`'s pid, with that race. Both callers go through K15 like any
//!   caller: untrusted by default, so a state-changing tool asks the owner (PD4) unless
//!   `policy.trusted_callers` names them or the tool is `allow`.
//! - **Identification, not a boundary.** The boundary is the user: a process of the same user can
//!   move itself into a running routine's scope (the user manager delegates those cgroups to the
//!   user) and so borrow the routine's name while it runs, can edit a routine's folder (which then
//!   is not offered until the owner looks again), and is `cli` whenever it likes. So trusting
//!   `cli` or a routine, or setting a tool to `allow`, hands that tool to every program of this
//!   user (`crate::host::settings`, said in the tooltip for `cli`). Likewise "a routine does not
//!   start routines" reads the caller: a script can run `systemd-run --user kai tool <routine>` and
//!   be `cli`.
//! - **`kai model <alias>`** sends the prompt (stdin) as one user message to lmgw's
//!   `/v1/chat/completions` as Kai's device (over the device link) and prints the answer: no
//!   credential enters the script. lmgw's scope for the device applies; Kai's tool policy does not
//!   (it is not a tool).
//!
//! Exit codes: 0 done, 1 the tool or the model failed (the reason on stderr), 2 a usage error, 3 no
//! Kai answers on the socket.

pub mod client;
pub mod server;

use serde::{Deserialize, Serialize};

/// What `kai` asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The tools Kai offers now (`kai tool` alone), or one tool's parameters (`kai tool <name>
    /// --help`).
    List {
        name: Option<String>,
    },
    /// One call: `--<param> <value>` pairs as written; Kai reads each by the parameter's type (a
    /// list parameter takes its flag once per item).
    Tool {
        name: String,
        args: Vec<(String, String)>,
    },
    Model {
        alias: String,
        prompt: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Done,
    /// The tool's or the model's error (a refusal of K15's included).
    Failed,
    /// The request itself was wrong (no such tool, a value of the wrong type).
    Usage,
}

/// A piece of a script routine's stdout, before the [`Answer`] (stdout is bytes, so base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stdout {
    pub stdout: String,
}

impl Stdout {
    pub fn of(bytes: &[u8]) -> Self {
        use base64::Engine;
        Self {
            stdout: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&self.stdout)
            .map_err(|e| format!("a stdout piece does not read: {e}"))
    }
}

/// Kai's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub status: Status,
    pub text: String,
    /// Said on stderr besides (a fallback that answered in place of the alias, an image block that
    /// a terminal cannot print).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Answer {
    pub fn done(text: impl Into<String>) -> Self {
        Self {
            status: Status::Done,
            text: text.into(),
            notes: Vec::new(),
        }
    }

    pub fn failed(text: impl Into<String>) -> Self {
        Self {
            status: Status::Failed,
            text: text.into(),
            notes: Vec::new(),
        }
    }

    pub fn usage(text: impl Into<String>) -> Self {
        Self {
            status: Status::Usage,
            text: text.into(),
            notes: Vec::new(),
        }
    }
}
