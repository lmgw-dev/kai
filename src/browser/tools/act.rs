//! The acting tools (kai-firefox design §5.4: `element_press`, `element_fill`, `element_choose`,
//! `element_check`, `element_file`, `element_focus`, `element_hover`, `element_drag`,
//! `element_show`, `element_scroll`, `page_scroll`, `page_key`) over the contract's operations of
//! the same names. Compact lines (K22), never cut:
//! - the first line is the tab as the operation left it (`crate::browser::tabs::head`: a press that
//!   navigated names the new page), then what was done to the element (`super::lines`'s words);
//! - `now:` the element afterwards where its value or state says more than its name (a sensitive
//!   field's value as "password field, filled (12 characters)", design §5); none where it could not
//!   be read again (`after`), which is said instead;
//! - `note:` the extension's own words where something did not go as asked;
//! - one line per request the commit guard's network gate held (`super::guard`): its method, the
//!   URL without its query (which can carry what the page sent; a `none` site's URL not at all),
//!   why it was held where that was a navigation to a site the user never visited, and what became
//!   of it (a timeout names the wait). A failed operation says the requests its gate held too.
//!
//! **Every acting operation carries the commit guard** (`super::guard::guard_arg`; a call without
//! the time for its watch is refused before anything is sent). A press or a key the page side
//! stops is asked about, every verdict of the list in one dialog, and sent again with every
//! verdict allowed in this call as `confirmed`, as often as the page side stops it (its second
//! look can stop a press after its pointer events went: what went is said in the dialog and the
//! lines); declined, nothing is sent again (`super::guard::page_side`). The network gate's
//! questions are followed while it runs (`super::guard::gated`). `element_show` is a view
//! operation and carries none. A press that needs a real key takes the trusted path
//! (`super::trusted`): the extension's `key` decides which, also with `browser.trusted_press =
//! "always"`, and where the guard stopped an always-trusted press whose key would be asked about
//! anyway, its dialog carries the key too.
//!
//! **The focus rule** (`crate::browser::moves`): every acting operation is marked as a navigation
//! of Kai's before it is sent, as `tab_go` is, and landed once it answered or failed, so a page it
//! took elsewhere (a pressed link, a submitted form, an Enter, a page that navigates on a scroll,
//! a hover, a key or a timer, a navigation that commits after the guard's window, a call
//! cancelled or timed out) does not move the site the tab is anchored on, also where the user
//! never acted in the tab; the trusted press is marked the same way around its key. With the
//! hands-and-eyes switch on, a call of a turn the owner started (`super::owners_move`, kai-firefox
//! WP10b and its review) is the owner's own move instead only where it pressed a link or a submit
//! control, or sent an Enter that submits a form, and the tab landed on the origin the extension
//! said it goes to (`goes`: the link's `href`, the form's action); for the trusted press, the tab
//! in front afterwards (the pressed one, or one Kai's key opened) on that origin. A redirect to
//! another origin and a window the page opened stay Kai's navigation and are asked about.
//!
//! `element_file` opens the file when the call is prepared, so the dialog names it by its real
//! path, its size and its type (the shared MIME-info database's guess by the name, else none), and
//! keeps it open: what is sent is that file, refused when its size or time changed since; only a
//! regular file of a real file system (not `/proc` or `/sys`, a device, a pipe or a socket). Its
//! bytes go as base64 inside the operation's frame, written as they are read (no second copy), in
//! pieces over Firefox's limit (`crate::browser::chunk`); the extension holds that frame in one
//! JavaScript string, at most [`JS_STRING_MAX`] characters, so a larger file is refused naming the
//! most it can send ([`file_max`]).

use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::guard::{self, Questions, Stopped, gated, guard_arg, page_side, verdicts};
use super::lines::{Control, Element, control, element};
use super::read::reply;
use super::trusted::{self, Key, Step};
use super::{Answer, Inner, Target, answered, failure, read, target};
use crate::browser::links::OpError;
use crate::browser::moves::origin;
use crate::browser::settings::TrustedPress;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head};
use crate::desktop::tools::launch::size as human;
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const PRESS: &str = "element.press@1";
pub const FILL: &str = "element.fill@1";
pub const CHOOSE: &str = "element.choose@1";
pub const CHECK: &str = "element.check@1";
pub const FILE: &str = "element.file@1";
pub const FOCUS: &str = "element.focus@1";
pub const HOVER: &str = "element.hover@1";
pub const DRAG: &str = "element.drag@1";
pub const SHOW: &str = "element.show@1";
pub const SCROLL: &str = "element.scroll@1";
pub const PAGE_SCROLL: &str = "page.scroll@1";
pub const KEY: &str = "page.key@1";

/// An acting operation's answer (the contract's Acting): the element as a Control with its tab,
/// and the operation's own keys. `element.press@1`'s `pressed` is the Control's `aria-pressed`;
/// that the press happened is `clicked`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Acted {
    #[serde(flatten)]
    control: Control,
    tab: Tab,
    after: Option<String>,
    /// The element is in a child frame (the contract's Acting: `frame`, its frame id).
    frame: Option<i64>,
    scrolled: bool,
    took: Option<bool>,
    note: Option<String>,
    requests: Option<Vec<Request>>,
    focused: Option<bool>,
    needs_trusted: Option<String>,
    key: Option<String>,
    clicked: Option<bool>,
    stopped: Option<String>,
    how: Option<String>,
    chosen: Option<String>,
    changed: Option<bool>,
    files: Option<Vec<Given>>,
    hovered: Option<bool>,
    dropped: Option<bool>,
    to: Option<Element>,
    hidden: bool,
    percent: Option<f64>,
    at_top: Option<bool>,
    at_bottom: Option<bool>,
    scrollable: Option<bool>,
    container: Option<Element>,
    keys: Option<String>,
    target: Option<Element>,
    prevented: bool,
    emulated: Option<String>,
    /// Where the press or the key's default takes the tab (the contract's Acting `goes`).
    goes: Option<Goes>,
}

