//! The commit guard (kai-firefox design §6.4) on Kai's side: what every acting operation carries
//! ([`guard_arg`]), the owner's questions while a call runs ([`ask_owner_spoken`]), the page side's
//! question before a press or a submit ([`page_side`]), the network gate's questions about the
//! requests it holds ([`follow`], [`gated`]) and the question before a navigation to a site the
//! user has never visited ([`navigation`]).
//!
//! - **The guard argument** (the contract's Guard): `"off"` while `browser.commit_guard` is off,
//!   else `browser.guard_words`, `browser.guard_window_ms` and `answer_ms`, the time a held
//!   request waits for the owner: what is left of the call's time (lmgw's deadline) less the
//!   window and `launch::ANSWER_MARGIN`, so the call answers what became of its requests before
//!   lmgw stops waiting; the contract's most (2^31 − 1 ms) for a call without a deadline. A call
//!   with no more time left than the window and the margin is refused before anything is sent,
//!   naming both.
//! - **The page side's verdicts** come as a list (`guard`): Kai asks the owner once about the
//!   whole list, each verdict in words, and sends the call again with every verdict the owner
//!   allowed in it as `confirmed`; a second look of the extension's may stop it again (a renamed
//!   button, a new form action), and is asked about again ([`super::act`]).
//! - **Every question is Kai's confirmation** (K15's dialog), naming the caller, the tab and the
//!   element; it offers no "always allow" (the guard is no policy state) and waits until the
//!   call's time less the margin, then counts as declined. Nothing of the page's request reaches
//!   Kai's log but its method.
//! - **The network gate's questions** come as `ask` frames ([`crate::browser::links`]); Kai
//!   follows them from the first gated call on, one task per question, and finds the call by the
//!   operation id the question names ([`Watch`], registered before the operation is sent). A
//!   question of an operation Kai is not waiting for is answered `allow: false` at once; one whose
//!   operation ended (the extension decided it: `timeout`, `cancelled`) closes its dialog. An
//!   operation's questions are asked one after the other; once the owner declined one (or nobody
//!   answered), the rest of that operation's are declined unasked. The trusted press holds its
//!   gate's questions until its key went, so no dialog takes the keyboard from Firefox first; a gate
//!   it opened with the guard off (for `gate.result@1`'s `key_seen`) lets every request go unasked.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::trusted::Key;
use super::{Destination, Inner, Target, said};
use crate::browser::announce::words::asked as asked_words;
use crate::browser::links::{Incoming, LinkId, OpError};
use crate::browser::moves::{now_ms, origin as origin_of};
use crate::browser::policy::BrowserPolicy;
use crate::browser::settings::CONTRACT_MS_MAX;
use crate::browser::tabs::Seen;
use crate::desktop::tools::launch::ANSWER_MARGIN;
use crate::host::confirm::{ConfirmRequest, Confirmer, quoted, shown_text};
use crate::host::policy::Caller;
use crate::host::registry::{Call, json_shown};
use crate::voice::speak::Words;

/// Whether the user has visited a URL's site.
pub const HISTORY_SEEN: &str = "history.seen@1";

/// The commit guard for an acting operation sent now by a call with `deadline` (module doc); a
/// call without the time for it is refused.
pub fn guard_arg(
    policy: &BrowserPolicy,
    deadline: Option<tokio::time::Instant>,
) -> Result<Value, String> {
    if !policy.commit_guard {
        return Ok(json!("off"));
    }
    Ok(json!({
        "words": policy.guard_words,
        "window_ms": policy.guard_window_ms,
        "answer_ms": answer_ms(policy.guard_window_ms, deadline)?,
    }))
}

/// The guard of the trusted press's focus check (`super::trusted`): the commit guard's, or with
/// the guard off one that watches nothing but the key (no words, no window), so `gate.result@1`
/// says which element the key reached.
pub fn gate_arg(
    policy: &BrowserPolicy,
    deadline: Option<tokio::time::Instant>,
) -> Result<Value, String> {
    if policy.commit_guard {
        return guard_arg(policy, deadline);
    }
    Ok(json!({"words": [], "window_ms": 0, "answer_ms": answer_ms(0, deadline)?}))
}

