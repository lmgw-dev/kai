//! Kai's site rules over the desktop's tools (kai-firefox design §6.4, kai-firefox WP7b): what a
//! window on the desktop shows of Firefox, so the desktop module's tools that capture, read or act
//! on a window do not reach a site the owner keeps from Kai. They are K24's `see_window`,
//! `see_screen`, `read_window`, `read_selection`, `ui_press`, `type_text` and `key_press`, and
//! through them `look` and `find`, whose delegates call the same tools (K25). The desktop's side
//! is `crate::desktop::sites`.
//!
//! - **Which windows are Firefox's** ([`Look::sight`]): the windows of a linked Firefox's main
//!   process (`LinkInfo::firefox_pid`, as [`super::desk`] finds Firefox's window). A window whose
//!   app id names Firefox (`org.mozilla.firefox`, `firefox`…) but that no link's process owns (a
//!   Firefox without Kai for Firefox, a pid Kai could not read, a sandbox's own pid namespace)
//!   shows a page Kai cannot tell.
//! - **Which tab it shows:** Firefox titles a window after the tab in front of it (`"<title> — "`
//!   and Firefox's name), so a window shows the tab in front of the extension's window whose title
//!   its own title starts with, and, while it has the desktop's focus, the tab in front of the
//!   window the extension says has the focus; where more than one fits, the strictest of their
//!   sites' rules decides. **One look takes every window of that Firefox's process** the caller
//!   knows: a tab in front whose title fits more than one of them decides for none of them, since
//!   one of those windows shows another page under that title (a page that copies another tab's
//!   title, a window whose own title has not caught up). Such a window, and one whose title fits
//!   no tab in front, gets the strictest rule any entry gives (`SiteRules::strictest`), as a page
//!   whose site Kai cannot tell does in the browser tools. That covers the windows the listing
//!   cannot name: a Private Browsing window (in any language: its tabs are never listed),
//!   Firefox's own windows (the Library, Page Info, the developer tools, Picture-in-Picture), a
//!   title KWin changed (whitespace, a ` <2>` for a second window of one title) or cut (Wayland
//!   titles end at 4095 bytes), a page without a title, and a Firefox whose tabs could not be
//!   listed. Nothing here depends on Firefox's localised window titles.
//! - **A page without a site** (`SiteRules::page_rule`, an allow-list: only an `http:`, `https:`
//!   or `file:` page with a host is a site's): an `about:` page (Firefox View, a new tab, the
//!   logins, the history, `about:blank`), a `data:`, host-less `file:` or opaque `blob:` page, one
//!   of Firefox's own (`chrome:`, `resource:`) or another extension's (`moz-extension:`) could
//!   show anything, so it gets the strictest rule. The listing does not say which tab opened an
//!   `about:blank`, so its opener's rule is not used yet.
//! - **Pixels lag the listing** ([`Shown::look`]'s `unsettled_after`): for a capture, a tab in
//!   front that came to the front less than `browser.see_settle_ms` before the capture's first
//!   check (`Tab::activated_at`), or that is still loading, counts as a page Kai cannot tell: the
//!   window may still show the page before it. A page that finished loading has no time in the
//!   listing yet, so only loading and switching are covered.
//! - **What is refused** ([`Look::refusal`]): capturing or reading ([`Reach::See`]) a window that
//!   shows a `none` site; acting in one ([`Reach::Act`]: `ui_press`, `type_text`, `key_press`)
//!   that shows a `read` or a `none` site. `see_screen` blanks such windows grey instead.
//! - **The frame with the keyboard focus** ([`Shown::focus_refusal`]): before keys go to a linked
//!   Firefox's window, `context.get@1` names the element with the focus and the origin of the
//!   frame it is in; a frame of a `read` or `none` site inside an `act` page refuses the keys.
//! - **Only the tab in front decides the window.** What the desktop shows of Firefox's window
//!   besides that tab's own site cannot be told from the window list: a frame of a `none` site
//!   inside an `act` page (the browser's `page_see` and `element_see` blank those,
//!   `super::tools::see`; the desktop's reading tools judge each page they read by its address,
//!   `crate::desktop::sites`) and the titles of other tabs in the tab strip are captured as they
//!   are, which the tools' answers say ([`GAP`]).
//! - **Not covered yet** (Kai's design, "Later": the full list): what Firefox's window shows
//!   besides its tab in front in a capture (frames, a split view's second tab, the sidebar's
//!   pages, docked developer tools) and the tab strip's titles ([`GAP`]), since the listing does
//!   not say which tabs a split view or a web panel shows; a focus inside Firefox's own controls
//!   (the address bar), which `context.get@1` does not tell from the page's body; a page that
//!   finished loading (no time in the listing); an `about:blank`'s opener; a window hidden other
//!   than minimized or on another desktop; rebound shortcuts; the compositor's thumbnails outside
//!   its effects; Firefox's titles in the window list; a `none` site's notifications and MPRIS
//!   metadata.
//! - **The fast path** ([`applies`]): while no site's rule could refuse what a tool does (no
//!   `none` site for seeing and reading, no `read` or `none` site for acting), the tools check
//!   nothing and list no tabs; and a window that is not a linked Firefox's costs no call either.
//!   Every look lists the tabs anew, so a tab switched since the last call is seen as it is now.
//!
//! Nothing here waits by a clock of Kai's: a listing ends with its answer, the link, or the call's
//! future being dropped (`super::links`).

