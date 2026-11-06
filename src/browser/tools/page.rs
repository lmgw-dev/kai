//! What the reading tools share with the call's preparation (kai-firefox design §5, D4): the
//! preparation of a tool that takes a ref ([`prepare_element`]: the link that gave the ref out,
//! `element.describe@1`'s answer, the site rules on its tab and on its frame's own origin, the
//! focus rule on its tab for a tool that acts), and an
//! operation's answer from a page ([`ask_page`]: sent for the prepared tab or element, checked
//! against the site rules once more, what comes from a frame of a `none` site taken out
//! ([`withhold`]), its refs as Kai's numbers).

use std::collections::HashSet;
use std::sync::Arc;

use kai_desktop::contract::ToolSpec;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{Aimed, DESCRIBE, Inner, Reach, Target, failure, lines, read, target, verb};
use crate::browser::announce::words::asked::{self as asked_words, About, Question};
use crate::browser::links::{LinkInfo, OpError};
use crate::browser::moves::Away;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, or_default};
use crate::host::registry::{Args, Call, Origin, Prepared};

/// What `element.describe@1` answers.
#[derive(Debug, Deserialize)]
struct Described {
    tab: Tab,
    /// 0: the top document.
    #[serde(default, deserialize_with = "or_default")]
    frame: i64,
    /// Its frame's origin; `None` when opaque.
    #[serde(default)]
    origin: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    role: String,
    #[serde(default, deserialize_with = "or_default")]
    name: String,
    #[serde(default)]
    level: Option<i64>,
}

/// The preparation of a tool that takes a ref (module doc): the link that gave it out, then what
/// it names now and in which tab and frame, and the site rules on that tab and on the frame's own
/// origin (a frame of a `none` site in another site's page is not reached either). A tool that
/// acts ([`Reach::Act`]) is then held to the focus rule on the ref's tab, as one that names its
/// tab is (`super::focus`), also for a call a trusted approver approved in lmgw (kai-firefox WP6's
/// review): its approver saw a ref, not the tab; `element_drag`'s `to` is prepared the same way,
/// in the same tab, and `element_file`'s file is opened (`super::act::pick`), so the dialog says
/// what is sent and the handler sends that file.
pub(super) async fn prepare_element(
    inner: &Inner,
    spec: &ToolSpec,
    reach: Reach,
    args: &Args,
    origin: Origin,
) -> Result<Prepared, String> {
    let tool = spec.name;
    let n = args
        .integer("ref")
        .ok_or_else(|| format!("{tool} needs an element's ref, a number"))?;
    let live = inner.links.list();
    let cancel = CancellationToken::new();
    let (link, aimed, d) = described(inner, tool, reach, n, &cancel).await?;
    let sites = inner.browser().sites;
    let (role, name) = (d.role.clone(), d.name.clone());
    let shown = lines::element(&lines::Element {
        id: n,
        role: d.role,
        name: d.name,
        level: d.level,
    });
    let mut lines = vec![format!(
        "{}: {shown} in {}",
        verb(reach),
        crate::browser::tabs::line(&d.tab, &link, live.len() > 1, &sites)
    )];
    let to = match args.integer("to") {
        None => None,
        Some(m) => {
            let (to_link, to_aimed, e) = described(inner, tool, reach, m, &cancel).await?;
            if to_link.id != link.id || e.tab.id != d.tab.id {
                return Err(format!(
                    "ref {m} is in another tab than ref {n}; {tool} drops an element on one in \
                     the same page"
                ));
            }
            let line = lines::element(&lines::Element {
                id: m,
                role: e.role,
                name: e.name,
                level: e.level,
            });
            lines.push(format!("onto: {line}"));
            Some((to_aimed, line))
        }
    };
    let policy = inner.browser();
    let file = match args.text("path") {
        Some(path) => {
            let guard = super::guard::guard_arg(&policy, None)?;
            let rest = super::act::file_rest(&guard, aimed.ext, path);
            let picked = super::act::pick(path, rest)?;
            lines.push(format!("sends: {}", picked.line()));
            Some(Arc::new(picked))
        }
        None => None,
    };
    let mut tab = d.tab;
    let mut away = None;
    if reach == Reach::Act {
        // The focus rule needs the tab as the listing has it: whose it is, and its facts.
        let seen = super::look(inner, &cancel).await;
        let listed = seen
            .iter()
            .find(|s| s.link.id == link.id)
            .and_then(|s| s.listing.as_ref().ok())
            .and_then(|l| l.tabs.iter().find(|t| t.id == tab.id));
        if let Some(t) = listed {
            tab = t.clone();
        }
        if !policy.act_in_background {
            away = match listed {
                Some(t) => super::focus(inner, &seen, &link, t).err(),
                // Its tabs could not be listed: Kai cannot tell whose the tab is.
                None => Some(Away::Unsure),
            };
        }
    }
    // The question in Kai's words, for a call of Kai's own (`crate::approvals::asked`).
    let file_name = args
        .text("path")
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|f| f.to_string_lossy().into_owned());
    let spoken = Question {
        tool,
        about: About::Element {
            role: &role,
            name: &name,
        },
        why: away.map(asked_words::away).into_iter().collect(),
        file: file_name.as_deref(),
    }
    .spoken(policy.announce_max_chars);
    Ok(Prepared {
        lines,
        state: Arc::new(Target {
            link,
            tab: Some(tab),
            destination: None,
            before: HashSet::new(),
            element: Some(aimed),
            shown: Some(shown),
            to,
            file,
            approved: origin.approved_in_lmgw,
            fetch_origins: Vec::new(),
            chrome: None,
        }),
        notes: away.map(|a| a.note().to_owned()).into_iter().collect(),
        options: Vec::new(),
        ask: away.map(Away::why),
        spoken: Some(spoken),
        confirmed: false,
    })
}