/// Where an activation takes the tab (the contract's `goes`): `by` "link" (a link that loads
/// another document by GET, its `href`'s origin) or "submit" (a form's action's origin); for the
/// navigations, "url", "search" or "history".
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Goes {
    pub by: String,
    pub origin: Option<String>,
}

impl Goes {
    /// The tab at `url` is where this was meant to go (module doc).
    pub fn landed_at(&self, url: &str) -> bool {
        self.origin.as_deref().is_some_and(|o| o == origin(url))
    }
}

/// A request the network gate held (`requests`).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Request {
    method: String,
    url: String,
    #[serde(rename = "type")]
    kind: String,
    /// `write`, or `unseen_origin`: a navigation to a site the user has never visited.
    reason: String,
    decision: String,
}

/// A file `element.file@1` gave the input.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Given {
    name: String,
    bytes: u64,
}

/// A request's line (module doc); `answer_ms`: how long a held request waited for an answer.
pub fn request_line(r: &Request, sites: &SiteRules, answer_ms: Option<u64>) -> String {
    let mut notes = Vec::new();
    if !r.kind.is_empty() {
        notes.push(shown_text(&r.kind).into_owned());
    }
    let url = if sites.rule(&r.url) == Rule::None {
        "a site Kai does not reach (Kai's site rules)".to_owned()
    } else {
        match url::Url::parse(&r.url) {
            Ok(mut u) => {
                if u.query().is_some() || u.fragment().is_some() {
                    notes.push("its query left out".into());
                }
                u.set_query(None);
                u.set_fragment(None);
                let _ = u.set_username("");
                let _ = u.set_password(None);
                shown_text(u.as_str()).into_owned()
            }
            Err(_) => "a URL Kai cannot read".to_owned(),
        }
    };
    if r.reason == "unseen_origin" {
        notes.push("a site the user has never visited".into());
    }
    let what = if notes.is_empty() {
        format!("a {} to {url}", shown_text(&r.method))
    } else {
        format!(
            "a {} to {url} ({})",
            shown_text(&r.method),
            notes.join("; ")
        )
    };
    match r.decision.as_str() {
        "allowed" => format!("{what} went: the owner allowed it"),
        "declined" => format!("{what} was cancelled: the owner declined it"),
        "timeout" => match answer_ms {
            Some(ms) => format!(
                "{what} was cancelled: nobody answered within {ms} ms (the call's time less the \
                 commit guard's watch, browser.guard_window_ms)"
            ),
            None => format!("{what} was cancelled: nobody answered in time"),
        },
        "cancelled" => format!("{what} was cancelled with the call"),
        "no_link" => format!("{what} was cancelled: Kai's link to Firefox was gone"),
        "tab_closed" => format!("{what} was cancelled: its tab closed"),
        other => format!("{what}: {}", quoted(other)),
    }
}

/// The lines of the requests in `v` (`requests` of an answer or of `gate.result@1`).
fn requests(v: Option<&Vec<Request>>, sites: &SiteRules, answer_ms: Option<u64>) -> Vec<String> {
    v.into_iter()
        .flatten()
        .map(|r| request_line(r, sites, answer_ms))
        .collect()
}

/// `requests` of an extension's answer or error, read.
fn held(v: Option<&Value>) -> Vec<Request> {
    v.and_then(|r| serde_json::from_value(r.clone()).ok())
        .unwrap_or_default()
}

/// The lines every acting tool ends with (module doc): `now:` or what came after, the note, the
/// requests.
fn tail(a: &Acted, sites: &SiteRules, now: bool, answer_ms: Option<u64>) -> Vec<String> {
    let mut out = Vec::new();
    match a.after.as_deref() {
        None if now => {
            let line = control(&a.control, false);
            if line != element(&a.control.element) {
                out.push(format!("now: {line}"));
            }
        }
        None => {}
        // A frame's document gone is `page_left` too (the contract's `stale_document`): the page
        // around it stays, so the user hears what went (kai-firefox WP10: a cookie banner's frame
        // removed by its own close button).
        Some("page_left") if a.frame.is_some() => out.push(
            "then its frame was removed or went to another page, so its element is gone".into(),
        ),
        Some("page_left") => out.push("then the page was left, so its element is gone".into()),
        Some("removed") => out.push("then the page removed the element".into()),
        Some(code) => out.push(format!(
            "the element could not be read again afterwards ({})",
            quoted(code)
        )),
    }
    if let Some(n) = &a.note {
        out.push(format!("note: {}", shown_text(n)));
    }
    out.extend(requests(a.requests.as_ref(), sites, answer_ms));
    out
}

/// The first line: the tab, then `did`.
fn first(a: &Acted, sites: &SiteRules, did: &str) -> String {
    format!("{}: {did}", head(&a.tab, sites))
}

/// `did` with "scrolled … into view and" before it where the element was scrolled.
fn scrolled(a: &Acted, what: &str, did: &str) -> String {
    if a.scrolled {
        format!("scrolled {what} into view and {did}")
    } else {
        did.to_owned()
    }
}

/// The element of an answer, as its lines name it: Kai's number, role and quoted name.
fn named(a: &Acted) -> String {
    element(&a.control.element)
}

/// What an acting operation sends besides the ref (or tab), the guard and `confirmed`.
enum Extra {
    Args(Map<String, Value>),
    /// `element_file`'s file, written into the frame as it is read (module doc).
    File(Arc<Picked>),
}

