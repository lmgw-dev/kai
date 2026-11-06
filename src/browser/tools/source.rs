//! The source and the DOM (kai-firefox design §5.6): `page_source` (the HTML the server sent,
//! fetched again, with whether the browser's cache or the server answered), `page_dom` and
//! `element_dom` (the live document as HTML, whole, with its shadow roots and frames), over the
//! contract's operations of the same names. Page text under the tab's title and URL, line by line
//! (`super::lines::page_lines`), nothing cut: a large document passes whole, and a result larger
//! than lmgw's link takes is the call's error naming that limit (the host's, `Call::room`).
//!
//! - **The source** of a page that came from a `POST` (`not_get`), or that Kai for Firefox does not
//!   know how it was requested (`source_unknown`: loaded before the extension started, written by
//!   a script, or reported by Firefox later than `content_ms`), is not fetched again; the error
//!   points to `page_dom`. A source fetched from a site Kai does not reach, or redirected to one,
//!   is not said. The fetch asks the browser's cache first and the server only when the cache has
//!   no copy, which repeats the page's request (`cache: false`; the tool's description says so).
//! - **Its frames' addresses:** the server's text is not changed by the extension, which lists
//!   the address of every frame element in it (`frames`, each `src` exactly as the text writes it
//!   with the origin it names). Kai takes every occurrence of each address whose origin is a site
//!   Kai does not reach (an opaque one while some site is `none`: the strictest rule) out of the
//!   text before it is said, as `page_dom`'s text has such a frame's origin only, and says how
//!   many it took out. A source whose frames the extension does not list is not said while some
//!   site is `none`.
//! - **Frames** come nested in the extension's answer (each child document after its `<iframe>`);
//!   Kai writes where each begins and ends as an HTML comment on its own line, with the document's
//!   URL and the `src` its element names where that differs. A frame of a site Kai does not reach
//!   is taken out by its own origin (`super::page::withhold`) and stands as one comment line of
//!   [`WITHHELD`]; a frame the extension could not read is named with its reason where it is and
//!   counted at the end.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::lines::{Element, Frames, WITHHELD, element, frame_name, frames_line, page_lines};
use super::read::reply;
use super::{Inner, answered, ask_page, failure, read, send, target};
use crate::browser::links::OpError;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head, or_default};
use crate::host::confirm::shown_text;
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const SOURCE: &str = "page.source@1";
pub const DOM: &str = "page.dom@1";
pub const ELEMENT: &str = "element.dom@1";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Source {
    tab: Tab,
    #[serde(deserialize_with = "or_default")]
    url: String,
    method: Option<String>,
    cache: Option<bool>,
    status: Option<u64>,
    content_type: Option<String>,
    charset: Option<String>,
    response_url: Option<String>,
    bytes: Option<u64>,
    #[serde(deserialize_with = "or_default")]
    text: String,
    /// `None` from an extension that does not list them.
    frames: Option<Vec<SourceFrame>>,
}

/// A frame address of the source (`page.source@1`'s `frames`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SourceFrame {
    #[serde(deserialize_with = "or_default")]
    src: String,
    origin: Option<String>,
}

/// Where Kai cannot tell which frame addresses the source holds (module doc).
const UNLISTED: &str = "Kai for Firefox did not list the frame addresses of the page's source, and \
                        Kai's site rules keep some sites from Kai, so Kai does not say it (an \
                        update of the extension lists them)";

/// `text` with every occurrence of each frame address of `frames` whose origin is a site Kai does
/// not reach taken out (module doc), and how many addresses were.
fn redacted(text: &str, frames: &[SourceFrame], sites: &SiteRules) -> (String, usize) {
    let mut gone: Vec<&str> = frames
        .iter()
        .filter(|f| !f.src.is_empty() && sites.frame_rule(f.origin.as_deref()) == Rule::None)
        .map(|f| f.src.as_str())
        .collect();
    gone.sort_unstable();
    gone.dedup();
    // The longest first, so an address inside a longer one leaves nothing of the longer behind.
    gone.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_owned();
    for src in &gone {
        out = out.replace(src, "");
    }
    (out, gone.len())
}