/// How long a held request waits for the owner (module doc), in ms: what is left of the call's
/// time less `window_ms` and the answer's margin; refused where nothing is left of it.
pub fn answer_ms(window_ms: u64, deadline: Option<tokio::time::Instant>) -> Result<u64, String> {
    let Some(d) = deadline else {
        return Ok(CONTRACT_MS_MAX);
    };
    let left = d
        .saturating_duration_since(tokio::time::Instant::now())
        .as_millis() as u64;
    let margin = ANSWER_MARGIN.as_millis() as u64;
    match left.checked_sub(window_ms + margin) {
        Some(answer) if answer > 0 => Ok(answer.min(CONTRACT_MS_MAX)),
        _ => Err(format!(
            "the call has {left} ms left (lmgw/timeout_ms), no more than the commit guard's watch \
             of {window_ms} ms after the action (browser.guard_window_ms) and the {margin} ms Kai \
             keeps to answer, so a request the page sent could not wait for anyone's answer; \
             nothing was done"
        )),
    }
}

/// How a question the call asked the owner ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    Yes,
    No,
    /// Nobody answered before the call's time (less the margin) was up.
    Late,
    /// The dialog could not be shown.
    Unshown,
    /// The call was cancelled while it waited.
    Cancelled,
}

impl Answered {
    /// Why a question that was not answered yes did not let the call go on.
    pub fn why(self) -> &'static str {
        match self {
            Answered::Yes => "the owner allowed it",
            Answered::No => "the owner declined it",
            Answered::Late => "nobody answered Kai's question in time",
            Answered::Unshown => "Kai could not show its question (the dialog cannot be shown)",
            Answered::Cancelled => "the call was cancelled while Kai asked",
        }
    }
}

/// A question for the owner while a call runs ([`ask_owner_spoken`]).
pub struct Question {
    /// What the dialog's heading names ("Let <caller> run <tool>?").
    pub tool: String,
    pub description: String,
    pub lines: Vec<String>,
    pub notes: Vec<String>,
}

/// Asks the owner `q` for `caller`'s call (module doc): until the answer, `cancel`, or `deadline`
/// less `ANSWER_MARGIN`. `spoken`: the question in Kai's words for a call of Kai's own, which Kai
/// speaks and hears answered by voice too (`crate::approvals::asked`, kai-firefox WP10b); every
/// question a call of Kai's own asks while it runs has them.
pub async fn ask_owner_spoken(
    confirmer: &dyn Confirmer,
    caller: &Caller,
    q: Question,
    spoken: Option<Words>,
    deadline: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
) -> Answered {
    let until = deadline.map(|d| d.checked_sub(ANSWER_MARGIN).unwrap_or(d));
    let (reply, answer) = oneshot::channel();
    let request = ConfirmRequest {
        tool: q.tool,
        description: q.description,
        caller: caller.describe(),
        lines: q.lines,
        destructive: false,
        deadline: until.map(|d| {
            std::time::Instant::now() + d.saturating_duration_since(tokio::time::Instant::now())
        }),
        options: Vec::new(),
        notes: q.notes,
        reply,
    };
    match spoken.filter(|_| *caller == Caller::Own) {
        Some(words) => confirmer.ask_spoken(request, words),
        None => confirmer.ask(request),
    }
    let late = async move {
        match until {
            Some(d) => tokio::time::sleep_until(d).await,
            None => std::future::pending().await,
        }
    };
    // Dropping `answer` (any arm but the first) closes the dialog.
    tokio::select! {
        a = answer => match a {
            Ok(true) => Answered::Yes,
            Ok(false) => Answered::No,
            Err(_) => Answered::Unshown,
        },
        () = cancel.cancelled() => Answered::Cancelled,
        () = late => Answered::Late,
    }
}

