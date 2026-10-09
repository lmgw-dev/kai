//! One routine as its directory declares it (K28): `<dir>/<name>/routine.toml`, and for a script the
//! executable beside it.
//!
//! ```toml
//! description = "Back up the notes folder to the NAS."   # one line (K22)
//! kind = "script"         # script | steps | goal
//! script = "run.sh"       # script: the executable in this directory (default run.sh), any
//!                         # language with a shebang; its parameters are its arguments, in order
//!                         # (an omitted optional one is an empty argument)
//! # Optional, with their defaults:
//! read_only = false       # MCP annotations; a read-only routine asks to run for trusted
//! destructive = false     # callers unasked (below)
//! open_world = false      # it reaches the network
//! policy = "hints"        # what it asks of K15: hints (from read_only), trusted (trusted callers
//!                         # unasked, the others confirmed) or always (confirmed for everyone)
//! level = "any"           # who it asks to be callable by: own, owner, trusted, any
//! async = false           # K26's open-ended class: lmgw calls it as a job (WP18), its result later
//! announce = true         # ruling 22, for an async routine (default: on when started by voice)
//!
//! [[params]]              # at most two (K22), each a plain value
//! name = "folder"
//! kind = "text"           # text | integer | number | boolean
//! optional = false
//! description = "the folder under ~/Notes"   # only where the name is not enough
//!
//! # steps: fixed tool calls, no model; "{folder}" in a text is the parameter's value, and a text
//! # that is only "{folder}" is the value itself (omitted when the call omitted it)
//! [[steps]]
//! tool = "notify"
//! args = { title = "Backup", body = "{folder} saved" }
//!
//! # goal: the delegate loop (K25) with these tools, on this alias (default: the Kai
//! # conversation's)
//! goal = "Tile the two newest terminal windows side by side."
//! tools = ["window_list", "window_tile"]
//! alias = "gemma4-12b"
//! ```
//!
//! **What Kai honours of `policy`, `read_only` and `level`** (K28; a draft of 2026-10-09): at most
//! confirm and the owner's level. The folder can change after the owner switched the routine on (no
//! pin, the owner's decision), by anything that can write it, so the file cannot raise its own
//! permissions: unasked runs for trusted callers and a level above `owner` are only [`Asks`],
//! granted by the owner's `policy.tools.<name> = "allow_trusted"` and
//! `policy.levels.<name>` (K30's guard covers those), which the switch-on dialog offers to write.
//! `always`, `own` and `owner` are honoured as written.
//!
//! The directory's name is the routine's name and its tool's: lowercase letters, digits and single
//! underscores, no name of Kai's own tools; `name`, when the file has it, must be the same. The
//! spec is checked by the contract's rules ([`kai_desktop::contract::check`]) like every tool.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use kai_desktop::contract::{self, Ask, Class, Hints, Level, Param, ToolSpec};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::host::settings::LevelSetting;

/// The routine's file in its directory.
pub const FILE: &str = "routine.toml";
/// A script routine's executable when `script` names none.
pub const DEFAULT_SCRIPT: &str = "run.sh";

/// A tool's name by the contract's rule: lowercase letters, digits and single underscores, starting
/// with a letter.
pub fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__")
        && !name.ends_with('_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum KindWord {
    Script,
    Steps,
    Goal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PolicyWord {
    #[default]
    Hints,
    Trusted,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ParamKind {
    Text,
    Integer,
    Number,
    Boolean,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParam {
    name: String,
    kind: ParamKind,
    #[serde(default)]
    optional: bool,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    tool: String,
    #[serde(default)]
    args: toml::Table,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: Option<String>,
    description: String,
    kind: KindWord,
    #[serde(default)]
    params: Vec<RawParam>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    destructive: bool,
    #[serde(default)]
    open_world: bool,
    #[serde(default)]
    policy: PolicyWord,
    level: Option<LevelSetting>,
    #[serde(default, rename = "async")]
    is_async: bool,
    announce: Option<bool>,
    script: Option<String>,
    #[serde(default)]
    steps: Vec<RawStep>,
    goal: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    alias: Option<String>,
}

/// One fixed call of a `steps` routine.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tool: String,
    pub args: Map<String, Value>,
}

/// What the routine does when called.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// The executable, resolved in the routine's directory.
    Script {
        path: PathBuf,
    },
    Steps(Vec<Step>),
    Goal {
        goal: String,
        tools: Vec<String>,
        alias: Option<String>,
    },
}

impl Body {
    pub fn word(&self) -> &'static str {
        match self {
            Body::Script { .. } => "script",
            Body::Steps(_) => "steps",
            Body::Goal { .. } => "goal",
        }
    }
}

