//! The tab tools (design §5.2) over `tabs.*@1`: compact lines (K22), each result that shows a
//! page starting with the tab's title and URL (`super::super::tabs::head`), nothing cut. The
//! navigations wait for the page to load (the operations' default `wait`), bounded by the call's
//! cancellation (lmgw's deadline), not by a clock of Kai's.
//!
//! Each marks its move for the focus rule (`crate::browser::moves`) before it sends the operation:
//! a call cancelled while it waits is dropped with its answer, and what Firefox started goes on.
//! With the hands-and-eyes switch on, a call of a turn the owner started marks it as the owner's
//! own act once it answered (`super::owners_move`, kai-firefox WP10b and its review): a switch and
//! the tab Firefox shows after a close are the owner's activation; the tab Kai opened and where a
//! navigation went are the owner's site only where the tab landed on the origin it was meant for
//! (`tab_go`'s and `tab_open`'s URL, else the extension's `goes`: a search's engine, the history
//! entry of `tab_back` and `tab_forward`). A redirect to another origin stays Kai's navigation.
//! `tab_open {sandbox: true}` opens the tab in Kai's own container.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::act::Goes;
use super::{Destination, Inner, Target, look, op, owners_move, read, said, target};
use crate::browser::moves::origin;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head};
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

/// The answer of a navigation (`tabs.open`, `go`, `back`, `forward`, `reload`).
#[derive(Debug, Deserialize)]
struct Navigation {
    tab: Tab,
    #[serde(default)]
    loaded: bool,
    #[serde(default)]
    load_error: Option<String>,
    /// Where it was meant to go (the contract's `goes`).
    #[serde(default)]
    goes: Option<Goes>,
}

/// The navigation landed where it was meant to (module doc): on the origin of `d`'s URL, else the
/// one the extension says it went for (a search's engine, the history entry).
fn landed_as_meant(nav: &Navigation, d: Option<&Destination>) -> bool {
    match d {
        Some(Destination::Url(u)) => origin(u) == origin(&nav.tab.url),
        _ => nav.goes.as_ref().is_some_and(|g| g.landed_at(&nav.tab.url)),
    }
}

/// `{"tab": Tab}` (`tabs.switch`, `tabs.mute`).
#[derive(Debug, Deserialize)]
struct OneTab {
    tab: Tab,
}

fn reply(r: Result<String, String>) -> Reply {
    match r {
        Ok(text) => Reply::text(text),
        Err(e) => Reply::error(e),
    }
}

/// A navigation's line: the tab as it is now, what was done, and whether the page loaded.
fn navigated(nav: &Navigation, done: &str, sites: &SiteRules) -> String {
    let head = head(&nav.tab, sites);
    match (&nav.load_error, nav.loaded) {
        (Some(e), _) => format!("{head}: {done}; the page did not load: {}", load_error(e)),
        (None, true) => format!("{head}: {done}"),
        (None, false) => format!("{head}: {done}; the page is still loading"),
    }
}

/// `load_error` in words, with Firefox's name for it.
pub(super) fn load_error(code: &str) -> String {
    let words = match code {
        "NS_ERROR_CONNECTION_REFUSED" => "the server refused the connection",
        "NS_ERROR_UNKNOWN_HOST" => "no such host",
        "NS_ERROR_NET_TIMEOUT" => "the server did not answer in time",
        "NS_ERROR_OFFLINE" => "Firefox is offline",
        "NS_ERROR_NET_RESET" => "the connection was reset",
        "NS_ERROR_NET_INTERRUPT" => "the connection was interrupted",
        "SSL_ERROR_BAD_CERT_DOMAIN" => "the site's certificate is for another name",
        "NS_BINDING_ABORTED" => "the load was stopped, or another navigation replaced it",
        "ERROR_PAGE" => "Firefox shows its error page",
        "NO_PAGE" => {
            "Firefox showed no new page (a download, perhaps); the tab shows what it showed before"
        }
        other => return format!("Firefox's error {}", shown_text(other)),
    };
    format!("{words} ({code})")
}

