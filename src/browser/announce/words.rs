//! What an event says (kai-firefox design §7.2; the contract's Events, "Page text"): the model's
//! line for `browser_context` (English, page text quoted), and Kai's sentence, in English and in
//! German (`crate::voice::speak::Words`; the speech picks Kai's reply language).
//!
//! - **The page's text is always introduced as the page's, and comes last.** Kai's own words come
//!   first ("A page has loaded; its title says: …", "The page says: …"), and once the page's text
//!   begins nothing of Kai's follows it, so nothing a page controls can come out as Kai's own
//!   sentence. A name the page controls is quoted inside the framing with the page's text after it
//!   ("The page says, at the field “E-mail”: invalid address", the contract's), or comes last
//!   ("Focus on a button, the page says: Back"); a role is said in Kai's words, an unknown role as
//!   "element", never as the page wrote it. A tab's address the page opened is page text too: its
//!   site, introduced as the page's.
//! - **A live region's place** (the contract's live `name`, kai-firefox WP10b): its name, an
//!   `<output>`'s controls' names or its labelled ancestor's, quoted inside the framing ("The page
//!   says, at “Tip for the courier”: 3"), so a bare value is said with where it is.
//! - **Long page text**: past `browser.announce_max_chars` the text is spoken to the last word
//!   that fits, and Kai says so before it ("…, its first 600 of 2 413 characters: …"). A name
//!   longer than that is left out and the role said instead. `browser_context` has it all.
//! - **Not spoken:** a `navigated` (its `loaded` follows; a same-document navigation's title often
//!   changes later), kept for `browser_context`; `quiet_by_kai` marks what Kai's own action caused
//!   that its tool's answer says already (a page loaded, a focus, a tab opened).

pub mod asked;

use serde_json::{Map, Value};

use crate::browser::tools::lines::{self, Element, site};
use crate::host::confirm::{quoted, shown_text};
use crate::voice::speak::Words;

/// An event's fields as Kai reads them.
#[derive(Debug, Default)]
pub struct Event {
    pub seq: Option<u64>,
    /// When the extension learned of it, ms since the epoch.
    pub at: Option<i64>,
    pub tab: Option<i64>,
    pub title: String,
    pub url: Option<String>,
    pub focused: bool,
    pub by_kai: bool,
    pub assertive: bool,
    /// A `live` region's `atomic`.
    pub atomic: bool,
    /// A `live` event's region.
    pub region: Option<i64>,
    /// A `live` event's `name` (module doc).
    pub name: Option<String>,
    /// A `field_error`'s `regions`.
    pub regions: Vec<i64>,
    pub text: Option<String>,
    pub error: Option<String>,
    pub description: Option<String>,
}

/// Where an event's site rules look (§6.4).
#[derive(Debug, Clone)]
pub enum Site {
    /// A URL: its site's rule (an empty one the strictest).
    Url(String),
    /// A document's origin: `None` (opaque, unknown) the strictest rule.
    Origin(Option<String>),
}

/// What an event says (module doc).
pub struct Said {
    pub line: String,
    /// Kai's sentence; `None`: not spoken.
    pub spoken: Option<Words>,
    /// Not spoken when it comes of Kai's own action.
    pub quiet_by_kai: bool,
}

fn text_of(f: &Map<String, Value>, k: &str) -> Option<String> {
    f.get(k).and_then(Value::as_str).map(str::to_owned)
}

