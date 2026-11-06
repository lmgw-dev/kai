//! The trusted path (kai-firefox design §6.3): where a page takes only a real key (the extension's
//! `needs_trusted`, or every press with `browser.trusted_press = "always"`), Kai sends Enter or
//! Space through the desktop, in this order:
//! 1. **the decision** ([`step`]): under `key_press`'s policy state for the call's caller (K24:
//!    confirmed by default), the dialog naming the tab, the element and the key; with the
//!    hands-and-eyes switch on, `element_press`'s own state, which let the call run, decides it.
//!    A call approved in lmgw (by anyone, the owner in a thread included) is asked at Kai's desk
//!    unless the deciding tool is set to `allow`: its approver saw only a ref. Where the commit
//!    guard stopped an always-trusted press too, one dialog carries both (`super::act`);
//! 2. **no overlay of Kai's holds the keyboard**: while the approval card does (it takes Enter),
//!    Kai waits until it lets go, for as long as the call runs;
//! 3. **the input turn** ([`Desk::turn`]): one input call at a time, taken before anything is
//!    looked at, so no other input call's keys come between the focus check and Kai's key;
//! 4. **the spans** (`crate::browser::spans`): a look at every linked Firefox's tabs, then an
//!    input span in each, so Kai's window activation and whatever Firefox gets until the spans
//!    end is Kai's, never the user's act;
//! 5. **Firefox's window to the front** ([`Desk::activate`]): the window of the link's Firefox
//!    process whose title is the tab's (`crate::browser::desk`);
//! 6. **the focus check** (`focus.check@1` with a guard, the commit guard's, or with the guard off
//!    one that watches only the key, so the gate opens either way), right before the key and
//!    after every question: the element must have the keyboard focus, else nothing is pressed and
//!    Kai says where the focus is. The gate's questions wait until the key went;
//! 7. **right before the key**: the call is not cancelled and the screen is not locked;
//! 8. **the key** ([`Desk::press`]) through Kai's Input capability, the input tools' path, in the
//!    turn: the focus guard on Firefox's window and the approval card;
//! 9. **the gate** (`gate.result@1`): what the requests after the key became, which element the
//!    key reached (`key_seen`), and whether the gate had expired (then the key went unguarded);
//! 10. **the spans end** (`settle_ms` 0: the gate's window waited), and a look: a tab the key
//!     opened, and the tab it brought to the front, are Kai's (`crate::browser::moves`).
//!
//! However the path ends once the gate is open (an error, a refusal, the call's future dropped),
//! the gate is read with a token of its own and then the spans end ([`Closing`]): a gate never
//! stays open, and a span never outlives the press.
//!
//! **Through accessibility first** ([`accessible`], kai-firefox WP12): where the desktop offers
//! accessibility, the link knows its Firefox's process and the extension serves the mark and the
//! gate without a focus check, Kai presses the element with AT-SPI's `doAction` instead of a key,
//! a click Firefox makes trusted and user-activated (kai-firefox WP0 (d)), with no window brought
//! to the front and no key on the desktop. The decision (1.) is the same, its question saying how;
//! then the turn and the spans, the tab's own window, the element marked, found in that window's
//! page by its mark and unmarked, checked against the extension's facts, `gate.open@1` on its
//! document, the screen lock, the cancel and the tab's window checked, the press, `gate.result@1`
//! with `click_seen`, the spans' end. An element Kai does not find there (Firefox not on the bus,
//! the tab not in front of its window, the element not exposed) takes the key's path (2. on); a
//! mark on two elements, or an element that is not the extension's, ends the press; a press that
//! was sent is never sent again by the key.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::windows::Window;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod accessible;

use super::guard::{self, Question, Questions, Watching, gate_arg};
use super::lines::{Element, element};
use super::page::withhold;
use super::{Inner, Target, answered, failure, look, read};
use crate::browser::links::{LinkId, OpError};
use crate::browser::moves::now_ms;
use crate::browser::spans::Open;
use crate::browser::tabs::Tab;
use crate::host::policy::Decision;
use crate::host::registry::Call;
use crate::host::settings::ToolState;

pub const FOCUS_CHECK: &str = "focus.check@1";
pub const GATE_RESULT: &str = "gate.result@1";
pub use accessible::{GATE_OPEN, UNMARK};

/// How often Kai looks again whether the approval card still holds the keyboard.
const OVERLAY_POLL: Duration = Duration::from_millis(25);

