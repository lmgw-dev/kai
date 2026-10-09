//! The configuration operations (WP21a, ruling 26/27, K29, K30): every setting of
//! `$XDG_CONFIG_HOME/kai/config.toml` as an operation of Kai's core. The owner tools
//! (`kai_desktop::contract::config`, [`tools`]) and the configuration window (WP21b, through the
//! scheme handler) are two clients of [`apply`]; the operations know neither.
//!
//! **Transport-neutral:** a [`Request`] in, a [`Refusal`] or an [`Outcome`] out, all serde. Nothing
//! here touches the core, the host or a thread: `apply` reads and writes the file it is given.
//!
//! **Generic over the sections.** A key is `section.key` (or `policy.tools.<tool>`). What exists,
//! its default and its type come from [`crate::config::Config`] itself: the effective values are
//! its serialization ([`tree`]) and a write is accepted only if the whole file, as it would then
//! be, loads again ([`Config::parse`]: the loader's rules and the loader's message). So a section
//! another work package adds needs nothing here, except that its structs derive `Serialize` (the
//! compiler says so).
//!
//! **The write** is §2's: [`crate::config::edit_text`] changes only the value (the file's comments
//! and spacing stay) and [`crate::config::write_atomically`] puts it in place atomically and
//! durably. Nothing is written if the result does not load.
//!
//! **Secrets** (keys, tokens, passwords) are not in the configuration: the device key lives in the
//! Secret Service. A key whose name says it would be one is neither read, listed nor written
//! ([`secret_key`]); `a_default_configuration_holds_no_secret` is the tripwire for a future one.
//!
//! **Effects.** `policy.*` takes effect at once ([`Effect::Now`]: the core builds the policy again
//! and the host lists the tools again, with `list_changed` when they changed), and so does
//! `delegate.screenshots` (the host lists again, `crate::delegate::screenshots`). Every other key is
//! read when Kai starts ([`Effect::Restart`]); the tray's live switches keep their own path.
//!
//! **The routines (WP20):** their switches are `routines.enabled.<name>`, written only by the
//! [`Request::Routine`] arm, and on as `true` (the tray and `routine_enable` call it
//! after showing the routine in full; the latest version of the folder always runs, decided by
//! the owner 2026-10-09): `Set` refuses those keys except with `false` (off, the name kept: what
//! the routines write for a switch whose folder is gone), `Unset` (switching one off) is allowed. The window
//! does not show routines yet, so its route refuses switching one on (`window::route`); the tray
//! and `routine_enable` are the way. `known_tool` answers for a routine's name too, so
//! `policy.tools.<routine>` and `policy.levels.<routine>` can be set.

use std::path::Path;

use kai_desktop::caps::Capability;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{self, Config, ConfigError};
use crate::desktop::settings::Choice;
use crate::host::settings::{LevelSetting, ToolState};

mod choices;
pub mod tools;
mod tree;
mod widen;
pub mod window;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod widen_tests;

/// One operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Every setting (or a section's) with its effective value and whether the file sets it.
    List {
        section: Option<String>,
    },
    Get {
        key: String,
    },
    /// `value` is TOML text: `30`, `true`, `"text"`, `["a", "b"]`. Text that is no TOML value is a
    /// string, and so is text a string-typed key needs (`5` for a key that holds text).
    Set {
        key: String,
        value: String,
    },
    /// Removes the key from the file, so its default applies again.
    Unset {
        key: String,
    },
    /// `policy.tools.<tool>`; `None`: K15's default for the tool.
    ToolState {
        tool: String,
        state: Option<ToolState>,
    },
    /// `policy.levels.<tool>`; `None`: the contract's level.
    ToolLevel {
        tool: String,
        level: Option<LevelSetting>,
    },
    /// `desktop.<capability>`; `None`: the probe's choice.
    Provider {
        capability: String,
        provider: Option<Choice>,
    },
    /// `routines.enabled.<name>` (K28): on (`true`; the caller showed the routine to the owner
    /// first) or off (the key removed).
    Routine {
        name: String,
        enabled: bool,
    },
}

/// One setting as the owner sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    /// The effective value; `null` for an optional setting that is not set (`audio.input`).
    pub value: Value,
    /// The file has the key; otherwise the value is the default.
    pub set: bool,
    /// The words a fixed-word setting takes (an enum's, `overlay.weather.position`'s nine anchors),
    /// read off its type ([`choices`]); empty for every other setting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// When a change takes hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Now,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Entries {
        entries: Vec<Entry>,
    },
    Entry {
        entry: Entry,
    },
    /// The file was written; `entry` is the key as it now reads.
    Changed {
        entry: Entry,
        effect: Effect,
    },
    /// An unset of a key the file does not have.
    Unchanged {
        entry: Entry,
    },
}

/// What [`apply`] hands back: the answer, and the configuration as the file now reads when the
/// operation wrote it (the core takes it as its own).
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub response: Response,
    pub config: Option<Config>,
}

