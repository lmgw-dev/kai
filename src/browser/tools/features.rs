//! Firefox's own features (kai-firefox design §5.5): `page_zoom`, `reader_view`, `bookmarks_find`,
//! `history_find` and `downloads_list`, over the contract's operations of the same names. Compact
//! lines (K22), nothing cut.
//!
//! - **The zoom** is set (`browser_context` reads it). Firefox keeps it per site by default (the
//!   answer's `scope`): the line says that it applies to every tab of the site.
//! - **Reader view** is a page Kai's reading tools cannot read (design §3); turning it on says so.
//! - **Both act only on the page the dialog showed:** the call carries that page's origin, and the
//!   extension checks it right before and again right after, since Firefox's zoom and Reader view
//!   take a tab, not a document. Its `page_changed` is worded for each tool
//!   (`zoom_page_changed`, `reader_page_changed`): found after, the zoom was set (or Reader view
//!   changed) already, and may have reached the page the tab shows now.
//! - **Bookmarks, the history and the downloads** name no tab, so they have no preparation: each
//!   linked Firefox that serves the operation is asked (under its profile's heading while more
//!   than one is linked), and Kai's site rules apply to every URL of every entry: an entry with a
//!   URL on a `none` site is left out whole and counted ("2 more on sites Kai does not reach"),
//!   as `tab_list` does with tabs. A download names three: where it came from, where that led and
//!   the page that linked it. Private windows leave no history and their downloads are not listed
//!   (the extension's part).
//! - **A finished download** is opened with Kai's own `open` on its path: Firefox's
//!   `downloads.open` needs a click in the extension, so the result says which tool does it.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::lines::{number, site, when};
use super::read::reply;
use super::tabs::{current, load_error};
use super::{Inner, answered, failure, op, read, send, target};
use crate::browser::links::{LinkInfo, OpError};
use crate::browser::moves::origin;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head, or_default};
use crate::desktop::tools::launch::size as human;
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const ZOOM: &str = "page.zoom@1";
pub const READER: &str = "reader.view@1";
pub const BOOKMARKS: &str = "bookmarks.find@1";
pub const HISTORY: &str = "history.find@1";
pub const DOWNLOADS: &str = "downloads.list@1";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Zoom {
    tab: Tab,
    percent: f64,
    was: f64,
    default_percent: f64,
    #[serde(deserialize_with = "or_default")]
    scope: String,
}

/// `op` on the page the dialog showed (module doc), with `extra` and that page's origin; its
/// `page_changed` as `changed` (the tool's code for it, worded in `super::failed`).
async fn on_page(
    inner: &Inner,
    call: &Call,
    op: &str,
    mut extra: Map<String, Value>,
    changed: &str,
) -> Result<super::Answer, String> {
    let t = target(call)?;
    // Acts after the owner was asked: on the tab and the site the dialog showed.
    current(inner, call, &t).await?;
    let tab = t.tab.as_ref().ok_or("no tab was prepared")?;
    extra.insert("origin".into(), origin(&tab.url).into());
    match send(inner, call, &t, op, extra).await {
        Ok(v) => answered(inner, call, &t, v),
        Err(e) => {
            let e = match e {
                OpError::Failed {
                    code,
                    message,
                    about,
                } if code == "page_changed" => OpError::Failed {
                    code: changed.into(),
                    message,
                    about,
                },
                other => other,
            };
            Err(failure(inner, &t.link, e, None, &call.cancel).await)
        }
    }
}

/// `page_zoom`: the zoom set, with what Firefox does with it.
pub fn zoom(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let percent = call
                    .args
                    .integer("percent")
                    .ok_or("page_zoom needs percent, the zoom to set")?;
                if percent < 1 {
                    return Err("page_zoom: percent is a whole number from 1".to_owned());
                }
                let mut extra = Map::new();
                extra.insert("percent".into(), percent.into());
                let a = on_page(&inner, &call, ZOOM, extra, "zoom_page_changed").await?;
                let z: Zoom = read(ZOOM, a.value)?;
                let sites = inner.browser().sites;
                let now = number(z.percent);
                let default = number(z.default_percent);
                let what = if z.was == z.percent {
                    format!("zoom {now} %, as it was (Firefox's default {default} %)")
                } else {
                    format!(
                        "zoom {now} % (was {} %, Firefox's default {default} %)",
                        number(z.was)
                    )
                };
                let the_site = site(Some(&origin(&z.tab.url)))
                    .map_or_else(|| "its site".to_owned(), |s| shown_text(&s).into_owned());
                let scope = match z.scope.as_str() {
                    "per-origin" => format!(
                        "Firefox zooms every tab of {the_site} alike and keeps the zoom for the \
                         site"
                    ),
                    "per-tab" => "Firefox zooms this tab only".to_owned(),
                    other => format!("Firefox's zoom scope: {}", shown_text(other)),
                };
                Ok(format!("{}: {what}; {scope}", head(&z.tab, &sites)))
            }
            .await,
        )
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Reader {
    tab: Tab,
    loaded: bool,
    load_error: Option<String>,
    on: bool,
    changed: bool,
}