/// `tab_list`: one line per tab, each linked profile under its own heading when there are two.
pub fn list(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        let sites = inner.browser().sites;
        let seen = look(&inner, &call.cancel).await;
        if seen.is_empty() {
            return Reply::error("no Firefox with Kai for Firefox is linked");
        }
        let mut out = Vec::new();
        for s in &seen {
            if seen.len() > 1 {
                out.push(format!(
                    "profile {} (Firefox {}):",
                    shown_text(&s.link.profile),
                    shown_text(&s.link.firefox)
                ));
            }
            let listing = match &s.listing {
                Ok(l) => l,
                Err(e) => {
                    out.push(said(e));
                    continue;
                }
            };
            let mut windows: Vec<i64> = listing.tabs.iter().map(|t| t.window).collect();
            windows.sort_unstable();
            windows.dedup();
            let mut unreached = 0;
            for t in &listing.tabs {
                if sites.rule(&t.url) == Rule::None {
                    unreached += 1;
                } else {
                    out.push(list_line(t, windows.len() > 1));
                }
            }
            if listing.tabs.is_empty() {
                // No normal window Kai sees: none is open, or only private ones.
                out.push(
                    "no tabs: Firefox has no window open, or only private windows, which Kai has \
                     no access to"
                        .into(),
                );
            }
            if unreached > 0 {
                out.push(format!(
                    "{unreached} more {} on sites Kai does not reach (Kai's site rules)",
                    if unreached == 1 { "tab" } else { "tabs" }
                ));
            }
            if listing.user_tab.is_none() && !listing.tabs.is_empty() {
                out.push("the user is in a private window, which Kai has no access to".to_owned());
            }
        }
        Reply::text(out.join("\n"))
    })
}

/// `<id> "<title>" <URL> (<state>)`: the window only where there are several, `active` for the
/// front tab of a window that is not the user's.
fn list_line(t: &Tab, windows: bool) -> String {
    let mut state = Vec::new();
    if windows {
        state.push(format!("window {}", t.window));
    }
    if t.user {
        state.push("the user's tab".to_owned());
    } else if t.active {
        state.push("active".to_owned());
    }
    for (on, word) in [
        (t.pinned, "pinned"),
        (t.audible, "playing sound"),
        (t.muted, "muted"),
        (t.loading, "loading"),
        (t.discarded, "unloaded"),
    ] {
        if on {
            state.push(word.to_owned());
        }
    }
    match (&t.container_name, &t.container) {
        (Some(n), _) => state.push(format!("container {}", quoted(n))),
        (None, Some(c)) => state.push(format!("container {}", shown_text(c))),
        (None, None) => {}
    }
    let line = format!("{} {} {}", t.id, quoted(&t.title), shown_text(&t.url));
    if state.is_empty() {
        line
    } else {
        format!("{line} ({})", state.join(", "))
    }
}

/// The destination's words for the line: a search names its text.
fn where_to(d: &Destination, done: &str) -> String {
    match d {
        Destination::Url(_) => done.to_owned(),
        Destination::Search(s) => format!("{done}, a search for {}", quoted(s)),
    }
}

/// A `tab_open` Kai sent and has not seen the end of (`Moves::opening`). Dropped without its tab
/// (an error, or a cancelled call whose future the host drops), it tells the moves that the call
/// is over and looks at the tabs at once, so every tab that was not there before is Kai's.
struct Opening {
    inner: Arc<Inner>,
    profile: String,
    ticket: u64,
    named: bool,
}

impl Opening {
    fn new(inner: &Arc<Inner>, t: &Target) -> Self {
        let ticket = inner.moves().opening(&t.link.profile, t.before.clone());
        Self {
            inner: inner.clone(),
            profile: t.link.profile.clone(),
            ticket,
            named: false,
        }
    }

    fn opened(mut self, tab: i64) {
        self.inner.moves().opened(&self.profile, self.ticket, tab);
        self.named = true;
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        if self.named {
            return;
        }
        self.inner.moves().open_ended(&self.profile, self.ticket);
        // Bounded by the link, like every operation of Kai's: the list answers or the link ends.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let inner = self.inner.clone();
            rt.spawn(async move {
                look(&inner, &CancellationToken::new()).await;
            });
        }
    }
}