/// A routine that loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub name: String,
    pub dir: PathBuf,
    /// Its tool, as every hosted tool is declared (interned: the same declaration is the same
    /// `&'static` spec on every load).
    pub spec: &'static ToolSpec,
    pub body: Body,
    /// Ruling 22's flag for an async routine; `None`: on when started by voice.
    pub announce: Option<bool>,
    /// What `routine.toml` asks beyond what Kai honours from it (module doc).
    pub asks: Asks,
}

/// What a routine's `routine.toml` asks for beyond Kai's default for a routine, confirm and the
/// owner's level (module doc): its [`spec`](Routine::spec) holds the default, this the rest, which
/// only the owner's `policy.tools.<name>` and `policy.levels.<name>` grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Asks {
    /// To run unasked for the trusted callers (`policy = "trusted"`, or `read_only = true` under
    /// `policy = "hints"`): `policy.tools.<name> = "allow_trusted"` grants it.
    pub allow_trusted: bool,
    /// A level above `owner` (`trusted`, or `any`, which an absent `level` means):
    /// `policy.levels.<name>` grants it.
    pub level: Option<LevelSetting>,
}

impl Asks {
    pub fn is_empty(&self) -> bool {
        !self.allow_trusted && self.level.is_none()
    }

    /// The owner's entries that grant it, `(key, value)` as `config_set` takes them.
    pub fn grants(&self, name: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.allow_trusted {
            out.push((
                format!("policy.tools.{name}"),
                "\"allow_trusted\"".to_owned(),
            ));
        }
        if let Some(l) = self.level {
            let word = match l {
                LevelSetting::Own => "own",
                LevelSetting::Owner => "owner",
                LevelSetting::Trusted => "trusted",
                LevelSetting::Any => "any",
            };
            out.push((format!("policy.levels.{name}"), format!("\"{word}\"")));
        }
        out
    }
}

/// A routine directory as Kai read it: the routine, or why it does not load (said in the log and
/// the tray).
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub name: String,
    pub routine: Result<Routine, String>,
}

/// Reads the routine in `dir` (whose last component is its name): its folder once, the routine
/// parsed from exactly the bytes it read.
pub fn load(dir: &Path) -> Loaded {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Err(why) = check_name(&name) {
        return Loaded {
            name,
            routine: Err(why),
        };
    }
    match read_folder(dir, |f| f == FILE) {
        Ok(folder) => Loaded {
            routine: folder.routine(dir, &name),
            name,
        },
        Err(why) => Loaded {
            name,
            routine: Err(why),
        },
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if !plain_name(name) {
        return Err(format!(
            "the folder's name is the routine's: lowercase letters, digits and single \
             underscores, starting with a letter ({name:?} is not)"
        ));
    }
    if contract::find(name).is_some() {
        return Err(format!(
            "{name} is the name of one of Kai's own tools; name the folder otherwise"
        ));
    }
    Ok(())
}

#[cfg(test)]
fn read(dir: &Path, name: &str) -> Result<Routine, String> {
    check_name(name)?;
    read_folder(dir, |f| f == FILE)?.routine(dir, name)
}

/// One entry at the top of a routine's folder as Kai read it.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderEntry {
    /// Its name as the dialog shows it (the file's own bytes).
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    /// A file (a link to one read through it): its size and SHA-256 as read, and its bytes when
    /// they were kept.
    File {
        size: u64,
        sha256: String,
        bytes: Option<Vec<u8>>,
    },
    /// A subfolder: named, never read, never shown (a script may keep its state there).
    Folder,
    /// Anything else (a link that leads nowhere, a pipe, a socket): named, not read.
    Other(&'static str),
}

/// The top of a routine's folder, read once: what the switch-on dialog shows in full.
#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    pub entries: Vec<FolderEntry>,
}

