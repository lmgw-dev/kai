//! The trusted press through accessibility (kai-firefox design §6.3, WP12; the parent's module
//! doc): Firefox's `doAction` on the element's accessible, a click the page sees as trusted and
//! user-activated (kai-firefox WP0 (d), on a Plasma test session), with no window to the front and
//! no key on the desktop.
//!
//! **Which accessible is the element** (the ref→node mapping): `element.mark@1` sets a new value
//! (128 random bits) in the element's `aria-kai-mark`, which Firefox gives the accessibility bus as
//! the object attribute `kai-mark` within one 10 ms poll (measured on a headless Firefox 159 on a
//! private bus, WP12), and answers with the element's role and name and the addresses of its
//! frames, top page first. Kai first finds the tab's own window, the one whose page in front is
//! the tab's and whose selected tab is the tab (`super::super::chrome::window`), and asks only that
//! page (the Collection interface, frames of any origin included) for the elements that carry the
//! mark; `element.unmark@1` takes it away at once, found or not (the extension also takes it away
//! by itself, `MARK_TTL_MS` of its contract after setting it). The element is taken only when:
//! - exactly one element carries the mark; **two or more is the page copying it, taken for
//!   tampering**: refused, and not pressed with the key either;
//! - its documents on the bus, top page first, are the frames the extension named (each address
//!   without its fragment; an empty `DocURL`, which Firefox gives `data:` and `blob:` documents, is
//!   refused where the extension named an address);
//! - none of those documents is one Kai never reads (`super::super::chrome::never`) or on a
//!   `read` or `none` site (`SiteRules::page_rule`, as the desktop's tools judge a page);
//! - its role fits the role the extension gave (where the contract's role is one of those a press
//!   takes, [`fits`]) and its name is the extension's (whitespace and case aside; Firefox may add
//!   a file input's button and state after it).
//!
//! Every one of these failing ends the press with what failed. Right before its action the tab's
//! window is looked at again: its page in front must still be the one searched (another tab came
//! to the front: nothing pressed); the provider then checks the element's role, name and pages
//! again (`crate::desktop::fdo::atspi::press`).
//!
//! **When** ([`usable`]): the desktop offers accessibility, the link knows its Firefox's main
//! process (`LinkInfo::firefox_pid`) and the extension serves `element.mark@1`,
//! `element.unmark@1` and `gate.open@1`. An element not found on the bus (Firefox not on it, the
//! tab not in front of its window, the element not exposed, an element with no action to run, such
//! as a media element) takes the key's path; every other failure ends the press, and a press that
//! was sent is never sent again.
//!
//! **The order** (the commit guard and the spans as the key's path has them): the input turn, the
//! tabs looked at, the input spans; the tab's window; the mark, the search, the mark taken away;
//! the checks above; `gate.open@1` on the element's document (its questions held until the press
//! went, every request allowed with the guard off, so `click_seen` still says where the click
//! went); the cancel, the screen lock and the tab's window again; the press; `gate.result@1`; the
//! spans' end and a look, so a tab the click opened is Kai's. However it ends early,
//! [`super::Closing`] takes the mark away, reads the gate and ends the spans.

use std::sync::Arc;

use kai_desktop::caps::accessibility::{Accessibility, Found, Marked};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::super::chrome::{never, window};
use super::super::lines::{Element, element};
use super::super::page::withhold;
use super::super::{Inner, Target, answered, failure, look, read};
use super::{Closing, GATE_RESULT, Pressed, Via};
use crate::browser::links::OpError;
use crate::browser::moves::now_ms;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::Tab;
use crate::browser::tools::guard::{self, Questions, gate_arg};

pub const MARK: &str = "element.mark@1";
pub const UNMARK: &str = "element.unmark@1";
pub const GATE_OPEN: &str = "gate.open@1";

/// The desktop's accessibility and the Firefox process the press searches (module doc).
pub struct Access {
    access: Arc<dyn Accessibility>,
    pid: u32,
}