impl Event {
    pub fn read(kind: &str, f: &Map<String, Value>) -> Self {
        let flag = |k: &str| f.get(k).and_then(Value::as_bool).unwrap_or(false);
        let refs = |v: Option<&Value>| -> Vec<i64> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.get("ref").and_then(Value::as_i64))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            seq: f.get("seq").and_then(Value::as_u64),
            at: f.get("at").and_then(Value::as_i64),
            tab: f.get("tab").and_then(Value::as_i64),
            title: text_of(f, "title").unwrap_or_default(),
            url: text_of(f, "url"),
            focused: flag("focused"),
            by_kai: flag("by_kai"),
            assertive: f.get("politeness").and_then(Value::as_str) == Some("assertive"),
            atomic: flag("atomic"),
            region: (kind == "live")
                .then(|| f.get("region").and_then(|r| r.get("ref")))
                .flatten()
                .and_then(Value::as_i64),
            name: (kind == "live")
                .then(|| text_of(f, "name"))
                .flatten()
                .filter(|n| !n.trim().is_empty()),
            regions: if kind == "field_error" {
                refs(f.get("regions"))
            } else {
                Vec::new()
            },
            text: text_of(f, "text"),
            error: text_of(f, "error"),
            description: text_of(f, "description"),
        }
    }

    /// The URLs and origins whose site rules decide (§6.4, the contract's Events).
    pub fn sites(&self, kind: &str, f: &Map<String, Value>) -> Vec<Site> {
        let mut out = Vec::new();
        if kind == "download_done" {
            for k in ["url", "final_url"] {
                if let Some(u) = text_of(f, k) {
                    out.push(Site::Url(u));
                }
            }
            let referrer = text_of(f, "referrer").filter(|r| !r.trim().is_empty());
            if let Some(r) = &referrer {
                out.push(Site::Url(r.clone()));
            }
            match f.get("source").and_then(Value::as_object) {
                Some(src) => {
                    // The tab's URL now (none when it closed meanwhile), and the document that
                    // began the navigation (an opaque or unknown one the strictest rule).
                    if let Some(u) = text_of(src, "url") {
                        out.push(Site::Url(u));
                    }
                    out.push(Site::Origin(text_of(src, "origin")));
                }
                // Neither source nor referrer names a page: the strictest rule.
                None if referrer.is_none() => out.push(Site::Url(String::new())),
                None => {}
            }
            return out;
        }
        out.push(Site::Url(self.url.clone().unwrap_or_default()));
        out.push(Site::Origin(text_of(f, "origin")));
        if kind == "tab_opened" {
            out.push(Site::Url(text_of(f, "opened_url").unwrap_or_default()));
        }
        out
    }

    /// The line and the sentence of `kind` (module doc); `None` for a kind Kai does not know.
    pub fn said(&self, kind: &str, f: &Map<String, Value>, max: u64) -> Option<Said> {
        let elem = |k: &str| {
            f.get(k)
                .cloned()
                .and_then(|v| serde_json::from_value::<Element>(v).ok())
        };
        let said = |line: String, spoken: Option<Words>, quiet_by_kai: bool| {
            Some(Said {
                line,
                spoken,
                quiet_by_kai,
            })
        };
        let page = |t: &str| Page::of(t, max);
        match kind {
            "loaded" => {
                let spoken = if self.title.trim().is_empty() {
                    Words::new("A page has loaded.", "Eine Seite wurde geladen.")
                } else {
                    page(&self.title).after(
                        "A page has loaded; its title says",
                        "Eine Seite wurde geladen; ihr Titel lautet",
                    )
                };
                said(
                    format!("loaded {}", quoted(&self.title)),
                    Some(spoken),
                    true,
                )
            }
            "navigated" => {
                let same = f.get("same_document").and_then(Value::as_bool) == Some(true);
                let url = self.url.as_deref().unwrap_or_default();
                let line = if same {
                    format!("moved within the page to {}", shown_text(url))
                } else {
                    format!("went to {}", shown_text(url))
                };
                said(line, None, true)
            }
            "dialog" => {
                let open = f.get("open").and_then(Value::as_bool).unwrap_or(true);
                let d = elem("dialog").unwrap_or_default();
                let line = format!(
                    "dialog {}: {}",
                    if open { "opened" } else { "closed" },
                    lines::element(&d)
                );
                let alert = d.role == "alertdialog";
                let (en, de) = match (open, alert) {
                    (true, false) => ("A dialog opened", "Ein Dialog wurde geöffnet"),
                    (true, true) => ("An alert dialog opened", "Ein Warndialog wurde geöffnet"),
                    (false, false) => ("A dialog closed", "Ein Dialog wurde geschlossen"),
                    (false, true) => ("An alert dialog closed", "Ein Warndialog wurde geschlossen"),
                };
                let spoken = if open && !d.name.trim().is_empty() {
                    page(&d.name).after(
                        &format!("{en}, the page says"),
                        &format!("{de}, die Seite sagt"),
                    )
                } else {
                    Words::new(format!("{en}."), format!("{de}."))
                };
                said(line, Some(spoken), false)
            }
            "tab_opened" => {
                let to = text_of(f, "opened_url").unwrap_or_default();
                let new_window = f.get("new_window").and_then(Value::as_bool) == Some(true);
                let tab = f
                    .get("opened_tab")
                    .and_then(Value::as_i64)
                    .map(|t| format!(" {t}"))
                    .unwrap_or_default();
                let (what, en, de) = if new_window {
                    (
                        "window",
                        "The page opened a new window",
                        "Die Seite hat ein neues Fenster geöffnet",
                    )
                } else {
                    (
                        "tab",
                        "The page opened a new tab",
                        "Die Seite hat einen neuen Tab geöffnet",
                    )
                };
                let host = url::Url::parse(&to)
                    .ok()
                    .and_then(|u| site(Some(u.origin().ascii_serialization().as_str())));
                // The address is the page's: its site said as the page's (module doc).
                let spoken = match host {
                    Some(h) => page(&h).after(
                        &format!("{en}; the page gives its address on"),
                        &format!("{de}; als Ziel nennt die Seite"),
                    ),
                    None => Words::new(format!("{en}."), format!("{de}.")),
                };
                said(
                    format!("the page opened {what}{tab}: {}", shown_text(&to)),
                    Some(spoken),
                    true,
                )
            }
            "live" => {
                let text = self.text.clone().unwrap_or_default();
                let r = elem("region").unwrap_or_default();
                let at = match &self.name {
                    Some(n) => format!(", at {}", quoted(n)),
                    None => String::new(),
                };
                let line = format!(
                    "the page says ({}{at}): {}",
                    lines::element(&r),
                    quoted(&text)
                );
                let place = self.name.as_deref().and_then(|n| named(n, max));
                let spoken = (!text.trim().is_empty()).then(|| match place {
                    Some(n) => {
                        let n = clean_name(n);
                        page(&text).after(
                            &format!("The page says, at “{n}”"),
                            &format!("Die Seite sagt, bei „{n}“"),
                        )
                    }
                    None => page(&text).after("The page says", "Die Seite sagt"),
                });
                said(line, spoken, false)
            }
            "field_error" => {
                let field = elem("field").unwrap_or_default();
                let mut line = format!("error at {}", lines::element(&field));
                for (label, t) in [("", &self.error), ("description ", &self.description)] {
                    if let Some(t) = t {
                        line.push_str(&format!(": {label}{}", quoted(t)));
                    }
                }
                said(line, Some(field_error(&field, self, max)), false)
            }
            "download_done" => {
                let file = text_of(f, "file").unwrap_or_default();
                let path = text_of(f, "path").unwrap_or_default();
                let size = f
                    .get("size")
                    .and_then(Value::as_u64)
                    .map(|n| format!(", {n} bytes"))
                    .unwrap_or_default();
                let line = format!(
                    "download finished: {} at {}{size}",
                    quoted(&file),
                    shown_text(&path)
                );
                let spoken = if file.trim().is_empty() {
                    Words::new("A download finished.", "Ein Download ist abgeschlossen.")
                } else {
                    page(&file).after(
                        "A download finished; the page names the file",
                        "Ein Download ist abgeschlossen; die Seite nennt die Datei",
                    )
                };
                said(line, Some(spoken), false)
            }
            "focus" => {
                let e = elem("element").unwrap_or_default();
                let r = Role::of(&e.role);
                let (en, de) = (
                    format!("Focus on {} {}", r.a(), r.en),
                    format!("Fokus auf {} {}", r.einem(), r.de),
                );
                let spoken = match named(&e.name, max) {
                    Some(name) => page(name).after(
                        &format!("{en}, the page says"),
                        &format!("{de}, die Seite sagt"),
                    ),
                    None => Words::new(format!("{en}."), format!("{de}.")),
                };
                said(
                    format!("focus moved to {}", lines::element(&e)),
                    Some(spoken),
                    true,
                )
            }
            _ => None,
        }
    }
}