impl Folder {
    /// The entry `name`, a file's.
    pub fn file(&self, name: &str) -> Option<(&u64, &str, Option<&[u8]>)> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .and_then(|e| match &e.kind {
                EntryKind::File {
                    size,
                    sha256,
                    bytes,
                } => Some((size, sha256.as_str(), bytes.as_deref())),
                _ => None,
            })
    }

    /// The routine as `routine.toml` reads in this read of the folder (it must have been kept).
    pub fn routine(&self, dir: &Path, name: &str) -> Result<Routine, String> {
        let Some((_, _, bytes)) = self.file(FILE) else {
            return Err(format!("no {FILE} in the folder"));
        };
        let bytes = bytes.ok_or_else(|| format!("{FILE} was not kept when it was read"))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| format!("{FILE} cannot be read: it is not text (UTF-8)"))?;
        let routine = parse(dir, name, text)?;
        if let Body::Script { path } = &routine.body {
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if self.file(&file).is_none() {
                return Err(format!(
                    "the script {file} is not a file in the routine's folder"
                ));
            }
        }
        Ok(routine)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Reads the top of `dir` (module doc of [`Folder`]): every file is read through once and hashed;
/// the bytes of the files `keep` names are kept. A pipe or a socket is never opened for reading
/// (opened without blocking and looked at first). An entry that cannot be read is the error.
pub fn read_folder(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Folder, String> {
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

    use sha2::{Digest, Sha256};

    let rd = std::fs::read_dir(dir).map_err(|e| format!("the folder cannot be read: {e}"))?;
    let mut raw: Vec<(Vec<u8>, String)> = Vec::new();
    for e in rd {
        let e = e.map_err(|e| format!("the folder cannot be read: {e}"))?;
        let n = e.file_name();
        raw.push((n.as_bytes().to_vec(), n.to_string_lossy().into_owned()));
    }
    raw.sort();
    let mut entries = Vec::with_capacity(raw.len());
    for (name_bytes, name) in raw {
        let path = dir.join(std::ffi::OsStr::from_bytes(&name_bytes));
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(&path);
        let kind = match opened {
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ENOENT | libc::ELOOP | libc::ENXIO)
                ) =>
            {
                EntryKind::Other(match std::fs::symlink_metadata(&path) {
                    Ok(m) if m.file_type().is_socket() => "socket",
                    Ok(m) if m.file_type().is_symlink() => "link that leads nowhere",
                    _ => "entry that cannot be opened",
                })
            }
            Err(e) => return Err(format!("{name} cannot be read: {e}")),
            Ok(mut f) => {
                let m = f
                    .metadata()
                    .map_err(|e| format!("{name} cannot be read: {e}"))?;
                let t = m.file_type();
                if t.is_dir() {
                    EntryKind::Folder
                } else if !t.is_file() {
                    EntryKind::Other(if t.is_fifo() {
                        "pipe"
                    } else if t.is_char_device() || t.is_block_device() {
                        "device"
                    } else {
                        "special file"
                    })
                } else {
                    let kept = keep(&name);
                    let mut bytes = Vec::new();
                    let mut sha = Sha256::new();
                    let mut size = 0u64;
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let n = match f.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err(format!("{name} cannot be read: {e}")),
                        };
                        sha.update(&buf[..n]);
                        size += n as u64;
                        if kept {
                            bytes.extend_from_slice(&buf[..n]);
                        }
                    }
                    EntryKind::File {
                        size,
                        sha256: hex(&sha.finalize()),
                        bytes: kept.then_some(bytes),
                    }
                }
            }
        };
        entries.push(FolderEntry { name, kind });
    }
    Ok(Folder { entries })
}