/// The press can go through accessibility for `t` (module doc, "When").
pub fn usable(inner: &Inner, t: &Target) -> Option<Access> {
    let access = inner.desk.access()?;
    let pid = t.link.firefox_pid.filter(|p| *p != 0)?;
    let serves = |op: &str| t.link.ops.iter().any(|o| o == op);
    (serves(MARK) && serves(UNMARK) && serves(GATE_OPEN)).then_some(Access { access, pid })
}

/// The roles of the bus (Firefox's AT-SPI roles, as `role_name` names them) that fit each role of
/// the contract a press takes (module doc); a role of the contract not listed here is not
/// compared.
const ROLES: &[(&str, &[&str])] = &[
    ("button", &["button", "toggle button", "push button menu"]),
    ("link", &["link"]),
    ("checkbox", &["check box"]),
    ("radio", &["radio button"]),
    ("switch", &["switch", "toggle button", "check box"]),
    ("menuitem", &["menu item"]),
    ("menuitemcheckbox", &["check menu item"]),
    ("menuitemradio", &["radio menu item"]),
    ("tab", &["page tab"]),
    ("option", &["list item", "menu item"]),
    ("treeitem", &["tree item", "list item"]),
    ("combobox", &["combo box", "entry", "autocomplete"]),
    ("textbox", &["entry", "text"]),
    ("searchbox", &["entry", "text"]),
    ("slider", &["slider"]),
    ("spinbutton", &["spin button"]),
    ("img", &["image"]),
    ("heading", &["heading"]),
];

/// The bus's role `atspi` fits the contract's role `role` (module doc).
pub fn fits(role: &str, atspi: &str) -> bool {
    match ROLES.iter().find(|(r, _)| *r == role) {
        Some((_, bus)) => bus.contains(&atspi),
        None => true,
    }
}

/// The bus's name `atspi` is the extension's `name` (module doc).
fn same_name(name: &str, atspi: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let (want, got) = (norm(name), norm(atspi));
    want.is_empty() || got == want || got.starts_with(&format!("{want} "))
}

/// What does not fit between the element found on the bus and the extension's facts (module doc);
/// `None` where everything does.
fn mismatch(
    found: &Found,
    frames: &[String],
    role: &str,
    name: &str,
    sites: &SiteRules,
) -> Option<String> {
    if frames.is_empty() {
        return Some(
            "Kai for Firefox did not name the element's frames, so Kai cannot tell that the \
             element it found by its mark is the element"
                .into(),
        );
    }
    if found.documents.len() != frames.len() {
        return Some(format!(
            "the element Kai found by its mark is {} page(s) and frame(s) deep, the element {} \
             (the page may have copied the mark)",
            found.documents.len(),
            frames.len()
        ));
    }
    for (doc, frame) in found.documents.iter().zip(frames) {
        if doc.is_empty() && !frame.is_empty() {
            return Some(
                "the page or frame of the element Kai found by its mark gives no address on the \
                 accessibility bus (Firefox gives none for a data: or blob: document), so Kai \
                 cannot tell that it is the element's"
                    .into(),
            );
        }
        if window::bare(doc) != window::bare(frame) {
            return Some(
                "the element Kai found on the accessibility bus by its mark is in another page \
                 than the element (the page may have copied the mark)"
                    .into(),
            );
        }
    }
    for doc in &found.documents {
        if let Some(what) = never::never_read(doc) {
            return Some(format!(
                "the element is inside {what}, which Kai never reads or presses in"
            ));
        }
        if sites.page_rule(doc) >= Rule::Read {
            return Some(
                "the element is inside a page or frame where Kai may only read, that Kai does not \
                 reach, or whose site Kai cannot tell (Kai's site rules, as the desktop's tools \
                 judge a page)"
                    .into(),
            );
        }
    }
    if !fits(role, &found.role) {
        return Some(format!(
            "the element Kai found by its mark is a {} on the accessibility bus, the element a \
             {role} (the page may have moved the mark)",
            found.role
        ));
    }
    if !same_name(name, &found.name) {
        return Some(
            "the element Kai found by its mark has another name on the accessibility bus than \
             the element (the page may have moved the mark)"
                .into(),
        );
    }
    None
}

