//! The terminal an application whose desktop entry asks for one (`Terminal=true`) starts in
//! (WP16a; the order decided by the owner, 2026-10-09). With `launch.terminal` set, that command
//! line, always; unset, the first of these that is installed:
//!
//! 1. **Plasma's configured terminal:** `[General] TerminalApplication` of `kdeglobals`, read as
//!    KConfig reads it: the cascade from the least important of `$XDG_CONFIG_DIRS` (default
//!    `/etc/xdg`) to the user's own `$XDG_CONFIG_HOME/kdeglobals`, the later file winning unless an
//!    earlier one marks the group or the key immutable (`[$i]`); `[$d]` deletes an earlier value,
//!    KConfig's escapes (`\s`, `\t`, `\n`, `\\`) are undone. KIO's `DesktopExecParser` reads the
//!    key with `readPathEntry`, which expands it with or without `[$e]`, so it is expanded as
//!    KConfig 6's `expandString` does ([`expand`]): `$VAR` and `${VAR}` from the session's
//!    environment, `$$` a `$`; there is no `~` and no `$(command)` (never run: a `$` before
//!    anything but a name, `{` or `$` is dropped, as KConfig drops it). Its words, then `-e`, as
//!    `DesktopExecParser` starts a terminal application.
//! 2. **`xdg-terminal-exec`** on the session's `PATH` (the command line follows it as it is).
//! 3. **Konsole** (`konsole -e`) when the session is Plasma (`XDG_CURRENT_DESKTOP` names KDE), KDE's
//!    own fallback.
//!
//! Only when none of them is there is it an error, and that names `launch.terminal`. A candidate
//! is "there" when its program is found on the session's `PATH` (or at its absolute path).

use std::path::{Path, PathBuf};

use super::entries::{Dirs, exec_words, find_program};

/// Where the terminal came from, for the log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `launch.terminal`.
    Setting,
    /// `kdeglobals`' `TerminalApplication`, from this file.
    Plasma(PathBuf),
    XdgTerminalExec,
    /// Konsole on Plasma, with no terminal configured.
    Konsole,
}

impl Source {
    /// The terminal as the owner would look for it, for an error: "<this> is not installed".
    pub fn describe(&self) -> String {
        match self {
            Source::Setting => "the terminal launch.terminal names".into(),
            Source::Plasma(file) => format!(
                "Plasma's terminal (TerminalApplication in {})",
                file.display()
            ),
            Source::XdgTerminalExec => "xdg-terminal-exec".into(),
            Source::Konsole => "Konsole, Plasma's fallback terminal".into(),
        }
    }
}

/// The command line an application's own is appended to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub argv: Vec<String>,
    pub source: Source,
}

/// The refusal when `launch.terminal` is set to an empty list.
pub const NAMES_NONE: &str = "launch.terminal names none";

/// The terminal for a `Terminal=true` application (module doc). `env` is the session's environment
/// (for `[$e]` values). The error is the sentence's end after "<app> runs in a terminal, and ".
pub fn resolve(
    configured: Option<&[String]>,
    dirs: &Dirs,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Terminal, String> {
    if let Some(argv) = configured {
        if argv.is_empty() {
            return Err(NAMES_NONE.to_owned());
        }
        return Ok(Terminal {
            argv: argv.to_vec(),
            source: Source::Setting,
        });
    }
    let installed = |program: &str| find_program(program, &dirs.path, &dirs.home).is_some();
    let mut tried = Vec::new();
    match plasma_terminal(dirs, env) {
        Some((value, file)) => match exec_words(&value) {
            Ok(words) if words.first().is_some_and(|p| installed(p)) => {
                let mut argv = words;
                argv.push("-e".into());
                return Ok(Terminal {
                    argv,
                    source: Source::Plasma(file),
                });
            }
            Ok(_) => tried.push(format!(
                "Plasma's terminal ({value:?} in {}) is not installed",
                file.display()
            )),
            Err(_) => tried.push(format!(
                "Plasma's terminal ({value:?} in {}) has an unclosed quote",
                file.display()
            )),
        },
        None => tried.push("Plasma names no terminal (kdeglobals' TerminalApplication)".into()),
    }
    if installed("xdg-terminal-exec") {
        return Ok(Terminal {
            argv: vec!["xdg-terminal-exec".into()],
            source: Source::XdgTerminalExec,
        });
    }
    tried.push("xdg-terminal-exec is not installed".into());
    let plasma = dirs.desktops.iter().any(|d| d.eq_ignore_ascii_case("KDE"));
    if plasma && installed("konsole") {
        return Ok(Terminal {
            argv: vec!["konsole".into(), "-e".into()],
            source: Source::Konsole,
        });
    }
    tried.push(if plasma {
        "Konsole is not installed".into()
    } else {
        "this is not a Plasma session (no Konsole fallback)".into()
    });
    Err(format!(
        "no terminal was found ({}); set launch.terminal to one, e.g. [\"konsole\", \"-e\"]",
        tried.join(", ")
    ))
}

/// `[General] TerminalApplication` from the `kdeglobals` cascade, expanded as `readPathEntry`
/// expands it, with the file it came from.
pub fn plasma_terminal(
    dirs: &Dirs,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, PathBuf)> {
    // Least important first: `XDG_CONFIG_DIRS` lists the most important first; the user's own last.
    let files = dirs
        .config_dirs
        .iter()
        .rev()
        .chain(std::iter::once(&dirs.config_home))
        .map(|d| d.join("kdeglobals"));
    let mut found: Option<(String, PathBuf)> = None;
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        match read_key(&text, "General", "TerminalApplication") {
            Read::Absent => {}
            Read::Deleted => found = None,
            Read::Value(v, immutable) => {
                found = Some((v, file.clone()));
                if immutable {
                    break;
                }
            }
            Read::Locked(v) => {
                // The group or file is immutable from here: what it holds stands (none included).
                found = v.map(|v| (v, file.clone())).or(found);
                break;
            }
        }
    }
    found
        .map(|(v, file)| (expand(&v, env), file))
        .filter(|(v, _)| !v.trim().is_empty())
}

