//! The page's logic apart from the DOM: which rows the tool table has, what the settings list
//! shows for a value, how the entries group and filter. Plain functions over [`wire`](crate::wire)'s
//! types, so `cargo test` in `ui/` covers them natively.

use serde_json::Value;

use crate::wire::{Effect, Entry, ToolInfo};

/// One row of the policy table: the contract's facts and the owner's two settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRow {
    pub info: ToolInfo,
    /// `policy.tools.<name>` in the file; `None`: the default applies.
    pub state: Option<String>,
    /// `policy.levels.<name>` in the file; `None`: the contract's level.
    pub level: Option<String>,
}

/// The word a set string entry holds (`policy.tools.key_press` = `"allow"`); `None` when the file
/// does not set it.
pub fn set_word(entries: &[Entry], key: &str) -> Option<String> {
    entries
        .iter()
        .find(|e| e.key == key && e.set)
        .and_then(|e| e.value.as_str().map(str::to_owned))
}

/// A boolean entry that is true (`policy.widen_by_tools.key_press`).
pub fn flag_on(entries: &[Entry], key: &str) -> bool {
    entries
        .iter()
        .any(|e| e.key == key && e.value == Value::Bool(true))
}

/// `policy.config_tools`: the configuration tools are offered (K30).
pub fn config_tools_on(entries: &[Entry]) -> bool {
    entries
        .iter()
        .find(|e| e.key == "policy.config_tools")
        .is_some_and(|e| e.value == Value::Bool(true))
}

/// The table's rows in the contract's order.
pub fn tool_rows(tools: &[ToolInfo], entries: &[Entry]) -> Vec<ToolRow> {
    tools
        .iter()
        .map(|info| ToolRow {
            state: set_word(entries, &format!("policy.tools.{}", info.name)),
            level: set_word(entries, &format!("policy.levels.{}", info.name)),
            info: info.clone(),
        })
        .collect()
}

/// A row matches when every word of `filter` is in its name, area or description.
pub fn row_matches(row: &ToolRow, filter: &str) -> bool {
    let hay = format!(
        "{} {} {}",
        row.info.name.replace('_', " "),
        row.info.area,
        row.info.description
    )
    .to_lowercase();
    let name = row.info.name.to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w) || name.contains(w))
}

/// The rows that match, grouped by area in the order the areas first appear.
pub fn by_area(rows: Vec<ToolRow>, filter: &str) -> Vec<(String, Vec<ToolRow>)> {
    let mut groups: Vec<(String, Vec<ToolRow>)> = Vec::new();
    for row in rows.into_iter().filter(|r| row_matches(r, filter)) {
        match groups.iter_mut().find(|(a, _)| *a == row.info.area) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((row.info.area.clone(), vec![row])),
        }
    }
    groups
}

/// The description up to its first sentence end, for a table cell.
pub fn first_sentence(text: &str) -> String {
    let text = text.trim();
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        if c == '.' && text[i + 1..].starts_with([' ', '\n']) && !prev.is_ascii_digit() {
            return text[..=i].to_owned();
        }
        prev = c;
    }
    text.to_owned()
}

/// What a value looks like in its field: a string as it is (a plain text is accepted as a string
/// by `set`), a number or flag as TOML writes it, a list as `["a", "b"]`, nothing for a setting
/// that is not set.
pub fn show_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(toml_item).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

/// An array's item as TOML: a string quoted, the rest as it is.
fn toml_item(v: &Value) -> String {
    match v {
        Value::String(s) => Value::String(s.clone()).to_string(),
        other => show_value(other),
    }
}

/// How a setting is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    Flag,
    /// A fixed-word setting: a select over the words Kai's core sent ([`Entry::choices`]).
    Choice,
    Text,
    /// Kai's own flows write it (pairing): shown, not edited.
    ReadOnly,
}

pub fn editor(entry: &Entry) -> Editor {
    if crate::wire::READ_ONLY_KEYS.contains(&entry.key.as_str()) {
        Editor::ReadOnly
    } else if !entry.choices.is_empty() {
        Editor::Choice
    } else if entry.value.is_boolean() {
        Editor::Flag
    } else {
        Editor::Text
    }
}

