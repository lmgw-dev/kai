//! Starting Kai for the scheme handler (K13, §8.3).
//!
//! - **Its own app scope.** `gtk-launch` makes no scope (the child keeps its caller's, e.g. the
//!   browser's), so the portal would see the caller's app id. `systemd-run --user --scope -u
//!   app-dev.rootshell.kai-<rand>.scope` gives Kai its own; `<rand>` has no dash, or the portal
//!   cannot parse the id (K12, WP0 (g)).
//! - **The link on stdin, not in argv.** The handler writes the link into a pipe, closes its end and
//!   starts `kai --pair-stdin` with the pipe as stdin; `systemd-run --scope` execs Kai in place, so
//!   the pipe is Kai's fd 0. The handler exits at once.
//! - **The session's environment, not the caller's.** A browser's environment (`GDK_BACKEND`,
//!   `LD_PRELOAD`, its tokens) is not Kai's: Kai gets the systemd user manager's environment, which
//!   the Plasma session fills, plus Kai's own variables from the handler (`KAI_LOG` and the XDG base
//!   directories, which chose the configuration the handler read).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The longest pairing link that goes through the stdin pipe; a longer one is refused with this
/// number. A v=1 link is about 200 bytes.
pub const STDIN_LINK_MAX: usize = 16 * 1024;

/// Kai's own variables: taken from the handler over the session's.
const KAI_OWN: [&str; 4] = [
    "KAI_LOG",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_DATA_HOME",
];
/// What Kai and `systemd-run` cannot start without: taken from the handler when the session's
/// environment lacks them.
const NEEDED: [&str; 6] = [
    "PATH",
    "HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
];

/// The scope unit name for a start by the scheme handler.
pub fn scope_unit() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!(
        "app-{}-pair{:x}{:x}.scope",
        crate::APP_ID,
        std::process::id(),
        nanos
    )
}

/// The systemd user manager's environment (`Manager.Environment`), or `None` with a log line when
/// it cannot be read within `timeout` (`dbus.timeout_s`): Kai then starts with the handler's own.
pub async fn session_environment(bus: &zbus::Connection, timeout: Duration) -> Option<Vec<String>> {
    let call = bus.call_method(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.freedesktop.systemd1.Manager", "Environment"),
    );
    let fail = |why: String| {
        tracing::warn!(
            "the session's environment is unknown ({why}); Kai starts with the handler's"
        );
        None
    };
    match tokio::time::timeout(timeout, call).await {
        Err(_) => fail(format!(
            "systemd did not answer within {} s (dbus.timeout_s)",
            timeout.as_secs()
        )),
        Ok(Err(e)) => fail(e.to_string()),
        Ok(Ok(reply)) => match reply
            .body()
            .deserialize::<zbus::zvariant::OwnedValue>()
            .map_err(|e| e.to_string())
            .and_then(|v| Vec::<String>::try_from(v).map_err(|e| e.to_string()))
        {
            Ok(list) => Some(list),
            Err(e) => fail(e),
        },
    }
}

/// The environment Kai starts with: the session's `K=V` entries, Kai's own variables from `own`
/// (the handler's environment) over them, and what is needed taken from `own` when missing.
pub fn merge_env(
    session: &[String],
    own: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = session
        .iter()
        .filter_map(|e| e.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let mut set = |k: &str, v: String| match env.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = v,
        None => env.push((k.to_owned(), v)),
    };
    for k in KAI_OWN {
        if let Some(v) = own(k) {
            set(k, v);
        }
    }
    for k in NEEDED {
        if !session
            .iter()
            .any(|e| e.split_once('=').is_some_and(|(n, _)| n == k))
            && let Some(v) = own(k)
        {
            set(k, v);
        }
    }
    env
}