/// A future of the desktop's.
pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The input turn ([`Desk::turn`]), held until dropped.
pub type Turn = Box<dyn Send + Sync>;

/// The key Kai sends (the extension's `key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Space,
}

impl Key {
    /// The extension's word for it; Enter for any other.
    pub fn of(word: Option<&str>) -> Self {
        match word {
            Some("Space") => Key::Space,
            _ => Key::Enter,
        }
    }

    /// The key a role takes (the extension's rule: Space for a checkbox, a radio or a switch),
    /// where the extension has not named it yet.
    pub fn for_role(role: &str) -> Self {
        match role {
            "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio" => Key::Space,
            _ => Key::Enter,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Key::Enter => "Enter",
            Key::Space => "Space",
        }
    }
}

/// The desktop's part of the trusted path (module doc); the tests' is a fake.
pub trait Desk: Send + Sync + 'static {
    /// One of Kai's overlays (the approval card) holds the keyboard now.
    fn overlay_holds(&self) -> bool;
    /// The screen is locked: no key goes.
    fn locked(&self) -> bool;
    /// The call's turn among the input calls, within its time.
    fn turn<'a>(&'a self, call: &'a Call) -> Fut<'a, Result<Turn, String>>;
    /// Brings the window of the Firefox whose main process is `firefox` and whose active tab is
    /// titled `title` to the front and waits until it has the keyboard focus: that window, as the
    /// desktop names it.
    fn activate<'a>(
        &'a self,
        firefox: Option<u32>,
        title: &'a str,
    ) -> Fut<'a, Result<Window, String>>;
    /// Presses `key` in `window`, which must have the keyboard focus, through Kai's Input
    /// capability, in the turn the caller holds.
    fn press<'a>(&'a self, window: &'a Window, key: Key) -> Fut<'a, Result<(), String>>;
    /// The desktop's accessibility, where it offers one: the press through accessibility
    /// ([`accessible`]) and Firefox's own UI (`super::chrome`).
    fn access(&self) -> Option<Arc<dyn kai_desktop::caps::accessibility::Accessibility>> {
        None
    }
}

/// A desk without a desktop (a provider built for the tests' listing only).
#[cfg(test)]
pub struct NoDesk;

#[cfg(test)]
impl Desk for NoDesk {
    fn overlay_holds(&self) -> bool {
        false
    }

    fn locked(&self) -> bool {
        false
    }

    fn turn<'a>(&'a self, _: &'a Call) -> Fut<'a, Result<Turn, String>> {
        Box::pin(async { Ok(Box::new(()) as Turn) })
    }

    fn activate<'a>(&'a self, _: Option<u32>, _title: &'a str) -> Fut<'a, Result<Window, String>> {
        Box::pin(async { Err("Kai has no desktop to press a real key on".to_owned()) })
    }

    fn press<'a>(&'a self, _: &'a Window, _: Key) -> Fut<'a, Result<(), String>> {
        Box::pin(async { Err("Kai has no desktop to press a real key on".to_owned()) })
    }
}

/// Why an element takes only a real key, in words (the extension's `needs_trusted`).
pub fn needs(kind: &str) -> String {
    match kind {
        "new_window" => {
            "a link that opens a new window, which Firefox opens only on a real key or \
                         click"
                .into()
        }
        "media" => "a media control, which plays only on a real key or click".into(),
        "fullscreen" => "a full-screen control, which works only on a real key or click".into(),
        "always" => "Kai sends every press as a real key (browser.trusted_press)".into(),
        other => format!("Kai for Firefox says it needs a real key ({other})"),
    }
}

/// How the trusted step is decided (module doc, 1.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Allow,
    /// Refused, by the deciding tool's policy.
    Deny {
        tool: &'static str,
        why: String,
    },
    /// Asked at Kai's desk, and why.
    Ask(&'static str),
}

/// The trusted step's decision for the prepared call (module doc, 1.).
pub fn step(inner: &Inner, call: &Call, t: &Target) -> Step {
    let policy = inner.policy.read().expect("never poisoned");
    let hands = policy.browser().hands_and_eyes;
    let tool = if hands { "element_press" } else { "key_press" };
    let Some((_, spec)) = kai_desktop::contract::find(tool) else {
        return Step::Deny {
            tool,
            why: format!("this Kai has no {tool}"),
        };
    };
    // With the switch on, element_press's own state let the call run.
    let decision = if hands {
        Decision::Allow
    } else {
        policy.decide(spec, &call.caller, None)
    };
    match decision {
        Decision::Deny(why) => Step::Deny { tool, why },
        _ if t.approved && policy.state(tool) != Some(ToolState::Allow) => Step::Ask(
            "a real key for a call approved in lmgw, whose approver saw only the element's ref",
        ),
        Decision::Allow => Step::Allow,
        Decision::Confirm { why } => Step::Ask(why),
    }
}