/// The entries that match `filter` (words in the key), grouped by their first segment, sorted. The
/// per-tool entries (`policy.tools.*`, `policy.levels.*`, `policy.widen_by_tools.*`) are the policy table's, and stay out of
/// the list while no filter names them, so the generic list is not a second table of the same.
pub fn settings_sections(entries: &[Entry], filter: &str) -> Vec<(String, Vec<Entry>)> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let mut groups: Vec<(String, Vec<Entry>)> = Vec::new();
    for e in entries {
        let per_tool = e.key.starts_with("policy.tools.")
            || e.key.starts_with("policy.levels.")
            || e.key.starts_with("policy.widen_by_tools.");
        if per_tool && words.is_empty() {
            continue;
        }
        let key = e.key.to_lowercase();
        if !words.iter().all(|w| key.contains(w.as_str())) {
            continue;
        }
        let section = e.key.split('.').next().unwrap_or_default().to_owned();
        match groups.iter_mut().find(|(s, _)| *s == section) {
            Some((_, list)) => list.push(e.clone()),
            None => groups.push((section, vec![e.clone()])),
        }
    }
    groups
}

/// A section's entries by the table they are in: the section's own keys first (`None`), then one
/// group per table under it (`Some("overlay.weather")`: `[overlay.weather]`'s keys), in key order,
/// so a kind's or a place's settings read together under their table's name.
pub fn by_table(section: &str, entries: Vec<Entry>) -> Vec<(Option<String>, Vec<Entry>)> {
    let mut groups: Vec<(Option<String>, Vec<Entry>)> = vec![(None, Vec::new())];
    for e in entries {
        let table = e.key.rsplit_once('.').map(|(t, _)| t).unwrap_or_default();
        let sub = (table != section).then(|| table.to_owned());
        match groups.iter_mut().find(|(t, _)| *t == sub) {
            Some((_, list)) => list.push(e),
            None => groups.push((sub, vec![e])),
        }
    }
    groups.retain(|(_, list)| !list.is_empty());
    groups
}

/// The providers' rows: `desktop.<capability>` in the list's order, with the owner's choice.
pub fn provider_rows(entries: &[Entry]) -> Vec<(String, Option<String>)> {
    entries
        .iter()
        .filter_map(|e| Some((e.key.strip_prefix("desktop.")?, e)))
        .map(|(cap, e)| (cap.to_owned(), e.value.as_str().map(str::to_owned)))
        .collect()
}

/// When a change takes hold, for the line beside the control.
pub fn effect_note(effect: Effect) -> &'static str {
    match effect {
        Effect::Now => "applied",
        Effect::Restart => "saved; applies when Kai restarts",
    }
}

/// The Desktop page's line on the remote-control permission (`None`: Kai has not answered yet).
pub fn remote_status(stored: Option<bool>) -> &'static str {
    match stored {
        None => "Asking Kai whether a remote-control permission is stored\u{2026}",
        Some(true) => {
            "A remote-control permission is stored: the portal lets Kai type without asking."
        }
        Some(false) => {
            "No remote-control permission is stored: the desktop asks the next time Kai types \
             through the portal."
        }
    }
}

/// What a forget did, for the line beside the button.
pub fn forgot_note(forgot: Option<bool>) -> &'static str {
    match forgot {
        Some(true) => "forgotten",
        _ => "none was stored",
    }
}

/// `entries` with `entry` in place of its key's (or at the end when the key is new).
pub fn replaced(mut entries: Vec<Entry>, entry: &Entry) -> Vec<Entry> {
    match entries.iter_mut().find(|e| e.key == entry.key) {
        Some(slot) => *slot = entry.clone(),
        None => entries.push(entry.clone()),
    }
    entries
}