/// The press through accessibility (module doc) of the element of `t` in `tab` (as
/// `element.press@1` left it): `None` where the element is not found on the accessibility bus, so
/// the key's path goes on.
pub async fn press(
    inner: &Arc<Inner>,
    call: &crate::host::registry::Call,
    t: &Target,
    tab: &Tab,
    what: &str,
    via: &Access,
) -> Result<Option<Pressed>, String> {
    let refused = |because: &str| format!("{what} was not pressed: {because}");
    let aimed = t
        .element
        .ok_or("element_press was prepared without its element")?;
    let link = t.link.id;
    let turn = inner.desk.turn(call).await.map_err(|e| refused(&e))?;
    let before = look(inner, &call.cancel).await;
    let span = inner
        .spans
        .begin("the press through accessibility", &call.cancel)
        .await;
    let mut closing = Closing {
        inner: inner.clone(),
        link,
        check: None,
        gate: None,
        read: None,
        watching: None,
        span: Some(span),
        mark: None,
    };
    // The tab's own window: only its page in front is searched.
    let page = match window::find(&*via.access, via.pid, tab).await {
        Ok(w) => w.page.at,
        Err(why) => {
            tracing::info!(%why, "browser: the tab's window is not on the accessibility bus");
            None
        }
    };
    let Some(page) = page else {
        if let Some(open) = closing.span.take() {
            open.end(0).await;
        }
        return Ok(None);
    };
    // The mark, the search, the mark taken away.
    let marked = match inner
        .links
        .op(link, MARK, json!({"ref": aimed.ext}), &call.cancel)
        .await
    {
        Ok(v) => v,
        Err(e) => return Err(failure(inner, &t.link, e, Some(&aimed), &call.cancel).await),
    };
    let value = marked["value"].as_str().unwrap_or_default().to_owned();
    let name = marked["object_attribute"]
        .as_str()
        .unwrap_or("kai-mark")
        .to_owned();
    let frames: Vec<String> = marked
        .get("frames")
        .and_then(|f| serde_json::from_value(f.clone()).ok())
        .unwrap_or_default();
    let role = marked["role"].as_str().unwrap_or_default().to_owned();
    let label = marked["name"].as_str().unwrap_or_default().to_owned();
    if value.is_empty() {
        return Err(refused(
            "Kai for Firefox marked the element without a value to find it by",
        ));
    }
    closing.mark = Some(value.clone());
    let found = via.access.find_marked(&page, &name, &value).await;
    closing.mark = None;
    if let Err(e) = inner
        .links
        .op(
            link,
            UNMARK,
            json!({"value": value}),
            &CancellationToken::new(),
        )
        .await
    {
        tracing::info!(error = %e, "browser: the mark could not be taken away");
    }
    let found = match found {
        Ok(Marked::One(f)) => f,
        Ok(Marked::Several(n)) => {
            if let Some(open) = closing.span.take() {
                open.end(0).await;
            }
            return Err(refused(&format!(
                "{n} elements of the tab's page carry the mark Kai set on the element alone: the \
                 page copied it, which Kai takes for tampering, so it pressed nothing, neither \
                 through accessibility nor with a key"
            )));
        }
        other => {
            let why = match other {
                Err(e) => e.to_string(),
                _ => "no element of the tab's page carries the mark".to_owned(),
            };
            tracing::info!(
                %why,
                "browser: the element is not on the accessibility bus; the trusted press takes the key"
            );
            if let Some(open) = closing.span.take() {
                open.end(0).await;
            }
            return Ok(None);
        }
    };
    if let Some(why) = mismatch(&found, &frames, &role, &label, &inner.browser().sites) {
        if let Some(open) = closing.span.take() {
            open.end(0).await;
        }
        return Err(refused(&why));
    }
    // The gate, as the key's focus check opens it.
    let policy = inner.browser();
    let gate_guard = gate_arg(&policy, call.deadline).map_err(|e| refused(&e))?;
    let how = if policy.commit_guard {
        Questions::Held
    } else {
        Questions::Allowed
    };
    let (id, watching) = guard::watch(inner, call, t, how);
    closing.watching = Some(watching);
    let links = inner.links.clone();
    let args = json!({"ref": aimed.ext, "guard": gate_guard});
    let check = closing.check.insert(tokio::spawn(async move {
        links
            .op_as(id, link, GATE_OPEN, args, &CancellationToken::new())
            .await
    }));
    let opened = check
        .await
        .unwrap_or_else(|e| Err(OpError::failed("internal", e.to_string())));
    closing.check = None;
    match opened {
        Ok(v) => {
            closing.gate = v.get("gate").and_then(Value::as_u64);
            answered(inner, call, t, v)?;
        }
        Err(e) => return Err(failure(inner, &t.link, e, Some(&aimed), &call.cancel).await),
    }
    // Right before the press.
    if call.cancel.is_cancelled() {
        return Err(refused("the call was cancelled before the press"));
    }
    if inner.desk.locked() {
        return Err(refused(
            "the screen locked, and Kai presses nothing while it is",
        ));
    }
    // Still the tab's window with that page in front (module doc).
    match window::find(&*via.access, via.pid, tab).await {
        Ok(w) if w.page.at.as_ref() == Some(&page) => {}
        Ok(_) => {
            return Err(refused(
                "the tab's page is no longer the page in front of its window",
            ));
        }
        Err(why) => return Err(refused(&why)),
    }
    let pressed = via.access.press(&found).await;
    drop(turn);
    if let Some(w) = &closing.watching {
        w.release();
    }
    // The gate, read even when the press failed.
    let result = match closing.gate.take() {
        Some(g) => {
            let links = inner.links.clone();
            let read = closing.read.insert(tokio::spawn(async move {
                links
                    .op(
                        link,
                        GATE_RESULT,
                        json!({"gate": g}),
                        &CancellationToken::new(),
                    )
                    .await
            }));
            let r = read
                .await
                .unwrap_or_else(|e| Err(OpError::failed("internal", e.to_string())));
            closing.read = None;
            Some(match r {
                Ok(mut v) => {
                    withhold(&mut v, &inner.browser().sites);
                    inner.renumber(&t.link, &mut v);
                    Ok(v)
                }
                Err(e) => Err(failure(inner, &t.link, e, None, &CancellationToken::new()).await),
            })
        }
        None => None,
    };
    closing.watching = None;
    if let Some(open) = closing.span.take() {
        open.end(0).await;
    }
    let after = look(inner, &CancellationToken::new()).await;
    inner.moves().raised(&before, &after, now_ms());
    pressed.map_err(|e| refused(&e.to_string()))?;
    let mut done = Pressed {
        via: Via::Click,
        requests: None,
        expired: false,
        seen: None,
    };
    match result {
        None => {}
        Some(Ok(v)) => {
            done.expired = v.get("expired").and_then(Value::as_bool) == Some(true);
            done.requests = v.get("requests").cloned();
            done.seen = click_seen(&v, aimed.kai);
        }
        Some(Err(words)) => {
            return Err(format!(
                "pressed {what} through accessibility, but Kai could not read what became of the \
                 page's requests after it, nor which element got the click: {words}"
            ));
        }
    }
    Ok(Some(done))
}