/// Kai's ref `n` resolved and described (`element.describe@1`), with its link and the extension's
/// number, refused where the site rules keep `tool` from its tab or its frame.
async fn described(
    inner: &Inner,
    tool: &str,
    reach: Reach,
    n: i64,
    cancel: &CancellationToken,
) -> Result<(LinkInfo, Aimed, Described), String> {
    let live = inner.links.list();
    let found = inner.refs().find(&live, n, DESCRIBE);
    let (link, ext) = found?;
    let aimed = Aimed { kai: n, ext };
    let v = match inner
        .links
        .op(link.id, DESCRIBE, json!({"ref": ext}), cancel)
        .await
    {
        Ok(v) => v,
        Err(e) => return Err(failure(inner, &link, e, Some(&aimed), cancel).await),
    };
    let d: Described = read(DESCRIBE, v)?;
    let sites = inner.browser().sites;
    if d.tab.url.trim().is_empty() {
        return Err(format!(
            "Kai for Firefox named ref {n}'s tab {} without its URL, so Kai cannot tell its site \
             (Kai's site rules); {tool} did not run",
            d.tab.id
        ));
    }
    let frame = (d.frame != 0).then(|| sites.frame_rule(d.origin.as_deref()));
    match (sites.rule(&d.tab.url), frame, reach) {
        (Rule::None, ..) => {
            return Err(format!(
                "ref {n} is in tab {}, on a site Kai does not reach (Kai's site rules); {tool} did \
                 not run",
                d.tab.id
            ));
        }
        (_, Some(Rule::None), _) => {
            return Err(format!(
                "ref {n} is in a frame of a site Kai does not reach (Kai's site rules), in tab {}; \
                 {tool} did not run",
                d.tab.id
            ));
        }
        (Rule::Read, _, Reach::Act) | (_, Some(Rule::Read), Reach::Act) => {
            return Err(format!(
                "ref {n} is in {}, on a site where Kai may only read (Kai's site rules); {tool} \
                 did not run",
                crate::browser::tabs::head(&d.tab, &sites)
            ));
        }
        _ => {}
    }
    Ok((link, aimed, d))
}

