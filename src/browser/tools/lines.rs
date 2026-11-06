//! The words the reading tools share (kai-firefox design §5, §5.3; kai K22): the contract's
//! shapes (`docs/protocol/operations.md`: Element, Control, Block, Frames) read past what a newer
//! extension adds, and written as compact lines, never cut.
//!
//! - **An element** is `<ref> <role> "<name>"`, a heading's level after its role; the name is page
//!   text and always quoted, so it cannot forge a line.
//! - **A control** adds its value (quoted), a link's URL, and its states, label, error and
//!   description in parentheses. A sensitive field's value is never in an answer: Kai says
//!   "password field, filled (12 characters)" (§5's words).
//! - **Text blocks** are page text: line by line as the page shows it, a heading as `#`s with its
//!   ref, a list item after `- `. A line with a character that could deceive (a control or bidi
//!   character) is quoted; a table row's tab between cells stays a tab.
//! - **Frames that could not be read** are said where they are (the text tools) and named at the
//!   end, each by its site (its origin's host) or its `<iframe>`'s title, with the reason.
//! - **A frame of a site Kai does not reach** (`super::page::withhold`) is one line where it is,
//!   [`WITHHELD`], and nothing of it.

use serde::Deserialize;
use serde_json::Value;

use crate::browser::tabs::or_default;
use crate::host::confirm::{quoted, shown_text};

/// An Element (the contract's): ref, role, name, and a heading's level where it has one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Element {
    #[serde(rename = "ref")]
    pub id: i64,
    #[serde(deserialize_with = "or_default")]
    pub role: String,
    #[serde(deserialize_with = "or_default")]
    pub name: String,
    pub level: Option<i64>,
}

/// `<ref> <role> "<name>"`, the name left out when the element has none.
pub fn element(e: &Element) -> String {
    let role = match e.level {
        Some(l) => format!("{} {l}", shown_text(&e.role)),
        None => shown_text(&e.role).into_owned(),
    };
    if e.name.is_empty() {
        format!("{} {role}", e.id)
    } else {
        format!("{} {role} {}", e.id, quoted(&e.name))
    }
}

/// A sensitive field's value as Kai says it (design §5): "password field, filled (12
/// characters)", or "password field, empty".
pub fn masked(length: u64) -> String {
    match length {
        0 => "password field, empty".into(),
        1 => "password field, filled (1 character)".into(),
        n => format!("password field, filled ({n} characters)"),
    }
}

/// A Control (the contract's): an Element with its value and states, each there only where it
/// applies.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Control {
    #[serde(flatten)]
    pub element: Element,
    pub value: Option<String>,
    pub masked: bool,
    pub length: u64,
    pub min: Option<String>,
    pub max: Option<String>,
    /// `true`, `false` or `"mixed"`.
    pub checked: Option<Value>,
    pub selected: Option<bool>,
    pub pressed: Option<Value>,
    pub expanded: Option<bool>,
    pub disabled: bool,
    pub readonly: bool,
    pub required: bool,
    pub invalid: bool,
    pub error: Option<String>,
    pub description: Option<String>,
    pub label: Option<String>,
    pub url: Option<String>,
    pub group: Option<String>,
    /// `page.links@1`, `page.fields@1`: partly visible now.
    pub in_view: Option<bool>,
    pub inert: bool,
    /// A child frame's id; 0 or none: the top document.
    pub frame: Option<i64>,
    /// In a frame of a site Kai does not reach: nothing else is there (`super::page::withhold`).
    pub withheld: bool,
    /// The extension's number where Kai's differs: what its hint label shows
    /// (`crate::browser::refs`).
    pub ext_ref: Option<i64>,
}

/// The lines of `items` (`line` each), a run of withheld ones of one frame (`held` gives an item's
/// frame when it is withheld) as one [`WITHHELD`] line.
pub fn listed<T>(
    items: &[T],
    held: impl Fn(&T) -> Option<Option<i64>>,
    line: impl Fn(&T) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut last: Option<Option<i64>> = None;
    for item in items {
        match held(item) {
            Some(frame) => {
                if last != Some(frame) {
                    out.push(WITHHELD.to_owned());
                }
                last = Some(frame);
            }
            None => {
                last = None;
                out.push(line(item));
            }
        }
    }
    out
}

/// The lines of a list of controls ([`listed`]); `hint` as [`control`].
pub fn controls(list: &[Control], hint: bool) -> Vec<String> {
    listed(
        list,
        |c| c.withheld.then_some(c.frame),
        |c| control(c, hint),
    )
}

/// `true`, `false` or `"mixed"` in words.
fn tri(v: &Value, on: &str, off: &str, mixed: &str) -> Option<String> {
    match v {
        Value::Bool(true) => Some(on.into()),
        Value::Bool(false) => Some(off.into()),
        Value::String(s) if s == "mixed" => Some(mixed.into()),
        _ => None,
    }
}