/// How the trusted press went: a key through the desktop, or a click through accessibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Key(Key),
    Click,
}

/// What the trusted press did (`gate.result@1`).
pub struct Pressed {
    pub via: Via,
    pub requests: Option<Value>,
    /// The gate had closed before Kai read it (the extension's `expired`): the requests after
    /// the key went unasked.
    pub expired: bool,
    /// Where the page saw the key (`key_seen`), in words; `None` where it reached the element.
    pub seen: Option<String>,
}

/// The trusted press's word on where its key went (`key_seen` of `gate.result@1`, renumbered);
/// `None`: the element got it.
fn key_seen(v: &Value, aimed: i64, key: Key) -> Option<String> {
    let Some(seen) = v.get("key_seen").filter(|s| !s.is_null()) else {
        return Some(
            "the page did not see the key: it went elsewhere (Firefox's own bar or sidebar, \
             another frame or window) or came after the watch"
                .into(),
        );
    };
    let name = seen.get("key").and_then(Value::as_str).unwrap_or_default();
    let other_key = (name != key.name()).then(|| {
        format!(
            "; the first key the page got was {}, not Kai's {} (someone typed meanwhile?)",
            crate::host::confirm::quoted(name),
            key.name()
        )
    });
    let at = match seen.get("element") {
        None | Some(Value::Null) => Some("the page's body got the key, not the element".into()),
        Some(e) if e.get("withheld").is_some() => Some(
            "an element in a frame of a site Kai does not reach got the key, not the element"
                .into(),
        ),
        Some(e) if e.get("ref").and_then(Value::as_i64) == Some(aimed) => None,
        Some(e) => Some(format!(
            "another element got the key: {}",
            element(&read::<Element>(GATE_RESULT, e.clone()).unwrap_or_default())
        )),
    };
    match (at, other_key) {
        (None, None) => None,
        (Some(a), o) => Some(format!("{a}{}", o.unwrap_or_default())),
        (None, Some(o)) => Some(format!("the element got a key{o}")),
    }
}

/// The trusted key's question (module doc, 1.); `click`: the press goes through accessibility
/// where Kai finds the element there.
fn question(head: &str, what: &str, key: Key, why: &str, click: bool) -> Question {
    if click {
        return Question {
            tool: "key_press (a real press in Firefox, for element_press)".to_owned(),
            description: format!(
                "Kai presses an element that takes only a real press ({why}): a click through \
                 accessibility, which Firefox counts as the user's; where Kai cannot find the \
                 element there, {} through the desktop, with Firefox's window to the front first.",
                key.name()
            ),
            lines: vec![
                format!("tab: {head}"),
                format!("element: {what}"),
                format!("how: a click through accessibility, else {}", key.name()),
            ],
            notes: Vec::new(),
        };
    }
    Question {
        tool: format!("key_press ({} in Firefox, for element_press)", key.name()),
        description: format!(
            "Kai presses {} through the desktop on an element that takes only a real key ({why}); \
             Firefox's window comes to the front first.",
            key.name()
        ),
        lines: vec![
            format!("tab: {head}"),
            format!("element: {what}"),
            format!("key: {}", key.name()),
        ],
        notes: Vec::new(),
    }
}