/// `text` as the routine `name` in `dir` (the tests give theirs).
pub fn parse(dir: &Path, name: &str, text: &str) -> Result<Routine, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| {
        let text = e.to_string();
        let one: Vec<&str> = text.lines().map(str::trim).collect();
        format!("{FILE}: {}", one.join(" "))
    })?;
    if let Some(n) = &raw.name
        && n != name
    {
        return Err(format!(
            "{FILE} names it {n:?}, its folder {name:?}: the folder's name is the routine's"
        ));
    }
    let only = |what: &str, set: bool, kinds: &str| {
        if set {
            Err(format!("{what} is for {kinds} routines only"))
        } else {
            Ok(())
        }
    };
    let body = match raw.kind {
        KindWord::Script => {
            only("steps", !raw.steps.is_empty(), "steps")?;
            only("goal", raw.goal.is_some(), "goal")?;
            only("tools", !raw.tools.is_empty(), "goal")?;
            only("alias", raw.alias.is_some(), "goal")?;
            let file = raw.script.as_deref().unwrap_or(DEFAULT_SCRIPT);
            if file.is_empty() || file.contains('/') || file.starts_with('.') {
                return Err(format!(
                    "script = {file:?}: the name of a file in the routine's folder"
                ));
            }
            let path = dir.join(file);
            executable(&path)?;
            Body::Script { path }
        }
        KindWord::Steps => {
            only("script", raw.script.is_some(), "script")?;
            only("goal", raw.goal.is_some(), "goal")?;
            only("tools", !raw.tools.is_empty(), "goal")?;
            only("alias", raw.alias.is_some(), "goal")?;
            if raw.steps.is_empty() {
                return Err("a steps routine needs at least one [[steps]]".into());
            }
            let steps = raw
                .steps
                .into_iter()
                .map(|s| {
                    let args = match serde_json::to_value(&s.args) {
                        Ok(Value::Object(m)) => m,
                        _ => return Err(format!("{}: the args do not read", s.tool)),
                    };
                    Ok(Step { tool: s.tool, args })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Body::Steps(steps)
        }
        KindWord::Goal => {
            only("script", raw.script.is_some(), "script")?;
            only("steps", !raw.steps.is_empty(), "steps")?;
            let goal = raw.goal.unwrap_or_default();
            if goal.trim().is_empty() {
                return Err("a goal routine needs goal, in plain words".into());
            }
            if raw.tools.is_empty() {
                return Err("a goal routine needs tools, the tools its loop may use".into());
            }
            if raw.alias.as_deref().is_some_and(|a| a.trim().is_empty()) {
                return Err(
                    "alias = \"\": an lmgw alias, or leave it out for the Kai conversation's"
                        .into(),
                );
            }
            Body::Goal {
                goal,
                tools: raw.tools,
                alias: raw.alias,
            }
        }
    };
    let params: Vec<Param> = raw
        .params
        .iter()
        .map(|p| {
            let kind = match p.kind {
                ParamKind::Text => contract::Kind::Text,
                ParamKind::Integer => contract::Kind::Integer,
                ParamKind::Number => contract::Kind::Number,
                ParamKind::Boolean => contract::Kind::Boolean,
            };
            let mut param = Param::new(intern(&p.name), kind);
            if p.optional {
                param = param.optional();
            }
            if let Some(d) = &p.description {
                param = param.described(intern(d));
            }
            param
        })
        .collect();
    let declared = ToolSpec {
        name: intern(name),
        description: intern(&raw.description),
        params: intern_params(params),
        hints: Hints {
            read_only: raw.read_only,
            destructive: raw.destructive,
            idempotent: false,
            open_world: raw.open_world,
        },
        ask: match raw.policy {
            PolicyWord::Hints => Ask::Hints,
            PolicyWord::Trusted => Ask::Trusted,
            PolicyWord::Always => Ask::Always,
        },
        level: raw.level.map(Level::from).unwrap_or(Level::Any),
        class: if raw.is_async {
            Class::Async
        } else {
            Class::Sync
        },
        returns_images: false,
    };
    let problems = contract::check(&declared);
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    // What Kai honours (module doc): at most confirm and the owner's level; the rest is asked.
    let allow_trusted =
        declared.ask == Ask::Trusted || (declared.ask == Ask::Hints && declared.hints.read_only);
    let level = raw.level.unwrap_or(LevelSetting::Any);
    let asks = Asks {
        allow_trusted,
        level: matches!(level, LevelSetting::Trusted | LevelSetting::Any).then_some(level),
    };
    let spec = ToolSpec {
        ask: if allow_trusted {
            Ask::Always
        } else {
            declared.ask
        },
        level: declared.level.min(Level::Owner),
        ..declared
    };
    Ok(Routine {
        name: name.to_owned(),
        dir: dir.to_owned(),
        spec: intern_spec(spec),
        body,
        announce: raw.announce,
        asks,
    })
}

fn executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let shown = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::metadata(path) {
        Ok(m) if !m.is_file() => Err(format!("{shown} is not a file")),
        Ok(m) if m.permissions().mode() & 0o100 == 0 => Err(format!(
            "{shown} is not executable (chmod +x, and a shebang line such as #!/bin/bash)"
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("the script {shown} is not in the routine's folder"))
        }
        Err(e) => Err(format!("{shown}: {e}")),
    }
}