/// Why an operation did not run. The text is for the owner (or the model): the loader's message
/// for an invalid value, otherwise a sentence naming the key.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{0}")]
pub struct Refusal(pub String);

impl From<ConfigError> for Refusal {
    fn from(e: ConfigError) -> Self {
        Refusal(e.to_string())
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(why.into()))
}

/// Keys Kai's own flows write; setting them by hand would unpair or misdirect it.
const PAIRING_KEYS: &[&str] = &["lmgw.url"];

/// Words in a key's name that make it a secret.
const SECRET_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passphrase",
    "bearer",
    "credential",
    "credentials",
    "apikey",
    "passwd",
    "pwd",
    "auth",
    "authorization",
    "secrets",
    "passwords",
];

/// The key's tail names a tool, whose name is not the setting's (`policy.tools.key_press`).
fn tool_keyed(key: &str) -> bool {
    key.starts_with("policy.tools.")
        || key.starts_with("policy.levels.")
        || key.starts_with("policy.widen_by_tools.")
}

/// The key's name says it is a secret (module doc): any of [`SECRET_WORDS`] as a word of a segment,
/// whatever the case (`API_KEY`, `apiKey`, `api-key`, `apikey`).
pub fn secret_key(key: &str) -> bool {
    !tool_keyed(key)
        && key
            .split('.')
            .flat_map(words)
            .any(|word| SECRET_WORDS.contains(&word.as_str()))
}