/// `reader_view`: Reader view on or off in the tab the dialog showed.
pub fn reader(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let on = call.args.boolean("on").ok_or(
                    "reader_view needs on: true to turn Reader view on, false to turn it off",
                )?;
                let mut extra = Map::new();
                extra.insert("on".into(), on.into());
                let a = on_page(&inner, &call, READER, extra, "reader_page_changed").await?;
                let r: Reader = read(READER, a.value)?;
                let state = if r.on { "on" } else { "off" };
                let mut line = format!("{}: ", head(&r.tab, &inner.browser().sites));
                if r.changed {
                    line.push_str(&format!("Reader view {state}"));
                } else {
                    line.push_str(&format!("Reader view was {state} already"));
                }
                match (&r.load_error, r.loaded) {
                    (Some(e), _) => {
                        line.push_str(&format!("; the page did not load: {}", load_error(e)));
                    }
                    (None, false) => line.push_str("; the page is still loading"),
                    (None, true) => {}
                }
                if r.on {
                    line.push_str(
                        "; Kai's reading tools cannot read Reader view (page_text reads the \
                         page's article with Reader view off)",
                    );
                }
                Ok(line)
            }
            .await,
        )
    })
}

/// `op` with `args` on every linked Firefox that serves it, side by side; each answer or its
/// failure in words.
async fn everywhere(
    inner: &Inner,
    call: &Call,
    op_name: &str,
    args: Value,
) -> Result<Vec<(LinkInfo, Result<Value, String>)>, String> {
    let linked: Vec<LinkInfo> = inner
        .links
        .list()
        .into_iter()
        .filter(|l| l.ops.iter().any(|o| o == op_name))
        .collect();
    if linked.is_empty() {
        return Err("no Firefox with Kai for Firefox is linked".into());
    }
    let calls = linked.into_iter().map(|l| {
        let args = args.clone();
        async move {
            let r = op(inner, &l, op_name, args, &call.cancel).await;
            (l, r)
        }
    });
    Ok(futures_util::future::join_all(calls).await)
}

/// The lines of every profile's answer (`each` gives an answer's lines and how many entries it
/// left out for Kai's site rules): under each profile's heading while more than one answered.
fn profiles(
    answers: Vec<(LinkInfo, Result<Value, String>)>,
    mut each: impl FnMut(Value) -> Result<(Vec<String>, u64), String>,
) -> Vec<String> {
    let several = answers.len() > 1;
    let mut out = Vec::new();
    for (link, answer) in answers {
        if several {
            out.push(format!(
                "profile {} (Firefox {}):",
                shown_text(&link.profile),
                shown_text(&link.firefox)
            ));
        }
        match answer.and_then(&mut each) {
            Ok((lines, unreached)) => {
                out.extend(lines);
                if unreached > 0 {
                    out.push(format!(
                        "{unreached} more on sites Kai does not reach (Kai's site rules)"
                    ));
                }
            }
            Err(e) => out.push(e),
        }
    }
    out
}

/// A URL Kai may name: one that is no site Kai's rules keep it from. An empty or missing URL
/// names no site and keeps nothing out.
fn reached(sites: &SiteRules, url: Option<&str>) -> bool {
    url.is_none_or(|u| u.trim().is_empty() || sites.rule(u) != Rule::None)
}