/// A verdict of the page side (the contract's Acting: `reason` and its keys) in words.
pub fn reason(g: &Value) -> String {
    let s = |k: &str| g.get(k).and_then(Value::as_str).unwrap_or_default();
    let word = quoted(s("word"));
    let text = quoted(s("text"));
    match s("reason") {
        "word" => match s("where") {
            "name" => format!("its name {text} holds the guard's word {word}"),
            "text" => format!("the text it shows, {text}, holds the guard's word {word}"),
            "value" => format!("its value {text} holds the guard's word {word}"),
            "title" => format!("its title {text} holds the guard's word {word}"),
            "submitter" => {
                format!("it submits a form whose button {text} holds the guard's word {word}")
            }
            "form_action" => format!(
                "it submits a form to {}, which holds the guard's word {word}",
                shown_text(s("text"))
            ),
            other => format!(
                "what it says ({}) holds the guard's word {word}: {text}",
                quoted(other)
            ),
        },
        "cross_origin_submit" => format!(
            "it sends a form ({}) to another site: {}",
            s("method").to_uppercase(),
            shown_text(s("action"))
        ),
        "enter_in_editor" => format!(
            "it is Enter in the editable field {text}, whose page may send what the field holds"
        ),
        other => format!("the commit guard's reason {}", quoted(other)),
    }
}

/// The page side's verdicts of an answer (`guard`: a list; a lone verdict read as one).
pub fn verdicts(answer: &Value) -> Vec<Value> {
    match answer.get("guard") {
        Some(Value::Array(list)) => list.iter().filter(|v| v.is_object()).cloned().collect(),
        Some(v @ Value::Object(_)) => vec![v.clone()],
        _ => Vec::new(),
    }
}

/// What the page side's question is about ([`page_side`]).
pub struct Stopped<'a> {
    /// The tab's line.
    pub head: &'a str,
    /// The element as the answer names it.
    pub what: &'a str,
    /// A verb: "press", "send a key to".
    pub did: &'a str,
    pub verdicts: &'a [Value],
    /// The guard's second look stopped it: the press's pointer events (or the key's events) had
    /// gone to the page already.
    pub late: bool,
    /// The press goes on as a real key Kai's trusted step would ask about anyway
    /// (`browser.trusted_press = "always"`): one dialog for both (`super::trusted`).
    pub key: Option<Key>,
    /// The element as the answer names it, for the question in Kai's words (none: the page's
    /// body).
    pub element: Option<&'a super::lines::Element>,
}

/// The page side's question (module doc): one dialog for every verdict of `s`.
pub async fn page_side(inner: &Inner, call: &Call, s: &Stopped<'_>) -> Answered {
    tracing::info!(
        tool = call.spec.name,
        caller = %call.caller.describe(),
        verdicts = s.verdicts.len(),
        late = s.late,
        "browser: the commit guard stopped an action on the page; asking the owner"
    );
    let did = s.did;
    let mut description = format!(
        "Kai is about to {did} an element that may commit something (the reasons below). It does \
         so only if you allow it."
    );
    let mut lines = vec![format!("tab: {}", s.head), format!("element: {}", s.what)];
    lines.extend(s.verdicts.iter().map(|v| format!("why: {}", reason(v))));
    if s.late {
        lines.push(if did == "press" {
            "already: the page got the press's pointer events; the guard stopped it before the \
             click, since the page changed what the press does"
                .into()
        } else {
            "already: the page got the key's events; the guard stopped its default, since the \
             page changed what it does"
                .into()
        });
    }
    if let Some(k) = s.key {
        description.push_str(&format!(
            " Kai presses it with a real {} through the desktop (browser.trusted_press = \
             \"always\"), Firefox's window to the front; this answer decides that key too.",
            k.name()
        ));
        lines.push(format!(
            "key: {} through the desktop (key_press's question, asked with this one)",
            k.name()
        ));
    }
    let q = Question {
        tool: format!("{} (the commit guard)", call.spec.name),
        description,
        lines,
        notes: Vec::new(),
    };
    let spoken = asked_words::Question {
        tool: call.spec.name,
        about: match s.element {
            Some(e) => asked_words::About::Element {
                role: &e.role,
                name: &e.name,
            },
            None => asked_words::About::Nothing,
        },
        why: asked_words::verdicts(s.verdicts, s.late, s.key.is_some()),
        file: None,
    }
    .spoken(inner.browser().announce_max_chars);
    let a = ask_owner_spoken(
        &*inner.confirmer,
        &call.caller,
        q,
        Some(spoken),
        call.deadline,
        &call.cancel,
    )
    .await;
    tracing::info!(
        tool = call.spec.name,
        answered = ?a,
        "browser: the commit guard's question answered"
    );
    a
}

