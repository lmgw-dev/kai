//! The settings catalog (the owner's ruling, 2026-10-19: no setting is shown as a raw text field
//! labelled with its configuration key): for every key of the configuration a human
//! label in sentence case, one plain-language help line, its section, its unit, its control and
//! whether it is advanced tuning. The configuration window shows these, never the key (at most as
//! a tooltip); the owner tools and the file keep the keys.
//!
//! The table is [`table`]; a key with no entry there fails `every_key_has_a_catalog_entry` in the
//! tests, so a new key cannot ship unlabelled. What a key takes is still the configuration's own
//! business: a choice's words are checked against its type's (`choices`), a number's bounds against
//! the loader's rules, and a write is accepted only if the file loads ([`super::set`]).
//!
//! A map's entries (`policy.tools.<tool>`, `weather.places.<name>`) are one pattern entry each,
//! `policy.tools.*`, whose label may name the entry (`{}` is its last segment).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Refusal;

mod table;
#[cfg(test)]
mod tests;

/// How a setting is edited, as the window gets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Control {
    /// On or off.
    Switch,
    /// One of fixed words, each with its human name; `unset` names the state with no value (an
    /// optional word: `desktop.capture`'s "Automatic").
    Choice {
        options: Vec<Opt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unset: Option<String>,
    },
    /// A whole number; a bound only where the configuration has one.
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// Any one line of text; `unset` says what an empty field means ("System default").
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unset: Option<String>,
    },
    /// A list of texts, one per line.
    List,
    /// Shown, not edited here (Kai's own flows write it).
    ReadOnly,
    /// Not shown (Kai's own bookkeeping, such as the folder it remembers).
    Hidden,
    /// One of what Kai can list: the window offers them by name, with `unset` for none picked
    /// ("System default"). The value is the thing's identifier, which the owner never types.
    Pick { from: Source, unset: String },
    /// A name that is either the one every client shares (`shared`) or this computer's own
    /// (`conversation.folder_name`: the Kai conversation, `own` the suggested own name).
    SharedName { shared: String, own: String },
}

/// What a [`Control::Pick`] lists (`GET /api/devices`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Microphones,
    Speakers,
    Screens,
}

/// One word of a [`Control::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opt {
    pub word: String,
    pub name: String,
}

/// What the window shows of a setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// The section's id (`voice`) and its heading (`Voice`).
    pub section: String,
    pub section_label: String,
    /// A heading inside the section ("Text overlay"), for keys that read together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub label: String,
    pub help: String,
    /// "s", "ms", "MB", "px", "dBFS": after the control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub control: Control,
    /// Rare tuning: under the section's "Advanced" heading.
    pub advanced: bool,
    /// The catalog's order, which the window keeps (sections and keys).
    pub order: u32,
    /// The value the setting has when the file does not set it (`null` for an optional one).
    pub default: Value,
}

/// The catalog's entry for `key`, built for the window; `None` for a key the catalog lacks (the
/// tests keep that from happening for every key the configuration has).
pub fn meta(key: &str) -> Option<Meta> {
    let (order, section, spec) = table::find(key)?;
    let tail = key.rsplit('.').next().unwrap_or(key);
    let fill = |s: &str| s.replace("{}", tail);
    Some(Meta {
        section: section.id.to_owned(),
        section_label: section.label.to_owned(),
        group: spec.group.map(fill),
        label: fill(spec.label),
        help: fill(spec.help),
        unit: spec.unit.map(str::to_owned),
        control: spec.control.wire(),
        advanced: spec.advanced,
        order,
        default: default_of(key),
    })
}

/// The suggested name of a conversation of this computer's own: "Kai " and the machine's pretty
/// hostname (`/etc/machine-info`), else its hostname; "Kai desktop" when neither can be read.
pub fn own_conversation_name() -> String {
    let host = crate::pairing::device_name();
    host.map_or_else(|| "Kai desktop".to_owned(), |h| format!("Kai {h}"))
}

/// The value `key` has in a configuration with nothing set.
fn default_of(key: &str) -> Value {
    static DEFAULTS: std::sync::OnceLock<Vec<(String, Value)>> = std::sync::OnceLock::new();
    DEFAULTS
        .get_or_init(|| super::tree::leaves(&crate::config::Config::default()))
        .iter()
        .find(|(k, _)| k == key)
        .map_or(Value::Null, |(_, v)| v.clone())
}

/// The label for `key` (the key itself for one the catalog lacks).
pub fn label(key: &str) -> String {
    meta(key).map_or_else(|| key.to_owned(), |m| m.label)
}

/// What the loader found wrong with a value, from `ConfigError::Invalid`'s own fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub key: String,
    pub value: String,
    pub rule: String,
}

/// A refusal in the window's words: a value the loader refused is "Label: rule; you entered
/// value", from the error's own key, value and rule; a file-level error (the file does not parse,
/// cannot be read or written) keeps its path, which is the fact the owner needs; any other key a
/// text mentions is named by its label too. The owner tools keep the keys (a model writes them).
pub fn in_words(refusal: &Refusal) -> String {
    match &refusal.1 {
        Some(i) => named(&format!(
            "{}: {}; you entered {}",
            label(&i.key),
            i.rule,
            i.value
        )),
        None => named(&refusal.0),
    }
}

/// Every word of `text` that is a catalog key, by its label in quotes.
fn named(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let key = word.trim_end_matches('.');
        if key.contains('.') && meta(key).is_some() {
            out.push('“');
            out.push_str(&label(key));
            out.push('”');
            out.push_str(&word[key.len()..]);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// [`meta`], or for a key the catalog lacks (none the configuration has: the tests) a plain entry
/// read off the key's last segment, under "Other", shown and not edited.
pub fn meta_or_plain(key: &str) -> Meta {
    meta(key).unwrap_or_else(|| {
        let tail = key.rsplit('.').next().unwrap_or(key).replace('_', " ");
        let mut label = tail.clone();
        if let Some(first) = tail.chars().next() {
            label = first.to_uppercase().collect::<String>() + &tail[first.len_utf8()..];
        }
        Meta {
            section: "other".to_owned(),
            section_label: "Other".to_owned(),
            group: None,
            label,
            help: String::new(),
            unit: None,
            control: Control::ReadOnly,
            advanced: true,
            order: u32::MAX,
            default: default_of(key),
        }
    })
}
