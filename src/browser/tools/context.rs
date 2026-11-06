//! `browser_context` (design §5.1) over `context.get@1`: where the user is, in compact lines with
//! no empty part. It starts with the user's tab's title and URL; element names and the selection's
//! first line are page text and always quoted. A tab on a `none` site is named by its id only and
//! nothing of its page is said. With two profiles linked each says its own, under its heading.
//! Its refs are Kai's numbers (`crate::browser::refs`), so element tools resolve them.
//! While announcements are on, the announcements since the previous call follow, each with
//! whether it was spoken (`crate::browser::announce`), and a reading aloud's place and state
//! (`super::aloud`).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use super::lines::{Element, Frames, element, frames_line};
use super::{Inner, page_error, read, said};
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::Tab;
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const GET: &str = "context.get@1";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Context {
    tab: Option<ContextTab>,
    tabs_open: Option<u64>,
    focus: Option<Element>,
    selection: Option<Selection>,
    pointer: Option<Element>,
    view: Option<View>,
    dialog: Option<Element>,
    frames: Option<Frames>,
    page_error: Option<PageError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ContextTab {
    #[serde(flatten)]
    tab: Tab,
    reader: Option<Reader>,
    zoom_percent: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Reader {
    available: bool,
    on: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Selection {
    length: Option<u64>,
    first_line: Option<String>,
    masked: bool,
    field: Option<Element>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct View {
    scrollable: bool,
    percent: f64,
    heading: Option<Element>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageError {
    code: String,
    message: String,
}

pub fn run(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        let sites = inner.browser().sites;
        let linked: Vec<_> = inner
            .links
            .list()
            .into_iter()
            .filter(|l| l.ops.iter().any(|o| o == GET))
            .collect();
        let answers = futures_util::future::join_all(
            linked
                .iter()
                .map(|l| inner.links.op(l.id, GET, json!({}), &call.cancel)),
        )
        .await;
        let several = linked.len() > 1;
        let mut out = Vec::new();
        let mut failed = 0;
        for (l, answer) in linked.iter().zip(answers) {
            if several {
                out.push(format!("profile {}:", shown_text(&l.profile)));
            }
            match answer
                .map_err(|e| said(&e))
                .map(|mut v| {
                    super::page::withhold(&mut v, &sites);
                    inner.renumber(l, &mut v);
                    v
                })
                .and_then(|v| read::<Context>(GET, v))
            {
                Ok(c) => out.extend(lines(&c, &sites)),
                Err(e) => {
                    failed += 1;
                    out.push(e);
                }
            }
        }
        if linked.is_empty() {
            return Reply::error("no Firefox with Kai for Firefox is linked");
        }
        if let Some(r) = inner.reading().as_ref() {
            out.push(r.line(&sites));
        }
        out.extend(inner.announcer.since_last_look(several));
        if failed == linked.len() {
            Reply::error(out.join("\n"))
        } else {
            Reply::text(out.join("\n"))
        }
    })
}

fn lines(c: &Context, sites: &SiteRules) -> Vec<String> {
    let mut out = Vec::new();
    let tabs_open = c.tabs_open.map(|n| format!("tabs open: {n}"));
    let Some(t) = &c.tab else {
        out.extend(tabs_open);
        return out;
    };
    if sites.rule(&t.tab.url) == Rule::None {
        out.push(format!(
            "the user's tab (tab {}) shows a site Kai does not reach (Kai's site rules)",
            t.tab.id
        ));
        out.extend(tabs_open);
        return out;
    }
    let mut state = vec![format!("tab {}", t.tab.id)];
    if t.tab.loading {
        state.push("loading".into());
    }
    match &t.reader {
        Some(r) if r.on => state.push("Reader view on".into()),
        Some(r) if r.available => state.push("Reader view available".into()),
        _ => {}
    }
    if let Some(z) = t.zoom_percent
        && (z - 100.0).abs() > f64::EPSILON
    {
        state.push(format!("zoom {} %", z.round()));
    }
    let first = format!(
        "{} {} ({})",
        quoted(&t.tab.title),
        shown_text(&t.tab.url),
        state.join(", ")
    );
    if let Some(e) = &c.page_error {
        let why = page_error(&e.code, &e.message).unwrap_or_else(|| {
            format!(
                "Kai cannot read this page ({}: {})",
                shown_text(&e.code),
                shown_text(&e.message)
            )
        });
        out.push(format!("{first}: {why}"));
        out.extend(tabs_open);
        return out;
    }
    out.push(first);
    if let Some(f) = &c.focus {
        out.push(format!("focus: {}", element(f)));
    }
    if let Some(s) = &c.selection {
        let chars = |n: u64| {
            if n == 1 {
                "1 character".to_owned()
            } else {
                format!("{n} characters")
            }
        };
        let at = s
            .field
            .as_ref()
            .map(|f| format!(" in {}", element(f)))
            .unwrap_or_default();
        out.push(match (s.masked, s.length) {
            // §5's words: a password field's selection is never shown.
            (true, Some(n)) => format!(
                "selection{at}: password field, {} selected, not shown",
                chars(n)
            ),
            (true, None) => "a password field has the focus; its selection is not shown".into(),
            (false, n) => {
                let mut line = format!("selection: {}{at}", chars(n.unwrap_or_default()));
                if let Some(first) = &s.first_line {
                    line.push_str(&format!("; first line: {}", quoted(first)));
                }
                line
            }
        });
    }
    if let Some(p) = &c.pointer {
        out.push(format!("pointer: {}", element(p)));
    }
    if let Some(v) = &c.view {
        let mut parts = Vec::new();
        if v.scrollable {
            parts.push(format!("{} % down the page", v.percent.round()));
        }
        if let Some(h) = &v.heading {
            parts.push(format!("first heading in view: {}", element(h)));
        }
        if !parts.is_empty() {
            out.push(format!("view: {}", parts.join("; ")));
        }
    }
    if let Some(d) = &c.dialog {
        out.push(format!("modal dialog open: {}", element(d)));
    }
    if let Some(f) = &c.frames {
        out.extend(frames_line(f));
    }
    out.extend(tabs_open);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(v: serde_json::Value) -> Context {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_context_reads_in_compact_lines() {
        let c = ctx(json!({
            "tab": {"id": 12, "window": 1, "title": "Checkout", "url": "https://shop.example/cart",
                    "active": true, "user": true, "loading": false,
                    "reader": {"available": true, "on": false}, "zoom_percent": 110},
            "tabs_open": 7,
            "focus": {"ref": 214, "role": "textbox", "name": "E-mail"},
            "selection": {"length": 10, "first_line": "trail lamp", "masked": false, "field": null},
            "pointer": {"ref": 215, "role": "link", "name": ""},
            "view": {"scrollable": true, "percent": 40.4,
                     "heading": {"ref": 216, "role": "heading", "name": "Reviews", "level": 2}},
            "dialog": null,
            "frames": {"read": 2, "unread": 1, "unread_frames": [
                {"frame": 7, "origin": "https://pay.example", "code": "content_timeout",
                 "title": "Payment"}]},
            "page_error": null
        }));
        assert_eq!(
            lines(&c, &SiteRules::default()),
            [
                "\"Checkout\" https://shop.example/cart (tab 12, Reader view available, zoom 110 %)",
                "focus: 214 textbox \"E-mail\"",
                "selection: 10 characters; first line: \"trail lamp\"",
                "pointer: 215 link",
                "view: 40 % down the page; first heading in view: 216 heading 2 \"Reviews\"",
                "1 frame of the page could not be read: pay.example \"Payment\" (it did not \
                 answer within Kai for Firefox's setting content_ms)",
                "tabs open: 7",
            ]
        );
        let masked = ctx(json!({
            "tab": {"id": 3, "title": "Sign in", "url": "https://bank.example/"},
            "selection": {"length": 12, "first_line": null, "masked": true,
                          "field": {"ref": 9, "role": "textbox", "name": "Password"}},
            "tabs_open": 1
        }));
        assert_eq!(
            lines(&masked, &SiteRules::default())[1],
            "selection in 9 textbox \"Password\": password field, 12 characters selected, not \
             shown"
        );
        let firefox = ctx(json!({
            "tab": {"id": 4, "title": "Settings", "url": "about:preferences"},
            "tabs_open": 2, "focus": null,
            "page_error": {"code": "firefox_page", "message": "about:preferences"}
        }));
        assert_eq!(
            lines(&firefox, &SiteRules::default()),
            [
                "\"Settings\" about:preferences (tab 4): Kai cannot read Firefox's own pages \
                 (about:preferences)",
                "tabs open: 2"
            ]
        );
    }
}