/// Sends `op` for the prepared call (its ref, or its tab) with `extra` and the commit guard; the
/// page side's verdicts (the operations that press or submit) are asked about in one dialog and
/// the operation sent again with every verdict allowed in this call confirmed, as often as the
/// page side stops it (module doc). `did` names the action for the question ("press"); `merge`:
/// the press goes on as a real key whose question the guard's dialog asks too, and the key the
/// owner allowed in it comes back there. The answer checked and renumbered (`super::answered`),
/// with the requests of attempts the page side stopped late before its own.
async fn act(
    inner: &Arc<Inner>,
    call: &Call,
    t: &Target,
    op: &'static str,
    extra: Extra,
    did: &str,
    mut merge: Option<&mut Option<Key>>,
) -> Result<Answer, String> {
    let mut confirmed: Vec<Value> = Vec::new();
    let mut earlier: Vec<Value> = Vec::new();
    loop {
        let policy = inner.browser();
        let mut args = match &extra {
            Extra::Args(m) => m.clone(),
            Extra::File(_) => Map::new(),
        };
        match (&t.element, &t.tab) {
            (Some(a), _) => {
                args.insert("ref".into(), a.ext.into());
            }
            (None, Some(tab)) => {
                args.insert("tab".into(), tab.id.into());
            }
            (None, None) => {}
        }
        let mut answer_ms = None;
        if op != SHOW {
            let g = guard_arg(&policy, call.deadline)
                .map_err(|e| format!("{}: {e}", call.spec.name))?;
            answer_ms = g.get("answer_ms").and_then(Value::as_u64);
            args.insert("guard".into(), g);
        }
        if !confirmed.is_empty() {
            args.insert("confirmed".into(), Value::Array(confirmed.clone()));
        }
        // Kai's navigation for the focus rule (module doc), marked before it is sent: a call that
        // is cancelled or times out leaves the mark, and so does what commits after its answer.
        if let Some(tab) = &t.tab {
            inner.moves().navigated(&t.link.profile, tab.id, &tab.url);
        }
        let sent = match &extra {
            Extra::Args(_) => Ok(gated(inner, call, t, op, Value::Object(args)).await),
            Extra::File(picked) => send_file(inner, call, t, args, picked.clone()).await,
        };
        if let Some(tab) = &t.tab {
            inner.moves().landed(&t.link.profile, tab.id);
        }
        let v = match sent? {
            Ok(v) => v,
            Err(e) => {
                let mut held_now = earlier.clone();
                if let OpError::Failed { about: Some(a), .. } = &e {
                    held_now.extend(a.requests.iter().flatten().cloned());
                }
                let mut words = failure(inner, &t.link, e, t.element.as_ref(), &call.cancel).await;
                let lines = requests(
                    Some(&held(Some(&Value::Array(held_now)))),
                    &policy.sites,
                    answer_ms,
                );
                if !lines.is_empty() {
                    words.push('\n');
                    words.push_str(&lines.join("\n"));
                }
                return Err(words);
            }
        };
        let mut a = answered(inner, call, t, v)?;
        a.answer_ms = answer_ms;
        let stopped = verdicts(&a.value);
        if stopped.is_empty() {
            owners_move(inner, call, t, op, &a.value);
            if !earlier.is_empty() {
                let now = a.value.get("requests").and_then(Value::as_array).cloned();
                earlier.extend(now.into_iter().flatten());
                a.value["requests"] = Value::Array(earlier);
            }
            return Ok(a);
        }
        let shown: Acted = read(op, a.value.clone())?;
        // The second look stopped it after the events went (the contract's Acting).
        let late = shown.clicked == Some(false) || a.value.get("emulated").is_some();
        earlier.extend(
            a.value
                .get("requests")
                .and_then(Value::as_array)
                .cloned()
                .into_iter()
                .flatten(),
        );
        let what = match (&shown.target, op) {
            (Some(e), KEY) => element(e),
            (None, KEY) => "the page's body".to_owned(),
            _ => named(&shown),
        };
        let shown_element = match op {
            KEY => shown.target.as_ref(),
            _ => Some(&shown.control.element),
        };
        let head = head(&shown.tab, &policy.sites);
        let key = merge
            .is_some()
            .then(|| Key::for_role(&shown.control.element.role));
        let answered = page_side(
            inner,
            call,
            &Stopped {
                head: &head,
                what: &what,
                did,
                verdicts: &stopped,
                late,
                key,
                element: shown_element,
            },
        )
        .await;
        if answered != guard::Answered::Yes {
            let reasons: Vec<String> = stopped.iter().map(guard::reason).collect();
            let mut text = format!(
                "{head}: {what} was not {}: {} (the commit guard asked because {})",
                past(did),
                answered.why(),
                reasons.join("; ")
            );
            if late {
                text.push_str(if op == KEY {
                    "; the page got the key's events already, not its default"
                } else {
                    "; the page got the press's pointer events already, not its click"
                });
            }
            let lines = requests(
                Some(&held(Some(&Value::Array(earlier)))),
                &policy.sites,
                answer_ms,
            );
            if !lines.is_empty() {
                text.push('\n');
                text.push_str(&lines.join("\n"));
            }
            return Err(text);
        }
        if let (Some(m), Some(k)) = (merge.as_deref_mut(), key) {
            *m = Some(k);
        }
        confirmed.extend(stopped);
    }
}

