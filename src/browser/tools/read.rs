//! The reading tools of a page's structure (kai-firefox design §5.3: `page_outline`,
//! `page_controls`, `page_links`, `page_fields`, `page_selection`, `page_find`) and the ones that
//! show the user something (§5.4: `hints_show`, `hints_hide`, `element_point`), over the
//! contract's operations of the same names. Compact lines (K22): the tab's title and URL first
//! (`crate::browser::tabs::head`), an element as its ref, role and quoted name (`super::lines`),
//! the frames Kai for Firefox could not read at the end, nothing cut. What the lines leave out is
//! said in the tools' descriptions, not in every answer.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::lines::{self, Control, Element, Frames, element, frames_line, listed, page_line, site};
use super::{Answer, Inner, answered, ask_page, failure, read, send, target};
use crate::browser::sites::SiteRules;
use crate::browser::tabs::{Tab, head, or_default};
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const OUTLINE: &str = "page.outline@1";
pub const CONTROLS: &str = "page.controls@1";
pub const LINKS: &str = "page.links@1";
pub const FIELDS: &str = "page.fields@1";
pub const SELECTION: &str = "page.selection@1";
pub const FIND: &str = "page.find@1";
pub const HINTS_SHOW: &str = "hints.show@1";
pub const HINTS_HIDE: &str = "hints.hide@1";
pub const POINT: &str = "element.point@1";

pub(super) fn reply(r: Result<String, String>) -> Reply {
    match r {
        Ok(text) => Reply::text(text),
        Err(e) => Reply::error(e),
    }
}

/// Runs `op` on what the call was prepared for and words its answer with `say`.
pub(super) fn page_tool(
    inner: Arc<Inner>,
    call: Call,
    op: &'static str,
    extra: Map<String, Value>,
    say: fn(&Answer, &SiteRules) -> Result<Vec<String>, String>,
) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let a = ask_page(&inner, &call, op, extra).await?;
                Ok(say(&a, &inner.browser().sites)?.join("\n"))
            }
            .await,
        )
    })
}

/// "a frame", or "a frame of <site>" where its origin names one.
fn a_frame(origin: Option<&str>) -> String {
    match site(origin) {
        Some(s) => format!("a frame of {}", shown_text(&s)),
        None => "a frame".to_owned(),
    }
}

/// An open modal dialog (the contract's Modal): an Element and the frame whose document it makes
/// inert (0: the top document), or one in a frame of a site Kai does not reach (`withheld`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Modal {
    #[serde(flatten)]
    pub element: Element,
    pub frame: Option<i64>,
    pub origin: Option<String>,
    pub withheld: bool,
}

impl Modal {
    /// It restricts a child frame's document only.
    fn in_frame(&self) -> bool {
        self.withheld || self.frame.is_some_and(|f| f != 0)
    }

    /// Where it is, as the first line says it: ` in a frame of <site>`, or nothing.
    fn place(&self) -> String {
        if self.withheld {
            " in a frame of a site Kai does not reach".to_owned()
        } else if self.in_frame() {
            format!(" in {}", a_frame(self.origin.as_deref()))
        } else {
            String::new()
        }
    }

