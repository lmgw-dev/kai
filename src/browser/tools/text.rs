//! The text tools (kai-firefox design §5.3): `page_text` (the article Readability finds, with what
//! it left out, else all the text; a PDF's text through poppler, `super::pdf`), `page_text_all`,
//! `page_text_visible`, `element_text` (a heading's whole section) and `table_read` (each cell
//! with its headers, as a screen reader announces it). Page text is said line by line under the
//! tab's title and URL (`super::lines::blocks`), whole.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::lines::{Block, Element, Frames, blocks, element, frames_line};
use super::read::{page_tool, reply};
use super::{Answer, Inner, answered, failure, read, send, target};
use crate::browser::links::OpError;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head, or_default};
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const TEXT: &str = "page.text@1";
pub const TEXT_ALL: &str = "page.text_all@1";
pub const TEXT_VISIBLE: &str = "page.text_visible@1";
pub const ELEMENT: &str = "element.text@1";
pub const TABLE: &str = "table.read@1";
pub const PDF: &str = "pdf.bytes@1";

/// `page.text_all@1`'s and `page.text_visible@1`'s answer; `page.text@1`'s, with the article's
/// fields.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Text {
    tab: Tab,
    article: bool,
    title: Option<String>,
    byline: Option<String>,
    lang: Option<String>,
    why: Option<String>,
    blocks: Vec<Block>,
    left_out: Map<String, Value>,
    frames: Frames,
}

/// The blocks' lines under `out`'s, `empty` when there are none, and the frames that could not
/// be read.
fn text_lines(t: &Text, out: &mut Vec<String>, empty: &str) {
    if t.blocks.is_empty() {
        out.push(empty.into());
    }
    blocks(&t.blocks, out);
    out.extend(frames_line(&t.frames));
}

/// `page_text_all`.
pub fn all(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, TEXT_ALL, Map::new(), |a, sites| {
        let t: Text = read(TEXT_ALL, a.value.clone())?;
        let mut out = vec![head(&t.tab, sites)];
        text_lines(&t, &mut out, "the page has no text");
        Ok(out)
    })
}

/// `page_text_visible`.
pub fn visible(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, TEXT_VISIBLE, Map::new(), |a, sites| {
        let t: Text = read(TEXT_VISIBLE, a.value.clone())?;
        let mut out = vec![head(&t.tab, sites)];
        text_lines(&t, &mut out, "no text in view");
        Ok(out)
    })
}

/// What the article left out (`left_out`), in the contract's order, as one phrase.
fn left_out(kinds: &Map<String, Value>) -> Option<String> {
    const KNOWN: [(&str, &str, &str); 7] = [
        ("navigation", "navigation", "navigation menus"),
        ("banner", "the page's header", "headers"),
        ("contentinfo", "the footer", "footers"),
        ("complementary", "an aside", "asides"),
        ("comments", "comments", "comment sections"),
        ("form", "a form", "forms"),
        ("frames", "a frame", "frames"),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut say = |n: u64, one: &str, many: &str| match n {
        0 => {}
        1 => parts.push(one.to_owned()),
        n => parts.push(format!("{n} {many}")),
    };
    for (key, one, many) in KNOWN {
        say(
            kinds.get(key).and_then(Value::as_u64).unwrap_or_default(),
            one,
            many,
        );
    }
    for (key, n) in kinds {
        if !KNOWN.iter().any(|(k, ..)| k == key) {
            let kind = shown_text(key);
            say(n.as_u64().unwrap_or_default(), &format!("1 {kind}"), &kind);
        }
    }
    let last = parts.pop()?;
    Some(if parts.is_empty() {
        last
    } else {
        format!("{} and {last}", parts.join(", "))
    })
}

/// `page.text@1`'s lines: the article with what it left out, else all the text and why.
fn article_lines(t: &Text, sites: &SiteRules) -> Vec<String> {
    let mut out = vec![head(&t.tab, sites)];
    if t.article {
        let mut line = "the article".to_owned();
        if let Some(title) = t.title.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(" {}", quoted(title)));
        }
        if let Some(by) = t.byline.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(", byline {}", quoted(by)));
        }
        if let Some(lang) = t.lang.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(", language {}", shown_text(lang)));
        }
        line.push(':');
        out.push(line);
        if let Some(what) = left_out(&t.left_out) {
            out.push(format!("{what} left out; page_text_all reads everything"));
        }
        text_lines(t, &mut out, "the article has no text");
    } else {
        out.push(match t.why.as_deref().filter(|s| !s.is_empty()) {
            Some(why) => format!(
                "no article ({}), so all of the page's text:",
                shown_text(why)
            ),
            None => "no article, so all of the page's text:".to_owned(),
        });
        text_lines(t, &mut out, "the page has no text");
    }
    out
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PdfBytes {
    tab: Tab,
    /// Where the bytes came from, after redirects.
    #[serde(deserialize_with = "or_default")]
    url: String,
    #[serde(deserialize_with = "or_default")]
    base64: String,
}