/// An acting operation's answer took its tab to another page: the owner's own move where it was
/// one (module doc: a press of a link or a submit control, an Enter that submitted a form, of a
/// turn the owner started with the switch on) and the tab landed on the origin the extension said
/// it goes to. Anything else stays the navigation of Kai's it was marked as before it was sent.
fn owners_move(inner: &Inner, call: &Call, t: &Target, op: &str, answer: &Value) {
    let Some(before) = &t.tab else {
        return;
    };
    let after = answer
        .get("tab")
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .unwrap_or(&before.url);
    if after == before.url || !super::owners_move(inner, call) {
        return;
    }
    let goes = answer
        .get("goes")
        .and_then(|g| serde_json::from_value::<Goes>(g.clone()).ok())
        .filter(|g| match op {
            PRESS => g.by == "link" || g.by == "submit",
            KEY => g.by == "submit",
            _ => false,
        });
    match goes {
        Some(g) if g.landed_at(after) => {
            inner.moves().owned(&t.link.profile, before.id, Some(after));
        }
        _ => tracing::info!(
            op,
            goes = ?goes,
            "browser: the tab went to another page after an acting operation of the owner's turn \
             that was no navigation of theirs to there; Kai's navigation for the focus rule"
        ),
    }
}

/// `did`'s past for a refusal.
fn past(did: &str) -> &'static str {
    match did {
        "press" => "pressed",
        "send a key to" => "sent the key",
        _ => "acted on",
    }
}

/// `element_press` (design §6.3): a synthetic press, or the trusted path where the element needs a
/// real key or `browser.trusted_press` says always.
pub fn press(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(press_run(&inner, &call).await) })
}

async fn press_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let policy = inner.browser();
    let always = policy.trusted_press == TrustedPress::Always;
    let mut extra = Map::new();
    if always {
        extra.insert("activate".into(), false.into());
    }
    // One dialog for the guard and the key where both would ask (module doc).
    let mut merged = None;
    let merge =
        (always && matches!(trusted::step(inner, call, &t), Step::Ask(_))).then_some(&mut merged);
    let a = act(inner, call, &t, PRESS, Extra::Args(extra), "press", merge).await?;
    let answer_ms = a.answer_ms;
    let p: Acted = read(PRESS, a.value)?;
    let sites = &policy.sites;
    let what = named(&p);
    let why = match (&p.needs_trusted, always) {
        (Some(kind), _) if kind == "file_input" => {
            return Err(format!(
                "{}: {what} was not pressed: it is a file input, whose real press opens \
                 Firefox's file picker; element_file gives it a file without one",
                head(&p.tab, sites)
            ));
        }
        (Some(kind), _) => trusted::needs(kind),
        (None, true) => trusted::needs("always"),
        (None, false) => {
            let mut out = vec![first(&p, sites, &synthetic(&p, &what))];
            out.extend(tail(&p, sites, true, answer_ms));
            return Ok(out.join("\n"));
        }
    };
    // The extension's key (`activate: false` and `needs_trusted` both name it).
    let key = match p.key.as_deref() {
        Some(k) => Key::of(Some(k)),
        None => Key::for_role(&p.control.element.role),
    };
    let tab_head = head(&p.tab, sites);
    let since = crate::browser::moves::now_ms();
    // Kai's navigation for the focus rule around the real key, as for every acting operation
    // (module doc); the owner's own move only where it landed where the press goes.
    inner
        .moves()
        .navigated(&t.link.profile, p.tab.id, &p.tab.url);
    let spoken = real_key_words(inner, &p.control.element, key);
    let done = trusted::press(
        inner, call, &t, &p.tab, &tab_head, &what, key, &why, merged, spoken,
    )
    .await;
    inner.moves().landed(&t.link.profile, p.tab.id);
    let done = done?;
    if let Some(g) = p
        .goes
        .as_ref()
        .filter(|g| (g.by == "link" || g.by == "submit") && super::owners_move(inner, call))
    {
        super::owned_front(inner, &t.link.profile, &p.tab, g, since, &call.cancel).await;
    }
    let mut out = vec![match done.via {
        trusted::Via::Key(k) => format!(
            "{tab_head}: pressed {what} with {} through the desktop ({why})",
            k.name()
        ),
        trusted::Via::Click => {
            format!("{tab_head}: pressed {what} with a real click through accessibility ({why})")
        }
    }];
    if let Some(seen) = done.seen {
        out.push(seen);
    }
    if done.expired {
        out.push(
            "the commit guard's watch had ended before Kai read it, so the page's requests \
             after the press went unasked"
                .into(),
        );
    }
    out.extend(requests(
        Some(&held(done.requests.as_ref())),
        sites,
        answer_ms,
    ));
    Ok(out.join("\n"))
}

/// The trusted key's question in Kai's words, for a call of Kai's own (kai-firefox WP10b's review,
/// L4): what the press does to the element, that it takes a real key, and the page's name for it
/// last.
fn real_key_words(inner: &Inner, e: &Element, key: Key) -> Option<crate::voice::speak::Words> {
    use crate::browser::announce::words::asked as w;
    Some(
        w::Question {
            tool: "element_press",
            about: w::About::Element {
                role: &e.role,
                name: &e.name,
            },
            why: vec![w::real_key(key.name())],
            file: None,
        }
        .spoken(inner.browser().announce_max_chars),
    )
}

/// A synthetic press's words: pressed, or where it stopped (`clicked: false`, `stopped`).
fn synthetic(p: &Acted, what: &str) -> String {
    match (p.clicked, p.stopped.as_deref()) {
        (Some(false), Some("removed")) => format!(
            "{what} was not pressed: the page removed it while it was being pressed (a page \
             drawing itself again), and its replacement was not pressed; read the page again for \
             its ref"
        ),
        (Some(false), _) => format!("{what} was not pressed: the click was not sent"),
        _ => scrolled(p, what, &format!("pressed {what}")),
    }
}

/// `element_fill`.
pub fn fill(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(fill_run(&inner, &call).await) })
}