use std::collections::HashMap;

use kai_desktop::caps::windows::Window;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::links::{LinkId, LinkInfo, Links, OpError};
use super::sites::{Rule, SiteRules};
use super::tabs::{LIST, Listing, Tab};
use crate::host::confirm::shown_text;
use crate::host::policy::SharedPolicy;

#[cfg(test)]
pub(crate) mod fake;
mod focus;

/// What a desktop tool does with a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Captures or reads it: refused on a `none` site.
    See,
    /// Acts in it (keys, presses): refused on a `read` or a `none` site.
    Act,
}

impl Reach {
    /// The least strict rule that refuses it.
    pub fn refused_from(self) -> Rule {
        match self {
            Reach::See => Rule::None,
            Reach::Act => Rule::Read,
        }
    }
}

/// Some site's rule could refuse `reach` (the fast path's test, module doc).
pub fn applies(sites: &SiteRules, reach: Reach) -> bool {
    sites.strictest() >= reach.refused_from()
}

/// What the tools' answers say of a Firefox window they captured or read (module doc).
pub const GAP: &str = "Kai's site rules checked the tab in front of Firefox's window: other \
                       tabs' titles in its tab strip are not taken out, nor, in a capture, what \
                       else the window shows (a frame of a site Kai does not reach inside the \
                       page, a second tab of a split view, the sidebar, docked developer tools); \
                       page_see and the browser's reading tools leave such frames out";

/// What a desktop window shows, as Kai's site rules see it (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sight {
    /// Not a window of Firefox's.
    NotFirefox,
    /// A linked Firefox's window whose tab in front is on a site of this rule (the strictest of
    /// the tabs it may be).
    Site(Rule),
    /// A Firefox window whose page Kai cannot tell, and why.
    Unknown(String),
}

/// Kai's site rules over the desktop's tools (module doc): the links and the policy, both live.
/// Cheap to clone.
#[derive(Clone)]
pub struct Shown {
    links: Links,
    policy: SharedPolicy,
}