/// What one file says about one key.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    Absent,
    /// `key[$d]`: an earlier file's value is gone.
    Deleted,
    /// The value, and whether the key is immutable (`key[$i]`).
    Value(String, bool),
    /// The file or the group is immutable (`[$i]`): its value, if any, ends the cascade.
    Locked(Option<String>),
}

/// One KConfig file's value of `key` in `[group]`, its escapes undone (module doc). `[$e]` changes
/// nothing here: `readPathEntry` expands the value either way ([`plasma_terminal`]).
fn read_key(text: &str, group: &str, key: &str) -> Read {
    let mut file_locked = false;
    let mut in_group = false;
    let mut group_locked = false;
    let mut out = Read::Absent;
    let mut seen_group = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // `[$i]` before any group locks the whole file.
            if !seen_group && line == "[$i]" {
                file_locked = true;
                continue;
            }
            seen_group = true;
            let (name, flags) = group_header(line);
            in_group = name == group;
            if in_group {
                group_locked |= flags.contains('i');
            }
            continue;
        }
        if !in_group {
            continue;
        }
        // `key[$d]` is written without a value.
        let (k, v) = line.split_once('=').unwrap_or((line, ""));
        let (name, flags) = key_name(k.trim_end());
        // A localised value (`key[de]`) is not the key's own.
        let Some(flags) = flags else { continue };
        if name != key {
            continue;
        }
        if flags.contains('d') {
            out = Read::Deleted;
            continue;
        }
        if !line.contains('=') {
            continue;
        }
        let value = unescape(v.trim_start());
        // The first immutable value in a file is the one that stands.
        let immutable = flags.contains('i');
        out = Read::Value(value, immutable);
        if immutable {
            break;
        }
    }
    if file_locked || group_locked {
        return Read::Locked(match out {
            Read::Value(v, _) => Some(v),
            _ => None,
        });
    }
    out
}

/// `[General]` or `[General][$i]` (nested groups `[A][B]` are not `[General]`): the name and the
/// flags.
fn group_header(line: &str) -> (String, String) {
    let mut parts = Vec::new();
    let mut flags = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            break;
        };
        let part = &rest[start + 1..start + end];
        match part.strip_prefix('$') {
            Some(f) => flags.push_str(f),
            None => parts.push(part.to_owned()),
        }
        rest = &rest[start + end + 1..];
    }
    (parts.join("]["), flags)
}

/// `key`, `key[$ie]` (its flags) or `key[de]` (a locale: `None`).
fn key_name(k: &str) -> (&str, Option<String>) {
    let Some(open) = k.find('[') else {
        return (k, Some(String::new()));
    };
    let name = &k[..open];
    let mut flags = String::new();
    for part in k[open..].split('[').filter(|p| !p.is_empty()) {
        let part = part.trim_end_matches(']');
        match part.strip_prefix('$') {
            Some(f) => flags.push_str(f),
            None => return (name, None),
        }
    }
    (name, Some(flags))
}

/// KConfig's escapes in a value.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// A value expanded as KConfig 6's `KConfigPrivate::expandString` expands it (6.30): `$NAME`
/// (letters, digits and `_`) and `${NAME}` (an unclosed one runs to the end) from `env`, an unset or
/// empty one is empty, except `QT_DATA_HOME`, `QT_CONFIG_HOME` and `QT_CACHE_HOME`, which are then
/// Qt's writable XDG locations; `$$` is a `$`; a `$` before anything else is dropped, one at the
/// end kept. Nothing is run and nothing expanded twice.
pub fn expand(v: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let lookup = |name: &str| -> String {
        if let Some(value) = env(name).filter(|v| !v.is_empty()) {
            return value;
        }
        let xdg = |var: &str, under_home: &str| {
            env(var)
                .filter(|d| d.starts_with('/'))
                .or_else(|| env("HOME").map(|h| format!("{h}/{under_home}")))
                .unwrap_or_default()
        };
        match name {
            "QT_DATA_HOME" => xdg("XDG_DATA_HOME", ".local/share"),
            "QT_CONFIG_HOME" => xdg("XDG_CONFIG_HOME", ".config"),
            "QT_CACHE_HOME" => xdg("XDG_CACHE_HOME", ".cache"),
            _ => String::new(),
        }
    };
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if after.is_empty() {
            // A `$` at the end is kept.
            out.push('$');
            rest = after;
        } else if let Some(r) = after.strip_prefix('$') {
            out.push('$');
            rest = r;
        } else if let Some(r) = after.strip_prefix('{') {
            let (name, r) = r.split_once('}').unwrap_or((r, ""));
            if !name.is_empty() {
                out.push_str(&lookup(name));
            }
            rest = r;
        } else {
            let len = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if len > 0 {
                out.push_str(&lookup(&after[..len]));
            }
            // Without a name the `$` alone is dropped.
            rest = &after[len..];
        }
    }
    out.push_str(rest);
    out
}

/// The program a terminal's command line starts, for the log (never the application's arguments).
pub fn program(t: &Terminal) -> &str {
    t.argv
        .first()
        .map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(p)
        })
        .unwrap_or("")
}