async fn fill_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let mut extra = Map::new();
    extra.insert(
        "text".into(),
        call.args.text("text").unwrap_or_default().into(),
    );
    let a = act(inner, call, &t, FILL, Extra::Args(extra), "fill", None).await?;
    let answer_ms = a.answer_ms;
    let f: Acted = read(FILL, a.value)?;
    let sites = &inner.browser().sites;
    let what = named(&f);
    let mut did = scrolled(&f, &what, &format!("filled {what}"));
    if f.how.as_deref() == Some("keys") {
        did.push_str(", typing key by key, so a list of suggestions may be open");
    }
    let mut out = vec![first(&f, sites, &did)];
    if f.took == Some(false) {
        out.push("the field does not show the text afterwards".into());
    }
    out.extend(tail(&f, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// `element_choose`.
pub fn choose(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(choose_run(&inner, &call).await) })
}

async fn choose_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let mut extra = Map::new();
    extra.insert(
        "option".into(),
        call.args.text("option").unwrap_or_default().into(),
    );
    let a = act(
        inner,
        call,
        &t,
        CHOOSE,
        Extra::Args(extra),
        "choose an option of",
        None,
    )
    .await?;
    let answer_ms = a.answer_ms;
    let c: Acted = read(CHOOSE, a.value)?;
    let sites = &inner.browser().sites;
    let chosen = c
        .chosen
        .as_deref()
        .or(call.args.text("option"))
        .unwrap_or_default();
    let mut out = vec![first(
        &c,
        sites,
        &format!("chose {} in {}", quoted(chosen), named(&c)),
    )];
    if c.took == Some(false) {
        out.push("it does not show as chosen afterwards".into());
    }
    out.extend(tail(&c, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// `element_check`.
pub fn check(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(check_run(&inner, &call).await) })
}