/// A reading tool's answer, its refs as Kai's numbers (the extension's kept as `ext_ref` where
/// they differ, `crate::browser::refs`).
pub struct Answer {
    pub value: Value,
    /// An acting operation's guard's `answer_ms`: how long a held request waited for an answer
    /// (`super::act`'s lines name it).
    pub answer_ms: Option<u64>,
}

/// Sends `op` for the prepared tab (`tab`) or element (`ref`, the extension's number), with
/// `extra`; the extension's error as it is.
pub async fn send(
    inner: &Inner,
    call: &Call,
    t: &Target,
    op: &str,
    extra: Map<String, Value>,
) -> Result<Value, OpError> {
    let mut args = extra;
    match (&t.element, &t.tab) {
        (Some(a), _) => {
            args.insert("ref".into(), a.ext.into());
        }
        (None, Some(tab)) => {
            args.insert("tab".into(), tab.id.into());
        }
        (None, None) => {}
    }
    inner
        .links
        .op(t.link.id, op, Value::Object(args), &call.cancel)
        .await
}

/// An answer from a page (`value`'s `tab`), checked against the site rules once more, since the
/// tab may have gone elsewhere while the call ran; one that names no URL is not said either. Then
/// what comes from a frame of a `none` site is taken out ([`withhold`]) and the rest renumbered
/// (`crate::browser::refs`).
pub fn answered(
    inner: &Inner,
    call: &Call,
    target: &Target,
    mut value: Value,
) -> Result<Answer, String> {
    let sites = inner.browser().sites;
    checked(call, &value, &sites)?;
    withhold(&mut value, &sites);
    inner.renumber(&target.link, &mut value);
    Ok(Answer {
        value,
        answer_ms: None,
    })
}

/// [`answered`]'s check of the page alone: an answer whose `tab` names no URL, or one on a `none`
/// site, is not said. For an answer whose other parts are not the page's structure (a
/// screenshot's frames Kai blanks out itself, a script's value, which no walk may change).
pub fn checked(call: &Call, value: &Value, sites: &SiteRules) -> Result<(), String> {
    let tab = value
        .pointer("/tab/id")
        .and_then(Value::as_i64)
        .map_or_else(|| "its tab".to_owned(), |id| format!("tab {id}"));
    let url = value
        .pointer("/tab/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if url.trim().is_empty() {
        return Err(format!(
            "Kai for Firefox answered {} without the URL of {tab}, so Kai cannot tell its site \
             (Kai's site rules) and says nothing of it",
            call.spec.name
        ));
    }
    if sites.rule(url) == Rule::None {
        return Err(format!(
            "{tab} went to a site Kai does not reach (Kai's site rules) while {} ran, so nothing \
             of it is said",
            call.spec.name
        ));
    }
    Ok(())
}

/// The frame of `v` (an object of an answer) when it comes from a child frame whose own origin's
/// rule is `none` (`SiteRules::frame_rule`).
fn unreached(v: &Value, sites: &SiteRules) -> Option<Value> {
    let o = v.as_object()?;
    let frame = o
        .get("frame")
        .filter(|f| !f.is_null() && f.as_i64() != Some(0))?;
    let origin = o.get("origin").and_then(Value::as_str);
    (sites.frame_rule(origin) == Rule::None).then(|| frame.clone())
}

