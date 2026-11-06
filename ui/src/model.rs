//! The page's logic apart from the DOM: which rows the tool table has, what the settings list
//! shows for a value, how the entries group (by the catalog's sections) and filter. Plain functions over [`wire`](crate::wire)'s
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

/// One section of the Settings page: its heading and its keys, the everyday ones first and the
/// rare tuning under "Advanced", each part in groups (`None`: the section's own keys).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionView {
    pub id: String,
    pub label: String,
    pub main: Vec<(Option<String>, Vec<String>)>,
    pub advanced: Vec<(Option<String>, Vec<String>)>,
}

impl SectionView {
    pub fn len(&self) -> usize {
        self.main
            .iter()
            .chain(&self.advanced)
            .map(|(_, k)| k.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn advanced_len(&self) -> usize {
        self.advanced.iter().map(|(_, k)| k.len()).sum()
    }
}

/// An entry matches when every word of `filter` is in its label, help, section or group (or its
/// key, for someone who knows the file).
pub fn entry_matches(e: &Entry, filter: &str) -> bool {
    let m = &e.meta;
    let hay = format!(
        "{} {} {} {} {}",
        m.label,
        m.help,
        m.section_label,
        m.group.as_deref().unwrap_or_default(),
        e.key
    )
    .to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w))
}

/// The Settings page's sections in the catalog's order, with the entries that match `filter`. The
/// sections with pages of their own (the policy table's, the providers') stay out.
pub fn settings_sections(entries: &[Entry], filter: &str) -> Vec<SectionView> {
    let mut sorted: Vec<&Entry> = entries
        .iter()
        .filter(|e| !crate::wire::OWN_PAGE_SECTIONS.contains(&e.meta.section.as_str()))
        .filter(|e| e.meta.control != crate::wire::Control::Hidden)
        .filter(|e| entry_matches(e, filter))
        .collect();
    sorted.sort_by(|a, b| (a.meta.order, &a.key).cmp(&(b.meta.order, &b.key)));
    let mut sections: Vec<SectionView> = Vec::new();
    for e in sorted {
        let at = match sections.iter().position(|s| s.id == e.meta.section) {
            Some(i) => i,
            None => {
                sections.push(SectionView {
                    id: e.meta.section.clone(),
                    label: e.meta.section_label.clone(),
                    main: Vec::new(),
                    advanced: Vec::new(),
                });
                sections.len() - 1
            }
        };
        let section = &mut sections[at];
        let part = if e.meta.advanced {
            &mut section.advanced
        } else {
            &mut section.main
        };
        match part.iter_mut().find(|(g, _)| *g == e.meta.group) {
            Some((_, keys)) => keys.push(e.key.clone()),
            None => part.push((e.meta.group.clone(), vec![e.key.clone()])),
        }
    }
    sections
}

/// A list's items as the text box shows them: one per line.
pub fn list_lines(v: &Value) -> String {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => show_value(other),
    }
}

/// The lines of a list box as the TOML array Kai's `set` takes; `None` for no line (the default
/// applies again).
pub fn list_value(text: &str) -> Option<String> {
    let items: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| Value::String(l.to_owned()).to_string())
        .collect();
    (!items.is_empty()).then(|| format!("[{}]", items.join(", ")))
}

/// What the owner typed into a number field: `Ok(None)` for nothing (the default applies again),
/// the number when it is a whole one within the bounds, else why not, with the unit.
pub fn number_value(
    text: &str,
    min: Option<i64>,
    max: Option<i64>,
    unit: Option<&str>,
) -> Result<Option<i64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let unit = unit.map(|u| format!(" {u}")).unwrap_or_default();
    let n: i64 = text
        .parse()
        .map_err(|_| "Enter a whole number.".to_owned())?;
    if let Some(min) = min
        && n < min
    {
        return Err(format!("At least {min}{unit}."));
    }
    if let Some(max) = max
        && n > max
    {
        return Err(format!("At most {max}{unit}."));
    }
    Ok(Some(n))
}

/// A value shown, not edited: a flag as On or Off, nothing as "Not set".
pub fn read_only_text(v: &Value) -> String {
    match v {
        Value::Bool(true) => "On".to_owned(),
        Value::Bool(false) => "Off".to_owned(),
        Value::Null => "Not set".to_owned(),
        other => show_value(other),
    }
}