async fn check_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let on = call.args.boolean("on").unwrap_or(true);
    let mut extra = Map::new();
    extra.insert("on".into(), on.into());
    let a = act(inner, call, &t, CHECK, Extra::Args(extra), "check", None).await?;
    let answer_ms = a.answer_ms;
    let c: Acted = read(CHECK, a.value)?;
    let sites = &inner.browser().sites;
    let what = named(&c);
    let did = match (c.changed, on) {
        (Some(false), true) => format!("{what} was checked already; nothing changed"),
        (Some(false), false) => format!("{what} was unchecked already; nothing changed"),
        (_, true) => format!("checked {what}"),
        (_, false) => format!("unchecked {what}"),
    };
    let mut out = vec![first(&c, sites, &did)];
    if c.took == Some(false) {
        out.push(if on {
            "it does not show as checked afterwards".into()
        } else {
            "it still shows as checked afterwards".into()
        });
    }
    out.extend(tail(&c, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// The longest string Firefox's JavaScript holds (SpiderMonkey's `JSString::MAX_LENGTH`, 2^30 − 2
/// characters): the extension joins a frame Kai split into one string before it reads it, so a
/// whole `element.file@1` frame, the file's base64 in it, is at most this long (module doc).
pub const JS_STRING_MAX: u64 = (1 << 30) - 2;

/// The most bytes a file can have for its frame to fit [`JS_STRING_MAX`], with `rest` bytes of
/// the frame besides the file's base64 (4 characters for every 3 bytes, the last ones padded).
pub fn file_max(rest: u64) -> u64 {
    JS_STRING_MAX.saturating_sub(rest) / 4 * 3
}

/// The base64 characters of `n` bytes.
fn base64_len(n: u64) -> u64 {
    n.div_ceil(3) * 4
}

/// The file `element_file` was prepared for (module doc): open, as the dialog named it.
#[derive(Debug)]
pub struct Picked {
    file: std::fs::File,
    /// Where it really is (the open file's own path).
    pub path: PathBuf,
    /// The name the page gets: the given path's last part.
    pub name: String,
    /// Its type by that name, or empty.
    pub kind: String,
    pub size: u64,
    /// Its modification time when opened (seconds, nanoseconds).
    mtime: (i64, i64),
}

impl Picked {
    /// The dialog's line: the name, the size, the type and the real path.
    pub fn line(&self) -> String {
        let kind = if self.kind.is_empty() {
            String::new()
        } else {
            format!(", {}", shown_text(&self.kind))
        };
        let exact = if self.size >= 1024 {
            format!(", {} bytes", self.size)
        } else {
            String::new()
        };
        format!(
            "{} ({}{exact}{kind}) from {}",
            quoted(&self.name),
            human(self.size),
            shown_text(&self.path.display().to_string())
        )
    }
}

/// The file systems of the kernel's own files (`statfs`'s `f_type`), whose size says nothing of
/// what they hold: procfs, sysfs, debugfs, tracefs, securityfs, cgroup2, configfs, efivarfs.
const PSEUDO_FS: [i64; 8] = [
    0x9fa0,
    0x6265_6572,
    0x6462_6720,
    0x7472_6163,
    0x7363_6673,
    0x6367_7270,
    0x6265_6570,
    0xde5e_81e4,
];

/// Opens the file at `path` for `element_file` (module doc), at the call's preparation; `rest`
/// bytes of its frame besides the file's base64, for the most it can send.
pub fn pick(path: &str, rest: u64) -> Result<Picked, String> {
    let shown = shown_text(path).into_owned();
    let cannot =
        |e: std::io::Error| format!("Kai cannot read {shown}: {e}; element_file did not run");
    // Without blocking on a pipe nobody writes, and never as a controlling terminal.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(cannot)?;
    let meta = file.metadata().map_err(cannot)?;
    if !meta.file_type().is_file() {
        return Err(format!(
            "{shown} is no regular file (a folder, a device, a pipe or a socket); element_file \
             sends a file's bytes"
        ));
    }
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    // SAFETY: `fs` is a plain struct statfs fills in; `fd` is the open file's.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(fd, &mut fs) } == 0 && PSEUDO_FS.contains(&(fs.f_type as i64)) {
        return Err(format!(
            "{shown} is one of the kernel's own files (/proc, /sys), whose size says nothing of \
             what it holds; element_file sends a regular file"
        ));
    }
    let real =
        std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap_or_else(|_| PathBuf::from(path));
    let size = meta.len();
    let max = file_max(rest);
    if size > max {
        return Err(format!(
            "{shown} has {size} bytes ({}); element_file sends at most {max} bytes ({}): Kai \
             for Firefox holds the file in one JavaScript string of Firefox's, at most \
             {JS_STRING_MAX} characters, as base64 with the rest of its message",
            human(size),
            human(max)
        ));
    }
    let (name, kind) = name_and_type(path);
    Ok(Picked {
        file,
        path: real,
        name,
        kind,
        size,
        mtime: (meta.mtime(), meta.mtime_nsec()),
    })
}

/// A path's file name and its type by that name (module doc).
fn name_and_type(path: &str) -> (String, String) {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dirs = crate::desktop::fdo::launch::entries::Dirs::from_env(|k| std::env::var(k).ok());
    let kind = crate::desktop::fdo::launch::mime::MimeDb::load(&dirs)
        .by_name(&name)
        .unwrap_or_default();
    (name, kind)
}

/// The text of an `op` frame for `op` with `args` and the file's base64 in `files`, the bytes
/// cut where `base64` goes (module doc): what comes before them and after.
fn frame_ends(id: u64, op: &str, args: &Map<String, Value>, p: &Picked) -> (String, String) {
    let file = json!({"name": p.name, "type": p.kind, "base64": ""});
    let mut args = args.clone();
    args.insert("files".into(), json!([file]));
    let whole = json!({"t": "op", "id": id, "op": op, "args": args}).to_string();
    let at = whole
        .find(r#""base64":"""#)
        .map(|i| i + r#""base64":""#.len())
        .expect("the frame has the file's base64");
    (whole[..at].to_owned(), whole[at..].to_owned())
}

/// The bytes besides the file's base64 of an `element.file@1` frame for the prepared call (the
/// largest operation id and answer time stand for the ones it gets), for [`pick`].
pub fn file_rest(guard: &Value, ext_ref: i64, path: &str) -> u64 {
    let mut args = Map::new();
    args.insert("ref".into(), ext_ref.into());
    args.insert("guard".into(), guard.clone());
    let (name, kind) = name_and_type(path);
    let file = json!({"name": name, "type": kind, "base64": ""});
    args.insert("files".into(), json!([file]));
    json!({"t": "op", "id": u64::MAX, "op": FILE, "args": args})
        .to_string()
        .len() as u64
}

/// Reads the picked file into its frame (module doc): refused where it changed since it was
/// picked, or where the frame would be longer than Firefox holds.
fn file_frame(id: u64, args: &Map<String, Value>, p: &Picked) -> Result<String, String> {
    let changed = |why: &str| {
        format!(
            "{} changed since the dialog named it ({why}); element_file sends only the file the \
             owner saw, so nothing was sent",
            shown_text(&p.path.display().to_string())
        )
    };
    let meta = p.file.metadata().map_err(|e| changed(&e.to_string()))?;
    if meta.len() != p.size || (meta.mtime(), meta.mtime_nsec()) != p.mtime {
        return Err(changed("its size or its time is another now"));
    }
    let (start, end) = frame_ends(id, FILE, args, p);
    let rest = (start.len() + end.len()) as u64;
    let total = rest + base64_len(p.size);
    if total > JS_STRING_MAX {
        return Err(format!(
            "{} has {} bytes; with the rest of its message ({rest} bytes) element_file sends at \
             most {} bytes: Kai for Firefox holds the file in one JavaScript string of Firefox's, \
             at most {JS_STRING_MAX} characters",
            shown_text(&p.path.display().to_string()),
            p.size,
            file_max(rest)
        ));
    }
    let mut frame = String::with_capacity(total as usize);
    frame.push_str(&start);
    // A multiple of 3 bytes, so each block's base64 joins the next without padding.
    const BLOCK: usize = 3 * 256 * 1024;
    let mut buf = vec![0u8; BLOCK];
    let mut at = 0u64;
    while at < p.size {
        let want = BLOCK.min((p.size - at) as usize);
        let mut got = 0;
        while got < want {
            match p.file.read_at(&mut buf[got..want], at + got as u64) {
                Ok(0) => return Err(changed("it is shorter now")),
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(changed(&e.to_string())),
            }
        }
        base64::Engine::encode_string(
            &base64::engine::general_purpose::STANDARD,
            &buf[..want],
            &mut frame,
        );
        at += want as u64;
    }
    frame.push_str(&end);
    Ok(frame)
}

/// `element.file@1` with `args` and the picked file, its network gate watched (module doc): the
/// extension's answer, or Kai's own refusal (the file changed, or does not fit) before anything
/// was sent.
async fn send_file(
    inner: &Arc<Inner>,
    call: &Call,
    t: &Target,
    args: Map<String, Value>,
    p: Arc<Picked>,
) -> Result<Result<Value, OpError>, String> {
    let (id, _watching) = guard::watch(inner, call, t, Questions::Asked);
    let frame = tokio::task::spawn_blocking(move || file_frame(id, &args, &p))
        .await
        .map_err(|e| format!("Kai failed while it read the file: {e}; nothing was sent"))??;
    Ok(inner
        .links
        .op_frame(id, t.link.id, FILE, frame, &call.cancel)
        .await)
}

/// `element_file`.
pub fn file(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(file_run(&inner, &call).await) })
}

async fn file_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let picked = t
        .file
        .clone()
        .ok_or("element_file was prepared without its file")?;
    let a = act(
        inner,
        call,
        &t,
        FILE,
        Extra::File(picked),
        "give a file to",
        None,
    )
    .await?;
    let answer_ms = a.answer_ms;
    let f: Acted = read(FILE, a.value)?;
    let sites = &inner.browser().sites;
    let given: Vec<String> = f
        .files
        .iter()
        .flatten()
        .map(|g| format!("{} ({} bytes)", quoted(&g.name), g.bytes))
        .collect();
    let did = if given.is_empty() {
        format!("gave {} the file", named(&f))
    } else {
        format!("gave {} the file {}", named(&f), given.join(", "))
    };
    let mut out = vec![first(&f, sites, &did)];
    out.extend(tail(&f, sites, false, answer_ms));
    Ok(out.join("\n"))
}