/// `tab_open`: a new tab in the user's window of the prepared Firefox.
pub fn open(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let d = t.destination.clone().ok_or("tab_open needs a url")?;
                let (key, value) = d.arg();
                let mut args = serde_json::Map::new();
                args.insert(key.into(), value.into());
                let sandbox = call.args.boolean("sandbox") == Some(true);
                if sandbox {
                    args.insert("sandbox".into(), true.into());
                }
                let opening = Opening::new(&inner, &t);
                let v = op(
                    &inner,
                    &t.link,
                    "tabs.open@1",
                    Value::Object(args),
                    &call.cancel,
                )
                .await?;
                let nav: Navigation = read("tabs.open@1", v)?;
                opening.opened(nav.tab.id);
                if owners_move(&inner, &call) && landed_as_meant(&nav, Some(&d)) {
                    inner
                        .moves()
                        .owned(&t.link.profile, nav.tab.id, Some(&nav.tab.url));
                }
                let done = if sandbox {
                    "opened in a new tab in Kai's own container"
                } else {
                    "opened in a new tab"
                };
                Ok(navigated(&nav, &where_to(&d, done), &inner.browser().sites))
            }
            .await,
        )
    })
}

/// The prepared tab as it is now: after the owner was asked, Kai lists the tabs again and refuses
/// one that closed or went to another site while the dialog waited; unasked, the tab as prepared.
pub(super) async fn current(inner: &Inner, call: &Call, t: &Target) -> Result<Tab, String> {
    let tab = t.tab.as_ref().ok_or("no tab was prepared")?;
    if !call.prepared.as_ref().is_some_and(|p| p.confirmed) {
        return Ok(tab.clone());
    }
    let tool = call.spec.name;
    let seen = look(inner, &call.cancel).await;
    let listing = match seen.iter().find(|s| s.link.id == t.link.id) {
        None => {
            return Err(format!(
                "Firefox closed while the owner was asked; {tool} did not run"
            ));
        }
        Some(s) => s.listing.as_ref().map_err(said)?,
    };
    let sites = inner.browser().sites;
    let Some(now) = listing.tabs.iter().find(|n| n.id == tab.id) else {
        return Err(format!(
            "tab {} closed while the owner was asked; {tool} did not run",
            tab.id
        ));
    };
    if origin(&now.url) != origin(&tab.url) {
        return Err(format!(
            "{} went to another site than the dialog showed while the owner was asked; {tool} \
             did not run",
            head(now, &sites)
        ));
    }
    Ok(now.clone())
}

/// A navigation of the prepared tab: `op` with `extra` arguments. Kai's move is marked before it is
/// sent, and where it went once it answered.
async fn navigate(
    inner: &Inner,
    call: &Call,
    op_name: &str,
    extra: serde_json::Map<String, Value>,
    done: &str,
) -> Result<String, String> {
    let t: Arc<Target> = target(call)?;
    let tab = current(inner, call, &t).await?;
    let mut args = extra;
    args.insert("tab".into(), tab.id.into());
    let moves = op_name != "tabs.reload@1";
    if moves {
        inner.moves().navigated(&t.link.profile, tab.id, &tab.url);
    }
    let v = op(inner, &t.link, op_name, Value::Object(args), &call.cancel).await?;
    let nav: Navigation = read(op_name, v)?;
    if moves {
        inner.moves().landed(&t.link.profile, tab.id);
        if owners_move(inner, call) && landed_as_meant(&nav, t.destination.as_ref()) {
            inner
                .moves()
                .owned(&t.link.profile, tab.id, Some(&nav.tab.url));
        }
    }
    Ok(navigated(&nav, done, &inner.browser().sites))
}

/// `tab_go`.
pub fn go(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let d = t.destination.clone().ok_or("tab_go needs a url")?;
                let (key, value) = d.arg();
                let mut extra = serde_json::Map::new();
                extra.insert(key.into(), value.into());
                navigate(
                    &inner,
                    &call,
                    "tabs.go@1",
                    extra,
                    &where_to(&d, "went there"),
                )
                .await
            }
            .await,
        )
    })
}