/// A call whose operation's network gate may ask now (module doc).
pub struct Watch {
    pub tool: &'static str,
    pub caller: Caller,
    /// What the call acts on, as its dialog named it.
    pub acts_on: String,
    pub deadline: Option<tokio::time::Instant>,
    pub cancel: CancellationToken,
    /// The operation ended: every question it held is decided.
    pub done: CancellationToken,
    /// Its questions wait until this fires (module doc: the trusted press's key).
    pub release: CancellationToken,
    /// The guard asks; `false`: a gate opened with the guard off lets every request go.
    pub asks: bool,
    /// One question at a time; `true` once one was not allowed (module doc).
    pub declined: tokio::sync::Mutex<bool>,
}

/// The registration of a [`Watch`] ([`watch`]), taken out as it is dropped.
pub struct Watching {
    inner: Arc<Inner>,
    key: (LinkId, u64),
    done: CancellationToken,
    release: CancellationToken,
}

impl Watching {
    /// The questions held so far are asked now (module doc).
    pub fn release(&self) {
        self.release.cancel();
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.inner.gates().remove(&self.key);
        self.done.cancel();
        self.release.cancel();
    }
}

/// How a [`watch`]'s questions go (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Questions {
    /// Asked as they come.
    Asked,
    /// Held until [`Watching::release`].
    Held,
    /// Every request goes (the guard is off).
    Allowed,
}

/// A new operation id for the prepared call, its network gate's questions followed for as long
/// as the registration lives (module doc): `focus.check@1`'s gate asks under its id until
/// `gate.result@1` has answered.
pub fn watch(inner: &Arc<Inner>, call: &Call, t: &Target, how: Questions) -> (u64, Watching) {
    follow(inner);
    let id = inner.links.reserve();
    let done = CancellationToken::new();
    let release = CancellationToken::new();
    if how != Questions::Held {
        release.cancel();
    }
    inner.gates().insert(
        (t.link.id, id),
        Arc::new(Watch {
            tool: call.spec.name,
            caller: call.caller.clone(),
            acts_on: acts_on(inner, t),
            deadline: call.deadline,
            cancel: call.cancel.clone(),
            done: done.clone(),
            release: release.clone(),
            asks: how != Questions::Allowed,
            declined: tokio::sync::Mutex::new(false),
        }),
    );
    let watching = Watching {
        inner: inner.clone(),
        key: (t.link.id, id),
        done,
        release,
    };
    (id, watching)
}

/// What a prepared call acts on, as the questions name it: the element in its tab, or the tab.
pub fn acts_on(inner: &Inner, t: &Target) -> String {
    let sites = inner.browser().sites;
    let tab = t
        .tab
        .as_ref()
        .map(|tab| crate::browser::tabs::head(tab, &sites))
        .unwrap_or_default();
    match &t.shown {
        Some(e) => format!("{e} in {tab}"),
        None => tab,
    }
}

