//! The style sheets (kai-firefox design §5.7): `page_css` adds one to the tab's page until the page
//! is left, every declaration made `!important` (a sheet at the user origin beats the page's
//! rules only so), and `page_css_remove` takes it out again, over `page.css@1` and
//! `page.css_remove@1`.
//!
//! - **Where it goes:** the sheet is added to the page the dialog showed: the call carries that
//!   page's origin, and the extension refuses (`page_changed`) when the tab has gone elsewhere
//!   meanwhile.
//! - **What it fetches** (kai-firefox WP7's reviews): a `url()` of the sheet, a file a function
//!   names, and every string of a declaration that is an absolute URL are fetched by Firefox once
//!   the sheet applies (a custom property through the page's own rules), and a URL the model wrote
//!   can carry data to a site. The preparation asks the extension's own parser what the sheet
//!   fetches (`page.css_check@1`, the same function `page.css@1` uses), names each fetch in the
//!   dialog and refuses a sheet that fetches from a site Kai does not reach or from an opaque
//!   origin; each other site than the page's that the user has never been to is asked about as
//!   the commit guard asks before a navigation (`super::guard::style_fetch`), whatever
//!   `page_css`'s state and also for a call approved in lmgw (its approver saw the sheet, not
//!   which of its sites are new to the user). The call then passes exactly the origins the dialog
//!   named (`fetch_origins`, the page's own among them), and the extension refuses a sheet that
//!   fetches from any other (`fetch_not_approved`). An answer naming a fetch from an origin Kai
//!   did not pass breaks that contract: Kai takes the sheet out again and says so as the call's
//!   error.
//! - **The id** is the extension's (unique for a Firefox run, one counter per profile); Kai keeps
//!   which linked Firefox and which tab each sheet it added went to (`super::Inner::sheets`, keyed
//!   by link and id), so `page_css_remove` reaches that Firefox, its tab's site rules are checked,
//!   and it is a view tool: it only takes back what Kai did. A sheet Kai does not know (added
//!   before Kai started again) goes to the one linked Firefox that removes sheets; with several,
//!   Kai cannot tell which, and says that reloading the tab takes Kai's sheets out.

use std::collections::HashSet;
use std::sync::Arc;

use kai_desktop::contract::ToolSpec;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::lines::not_understood;
use super::read::reply;
use super::{Inner, Target, answered, failure, guard, look, read, send, target};
use crate::browser::links::{LinkInfo, OpError};
use crate::browser::moves::origin;
use crate::browser::policy::BrowserPolicy;
use crate::browser::sites::Rule;
use crate::browser::tabs::{Seen, Tab, head, or_default};
use crate::host::confirm::shown_text;
use crate::host::registry::{Args, Call, Origin, Prepared, Reply};
use crate::voice::link::Pending;

pub const CHECK: &str = "page.css_check@1";
pub const ADD: &str = "page.css@1";
pub const REMOVE: &str = "page.css_remove@1";

/// A fetch of a sheet's (`page.css_check@1`'s and `page.css@1`'s `fetches`): its origin (`None`
/// when opaque) and URL.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Fetch {
    pub origin: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub url: String,
}

impl Fetch {
    /// Its origin, where it has one.
    fn site(&self) -> Option<&str> {
        self.origin
            .as_deref()
            .filter(|o| !o.trim().is_empty() && *o != "null")
    }
}

/// What `page.css_check@1` answers.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Checked {
    fetches: Vec<Fetch>,
}

/// What the dialog says of a sheet's fetches, the commit guard's questions about them, and the
/// origins it named.
#[derive(Debug, Default)]
pub struct Fetched {
    pub lines: Vec<String>,
    pub asked: Vec<guard::Asked>,
    /// Each origin a fetch is from, once, in the sheet's order: `fetch_origins`.
    pub origins: Vec<String>,
}