/// A field's error (module doc): the error and the description it has (the description only
/// where it differs), the field's name quoted inside the framing.
fn field_error(field: &Element, ev: &Event, max: u64) -> Words {
    let r = Role::field(&field.role);
    let says: Vec<&str> = [&ev.error, &ev.description]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .filter(|t| !t.trim().is_empty())
        .collect();
    let name = named(&field.name, max);
    if says.is_empty() {
        let (en, de) = (
            format!("The page marks {} {} as invalid", r.a(), r.en),
            format!("Die Seite markiert {} {} als ungültig", r.ein(), r.de),
        );
        return match name {
            Some(n) => Page::of(n, max).after(
                &format!("{en}; the page names it"),
                &format!("{de}; die Seite nennt {}", r.es()),
            ),
            None => Words::new(format!("{en}."), format!("{de}.")),
        };
    }
    let text = Page::of(&says.join("; "), max);
    match name {
        Some(n) => {
            let n = clean_name(n);
            text.after(
                &format!("The page says, at the {} “{n}”", r.en),
                &format!("Die Seite sagt, {} {} „{n}“", r.zum(), r.de),
            )
        }
        None => text.after(
            &format!("Error at {} {}, the page says", r.a(), r.en),
            &format!("Fehler bei {} {}, die Seite sagt", r.einem(), r.de),
        ),
    }
}