/// The texts of the routines' specs, kept for the process's life: a routine's tool is a
/// `&'static ToolSpec` like every other. Interned, so loading the same text again takes nothing
/// more; only a changed text adds its new version.
static STRINGS: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Mutex::default);
static PARAMS: LazyLock<Mutex<Vec<&'static [Param]>>> = LazyLock::new(Mutex::default);
static SPECS: LazyLock<Mutex<Vec<&'static ToolSpec>>> = LazyLock::new(Mutex::default);

pub fn intern(s: &str) -> &'static str {
    let mut all = STRINGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = all.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    all.insert(leaked);
    leaked
}

static NAMES: LazyLock<Mutex<HashMap<Vec<&'static str>, &'static [&'static str]>>> =
    LazyLock::new(Mutex::default);

/// A list of names for a `&'static` field (a goal's tools), interned like the texts.
pub fn intern_names(names: Vec<&'static str>) -> &'static [&'static str] {
    let mut all = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = all.get(&names) {
        return found;
    }
    let leaked: &'static [&'static str] = Box::leak(names.clone().into_boxed_slice());
    all.insert(names, leaked);
    leaked
}

fn intern_params(params: Vec<Param>) -> &'static [Param] {
    let mut all = PARAMS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = all.iter().find(|p| **p == params.as_slice()) {
        return found;
    }
    let leaked: &'static [Param] = Box::leak(params.into_boxed_slice());
    all.push(leaked);
    leaked
}

fn intern_spec(spec: ToolSpec) -> &'static ToolSpec {
    let mut all = SPECS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = all.iter().find(|s| ***s == spec) {
        return found;
    }
    let leaked: &'static ToolSpec = Box::leak(Box::new(spec));
    all.push(leaked);
    leaked
}

