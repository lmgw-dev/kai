//! The freedesktop Launch (WP16a, K14, K23): applications from their desktop entries, `open` by the
//! shared MIME-info database and `mimeapps.list`, and `run_command`; every program in its own
//! transient app scope on the user's service manager ([`spawn`]).
//!
//! - **The environment** a program gets is the session's: the user manager's (`Manager.Environment`,
//!   which Plasma fills, as Kai's scheme handler does, `crate::launch`), with what a program cannot
//!   start without taken from Kai's own where it lacks them. Where the manager's cannot be read,
//!   Kai's own without Kai's variables (`KAI_*`), said in the log. The desktop entries and the
//!   defaults are looked up in the same environment's directories.
//! - **Applications** (`open`, `app_launch`) start in `app-<id>-<random>.scope`, in their entry's
//!   `Path` (one that is not a directory is an error) or the home directory, their stdout and
//!   stderr on a journal stream of their own (their id); the call answers once they `exec`ed. One
//!   that asks for a terminal starts in `launch.terminal`, or unset in Plasma's configured terminal,
//!   `xdg-terminal-exec` or Konsole ([`terminal`]). `open` resolves first
//!   ([`Launch::open_plan`], shown in the owner's confirmation) and refuses executable types.
//! - **Commands** (`run_command`) start in `app-dev.rootshell.kai.run-<random>.scope` (Kai's id
//!   with `.run`: not Kai's, so a portal grants it nothing of Kai's), in `cwd` or the home
//!   directory, stdin empty, stdout and stderr each on a pipe Kai reads into memory, keeping each
//!   stream's start and end within the caller's bound ([`spawn::Keep`]). The call answers when the
//!   command's leader ends: what waits in the pipes then is read and the pipes are closed, so what
//!   it left in the background neither holds the answer nor fills anything (it gets EPIPE), and
//!   otherwise keeps running. At its time limit (the call's deadline less the answer's margin, or
//!   `launch.command_timeout_s` where the owner set one and it comes first), or when the call is
//!   cancelled (lmgw's `notifications/cancelled`, which drops the call), every process of its
//!   scope is killed (`KillUnit`, and the process group). The result carries the real output
//!   sizes.

mod entries;
#[cfg(test)]
mod live_tests;
mod mime;
pub mod settings;
pub mod spawn;
pub mod terminal;
#[cfg(test)]
pub(crate) mod tests;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use kai_desktop::caps::launch::{Launch, Opening, Ran, RunBounds, Started, Status};
use kai_desktop::caps::{Capability, Failure, Fut};
use tokio_util::sync::CancellationToken;

use super::super::probe::{Offer, Service};
use super::super::{Env, has_owner};
use entries::{Dirs, Entry, Target};
use settings::LaunchConfig;
use spawn::{Program, UserManager};

/// The Launch offer: the user's service manager on the session bus, which makes the scopes.
pub async fn probe(env: &Env) -> Offer {
    let Some(bus) = &env.session else {
        return Offer::absent(Capability::Launch, "no session bus");
    };
    let conn = match bus.get().await {
        Ok(c) => c,
        Err(e) => return Offer::absent(Capability::Launch, e),
    };
    match has_owner(&conn, spawn::SYSTEMD, env.timeout).await {
        Ok(true) => Offer::ready(
            Service::Launch(Arc::new(FdoLaunch {
                manager: UserManager {
                    bus: bus.clone(),
                    timeout: env.timeout,
                },
                config: env.launch.clone(),
                own_env: process_env,
            })),
            "app scopes on the user's service manager; desktop entries, mimeapps.list",
        ),
        Ok(false) => Offer::absent(
            Capability::Launch,
            "no user service manager (org.freedesktop.systemd1) on the session bus for the \
             programs' scopes (K14)",
        ),
        Err(e) => Offer::absent(Capability::Launch, e),
    }
}

/// What a program cannot start without: taken from Kai's own environment where the session's lacks
/// them (`crate::launch` keeps the same list for Kai itself).
const NEEDED: [&str; 6] = [
    "PATH",
    "HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
];

pub struct FdoLaunch {
    pub manager: UserManager,
    pub config: LaunchConfig,
    /// Kai's own environment (the process's; the tests give theirs).
    pub own_env: fn() -> Vec<(String, String)>,
}

/// The process's environment.
pub fn process_env() -> Vec<(String, String)> {
    std::env::vars().collect()
}