/// `page.source@1`'s `fetch_failed` as Kai words it for a page's source, not a PDF's (the code is
/// the contract's for both; `super::failed`).
fn of_source(e: OpError) -> OpError {
    match e {
        OpError::Failed {
            code,
            message,
            about,
        } if code == "fetch_failed" => OpError::Failed {
            code: "source_fetch_failed".into(),
            message,
            about,
        },
        other => other,
    }
}

/// `page_source`: what the fetch was and who answered, then the text.
pub fn page_source(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let v = match send(&inner, &call, &t, SOURCE, Map::new()).await {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(
                            failure(&inner, &t.link, of_source(e), None, &call.cancel).await
                        );
                    }
                };
                let a = answered(&inner, &call, &t, v)?;
                let mut s: Source = read(SOURCE, a.value)?;
                let sites = inner.browser().sites;
                let fetched = [Some(&s.url), s.response_url.as_ref()];
                if fetched
                    .into_iter()
                    .flatten()
                    .any(|u| !u.trim().is_empty() && sites.rule(u) == Rule::None)
                {
                    return Err(format!(
                        "the source of tab {} came from a site Kai does not reach (Kai's site \
                         rules), so Kai does not say it",
                        s.tab.id
                    ));
                }
                let gone = match &s.frames {
                    Some(frames) => {
                        let (text, gone) = redacted(&s.text, frames, &sites);
                        s.text = text;
                        gone
                    }
                    None if sites.strictest() == Rule::None => return Err(UNLISTED.to_owned()),
                    None => 0,
                };
                Ok(source_lines(&s, gone, &sites).join("\n"))
            }
            .await,
        )
    })
}

/// The source's lines: what was fetched and who answered, `gone` frame addresses taken out, the
/// text.
fn source_lines(s: &Source, gone: usize, sites: &SiteRules) -> Vec<String> {
    let mut facts = Vec::new();
    if let Some(m) = &s.method {
        facts.push(shown_text(m).into_owned());
    }
    if let Some(n) = s.status {
        facts.push(format!("HTTP {n}"));
    }
    if let Some(c) = &s.content_type {
        facts.push(shown_text(c).into_owned());
    }
    if let Some(c) = &s.charset {
        facts.push(format!("read as {}", shown_text(c)));
    }
    if let Some(n) = s.bytes {
        facts.push(if n == 1 {
            "1 byte uncompressed".to_owned()
        } else {
            format!("{n} bytes uncompressed")
        });
    }
    facts.push(
        match s.cache {
            Some(true) => "the browser's cache answered, the server was not asked again",
            Some(false) => "the server was asked again, so it may differ from what the tab shows",
            None => "Kai for Firefox could not tell whether the cache or the server answered",
        }
        .to_owned(),
    );
    if let Some(r) = &s.response_url {
        facts.push(format!("redirected to {}", shown_text(r)));
    }
    match gone {
        0 => {}
        1 => facts.push(
            "the address of 1 frame of a site Kai does not reach taken out of the text".to_owned(),
        ),
        n => facts.push(format!(
            "the addresses of {n} frames of sites Kai does not reach taken out of the text"
        )),
    }
    let mut out = vec![
        head(&s.tab, sites),
        format!("source of {}: {}:", shown_text(&s.url), facts.join(", ")),
    ];
    if s.text.is_empty() {
        out.push("(the answer has no body)".to_owned());
    } else {
        page_lines(&s.text, &mut out);
    }
    out
}

/// `u` where it names no site Kai does not reach.
fn named<'a>(sites: &SiteRules, u: Option<&'a str>) -> Option<&'a str> {
    u.filter(|u| sites.rule(u) != Rule::None)
}