/// The step's arguments with the call's parameters put in (module doc): `{name}` of a `declared`
/// parameter in a text is its value as text (empty when the call omitted it), a text that is only
/// `{name}` is the value itself, left out when the call omitted it. Braces naming no parameter
/// stay as written.
pub fn fill(
    args: &Map<String, Value>,
    params: &Map<String, Value>,
    declared: &[&str],
) -> Map<String, Value> {
    let shown = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    args.iter()
        .filter_map(|(k, v)| {
            let Value::String(text) = v else {
                return Some((k.clone(), v.clone()));
            };
            if let Some(name) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}'))
                && declared.contains(&name)
            {
                return params.get(name).map(|p| (k.clone(), p.clone()));
            }
            let mut out = text.clone();
            for name in declared {
                let value = params.get(*name).map(shown).unwrap_or_default();
                out = out.replace(&format!("{{{name}}}"), &value);
            }
            Some((k.clone(), Value::String(out)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn dir() -> PathBuf {
        PathBuf::from("/nowhere/routines/backup")
    }

    #[test]
    fn a_steps_routine_reads_as_a_tool() {
        let r = parse(
            &dir(),
            "backup",
            r#"
description = "Say that the backup is done."
kind = "steps"
[[params]]
name = "folder"
kind = "text"
[[params]]
name = "loud"
kind = "boolean"
optional = true
description = "say it aloud too"
[[steps]]
tool = "notify"
args = { title = "Backup", body = "{folder} saved" }
"#,
        )
        .unwrap();
        assert_eq!(r.spec.name, "backup");
        assert_eq!(r.spec.params.len(), 2);
        assert!(r.spec.params[0].required && !r.spec.params[1].required);
        assert_eq!(r.spec.params[1].description, Some("say it aloud too"));
        assert_eq!(r.spec.ask, Ask::Hints);
        // No level: it asks for any; Kai honours the owner's level (module doc).
        assert_eq!(r.spec.level, Level::Owner);
        assert_eq!(
            r.asks,
            Asks {
                allow_trusted: false,
                level: Some(LevelSetting::Any)
            }
        );
        assert_eq!(r.spec.class, Class::Sync);
        assert!(!r.spec.hints.read_only);
        assert_eq!(
            r.body,
            Body::Steps(vec![Step {
                tool: "notify".into(),
                args: json!({"title": "Backup", "body": "{folder} saved"})
                    .as_object()
                    .cloned()
                    .unwrap()
            }])
        );
    }

    #[test]
    fn the_same_text_is_the_same_spec() {
        let text =
            "description = \"x.\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"window_list\"]\n";
        let a = parse(&dir(), "same_one", text).unwrap();
        let b = parse(&dir(), "same_one", text).unwrap();
        assert!(std::ptr::eq(a.spec, b.spec));
        let c = parse(&dir(), "same_one", &text.replace("x.", "y.")).unwrap();
        assert!(!std::ptr::eq(a.spec, c.spec));
    }

    #[test]
    fn broken_routines_say_why() {
        for (name, text, says) in [
            (
                "Bad",
                "description = \"x\"\nkind = \"goal\"",
                "folder's name",
            ),
            (
                "notify",
                "description = \"x\"\nkind = \"goal\"",
                "Kai's own tools",
            ),
            ("r", "kind = \"goal\"", "description"),
            ("r", "description = \"x\"\nkind = \"magic\"", "magic"),
            (
                "r",
                "description = \"x\"\nkind = \"goal\"\ngoal = \"g\"",
                "needs tools",
            ),
            ("r", "description = \"x\"\nkind = \"steps\"", "at least one"),
            (
                "r",
                "description = \"two\\nlines\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"a\"]",
                "not one line",
            ),
            (
                "r",
                "name = \"s\"\ndescription = \"x\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"a\"]",
                "folder's name is the routine's",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"a\"]\n\
                 [[params]]\nname = \"a\"\nkind = \"text\"\n[[params]]\nname = \"b\"\nkind = \
                 \"text\"\n[[params]]\nname = \"c\"\nkind = \"text\"",
                "at most 2",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"a\"]\nscript = \"x\"",
                "script routines only",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"a\"]\nread_only = true\npolicy = \"trusted\"",
                "Ask::Hints",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"script\"\nscript = \"../x\"",
                "a file in the routine's folder",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"script\"",
                "run.sh is not in the routine's folder",
            ),
            (
                "r",
                "description = \"x\"\nkind = \"goal\"\nextra = 1",
                "extra",
            ),
        ] {
            let e = if name == "Bad" || name == "notify" {
                read(&dir(), name).unwrap_err()
            } else {
                parse(&dir(), name, text).unwrap_err()
            };
            assert!(e.contains(says), "{text}: {e}");
        }
    }

    #[test]
    fn a_script_must_be_executable() {
        let tmp = crate::desktop::tests::bus::temp_dir();
        let d = tmp.path().join("hello");
        std::fs::create_dir_all(&d).unwrap();
        let text = "description = \"Say hello.\"\nkind = \"script\"\n";
        std::fs::write(d.join("run.sh"), "#!/bin/bash\necho hi\n").unwrap();
        assert!(
            parse(&d, "hello", text)
                .unwrap_err()
                .contains("not executable")
        );
        crate::desktop::fdo::launch::tests::script(&d.join("run.sh"), "echo hi");
        let r = parse(&d, "hello", text).unwrap();
        assert_eq!(
            r.body,
            Body::Script {
                path: d.join("run.sh")
            }
        );
        let r = parse(
            &d,
            "hello",
            "description = \"Say hello.\"\nkind = \"script\"\nasync = true\nlevel = \"owner\"\n\
             policy = \"trusted\"\nopen_world = true\n",
        )
        .unwrap();
        assert_eq!(r.spec.class, Class::Async);
        assert_eq!(r.spec.level, Level::Owner);
        // policy = "trusted" is asked, not honoured: confirmed for every caller until granted.
        assert_eq!(r.spec.ask, Ask::Always);
        assert_eq!(
            r.asks,
            Asks {
                allow_trusted: true,
                level: None
            }
        );
        assert!(r.spec.hints.open_world);
    }

    /// Module doc: `routine.toml` gets at most confirm and the owner's level; what it asks beyond is
    /// [`Asks`], with the owner's entries that would grant it.
    #[test]
    fn a_routine_gets_at_most_confirm_and_the_owners_level() {
        let goal =
            "description = \"x.\"\nkind = \"goal\"\ngoal = \"g\"\ntools = [\"window_list\"]\n";
        let r = |extra: &str| parse(&dir(), "capped", &format!("{goal}{extra}")).unwrap();
        // read_only: still read-only to the callers, confirmed for every caller here.
        let ro = r("read_only = true\nlevel = \"trusted\"\n");
        assert!(ro.spec.hints.read_only);
        assert_eq!((ro.spec.ask, ro.spec.level), (Ask::Always, Level::Owner));
        assert_eq!(
            ro.asks.grants("capped"),
            [
                (
                    "policy.tools.capped".to_owned(),
                    "\"allow_trusted\"".to_owned()
                ),
                ("policy.levels.capped".to_owned(), "\"trusted\"".to_owned()),
            ]
        );
        assert!(
            contract::check(ro.spec).is_empty(),
            "read-only and Always passes the rules"
        );
        // What is not wider is honoured as written, and asks nothing.
        for (extra, ask, level) in [
            ("level = \"owner\"\n", Ask::Hints, Level::Owner),
            (
                "level = \"own\"\npolicy = \"always\"\n",
                Ask::Always,
                Level::Own,
            ),
            (
                "read_only = true\npolicy = \"always\"\nlevel = \"owner\"\n",
                Ask::Always,
                Level::Owner,
            ),
        ] {
            let x = r(extra);
            assert_eq!((x.spec.ask, x.spec.level), (ask, level), "{extra}");
            assert!(
                x.asks.is_empty() && x.asks.grants("capped").is_empty(),
                "{extra}"
            );
        }
        let any = r("level = \"any\"\n");
        assert_eq!(any.spec.level, Level::Owner);
        assert_eq!(any.asks.level, Some(LevelSetting::Any));
    }

    #[test]
    fn the_parameters_fill_the_steps() {
        let args = json!({
            "title": "{folder}", "body": "{folder} saved {loud} by {who}", "n": 3,
            "loud": "{loud}", "other": "{x}"
        });
        let declared = ["folder", "loud"];
        let params = json!({"folder": "Notes", "who": "Kai"});
        let filled = fill(
            args.as_object().unwrap(),
            params.as_object().unwrap(),
            &declared,
        );
        assert_eq!(
            Value::Object(filled),
            json!({"title": "Notes", "body": "Notes saved  by {who}", "n": 3, "other": "{x}"}),
            "an omitted parameter is empty in a text and left out as a whole value"
        );
        let params = json!({"folder": "Notes", "loud": true});
        let filled = fill(
            args.as_object().unwrap(),
            params.as_object().unwrap(),
            &declared,
        );
        assert_eq!(filled["loud"], json!(true), "the value itself, typed");
        assert_eq!(filled["body"], json!("Notes saved true by {who}"));
    }
}