/// `element_focus`.
pub fn focus(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(focus_run(&inner, &call).await) })
}

async fn focus_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let a = act(
        inner,
        call,
        &t,
        FOCUS,
        Extra::Args(Map::new()),
        "focus",
        None,
    )
    .await?;
    let answer_ms = a.answer_ms;
    let f: Acted = read(FOCUS, a.value)?;
    let sites = &inner.browser().sites;
    let what = named(&f);
    let did = if f.focused == Some(false) {
        format!("{what} cannot take the keyboard focus")
    } else {
        scrolled(&f, &what, &format!("moved the keyboard focus to {what}"))
    };
    let mut out = vec![first(&f, sites, &did)];
    out.extend(tail(&f, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// `element_hover`.
pub fn hover(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(hover_run(&inner, &call).await) })
}

async fn hover_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let a = act(
        inner,
        call,
        &t,
        HOVER,
        Extra::Args(Map::new()),
        "hover over",
        None,
    )
    .await?;
    let answer_ms = a.answer_ms;
    let h: Acted = read(HOVER, a.value)?;
    let sites = &inner.browser().sites;
    let what = named(&h);
    let did = scrolled(&h, &what, &format!("moved the pointer over {what}"));
    let mut out = vec![first(&h, sites, &did)];
    out.extend(tail(&h, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// `element_drag`: `to` is sent as the extension's number Kai's preparation found.
pub fn drag(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(drag_run(&inner, &call).await) })
}

async fn drag_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let (to, _) =
        t.to.clone()
            .ok_or("element_drag was prepared without the element to drop on")?;
    let mut extra = Map::new();
    extra.insert("to".into(), to.ext.into());
    let a = act(inner, call, &t, DRAG, Extra::Args(extra), "drag", None).await?;
    let answer_ms = a.answer_ms;
    let d: Acted = read(DRAG, a.value)?;
    let sites = &inner.browser().sites;
    let onto = d.to.as_ref().map(element).unwrap_or_default();
    let did = match (d.how.as_deref(), d.dropped) {
        (Some("pointer"), _) => format!(
            "dragged {} onto {onto} with the pointer; whether the page took it cannot be \
             seen",
            named(&d)
        ),
        (_, Some(false)) => format!(
            "dragged {} onto {onto}; it did not take the drop",
            named(&d)
        ),
        _ => format!("dragged {} onto {onto}", named(&d)),
    };
    let mut out = vec![first(&d, sites, &did)];
    out.extend(tail(&d, sites, true, answer_ms));
    Ok(out.join("\n"))
}

/// `element_show`: a view operation (no guard).
pub fn show(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(show_run(&inner, &call).await) })
}

async fn show_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let a = act(inner, call, &t, SHOW, Extra::Args(Map::new()), "show", None).await?;
    let s: Acted = read(SHOW, a.value)?;
    let sites = &inner.browser().sites;
    let what = named(&s);
    let did = if s.hidden {
        format!("{what} is hidden; nothing was scrolled")
    } else if s.scrolled {
        format!("scrolled {what} into view")
    } else {
        format!("{what} is in view already")
    };
    Ok(first(&s, sites, &did))
}

/// `to` of the scrolling tools, as the contract takes it.
fn to_screen(call: &Call) -> Result<String, String> {
    let to = call
        .args
        .text("to")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    match to.as_str() {
        "down" | "up" | "top" | "bottom" => Ok(to),
        _ => Err(format!(
            "{}: to must be down, up, top or bottom",
            call.spec.name
        )),
    }
}

/// Where a scroll left its container: `N % down`, at an end.
fn position(a: &Acted) -> String {
    match (a.at_top, a.at_bottom) {
        (Some(true), Some(true)) => "it shows all of it".into(),
        (Some(true), _) => "at the top".into(),
        (_, Some(true)) => "at the bottom".into(),
        _ => match a.percent {
            Some(p) => format!("{} % down", p.round()),
            None => "somewhere in between".into(),
        },
    }
}

/// `element_scroll`.
pub fn scroll(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(scroll_run(&inner, &call).await) })
}

async fn scroll_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let to = to_screen(call)?;
    let mut extra = Map::new();
    extra.insert("to".into(), to.clone().into());
    let a = act(inner, call, &t, SCROLL, Extra::Args(extra), "scroll", None).await?;
    let answer_ms = a.answer_ms;
    let s: Acted = read(SCROLL, a.value)?;
    let sites = &inner.browser().sites;
    let did = if s.scrolled {
        format!("scrolled {} {to}: {}", named(&s), position(&s))
    } else {
        format!("{} did not move: {}", named(&s), position(&s))
    };
    let mut out = vec![first(&s, sites, &did)];
    out.extend(tail(&s, sites, false, answer_ms));
    Ok(out.join("\n"))
}

/// `page_scroll`.
pub fn page_scroll(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(page_scroll_run(&inner, &call).await) })
}

