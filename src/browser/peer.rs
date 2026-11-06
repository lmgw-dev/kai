//! Who may use `browser.sock` (kai-firefox design §4.4): a process of Kai's own user inside
//! Firefox's systemd unit, as `browser.peer_scopes` names it.
//!
//! - **Read like the CLI's caller** (`crate::cli`): the peer's cgroup through its pidfd
//!   (`crate::local_socket::peer_read`), with its executable for the log. The peer is
//!   `kai-browser-host`, which Firefox starts as its own child, so it runs in Firefox's unit.
//! - **A pattern matches the unit the process runs in**: the innermost part of the cgroup path
//!   (`/user.slice/…/app.slice/<unit>`) that ends in `.service` or `.scope`, on any line of the
//!   file; `*` is any text, `?` one character, the rest literal and case-sensitive. The units and
//!   slices above it (`user@1000.service`, `app.slice`), which every process of the session is
//!   under, never count, and `browser.peer_scopes` refuses a pattern that would match one of them
//!   ([`too_wide`]). A unit name keeps systemd's escapes as the file has them (`\x2d` for a dash
//!   inside a desktop id).
//! - **What Plasma names a Firefox it starts** (the defaults, [`DEFAULT_SCOPES`]): Plasma 6 starts
//!   an application from the launcher, the task manager or an opened link through KIO, as a
//!   transient service `app-<desktop id>@<random>.service`; XDG autostart (systemd's generator)
//!   makes `app-<desktop id>@autostart.service`; a start as a scope (`systemd-run --scope`, KIO
//!   with `KDE_APPLICATIONS_AS_SCOPE`, GNOME's launcher) makes `app-<id>-<random>.scope`, the form
//!   Kai's own `just run` and routines use (`crate::cli`). Fedora's desktop id is
//!   `org.mozilla.firefox`, other distributions' `firefox`. When the user picks Firefox in an
//!   "Open with" dialog, Plasma writes its own entry `userapp-Firefox-<random>.desktop` and starts
//!   Firefox from it as `app-userapp\x2dFirefox\x2d<random>@<uuid>.service` (a Firefox started from a
//!   link Plasma's welcome window opened runs in such a unit).
//!   *Assumption, not measured yet:* the menu entry's own start (`org.mozilla.firefox`) and the
//!   autostart unit, which follow from KIO's and systemd's naming; WP0 (a)'s runs with a real
//!   extension are the check.
//! - **Not let in:** a Firefox started from a terminal (it runs in the terminal's unit, such as
//!   `app-org.kde.konsole-<random>.scope`), from an IDE or by a test harness, unless a pattern
//!   names that unit. The refusal is logged with the peer's executable and cgroup, so the owner
//!   sees what to add; the extension gets the reason in a `refused` frame.
//! - **This raises the bar; it is not a boundary** (design §4.4): a process of the same user can
//!   move itself into Firefox's unit.

/// `browser.peer_scopes`' defaults (module doc).
pub const DEFAULT_SCOPES: [&str; 5] = [
    "app-org.mozilla.firefox@*.service",
    "app-org.mozilla.firefox-*.scope",
    "app-firefox@*.service",
    "app-firefox-*.scope",
    "app-userapp\\x2dFirefox\\x2d*",
];

/// The process that connected, as `/proc` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub pid: i32,
    /// `/proc/<pid>/cgroup` as read.
    pub cgroup: String,
    /// `/proc/<pid>/exe`'s target, for the log; `None` when it cannot be read.
    pub exe: Option<String>,
    /// Its parent (`PPid` of `/proc/<pid>/status`): for `kai-browser-host`, Firefox's main process,
    /// which owns its windows (`super::desk`); `None` when it cannot be read.
    pub parent: Option<i32>,
}

impl Peer {
    /// The cgroup's unified-hierarchy path (`0::<path>`), else its first line, for a log line.
    pub fn cgroup_line(&self) -> &str {
        self.cgroup
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .or_else(|| self.cgroup.lines().next())
            .unwrap_or("")
    }

