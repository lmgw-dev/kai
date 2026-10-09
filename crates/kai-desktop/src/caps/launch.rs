//! The Launch capability (K23, K14, WP16a): `open`, `app_launch` and `run_command`. Every program
//! a provider starts runs in its own transient app scope on the user's service manager, in a
//! session of its own: it survives Kai and inherits none of Kai's descriptors (K14).

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Fut;

/// What `open` or `app_launch` started.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Started {
    /// The application's id (its desktop entry's: `org.kde.dolphin`).
    pub id: String,
    /// Its name as its desktop entry gives it, in the session's language where it has one.
    pub name: String,
    /// `open`: the type the target was opened as (`text/plain`, `x-scheme-handler/https`).
    pub mime: Option<String>,
    /// The processes started: more than one where the application takes one file per process and
    /// was given several.
    pub instances: usize,
    /// What the result says besides, one line each (files it does not take).
    pub notes: Vec<String>,
}

/// What `open` will start, resolved before the owner's confirmation (the dialog names the
/// application): [`Launch::open_start`] starts exactly this.
#[derive(Clone)]
pub struct Opening {
    /// The application's id and name, as [`Started`] says them.
    pub id: String,
    pub name: String,
    /// The type the target opens as.
    pub mime: String,
    pub notes: Vec<String>,
    /// The provider's resolution (its entry, the target, the session).
    pub plan: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for Opening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opening")
            .field("id", &self.id)
            .field("mime", &self.mime)
            .finish()
    }
}

/// One of a command's streams, as kept: its real size, its first bytes and its last bytes. What is
/// shown of them is the caller's choice (the result's room is the caller's to know).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Captured {
    /// Everything the command wrote to it, in bytes, whatever is kept.
    pub total: u64,
    /// Its first bytes: all of it when [`Captured::cut`] is 0 and `tail` is empty.
    pub head: Vec<u8>,
    /// Its last bytes, after `head`; empty when `head` holds all of it.
    pub tail: Vec<u8>,
}

impl Captured {
    /// The bytes between `head` and `tail` that were not kept.
    pub fn cut(&self) -> u64 {
        self.total
            .saturating_sub(self.head.len() as u64 + self.tail.len() as u64)
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Exited(i32),
    /// Killed by a signal it did not handle (its number).
    Signalled(i32),
    /// Stopped by Kai at its time limit; `setting` names the limit with its value
    /// (`lmgw/timeout_ms = 60000, less 0.5 s for the answer`, `launch.command_timeout_s = 50`).
    /// Its scope's processes were killed.
    TimedOut {
        setting: String,
    },
}

/// A command's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub status: Status,
    pub elapsed: Duration,
    pub stdout: Captured,
    pub stderr: Captured,
    /// The provider's own bound on what is kept of the output, in bytes, with its setting
    /// (`launch.output_max_mb = 8`), where the owner set one.
    pub cap: Option<(u64, String)>,
}

/// What bounds a command beyond the provider's own settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunBounds {
    /// The most of each stream worth keeping, in bytes: its first `keep` bytes and its last
    /// `keep / 2` (the caller shows at most `keep` of all of it); `None`: all of it.
    pub keep: Option<u64>,
    /// When the command must be stopped at the latest, and how the result names that limit.
    pub stop_at: Option<(Instant, String)>,
}

/// The Launch capability.
pub trait Launch: Send + Sync {
    /// Resolves `target`, a path or a URL: its type and the application the desktop's defaults
    /// name for it. A program or a launcher (an executable type) is refused: `open` never runs
    /// one.
    fn open_plan<'a>(&'a self, target: &'a str) -> Fut<'a, Opening>;

    /// Starts what [`Launch::open_plan`] resolved.
    fn open_start<'a>(&'a self, opening: &'a Opening) -> Fut<'a, Started>;

    /// Opens `target` with the application the desktop's defaults name for its type.
    fn open<'a>(&'a self, target: &'a str) -> Fut<'a, Started> {
        Box::pin(async move {
            let o = self.open_plan(target).await?;
            let mut s = self.open_start(&o).await?;
            s.mime = Some(o.mime.clone());
            Ok(s)
        })
    }

    /// Starts the installed application `app` names (its id, name or command; an ambiguous one is
    /// [`super::Failure::Refused`] listing the candidates), with `urls` (paths or URLs) to open.
    fn app_launch<'a>(&'a self, app: &'a str, urls: &'a [String]) -> Fut<'a, Started>;

    /// Runs `argv` (no shell) in `cwd` (default: the home directory) until it ends or its time
    /// limit, its output kept within `bounds`. Dropping the future stops it: its scope's processes
    /// are killed.
    fn run<'a>(
        &'a self,
        argv: &'a [String],
        cwd: Option<&'a str>,
        bounds: &'a RunBounds,
    ) -> Fut<'a, Ran>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cut_is_what_neither_end_shows() {
        let c = Captured {
            total: 100,
            head: vec![b'a'; 10],
            tail: vec![b'z'; 20],
        };
        assert_eq!(c.cut(), 70);
        let whole = Captured {
            total: 3,
            head: b"abc".to_vec(),
            tail: vec![],
        };
        assert_eq!(whole.cut(), 0);
    }
}