/// Where a frame of a site Kai does not reach is.
pub const WITHHELD: &str = "(a frame of a site Kai does not reach)";

/// One control's line: `<element>` ` = "<value>"` (or the masked words) ` <URL>` ` (<facts>)`;
/// with `hint`, the number its hint label shows where that is not its ref.
pub fn control(c: &Control, hint: bool) -> String {
    let mut line = element(&c.element);
    if c.masked {
        line.push_str(": ");
        line.push_str(&masked(c.length));
    } else if let Some(v) = &c.value {
        line.push_str(" = ");
        line.push_str(&quoted(v));
    }
    if let Some(u) = &c.url {
        line.push(' ');
        line.push_str(&shown_text(u));
    }
    let mut facts: Vec<String> = Vec::new();
    if let Some(n) = c.ext_ref.filter(|_| hint) {
        facts.push(format!("hint {n}"));
    }
    match (&c.min, &c.max) {
        (Some(lo), Some(hi)) => facts.push(format!("from {} to {}", quoted(lo), quoted(hi))),
        (Some(lo), None) => facts.push(format!("from {}", quoted(lo))),
        (None, Some(hi)) => facts.push(format!("up to {}", quoted(hi))),
        (None, None) => {}
    }
    facts.extend(
        c.checked
            .as_ref()
            .and_then(|v| tri(v, "checked", "not checked", "partly checked")),
    );
    if c.selected == Some(true) {
        facts.push("selected".into());
    }
    facts.extend(
        c.pressed
            .as_ref()
            .and_then(|v| tri(v, "pressed", "not pressed", "partly pressed")),
    );
    match c.expanded {
        Some(true) => facts.push("expanded".into()),
        Some(false) => facts.push("collapsed".into()),
        None => {}
    }
    for (on, word) in [
        (c.disabled, "disabled"),
        (c.readonly, "read-only"),
        (c.required, "required"),
    ] {
        if on {
            facts.push(word.into());
        }
    }
    if c.invalid {
        facts.push(match &c.error {
            Some(e) => format!("invalid: {}", quoted(e)),
            None => "invalid".into(),
        });
    }
    if let Some(l) = &c.label {
        facts.push(format!("label {}", quoted(l)));
    }
    if let Some(d) = &c.description {
        facts.push(format!("description {}", quoted(d)));
    }
    if let Some(g) = &c.group {
        facts.push(format!("in {}", quoted(g)));
    }
    if c.in_view == Some(true) {
        facts.push("in view".into());
    }
    if c.inert {
        facts.push("cannot be used now".into());
    }
    if !facts.is_empty() {
        line.push_str(&format!(" ({})", facts.join(", ")));
    }
    line
}

/// A Block (the contract's): one block of text, or a child frame that could not be read where its
/// `<iframe>` is (no `text`), or one of a site Kai does not reach (`withheld`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Block {
    pub text: Option<String>,
    pub heading: Option<i64>,
    #[serde(rename = "ref")]
    pub id: Option<i64>,
    pub item: bool,
    pub frame: Option<i64>,
    pub origin: Option<String>,
    pub unread: Option<String>,
    pub title: Option<String>,
    pub withheld: bool,
}

/// Frames (the contract's): how many of the page's frames answered and how many could not be
/// read, and those, each with its reason.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Frames {
    pub read: u64,
    pub unread: u64,
    pub unread_frames: Vec<UnreadFrame>,
}

/// A frame that could not be read (the contract's `unread_frames` entry).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UnreadFrame {
    pub frame: Option<i64>,
    pub origin: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub code: String,
    pub title: Option<String>,
    pub withheld: bool,
}

/// Why a frame could not be read, in words (its code; `hints.show@1`'s `undrawn` has the same).
pub fn unread_why(code: &str) -> String {
    match code {
        "content_timeout" => "it did not answer within Kai for Firefox's setting content_ms".into(),
        "restricted_domain" => "Firefox keeps extensions off its site".into(),
        "site_permission" => "the user took Kai for Firefox's access to its site".into(),
        "firefox_page" => "it shows one of Firefox's own pages".into(),
        "file_access" => "it shows a local file, which Kai for Firefox may not read".into(),
        "no_content" => "Kai for Firefox cannot run in it".into(),
        other => format!("Kai for Firefox says {}", shown_text(other)),
    }
}