/// Where the press's click went (`click_seen` of `gate.result@1`, renumbered); `None`: the element
/// got it.
pub fn click_seen(v: &Value, aimed: i64) -> Option<String> {
    let Some(seen) = v.get("click_seen").filter(|s| !s.is_null()) else {
        return Some(
            "the page did not see the click: it went elsewhere, or came after the watch".into(),
        );
    };
    match seen.get("element") {
        None | Some(Value::Null) => Some("the page's body got the click, not the element".into()),
        Some(e) if e.get("withheld").is_some() => Some(
            "an element in a frame of a site Kai does not reach got the click, not the element"
                .into(),
        ),
        Some(e) if e.get("ref").and_then(Value::as_i64) == Some(aimed) => None,
        Some(e) => Some(format!(
            "another element got the click: {}",
            element(&read::<Element>(GATE_RESULT, e.clone()).unwrap_or_default())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_click_went_reads_in_words() {
        let seen = |v: Value| click_seen(&json!({"click_seen": v}), 52);
        assert_eq!(
            seen(json!({"element": {"ref": 52, "role": "button", "name": "Go"}})),
            None
        );
        assert!(
            seen(Value::Null)
                .unwrap()
                .starts_with("the page did not see the click")
        );
        assert_eq!(
            seen(json!({"element": {"ref": 61, "role": "link", "name": "Ad"}})).unwrap(),
            "another element got the click: 61 link \"Ad\""
        );
        assert!(
            seen(json!({"element": null}))
                .unwrap()
                .starts_with("the page's body")
        );
    }

    fn found(role: &str, name: &str, docs: &[&str]) -> Found {
        Found {
            role: role.into(),
            name: name.into(),
            at: (":1.9".into(), "/e".into()),
            documents: docs.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    fn frames(f: &[&str]) -> Vec<String> {
        f.iter().map(|d| (*d).to_owned()).collect()
    }

    /// What the extension says of the element against what the bus says of the element found:
    /// its frames, the pages' rules, its role and its name.
    #[test]
    fn the_element_found_must_be_the_extensions_element() {
        let sites = SiteRules::default();
        let top = "https://shop.example/cart";
        let fine = |f: &Found, fr: &[&str], role: &str, name: &str| {
            mismatch(f, &frames(fr), role, name, &sites)
        };
        let ok = found("button", "Add to cart", &[top]);
        let anchored = format!("{top}#x");
        assert_eq!(
            fine(&ok, &[anchored.as_str()], "button", "Add to  CART"),
            None
        );
        // A file input: Firefox adds its button and state to the name.
        let file = found("button", "Upload file Browse… No file selected.", &[top]);
        assert_eq!(fine(&file, &[top], "button", "Upload file"), None);
        // A copy in a blob: frame: Firefox gives it no address.
        let blob = found("button", "Add to cart", &[top, ""]);
        let why = fine(
            &blob,
            &[top, "blob:https://shop.example/0b1c"],
            "button",
            "Add to cart",
        );
        assert!(why.unwrap().contains("gives no address"));
        // Another depth, another page.
        let why = fine(&blob, &[top], "button", "Add to cart").unwrap();
        assert!(why.contains("deep"), "{why}");
        let other = found("button", "Add to cart", &["https://bank.example/"]);
        let why = fine(&other, &[top], "button", "Add to cart").unwrap();
        assert!(why.contains("in another page"), "{why}");
        // A wrong role, a wrong name; a role a press does not list is not compared.
        let why = fine(
            &found("link", "Add to cart", &[top]),
            &[top],
            "button",
            "Add to cart",
        );
        assert!(why.unwrap().contains("is a link on the accessibility bus"));
        let why = fine(
            &found("button", "Delete", &[top]),
            &[top],
            "button",
            "Add to cart",
        );
        assert!(why.unwrap().contains("another name"));
        assert_eq!(
            fine(&found("section", "", &[top]), &[top], "generic", ""),
            None
        );
        // No frames named; another extension's page; a read site's frame.
        assert!(fine(&ok, &[], "button", "Add to cart").is_some());
        let vault = "moz-extension://4f0c/vault.html";
        let why = fine(
            &found("button", "Unlock", &[vault]),
            &[vault],
            "button",
            "Unlock",
        );
        assert!(why.unwrap().contains("another extension's page"));
        let read = SiteRules::read(&crate::browser::settings::Sites {
            read: vec!["pay.example".into()],
            ..Default::default()
        })
        .unwrap();
        let pay = found("button", "Pay", &[top, "https://pay.example/"]);
        let why = mismatch(
            &pay,
            &frames(&[top, "https://pay.example/"]),
            "button",
            "Pay",
            &read,
        );
        assert!(why.unwrap().contains("may only read"));
    }
}