/// The document's parts as lines (module doc): HTML text as the page has it, Kai's comment lines
/// where a frame begins and ends, one of a site Kai does not reach as [`WITHHELD`], one the
/// extension could not read with its reason.
fn dom_lines(parts: &[Value], sites: &SiteRules, out: &mut Vec<String>) {
    let mut html = String::new();
    let flush = |html: &mut String, out: &mut Vec<String>| {
        if !html.is_empty() {
            page_lines(html, out);
            html.clear();
        }
    };
    for part in parts {
        if let Some(text) = part.as_str() {
            html.push_str(text);
            continue;
        }
        flush(&mut html, out);
        let f = part.as_object();
        let text = |k: &str| {
            f.and_then(|f| f.get(k))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        };
        if f.and_then(|f| f.get("withheld")).and_then(Value::as_bool) == Some(true) {
            out.push(format!("<!-- {WITHHELD} -->"));
            continue;
        }
        // A URL of a frame Kai reaches may still name a site it does not (its `src` before a
        // redirect): such a URL is left out of the line.
        let url = named(sites, text("url"));
        if let Some(code) = text("unread") {
            let name = frame_name(text("origin"), text("title"));
            out.push(format!(
                "<!-- frame {name} could not be read here: {} -->",
                super::lines::unread_why(code)
            ));
            continue;
        }
        let mut begins = match url {
            Some(u) => format!("<!-- frame {}", shown_text(u)),
            None => "<!-- frame".to_owned(),
        };
        if let Some(src) = named(sites, text("src")).filter(|s| Some(*s) != url) {
            begins.push_str(&format!(" (its element's src {})", shown_text(src)));
        }
        out.push(format!("{begins} begins -->"));
        let inner: &[Value] = f
            .and_then(|f| f.get("html"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        dom_lines(inner, sites, out);
        out.push(match url {
            Some(u) => format!("<!-- frame {} ends -->", shown_text(u)),
            None => "<!-- frame ends -->".to_owned(),
        });
    }
    flush(&mut html, out);
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Dom {
    tab: Tab,
    html: Vec<Value>,
    frames: Frames,
}

/// `page_dom`: the live document as HTML, whole, with Kai's marks where frames begin and end.
pub fn page_dom(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let a = ask_page(&inner, &call, DOM, Map::new()).await?;
                let d: Dom = read(DOM, a.value)?;
                let sites = inner.browser().sites;
                let mut out = vec![head(&d.tab, &sites)];
                dom_lines(&d.html, &sites, &mut out);
                out.extend(frames_line(&d.frames));
                Ok(out.join("\n"))
            }
            .await,
        )
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ElementDom {
    #[serde(flatten)]
    element: Element,
    tab: Tab,
    html: Vec<Value>,
}

/// `element_dom`: one element's live HTML, the same way.
pub fn element_dom(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let a = ask_page(&inner, &call, ELEMENT, Map::new()).await?;
                let d: ElementDom = read(ELEMENT, a.value)?;
                let sites = inner.browser().sites;
                let mut out = vec![head(&d.tab, &sites), format!("{}:", element(&d.element))];
                dom_lines(&d.html, &sites, &mut out);
                Ok(out.join("\n"))
            }
            .await,
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::browser::settings::Sites;

    #[test]
    fn a_document_reads_as_lines_with_its_frames_marked() {
        let sites = SiteRules::read(&Sites {
            none: vec!["tracker.example".into()],
            ..Default::default()
        })
        .unwrap();
        let parts = json!([
            "<!DOCTYPE html><html><body>\r\n<p>a</p><iframe src=\"https://consent.example\">",
            {"frame": 5, "url": "https://consent.example/", "origin": "https://consent.example",
             "src": "https://consent.example/?a=1",
             "html": ["<p>consent</p>",
                      {"frame": 6, "withheld": true},
                      "<p>end</p>"]},
            "</iframe><p>\u{202e}x</p>",
            {"frame": 7, "url": "https://pay.example/", "origin": "https://pay.example",
             "unread": "content_timeout", "title": "Pay"},
            {"frame": 8, "url": "https://ok.example/", "origin": "https://ok.example",
             "src": "https://tracker.example/r?to=ok", "html": []},
            "</body></html>"
        ]);
        let mut out = Vec::new();
        dom_lines(parts.as_array().unwrap(), &sites, &mut out);
        assert_eq!(
            out,
            [
                "<!DOCTYPE html><html><body>",
                "<p>a</p><iframe src=\"https://consent.example\">",
                "<!-- frame https://consent.example/ (its element's src \
                 https://consent.example/?a=1) begins -->",
                "<p>consent</p>",
                "<!-- (a frame of a site Kai does not reach) -->",
                "<p>end</p>",
                "<!-- frame https://consent.example/ ends -->",
                "\"</iframe><p>\\u202ex</p>\"",
                "<!-- frame pay.example \"Pay\" could not be read here: it did not answer within \
                 Kai for Firefox's setting content_ms -->",
                "<!-- frame https://ok.example/ begins -->",
                "<!-- frame https://ok.example/ ends -->",
                "</body></html>",
            ]
        );
    }
}