/// The Conversation choice as the stored name reads: `None` while it is the shared one, else the
/// name of this computer's own.
pub fn own_conversation(value: &Value, shared: &str) -> Option<String> {
    let name = value.as_str().unwrap_or(shared).trim();
    (name != shared.trim()).then(|| name.to_owned())
}

/// The write the Conversation's own-name field asks for: the name (`Some`), `None` for the shared
/// one (the key removed: its default is the shared name), or why the name cannot be saved.
pub fn own_name_value(text: &str, shared: &str) -> Result<Option<String>, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err(
            "Give this computer's conversation a name, or choose the shared one.".to_owned(),
        );
    }
    Ok((name != shared).then(|| name.to_owned()))
}

/// The providers' rows: `desktop.<capability>` in the list's order, with the owner's choice.
pub fn provider_rows(entries: &[Entry]) -> Vec<(String, Option<String>)> {
    entries
        .iter()
        .filter_map(|e| Some((e.key.strip_prefix("desktop.")?, e)))
        .map(|(cap, e)| (cap.to_owned(), e.value.as_str().map(str::to_owned)))
        .collect()
}

/// What the line beside a control says once a change was saved: a brief "Saved" for one that took
/// hold at once (the page clears it again), and for one that waits for a restart, the fact.
pub fn effect_note(effect: Effect) -> &'static str {
    match effect {
        Effect::Now => "Saved",
        Effect::Restart => "Applies after Kai restarts",
    }
}

/// A setting that waits for a restart: what it was when Kai started (so changing it back drops it
/// from the footer).
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting {
    pub key: String,
    pub label: String,
    pub at_start: Value,
}

/// `waiting` after `entry` was saved with an effect that applies only at a restart: it is added
/// (remembering `before`, its value until now), or dropped when it is back to what it was at
/// the start.
pub fn waiting_after(mut waiting: Vec<Waiting>, entry: &Entry, before: &Value) -> Vec<Waiting> {
    match waiting.iter().position(|w| w.key == entry.key) {
        Some(i) if waiting[i].at_start == entry.value => {
            waiting.remove(i);
        }
        Some(_) => {}
        None => waiting.push(Waiting {
            key: entry.key.clone(),
            label: entry.meta.label.clone(),
            at_start: before.clone(),
        }),
    }
    waiting
}

/// The footer's sentence: which settings wait for a restart.
pub fn waiting_line(waiting: &[Waiting]) -> String {
    let names = waiting
        .iter()
        .map(|w| format!("\u{201c}{}\u{201d}", w.label))
        .collect::<Vec<_>>()
        .join(", ");
    format!("These settings apply after Kai restarts: {names}.")
}

/// The Reset button shows only for a value that differs from the default.
pub fn resettable(entry: &Entry) -> bool {
    entry.set && entry.value != entry.meta.default
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

/// The Firefox section's line on the extension's link to Kai (`None`: Kai has not answered yet).
pub fn firefox_link_line(linked: Option<usize>) -> String {
    match linked {
        None => "Asking Kai whether Kai for Firefox is connected\u{2026}".to_owned(),
        Some(0) => "Kai for Firefox is not connected right now: Firefox is not running, or the \
                    extension is not installed or does not reach Kai."
            .to_owned(),
        Some(1) => "Kai for Firefox is connected.".to_owned(),
        Some(n) => format!("Kai for Firefox is connected in {n} Firefox profiles."),
    }
}

/// What the section says about installing: with a link in the build the button's help, else how to
/// install by hand (a link that was set but is of no use comes first, `problem`).
pub fn firefox_install_help(install: bool, problem: Option<&str>) -> String {
    if install {
        return "Opens the signed extension in your default browser, which has to be Firefox; \
                Firefox asks you before it installs it."
            .to_owned();
    }
    let by_hand = "Install it by hand: get the signed file kai-firefox-\u{2026}.xpi from the Kai \
                   for Firefox releases, open about:addons in Firefox, click the gear button, \
                   choose \u{201c}Install Add-on From File\u{2026}\u{201d} and pick the file.";
    match problem {
        Some(rule) => format!(
            "This build of Kai has a download link for Kai for Firefox that cannot be used \
             ({rule}). {by_hand}"
        ),
        None => format!("This build of Kai has no download link for Kai for Firefox. {by_hand}"),
    }
}

/// What the click's answer says beside the button.
pub fn firefox_opened_note(opened: Option<bool>) -> &'static str {
    match opened {
        Some(true) => "Opened in your browser: confirm the installation there.",
        _ => "Kai did not open the link.",
    }
}