/// The environment a program gets (module doc): `session` is the manager's `K=V` list.
pub fn program_env(session: Option<&[String]>, own: &[(String, String)]) -> Vec<(String, String)> {
    let Some(session) = session else {
        return own
            .iter()
            .filter(|(k, _)| !k.starts_with("KAI_"))
            .cloned()
            .collect();
    };
    let mut env: Vec<(String, String)> = session
        .iter()
        .filter_map(|e| e.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    for k in NEEDED {
        if !env.iter().any(|(n, _)| n == k)
            && let Some((_, v)) = own.iter().find(|(n, _)| n == k)
        {
            env.push((k.to_owned(), v.clone()));
        }
    }
    env
}

/// One environment, with its directories.
#[derive(Clone)]
struct Session {
    env: Vec<(String, String)>,
    dirs: Dirs,
}

impl FdoLaunch {
    async fn session(&self) -> Session {
        let own = (self.own_env)();
        let env = match self.manager.environment().await {
            Ok(list) => program_env(Some(&list), &own),
            Err(e) => {
                tracing::warn!(
                    "launch: the session's environment is unknown ({e}); the program gets Kai's \
                     own without Kai's variables"
                );
                program_env(None, &own)
            }
        };
        let dirs = Dirs::from_env(|k| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()));
        Session { env, dirs }
    }

    /// Starts `entry` with `targets`: one process, or one per target (`entries::plan`).
    async fn start(
        &self,
        s: &Session,
        entry: &Entry,
        targets: &[Target],
    ) -> Result<Started, Failure> {
        let plan = entries::plan(entry, targets).map_err(Failure::Refused)?;
        let name = entry.shown_name().to_owned();
        let cwd = match entry.workdir.as_deref().map(PathBuf::from) {
            None => s.dirs.home.clone(),
            Some(p) if p.is_absolute() && p.is_dir() => p,
            Some(_) => {
                return Err(Failure::Refused(format!(
                    "{}'s desktop entry names a working directory (Path) that is not a folder \
                     here",
                    crate::host::confirm::shown_text(&name)
                )));
            }
        };
        let term = if entry.terminal {
            let env = |k: &str| s.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let t = terminal::resolve(self.config.terminal.as_deref(), &s.dirs, &env).map_err(
                |why| {
                    Failure::Refused(format!(
                        "{} runs in a terminal, and {why}",
                        crate::host::confirm::shown_text(&name)
                    ))
                },
            )?;
            tracing::info!(app = %entry.id, terminal = terminal::program(&t), source = ?t.source,
                "launch: the application runs in a terminal");
            Some(t)
        } else {
            None
        };
        for mut argv in plan.argvs.iter().cloned() {
            if let Some(t) = &term {
                argv.splice(0..0, t.argv.iter().cloned());
            }
            let path =
                entries::find_program(&argv[0], &s.dirs.path, &s.dirs.home).ok_or_else(|| {
                    Failure::Refused(if let Some(t) = &term {
                        format!(
                            "{} runs in a terminal, and {} is not installed",
                            crate::host::confirm::shown_text(&name),
                            t.source.describe()
                        )
                    } else {
                        format!(
                            "{}'s program is not installed (its desktop entry's Exec is not on \
                             PATH)",
                            crate::host::confirm::shown_text(&name)
                        )
                    })
                })?;
            let out =
                |fd: Option<std::os::fd::OwnedFd>| fd.map(Stdio::from).unwrap_or(Stdio::null());
            let journal = spawn::journal_stream(&entry.id);
            let stderr = journal.as_ref().and_then(|j| j.try_clone().ok());
            let unit = spawn::scope_unit(&entry.id);
            spawn::spawn(
                &self.manager,
                Program {
                    path,
                    argv,
                    cwd: cwd.clone(),
                    env: s.env.clone(),
                    unit: unit.clone(),
                    description: crate::host::confirm::plain(&name),
                    stdout: out(journal),
                    stderr: out(stderr),
                    kill_on_drop: false,
                },
            )
            .await?;
            tracing::info!(app = %entry.id, %unit, "launch: started an application");
        }
        Ok(Started {
            id: entry.id.clone(),
            name,
            mime: None,
            instances: plan.argvs.len(),
            notes: plan.notes,
        })
    }

    async fn launch_app(&self, app: &str, urls: &[String]) -> Result<Started, Failure> {
        let s = self.session().await;
        let dirs = s.dirs.clone();
        let (apps, schemes) = blocking(move || {
            let apps = entries::scan(&dirs);
            let schemes = mime::Schemes::load(&dirs, &apps);
            (apps, schemes)
        })
        .await?;
        let targets = urls
            .iter()
            .map(|u| Target::parse(u, &s.dirs.home, |sc| schemes.known(sc)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Failure::Refused(format!("urls: {e}")))?;
        let entry = entries::resolve(&apps, app).map_err(Failure::Refused)?;
        self.start(&s, entry, &targets).await
    }

    async fn plan_open(&self, target: &str) -> Result<Opening, Failure> {
        let s = self.session().await;
        let dirs = s.dirs.clone();
        let target = target.to_owned();
        let home = s.dirs.home.clone();
        let (apps, db, parsed) = blocking(move || {
            let db = mime::MimeDb::load(&dirs);
            let apps = entries::scan(&dirs);
            let schemes = mime::Schemes::load(&dirs, &apps);
            let parsed = Target::parse(&target, &home, |sc| schemes.known(sc)).map(|t| {
                let mime = match &t {
                    Target::Url(u) => Some(format!(
                        "x-scheme-handler/{}",
                        u.split(':').next().unwrap_or_default().to_ascii_lowercase()
                    )),
                    Target::File(p) => p.exists().then(|| db.of_path(p)),
                };
                (t, mime)
            });
            (apps, db, parsed)
        })
        .await?;
        let (t, mime) = parsed.map_err(Failure::Refused)?;
        let Some(mime) = mime else {
            return Err(Failure::Refused(if matches!(t, Target::Url(_)) {
                "no such file or folder".into()
            } else {
                "no such file or folder (a URL needs its scheme, https://…)".into()
            }));
        };
        if let Some(kind) = mime::executable(&mime, &db) {
            return Err(Failure::Refused(format!(
                "this is {kind} ({mime}): open does not start programs or launchers; run it with \
                 run_command, which the owner confirms"
            )));
        }
        let (entry, found_as) = mime::default_app(&mime, &db, &s.dirs, &apps).ok_or_else(|| {
            Failure::Refused(format!(
                "no application opens {mime} here (mimeapps.list names no default, and no \
                 installed application lists the type)"
            ))
        })?;
        let mut notes = Vec::new();
        if found_as != mime {
            notes.push(format!("no default for {mime}: the one for {found_as}"));
        }
        Ok(Opening {
            id: entry.id.clone(),
            name: entry.shown_name().to_owned(),
            mime,
            notes,
            plan: Arc::new(OpenPlan {
                session: s,
                entry: entry.clone(),
                target: t,
            }),
        })
    }

    async fn start_open(&self, o: &Opening) -> Result<Started, Failure> {
        let Some(plan) = o.plan.downcast_ref::<OpenPlan>() else {
            return Err(Failure::Failed(
                "the resolution of open is not this provider's".into(),
            ));
        };
        let mut started = self
            .start(
                &plan.session,
                &plan.entry,
                std::slice::from_ref(&plan.target),
            )
            .await?;
        started.notes.splice(0..0, o.notes.iter().cloned());
        started.mime = Some(o.mime.clone());
        Ok(started)
    }

    async fn run_command(
        &self,
        argv: &[String],
        cwd: Option<&str>,
        bounds: &RunBounds,
    ) -> Result<Ran, Failure> {
        let Some(first) = argv.first().filter(|a| !a.is_empty()) else {
            return Err(Failure::Refused("argv names no program".into()));
        };
        // The earlier of the call's limit and the owner's own.
        let own = self
            .config
            .command_timeout()
            .map(|t| (Instant::now() + t, self.config.timeout_setting()));
        let stop_at = match (bounds.stop_at.clone(), own) {
            (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
            (a, b) => a.or(b),
        };
        if let Some((at, says)) = &stop_at
            && *at <= Instant::now()
        {
            return Err(Failure::Refused(format!(
                "no time is left to run it before its limit ({says})"
            )));
        }
        let s = self.session().await;
        let home = &s.dirs.home;
        let dir = match cwd.map(str::trim).filter(|c| !c.is_empty()) {
            None => home.clone(),
            Some("~") => home.clone(),
            Some(c) => match c.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => home.join(c),
            },
        };
        if !dir.is_dir() {
            return Err(Failure::Refused("cwd: no such folder".into()));
        }
        let relative = first.contains('/') && !first.starts_with('/') && !first.starts_with("~/");
        let program = if relative {
            Some(dir.join(first))
                .filter(|p| entries::find_program(&p.to_string_lossy(), "", home).is_some())
        } else {
            entries::find_program(first, &s.dirs.path, home)
        };
        let Some(path) = program else {
            return Err(Failure::Refused(if first.contains('/') {
                "the program does not exist or is not executable".into()
            } else {
                "the program is not on PATH".into()
            }));
        };
        // What is kept of each stream: the caller's bound, or the owner's where that is smaller.
        let cap = self.config.output_max();
        let keep = match (bounds.keep, cap) {
            (Some(k), Some(c)) => Some(k.min(c)),
            (k, c) => k.or(c),
        };
        let unit = spawn::scope_unit(&format!("{}.run", crate::APP_ID));
        let started = Instant::now();
        let (mut child, mut scope) = spawn::spawn(
            &self.manager,
            Program {
                path,
                argv: argv.to_vec(),
                cwd: dir,
                env: s.env.clone(),
                unit: unit.clone(),
                description: "Kai: run_command".into(),
                stdout: Stdio::piped(),
                stderr: Stdio::piped(),
                kill_on_drop: true,
            },
        )
        .await?;
        tracing::info!(%unit, "launch: run_command started");
        let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
            scope.kill().await;
            return Err(Failure::Failed(
                "the command's output pipes are missing".into(),
            ));
        };
        let leader_gone = CancellationToken::new();
        let out = spawn::read_pipe(out, spawn::Keep::new(keep), leader_gone.clone());
        let err = spawn::read_pipe(err, spawn::Keep::new(keep), leader_gone.clone());
        let waiting = async {
            let waited = match &stop_at {
                Some((at, _)) => {
                    let until = tokio::time::Instant::from_std(*at);
                    tokio::time::timeout_at(until, child.wait()).await.ok()
                }
                None => Some(child.wait().await),
            };
            let status = match waited {
                Some(Ok(st)) => {
                    scope.disarm();
                    use std::os::unix::process::ExitStatusExt;
                    Ok(match (st.code(), st.signal()) {
                        (Some(c), _) => Status::Exited(c),
                        (None, Some(sig)) => Status::Signalled(sig),
                        (None, None) => Status::Exited(-1),
                    })
                }
                Some(Err(e)) => {
                    scope.kill().await;
                    Err(Failure::Failed(format!("cannot wait for the command: {e}")))
                }
                None => {
                    scope.kill().await;
                    let _ = child.wait().await;
                    Ok(Status::TimedOut {
                        setting: stop_at.as_ref().map(|(_, s)| s.clone()).unwrap_or_default(),
                    })
                }
            };
            leader_gone.cancel();
            status
        };
        let (status, out, err) = tokio::join!(waiting, out, err);
        let status = status?;
        let (stdout, stderr) = (out.captured(), err.captured());
        let elapsed = started.elapsed();
        tracing::info!(
            %unit,
            ?status,
            stdout_bytes = stdout.total,
            stderr_bytes = stderr.total,
            ms = elapsed.as_millis() as u64,
            "launch: run_command ended"
        );
        Ok(Ran {
            status,
            elapsed,
            stdout,
            stderr,
            cap: cap.map(|c| (c, self.config.output_setting())),
        })
    }
}

/// `open`'s resolution ([`Opening::plan`]): what [`FdoLaunch::start_open`] starts.
struct OpenPlan {
    session: Session,
    entry: Entry,
    target: Target,
}

/// `f` on a blocking thread (file system work).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failure::Failed(format!("the lookup did not finish: {e}")))
}

impl Launch for FdoLaunch {
    fn open_plan<'a>(&'a self, target: &'a str) -> Fut<'a, Opening> {
        Box::pin(self.plan_open(target))
    }

    fn open_start<'a>(&'a self, opening: &'a Opening) -> Fut<'a, Started> {
        Box::pin(self.start_open(opening))
    }

    fn app_launch<'a>(&'a self, app: &'a str, urls: &'a [String]) -> Fut<'a, Started> {
        Box::pin(self.launch_app(app, urls))
    }

    fn run<'a>(
        &'a self,
        argv: &'a [String],
        cwd: Option<&'a str>,
        bounds: &'a RunBounds,
    ) -> Fut<'a, Ran> {
        Box::pin(self.run_command(argv, cwd, bounds))
    }
}