/// `page_css`'s preparation of its sheet `css` for `tab` in the Firefox `at` (module doc): what
/// the extension's parser says it fetches, each fetch named, refused where one is on a site Kai
/// does not reach or has no origin; each other site than the page's that the user has never been
/// to asked about.
pub async fn fetches(
    inner: &Inner,
    at: &Seen,
    tab: &Tab,
    css: &str,
    policy: &BrowserPolicy,
    cancel: &CancellationToken,
) -> Result<Fetched, String> {
    if css.trim().is_empty() {
        return Err("page_css needs a style sheet; it was empty".into());
    }
    let v = match inner
        .links
        .op(at.link.id, CHECK, json!({"css": css}), cancel)
        .await
    {
        Ok(v) => v,
        Err(e) => return Err(failure(inner, &at.link, e, None, cancel).await),
    };
    let checked: Checked = read(CHECK, v)?;
    let page = origin(&tab.url);
    let mut out = Fetched::default();
    let mut asked_for = HashSet::new();
    for f in &checked.fetches {
        let Some(o) = f.site() else {
            return Err(format!(
                "the style sheet fetches {}, whose origin is opaque, so Kai cannot let Firefox \
                 fetch it from a site the dialog names; page_css did not run",
                shown_text(&f.url)
            ));
        };
        if policy.sites.rule(o) == Rule::None || policy.sites.rule(&f.url) == Rule::None {
            return Err(
                "the style sheet fetches from a site Kai does not reach (Kai's site rules); \
                 page_css did not run"
                    .into(),
            );
        }
        if !out.origins.iter().any(|n| n == o) {
            out.origins.push(o.to_owned());
        }
        if o == page {
            out.lines.push(format!("fetches: {}", shown_text(&f.url)));
            continue;
        }
        out.lines
            .push(format!("fetches from another site: {}", shown_text(&f.url)));
        if asked_for.insert(o.to_owned())
            && let Some(a) = guard::style_fetch(inner, at, &f.url, policy, cancel).await
        {
            out.asked.push(a);
        }
    }
    Ok(out)
}

/// The fetches of `fetches` from origins not in `named`: none, unless the extension broke its
/// contract (module doc).
fn unnamed<'a>(fetches: &'a [Fetch], named: &[String]) -> Vec<&'a Fetch> {
    fetches
        .iter()
        .filter(|f| !f.site().is_some_and(|o| named.iter().any(|n| n == o)))
        .collect()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Added {
    id: i64,
    tab: Tab,
    rules: u64,
    dropped: Vec<String>,
    fetches: Vec<Fetch>,
}

/// `page_css`: the sheet added to the page the dialog showed, letting Firefox fetch from the
/// origins the dialog named; its id, what was made `!important`, what the parser did not
/// understand, and what it fetches.
pub fn add(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let tab = t.tab.clone().ok_or("no tab was prepared")?;
                let css = call.args.text("css").unwrap_or_default().to_owned();
                let mut extra = Map::new();
                extra.insert("css".into(), css.into());
                extra.insert("origin".into(), origin(&tab.url).into());
                extra.insert("fetch_origins".into(), t.fetch_origins.clone().into());
                let v = match send(&inner, &call, &t, ADD, extra).await {
                    Ok(v) => v,
                    Err(e) => return Err(failure(&inner, &t.link, e, None, &call.cancel).await),
                };
                let a = answered(&inner, &call, &t, v)?;
                let s: Added = read(ADD, a.value)?;
                inner.sheets().insert((t.link.id, s.id), s.tab.id);
                let sites = inner.browser().sites;
                let head = head(&s.tab, &sites);
                let unnamed = unnamed(&s.fetches, &t.fetch_origins);
                if !unnamed.is_empty() {
                    return Err(broken(&inner, &call, &t, s.id, &head, &unnamed).await);
                }
                let rules = match s.rules {
                    1 => "its 1 rule".to_owned(),
                    n => format!("its {n} rules"),
                };
                let mut out = vec![format!(
                    "{head}: style sheet {} added until the page is left; every declaration of \
                     {rules} was made !important, since a sheet of Kai's beats the page's rules \
                     only so",
                    s.id
                )];
                if !s.dropped.is_empty() {
                    out.push(format!(
                        "not understood, left out: {}",
                        not_understood(&s.dropped)
                    ));
                }
                let page = origin(&tab.url);
                for f in &s.fetches {
                    out.push(if f.site() == Some(page.as_str()) {
                        format!("fetches: {}", shown_text(&f.url))
                    } else {
                        format!("fetches from another site: {}", shown_text(&f.url))
                    });
                }
                Ok(out.join("\n"))
            }
            .await,
        )
    })
}