/// Takes out of `v` (an answer from a page) what comes from a child frame of a `none` site, by the
/// frame's own origin (kai-firefox WP5b's design decision): each object with a `frame` other than 0
/// and such an `origin` (a block, a control, a link, a field, an outline item, the selection, the
/// modal dialog, an unread frame) becomes `{"frame": <id>, "withheld": true}`, so the lines can say
/// where such a frame is and nothing of it, and its refs never become Kai's. `page.find@1`'s
/// matches in such a frame go whole, `count` less them, and so do `hints.show@1`'s `undrawn`
/// entries of such a frame (its controls are withheld already).
pub fn withhold(v: &mut Value, sites: &SiteRules) {
    if let Some(Value::Array(matches)) = v.get_mut("matches") {
        let before = matches.len();
        matches.retain(|m| unreached(m, sites).is_none());
        let gone = (before - matches.len()) as u64;
        if gone > 0
            && let Some(count) = v.get_mut("count")
            && let Some(n) = count.as_u64()
        {
            *count = n.saturating_sub(gone).into();
        }
    }
    walk(v, sites);
}

/// [`withhold`]'s walk; serde_json's own depth limit bounds the recursion.
fn walk(v: &mut Value, sites: &SiteRules) {
    let mark = |v: &mut Value| match unreached(v, sites) {
        Some(frame) => {
            *v = json!({"frame": frame, "withheld": true});
            true
        }
        None => false,
    };
    match v {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "undrawn"
                    && let Value::Array(items) = child
                {
                    items.retain(|i| unreached(i, sites).is_none());
                } else if !mark(child) {
                    walk(child, sites);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                if !mark(item) {
                    walk(item, sites);
                }
            }
        }
        _ => {}
    }
}

/// `op` on what the call was prepared for, its answer checked and renumbered ([`send`],
/// [`answered`]); the extension's error in words (`super::failure`).
pub async fn ask_page(
    inner: &Inner,
    call: &Call,
    op: &str,
    extra: Map<String, Value>,
) -> Result<Answer, String> {
    let t = target(call)?;
    match send(inner, call, &t, op, extra).await {
        Ok(v) => answered(inner, call, &t, v),
        Err(e) => Err(failure(inner, &t.link, e, t.element.as_ref(), &call.cancel).await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::settings::Sites;

    #[test]
    fn a_frame_of_a_none_site_is_taken_out_by_its_own_origin() {
        let sites = SiteRules::read(&Sites {
            none: vec!["pay.example".into()],
            ..Default::default()
        })
        .unwrap();
        let mut v = json!({
            "tab": {"id": 12, "url": "https://shop.example/"},
            "modal": {"ref": 50, "role": "dialog", "name": "Pay", "frame": 7,
                      "origin": "https://pay.example"},
            "blocks": [{"text": "Cart"},
                       {"text": "Card 4111", "frame": 7, "origin": "https://pay.example"},
                       {"text": "Cookies", "frame": 8, "origin": "https://consent.example"},
                       {"frame": 9, "origin": "https://pay.example", "unread": "no_content"},
                       {"text": "Opaque", "frame": 10, "origin": null}],
            "frames": {"read": 3, "unread": 1,
                       "unread_frames": [{"frame": 9, "origin": "https://pay.example",
                                          "code": "no_content", "title": "Pay"}]},
            "undrawn": [{"frame": 7, "origin": "https://pay.example", "code": "content_timeout",
                         "labels": 3}],
            "count": 3,
            "matches": [{"text": "a", "frame": 7, "origin": "https://pay.example"},
                        {"text": "a", "frame": 0}, {"text": "a", "element": null}]
        });
        withhold(&mut v, &sites);
        let w = |f: i64| json!({"frame": f, "withheld": true});
        assert_eq!(v["modal"], w(7));
        assert_eq!(
            v["blocks"],
            json!([{"text": "Cart"}, w(7),
                   {"text": "Cookies", "frame": 8, "origin": "https://consent.example"},
                   w(9), w(10)])
        );
        assert_eq!(v["frames"]["unread_frames"], json!([w(9)]));
        assert_eq!(v["undrawn"], json!([]));
        assert_eq!(v["count"], 2);
        assert_eq!(v["matches"].as_array().unwrap().len(), 2);
    }
}