    pub fn exe_or_unknown(&self) -> &str {
        self.exe.as_deref().unwrap_or("(unknown)")
    }
}

/// The process `pid` from `/proc`: its cgroup (required) and executable.
pub fn proc_peer(pid: i32) -> Option<Peer> {
    let cgroup = crate::local_socket::proc_cgroup(pid)?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.display().to_string());
    let parent = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| parent_of(&s));
    Some(Peer {
        pid,
        cgroup,
        exe,
        parent,
    })
}

/// The `PPid` line of a `/proc/<pid>/status` text.
pub fn parent_of(status: &str) -> Option<i32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("PPid:"))
        .and_then(|v| v.trim().parse().ok())
}

/// The unit a process with the cgroup path `path` runs in: its innermost part that ends in
/// `.service` or `.scope` (a part below it, a cgroup the unit made inside itself, is not a unit).
pub fn innermost_unit(path: &str) -> Option<&str> {
    path.split('/')
        .rev()
        .find(|part| part.ends_with(".service") || part.ends_with(".scope"))
}

/// The unit of the cgroup file `text` (one per line, [`innermost_unit`]) that one of `patterns`
/// matches, if any.
pub fn matching_unit<'a>(text: &'a str, patterns: &[String]) -> Option<&'a str> {
    crate::local_socket::cgroup_paths(text)
        .filter_map(innermost_unit)
        .find(|unit| patterns.iter().any(|p| glob(p, unit)))
}

/// Why `pattern` would let in more than one application, if it would: it matches the user
/// manager's unit `user@<uid>.service` or one of the slices every process of the session is
/// under (`user.slice`, `user-<uid>.slice`, `app.slice`, `session.slice`, `background.slice`), or
/// it names slices at all (ends in `.slice`), which are never the unit a process runs in.
pub fn too_wide(pattern: &str, uid: u32) -> Option<String> {
    if pattern.ends_with(".slice") {
        return Some(
            "it names a slice, which holds many units; each entry names the unit Firefox runs in \
             (a .service or a .scope)"
                .into(),
        );
    }
    let manager = format!("user@{uid}.service");
    let user_slice = format!("user-{uid}.slice");
    [
        manager.as_str(),
        "user.slice",
        user_slice.as_str(),
        "app.slice",
        "session.slice",
        "background.slice",
    ]
    .into_iter()
    .find(|name| glob(pattern, name))
    .map(|name| {
        format!(
            "it also matches {name}, which every program of the session runs under; each entry \
             names only the unit Firefox runs in (a .service or a .scope)"
        )
    })
}

/// `pattern` matches the whole of `text`: `*` any run of characters (none included), `?` exactly
/// one, every other character itself.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    // Where the last `*` was and the text position it stood for, to take one more character.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || (p[pi] != '*' && p[pi] == t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Why a peer is not let in, for the log and the `refused` frame; `Ok` with the unit it is in.
pub fn check(
    peer_uid: u32,
    own_uid: u32,
    peer: Option<&Peer>,
    patterns: &[String],
) -> Result<String, String> {
    if peer_uid != own_uid {
        return Err(format!(
            "Kai links only to its own user's Firefox (this process is user {peer_uid}'s)"
        ));
    }
    let Some(peer) = peer else {
        return Err("Kai could not read which program connected (it may have exited)".into());
    };
    if patterns.is_empty() {
        return Err(
            "Kai lets no Firefox link: its list of Firefox's app scopes is empty (Kai's settings, \
             Firefox)"
                .into(),
        );
    }
    match matching_unit(&peer.cgroup, patterns) {
        Some(unit) => Ok(unit.to_owned()),
        None => Err(format!(
            "Kai links only to a Firefox started from Plasma (its own app scope); this one runs \
             in {}. A Firefox started from a terminal runs in the terminal's: start it from the \
             menu, or add its unit to Firefox's app scopes in Kai's settings",
            peer.cgroup_line()
        )),
    }
}