/// The site an origin is, as a line names it: its host, and its port where it has one.
pub fn site(origin: Option<&str>) -> Option<String> {
    let u = url::Url::parse(origin?).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// A frame as a line names it: its site and its `<iframe>`'s title where it has them.
pub fn frame_name(origin: Option<&str>, title: Option<&str>) -> String {
    let title = title.filter(|t| !t.trim().is_empty()).map(quoted);
    match (site(origin), title) {
        (Some(s), Some(t)) => format!("{} {t}", shown_text(&s)),
        (Some(s), None) => shown_text(&s).into_owned(),
        (None, Some(t)) => format!("frame {t}"),
        (None, None) => "a frame without a title".into(),
    }
}

/// A line of page text: as it is, unless a character in it could deceive (then quoted); a tab
/// between a table row's cells stays a tab.
pub fn page_line(line: &str) -> String {
    line.split('\t')
        .map(|cell| shown_text(cell).into_owned())
        .collect::<Vec<_>>()
        .join("\t")
}

/// A text from a page as lines (`page_line` each): a source, a document's HTML, a script's value.
/// A line's carriage return before its line feed goes with it, so a document written with CRLF
/// reads as one written with LF; any other is quoted as a character that could deceive.
pub fn page_lines(text: &str, out: &mut Vec<String>) {
    for line in text.split('\n') {
        out.push(page_line(line.strip_suffix('\r').unwrap_or(line)));
    }
}

/// A number of the extension's as a line says it: whole where it is, else as it came (a zoom of
/// 133.3 %, a scale of 1.5018).
pub fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// What a style sheet's parser did not understand (`page.css@1`'s `dropped`), each quoted.
pub fn not_understood(dropped: &[String]) -> String {
    let each: Vec<String> = dropped.iter().map(|d| quoted(d)).collect();
    each.join(", ")
}

/// A time of Firefox's (milliseconds since the epoch) to the minute, in UTC: `2026-01-01 12:00
/// UTC`.
pub fn when(ms: i64) -> String {
    let t = std::time::UNIX_EPOCH
        + std::time::Duration::from_millis(u64::try_from(ms).unwrap_or_default());
    crate::desktop::read::when(&crate::jobs::iso(t))
}

/// Page text's lines into `out` (`page_line` each), the first after `first`, the others after
/// `rest`.
fn text_lines(text: &str, first: &str, rest: &str, out: &mut Vec<String>) {
    for (i, line) in text.split('\n').enumerate() {
        let lead = if i == 0 { first } else { rest };
        out.push(format!("{lead}{}", page_line(line)));
    }
}

/// The blocks' lines into `out`: a run of withheld ones (one frame's) as one [`WITHHELD`] line.
pub fn blocks(bs: &[Block], out: &mut Vec<String>) {
    let mut last: Option<Option<i64>> = None;
    for b in bs {
        if b.withheld {
            if last != Some(b.frame) {
                out.push(WITHHELD.to_owned());
            }
            last = Some(b.frame);
            continue;
        }
        last = None;
        block(b, out);
    }
}

/// One block's lines into `out`.
fn block(b: &Block, out: &mut Vec<String>) {
    let Some(text) = &b.text else {
        out.push(format!(
            "({} could not be read here: {})",
            frame_name(b.origin.as_deref(), b.title.as_deref()),
            unread_why(b.unread.as_deref().unwrap_or_default())
        ));
        return;
    };
    if let Some(level) = b.heading {
        let hashes = "#".repeat(usize::try_from(level.max(1)).unwrap_or(1));
        let words: Vec<String> = text.split('\n').map(page_line).collect();
        let at = b.id.map(|r| format!(" (ref {r})")).unwrap_or_default();
        out.push(format!("{hashes} {}{at}", words.join(" ")));
    } else if b.item {
        text_lines(text, "- ", "  ", out);
    } else {
        text_lines(text, "", "", out);
    }
}

/// The line about the frames that could not be read, if any: counted, and each named with its
/// reason; one of a site Kai does not reach by that only.
pub fn frames_line(f: &Frames) -> Option<String> {
    let named = &f.unread_frames;
    let n = f.unread.max(named.len() as u64);
    if n == 0 {
        return None;
    }
    let count = if n == 1 {
        "1 frame of the page could not be read".to_owned()
    } else {
        format!("{n} frames of the page could not be read")
    };
    let mut names: Vec<String> = named
        .iter()
        .map(|u| {
            if u.withheld {
                "a frame of a site Kai does not reach".to_owned()
            } else {
                format!(
                    "{} ({})",
                    frame_name(u.origin.as_deref(), u.title.as_deref()),
                    unread_why(&u.code)
                )
            }
        })
        .collect();
    let rest = n - named.len() as u64;
    if rest > 0 {
        names.push(format!("{rest} more Kai for Firefox does not name"));
    }
    Some(format!("{count}: {}", names.join("; ")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn control_of(v: Value) -> String {
        control(&serde_json::from_value(v).unwrap(), true)
    }

    #[test]
    fn a_control_says_its_value_and_states_and_never_a_masked_value() {
        assert_eq!(
            control_of(json!({"ref": 41, "role": "textbox", "name": "E-mail",
                "value": "ada@shop", "required": true, "invalid": true,
                "error": "Enter an address like ada@shop.example.",
                "description": "We send receipts here.", "label": "Your e-mail"})),
            "41 textbox \"E-mail\" = \"ada@shop\" (required, invalid: \"Enter an address like \
             ada@shop.example.\", label \"Your e-mail\", description \"We send receipts here.\")"
        );
        assert_eq!(
            control_of(json!({"ref": 42, "role": "textbox", "name": "Password",
                "masked": true, "length": 12})),
            "42 textbox \"Password\": password field, filled (12 characters)"
        );
        assert_eq!(
            control_of(json!({"ref": 43, "role": "textbox", "name": "Card number",
                "masked": true, "length": 0, "required": true})),
            "43 textbox \"Card number\": password field, empty (required)"
        );
        assert_eq!(
            control_of(json!({"ref": 44, "role": "checkbox", "name": "Gift wrap",
                "checked": "mixed", "disabled": true})),
            "44 checkbox \"Gift wrap\" (partly checked, disabled)"
        );
        assert_eq!(
            control_of(
                json!({"ref": 45, "role": "slider", "name": "Volume", "value": "40",
                "min": "0", "max": "100"})
            ),
            "45 slider \"Volume\" = \"40\" (from \"0\" to \"100\")"
        );
        assert_eq!(
            control_of(json!({"ref": 46, "role": "link", "name": "Help",
                "url": "https://shop.example/help", "in_view": true})),
            "46 link \"Help\" https://shop.example/help (in view)"
        );
        assert_eq!(
            control_of(
                json!({"ref": 47, "role": "button", "name": "", "expanded": false,
                "pressed": false, "inert": true})
            ),
            "47 button (not pressed, collapsed, cannot be used now)"
        );
        assert_eq!(
            control_of(
                json!({"ref": 48, "role": "radio", "name": "Express", "checked": true,
                "group": "Shipping", "in_view": false})
            ),
            "48 radio \"Express\" (checked, in \"Shipping\")"
        );
    }

    #[test]
    fn numbers_read_whole_where_they_are_and_times_in_utc() {
        assert_eq!(number(150.0), "150");
        assert_eq!(number(1.5018), "1.5018");
        assert_eq!(when(1_767_225_600_000), "2026-01-01 00:00 UTC");
        let mut out = Vec::new();
        page_lines("<p>a</p>\r\n<p>b\rc</p>\n", &mut out);
        assert_eq!(out, ["<p>a</p>", "\"<p>b\\rc</p>\"", ""]);
    }

    #[test]
    fn blocks_read_as_page_text_and_frames_as_where_they_are() {
        let bs: Vec<Block> = serde_json::from_value(json!([
            {"text": "Lamps and tents", "heading": 1, "ref": 5},
            {"text": "Gear for long evenings outside.\nOpen daily."},
            {"text": "Two people, three seasons.", "item": true},
            {"text": "Tent\t2.9 kg"},
            {"text": "shop.example uses cookies.", "frame": 17179869185u64,
             "origin": "https://consent.example"},
            {"frame": 17179869186u64, "origin": "https://pay.example:8443",
             "unread": "content_timeout", "title": "Payment"},
            {"frame": 7, "withheld": true},
            {"frame": 7, "withheld": true},
            {"frame": 8, "withheld": true},
            {"frame": 9, "origin": null, "unread": "restricted_domain", "title": ""},
            {"text": "fine\u{202e}txt"},
        ]))
        .unwrap();
        let mut out = Vec::new();
        blocks(&bs, &mut out);
        assert_eq!(
            out,
            [
                "# Lamps and tents (ref 5)",
                "Gear for long evenings outside.",
                "Open daily.",
                "- Two people, three seasons.",
                "Tent\t2.9 kg",
                "shop.example uses cookies.",
                "(pay.example:8443 \"Payment\" could not be read here: it did not answer within \
                 Kai for Firefox's setting content_ms)",
                WITHHELD,
                WITHHELD,
                "(a frame without a title could not be read here: Firefox keeps extensions off \
                 its site)",
                "\"fine\\u202etxt\"",
            ]
        );
        let f: Frames = serde_json::from_value(json!({"read": 2, "unread": 4, "unread_frames": [
            {"frame": 3, "origin": "https://consent.example", "code": "site_permission",
             "title": ""},
            {"frame": 4, "origin": null, "code": "no_content", "title": "Ad"},
            {"frame": 5, "withheld": true}]}))
        .unwrap();
        assert_eq!(
            frames_line(&f).unwrap(),
            "4 frames of the page could not be read: consent.example (the user took Kai for \
             Firefox's access to its site); frame \"Ad\" (Kai for Firefox cannot run in it); a \
             frame of a site Kai does not reach; 1 more Kai for Firefox does not name"
        );
        assert_eq!(frames_line(&Frames::default()), None);
    }
}