/// Milliseconds since the epoch now, the clock of the listing's times.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Shown {
    pub fn new(links: Links, policy: SharedPolicy) -> Self {
        Self { links, policy }
    }

    /// The site rules now.
    pub fn sites(&self) -> SiteRules {
        self.policy
            .read()
            .expect("never poisoned")
            .browser()
            .sites
            .clone()
    }

    /// `browser.see_settle_ms` now (module doc).
    pub fn see_settle_ms(&self) -> u64 {
        self.policy
            .read()
            .expect("never poisoned")
            .browser()
            .see_settle_ms
    }

    /// The link whose Firefox process owns `w`.
    fn link_of(&self, w: &Window) -> Option<LinkInfo> {
        if w.pid == 0 {
            return None;
        }
        self.links
            .list()
            .into_iter()
            .find(|l| l.firefox_pid == Some(w.pid))
    }

    /// `w` is Firefox's as far as the desktop can say without asking the extension: a linked
    /// Firefox's process owns it, or its app id names Firefox.
    pub fn firefox(&self, w: &Window) -> bool {
        firefox_app(w) || self.link_of(w).is_some()
    }

    /// `w` belongs to a linked Firefox's process (its tabs can be listed).
    pub fn linked(&self, w: &Window) -> bool {
        self.link_of(w).is_some()
    }

    /// One look at `windows` under `sites`: the tabs of each linked Firefox whose process owns one
    /// of them, listed side by side; no call when none does. `windows` should hold every window
    /// of those processes the caller knows (module doc: a title that fits two of them decides for
    /// none). `unsettled_after`: for a capture, a tab in front activated after this time (ms since
    /// the epoch) or still loading counts as not settled (module doc).
    pub async fn look(
        &self,
        sites: SiteRules,
        windows: &[&Window],
        unsettled_after: Option<i64>,
    ) -> Look {
        let links = self.links.list();
        let cancel = CancellationToken::new();
        let calls = links
            .iter()
            .filter(|l| {
                l.firefox_pid
                    .is_some_and(|p| p != 0 && windows.iter().any(|w| w.pid == p))
            })
            .map(|l| {
                let cancel = &cancel;
                async move { (l.id, list(&self.links, l, cancel).await) }
            });
        let listed = futures_util::future::join_all(calls)
            .await
            .into_iter()
            .collect();
        Look {
            sites,
            links,
            listed,
            windows: windows.iter().map(|w| (*w).clone()).collect(),
            unsettled_after,
        }
    }
}

/// The app id names Firefox.
fn firefox_app(w: &Window) -> bool {
    w.app.to_lowercase().contains("firefox")
}

/// `link`'s tabs. The focus rule's marks are not fed from here: they follow the browser tools'
/// own looks (`super::tools::look`).
async fn list(
    links: &Links,
    link: &LinkInfo,
    cancel: &CancellationToken,
) -> Result<Listing, OpError> {
    let v = links.op(link.id, LIST, json!({}), cancel).await?;
    serde_json::from_value::<Listing>(v).map_err(|e| {
        OpError::failed(
            "bad_answer",
            format!("{LIST} answered something Kai does not read: {e}"),
        )
    })
}

/// One look at Firefox's windows ([`Shown::look`]): the rules it was taken under, the tabs of
/// each linked Firefox it asked, and the windows it was taken over.
pub struct Look {
    sites: SiteRules,
    links: Vec<LinkInfo>,
    listed: HashMap<LinkId, Result<Listing, OpError>>,
    windows: Vec<Window>,
    unsettled_after: Option<i64>,
}

/// Firefox's window as an answer names it: its id, never its title (which may be a `none` site's
/// page's).
pub fn window_name(w: &Window) -> String {
    format!("Firefox's window {}", shown_text(&w.id))
}

/// `w`'s title is the one Firefox gives a window showing `t` (module doc).
fn fits(w: &Window, t: &Tab) -> bool {
    !t.title.is_empty() && w.title.starts_with(&format!("{} — ", t.title))
}