/// What a forget did, for the line beside the button.
pub fn forgot_note(forgot: Option<bool>) -> &'static str {
    match forgot {
        Some(true) => "Forgotten",
        _ => "None was stored",
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

/// The announcements' switch, which the hands-and-eyes switch turns on when the owner turns that
/// one on (kai-firefox design §7.2).
pub const ANNOUNCE_KEY: &str = "browser.announce";

const HANDS_AND_EYES_KEY: &str = "browser.hands_and_eyes";

/// Turning hands and eyes on from off in the window also writes `browser.announce` on, so one
/// switch gives a screen-reader user the announcements; only that transition (`before` is the
/// switch's shown value), and not when announcements are on already. The owner can turn them off
/// again and that stays.
pub fn announces_with(
    key: &str,
    new_value: Option<&str>,
    before: &Value,
    entries: &[Entry],
) -> bool {
    key == HANDS_AND_EYES_KEY
        && new_value == Some("true")
        && *before != Value::Bool(true)
        && entries
            .iter()
            .find(|e| e.key == ANNOUNCE_KEY)
            .is_none_or(|e| e.value != Value::Bool(true))
}

/// The capability's heading: `windows` as `Windows`.
pub fn title_case(word: &str) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A state, level or provider word as the page shows it: the catalog's name for it
/// ([`crate::wire::word_name`]).
pub fn word_label(word: &str) -> String {
    crate::wire::word_name(word).to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::wire::{Control, Meta};

    #[test]
    fn hands_and_eyes_on_switches_announcements_on_once() {
        let off = Value::Bool(false);
        let announce_off = [entry(ANNOUNCE_KEY, Value::Bool(false), false)];
        assert!(announces_with(
            "browser.hands_and_eyes",
            Some("true"),
            &off,
            &announce_off
        ));
        // Already on, turned off, another key, an unset: no second write.
        assert!(!announces_with(
            "browser.hands_and_eyes",
            Some("true"),
            &Value::Bool(true),
            &announce_off
        ));
        assert!(!announces_with(
            "browser.hands_and_eyes",
            Some("false"),
            &Value::Bool(true),
            &announce_off
        ));
        assert!(!announces_with(
            "browser.announce",
            Some("true"),
            &off,
            &announce_off
        ));
        assert!(!announces_with(
            "browser.hands_and_eyes",
            None,
            &off,
            &announce_off
        ));
        // Announcements on already: nothing to write.
        let on = [entry(ANNOUNCE_KEY, Value::Bool(true), true)];
        assert!(!announces_with(
            "browser.hands_and_eyes",
            Some("true"),
            &off,
            &on
        ));
    }

    /// An entry as Kai sends it: the section is the key's first segment (the per-tool and
    /// provider keys in their pages' sections), the label a stand-in.
    fn entry(key: &str, value: Value, set: bool) -> Entry {
        let section = if key.starts_with("policy.tools.")
            || key.starts_with("policy.levels.")
            || key.starts_with("policy.widen_by_tools.")
        {
            "tools"
        } else {
            key.split('.').next().unwrap()
        };
        Entry {
            key: key.into(),
            value,
            set,
            meta: Meta {
                section: section.into(),
                section_label: title_case(section),
                group: None,
                label: format!("Label of {key}"),
                help: String::new(),
                unit: None,
                control: Control::Text { unset: None },
                advanced: false,
                order: 0,
                default: Value::Null,
            },
        }
    }

    fn with(mut e: Entry, f: impl FnOnce(&mut Meta)) -> Entry {
        f(&mut e.meta);
        e
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
            by_switch: false,
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
    fn sections_follow_the_catalogs_order_with_advanced_keys_apart() {
        let entries = [
            with(entry("voice.keepalive_s", json!(15), false), |m| {
                m.order = 3;
                m.advanced = true;
            }),
            with(entry("voice.mode", json!("automatic"), true), |m| {
                m.order = 2
            }),
            with(
                entry("conversation.folder_name", json!("Kai"), false),
                |m| m.order = 0,
            ),
            with(entry("overlay.text.width", json!(360), false), |m| {
                m.order = 5;
                m.group = Some("Text overlay".into());
            }),
            with(entry("overlay.output", Value::Null, false), |m| m.order = 4),
            with(entry("policy.tools.key_press", json!("allow"), true), |m| {
                m.order = 9
            }),
            with(entry("desktop.capture", Value::Null, false), |m| {
                m.order = 10
            }),
        ];
        let sections = settings_sections(&entries, "");
        let ids: Vec<&str> = sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            ["conversation", "voice", "overlay"],
            "the catalog's order; the Tools and Desktop pages' keys are theirs"
        );
        assert_eq!(sections[1].main, [(None, vec!["voice.mode".to_owned()])]);
        assert_eq!(
            sections[1].advanced,
            [(None, vec!["voice.keepalive_s".to_owned()])]
        );
        assert_eq!((sections[1].len(), sections[1].advanced_len()), (2, 1));
        assert_eq!(
            sections[2].main,
            [
                (None, vec!["overlay.output".to_owned()]),
                (
                    Some("Text overlay".to_owned()),
                    vec!["overlay.text.width".to_owned()]
                )
            ]
        );
    }

    #[test]
    fn the_filter_reads_the_words_not_only_the_key() {
        let entries = [
            with(entry("voice.grace_s", json!(30), false), |m| {
                m.label = "Keep a hold-to-talk session open".into();
                m.help = "With listening off, the session closes after the reply.".into();
            }),
            with(entry("audio.echo_mode", json!("app"), false), |m| {
                m.label = "Echo handling".into();
                m.section_label = "Audio".into();
            }),
        ];
        let keys = |f: &str| -> Vec<String> {
            settings_sections(&entries, f)
                .into_iter()
                .flat_map(|s| s.main.into_iter().flat_map(|(_, k)| k))
                .collect()
        };
        assert_eq!(keys("hold session"), ["voice.grace_s"]);
        assert_eq!(keys("ECHO"), ["audio.echo_mode"]);
        assert_eq!(keys("audio"), ["audio.echo_mode"]);
        assert_eq!(keys("grace_s"), ["voice.grace_s"], "the key still finds it");
        assert!(keys("nothing like this").is_empty());
    }

    #[test]
    fn lists_are_lines_and_numbers_are_checked_against_their_bounds() {
        assert_eq!(list_lines(&json!(["konsole", "-e"])), "konsole\n-e");
        assert_eq!(list_lines(&Value::Null), "");
        assert_eq!(
            list_value("  yes \n\nja bitte\n"),
            Some(r#"["yes", "ja bitte"]"#.to_owned())
        );
        assert_eq!(
            list_value(r#"say "hi""#),
            Some(r#"["say \"hi\""]"#.to_owned())
        );
        assert_eq!(list_value(" \n "), None, "no line: the default again");
        assert_eq!(number_value(" 42 ", Some(1), None, Some("s")), Ok(Some(42)));
        assert_eq!(number_value("", Some(1), None, Some("s")), Ok(None));
        assert_eq!(
            number_value("0", Some(1), None, Some("s")),
            Err("At least 1 s.".to_owned())
        );
        assert_eq!(
            number_value("5", Some(-120), Some(0), Some("dBFS")),
            Err("At most 0 dBFS.".to_owned())
        );
        assert_eq!(
            number_value("1.5", None, None, None),
            Err("Enter a whole number.".to_owned())
        );
        assert_eq!(read_only_text(&json!(true)), "On");
        assert_eq!(read_only_text(&Value::Null), "Not set");
        assert_eq!(read_only_text(&json!(12)), "12");
    }

    #[test]
    fn the_conversation_is_shared_unless_it_has_a_name_of_its_own() {
        assert_eq!(own_conversation(&json!("Kai"), "Kai"), None);
        assert_eq!(own_conversation(&Value::Null, "Kai"), None);
        assert_eq!(
            own_conversation(&json!("Kai desktop"), "Kai"),
            Some("Kai desktop".to_owned())
        );
        assert_eq!(
            own_name_value(" Kai desktop ", "Kai"),
            Ok(Some("Kai desktop".to_owned()))
        );
        assert_eq!(own_name_value("Kai", "Kai"), Ok(None), "the shared one");
        assert!(own_name_value("  ", "Kai").is_err());
    }

    #[test]
    fn the_per_tool_opt_in_is_read() {
        let entries = [
            entry("policy.widen_by_tools.key_press", json!(true), true),
            entry("policy.widen_by_tools.type_text", json!(false), true),
            entry("policy.widen_trust_by_tools", json!(false), false),
        ];
        assert!(flag_on(&entries, "policy.widen_by_tools.key_press"));
        assert!(!flag_on(&entries, "policy.widen_by_tools.type_text"));
        assert!(!flag_on(&entries, "policy.widen_by_tools.other"));
        // The trust opt-in is a setting of the list; the per-tool ones are the table's.
        assert_eq!(settings_sections(&entries, "").len(), 1);
        assert_eq!(settings_sections(&entries, "")[0].len(), 1);
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
        assert_eq!(word_label("own"), "Kai's own key");
        assert_eq!(word_label("owner"), "You");
        assert_eq!(word_label("trusted"), "Trusted callers");
        assert_eq!(word_label("any"), "Any caller");
    }

    #[test]
    fn a_setting_waiting_for_a_restart_is_listed_until_it_is_changed_back() {
        let mut e = entry("audio.capture_ring_ms", json!(500), true);
        e.meta.label = "Microphone buffer".into();
        let waiting = waiting_after(Vec::new(), &e, &json!(200));
        assert_eq!(waiting.len(), 1);
        assert_eq!(
            waiting_line(&waiting),
            "These settings apply after Kai restarts: \u{201c}Microphone buffer\u{201d}."
        );
        // Changed again to something else: still listed, still remembering the start.
        e.value = json!(600);
        let waiting = waiting_after(waiting, &e, &json!(500));
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].at_start, json!(200));
        // Back to what it was at the start: gone.
        e.value = json!(200);
        assert!(waiting_after(waiting, &e, &json!(600)).is_empty());
    }

    #[test]
    fn reset_shows_only_for_a_value_that_differs_from_the_default() {
        let mut e = entry("a.b", json!(30), true);
        e.meta.default = json!(30);
        assert!(!resettable(&e), "set to the default");
        e.value = json!(40);
        assert!(resettable(&e));
        e.set = false;
        assert!(!resettable(&e), "not in the file");
    }

    #[test]
    fn a_saved_line_is_a_sentence_not_a_lowercase_status_word() {
        assert_eq!(effect_note(Effect::Now), "Saved");
        assert_eq!(effect_note(Effect::Restart), "Applies after Kai restarts");
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
        assert_eq!(forgot_note(Some(true)), "Forgotten");
        assert_eq!(forgot_note(Some(false)), "None was stored");
        assert_eq!(forgot_note(None), "None was stored");
    }

    #[test]
    fn the_firefox_section_says_whether_the_extension_is_connected() {
        assert!(firefox_link_line(None).starts_with("Asking Kai"));
        assert!(firefox_link_line(Some(0)).contains("not connected"));
        assert_eq!(firefox_link_line(Some(1)), "Kai for Firefox is connected.");
        assert_eq!(
            firefox_link_line(Some(3)),
            "Kai for Firefox is connected in 3 Firefox profiles."
        );
    }

    #[test]
    fn the_firefox_section_offers_the_link_or_says_how_to_install_by_hand() {
        let with_link = firefox_install_help(true, None);
        assert!(with_link.contains("default browser") && with_link.contains("Firefox asks you"));
        assert!(!with_link.contains("about:addons"));
        for (problem, said) in [
            (None, "no download link"),
            (Some("it has no host"), "cannot be used"),
        ] {
            let by_hand = firefox_install_help(false, problem);
            assert!(by_hand.contains(said), "{by_hand}");
            for step in ["about:addons", "Install Add-on From File", ".xpi"] {
                assert!(by_hand.contains(step), "{by_hand}");
            }
        }
        assert!(firefox_install_help(false, Some("it has no host")).contains("(it has no host)"));
        assert!(firefox_opened_note(Some(true)).starts_with("Opened in your browser"));
        assert_eq!(firefox_opened_note(None), "Kai did not open the link.");
    }
}