/// A name to say: none when empty or longer than `max` (module doc).
fn named(name: &str, max: u64) -> Option<&str> {
    let n = name.trim();
    let long = max > 0 && n.chars().count() as u64 > max;
    (!n.is_empty() && !long).then_some(n)
}

/// A name inside Kai's quotation marks: the marks it might carry, and line breaks, taken out.
fn clean_name(n: &str) -> String {
    n.chars()
        .map(|c| match c {
            '“' | '”' | '„' | '"' | '\n' | '\r' => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Page text to speak (module doc), at most `max` characters (0: all of it).
struct Page {
    text: String,
    /// Cut: the characters spoken, and how many there were.
    cut: Option<(u64, u64)>,
}

impl Page {
    fn of(text: &str, max: u64) -> Self {
        let t = text.trim();
        let n = t.chars().count() as u64;
        if max == 0 || n <= max {
            return Self {
                text: t.to_owned(),
                cut: None,
            };
        }
        let cut: String = t.chars().take(max as usize).collect();
        let head = match cut.rfind(char::is_whitespace) {
            Some(i) if i > 0 => cut[..i].trim_end(),
            _ => cut.as_str(),
        };
        Self {
            text: head.to_owned(),
            cut: Some((head.chars().count() as u64, n)),
        }
    }

    /// Kai's words `en` and `de`, then the page's text, with what was left out said first.
    fn after(&self, en: &str, de: &str) -> Words {
        match self.cut {
            None => Words::new(
                format!("{en}: {}", self.text),
                format!("{de}: {}", self.text),
            ),
            Some((k, n)) => Words::new(
                format!("{en}, its first {k} of {n} characters: {}", self.text),
                format!("{de}, die ersten {k} von {n} Zeichen: {}", self.text),
            ),
        }
    }
}

/// German grammatical gender, for the articles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gender {
    M,
    F,
    N,
}

/// A role in Kai's words (module doc): the page's own word is never spoken.
struct Role {
    en: &'static str,
    de: &'static str,
    g: Gender,
}

impl Role {
    fn of(role: &str) -> Self {
        let (en, de, g) = match role {
            "textbox" => ("text field", "Textfeld", Gender::N),
            "searchbox" => ("search field", "Suchfeld", Gender::N),
            "combobox" => ("combo box", "Auswahlfeld", Gender::N),
            "checkbox" => ("checkbox", "Kontrollkästchen", Gender::N),
            "radio" => ("radio button", "Optionsfeld", Gender::N),
            "button" => ("button", "Schaltfläche", Gender::F),
            "link" => ("link", "Link", Gender::M),
            "listbox" => ("list box", "Listenfeld", Gender::N),
            "option" => ("option", "Option", Gender::F),
            "slider" => ("slider", "Schieberegler", Gender::M),
            "spinbutton" => ("number field", "Zahlenfeld", Gender::N),
            "switch" => ("switch", "Schalter", Gender::M),
            "tab" => ("tab", "Reiter", Gender::M),
            "menuitem" | "menuitemcheckbox" | "menuitemradio" => {
                ("menu item", "Menüeintrag", Gender::M)
            }
            "heading" => ("heading", "Überschrift", Gender::F),
            "img" | "image" => ("image", "Bild", Gender::N),
            "dialog" => ("dialog", "Dialog", Gender::M),
            "alertdialog" => ("alert dialog", "Warndialog", Gender::M),
            "row" => ("row", "Zeile", Gender::F),
            "cell" | "gridcell" => ("cell", "Zelle", Gender::F),
            "treeitem" => ("tree item", "Baumeintrag", Gender::M),
            "region" => ("region", "Bereich", Gender::M),
            // The contract's role words for inputs HTML-AAM gives no ARIA role.
            "file" => ("file field", "Dateifeld", Gender::N),
            "date" => ("date field", "Datumsfeld", Gender::N),
            "time" => ("time field", "Zeitfeld", Gender::N),
            "datetime" => ("date and time field", "Datums- und Zeitfeld", Gender::N),
            "month" => ("month field", "Monatsfeld", Gender::N),
            "week" => ("week field", "Wochenfeld", Gender::N),
            "color" => ("colour field", "Farbfeld", Gender::N),
            "status" => ("status", "Statusmeldung", Gender::F),
            "group" => ("group", "Gruppe", Gender::F),
            _ => ("element", "Element", Gender::N),
        };
        Self { en, de, g }
    }

    /// A field's role as a field's error says it: the contract's "field" for text input.
    fn field(role: &str) -> Self {
        match role {
            "textbox" | "searchbox" | "spinbutton" | "" => Self {
                en: "field",
                de: "Feld",
                g: Gender::N,
            },
            other => Self::of(other),
        }
    }

    fn a(&self) -> &'static str {
        if self.en.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        }
    }

    /// The German indefinite article, accusative.
    fn ein(&self) -> &'static str {
        match self.g {
            Gender::M => "einen",
            Gender::F => "eine",
            Gender::N => "ein",
        }
    }

    /// The German indefinite article, dative.
    fn einem(&self) -> &'static str {
        match self.g {
            Gender::F => "einer",
            Gender::M | Gender::N => "einem",
        }
    }

    /// "zu" with the German definite article, dative.
    fn zum(&self) -> &'static str {
        match self.g {
            Gender::F => "zur",
            Gender::M | Gender::N => "zum",
        }
    }

    /// The German pronoun, accusative.
    fn es(&self) -> &'static str {
        match self.g {
            Gender::M => "ihn",
            Gender::F => "sie",
            Gender::N => "es",
        }
    }

    /// The German definite article, accusative.
    fn den(&self) -> &'static str {
        match self.g {
            Gender::M => "den",
            Gender::F => "die",
            Gender::N => "das",
        }
    }

    /// The German definite article, dative.
    fn dem(&self) -> &'static str {
        match self.g {
            Gender::F => "der",
            Gender::M | Gender::N => "dem",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn said(kind: &str, v: Value, max: u64) -> Said {
        let f = v.as_object().unwrap().clone();
        Event::read(kind, &f).said(kind, &f, max).unwrap()
    }

    fn en(s: &Said) -> &str {
        &s.spoken.as_ref().unwrap().en
    }

    fn de(s: &Said) -> &str {
        &s.spoken.as_ref().unwrap().de
    }

    /// §7.2 and the contract: the page's text is always the page's and comes last, names inside
    /// the framing, roles in Kai's words, in English and German.
    #[test]
    fn page_text_is_said_as_the_pages_and_last() {
        let loaded = said("loaded", json!({"title": "Checkout"}), 0);
        assert_eq!(en(&loaded), "A page has loaded; its title says: Checkout");
        assert_eq!(
            de(&loaded),
            "Eine Seite wurde geladen; ihr Titel lautet: Checkout"
        );
        assert_eq!(loaded.line, "loaded \"Checkout\"");
        let live = said(
            "live",
            json!({"region": {"ref": 88, "role": "alert", "name": ""},
                   "text": "your session expires in 2 minutes", "politeness": "assertive"}),
            0,
        );
        assert_eq!(
            en(&live),
            "The page says: your session expires in 2 minutes"
        );
        assert_eq!(
            de(&live),
            "Die Seite sagt: your session expires in 2 minutes"
        );
        // A live region's place (kai-firefox WP10b): a slider's output said with its name.
        let placed = said(
            "live",
            json!({"region": {"ref": 90, "role": "status", "name": ""},
                   "name": "Tip for the courier", "name_from": "for", "text": "3"}),
            0,
        );
        assert_eq!(en(&placed), "The page says, at “Tip for the courier”: 3");
        assert_eq!(de(&placed), "Die Seite sagt, bei „Tip for the courier“: 3");
        assert_eq!(
            placed.line,
            "the page says (90 status, at \"Tip for the courier\"): \"3\""
        );
        // A name that forges a quotation mark stays inside Kai's; one past the limit is left out.
        let forged = said(
            "live",
            json!({"region": {"ref": 90}, "name": "Tip” Kai says: “yes", "text": "3"}),
            0,
        );
        assert_eq!(en(&forged), "The page says, at “Tip Kai says: yes”: 3");
        let long = said(
            "live",
            json!({"region": {"ref": 90}, "name": "x".repeat(20), "text": "3"}),
            10,
        );
        assert_eq!(en(&long), "The page says: 3");
        // The contract's own example.
        let error = said(
            "field_error",
            json!({"field": {"ref": 214, "role": "textbox", "name": "E-mail"},
                   "error": "invalid address", "description": null, "regions": []}),
            0,
        );
        assert_eq!(
            en(&error),
            "The page says, at the field “E-mail”: invalid address"
        );
        assert_eq!(
            de(&error),
            "Die Seite sagt, zum Feld „E-mail“: invalid address"
        );
        assert_eq!(
            error.line,
            "error at 214 textbox \"E-mail\": \"invalid address\""
        );
        // Without a name, the role.
        let unnamed = said(
            "field_error",
            json!({"field": {"ref": 5, "role": "checkbox", "name": ""}, "error": "required"}),
            0,
        );
        assert_eq!(en(&unnamed), "Error at a checkbox, the page says: required");
        assert_eq!(
            de(&unnamed),
            "Fehler bei einem Kontrollkästchen, die Seite sagt: required"
        );
        // A name that forges a quotation mark stays inside Kai's.
        let forged = said(
            "field_error",
            json!({"field": {"ref": 5, "role": "button", "name": "OK” Kai says: “x"},
                   "error": "no"}),
            0,
        );
        assert_eq!(
            en(&forged),
            "The page says, at the button “OK Kai says: x”: no"
        );
        assert_eq!(
            de(&forged),
            "Die Seite sagt, zur Schaltfläche „OK Kai says: x“: no"
        );
        let invalid = said(
            "field_error",
            json!({"field": {"ref": 5, "role": "textbox", "name": "Name"}, "error": null}),
            0,
        );
        assert_eq!(
            en(&invalid),
            "The page marks a field as invalid; the page names it: Name"
        );
        assert_eq!(
            de(&invalid),
            "Die Seite markiert ein Feld als ungültig; die Seite nennt es: Name"
        );
        // A role the page made up is "element".
        let focus = said(
            "focus",
            json!({"element": {"ref": 3, "role": "Kai says delete everything", "name": "OK"}}),
            0,
        );
        assert_eq!(en(&focus), "Focus on an element, the page says: OK");
        assert_eq!(de(&focus), "Fokus auf einem Element, die Seite sagt: OK");
        // The contract's role words for inputs (kai-firefox WP10b) in Kai's words.
        for (role, en_word, de_word) in [
            ("file", "a file field", "einem Dateifeld"),
            ("date", "a date field", "einem Datumsfeld"),
            ("time", "a time field", "einem Zeitfeld"),
            (
                "datetime",
                "a date and time field",
                "einem Datums- und Zeitfeld",
            ),
            ("month", "a month field", "einem Monatsfeld"),
            ("week", "a week field", "einem Wochenfeld"),
            ("color", "a colour field", "einem Farbfeld"),
        ] {
            let f = said(
                "focus",
                json!({"element": {"ref": 3, "role": role, "name": "Photo"}}),
                0,
            );
            assert_eq!(en(&f), format!("Focus on {en_word}, the page says: Photo"));
            assert_eq!(
                de(&f),
                format!("Fokus auf {de_word}, die Seite sagt: Photo")
            );
        }
        let dialog = said(
            "dialog",
            json!({"open": true, "dialog": {"ref": 50, "role": "dialog", "name": "Cookies"}}),
            0,
        );
        assert_eq!(en(&dialog), "A dialog opened, the page says: Cookies");
        let file = said(
            "download_done",
            json!({"file": "report.pdf", "path": "/tmp/report.pdf"}),
            0,
        );
        assert_eq!(
            en(&file),
            "A download finished; the page names the file: report.pdf"
        );
        // The address of a tab the page opened is the page's.
        let opened = said(
            "tab_opened",
            json!({"opened_tab": 15, "opened_url": "https://evil.example/x", "new_window": false}),
            0,
        );
        assert_eq!(
            en(&opened),
            "The page opened a new tab; the page gives its address on: evil.example"
        );
        assert_eq!(
            de(&opened),
            "Die Seite hat einen neuen Tab geöffnet; als Ziel nennt die Seite: evil.example"
        );
        let nav = said(
            "navigated",
            json!({"url": "https://shop.example/", "same_document": false}),
            0,
        );
        assert!(nav.spoken.is_none(), "its loaded follows");
    }

    #[test]
    fn long_page_text_says_how_much_before_it() {
        let log = "word ".repeat(400);
        let s = said(
            "live",
            json!({"region": {"ref": 1, "role": "log"}, "text": log}),
            30,
        );
        // "word word …" up to 30 characters, cut at the last whole word: 29 of 1999.
        let spoken = en(&s);
        assert!(
            spoken.starts_with("The page says, its first 29 of 1999 characters: word word"),
            "{spoken}"
        );
        assert!(
            spoken.ends_with("word"),
            "the page's text comes last: {spoken}"
        );
        assert!(de(&s).starts_with("Die Seite sagt, die ersten 29 von 1999 Zeichen: word"));
        // The model's line has it whole; 0 says all of it.
        assert_eq!(s.line.matches("word").count(), 400);
        let all = said("live", json!({"region": {"ref": 1}, "text": log}), 0);
        assert!(!en(&all).contains("characters"));
        // A name longer than the limit is left out: the role is said.
        let long = said(
            "focus",
            json!({"element": {"ref": 3, "role": "button", "name": "x".repeat(40)}}),
            30,
        );
        assert_eq!(en(&long), "Focus on a button.");
    }

    /// The contract's download rule: url, final_url, referrer, source.url and source.origin are
    /// judged; with neither source nor referrer, the strictest rule (an empty URL).
    #[test]
    fn a_downloads_sites() {
        let sites = |v: Value| {
            let f = v.as_object().unwrap().clone();
            Event::read("download_done", &f)
                .sites("download_done", &f)
                .into_iter()
                .map(|s| match s {
                    Site::Url(u) => format!("url {u}"),
                    Site::Origin(o) => format!("origin {}", o.unwrap_or_else(|| "null".into())),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sites(
                json!({"url": "https://cdn.example/a", "final_url": "https://cdn.example/b",
                         "referrer": null,
                         "source": {"tab": 1, "url": "https://bank.example/", "frame": 0,
                                    "origin": "https://shop.example"}})
            ),
            [
                "url https://cdn.example/a",
                "url https://cdn.example/b",
                "url https://bank.example/",
                "origin https://shop.example"
            ],
            "a source without a referrer: no strictest rule"
        );
        assert_eq!(
            sites(json!({"url": "https://cdn.example/a", "referrer": null, "source": null})),
            ["url https://cdn.example/a", "url "],
            "nothing names its page: the strictest rule"
        );
        assert_eq!(
            sites(
                json!({"url": "https://cdn.example/a", "referrer": "https://shop.example/",
                         "source": {"tab": 1, "url": null, "origin": null}})
            ),
            [
                "url https://cdn.example/a",
                "url https://shop.example/",
                "origin null"
            ],
            "a closed source tab: its document's origin still judged, an opaque one strictly"
        );
    }
}