/// The extension added sheet `id` although it fetches from origins Kai did not pass (`unnamed`):
/// Kai takes it out again, and the call's error says what happened.
async fn broken(
    inner: &Inner,
    call: &Call,
    t: &Target,
    id: i64,
    head: &str,
    unnamed: &[&Fetch],
) -> String {
    let urls: Vec<String> = unnamed
        .iter()
        .map(|f| shown_text(&f.url).into_owned())
        .collect();
    tracing::warn!(
        tool = call.spec.name,
        fetches = unnamed.len(),
        "browser: Kai for Firefox added a style sheet that fetches from origins Kai did not pass; \
         taking it out again"
    );
    let what = format!(
        "{head}: Kai for Firefox added style sheet {id} although it loads files from origins the \
         dialog did not name ({}), which page.css@1 refuses by its contract",
        urls.join(", ")
    );
    match inner
        .links
        .op(t.link.id, REMOVE, json!({"id": id}), &call.cancel)
        .await
    {
        Ok(_) => {
            inner.sheets().remove(&(t.link.id, id));
            format!(
                "{what}; Kai took the sheet out again (an update of Kai for Firefox fixes this)"
            )
        }
        Err(e) => format!(
            "{what}; Kai could not take it out again ({}): page_css_remove {{id: {id}}} removes it, \
             and so does reloading the tab",
            super::said(&e)
        ),
    }
}

/// `page_css_remove`'s preparation (module doc): the Firefox and the tab the sheet went to, that
/// tab's site rules.
pub(super) async fn prepare_remove(
    inner: &Inner,
    spec: &ToolSpec,
    args: &Args,
    origin: Origin,
) -> Result<Prepared, String> {
    let tool = spec.name;
    let id = args
        .integer("id")
        .ok_or_else(|| format!("{tool} needs the id page_css answered"))?;
    let live = inner.links.list();
    let known: Vec<(LinkInfo, i64)> = inner
        .sheets()
        .iter()
        .filter(|((_, ext), _)| *ext == id)
        .filter_map(|((l, _), tab)| live.iter().find(|x| x.id == *l).map(|x| (x.clone(), *tab)))
        .collect();
    let (link, tab_id) = match known.as_slice() {
        [(l, t)] => (l.clone(), Some(*t)),
        [] => {
            let removes: Vec<&LinkInfo> = live
                .iter()
                .filter(|l| l.ops.iter().any(|o| o == REMOVE))
                .collect();
            match removes.as_slice() {
                [l] => ((*l).clone(), None),
                [] => return Err("no linked Kai for Firefox removes style sheets".into()),
                _ => {
                    return Err(format!(
                        "Kai does not know which linked Firefox has style sheet {id} (it was \
                         added before Kai started again); reloading its tab takes Kai's sheets \
                         out"
                    ));
                }
            }
        }
        _ => {
            return Err(format!(
                "style sheet {id} is a number in more than one linked Firefox profile, so Kai \
                 cannot tell which is meant; reloading its tab takes Kai's sheets out"
            ));
        }
    };
    let seen = look(inner, &CancellationToken::new()).await;
    let tab = tab_id.and_then(|id| {
        seen.iter()
            .find(|s| s.link.id == link.id)?
            .listing
            .as_ref()
            .ok()?
            .tabs
            .iter()
            .find(|t| t.id == id)
            .cloned()
    });
    let sites = inner.browser().sites;
    if let Some(t) = &tab
        && sites.rule(&t.url) == Rule::None
    {
        return Err(format!(
            "style sheet {id} is in tab {}, on a site Kai does not reach (Kai's site rules); \
             {tool} did not run",
            t.id
        ));
    }
    let line = match &tab {
        Some(t) => format!(
            "removes style sheet {id} from: {}",
            crate::browser::tabs::line(t, &link, live.len() > 1, &sites)
        ),
        None => format!("removes style sheet {id} (Kai does not know its tab now)"),
    };
    Ok(Prepared {
        lines: vec![line],
        state: Arc::new(Target {
            link,
            tab,
            destination: None,
            before: HashSet::new(),
            element: None,
            shown: None,
            to: None,
            file: None,
            approved: origin.approved_in_lmgw,
            fetch_origins: Vec::new(),
            chrome: None,
        }),
        notes: Vec::new(),
        options: Vec::new(),
        ask: None,
        spoken: None,
        confirmed: false,
    })
}