/// The query, checked: a tool that finds by words needs some.
fn query(call: &Call) -> Result<String, String> {
    let q = call.args.text("query").unwrap_or_default();
    if q.trim().is_empty() {
        return Err(format!(
            "{} needs words to find; the query was empty",
            call.spec.name
        ));
    }
    Ok(q.to_owned())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Bookmarks {
    bookmarks: Vec<Bookmark>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Bookmark {
    #[serde(deserialize_with = "or_default")]
    title: String,
    #[serde(deserialize_with = "or_default")]
    url: String,
    added: Option<i64>,
    folder: Vec<String>,
}

/// `bookmarks_find`: one line per bookmark, the newest first.
pub fn bookmarks(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let q = query(&call)?;
                let answers = everywhere(&inner, &call, BOOKMARKS, json!({"query": q})).await?;
                let sites = inner.browser().sites;
                Ok(profiles(answers, |v| {
                    let b: Bookmarks = read(BOOKMARKS, v)?;
                    let mut out = Vec::new();
                    let mut unreached = 0;
                    for m in &b.bookmarks {
                        if !reached(&sites, Some(&m.url)) {
                            unreached += 1;
                            continue;
                        }
                        let mut facts = Vec::new();
                        if let Some(at) = m.added {
                            facts.push(format!("added {}", when(at)));
                        }
                        if !m.folder.is_empty() {
                            let path: Vec<String> = m.folder.iter().map(|f| quoted(f)).collect();
                            facts.push(format!("in {}", path.join(" › ")));
                        }
                        let mut line = format!("{} {}", quoted(&m.title), shown_text(&m.url));
                        if !facts.is_empty() {
                            line.push_str(&format!(" ({})", facts.join(", ")));
                        }
                        out.push(line);
                    }
                    if out.is_empty() {
                        out.push(format!("no bookmarks found for {}", quoted(&q)));
                    }
                    Ok((out, unreached))
                })
                .join("\n"))
            }
            .await,
        )
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct History {
    pages: Vec<Visited>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Visited {
    #[serde(deserialize_with = "or_default")]
    title: String,
    #[serde(deserialize_with = "or_default")]
    url: String,
    last_visit: Option<i64>,
    visits: Option<u64>,
}

/// `history_find`: one line per page, the most recently visited first.
pub fn history(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let q = query(&call)?;
                let answers = everywhere(&inner, &call, HISTORY, json!({"query": q})).await?;
                let sites = inner.browser().sites;
                Ok(profiles(answers, |v| {
                    let h: History = read(HISTORY, v)?;
                    let mut out = Vec::new();
                    let mut unreached = 0;
                    for p in &h.pages {
                        if !reached(&sites, Some(&p.url)) {
                            unreached += 1;
                            continue;
                        }
                        let mut facts = Vec::new();
                        if let Some(at) = p.last_visit {
                            facts.push(format!("last visited {}", when(at)));
                        }
                        match p.visits {
                            Some(1) => facts.push("1 visit".to_owned()),
                            Some(n) => facts.push(format!("{n} visits")),
                            None => {}
                        }
                        let mut line = format!("{} {}", quoted(&p.title), shown_text(&p.url));
                        if !facts.is_empty() {
                            line.push_str(&format!(" ({})", facts.join(", ")));
                        }
                        out.push(line);
                    }
                    if out.is_empty() {
                        out.push(format!("no pages in the history for {}", quoted(&q)));
                    }
                    Ok((out, unreached))
                })
                .join("\n"))
            }
            .await,
        )
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Downloads {
    downloads: Vec<Download>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Download {
    #[serde(deserialize_with = "or_default")]
    file: String,
    #[serde(deserialize_with = "or_default")]
    path: String,
    url: Option<String>,
    final_url: Option<String>,
    referrer: Option<String>,
    #[serde(deserialize_with = "or_default")]
    state: String,
    error: Option<String>,
    paused: bool,
    size: Option<u64>,
    received: Option<u64>,
    started: Option<i64>,
    exists: Option<bool>,
    danger: Option<String>,
}

impl Download {
    /// Every URL it names is on a site Kai reaches.
    fn reached(&self, sites: &SiteRules) -> bool {
        [&self.url, &self.final_url, &self.referrer]
            .into_iter()
            .all(|u| reached(sites, u.as_deref()))
    }

    fn line(&self) -> String {
        let mut facts = Vec::new();
        facts.push(match self.state.as_str() {
            "complete" => match self.size {
                Some(n) => format!("complete, {}", human(n)),
                None => "complete".to_owned(),
            },
            "in_progress" => {
                let got = human(self.received.unwrap_or_default());
                let mut s = match self.size {
                    Some(n) => format!("downloading, {got} of {}", human(n)),
                    None => format!("downloading, {got} so far"),
                };
                if self.paused {
                    s.push_str(", paused");
                }
                s
            }
            "interrupted" => "interrupted".to_owned(),
            other => format!("state {}", shown_text(other)),
        });
        if let Some(e) = &self.error {
            facts.push(format!("Firefox says {}", shown_text(e)));
        }
        if self.exists == Some(false) {
            facts.push("no longer on disk".to_owned());
        }
        if let Some(kind) = &self.danger {
            facts.push(format!(
                "Firefox flags it as dangerous ({})",
                shown_text(kind)
            ));
        }
        if let Some(at) = self.started {
            facts.push(format!("started {}", when(at)));
        }
        if let Some(from) = self.final_url.as_deref().or(self.url.as_deref()) {
            facts.push(format!("from {}", shown_text(from)));
        }
        format!(
            "{} {}: {}",
            quoted(&self.file),
            shown_text(&self.path),
            facts.join(", ")
        )
    }

    fn openable(&self) -> bool {
        self.state == "complete" && self.exists != Some(false)
    }
}

/// `downloads_list`: one line per download, the newest first, and how to open a finished one.
pub fn downloads(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let answers = everywhere(&inner, &call, DOWNLOADS, json!({})).await?;
                let sites = inner.browser().sites;
                let mut openable = false;
                let mut out = profiles(answers, |v| {
                    let d: Downloads = read(DOWNLOADS, v)?;
                    let mut out = Vec::new();
                    let mut unreached = 0;
                    for item in &d.downloads {
                        if !item.reached(&sites) {
                            unreached += 1;
                            continue;
                        }
                        openable |= item.openable();
                        out.push(item.line());
                    }
                    if out.is_empty() {
                        out.push("no downloads".to_owned());
                    }
                    Ok((out, unreached))
                });
                if openable {
                    out.push(
                        "a finished download opens with Kai's open tool on its path (open \
                         {target: <path>}): Firefox opens one only on a click in the extension"
                            .to_owned(),
                    );
                }
                Ok(out.join("\n"))
            }
            .await,
        )
    })
}
