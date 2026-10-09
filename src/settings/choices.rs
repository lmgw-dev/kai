//! The words a fixed-word setting takes (`overlay.weather.position`: the nine anchors), read off the
//! type the key deserializes into, so nothing here lists them a second time: the key is given a word
//! no enum has, alone in an otherwise empty file, and the deserializer's refusal of it names the
//! words the type accepts (serde's `unknown variant` message). A key whose type takes any text, a
//! number, a flag or a list refuses differently or not at all, and has no choices.
//!
//! Generic like the rest of [`super`]: an enum-typed key a later work package adds gets its choices
//! with nothing written here (`every_enum_key_has_its_words` in the tests is the tripwire for the
//! message's shape).

use std::path::Path;

use serde_json::Value;

use crate::config::{self, Config};

/// A word no setting's enum has.
const PROBE: &str = "kai-no-such-word";

/// The words `key` takes, in the type's order; empty for a key that is not a fixed-word setting.
/// Only a text or an unset value can be one (an enum serializes to its word, an unset optional one
/// to `null`).
pub(super) fn of(key: &str, value: &Value) -> Vec<String> {
    if !matches!(value, Value::String(_) | Value::Null) {
        return Vec::new();
    }
    let Some((table, leaf)) = key.rsplit_once('.') else {
        return Vec::new();
    };
    let item = Some(toml_edit::value(PROBE));
    let Ok(text) = config::edit_text("", Path::new("choices"), table, leaf, item) else {
        return Vec::new();
    };
    match toml::from_str::<Config>(&text) {
        Ok(_) => Vec::new(),
        Err(e) => words(e.message()),
    }
}

/// serde's refusal of an unknown variant: "unknown variant `x`, expected `a`", "… expected `a` or
/// `b`", "… expected one of `a`, `b`, `c`". Any other message names no words.
fn words(message: &str) -> Vec<String> {
    let Some(rest) = message
        .trim()
        .strip_prefix(&format!("unknown variant `{PROBE}`, expected "))
    else {
        return Vec::new();
    };
    rest.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serdes_three_shapes_are_read() {
        let m = |tail: &str| format!("unknown variant `{PROBE}`, expected {tail}");
        assert_eq!(words(&m("`a`")), ["a"]);
        assert_eq!(words(&m("`main` or `delegate`")), ["main", "delegate"]);
        assert_eq!(words(&m("one of `x`, `y-z`, `w`")), ["x", "y-z", "w"]);
        assert!(words("invalid type: string \"x\", expected u64").is_empty());
        assert!(words("unknown field `x`, expected `a`").is_empty());
    }
}