/// `page_css_remove`: the sheet taken out, or why it was gone already.
pub fn remove(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let id = call.args.integer("id").unwrap_or_default();
                let v = match inner
                    .links
                    .op(t.link.id, REMOVE, json!({"id": id}), &call.cancel)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        if matches!(&e, OpError::Failed { code, .. } if code == "unknown_css") {
                            inner.sheets().remove(&(t.link.id, id));
                        }
                        return Err(failure(&inner, &t.link, e, None, &call.cancel).await);
                    }
                };
                // Either way the id is done (the contract).
                inner.sheets().remove(&(t.link.id, id));
                if v.get("why").and_then(Value::as_str) == Some("tab_closed") {
                    let tab = v
                        .pointer("/tab/id")
                        .and_then(Value::as_i64)
                        .map_or_else(|| "its tab".to_owned(), |n| format!("tab {n}"));
                    return Ok(format!(
                        "style sheet {id} is gone: {tab} closed a moment ago, which took it out"
                    ));
                }
                let a = answered(&inner, &call, &t, v)?;
                #[derive(Debug, Default, Deserialize)]
                #[serde(default)]
                struct Removed {
                    tab: Tab,
                    removed: bool,
                    why: Option<String>,
                }
                let r: Removed = read(REMOVE, a.value)?;
                let head = head(&r.tab, &inner.browser().sites);
                Ok(match (r.removed, r.why.as_deref()) {
                    (true, _) => format!("{head}: style sheet {id} removed"),
                    (false, Some("page_left")) => format!(
                        "{head}: style sheet {id} was gone already: the page it was added to was \
                         left, which took it out"
                    ),
                    (false, Some(other)) => format!(
                        "{head}: style sheet {id} was not removed ({})",
                        shown_text(other)
                    ),
                    (false, None) => format!("{head}: style sheet {id} was not removed"),
                })
            }
            .await,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fetch_from_an_origin_kai_did_not_pass_is_unnamed() {
        let f = |origin: Option<&str>, url: &str| Fetch {
            origin: origin.map(str::to_owned),
            url: url.to_owned(),
        };
        let fetches = [
            f(Some("https://shop.example"), "https://shop.example/a.png"),
            f(Some("https://cdn.example"), "https://cdn.example/b.png"),
            f(None, "file:///etc/x.png"),
            f(Some("null"), "file:///etc/y.png"),
        ];
        let named = ["https://shop.example".to_owned()];
        let got: Vec<&str> = unnamed(&fetches, &named)
            .iter()
            .map(|f| f.url.as_str())
            .collect();
        assert_eq!(
            got,
            [
                "https://cdn.example/b.png",
                "file:///etc/x.png",
                "file:///etc/y.png"
            ]
        );
        assert!(unnamed(&fetches[..1], &named).is_empty());
    }
}