/// Sends `op` with `args` to the prepared call's link with its network gate watched (module doc).
pub async fn gated(
    inner: &Arc<Inner>,
    call: &Call,
    t: &Target,
    op: &str,
    args: Value,
) -> Result<Value, OpError> {
    let (id, _watching) = watch(inner, call, t, Questions::Asked);
    inner
        .links
        .op_as(id, t.link.id, op, args, &call.cancel)
        .await
}

/// Follows the network gate's questions from now on (module doc); once.
pub fn follow(inner: &Arc<Inner>) {
    if inner.following.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut rx = inner.links.subscribe();
    let links = inner.links.clone();
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            let Incoming::Ask { link, id, fields } = item else {
                continue;
            };
            let Some(inner) = weak.upgrade() else {
                let _ = links.answer(link, id, false);
                break;
            };
            tokio::spawn(question(inner, link, id, fields));
        }
    });
}

/// One question of the network gate (module doc).
async fn question(inner: Arc<Inner>, link: LinkId, id: Value, fields: Map<String, Value>) {
    let op = fields.get("op").and_then(Value::as_u64);
    let watch = op.and_then(|op| inner.gates().get(&(link, op)).cloned());
    let Some(w) = watch else {
        tracing::info!(
            link,
            "browser: the network gate asked about a request of an operation Kai is not waiting \
             for; refused"
        );
        let _ = inner.links.answer(link, id, false);
        return;
    };
    let method = fields
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("request")
        .to_owned();
    // Held until the trusted press's key went; the extension decides it if the operation ends.
    tokio::select! {
        () = w.release.cancelled() => {}
        () = w.done.cancelled() => return,
    }
    if !w.asks {
        tracing::debug!(
            link,
            tool = w.tool,
            method,
            "browser: the network gate held a request with the commit guard off; it goes"
        );
        let _ = inner.links.answer(link, id, true);
        return;
    }
    let mut declined = tokio::select! {
        d = w.declined.lock() => d,
        () = w.done.cancelled() => return,
    };
    if *declined {
        tracing::info!(
            link,
            tool = w.tool,
            method,
            "browser: the network gate holds another request of an operation whose earlier one \
             was not allowed; declined with it"
        );
        let _ = inner.links.answer(link, id, false);
        return;
    }
    tracing::info!(
        link,
        tool = w.tool,
        method,
        caller = %w.caller.describe(),
        "browser: the network gate holds a request the page sends; asking the owner"
    );
    let unseen = fields.get("reason").and_then(Value::as_str) == Some("unseen_origin");
    let description = if unseen {
        "After Kai acted on the page, the page goes to a site you have never visited, and its \
         address can carry what the page knows. The commit guard holds it: it goes only if you \
         allow it; denied, it is cancelled and the page sees a network error."
    } else {
        "After Kai acted on the page, the page sends this request. The commit guard holds it: it \
         goes only if you allow it; denied, it is cancelled and the page sees a network error."
    };
    let q = Question {
        tool: format!("{} (a request the page sends)", w.tool),
        description: description.to_owned(),
        lines: request_lines(&fields, &w.acts_on),
        notes: Vec::new(),
    };
    let host = fields
        .get("url")
        .and_then(Value::as_str)
        .map(super::host_of)
        .unwrap_or_default();
    let spoken = asked_words::gate(&method, unseen, &host, inner.browser().announce_max_chars);
    let a = tokio::select! {
        a = ask_owner_spoken(&*inner.confirmer, &w.caller, q, Some(spoken), w.deadline, &w.cancel) => a,
        // The extension decided it (its time, the call's cancel): the dialog closes.
        () = w.done.cancelled() => return,
    };
    let allow = a == Answered::Yes;
    if !allow {
        *declined = true;
    }
    tracing::info!(link, tool = w.tool, method, answered = ?a, "browser: the network gate's question answered");
    let _ = inner.links.answer(link, id, allow);
}