impl Look {
    /// What `w` shows (module doc).
    pub fn sight(&self, w: &Window) -> Sight {
        let link = self
            .links
            .iter()
            .find(|l| w.pid != 0 && l.firefox_pid == Some(w.pid));
        let Some(link) = link else {
            return if firefox_app(w) {
                Sight::Unknown("no Firefox linked with Kai for Firefox owns it".into())
            } else {
                Sight::NotFirefox
            };
        };
        let listing = match self.listed.get(&link.id) {
            Some(Ok(l)) => l,
            Some(Err(e)) => {
                return Sight::Unknown(format!(
                    "its tabs could not be listed: {}",
                    super::tools::said(e)
                ));
            }
            None => return Sight::Unknown("its tabs were not listed".into()),
        };
        let fronts: Vec<&Tab> = listing.tabs.iter().filter(|t| t.active).collect();
        let mut fitting: Vec<&Tab> = fronts.iter().copied().filter(|t| fits(w, t)).collect();
        // Every window of this process the look was taken over, `w` among them.
        let peers: Vec<&Window> = self
            .windows
            .iter()
            .filter(|p| p.pid == w.pid && p.id != w.id)
            .chain(std::iter::once(w))
            .collect();
        if fitting
            .iter()
            .any(|t| peers.iter().filter(|p| fits(p, t)).count() > 1)
        {
            return Sight::Unknown(
                "its title fits a tab in front that another of that Firefox's windows fits too, \
                 so the title does not tell which of them shows that tab"
                    .into(),
            );
        }
        if w.active
            && let Some(focused) = listing.windows.iter().find(|x| x.focused)
            && let Some(t) = fronts.iter().find(|t| t.window == focused.id)
            && !fitting.iter().any(|f| f.id == t.id)
        {
            fitting.push(t);
        }
        if let Some(after) = self.unsettled_after
            && fitting
                .iter()
                .any(|t| t.loading || t.activated_at.is_some_and(|at| at > after))
        {
            return Sight::Unknown(
                "its tab in front is still loading or came to the front less than \
                 browser.see_settle_ms ago, so the window may still show the page before"
                    .into(),
            );
        }
        match fitting.iter().map(|t| self.sites.page_rule(&t.url)).max() {
            Some(rule) => Sight::Site(rule),
            None => Sight::Unknown(
                "its title fits none of the tabs in front of that Firefox's windows: a Private \
                 Browsing window, one of Firefox's own windows, a page without a title, or a \
                 title still changing"
                    .into(),
            ),
        }
    }

    /// The rule `w`'s page gets: `None` for a window that is not Firefox's, the strictest any
    /// entry gives for one whose page Kai cannot tell.
    pub fn rule(&self, w: &Window) -> Option<Rule> {
        match self.sight(w) {
            Sight::NotFirefox => None,
            Sight::Site(r) => Some(r),
            Sight::Unknown(_) => Some(self.sites.strictest()),
        }
    }

    /// Why a tool may not `reach` `w`, said without the window's title (module doc); `None` when
    /// it may.
    pub fn refusal(&self, w: &Window, reach: Reach) -> Option<String> {
        let name = window_name(w);
        match self.sight(w) {
            Sight::NotFirefox => None,
            Sight::Site(r) => rule_refusal(r, reach, &name),
            Sight::Unknown(why) => cannot_tell(&self.sites, reach, &name, &why),
        }
    }
}

/// The refusal for `what` showing a site of rule `r`, where `r` refuses `reach`.
pub fn rule_refusal(r: Rule, reach: Reach, what: &str) -> Option<String> {
    if r < reach.refused_from() {
        return None;
    }
    Some(if r == Rule::None {
        format!("{what} shows a site Kai does not reach (Kai's site rules)")
    } else {
        format!("{what} shows a site where Kai may only read (Kai's site rules)")
    })
}

/// The refusal for a page Kai cannot tell (`what`, `why`) while the strictest rule refuses
/// `reach`; `None` while it does not.
pub fn cannot_tell(sites: &SiteRules, reach: Reach, what: &str, why: &str) -> Option<String> {
    if sites.strictest() < reach.refused_from() {
        return None;
    }
    let keeps = if sites.strictest() == Rule::None {
        "keep some sites from Kai"
    } else {
        "let Kai only read some sites"
    };
    Some(format!(
        "Kai cannot tell which site {what} shows ({why}), and Kai's site rules {keeps}"
    ))
}

#[cfg(test)]
mod tests;