/// The trusted path for the prepared element `what` in the tab `tab` (as `element.press@1` left
/// it), `head` its line (module doc); `why` says why it takes a real key; `confirmed`: the key
/// the owner allowed already in the commit guard's dialog (`super::act`); `spoken`: the key's
/// question in Kai's words, which Kai speaks for a call of its own (kai-firefox WP10b's review).
#[allow(clippy::too_many_arguments)]
pub async fn press(
    inner: &Arc<Inner>,
    call: &Call,
    t: &Target,
    tab: &Tab,
    head: &str,
    what: &str,
    key: Key,
    why: &str,
    confirmed: Option<Key>,
    spoken: Option<crate::voice::speak::Words>,
) -> Result<Pressed, String> {
    let refused = |because: &str| format!("{what} was not pressed: {because}");
    let via = accessible::usable(inner, t);
    // 1. The decision.
    match step(inner, call, t) {
        Step::Allow => {}
        Step::Deny { tool, why: d } => {
            return Err(refused(&format!(
                "it takes a real key ({why}), and {tool}, whose policy decides that, is refused: \
                 {d}"
            )));
        }
        Step::Ask(_) if confirmed == Some(key) => {}
        Step::Ask(asked) => {
            tracing::info!(
                caller = %call.caller.describe(),
                why = asked,
                "browser: asking the owner before a real key in Firefox"
            );
            let a = guard::ask_owner_spoken(
                &*inner.confirmer,
                &call.caller,
                question(head, what, key, why, via.is_some()),
                spoken,
                call.deadline,
                &call.cancel,
            )
            .await;
            if a != guard::Answered::Yes {
                return Err(refused(&format!(
                    "it takes a real key ({why}), and {} (key_press's question)",
                    a.why()
                )));
            }
        }
    }
    // Through accessibility first (module doc); an element not found there takes the key.
    if let Some(via) = &via
        && let Some(done) = accessible::press(inner, call, t, tab, what, via).await?
    {
        return Ok(done);
    }
    // 2. No overlay of Kai's holds the keyboard.
    while inner.desk.overlay_holds() {
        tokio::select! {
            () = tokio::time::sleep(OVERLAY_POLL) => {}
            () = call.cancel.cancelled() => {
                return Err(refused("the call was cancelled while Kai's approval card held the keyboard"));
            }
        }
    }
    // 3. The input turn.
    let turn = inner.desk.turn(call).await.map_err(|e| refused(&e))?;
    // 4. The tabs before, then the spans.
    let before = look(inner, &call.cancel).await;
    let span = inner.spans.begin("the trusted press", &call.cancel).await;
    let mut closing = Closing {
        inner: inner.clone(),
        link: t.link.id,
        check: None,
        gate: None,
        read: None,
        watching: None,
        span: Some(span),
        mark: None,
    };
    // 5. Firefox's window to the front: the one titled after the tab as the press left it.
    let window = inner
        .desk
        .activate(t.link.firefox_pid, &tab.title)
        .await
        .map_err(|e| refused(&e))?;
    // 6. The focus check right before the key, opening the gate. It runs to its answer whatever
    // becomes of the call, so Kai always learns the gate it opened ([`Closing`]).
    let aimed = t
        .element
        .ok_or("element_press was prepared without its element")?;
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
    let link = t.link.id;
    let args = json!({"ref": aimed.ext, "guard": gate_guard});
    let check = closing.check.insert(tokio::spawn(async move {
        links
            .op_as(id, link, FOCUS_CHECK, args, &CancellationToken::new())
            .await
    }));
    let checked = check
        .await
        .unwrap_or_else(|e| Err(OpError::failed("internal", e.to_string())));
    closing.check = None;
    let checked = match checked {
        Ok(v) => {
            closing.gate = v.get("gate").and_then(Value::as_u64);
            answered(inner, call, t, v)?.value
        }
        Err(e) => return Err(failure(inner, &t.link, e, Some(&aimed), &call.cancel).await),
    };
    if checked.get("focused").and_then(Value::as_bool) != Some(true) {
        return Err(refused(&format!(
            "{}; Kai sends a real key only to the element it checked",
            focus_words(&checked)
        )));
    }
    // 7. Right before the key.
    if call.cancel.is_cancelled() {
        return Err(refused("the call was cancelled before the key"));
    }
    if inner.desk.locked() {
        return Err(refused(
            "the screen locked, and Kai sends no key while it is",
        ));
    }
    // 8. The key, in the turn.
    let pressed = inner.desk.press(&window, key).await;
    drop(turn);
    if let Some(w) = &closing.watching {
        w.release();
    }
    // 9. The gate, read even when the key failed, to its answer whatever becomes of the call.
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
    // 10. The spans end, and what the key brought to the front is Kai's.
    if let Some(open) = closing.span.take() {
        open.end(0).await;
    }
    let after = look(inner, &CancellationToken::new()).await;
    inner.moves().raised(&before, &after, now_ms());
    pressed.map_err(|e| refused(&e))?;
    let mut done = Pressed {
        via: Via::Key(key),
        requests: None,
        expired: false,
        seen: None,
    };
    match result {
        None => {}
        Some(Ok(v)) => {
            done.expired = v.get("expired").and_then(Value::as_bool) == Some(true);
            done.requests = v.get("requests").cloned();
            done.seen = key_seen(&v, aimed.kai, key);
        }
        Some(Err(words)) => {
            return Err(format!(
                "pressed {what} with {} through the desktop, but Kai could not read what became \
                 of the page's requests after it, nor which element got the key: {words}",
                key.name()
            ));
        }
    }
    Ok(done)
}

