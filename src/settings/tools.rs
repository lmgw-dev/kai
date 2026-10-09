//! K30's tools over the operations: a call's arguments become a [`Request`], the answer becomes
//! compact lines (K22: no pretty JSON, no echo of the input beyond the key).

use super::tree::show;
use super::{Effect, Entry, Request, Response};
use crate::host::registry::Args;

/// The request of the call to `tool` (a tool of `contract::config`); the error is for the caller.
pub fn request(tool: &str, args: &Args) -> Result<Request, String> {
    let text = |name: &str| {
        args.text(name)
            .map(str::to_owned)
            .ok_or_else(|| format!("{tool} needs {name}"))
    };
    // "default" (or an empty text) is the unset of the per-tool settings.
    let word = |name: &str| -> Result<Option<String>, String> {
        Ok(Some(text(name)?.trim().to_lowercase())
            .filter(|w| !matches!(w.as_str(), "" | "default")))
    };
    match tool {
        "config_list" => Ok(Request::List {
            section: args.text("section").map(str::to_owned),
        }),
        "config_get" => Ok(Request::Get { key: text("key")? }),
        "config_set" => Ok(Request::Set {
            key: text("key")?,
            value: text("value")?,
        }),
        "config_unset" => Ok(Request::Unset { key: text("key")? }),
        "tool_state_set" => Ok(Request::ToolState {
            tool: text("tool")?,
            state: enum_word(
                "state",
                word("state")?,
                "allow, confirm, deny, off or default",
            )?,
        }),
        "tool_level_set" => Ok(Request::ToolLevel {
            tool: text("tool")?,
            level: enum_word(
                "level",
                word("level")?,
                "own, owner, trusted, any or default",
            )?,
        }),
        "provider_set" => Ok(Request::Provider {
            capability: text("capability")?,
            provider: enum_word(
                "provider",
                word("provider")?,
                "plasma, fdo, none or default",
            )?,
        }),
        other => Err(format!("Kai's config area has no tool {other}")),
    }
}

/// A lowercase word read as the enum `T`, or the sentence of what is accepted.
fn enum_word<T: serde::de::DeserializeOwned>(
    name: &str,
    word: Option<String>,
    accepted: &str,
) -> Result<Option<T>, String> {
    match word {
        None => Ok(None),
        Some(w) => serde_json::from_value(serde_json::Value::String(w.clone()))
            .map(Some)
            .map_err(|_| format!("{name} {w:?} is none of {accepted}")),
    }
}

fn line(e: &Entry) -> String {
    let default = if e.set || e.value.is_null() {
        ""
    } else {
        " (default)"
    };
    if e.value.is_null() {
        format!("{} {}", e.key, show(&e.value))
    } else {
        format!("{} = {}{default}", e.key, show(&e.value))
    }
}

/// The answer, as lines.
pub fn render(r: &Response) -> String {
    match r {
        Response::Entries { entries } => entries.iter().map(line).collect::<Vec<_>>().join("\n"),
        Response::Entry { entry } => line(entry),
        Response::Changed { entry, effect } => {
            let when = match effect {
                Effect::Now => "applied now",
                Effect::Restart => "applies at Kai's next start",
            };
            if entry.set {
                format!("{} ({when})", line(entry))
            } else {
                format!(
                    "{} = {} unset, back to the default ({when})",
                    entry.key,
                    show(&entry.value)
                )
            }
        }
        Response::Unchanged { entry } => format!("{} was not set; nothing changed", entry.key),
    }
}