async fn page_scroll_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let to = to_screen(call)?;
    let mut extra = Map::new();
    extra.insert("to".into(), to.clone().into());
    let a = act(
        inner,
        call,
        &t,
        PAGE_SCROLL,
        Extra::Args(extra),
        "scroll",
        None,
    )
    .await?;
    let answer_ms = a.answer_ms;
    let s: Acted = read(PAGE_SCROLL, a.value)?;
    let sites = &inner.browser().sites;
    let what = match &s.container {
        Some(c) => format!("the pane {} (the page itself does not scroll)", element(c)),
        None => "the page".to_owned(),
    };
    let did = if s.scrollable == Some(false) {
        "neither the page nor anything under the middle of the view scrolls".to_owned()
    } else if s.scrolled {
        format!("scrolled {what} {to}: {}", position(&s))
    } else {
        format!("{what} did not move: {}", position(&s))
    };
    let mut out = vec![first(&s, sites, &did)];
    out.extend(requests(s.requests.as_ref(), sites, answer_ms));
    Ok(out.join("\n"))
}

/// `page_key`: one `page.key@1` per key the text names (separated by spaces), in order; a key
/// that fails stops the rest, and the error says how many went.
pub fn page_key(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(page_key_run(&inner, &call).await) })
}

async fn page_key_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let keys: Vec<String> = call
        .args
        .text("keys")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if keys.is_empty() {
        return Err("page_key needs keys: Escape, ctrl+k".into());
    }
    let sites = inner.browser().sites;
    let mut first_line = None;
    let mut out = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let mut extra = Map::new();
        extra.insert("keys".into(), k.clone().into());
        let a = match act(
            inner,
            call,
            &t,
            KEY,
            Extra::Args(extra),
            "send a key to",
            None,
        )
        .await
        {
            Ok(a) => a,
            Err(e) if i == 0 => return Err(e),
            Err(e) => {
                return Err(format!(
                    "{i} of {} keys went to the page; then: {e}",
                    keys.len()
                ));
            }
        };
        let answer_ms = a.answer_ms;
        let p: Acted = read(KEY, a.value)?;
        first_line = Some(head(&p.tab, &sites));
        let to = p
            .target
            .as_ref()
            .map_or_else(|| "the page's body".to_owned(), element);
        let sent = p.keys.as_deref().unwrap_or(k);
        let mut line = format!("{} to {to}", shown_text(sent));
        if p.prevented {
            line.push_str(": the page took it and kept its default from happening");
        } else if let Some(e) = &p.emulated {
            line.push_str(match e.as_str() {
                "submit" => ": the form was submitted",
                "click" => ": it clicked the element",
                "close" => ": the dialog or popup closed",
                _ => "",
            });
        }
        out.push(line);
        out.extend(requests(p.requests.as_ref(), &sites, answer_ms));
    }
    let mut lines = vec![format!(
        "{}: sent {} key{}",
        first_line.unwrap_or_default(),
        keys.len(),
        if keys.len() == 1 { "" } else { "s" }
    )];
    lines.extend(out);
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_line_has_one_parenthesis_and_names_its_wait() {
        let sites = SiteRules::default();
        let r = |reason: &str, decision: &str, url: &str| Request {
            method: "POST".into(),
            url: url.into(),
            kind: "xmlhttprequest".into(),
            reason: reason.into(),
            decision: decision.into(),
        };
        assert_eq!(
            request_line(
                &r("write", "allowed", "https://shop.example/cart?t=1"),
                &sites,
                None
            ),
            "a POST to https://shop.example/cart (xmlhttprequest; its query left out) went: the \
             owner allowed it"
        );
        assert_eq!(
            request_line(
                &r("unseen_origin", "declined", "http://192.0.2.44/u"),
                &sites,
                None
            ),
            "a POST to http://192.0.2.44/u (xmlhttprequest; a site the user has never visited) \
             was cancelled: the owner declined it"
        );
        assert!(
            request_line(
                &r("write", "timeout", "https://a.example/"),
                &sites,
                Some(57_500)
            )
            .ends_with(
                "nobody answered within 57500 ms (the call's time less the commit \
                            guard's watch, browser.guard_window_ms)"
            )
        );
    }

    /// The most a file can be: its base64 and the rest of the frame within Firefox's string.
    #[test]
    fn the_largest_file_fits_firefoxs_string() {
        let max = file_max(300);
        assert!(base64_len(max) + 300 <= JS_STRING_MAX);
        assert!(base64_len(max + 3) + 300 > JS_STRING_MAX);
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(19), 28);
    }

    /// A file is opened when it is picked: only a regular file of a real file system, read whole
    /// into the frame, refused when it changed since.
    #[test]
    fn a_picked_file_is_the_one_sent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.txt");
        std::fs::write(&path, "the receipt's text\n").unwrap();
        let p = pick(path.to_str().unwrap(), 300).unwrap();
        assert_eq!((p.name.as_str(), p.size), ("receipt.txt", 19));
        assert!(
            p.line().starts_with("\"receipt.txt\" (19 bytes"),
            "{}",
            p.line()
        );
        let mut args = Map::new();
        args.insert("ref".into(), 57.into());
        let frame = file_frame(9, &args, &p).unwrap();
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["id"], 9);
        assert_eq!(v["args"]["ref"], 57);
        assert_eq!(v["args"]["files"][0]["name"], "receipt.txt");
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            v["args"]["files"][0]["base64"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(bytes, b"the receipt's text\n");
        std::fs::write(&path, "changed\n").unwrap();
        assert!(
            file_frame(9, &args, &p)
                .unwrap_err()
                .contains("changed since")
        );
        for not in ["/proc/self/status", "/dev/null"] {
            let e = pick(not, 300).unwrap_err();
            assert!(e.contains("element_file sends"), "{not}: {e}");
        }
        assert!(pick(dir.path().to_str().unwrap(), 300).is_err(), "a folder");
        assert!(
            pick("/nonexistent/x.pdf", 300)
                .unwrap_err()
                .starts_with("Kai cannot read")
        );
    }
}
