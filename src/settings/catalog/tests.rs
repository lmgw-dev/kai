use std::path::Path;

use serde_json::Value;

use super::table::{self, Kind};
use super::*;
use crate::config::{self, Config};

/// A configuration with an entry in every map, so their keys are leaves too.
const MAPS: &str = "\
[weather]
places = { home = \"53.5511,9.9937\" }

[policy.tools]
key_press = \"allow\"

[policy.levels]
key_press = \"owner\"

[policy.widen_by_tools]
key_press = true

[routines.enabled]
backup = false
";

fn leaves() -> Vec<(String, Value)> {
    let config = Config::parse(MAPS, Path::new("kai.toml")).unwrap();
    super::super::tree::leaves(&config)
}

/// `key = value` alone in a file, loaded.
fn load_with(key: &str, value: toml_edit::Item) -> Result<Config, config::ConfigError> {
    let (table, leaf) = key.rsplit_once('.').unwrap();
    let path = Path::new("kai.toml");
    let text = config::edit_text("", path, table, leaf, Some(value)).unwrap();
    Config::parse(&text, path)
}

/// The owner's rule (2026-10-19): no setting reaches the window without its words.
#[test]
fn every_key_has_a_catalog_entry() {
    let missing: Vec<String> = leaves()
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| meta(k).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "keys without a catalog entry (src/settings/catalog/table.rs): {missing:?}"
    );
}

#[test]
fn every_catalog_entry_names_a_key_the_configuration_has() {
    let leaves = leaves();
    for (_, _, spec) in table::all() {
        let found = match spec.key.strip_suffix('*') {
            Some(prefix) => leaves.iter().any(|(k, _)| k.starts_with(prefix)),
            None => leaves.iter().any(|(k, _)| k == spec.key),
        };
        assert!(
            found,
            "{} is in the catalog but not in the configuration",
            spec.key
        );
    }
}

#[test]
fn the_controls_fit_the_values() {
    for (key, value) in leaves() {
        let m = meta(&key).unwrap();
        let fits = match &m.control {
            Control::Switch => value.is_boolean(),
            Control::Number { .. } => value.is_number(),
            Control::Choice { unset, .. } => value.is_string() || unset.is_some(),
            Control::Text { unset } => value.is_string() || (value.is_null() && unset.is_some()),
            Control::List => value.is_array() || value.is_null(),
            Control::SharedName { .. } => value.is_string(),
            Control::Pick { .. } => value.is_string() || value.is_null(),
            Control::ReadOnly | Control::Hidden => true,
        };
        assert!(fits, "{key} = {value} does not fit {:?}", m.control);
    }
}

/// A choice's words are exactly its type's (serde's, read by `choices`), in its order, and every
/// fixed-word key is a choice.
#[test]
fn choices_are_their_types_words() {
    for (key, _) in leaves() {
        let words = super::super::choices::of(&key, &Value::Null);
        match meta(&key).unwrap().control {
            Control::Choice { options, .. } => {
                let ours: Vec<String> = options.into_iter().map(|o| o.word).collect();
                assert_eq!(ours, words, "{key}");
            }
            _ => assert!(
                words.is_empty(),
                "{key} takes the words {words:?}: a choice"
            ),
        }
    }
}

/// A number's bounds are the loader's: the bound loads, one past it does not.
#[test]
fn number_bounds_are_the_loaders() {
    for (key, _) in leaves() {
        let Control::Number { min, max } = meta(&key).unwrap().control else {
            continue;
        };
        if let Some(min) = min {
            assert!(
                load_with(&key, toml_edit::value(min)).is_ok(),
                "{key} = {min}"
            );
            assert!(
                load_with(&key, toml_edit::value(min - 1)).is_err(),
                "{key} = {}",
                min - 1
            );
        }
        if let Some(max) = max {
            assert!(
                load_with(&key, toml_edit::value(max)).is_ok(),
                "{key} = {max}"
            );
            assert!(
                load_with(&key, toml_edit::value(max + 1)).is_err(),
                "{key} = {}",
                max + 1
            );
        }
    }
    let Control::Number { min, .. } = meta("overlay.show_timeout_s").unwrap().control else {
        panic!("a number");
    };
    assert_eq!(
        min,
        Some(crate::widgets::settings::least_show_timeout_s() as i64)
    );
}

