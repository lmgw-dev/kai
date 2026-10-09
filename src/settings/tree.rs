//! The configuration as a flat list of keys: the effective values are [`Config`]'s serialization,
//! which keys the file sets are read off the file itself.

use std::path::Path;

use serde_json::Value;

use super::{Entry, Refusal, refuse, secret_key};
use crate::config::{self, Config};

/// Every leaf of the configuration: `(key, value)`, sorted by key. A table is walked into (an
/// empty one, such as `policy.tools` with no entry, has no leaf); an array and a null are leaves.
pub(super) fn leaves(config: &Config) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let tree = serde_json::to_value(config).expect("the configuration serializes");
    walk("", &tree, &mut out);
    out
}

fn walk(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                walk(&key, child, out);
            }
        }
        leaf => out.push((prefix.to_owned(), leaf.clone())),
    }
}

/// The file read once: the configuration it loads as, and its own keys.
pub(super) struct View {
    leaves: Vec<(String, Value)>,
    file: toml_edit::DocumentMut,
}

impl View {
    pub(super) fn read(path: &Path) -> Result<Self, Refusal> {
        let text = config::read_text(path)?;
        let config = Config::parse(&text, path)?;
        let file = text
            .parse()
            .map_err(|e: toml_edit::TomlError| Refusal(format!("{}: {e}", path.display())))?;
        Ok(Self {
            leaves: leaves(&config),
            file,
        })
    }

    /// The file has `key` (a value, not a table).
    pub(super) fn in_file(&self, key: &str) -> bool {
        let mut item = Some(self.file.as_item());
        for name in key.split('.') {
            item = item.and_then(|i| i.get(name));
        }
        item.is_some_and(|i| i.is_value())
    }

    fn entry(&self, key: &str, value: Value) -> Entry {
        Entry {
            key: key.to_owned(),
            choices: super::choices::of(key, &value),
            value,
            set: self.in_file(key),
        }
    }

    pub(super) fn get(&self, key: &str) -> Result<Entry, Refusal> {
        if secret_key(key) {
            return refuse(format!(
                "{key}: keys, tokens and passwords are not part of the configuration and are never \
                 read here"
            ));
        }
        if let Some((_, v)) = self.leaves.iter().find(|(k, _)| k == key) {
            return Ok(self.entry(key, v.clone()));
        }
        let under = format!("{key}.");
        if self.leaves.iter().any(|(k, _)| k.starts_with(&under)) {
            return refuse(format!(
                "{key} is a section; config_list {key} shows its settings"
            ));
        }
        // A map entry that is not set (`policy.tools.type_text`) has no leaf.
        if let Some((table, _)) = key.rsplit_once('.')
            && super::tool_keyed(key)
            && table.matches('.').count() == 1
        {
            return Ok(self.entry(key, Value::Null));
        }
        let sections: Vec<&str> = {
            let mut s: Vec<&str> = self
                .leaves
                .iter()
                .filter_map(|(k, _)| k.split('.').next())
                .collect();
            s.dedup();
            s
        };
        refuse(format!(
            "no setting {key} (the sections are {}; config_list <section> shows a section's keys)",
            sections.join(", ")
        ))
    }

    pub(super) fn list(&self, section: Option<&str>) -> Result<Vec<Entry>, Refusal> {
        let under = section.map(|s| format!("{}.", s.trim_end_matches('.')));
        let entries: Vec<Entry> = self
            .leaves
            .iter()
            .filter(|(k, _)| !secret_key(k))
            .filter(|(k, _)| under.as_ref().is_none_or(|u| k.starts_with(u.as_str())))
            .map(|(k, v)| self.entry(k, v.clone()))
            .collect();
        if entries.is_empty() && section.is_some() {
            return refuse(format!(
                "no section {}: config_list without a section shows every setting",
                section.unwrap_or_default()
            ));
        }
        Ok(entries)
    }
}

/// A value as one compact piece of text, TOML's notation.
pub fn show(v: &Value) -> String {
    match v {
        Value::Null => "(not set)".into(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(show).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}