/// `tab_back`.
pub fn back(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            navigate(
                &inner,
                &call,
                "tabs.back@1",
                Default::default(),
                "went back",
            )
            .await,
        )
    })
}

/// `tab_forward`.
pub fn forward(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            navigate(
                &inner,
                &call,
                "tabs.forward@1",
                Default::default(),
                "went forward",
            )
            .await,
        )
    })
}

/// `tab_reload`.
pub fn reload(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            navigate(
                &inner,
                &call,
                "tabs.reload@1",
                Default::default(),
                "reloaded",
            )
            .await,
        )
    })
}

/// `tab_switch`: the tab becomes the active one of its window and the window is focused, which
/// makes it the user's tab, unless the desktop keeps the window from taking the focus (KWin's
/// focus-stealing prevention): the answer's `user` says which, and the line says so.
pub fn switch(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let tab = current(&inner, &call, &t).await?;
                let owners = owners_move(&inner, &call);
                if owners {
                    inner.moves().switching_for_owner(&t.link.profile, tab.id);
                } else {
                    inner.moves().brought(&t.link.profile, tab.id);
                }
                let v = op(
                    &inner,
                    &t.link,
                    "tabs.switch@1",
                    json!({"tab": tab.id}),
                    &call.cancel,
                )
                .await?;
                let now: OneTab = read("tabs.switch@1", v)?;
                if now.tab.user && owners {
                    inner.moves().owned(&t.link.profile, tab.id, None);
                }
                let head = head(&now.tab, &inner.browser().sites);
                Ok(if now.tab.user {
                    format!("{head}: in front now")
                } else {
                    format!(
                        "{head}: the active tab of its window, but that window did not get the \
                         focus (the desktop may have kept it from taking it), so it is not the \
                         user's tab"
                    )
                })
            }
            .await,
        )
    })
}

/// `tab_close`.
pub fn close(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                #[derive(Deserialize)]
                struct Closed {
                    closed: Tab,
                }
                let t = target(&call)?;
                let tab = current(&inner, &call, &t).await?;
                let owned = owners_move(&inner, &call);
                inner
                    .moves()
                    .closing(&t.link.profile, tab.id, tab.user, owned);
                // A close that fails, or whose call ends unanswered, hands the next tab to nobody
                // (`Moves::close_failed`).
                let failed = CloseFailed {
                    inner: inner.clone(),
                    profile: t.link.profile.clone(),
                    armed: owned,
                };
                let v = op(
                    &inner,
                    &t.link,
                    "tabs.close@1",
                    json!({"tab": tab.id}),
                    &call.cancel,
                )
                .await?;
                let gone: Closed = read("tabs.close@1", v)?;
                failed.disarm();
                inner.moves().closed(&t.link.profile, tab.id);
                // The tab Firefox shows in its place, while it is still the one (`Moves::closing`).
                look(&inner, &call.cancel).await;
                Ok(format!(
                    "{}: closed",
                    head(&gone.closed, &inner.browser().sites)
                ))
            }
            .await,
        )
    })
}

/// The owner's close of their tab (`Moves::closing`) until it answered: dropped before (an
/// error, a refusal, the call's future dropped by the host), the tab Firefox shows next is not the
/// owner's own act.
struct CloseFailed {
    inner: Arc<Inner>,
    profile: String,
    armed: bool,
}

impl CloseFailed {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CloseFailed {
    fn drop(&mut self) {
        if self.armed {
            self.inner.moves().close_failed(&self.profile);
        }
    }
}

/// `tab_mute`.
pub fn mute(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let tab = current(&inner, &call, &t).await?;
                let on = call.args.boolean("on").unwrap_or(true);
                let v = op(
                    &inner,
                    &t.link,
                    "tabs.mute@1",
                    json!({"tab": tab.id, "on": on}),
                    &call.cancel,
                )
                .await?;
                let now: OneTab = read("tabs.mute@1", v)?;
                let state = if now.tab.muted { "muted" } else { "not muted" };
                Ok(format!(
                    "{}: {state}",
                    head(&now.tab, &inner.browser().sites)
                ))
            }
            .await,
        )
    })
}