#[test]
fn labels_and_help_are_words_not_keys() {
    let keys: Vec<String> = leaves().into_iter().map(|(k, _)| k).collect();
    for (_, section, spec) in table::all() {
        let first = spec.label.chars().next().unwrap();
        assert!(
            first.is_uppercase() || spec.label == "{}",
            "{}: {:?} is not sentence case",
            spec.key,
            spec.label
        );
        assert!(!spec.label.contains('_'), "{}: {:?}", spec.key, spec.label);
        assert!(spec.help.ends_with('.'), "{}: help is a sentence", spec.key);
        assert!(!spec.help.contains('_'), "{}: {:?}", spec.key, spec.help);
        for key in &keys {
            assert!(
                !spec.help.contains(key.as_str()),
                "{}: help names {key}",
                spec.key
            );
        }
        assert!(section.label.chars().next().unwrap().is_uppercase());
        if let Kind::Choice { options, .. } = spec.control {
            assert!(options.iter().all(|(_, name)| !name.contains('_')));
        }
    }
    let mut ids: Vec<&str> = table::SECTIONS.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), table::SECTIONS.len(), "section ids are unique");
}

#[test]
fn a_maps_entry_is_named_by_its_pattern() {
    let m = meta("policy.tools.run_command").unwrap();
    assert_eq!(m.label, "When run_command is called");
    assert_eq!(m.section, "tools");
    assert_eq!(meta("weather.places.work").unwrap().label, "work");
    assert!(meta("policy.tools").is_none());
    assert!(meta("no.such").is_none());
    assert_eq!(meta_or_plain("no.such_key").label, "Such key");
}

#[test]
fn the_own_conversation_is_named_after_this_computer() {
    let name = own_conversation_name();
    assert!(name.starts_with("Kai ") && name.trim() != "Kai", "{name}");
    assert_ne!(name, Config::default().conversation.folder_name);
}

#[test]
fn the_conversation_is_a_shared_or_own_name() {
    let m = meta("conversation.folder_name").unwrap();
    assert_eq!(m.label, "Conversation");
    assert!(!m.advanced);
    assert_eq!(
        m.control,
        Control::SharedName {
            shared: Config::default().conversation.folder_name,
            own: own_conversation_name(),
        }
    );
}

#[test]
fn a_refusal_names_the_setting_by_its_label() {
    let file = Path::new("/home/u/.config/kai/config.toml");
    let invalid = config::ConfigError::Invalid {
        path: file.to_owned(),
        key: "voice.send_queue_s",
        value: "5".into(),
        rule: "must be at least 15 (seconds: voice.connect_timeout_s, whose buffered audio goes \
               into it)"
            .into(),
    };
    assert_eq!(
        in_words(&Refusal::from(invalid)),
        "Outgoing audio buffer: must be at least 15 (seconds: “Connect timeout”, whose buffered \
         audio goes into it); you entered 5"
    );
    // A file-level error keeps the path: it is the fact the owner needs.
    let parse = config::ConfigError::Parse {
        path: file.to_owned(),
        message: "expected `=` at line 3".into(),
    };
    assert_eq!(
        in_words(&Refusal::from(parse)),
        "/home/u/.config/kai/config.toml: expected `=` at line 3"
    );
    assert_eq!(
        in_words(&Refusal::plain("x: not a setting")),
        "x: not a setting"
    );
    assert_eq!(
        in_words(&Refusal::plain("see ring.size.")),
        "see “Size”.",
        "a sentence's full stop is not the key's"
    );
}

/// A switch's help says what the setting does, not "On, …" / "Off, …" (the checkbox shows the
/// state), and every default is the configuration's own.
#[test]
fn help_lines_do_not_restate_the_state() {
    for (_, _, spec) in table::all() {
        assert!(
            !spec.help.starts_with("On,") && !spec.help.starts_with("Off,"),
            "{}: {}",
            spec.key,
            spec.help
        );
    }
    assert_eq!(
        meta("voice.grace_s").unwrap().default,
        serde_json::json!(30)
    );
    assert_eq!(meta("audio.input").unwrap().default, Value::Null);
    assert_eq!(
        meta("conversation.folder_name").unwrap().default,
        serde_json::json!("Kai")
    );
}