/// What the trusted path leaves open (module doc): the focus check in flight or the gate it
/// opened, the gate's read in flight, its questions' registration and the spans. Dropped with any
/// of them still open (an error, a refusal, the call's future dropped by the host), it reads the
/// gate with a token of its own (once the focus check answered, so its gate is known), and then
/// ends the spans, in the background.
struct Closing {
    inner: Arc<Inner>,
    link: LinkId,
    check: Option<JoinHandle<Result<Value, OpError>>>,
    gate: Option<u64>,
    read: Option<JoinHandle<Result<Value, OpError>>>,
    watching: Option<Watching>,
    span: Option<Open>,
    /// The press through accessibility: the mark set and not yet taken away ([`accessible`]).
    mark: Option<String>,
}

impl Drop for Closing {
    fn drop(&mut self) {
        let (check, mut gate, read) = (self.check.take(), self.gate.take(), self.read.take());
        let (watching, span, mark) = (self.watching.take(), self.span.take(), self.mark.take());
        if check.is_none() && gate.is_none() && read.is_none() && span.is_none() && mark.is_none() {
            return;
        }
        if let Some(w) = &watching {
            w.release();
        }
        let links = self.inner.links.clone();
        let link = self.link;
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            if let Some(value) = mark {
                let _ = links
                    .op(
                        link,
                        UNMARK,
                        json!({"value": value}),
                        &CancellationToken::new(),
                    )
                    .await;
            }
            if let Some(h) = check
                && let Ok(Ok(v)) = h.await
            {
                gate = v.get("gate").and_then(Value::as_u64);
            }
            if let Some(h) = read {
                // A read under way closes the gate itself.
                let _ = h.await;
                gate = None;
            }
            if let Some(g) = gate {
                let r = links
                    .op(
                        link,
                        GATE_RESULT,
                        json!({"gate": g}),
                        &CancellationToken::new(),
                    )
                    .await;
                tracing::info!(
                    link,
                    read = r.is_ok(),
                    "browser: the trusted press ended early; its gate was read"
                );
            }
            drop(watching);
            if let Some(open) = span {
                open.end(0).await;
            }
        });
    }
}

/// Where `focus.check@1` says the keyboard focus is, in words.
fn focus_words(v: &Value) -> String {
    match v.get("focus").and_then(Value::as_str).unwrap_or_default() {
        "other_tab" => "its tab is not the active tab of its window (tab_switch brings it to the \
                        front)"
            .into(),
        "other_window" => "another Firefox window has the keyboard focus".into(),
        "outside" => "Firefox does not have the keyboard focus: another application or a private \
                      window has it"
            .into(),
        "firefox" => "the sidebar or address bar has the keyboard focus".into(),
        "element" => match v.get("element") {
            Some(e) if e.get("withheld").is_some() => {
                "an element in a frame of a site Kai does not reach has the keyboard focus".into()
            }
            Some(e) => format!(
                "another element of the page has the keyboard focus: {}",
                element(&read::<Element>(FOCUS_CHECK, e.clone()).unwrap_or_default())
            ),
            None => "another element of the page has the keyboard focus".into(),
        },
        "nothing" => "no element of the page has the keyboard focus".into(),
        other => format!(
            "the element does not have the keyboard focus (Kai for Firefox says {})",
            crate::host::confirm::quoted(other)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_key_went_reads_in_words() {
        let seen = |v: Value| key_seen(&json!({"key_seen": v}), 52, Key::Enter);
        assert_eq!(
            seen(json!({"key": "Enter", "element": {"ref": 52, "role": "button", "name": "Go"}})),
            None
        );
        assert!(
            seen(Value::Null)
                .unwrap()
                .starts_with("the page did not see the key")
        );
        assert_eq!(
            seen(
                json!({"key": "Enter", "element": {"ref": 61, "role": "textbox", "name": "Notes"}})
            )
            .unwrap(),
            "another element got the key: 61 textbox \"Notes\""
        );
        assert!(
            seen(json!({"key": "Enter", "element": null}))
                .unwrap()
                .starts_with("the page's body")
        );
        assert!(
            seen(
                json!({"key": "character", "element": {"ref": 52, "role": "button", "name": "Go"}})
            )
            .unwrap()
            .contains("the first key the page got was \"character\", not Kai's Enter")
        );
    }
}