/// `page_text`: on a PDF in Firefox's viewer (the extension answers `pdf_viewer`), the PDF's
/// text (`super::pdf`).
pub fn page_text(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let sites = inner.browser().sites;
                match send(&inner, &call, &t, TEXT, Map::new()).await {
                    Ok(v) => {
                        let a = answered(&inner, &call, &t, v)?;
                        let text: Text = read(TEXT, a.value)?;
                        Ok(article_lines(&text, &sites).join("\n"))
                    }
                    Err(OpError::Failed { code, .. }) if code == "pdf_viewer" => {
                        pdf_text(&inner, &call, &sites).await
                    }
                    // One of Firefox's own pages, Reader view or an error page: read through
                    // the desktop's accessibility where it offers one (kai-firefox WP12).
                    Err(OpError::Failed { code, .. })
                        if super::chrome::OWN_PAGES.contains(&code.as_str())
                            && super::chrome::can_read_own(&inner) =>
                    {
                        super::chrome::own_page(&inner, &t).await
                    }
                    Err(e) => Err(failure(&inner, &t.link, e, None, &call.cancel).await),
                }
            }
            .await,
        )
    })
}

/// The PDF's text: its bytes from the extension, then `pdftotext` (`super::pdf`). The URL the
/// bytes came from (after redirects) is checked against the site rules like the tab's.
async fn pdf_text(inner: &Inner, call: &Call, sites: &SiteRules) -> Result<String, String> {
    let t = target(call)?;
    let v = match send(inner, call, &t, PDF, Map::new()).await {
        Ok(v) => v,
        Err(e) => return Err(failure(inner, &t.link, e, None, &call.cancel).await),
    };
    let a = answered(inner, call, &t, v)?;
    let p: PdfBytes = read(PDF, a.value)?;
    if p.url.trim().is_empty() {
        return Err(format!(
            "Kai for Firefox sent the PDF of tab {} without the URL it came from, so Kai cannot \
             tell its site (Kai's site rules) and does not read it",
            p.tab.id
        ));
    }
    if sites.rule(&p.url) == Rule::None {
        return Err(format!(
            "the PDF of tab {} came from a site Kai does not reach (Kai's site rules), so Kai \
             does not read it",
            p.tab.id
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(p.base64.as_bytes())
        .map_err(|e| {
            format!("Kai for Firefox sent the PDF's bytes in base64 Kai cannot read: {e}")
        })?;
    let b = inner.browser();
    let limits = super::pdf::Limits {
        time: Duration::from_secs(b.pdf_timeout_s),
        text_bytes: b.pdf_text_max_mb.saturating_mul(1 << 20),
        memory_bytes: b.pdf_memory_max_mb.saturating_mul(1 << 20),
    };
    let text = super::pdf::extract(&super::pdf::Tool::find(), bytes, &limits).await?;
    let mut out = vec![head(&p.tab, sites)];
    out.extend(super::pdf::lines(&text));
    Ok(out.join("\n"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ElementText {
    #[serde(flatten)]
    element: Element,
    tab: Tab,
    section: bool,
    hidden: bool,
    blocks: Vec<Block>,
}

/// `element_text`: the element's line, then its text; a heading's whole section.
pub fn element_text(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, ELEMENT, Map::new(), element_lines)
}

fn element_lines(a: &Answer, sites: &SiteRules) -> Result<Vec<String>, String> {
    let e: ElementText = read(ELEMENT, a.value.clone())?;
    let mut out = vec![head(&e.tab, sites)];
    let what = element(&e.element);
    if e.hidden {
        // Not shown itself; the visible children of a `visibility: hidden` element still are.
        out.push(if e.blocks.is_empty() {
            format!("{what} is hidden, and nothing of it is shown")
        } else {
            format!("{what} is hidden; what of it is shown:")
        });
        if e.blocks.is_empty() {
            return Ok(out);
        }
    } else {
        out.push(if e.section {
            format!("{what}, its section:")
        } else {
            format!("{what}:")
        });
    }
    let text = Text {
        blocks: e.blocks,
        ..Default::default()
    };
    text_lines(&text, &mut out, "(no text)");
    Ok(out)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Table {
    #[serde(flatten)]
    element: Element,
    tab: Tab,
    caption: Option<String>,
    rows: u64,
    columns: u64,
    cells: Vec<Cell>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Cell {
    row: u64,
    column: u64,
    #[serde(deserialize_with = "or_default")]
    text: String,
    rowspan: Option<u64>,
    colspan: Option<u64>,
    header: Option<String>,
    #[serde(deserialize_with = "or_default")]
    column_headers: Vec<String>,
    #[serde(deserialize_with = "or_default")]
    row_headers: Vec<String>,
}

/// One cell's line: `r<row> c<column>`, its headers (column headers first, as a screen reader
/// says "Price, Plan Pro: 12 €") or that it is one, its text, its spans.
fn cell_line(c: &Cell) -> String {
    let at = format!("r{} c{}", c.row, c.column);
    let text = if c.text.is_empty() {
        "(empty)".to_owned()
    } else {
        shown_text(&c.text).into_owned()
    };
    let mut line = match c.header.as_deref() {
        Some(kind) => format!("{at} {} header: {text}", shown_text(kind)),
        None => {
            let headers: Vec<String> = c
                .column_headers
                .iter()
                .chain(&c.row_headers)
                .map(|h| shown_text(h).into_owned())
                .collect();
            if headers.is_empty() {
                format!("{at}: {text}")
            } else {
                format!("{at} {}: {text}", headers.join(", "))
            }
        }
    };
    let mut facts = Vec::new();
    if let Some(n) = c.rowspan.filter(|n| *n > 1) {
        facts.push(format!("{n} rows"));
    }
    if let Some(n) = c.colspan.filter(|n| *n > 1) {
        facts.push(format!("{n} columns"));
    }
    if !facts.is_empty() {
        line.push_str(&format!(" ({})", facts.join(", ")));
    }
    line
}

/// `table_read`.
pub fn table(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    page_tool(inner, call, TABLE, Map::new(), |a, sites| {
        let t: Table = read(TABLE, a.value.clone())?;
        let mut first = format!(
            "{}, {} {}, {} {}",
            element(&t.element),
            t.rows,
            if t.rows == 1 { "row" } else { "rows" },
            t.columns,
            if t.columns == 1 { "column" } else { "columns" }
        );
        if let Some(c) = t.caption.as_deref().filter(|c| *c != t.element.name) {
            first.push_str(&format!(", caption {}", quoted(c)));
        }
        let mut out = vec![head(&t.tab, sites), first];
        if t.cells.is_empty() {
            out.push("no cells".into());
        }
        out.extend(t.cells.iter().map(cell_line));
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_the_article_left_out_reads_as_one_phrase() {
        let m = |v: Value| v.as_object().cloned().unwrap();
        assert_eq!(left_out(&m(json!({}))), None);
        assert_eq!(
            left_out(&m(
                json!({"complementary": 3, "navigation": 1, "comments": 1})
            ))
            .unwrap(),
            "navigation, 3 asides and comments"
        );
        assert_eq!(
            left_out(&m(
                json!({"banner": 1, "contentinfo": 2, "form": 1, "frames": 1,
                               "ads": 2})
            ))
            .unwrap(),
            "the page's header, 2 footers, a form, a frame and 2 ads"
        );
    }

    #[test]
    fn a_cell_says_its_headers_first() {
        let cell = |v: Value| cell_line(&serde_json::from_value(v).unwrap());
        assert_eq!(
            cell(json!({"row": 1, "column": 1, "text": "Tent", "rowspan": 2,
                        "header": "column", "column_headers": [], "row_headers": []})),
            "r1 c1 column header: Tent (2 rows)"
        );
        assert_eq!(
            cell(
                json!({"row": 4, "column": 2, "text": "2.9 kg", "colspan": 2,
                        "column_headers": ["Weight", "Packed", "Trail"],
                        "row_headers": ["Valley Tent"]})
            ),
            "r4 c2 Weight, Packed, Trail, Valley Tent: 2.9 kg (2 columns)"
        );
        assert_eq!(
            cell(
                json!({"row": 2, "column": 3, "text": "", "column_headers": ["Price"],
                        "row_headers": ["Plan Pro"]})
            ),
            "r2 c3 Price, Plan Pro: (empty)"
        );
        assert_eq!(
            cell(
                json!({"row": 3, "column": 1, "text": "Lamp\nTrail", "column_headers": null,
                        "row_headers": []})
            ),
            "r3 c1: \"Lamp\\nTrail\""
        );
    }
}