/// A held request as the owner's question shows it: the method and the URL (the page's sensitive
/// values masked by the extension), why it is held, where it comes from, and its decoded body,
/// whole.
fn request_lines(f: &Map<String, Value>, acts_on: &str) -> Vec<String> {
    let s = |k: &str| f.get(k).and_then(Value::as_str);
    let mut out = vec![
        format!("after Kai acted on: {acts_on}"),
        format!(
            "request: {} {}",
            shown_text(s("method").unwrap_or_default()),
            shown_text(s("url").unwrap_or_default())
        ),
    ];
    match s("reason") {
        Some("unseen_origin") => out.push(
            "why: it goes to a site you have never visited, and its address can carry what the \
             page knows"
                .into(),
        ),
        Some("write") | None => {}
        Some(other) => out.push(format!("why: {}", quoted(other))),
    }
    if let Some(o) = s("origin") {
        out.push(format!("sent by: {}", shown_text(o)));
    }
    if let Some(t) = s("type") {
        out.push(format!("kind: {}", shown_text(t)));
    }
    out.extend(body_lines(f.get("body")));
    out
}

/// The body of a held request (the contract's Questions), whole.
fn body_lines(body: Option<&Value>) -> Vec<String> {
    let Some(b) = body.filter(|b| !b.is_null()) else {
        return vec!["body: none".into()];
    };
    let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or_default();
    match s("kind") {
        "form" => {
            let fields: Vec<String> = b
                .get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    let text = |i: usize| p.get(i).and_then(Value::as_str).unwrap_or_default();
                    Some(format!("field {}: {}", quoted(text(0)), quoted(text(1))))
                })
                .collect();
            if fields.is_empty() {
                vec!["body: an empty form".into()]
            } else {
                fields
            }
        }
        "json" => vec![format!("body (JSON): {}", shown_text(s("text")))],
        "text" => vec![format!("body (text): {}", shown_text(s("text")))],
        "binary" => {
            let mut out = vec![format!(
                "body: {} bytes Kai cannot show as text",
                b.get("bytes").and_then(Value::as_u64).unwrap_or_default()
            )];
            for f in b
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                out.push(format!("file: {}", shown_text(f)));
            }
            out
        }
        "unreadable" => vec![format!(
            "body: Firefox could not read it ({})",
            shown_text(s("why"))
        )],
        "withheld" => vec![format!(
            "body: not shown, since the page could not mask its passwords in it ({})",
            shown_text(s("why"))
        )],
        _ => vec![format!("body: {}", json_shown(b))],
    }
}

/// Why the commit guard asks about a navigation ([`navigation`]): K15's log line and the
/// dialog's note.
#[derive(Debug, Clone)]
pub struct Asked {
    pub why: &'static str,
    pub note: String,
}

/// `url` names no page: `about:blank`, which no history has and which carries nothing anywhere.
fn blank(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.scheme() == "about" && u.path() == "blank")
}