/// The capability's heading: `windows` as `Windows`.
pub fn title_case(word: &str) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A state, level or provider word as the page shows it (`fdo` is the freedesktop base).
pub fn word_label(word: &str) -> String {
    match word {
        "fdo" => "freedesktop".to_owned(),
        "none" => "None (off)".to_owned(),
        "allow_trusted" => "Allow if trusted".to_owned(),
        other => title_case(other),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(key: &str, value: Value, set: bool) -> Entry {
        Entry {
            key: key.into(),
            value,
            set,
            choices: Vec::new(),
        }
    }

    fn tool(name: &str, area: &str, description: &str) -> ToolInfo {
        ToolInfo {
            name: name.into(),
            area: area.into(),
            description: description.into(),
            read_only: true,
            default_state: "confirm".into(),
            default_level: "any".into(),
            config_tool: false,
        }
    }

    #[test]
    fn the_rows_carry_what_the_file_sets_and_nothing_else() {
        let tools = [
            tool("key_press", "input", "x"),
            tool("window_list", "windows", "y"),
        ];
        let entries = [
            entry("policy.tools.key_press", json!("allow"), true),
            // a default the list shows for a key the file does not have is not a setting
            entry("policy.tools.window_list", json!("deny"), false),
            entry("policy.levels.window_list", json!("owner"), true),
        ];
        let rows = tool_rows(&tools, &entries);
        assert_eq!(rows[0].state.as_deref(), Some("allow"));
        assert_eq!(rows[0].level, None);
        assert_eq!(rows[1].state, None);
        assert_eq!(rows[1].level.as_deref(), Some("owner"));
    }

    #[test]
    fn rows_group_by_area_in_the_contract_order_and_filter_by_words() {
        let tools = [
            tool("window_list", "windows", "Lists the open windows."),
            tool("clipboard_get", "read", "Reads the clipboard."),
            tool("window_focus", "windows", "Raises one."),
        ];
        let rows = tool_rows(&tools, &[]);
        let groups = by_area(rows.clone(), "");
        assert_eq!(
            groups
                .iter()
                .map(|(a, r)| (a.as_str(), r.len()))
                .collect::<Vec<_>>(),
            [("windows", 2), ("read", 1)]
        );
        let found = by_area(rows.clone(), "window raises");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1[0].info.name, "window_focus");
        assert!(by_area(rows, "nothing like this").is_empty());
    }

    #[test]
    fn a_description_shortens_to_its_first_sentence() {
        assert_eq!(
            first_sentence("Lists windows. Then more."),
            "Lists windows."
        );
        assert_eq!(first_sentence("Waits 0.5 s. Done."), "Waits 0.5 s.");
        assert_eq!(first_sentence("No end"), "No end");
    }

    #[test]
    fn values_read_back_as_the_text_set_accepts() {
        assert_eq!(show_value(&json!("automatic")), "automatic");
        assert_eq!(show_value(&json!(30)), "30");
        assert_eq!(show_value(&json!(1.5)), "1.5");
        assert_eq!(show_value(&json!(true)), "true");
        assert_eq!(show_value(&Value::Null), "");
        assert_eq!(show_value(&json!(["a", "b"])), r#"["a", "b"]"#);
        assert_eq!(show_value(&json!([1, 2])), "[1, 2]");
    }

    #[test]
    fn editors_follow_the_value_and_pairing_is_read_only() {
        assert_eq!(
            editor(&entry("voice.grace_s", json!(30), true)),
            Editor::Text
        );
        assert_eq!(editor(&entry("a.b", json!(false), false)), Editor::Flag);
        assert_eq!(
            editor(&entry("lmgw.url", json!("http://x"), true)),
            Editor::ReadOnly
        );
    }

    #[test]
    fn an_entry_with_words_gets_a_select() {
        let mut position = entry("overlay.weather.position", json!("top-right"), false);
        assert_eq!(editor(&position), Editor::Text, "no words: a text box");
        position.choices = vec!["top-left".into(), "top-right".into()];
        assert_eq!(editor(&position), Editor::Choice);
        // An unset optional enum (null) is a select too; pairing's key stays read-only.
        let mut provider = entry("desktop.capture", Value::Null, false);
        provider.choices = vec!["plasma".into()];
        assert_eq!(editor(&provider), Editor::Choice);
        let mut url = entry("lmgw.url", json!("http://x"), true);
        url.choices = vec!["x".into()];
        assert_eq!(editor(&url), Editor::ReadOnly);
    }

    #[test]
    fn a_sections_keys_group_by_their_table() {
        let entries = vec![
            entry("overlay.app.position", json!("center"), false),
            entry("overlay.app_max_mb", json!(64), false),
            entry("overlay.output", Value::Null, false),
            entry("overlay.weather.height", json!(230), false),
            entry("overlay.weather.position", json!("top-right"), true),
        ];
        let groups: Vec<(Option<String>, Vec<String>)> = by_table("overlay", entries)
            .into_iter()
            .map(|(t, list)| (t, list.into_iter().map(|e| e.key).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                (
                    None,
                    vec!["overlay.app_max_mb".into(), "overlay.output".into()]
                ),
                (
                    Some("overlay.app".into()),
                    vec!["overlay.app.position".into()]
                ),
                (
                    Some("overlay.weather".into()),
                    vec![
                        "overlay.weather.height".into(),
                        "overlay.weather.position".into()
                    ]
                ),
            ]
        );
        // A section with no table under it is one group without a heading.
        let flat = by_table("voice", vec![entry("voice.grace_s", json!(30), true)]);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, None);
        assert!(by_table("voice", Vec::new()).is_empty());
    }

    #[test]
    fn the_settings_list_leaves_the_per_tool_entries_to_the_table_until_filtered() {
        let entries = [
            entry("audio.echo_mode", json!("app"), false),
            entry("policy.tools.key_press", json!("allow"), true),
            entry("policy.config_tools", json!(false), false),
            entry("voice.mode", json!("automatic"), true),
        ];
        let sections: Vec<String> = settings_sections(&entries, "")
            .into_iter()
            .map(|s| s.0)
            .collect();
        assert_eq!(sections, ["audio", "policy", "voice"]);
        assert_eq!(settings_sections(&entries, "")[1].1.len(), 1);
        let found = settings_sections(&entries, "key_press");
        assert_eq!(found[0].1[0].key, "policy.tools.key_press");
        assert_eq!(settings_sections(&entries, "echo mode")[0].1.len(), 1);
    }

    #[test]
    fn the_per_tool_opt_in_is_read_and_stays_out_of_the_list() {
        let entries = [
            entry("policy.widen_by_tools.key_press", json!(true), true),
            entry("policy.widen_by_tools.type_text", json!(false), true),
            entry("policy.widen_trust_by_tools", json!(false), false),
        ];
        assert!(flag_on(&entries, "policy.widen_by_tools.key_press"));
        assert!(!flag_on(&entries, "policy.widen_by_tools.type_text"));
        assert!(!flag_on(&entries, "policy.widen_by_tools.other"));
        // The trust opt-in is a plain flag in the list; the per-tool ones are the table's.
        assert_eq!(settings_sections(&entries, "")[0].1.len(), 1);
    }

    #[test]
    fn providers_come_from_the_desktop_section() {
        let entries = [
            entry("desktop.windows", json!("fdo"), true),
            entry("desktop.capture", Value::Null, false),
            entry("voice.mode", json!("automatic"), true),
        ];
        assert_eq!(
            provider_rows(&entries),
            [
                ("windows".into(), Some("fdo".into())),
                ("capture".into(), None)
            ]
        );
    }

    #[test]
    fn words_get_labels() {
        assert_eq!(word_label("fdo"), "freedesktop");
        assert_eq!(word_label("confirm"), "Confirm");
        assert_eq!(word_label("allow_trusted"), "Allow if trusted");
        assert_eq!(word_label("none"), "None (off)");
    }

    #[test]
    fn a_changed_entry_replaces_its_key_or_joins() {
        let a = entry("a.b", json!(1), true);
        let list = replaced(vec![a.clone()], &entry("a.b", json!(2), true));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].value, json!(2));
        assert_eq!(replaced(list, &entry("a.c", json!(3), true)).len(), 2);
        assert!(config_tools_on(&[entry(
            "policy.config_tools",
            json!(true),
            true
        )]));
        assert!(!config_tools_on(&[]));
    }

    #[test]
    fn the_remote_control_lines_say_what_is_stored() {
        assert!(remote_status(Some(true)).starts_with("A remote-control permission is stored"));
        assert!(remote_status(Some(false)).starts_with("No remote-control permission"));
        assert!(remote_status(None).starts_with("Asking Kai"));
        assert_eq!(forgot_note(Some(true)), "forgotten");
        assert_eq!(forgot_note(Some(false)), "none was stored");
        assert_eq!(forgot_note(None), "none was stored");
    }
}