    /// The dialog itself, after a colon; nothing for a withheld one.
    fn named(&self) -> String {
        if self.withheld {
            String::new()
        } else {
            format!(": {}", element(&self.element))
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Outline {
    tab: Tab,
    modal: Option<Modal>,
    items: Vec<Item>,
    frames: Frames,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Item {
    #[serde(flatten)]
    element: Element,
    here: bool,
    frame: Option<i64>,
    withheld: bool,
}

/// `page_outline`.
pub fn outline(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, OUTLINE, Map::new(), |a, sites| {
        let o: Outline = read(OUTLINE, a.value.clone())?;
        let mut first = head(&o.tab, sites);
        if let Some(m) = &o.modal {
            first.push_str(&format!(
                ": a modal dialog is open{}{}; the outline is of the whole page",
                m.place(),
                m.named()
            ));
        }
        let mut out = vec![first];
        if o.items.is_empty() {
            out.push("no landmarks or headings".into());
        }
        out.extend(listed(
            &o.items,
            |i| i.withheld.then_some(i.frame),
            |i| {
                let mut line = element(&i.element);
                if i.here {
                    line.push_str(" (the user is reading here)");
                }
                line
            },
        ));
        out.extend(frames_line(&o.frames));
        Ok(out)
    })
}

/// `page.controls@1`'s answer, and `hints.show@1`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Controls {
    tab: Tab,
    modal: Option<Modal>,
    controls: Vec<Control>,
    frames: Frames,
    undrawn: Vec<Undrawn>,
}

/// A frame whose hint labels could not be drawn (`hints.show@1`'s `undrawn`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Undrawn {
    origin: Option<String>,
    #[serde(deserialize_with = "or_default")]
    code: String,
    labels: u64,
}

/// The lines of the controls in view: with a modal dialog open, the first line says that only its
/// controls are listed, of its own document only when it is in a frame. `then` comes after the
/// first line; `hint` as `lines::control`.
fn controls_lines(
    c: &Controls,
    sites: &SiteRules,
    then: Option<String>,
    hint: bool,
) -> Vec<String> {
    let mut first = head(&c.tab, sites);
    if let Some(m) = &c.modal {
        let only = if m.in_frame() {
            format!(
                "a modal dialog is open{}, so of that frame only the dialog's controls are \
                 listed, of the rest of the page all in view",
                m.place()
            )
        } else {
            "a modal dialog is open, so only its controls are listed".to_owned()
        };
        first.push_str(&format!(": {only}{}", m.named()));
    }
    let mut out = vec![first];
    out.extend(then);
    if c.controls.is_empty() {
        out.push("no controls in view".into());
    }
    out.extend(lines::controls(&c.controls, hint));
    out.extend(frames_line(&c.frames));
    out
}

/// `page_controls`.
pub fn controls(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, CONTROLS, Map::new(), |a, sites| {
        let c: Controls = read(CONTROLS, a.value.clone())?;
        Ok(controls_lines(&c, sites, None, false))
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Whole {
    tab: Tab,
    modal: Option<Modal>,
    links: Vec<Control>,
    fields: Vec<Control>,
    frames: Frames,
}

/// `page_links` and `page_fields`: every one of the page, how many, and how many in view.
fn whole_lines(
    w: &Whole,
    list: &[Control],
    one: &str,
    many: &str,
    sites: &SiteRules,
) -> Vec<String> {
    let mut first = head(&w.tab, sites);
    if let Some(m) = &w.modal {
        let rest = if m.in_frame() {
            "the rest of that frame"
        } else {
            "the page outside it"
        };
        first.push_str(&format!(
            ": a modal dialog is open{}, so {rest} cannot be used now{}",
            m.place(),
            m.named()
        ));
    }
    let said: Vec<&Control> = list.iter().filter(|c| !c.withheld).collect();
    let in_view = said.iter().filter(|c| c.in_view == Some(true)).count();
    let count = match said.len() {
        0 => format!("no {many}"),
        1 => format!("1 {one}, {in_view} in view"),
        n => format!("{n} {many}, {in_view} of them in view"),
    };
    let mut out = vec![first, count];
    out.extend(lines::controls(list, false));
    out.extend(frames_line(&w.frames));
    out
}

/// `page_links`.
pub fn links(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, LINKS, Map::new(), |a, sites| {
        let w: Whole = read(LINKS, a.value.clone())?;
        Ok(whole_lines(&w, &w.links, "link", "links", sites))
    })
}

/// `page_fields`.
pub fn fields(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, FIELDS, Map::new(), |a, sites| {
        let w: Whole = read(FIELDS, a.value.clone())?;
        Ok(whole_lines(
            &w,
            &w.fields,
            "form field",
            "form fields",
            sites,
        ))
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Selected {
    tab: Tab,
    selection: Option<Selection>,
    frames: Frames,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Selection {
    text: Option<String>,
    length: Option<u64>,
    masked: bool,
    field: Option<Element>,
    frame: Option<i64>,
    origin: Option<String>,
    withheld: bool,
}

/// `n` characters.
fn characters(n: u64) -> String {
    if n == 1 {
        "1 character".to_owned()
    } else {
        format!("{n} characters")
    }
}

/// The selection's lines: how long it is and where, then its text; a password field's selection
/// in §5's words, never its text.
fn selection_lines(sel: &Selection, out: &mut Vec<String>) {
    if sel.withheld {
        out.push(
            "the selection is in a frame of a site Kai does not reach; it is not shown".into(),
        );
        return;
    }
    let mut at = String::new();
    if let Some(f) = &sel.field {
        at.push_str(&format!(" in {}", element(f)));
    }
    if sel.frame.is_some_and(|f| f != 0) {
        at.push_str(&format!(" in {}", a_frame(sel.origin.as_deref())));
    }
    if sel.masked {
        out.push(match sel.length {
            Some(n) => format!(
                "selection{at}: password field, {} selected, not shown",
                characters(n)
            ),
            None => "a password field has the focus; its selection is not shown".into(),
        });
        return;
    }
    let text = sel.text.as_deref().unwrap_or_default();
    let n = sel.length.unwrap_or_else(|| text.chars().count() as u64);
    out.push(format!("selection: {}{at}:", characters(n)));
    out.extend(text.split('\n').map(page_line));
}

/// `page_selection`: the selected text, whole, under a line that says how long it is and where.
pub fn selection(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, SELECTION, Map::new(), |a, sites| {
        let s: Selected = read(SELECTION, a.value.clone())?;
        let mut out = vec![head(&s.tab, sites)];
        match &s.selection {
            None => out.push("nothing is selected".into()),
            Some(sel) => selection_lines(sel, &mut out),
        }
        out.extend(frames_line(&s.frames));
        Ok(out)
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Found {
    tab: Tab,
    #[serde(deserialize_with = "or_default")]
    text: String,
    count: u64,
    highlighted: bool,
    matches: Vec<Match>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Match {
    #[serde(deserialize_with = "or_default")]
    text: String,
    element: Option<Element>,
    sentence: Option<String>,
    frame: Option<i64>,
    origin: Option<String>,
}

/// `page_find`: the number of matches, as the extension counts them (sensitive fields not
/// searched) less those in frames of sites Kai does not reach (`super::page::withhold`), then one
/// line per match: its element and its sentence.
pub fn find(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    let text = call.args.text("text").unwrap_or_default().to_owned();
    if text.trim().is_empty() {
        return Box::pin(async { Reply::error("page_find needs the text to find; it was empty") });
    }
    let mut extra = Map::new();
    extra.insert("text".into(), text.into());
    page_tool(inner, call, FIND, extra, |a, sites| {
        let f: Found = read(FIND, a.value.clone())?;
        let mut out = vec![head(&f.tab, sites)];
        let what = quoted(&f.text);
        let found = match f.count {
            0 => format!("no match for {what}"),
            1 => format!("1 match for {what}"),
            n => format!("{n} matches for {what}"),
        };
        out.push(match (f.count, f.highlighted) {
            (0, _) => found,
            (_, true) => format!("{found}, highlighted in the page:"),
            (_, false) => format!("{found}:"),
        });
        for m in &f.matches {
            let frame = if m.frame.is_some_and(|f| f != 0) {
                format!(" (in {})", a_frame(m.origin.as_deref()))
            } else {
                String::new()
            };
            out.push(match (&m.element, &m.sentence) {
                (Some(e), Some(s)) => format!("{}{frame}: {}", element(e), quoted(s)),
                (Some(e), None) => format!("{}{frame}", element(e)),
                _ => format!(
                    "a match Kai for Firefox could not place in the page{frame}: {}",
                    quoted(&m.text)
                ),
            });
        }
        Ok(out)
    })
}

/// `hints_show`: how many labels and where some could not be drawn, then the lines of
/// `page_controls`, each with the number its label shows where that is not its ref. An answer Kai
/// withholds (its page went to a `none` site while it ran) takes the labels off again.
pub fn hints_show(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let v = match send(&inner, &call, &t, HINTS_SHOW, Map::new()).await {
                    Ok(v) => v,
                    Err(e) => return Err(failure(&inner, &t.link, e, None, &call.cancel).await),
                };
                let tab = v.pointer("/tab/id").and_then(Value::as_i64);
                let a = match answered(&inner, &call, &t, v) {
                    Ok(a) => a,
                    Err(e) => return Err(take_off(&inner, &t.link, tab, e).await),
                };
                let c: Controls = read(HINTS_SHOW, a.value)?;
                let drawn = c.controls.iter().filter(|c| !c.withheld).count();
                let mut shown = match drawn {
                    1 => "hints on 1 control in view".to_owned(),
                    n => format!("hints on {n} controls in view"),
                };
                for u in &c.undrawn {
                    let place = site(u.origin.as_deref())
                        .map_or_else(|| "a frame".to_owned(), |s| shown_text(&s).into_owned());
                    let labels = match u.labels {
                        1 => "1 label".to_owned(),
                        n => format!("{n} labels"),
                    };
                    shown.push_str(&format!(
                        "; {labels} not drawn in {place} ({})",
                        lines::unread_why(&u.code)
                    ));
                }
                let sites = inner.browser().sites;
                Ok(controls_lines(&c, &sites, Some(shown), true).join("\n"))
            }
            .await,
        )
    })
}

/// A `hints.show@1` answer Kai withholds (`e`): the labels come off tab `tab` again (draft
/// decision), so a page Kai may not show keeps none of Kai's.
async fn take_off(
    inner: &Inner,
    link: &crate::browser::links::LinkInfo,
    tab: Option<i64>,
    e: String,
) -> String {
    let Some(id) = tab.filter(|_| link.ops.iter().any(|o| o == HINTS_HIDE)) else {
        return e;
    };
    let hidden = inner
        .links
        .op(
            link.id,
            HINTS_HIDE,
            json!({"tab": id}),
            &CancellationToken::new(),
        )
        .await;
    match hidden {
        Ok(_) => format!("{e}; Kai took the hint labels off that page again"),
        Err(_) => {
            format!("{e}; Kai could not take the hint labels off that page again (hints_hide does)")
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Hidden {
    tab: Tab,
    shown: bool,
    removed: u64,
}

/// `hints_hide`.
pub fn hints_hide(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, HINTS_HIDE, Map::new(), |a, sites| {
        let h: Hidden = read(HINTS_HIDE, a.value.clone())?;
        let labels = |n: u64| {
            if n == 1 {
                "1 label".to_owned()
            } else {
                format!("{n} labels")
            }
        };
        let done = match (h.shown, h.removed) {
            (true, n) => format!("hints hidden, {} removed", labels(n)),
            (false, 0) => "no hints were shown".to_owned(),
            (false, n) => format!("{} removed", labels(n)),
        };
        Ok(vec![format!("{}: {done}", head(&h.tab, sites))])
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Pointed {
    #[serde(flatten)]
    element: Element,
    tab: Tab,
    ms: u64,
    scrolled: bool,
    hidden: bool,
}

/// `element_point`: a ring for `browser.point_ms`.
pub fn point(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    let mut extra = Map::new();
    extra.insert("ms".into(), inner.browser().point_ms.into());
    page_tool(inner, call, POINT, extra, |a, sites| {
        let p: Pointed = read(POINT, a.value.clone())?;
        let what = element(&p.element);
        let done = if p.hidden {
            format!("{what}: the element is hidden; no ring")
        } else if p.scrolled {
            format!("scrolled {what} into view and ringed it for {} ms", p.ms)
        } else {
            format!("ringed {what} for {} ms", p.ms)
        };
        Ok(vec![format!("{}: {done}", head(&p.tab, sites))])
    })
}