/// How long ago `ms` (since the epoch) was, in words.
fn ago(ms: i64) -> String {
    let s = (now_ms() - ms).max(0) / 1000;
    match s {
        0..60 => "less than a minute ago".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The commit guard's question before `tab_open` or `tab_go` goes to `d` in the Firefox `at`
/// (module doc of `super`): none for a search, `about:blank`, or a site the user has been to: one
/// they acted on in a tab of that Firefox (the focus rule's facts, `crate::browser::moves`), or
/// one Firefox's history has a visit of theirs to (`history.seen@1`'s `seen_by_user`, asked for
/// the page's real site, `crate::browser::sites::effective`). Asked when the history has none of
/// theirs, when the extension does not say (`seen_by_user` missing), when the URL has no site a
/// history could have seen (`bad_url`: `data:`, Firefox's own pages), and when the history could
/// not be asked.
pub async fn navigation(
    inner: &Inner,
    at: &Seen,
    d: &Destination,
    policy: &BrowserPolicy,
    cancel: &CancellationToken,
) -> Option<Asked> {
    let Destination::Url(url) = d else {
        return None;
    };
    unseen(
        inner,
        at,
        url,
        policy,
        cancel,
        "a URL the model wrote can carry data to a site as it loads.",
    )
    .await
}

/// The commit guard's question before `page_css` adds a style sheet that fetches `url` from
/// another site than the page's (kai-firefox WP7's review): asked as [`navigation`] asks, since a
/// URL the model wrote into the sheet carries data to that site when Firefox fetches it, whatever
/// the tool's state.
pub async fn style_fetch(
    inner: &Inner,
    at: &Seen,
    url: &str,
    policy: &BrowserPolicy,
    cancel: &CancellationToken,
) -> Option<Asked> {
    unseen(
        inner,
        at,
        url,
        policy,
        cancel,
        "the style sheet fetches it, and a URL the model wrote can carry data to a site.",
    )
    .await
}

/// [`navigation`]'s question about `url` in the Firefox `at`, `because` saying why it is asked.
async fn unseen(
    inner: &Inner,
    at: &Seen,
    url: &str,
    policy: &BrowserPolicy,
    cancel: &CancellationToken,
    because: &str,
) -> Option<Asked> {
    if !policy.commit_guard || blank(url) {
        return None;
    }
    let mut origin = origin_of(url);
    if let Ok(l) = &at.listing {
        let moves = inner.moves();
        if l.tabs
            .iter()
            .any(|t| moves.users_origin(&at.link.profile, t).as_deref() == Some(origin.as_str()))
        {
            return None;
        }
    }
    let page = crate::browser::sites::effective(url).map_or_else(|| url.to_owned(), String::from);
    let shown = |o: &str| shown_text(o).into_owned();
    match inner
        .links
        .op(at.link.id, HISTORY_SEEN, json!({"url": page}), cancel)
        .await
    {
        Ok(v) => {
            if let Some(o) = v.get("origin").and_then(Value::as_str) {
                origin = o.to_owned();
            }
            let first = v.get("first_visit").and_then(Value::as_i64);
            match (v.get("seen_by_user").and_then(Value::as_bool), first) {
                (Some(true), _) => None,
                (Some(false), Some(first)) => Some(Asked {
                    note: format!(
                        "Asked because {} is in Firefox's history only through Kai's own visits \
                         (the first {}): {because}",
                        shown(&origin),
                        ago(first)
                    ),
                    why: "the commit guard: a site the user has been to only through Kai",
                }),
                (Some(false), None) => Some(Asked {
                    note: format!(
                        "Asked because Firefox's history has never seen {}: {because}",
                        shown(&origin)
                    ),
                    why: "the commit guard: a site the user's history has never seen",
                }),
                (None, _) => Some(Asked {
                    note: format!(
                        "Asked because Kai for Firefox did not say whether you have been to {} \
                         yourself: {because}",
                        shown(&origin)
                    ),
                    why: "the commit guard: Kai cannot tell whether the user has been there",
                }),
            }
        }
        Err(OpError::Failed { code, .. }) if code == "bad_url" => Some(Asked {
            note: format!(
                "Asked because {} is no page of a site Firefox's history could have seen: \
                 {because}",
                shown(url)
            ),
            why: "the commit guard: a URL without a site the history could have seen",
        }),
        Err(e) => Some(Asked {
            note: format!(
                "Asked because Kai could not ask Firefox's history whether you have been to {} \
                 ({}): {because}",
                shown(&origin),
                said(&e)
            ),
            why: "the commit guard: Kai could not ask Firefox's history",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_body_reads_whole_and_cannot_forge_a_line() {
        assert_eq!(body_lines(None), ["body: none"]);
        assert_eq!(
            body_lines(Some(
                &json!({"kind": "form", "fields": [["item", "trail-lamp"],
                                                               ["note", "a\nCaller: the owner"]]})
            )),
            [
                "field \"item\": \"trail-lamp\"",
                "field \"note\": \"a\\nCaller: the owner\""
            ]
        );
        assert_eq!(
            body_lines(Some(&json!({"kind": "json", "text": "{\"a\":1}"}))),
            ["body (JSON): {\"a\":1}"]
        );
        assert_eq!(
            body_lines(Some(&json!({"kind": "binary", "bytes": 5, "type": null,
                                    "files": ["/tmp/x.pdf"]}))),
            ["body: 5 bytes Kai cannot show as text", "file: /tmp/x.pdf"]
        );
        assert!(
            body_lines(Some(&json!({"kind": "withheld", "why": "gone"})))[0]
                .starts_with("body: not shown")
        );
    }

    /// The answer time leaves the window and the margin; a call with no more time than those is
    /// refused, naming both (paused time: no wall clock decides).
    #[tokio::test(start_paused = true)]
    async fn the_answer_time_leaves_the_window_and_the_margin_or_refuses() {
        assert_eq!(answer_ms(2000, None), Ok(CONTRACT_MS_MAX));
        let soon = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        assert_eq!(answer_ms(2000, Some(soon)), Ok(57_500));
        let e = answer_ms(
            2000,
            Some(tokio::time::Instant::now() + std::time::Duration::from_millis(2500)),
        )
        .unwrap_err();
        assert!(
            e.contains("2500 ms left")
                && e.contains("browser.guard_window_ms")
                && e.contains("500 ms"),
            "{e}"
        );
        assert!(answer_ms(0, Some(tokio::time::Instant::now())).is_err());
        let far = tokio::time::Instant::now() + std::time::Duration::from_secs(10_000_000);
        assert_eq!(answer_ms(0, Some(far)), Ok(CONTRACT_MS_MAX));
    }

    #[test]
    fn the_page_sides_verdicts_read_in_words() {
        let w = |at: &str| {
            reason(&json!({"reason": "word", "word": "buy", "where": at, "text": "Buy now"}))
        };
        assert_eq!(
            w("name"),
            "its name \"Buy now\" holds the guard's word \"buy\""
        );
        assert_eq!(
            w("text"),
            "the text it shows, \"Buy now\", holds the guard's word \"buy\""
        );
        assert_eq!(
            w("value"),
            "its value \"Buy now\" holds the guard's word \"buy\""
        );
        assert_eq!(
            w("title"),
            "its title \"Buy now\" holds the guard's word \"buy\""
        );
        assert!(w("submitter").starts_with("it submits a form whose button \"Buy now\""));
        assert_eq!(
            reason(
                &json!({"reason": "word", "word": "order", "where": "form_action",
                           "text": "https://shop.example/order"})
            ),
            "it submits a form to https://shop.example/order, which holds the guard's word \"order\""
        );
        assert_eq!(
            reason(
                &json!({"reason": "cross_origin_submit", "action": "https://evil.example/x",
                           "origin": "https://evil.example", "method": "post"})
            ),
            "it sends a form (POST) to another site: https://evil.example/x"
        );
        assert_eq!(
            reason(&json!({"reason": "enter_in_editor", "text": "Message"})),
            "it is Enter in the editable field \"Message\", whose page may send what the field holds"
        );
        assert!(reason(&json!({"reason": "later"})).contains("\"later\""));
        assert_eq!(
            verdicts(&json!({"guard": [{"reason": "word"}, {"reason": "enter_in_editor"}]})).len(),
            2
        );
        assert_eq!(verdicts(&json!({"guard": {"reason": "word"}})).len(), 1);
        assert!(verdicts(&json!({"clicked": true})).is_empty());
    }

    #[test]
    fn a_request_to_an_unvisited_site_says_why() {
        let f = json!({"method": "GET", "url": "http://192.0.2.44/upload?d=x", "type": "main_frame",
                       "reason": "unseen_origin", "origin": "https://shop.example", "body": null});
        let lines = request_lines(f.as_object().unwrap(), "52 button \"Go\"");
        assert_eq!(lines[1], "request: GET http://192.0.2.44/upload?d=x");
        assert!(lines[2].starts_with("why: it goes to a site you have never visited"));
    }
}