/// Starts `exe --pair-stdin` in a fresh app scope with `raw` on its stdin, and returns the unit
/// name. Nothing is waited for: the link sits in the pipe until Kai reads it. `session` is
/// [`session_environment`]'s answer.
pub fn start_with_link(
    exe: &Path,
    raw: &str,
    session: Option<Vec<String>>,
) -> std::io::Result<String> {
    if raw.len() > STDIN_LINK_MAX {
        return Err(std::io::Error::other(format!(
            "the pairing link is longer than {STDIN_LINK_MAX} bytes"
        )));
    }
    let (reader, mut writer) = std::io::pipe()?;
    // Never block: a pipe that cannot take the link whole is an error, not a hang.
    {
        use std::os::fd::AsRawFd;
        let fd = writer.as_raw_fd();
        // SAFETY: fcntl on a pipe fd this function owns.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    writer.write_all(raw.as_bytes())?;
    drop(writer);
    let unit = scope_unit();
    let mut cmd = Command::new("systemd-run");
    cmd.args(scope_args(&unit, exe)).stdin(Stdio::from(reader));
    if let Some(session) = session {
        cmd.env_clear()
            .envs(merge_env(&session, |k| std::env::var(k).ok()));
    }
    cmd.spawn()?;
    Ok(unit)
}

/// `systemd-run`'s arguments for Kai in its own app scope: `systemd-cat -t kai` execs Kai with its
/// stdout and stderr on the journal under `kai`, so its log does not go to the browser's stderr
/// (review KAI-2 K2-23); stdin, the link's pipe, stays.
fn scope_args(unit: &str, exe: &Path) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = [
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--unit",
        unit,
        "--",
        "systemd-cat",
        "-t",
        "kai",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    v.push(exe.into());
    v.push("--pair-stdin".into());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kai_starts_in_its_scope_logging_to_the_journal() {
        let args = scope_args("app-dev.rootshell.kai-1.scope", Path::new("/usr/bin/kai"));
        let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--unit",
                "app-dev.rootshell.kai-1.scope",
                "--",
                "systemd-cat",
                "-t",
                "kai",
                "/usr/bin/kai",
                "--pair-stdin"
            ]
        );
    }

    #[test]
    fn the_random_part_has_no_dash() {
        let unit = scope_unit();
        let rand = unit
            .strip_prefix("app-dev.rootshell.kai-")
            .and_then(|r| r.strip_suffix(".scope"))
            .unwrap();
        assert!(
            !rand.is_empty() && rand.bytes().all(|b| b.is_ascii_alphanumeric()),
            "{unit}"
        );
    }

    #[test]
    fn kai_starts_with_the_sessions_environment_not_the_callers() {
        let session: Vec<String> = [
            "PATH=/usr/bin",
            "HOME=/home/o",
            "WAYLAND_DISPLAY=wayland-0",
            "XDG_RUNTIME_DIR=/run/user/1000",
            "XDG_SESSION_TYPE=wayland",
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus",
            "LANG=de_DE.UTF-8",
        ]
        .map(String::from)
        .to_vec();
        let browser = |k: &str| {
            match k {
                "GDK_BACKEND" => Some("x11"),
                "LD_PRELOAD" => Some("/tmp/x.so"),
                "PATH" => Some("/opt/browser:/usr/bin"),
                "KAI_LOG" => Some("debug"),
                "XDG_CONFIG_HOME" => Some("/tmp/live/config"),
                _ => None,
            }
            .map(str::to_owned)
        };
        let env = merge_env(&session, browser);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(
            get("GDK_BACKEND"),
            None,
            "the caller's toolkit choice stays behind"
        );
        assert_eq!(get("LD_PRELOAD"), None);
        assert_eq!(get("PATH"), Some("/usr/bin"), "the session's");
        assert_eq!(get("WAYLAND_DISPLAY"), Some("wayland-0"));
        assert_eq!(get("LANG"), Some("de_DE.UTF-8"));
        assert_eq!(get("KAI_LOG"), Some("debug"), "Kai's own come along");
        assert_eq!(get("XDG_CONFIG_HOME"), Some("/tmp/live/config"));
        // A session without the bus address: the handler's fills the gap.
        let thin = vec!["PATH=/usr/bin".to_owned()];
        let env = merge_env(&thin, |k| {
            (k == "DBUS_SESSION_BUS_ADDRESS").then(|| "unix:x".into())
        });
        assert!(env.contains(&("DBUS_SESSION_BUS_ADDRESS".into(), "unix:x".into())));
    }

    #[test]
    fn the_link_goes_through_a_pipe_that_ends() {
        // What `start_with_link` hands Kai: the whole link, then EOF, with no writer left.
        use std::io::Read;
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"lmgw-pair:?v=1").unwrap();
        drop(writer);
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "lmgw-pair:?v=1");
        let long = "x".repeat(STDIN_LINK_MAX + 1);
        let e = start_with_link(Path::new("/nonexistent/kai"), &long, None).unwrap_err();
        assert!(e.to_string().contains("16384"), "{e}");
    }
}