/// A segment's words, lowercase: split at `_`, `-` and a lower-to-upper step (`apiKey`).
fn words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut prev_lower = false;
    for c in segment.chars() {
        let boundary = c == '_' || c == '-' || (c.is_uppercase() && prev_lower);
        if boundary && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        if c != '_' && c != '-' {
            word.extend(c.to_lowercase());
        }
        prev_lower = c.is_lowercase();
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A tool Kai offers, or a routine's (whether it is on or not).
fn known_tool(name: &str) -> bool {
    kai_desktop::contract::find(name).is_some() || crate::routines::known(name)
}

/// A routine's switch (`routines.enabled.<name>`), which only [`Request::Routine`] writes.
fn routine_switch(key: &str) -> bool {
    key.strip_prefix(crate::routines::settings::ENABLED_TABLE)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Who asks. The owner's own UI (the settings window, the tray) may widen what it likes; the
/// owner configuration tools may widen a permission only where the owner permitted it
/// ([`widen`], K30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Owner,
    Tool,
}

/// Runs `request` against the configuration file `file`, as the owner's own UI.
pub fn apply(file: &Path, request: Request) -> Result<Outcome, Refusal> {
    apply_from(file, request, Source::Owner)
}

/// Runs `request` for `source`.
pub fn apply_from(file: &Path, request: Request, source: Source) -> Result<Outcome, Refusal> {
    match request {
        Request::List { section } => {
            let view = tree::View::read(file)?;
            let entries = view.list(section.as_deref())?;
            Ok(read(Response::Entries { entries }))
        }
        Request::Get { key } => {
            let view = tree::View::read(file)?;
            Ok(read(Response::Entry {
                entry: view.get(&key)?,
            }))
        }
        // `false` is off with the name kept (a switch whose folder is gone, `Inner::rescan`).
        Request::Set { key, value } if routine_switch(&key) && value.trim() != "false" => {
            refuse(format!(
                "{key} switches a routine on only after the owner saw it: use routine_enable or \
                 the tray's Routines menu"
            ))
        }
        Request::Set { key, value } => set(file, &key, &value, source),
        Request::Unset { key } => unset(file, &key, source),
        Request::ToolState { tool, state } => {
            if !known_tool(&tool) {
                return refuse(format!("Kai offers no tool {tool}"));
            }
            let key = format!("policy.tools.{tool}");
            match state {
                Some(s) => set_word(file, &key, &s, source),
                None => unset(file, &key, source),
            }
        }
        Request::ToolLevel { tool, level } => {
            if !known_tool(&tool) {
                return refuse(format!("Kai offers no tool {tool}"));
            }
            let key = format!("policy.levels.{tool}");
            match level {
                Some(l) => set_word(file, &key, &l, source),
                None => unset(file, &key, source),
            }
        }
        Request::Routine { name, enabled } => {
            if !crate::routines::def::plain_name(&name) {
                return refuse(format!("{name:?} is no routine's name"));
            }
            let key = format!("{}.{name}", crate::routines::settings::ENABLED_TABLE);
            if !enabled {
                return unset(file, &key, source);
            }
            set(file, &key, "true", source)
        }
        Request::Provider {
            capability,
            provider,
        } => {
            let Some(cap) = Capability::ALL.into_iter().find(|c| c.key() == capability) else {
                let all: Vec<&str> = Capability::ALL.iter().map(|c| c.key()).collect();
                return refuse(format!(
                    "no capability {capability} (they are {})",
                    all.join(", ")
                ));
            };
            let key = format!("desktop.{}", cap.key());
            match provider {
                Some(p) => set_word(file, &key, &p, source),
                None => unset(file, &key, source),
            }
        }
    }
}

fn read(response: Response) -> Outcome {
    Outcome {
        response,
        config: None,
    }
}

/// An enum setting by its serialized word (`allow`, `owner`, `plasma`).
fn set_word(
    file: &Path,
    key: &str,
    word: &impl Serialize,
    source: Source,
) -> Result<Outcome, Refusal> {
    let Value::String(word) = serde_json::to_value(word).map_err(|e| Refusal(e.to_string()))?
    else {
        return refuse(format!("{key}: not a word"));
    };
    set(file, key, &format!("\"{word}\""), source)
}

/// `section.key` split at its last dot, checked: plain keys, at least one section, writable.
fn writable(key: &str) -> Result<(&str, &str), Refusal> {
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    let Some((table, leaf)) = key.rsplit_once('.') else {
        return refuse(format!(
            "{key:?} is no setting: a key is section.key (config_list shows them)"
        ));
    };
    if !plain(leaf) || !table.split('.').all(plain) {
        return refuse(format!(
            "{key:?} is no setting: letters, digits, _ and - in dotted names only"
        ));
    }
    if secret_key(key) {
        return refuse(format!(
            "{key}: keys, tokens and passwords are not part of the configuration; Kai keeps its \
             device key in the Secret Service and never reads or writes it here"
        ));
    }
    if PAIRING_KEYS.contains(&key) {
        return refuse(format!(
            "{key} is written by pairing (it selects which Secret Service item holds the key); pair \
             Kai with the other gateway instead"
        ));
    }
    Ok((table, leaf))
}

/// The value text as a TOML value: `Err` names why it is not one.
fn toml_value(text: &str) -> Result<toml_edit::Item, String> {
    let doc: toml_edit::DocumentMut = format!("v = {}", text.trim())
        .parse()
        .map_err(|_| "not a TOML value".to_owned())?;
    if doc.len() != 1 {
        return Err("more than one value".into());
    }
    match doc.get("v") {
        Some(item @ toml_edit::Item::Value(v)) if !v.is_inline_table() => Ok(item.clone()),
        Some(_) => Err("a table, not a value; set its keys one by one".into()),
        None => Err("not a TOML value".into()),
    }
}

fn set(file: &Path, key: &str, text: &str, source: Source) -> Result<Outcome, Refusal> {
    let (table, leaf) = writable(key)?;
    // The text read as TOML first (`30`, `true`, `["a"]`), then as a plain string (`Kai`, and `5`
    // for a text key): the first reading the loader accepts is written. When none is, the first
    // reading's refusal is the one said.
    if text.contains(['\n', '\r']) {
        return refuse(format!("{key}: a value is one line"));
    }
    let mut readings = Vec::new();
    match toml_value(text) {
        Ok(item) => readings.push(item),
        Err(why) if text.trim().starts_with(['[', '{']) => {
            return refuse(format!("{key} = {text}: {why}"));
        }
        Err(_) => {}
    }
    readings.push(toml_edit::value(text));
    let mut first_error = None;
    for item in readings {
        match write(file, table, leaf, Some(item), source) {
            Ok(config) => return changed(file, key, config),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.expect("at least one reading was tried"))
}

fn unset(file: &Path, key: &str, source: Source) -> Result<Outcome, Refusal> {
    let (table, leaf) = writable(key)?;
    let view = tree::View::read(file)?;
    if !view.in_file(key) {
        return Ok(read(Response::Unchanged {
            entry: view.get(key)?,
        }));
    }
    let config = write(file, table, leaf, None, source)?;
    changed(file, key, config)
}

/// The edit, checked by loading the result, then written.
fn write(
    file: &Path,
    table: &str,
    leaf: &str,
    item: Option<toml_edit::Item>,
    source: Source,
) -> Result<Config, Refusal> {
    let text = config::read_text(file)?;
    let edited = config::edit_text(&text, file, table, leaf, item)?;
    let config = Config::parse(&edited, file)?;
    if source == Source::Tool {
        // A file that did not load before counts as the defaults: nothing was permitted in it.
        let before = Config::parse(&text, file).unwrap_or_default();
        widen::check(&before, &config, &widen::offered)?;
    }
    config::write_atomically(file, edited.as_bytes()).map_err(|e| ConfigError::Write {
        path: file.to_owned(),
        message: e.to_string(),
    })?;
    Ok(config)
}

fn changed(file: &Path, key: &str, config: Config) -> Result<Outcome, Refusal> {
    let entry = tree::View::read(file)?.get(key).unwrap_or(Entry {
        key: key.to_owned(),
        value: Value::Null,
        set: false,
        choices: Vec::new(),
    });
    let effect = if key.starts_with("policy.")
        || routine_switch(key)
        || key == crate::routines::settings::RUN_TIMEOUT_KEY
        || key == crate::delegate::settings::SCREENSHOTS_KEY
    {
        Effect::Now
    } else {
        Effect::Restart
    };
    Ok(Outcome {
        response: Response::Changed { entry, effect },
        config: Some(config),
    })
}
