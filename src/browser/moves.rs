//! The focus rule (kai-firefox design §6.4): a tool that acts on a page follows its policy state
//! only on the user's tab, while that tab is one the user brought to the front themselves and on
//! the site it had when the user last acted there; anywhere else it is confirmed (unless
//! `browser.act_in_background`). A tab Kai switched to or opened does not become the user's until
//! the user acts in it.
//!
//! **The extension's facts first** (the contract's Tab: `activated_by`, `activated_at`,
//! `user_origin`, `user_at`). A tab the extension activated (`tabs.switch@1`, `tabs.open@1`, the
//! tab Firefox shows in place of one `tabs.close@1` closed) is Kai's until the user acts in it
//! (`user_at` at or after `activated_at`); one the user activated is theirs. The site is the
//! origin of the user's last act there (`user_origin`): a navigation Kai or the page made does not
//! move it, so a tab Kai took elsewhere, or a link that went to another site, is asked about until
//! the user acts on the new site.
//!
//! **Kai's own input is not the user's** (§6.3): Firefox takes a key Kai sends through the desktop
//! for the user's own, so Kai brackets every input it sends (the trusted press, `type_text`,
//! `key_press`, `ui_press`) with the extension's input span in every linked profile
//! (`super::spans`): inside it the extension moves `kai_input_at`, never `user_origin` or
//! `user_at`, and counts an activation it did not make as its own. Kai keeps each input's
//! *moment* ([`Moves::input_began`] to [`Moves::input_ended`]) per profile besides:
//! - a window's `focused_at` in a moment is Kai's (its window activation for the trusted press),
//!   so it never decides which profile holds the user ([`Moves::discount`], before
//!   `super::tabs::front` reads a look);
//! - where the span could not be opened (the begin failed, or the extension does not serve it)
//!   the moment is *unbracketed*: a tab whose `user_at` falls into it is Kai-touched
//!   ([`Away::Typed`]) and an activation in it Kai's, until the user acts there after it.
//!
//! **Kai's own marks, the second source,** where the extension has not seen the tab activated
//! (`activated_by: null`: it was in front before the extension started) or has not seen the user
//! act in it, and where Kai raised a window, which the extension cannot tell from the user's
//! doing. Kai keeps its moves per Firefox profile, marking each before it sends the operation (a
//! navigation a cancelled or timed-out call started goes on in Firefox, and an answer Kai stopped
//! waiting for never comes):
//! - a tab Kai switched to or opened is *brought*, at a time. A `tab_open` that ends without
//!   naming its tab leaves every tab the next look finds that was not there before marked as
//!   Kai's; so does Kai's trusted press for a tab its key opened ([`Moves::raised`]), and the tab
//!   that is the user's after it but was not before (Kai brought its window to the front, or the
//!   key switched tabs) is *brought* when Kai's input ended;
//! - when Kai closes the user's tab, the tab Firefox shows in its place is *brought* as well;
//! - a tab Kai navigated keeps the site it had before Kai's first navigation (a tab Kai opened has
//!   none of the user's) until a look finds it back on that site after Kai's last navigation of it
//!   answered;
//! - *brought* counts where it is newer than the extension's last activation of the tab, and ends
//!   when the user acted in the tab since (`user_at`), or when Kai sees, at one of its looks at
//!   the tabs (every call that names a tab lists them), that the user's tab was another one and
//!   later is this one again: the user came back to it. Their return does not end the site the tab
//!   must be on. A return Kai does not see between two looks leaves the mark, so such a tab is
//!   still confirmed: the rule errs toward asking.
//!
//! **The owner's own act through Kai** (kai-firefox WP10b, *design decision*, narrowed by its
//! review): with the hands-and-eyes switch on, a hands-free user never clicks or types in the tab,
//! so the rule would anchor nowhere they asked Kai to go. A move Kai makes in a turn the owner
//! started (Kai's own voice session, its typed turn, the sidebar: `Caller::Own`, not while Kai's
//! continuation for jobs' results runs) counts as the user's act there ([`Moves::owned`]), and only
//! these moves: a navigation the owner asked Kai for, which moves the tab's site to where it went
//! only where it landed on the origin it was meant for (`tab_go`'s and `tab_open`'s URL, a
//! search's engine, the history entry of `tab_back` and `tab_forward`, a pressed link's `href`, a
//! submitted form's action, an Enter's form; the extension's `goes`), and an activation
//! (`tab_switch`, the tab Firefox shows after Kai closed the user's), which is the user's
//! activation. A redirect to another origin, a navigation the page makes itself (on a scroll, a
//! hover, a timer) and a window the page opens are Kai's navigation as for anyone (the tools'
//! side, `super::tools`). The owner's act is newer than the extension's facts until the user acts
//! there themselves; Kai's later moves for anyone else end it ([`Moves::brought`]). It keeps Kai's
//! own marks beside it, and counts only while the switch is on ([`Moves::verdict`]'s
//! `hands_free`): with the switch off the rule is as it was without it.
//!
//! **Which Firefox run.** Tab ids hold for a Firefox run, so the marks belong to the profile and
//! outlive a link that drops and comes back (the extension reloaded or updated). A restart of
//! Firefox starts its ids again, and `hello` does not say which restart a new link follows, so Kai
//! takes what it can observe: at a new link's first look, another Firefox version, or not one tab
//! with the id and URL a look saw before, is a restart, and the profile's marks go. A restart whose
//! first tabs match the last ones by id and URL keeps them: a needless question, never a missed one.
//!
//! **Kai's own restart** (kai-firefox WP6's review left the choice to this pass): the marks and
//! moments are kept in `$XDG_STATE_HOME/kai/browser-moves` ([`STORE`], written whole at each
//! change) with the last look's tabs (each as a digest of its id and URL, so no URL is in the
//! file; a site Kai navigated a tab from is kept as its origin), so a Kai that starts again reads
//! them back and the first look of each profile tells a Firefox restart from the same run as a
//! new link's does. A moment still open when Kai stopped ends when it is read back. What lives
//! only while a call runs (a `tab_open` waiting for its tab) is not kept.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::links::{LinkId, LinkInfo};
use super::tabs::{Front, Listing, Seen, Tab};

/// Kai's marks and moments in its state directory (module doc).
pub const STORE: &str = "browser-moves";

/// Why a tab is not one the user has in front of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Away {
    /// Not the user's tab.
    Behind,
    /// Kai cannot tell which tab is in front: two profiles each report one, or one did not answer.
    Unsure,
    /// Kai switched to it, or brought its window to the front.
    Brought,
    /// Kai opened it.
    Opened,
    /// Kai took it to another site than the user had there.
    Elsewhere,
    /// Kai's own input reached it while the extension could not tell it from the user's (module
    /// doc: an unbracketed moment).
    Typed,
}

impl Away {
    /// For K15's log (`Decision::Confirm`'s why).
    pub fn why(self) -> &'static str {
        match self {
            Away::Behind => "the focus rule: not the user's tab",
            Away::Unsure => "the focus rule: Kai cannot tell which tab is in front",
            Away::Brought => "the focus rule: a tab Kai brought to the front",
            Away::Opened => "the focus rule: a tab Kai opened",
            Away::Elsewhere => "the focus rule: a site the user did not have open there",
            Away::Typed => "the focus rule: Kai's own input reached the tab unbracketed",
        }
    }

    /// The dialog's note.
    pub fn note(self) -> &'static str {
        match self {
            Away::Behind => {
                "Asked because this is not the tab in front of you (Kai acts unasked only there, \
                 unless it may act in background tabs)."
            }
            Away::Unsure => {
                "Asked because Kai cannot tell which tab is in front of you: Firefox runs in more \
                 than one profile with a tab in front, or one of them did not answer."
            }
            Away::Brought => {
                "Asked because Kai brought this tab to the front; it counts as yours once you act \
                 in it, or go to another tab and come back."
            }
            Away::Opened => {
                "Asked because Kai opened this tab; it counts as yours once you act in it, or go \
                 to another tab and come back."
            }
            Away::Elsewhere => {
                "Asked because Kai took this tab to another site than the one you had open there."
            }
            Away::Typed => {
                "Asked because Kai's own keys reached this tab while Kai for Firefox could not \
                 tell them from yours; it counts as yours again once you act in it."
            }
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Mark {
    /// Kai switched to it, opened it or brought its window to the front: when (ms since the
    /// epoch).
    brought: Option<i64>,
    /// Kai navigated it: the origin it had before (`None`: Kai opened it, none was the user's).
    site: Option<Option<String>>,
    /// Kai's last navigation of it answered, so the tab's URL at a look is where it went.
    landed: bool,
    /// The user's tab was another one at a look since the mark.
    left: bool,
    /// The owner's own act through Kai (module doc): when.
    owned_at: Option<i64>,
    /// Where an owner's navigation through Kai took the tab: when, and its origin.
    owned_site: Option<(i64, String)>,
}

/// A `tab_open` whose tab Kai does not know yet.
#[derive(Debug)]
struct Opening {
    /// The profile's tab ids when it was sent, and those a look marked for it since.
    known: HashSet<i64>,
    /// The call is over without naming its tab: the next look settles it.
    ended: bool,
}

/// One of Kai's inputs into Firefox, in a profile (module doc).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Moment {
    ticket: u64,
    from: i64,
    /// `None` while the input runs.
    to: Option<i64>,
    /// The extension's input span was open: it tells Kai's input from the user's itself.
    bracketed: bool,
}

impl Moment {
    fn covers(&self, at: i64) -> bool {
        at >= self.from && self.to.is_none_or(|to| at <= to)
    }
}

/// Milliseconds since the epoch, as the extension's facts count them.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A tab's facts as the rule reads them, Kai's unbracketed input taken out (module doc).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Facts {
    /// `Some(true)`: the user activated it; `Some(false)`: Kai did.
    by_user: Option<bool>,
    activated_at: Option<i64>,
    user_origin: Option<String>,
    user_at: Option<i64>,
    /// The tab's last act falls into an unbracketed moment of Kai's.
    typed: bool,
}

/// One Firefox profile's marks and what tells its runs apart (module doc).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Profile {
    tabs: HashMap<i64, Mark>,
    /// The link the last look came through (not kept: link ids are this Kai run's).
    #[serde(skip)]
    link: Option<LinkId>,
    /// Firefox's version at the last look; empty before the first.
    firefox: String,
    /// The tabs that look saw, each by its id and URL as a digest ([`seen_key`]): enough to tell
    /// the same tabs again, and no URL in Kai's state file.
    seen: HashSet<String>,
    #[serde(skip)]
    opening: HashMap<u64, Opening>,
    /// Kai closes this tab, which was the user's: the tab that is the user's after it is Kai's
    /// doing.
    handed: Option<i64>,
    /// That close was the owner's own act through Kai (module doc): the next tab is theirs.
    handed_owned: bool,
    /// Kai's inputs whose moment a fact may still fall into.
    moments: Vec<Moment>,
}

/// What [`Moves`] keeps in its file (module doc).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Kept {
    profiles: BTreeMap<String, Profile>,
}

/// Where the marks are kept, and what was written last.
#[derive(Debug)]
struct Store {
    path: PathBuf,
    written: String,
    /// A failed write is logged once.
    warned: bool,
}

/// Kai's moves, per Firefox profile (module doc).
#[derive(Debug, Default)]
pub struct Moves {
    profiles: HashMap<String, Profile>,
    next_ticket: u64,
    store: Option<Store>,
}

/// A tab as a look saw it, by its id and URL ([`Profile`]'s `seen`): the first 16 bytes of their
/// SHA-256, in hex.
fn seen_key(id: i64, url: &str) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(format!("{id}\n{url}").as_bytes());
    d[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// A URL's origin as the rule compares it: that of the page it shows (`super::sites::effective`),
/// `scheme://host:port`; for a URL without one (`about:blank`, `data:`) the URL itself.
pub fn origin(url: &str) -> String {
    match super::sites::effective(url) {
        Some(u) => match u.origin() {
            o @ url::Origin::Tuple(..) => o.ascii_serialization(),
            url::Origin::Opaque(_) => u.to_string(),
        },
        None => url.to_owned(),
    }
}

impl Profile {
    fn mark(&mut self, tab: i64) -> &mut Mark {
        self.tabs.entry(tab).or_default()
    }

    fn opened(&mut self, tab: i64, at: i64) {
        let m = self.mark(tab);
        m.brought = Some(at);
        m.site = Some(None);
        m.left = false;
        m.owned_at = None;
        m.owned_site = None;
    }

    /// Kai brought the tab to the front: a later move of Kai's than the owner's own act there,
    /// which ends it; `owners`: for the owner's own `tab_switch` or close, which keeps the site
    /// the owner's moves gave the tab (module doc).
    fn brought(&mut self, tab: i64, at: i64, owners: bool) {
        let m = self.mark(tab);
        m.brought = Some(at);
        m.left = false;
        m.owned_at = None;
        if !owners {
            m.owned_site = None;
        }
    }

    /// The owner's own act through Kai (module doc); `url`: where a navigation took the tab.
    /// Kai's own marks stay: they decide while the switch is off.
    fn owned(&mut self, tab: i64, url: Option<&str>, at: i64) {
        let m = self.mark(tab);
        m.owned_at = Some(at);
        if let Some(u) = url {
            m.owned_site = Some((at, origin(u)));
        }
    }

    /// Whether `at` falls into one of Kai's unbracketed moments.
    fn unbracketed(&self, at: Option<i64>) -> bool {
        at.is_some_and(|a| self.moments.iter().any(|m| !m.bracketed && m.covers(a)))
    }

    /// `tab`'s facts with Kai's unbracketed input taken out (module doc).
    fn facts(&self, tab: &Tab) -> Facts {
        let typed = self.unbracketed(tab.user_at);
        Facts {
            by_user: match tab.activated_by.as_deref() {
                Some("user") => Some(!self.unbracketed(tab.activated_at)),
                Some(_) => Some(false),
                None => None,
            },
            activated_at: tab.activated_at,
            user_origin: if typed { None } else { tab.user_origin.clone() },
            user_at: if typed { None } else { tab.user_at },
            typed,
        }
    }
}

impl Moves {
    /// Kai's moves kept in `path` (module doc): what an earlier Kai kept is read back; a file that
    /// does not read is logged, and Kai starts without marks.
    pub fn stored(path: PathBuf) -> Self {
        let unread = |why: &dyn std::fmt::Display| {
            tracing::warn!(
                path = %path.display(),
                error = %why,
                "browser: Kai's record of its moves in Firefox does not read; the focus rule \
                 starts without Kai's own marks"
            );
            Kept::default()
        };
        let kept = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Kept>(&text).unwrap_or_else(|e| unread(&e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Kept::default(),
            Err(e) => unread(&e),
        };
        let now = now_ms();
        let mut profiles: HashMap<String, Profile> = kept.profiles.into_iter().collect();
        let mut next_ticket = 0;
        for p in profiles.values_mut() {
            for m in &mut p.moments {
                // Kai stopped while its input ran: the moment ends now.
                m.to.get_or_insert(now);
                next_ticket = next_ticket.max(m.ticket);
            }
        }
        let mut moves = Self {
            profiles,
            next_ticket,
            store: Some(Store {
                path,
                written: String::new(),
                warned: false,
            }),
        };
        moves.save();
        moves
    }

    /// Writes what is kept (module doc) when it changed: a file beside it, then renamed over it.
    fn save(&mut self) {
        let Some(store) = &self.store else {
            return;
        };
        let kept: BTreeMap<&String, &Profile> = self.profiles.iter().collect();
        let text = serde_json::json!({ "profiles": kept }).to_string();
        if text == store.written {
            return;
        }
        let path = store.path.clone();
        let written = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &text)?;
            std::fs::rename(&tmp, &path)
        })();
        let Some(store) = &mut self.store else {
            return;
        };
        match written {
            Ok(()) => store.written = text,
            Err(e) if !store.warned => {
                store.warned = true;
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "browser: Kai cannot keep its moves in Firefox; after a restart of Kai the \
                     focus rule has only the extension's facts"
                );
            }
            Err(_) => {}
        }
    }

    fn profile(&mut self, profile: &str) -> &mut Profile {
        self.profiles.entry(profile.to_owned()).or_default()
    }

    /// A look at the tabs of `link`'s profile (module doc): a new link after a restart drops the
    /// marks; the tabs Kai opened without learning which, and the one that took a closed tab's
    /// place, are marked; marks of closed tabs go; a tab the user came back to is theirs again,
    /// and one back on its site after Kai's navigations answered keeps no site; a moment no fact
    /// falls into any more goes.
    pub fn look(&mut self, link: &LinkInfo, listing: &Listing) {
        let now = now_ms();
        let p = self.profile(&link.profile);
        let seen: HashSet<String> = listing
            .tabs
            .iter()
            .map(|t| seen_key(t.id, &t.url))
            .collect();
        if p.link != Some(link.id) {
            let restarted = !p.firefox.is_empty()
                && (p.firefox != link.firefox || (!p.seen.is_empty() && p.seen.is_disjoint(&seen)));
            if restarted {
                p.tabs.clear();
                p.opening.clear();
                p.handed = None;
                p.handed_owned = false;
                p.moments.retain(|m| m.to.is_none());
            }
            p.link = Some(link.id);
            p.firefox.clone_from(&link.firefox);
        }
        p.seen = seen;
        // A moment is kept while Kai's input runs or a fact falls into it.
        p.moments.retain(|m| {
            m.to.is_none()
                || listing.tabs.iter().any(|t| {
                    [t.user_at, t.activated_at]
                        .iter()
                        .any(|a| a.is_some_and(|a| m.covers(a)))
                })
                || listing
                    .windows
                    .iter()
                    .any(|w| w.focused_at.is_some_and(|a| m.covers(a)))
        });
        let mut new = Vec::new();
        for o in p.opening.values_mut() {
            for t in &listing.tabs {
                if o.known.insert(t.id) {
                    new.push(t.id);
                }
            }
        }
        for id in new {
            p.opened(id, now);
        }
        p.opening.retain(|_, o| !o.ended);
        if let Some(closed) = p.handed {
            // Still the user's: the close has not happened yet.
            if listing.user_tab != Some(closed) {
                p.handed = None;
                let owners = std::mem::take(&mut p.handed_owned);
                if let Some(next) = listing.user_tab {
                    p.brought(next, now, owners);
                    if owners {
                        p.owned(next, None, now);
                    }
                }
            }
        }
        p.tabs.retain(|id, m| {
            let Some(tab) = listing.tabs.iter().find(|t| t.id == *id) else {
                return false;
            };
            if listing.user_tab != Some(*id) {
                m.left = true;
            } else if m.left {
                m.left = false;
                m.brought = None;
                if m.site == Some(None) {
                    m.site = None;
                }
            }
            if m.landed
                && let Some(Some(site)) = &m.site
                && *site == origin(&tab.url)
            {
                m.site = None;
            }
            m.brought.is_some() || m.site.is_some() || m.owned_at.is_some()
        });
        self.save();
    }

    /// `listing` of `profile` as the rule reads it: a window's `focused_at` in one of Kai's
    /// moments is Kai's, not the user's choice of a window (module doc), so it is taken out.
    pub fn discount(&self, profile: &str, listing: &mut Listing) {
        let Some(p) = self.profiles.get(profile) else {
            return;
        };
        for w in &mut listing.windows {
            if w.focused_at
                .is_some_and(|a| p.moments.iter().any(|m| m.covers(a)))
            {
                w.focused_at = None;
            }
        }
    }

    /// Kai switches to the tab, or brought its window to the front: for anyone, this ends the
    /// owner's own act there (module doc).
    pub fn brought(&mut self, profile: &str, tab: i64) {
        self.profile(profile).brought(tab, now_ms(), false);
        self.save();
    }

    /// Kai switches to the tab for the owner's own turn, hands free (module doc): marked as
    /// [`Moves::brought`] before the switch is sent, keeping the site the owner's moves gave the
    /// tab; [`Moves::owned`] once it answered.
    pub fn switching_for_owner(&mut self, profile: &str, tab: i64) {
        self.profile(profile).brought(tab, now_ms(), true);
        self.save();
    }

    /// Kai sends a `tab_open` while the profile has the tabs `before`: its ticket, for
    /// [`Moves::opened`] or [`Moves::open_ended`].
    pub fn opening(&mut self, profile: &str, before: HashSet<i64>) -> u64 {
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        self.profile(profile).opening.insert(
            ticket,
            Opening {
                known: before,
                ended: false,
            },
        );
        ticket
    }

    /// The `tab_open` of `ticket` opened `tab`.
    pub fn opened(&mut self, profile: &str, ticket: u64, tab: i64) {
        let p = self.profile(profile);
        p.opening.remove(&ticket);
        p.opened(tab, now_ms());
        self.save();
    }

    /// The `tab_open` of `ticket` is over without naming its tab: the next look marks every tab
    /// that was not there before.
    pub fn open_ended(&mut self, profile: &str, ticket: u64) {
        if let Some(o) = self.profile(profile).opening.get_mut(&ticket) {
            o.ended = true;
        }
    }

    /// Kai navigates the tab from `before` (the URL it showed): the first navigation keeps the
    /// user's site; the tab's URL tells where it went once [`Moves::landed`].
    pub fn navigated(&mut self, profile: &str, tab: i64, before: &str) {
        let m = self.profile(profile).mark(tab);
        if m.site.is_none() {
            m.site = Some(Some(origin(before)));
        }
        m.landed = false;
        self.save();
    }

    /// Kai's navigation of the tab answered.
    pub fn landed(&mut self, profile: &str, tab: i64) {
        if let Some(m) = self.profile(profile).tabs.get_mut(&tab) {
            m.landed = true;
        }
        self.save();
    }

    /// Kai closes the tab; `was_users`: it is the user's tab, so the one Firefox shows next is
    /// Kai's doing, or with `owned` the owner's own (module doc).
    pub fn closing(&mut self, profile: &str, tab: i64, was_users: bool, owned: bool) {
        if was_users {
            let p = self.profile(profile);
            p.handed = Some(tab);
            p.handed_owned = owned;
            self.save();
        }
    }

    /// Kai's close of a tab failed, or its call ended without an answer: the tab Firefox shows
    /// next is Kai's doing at most, never the owner's own act (module doc).
    pub fn close_failed(&mut self, profile: &str) {
        if let Some(p) = self.profiles.get_mut(profile)
            && std::mem::take(&mut p.handed_owned)
        {
            self.save();
        }
    }

    /// The owner's own act through Kai in `tab` (module doc), now: `url` is where a navigation
    /// took it (the tab's URL as the operation answered), `None` for an activation.
    pub fn owned(&mut self, profile: &str, tab: i64, url: Option<&str>) {
        self.profile(profile).owned(tab, url, now_ms());
        self.save();
    }

    /// Kai brought `tab` of `profile` to the front, or opened it, at `since` or later.
    pub fn kais_since(&self, profile: &str, tab: i64, since: i64) -> bool {
        self.profiles
            .get(profile)
            .and_then(|p| p.tabs.get(&tab))
            .and_then(|m| m.brought)
            .is_some_and(|b| b >= since)
    }

    /// Kai closed the tab.
    pub fn closed(&mut self, profile: &str, tab: i64) {
        self.profile(profile).tabs.remove(&tab);
        self.save();
    }

    /// Kai's input begins in every profile of `profiles` (`true`: its input span is open there):
    /// its ticket, for [`Moves::input_ended`] (module doc).
    pub fn input_began(&mut self, profiles: &[(String, bool)]) -> u64 {
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let from = now_ms();
        for (profile, bracketed) in profiles {
            self.profile(profile).moments.push(Moment {
                ticket,
                from,
                to: None,
                bracketed: *bracketed,
            });
        }
        self.save();
        ticket
    }

    /// Kai's input of `ticket` is over in every profile, its keys settled by `to` (a later end
    /// than one said before; never an earlier one).
    pub fn input_ended(&mut self, ticket: u64, to: i64) {
        for p in self.profiles.values_mut() {
            for m in p.moments.iter_mut().filter(|m| m.ticket == ticket) {
                m.to = Some(m.to.map_or(to, |t| t.max(to)).max(m.from));
            }
        }
        self.save();
    }

    /// What Kai's input that ended at `at` did to the tabs, between the looks `before` and `after`
    /// (module doc): a tab a profile did not have before is one Kai opened; a profile's user's tab
    /// that was not its user's tab before, and the user's tab of the profile in front when another
    /// was in front before, is one Kai brought to the front.
    pub fn raised(&mut self, before: &[Seen], after: &[Seen], at: i64) {
        let front = |seen: &[Seen]| match super::tabs::front(seen) {
            Front::One(i) => Some(seen[i].link.profile.clone()),
            _ => None,
        };
        let (was, now) = (front(before), front(after));
        for s in after {
            let Ok(l) = &s.listing else {
                continue;
            };
            let profile = s.link.profile.as_str();
            let earlier = before
                .iter()
                .find(|b| b.link.profile == profile)
                .and_then(|b| b.listing.as_ref().ok());
            let raised = now.as_deref() == Some(profile) && was != now;
            let p = self.profile(profile);
            if let Some(e) = earlier {
                for t in &l.tabs {
                    if !e.tabs.iter().any(|b| b.id == t.id) {
                        p.opened(t.id, at);
                    }
                }
            }
            if let Some(user) = l.user_tab {
                let switched = earlier.is_some_and(|e| e.user_tab != Some(user));
                if switched || raised {
                    p.brought(user, at, false);
                }
            }
        }
        self.save();
    }

    /// The origin of the user's last act in `tab` of `profile` as the rule reads it (module doc):
    /// none where Kai's unbracketed input moved it.
    pub fn users_origin(&self, profile: &str, tab: &Tab) -> Option<String> {
        match self.profiles.get(profile) {
            Some(p) => p.facts(tab).user_origin,
            None => Profile::default().facts(tab).user_origin,
        }
    }

    /// Whether `tab` of `profile` is in front of the user as the rule means it (module doc);
    /// `hands_free`: the hands-and-eyes switch is on, so the owner's own acts through Kai count.
    pub fn verdict(&self, profile: &str, tab: &Tab, hands_free: bool) -> Result<(), Away> {
        if !tab.user {
            return Err(Away::Behind);
        }
        let none = Profile::default();
        let p = self.profiles.get(profile).unwrap_or(&none);
        let m = p.tabs.get(&tab.id);
        let mut f = p.facts(tab);
        if f.typed {
            return Err(Away::Typed);
        }
        // The owner's own act through Kai, where it is newer than the user's own (module doc):
        // their act, and where a navigation of theirs took the tab, their site. Only while the
        // switch is on.
        if let Some(m) = m.filter(|_| hands_free) {
            let newer = |at: i64| f.user_at.is_none_or(|u| at > u);
            if let Some((_, site)) = m.owned_site.clone().filter(|(at, _)| newer(*at)) {
                f.user_origin = Some(site);
            }
            if let Some(at) = m.owned_at.filter(|at| newer(*at)) {
                f.user_at = Some(at);
            }
        }
        let kais = if m.is_some_and(|m| m.site == Some(None)) {
            Away::Opened
        } else {
            Away::Brought
        };
        // The extension activated it, and the user has not acted in it since.
        if f.by_user == Some(false)
            && !f
                .user_at
                .zip(f.activated_at)
                .is_some_and(|(user, kai)| user >= kai)
        {
            return Err(kais);
        }
        // Kai's own mark, where it is newer than the extension's last activation: until the user
        // acted in the tab since.
        if let Some(at) = m.and_then(|m| m.brought) {
            let newer = f.by_user.is_none() || f.activated_at.is_none_or(|a| a < at);
            if newer && !f.user_at.is_some_and(|u| u >= at) {
                return Err(kais);
            }
        }
        let site = match (&f.user_origin, m.map(|m| &m.site)) {
            (Some(o), _) => Some(o.clone()),
            (None, Some(Some(Some(site)))) => Some(site.clone()),
            _ => None,
        };
        match site {
            Some(site) if site != origin(&tab.url) => Err(Away::Elsewhere),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::tabs::Window;

    fn tab(id: i64, url: &str, user: bool) -> Tab {
        Tab {
            id,
            url: url.into(),
            user,
            ..Default::default()
        }
    }

    fn link(id: LinkId, profile: &str) -> LinkInfo {
        LinkInfo {
            id,
            profile: profile.into(),
            ext_version: "0.1.0".into(),
            firefox: "159.0".into(),
            protocol: 1,
            ops: Vec::new(),
            firefox_pid: None,
        }
    }

    fn listing(tabs: &[(i64, &str)], user: Option<i64>) -> Listing {
        Listing {
            tabs: tabs
                .iter()
                .map(|(i, url)| tab(*i, url, Some(*i) == user))
                .collect(),
            user_tab: user,
            windows: Vec::new(),
        }
    }

    const MAIL: &str = "https://mail.example/inbox";
    const NEWS: &str = "https://news.example/";

    #[test]
    fn a_tab_kai_brought_is_the_users_once_they_come_back_or_act_in_it() {
        let mut m = Moves::default();
        let a = link(1, "a");
        let mail = tab(3, MAIL, true);
        assert_eq!(m.verdict("a", &mail, false), Ok(()));
        assert_eq!(
            m.verdict("a", &tab(4, NEWS, false), false),
            Err(Away::Behind)
        );
        m.brought("a", 3);
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS)], Some(3)));
        assert_eq!(m.verdict("a", &mail, false), Err(Away::Brought));
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS)], Some(4)));
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS)], Some(3)));
        assert_eq!(m.verdict("a", &mail, false), Ok(()), "the user came back");
        // Another profile's marks are its own.
        m.brought("b", 3);
        assert_eq!(m.verdict("a", &mail, false), Ok(()));
        assert_eq!(m.verdict("b", &mail, false), Err(Away::Brought));
        // The user's act in it since ends the mark too.
        let mut acted = mail.clone();
        acted.user_at = Some(now_ms() + 5);
        assert_eq!(m.verdict("b", &acted, false), Ok(()));
    }

    #[test]
    fn a_tab_kai_took_elsewhere_is_asked_about_until_back_on_its_site() {
        let mut m = Moves::default();
        let a = link(1, "a");
        m.navigated("a", 3, MAIL);
        assert_eq!(
            m.verdict("a", &tab(3, "https://mail.example/sent", true), false),
            Ok(()),
            "the same site"
        );
        assert_eq!(
            m.verdict("a", &tab(3, "https://evil.example/", true), false),
            Err(Away::Elsewhere)
        );
        m.navigated("a", 3, "https://evil.example/");
        assert_eq!(
            m.verdict("a", &tab(3, "https://mail.example/", true), false),
            Ok(()),
            "the first navigation keeps the user's site"
        );
        let t = m.opening("a", HashSet::from([3]));
        m.opened("a", t, 9);
        m.navigated("a", 9, "about:blank");
        assert_eq!(
            m.verdict("a", &tab(9, "https://x.example/", true), false),
            Err(Away::Opened)
        );
        // A closed tab's mark goes; the next tab with its id starts clean.
        m.closed("a", 9);
        assert_eq!(
            m.verdict("a", &tab(9, "https://x.example/", true), false),
            Ok(())
        );
        let t = m.opening("a", HashSet::from([3]));
        m.opened("a", t, 10);
        m.look(&a, &listing(&[(3, MAIL)], Some(3)));
        assert_eq!(
            m.verdict("a", &tab(10, "https://x.example/", true), false),
            Ok(())
        );
    }

    /// §6.4: the site the user last acted on. Coming back to a tab Kai took elsewhere ends
    /// "brought", not the site; the tab is the user's there again once it is back on that site.
    #[test]
    fn coming_back_keeps_the_site_until_the_tab_is_on_it_again() {
        let mut m = Moves::default();
        let a = link(1, "a");
        m.brought("a", 3);
        m.navigated("a", 3, MAIL);
        m.landed("a", 3);
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(4)));
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, NEWS, true), false),
            Err(Away::Elsewhere),
            "the user came back, the site stays Kai's choice"
        );
        // Kai's navigation back has not answered: the look does not take the URL for it.
        m.navigated("a", 3, NEWS);
        m.look(&a, &listing(&[(3, MAIL)], Some(3)));
        m.look(&a, &listing(&[(3, "https://other.example/")], Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, "https://other.example/", true), false),
            Err(Away::Elsewhere)
        );
        m.navigated("a", 3, "https://other.example/");
        m.landed("a", 3);
        m.look(&a, &listing(&[(3, "https://mail.example/sent")], Some(3)));
        m.look(&a, &listing(&[(3, "https://other.example/")], Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, "https://other.example/", true), false),
            Ok(()),
            "back on the user's site, the mark ended"
        );
    }

    #[test]
    fn an_open_without_its_answer_marks_every_new_tab_at_the_next_look() {
        let mut m = Moves::default();
        let a = link(1, "a");
        let t = m.opening("a", HashSet::from([3, 4]));
        // A look while the call runs marks it already, once.
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS), (5, NEWS)], Some(5)));
        assert_eq!(
            m.verdict("a", &tab(5, NEWS, true), false),
            Err(Away::Opened)
        );
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS), (5, NEWS)], Some(4)));
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS), (5, NEWS)], Some(5)));
        assert_eq!(
            m.verdict("a", &tab(5, NEWS, true), false),
            Ok(()),
            "came back"
        );
        m.open_ended("a", t);
        let six = [(3, MAIL), (4, NEWS), (5, NEWS), (6, NEWS)];
        m.look(&a, &listing(&six, Some(6)));
        assert_eq!(
            m.verdict("a", &tab(6, NEWS, true), false),
            Err(Away::Opened)
        );
        assert_eq!(m.verdict("a", &tab(5, NEWS, true), false), Ok(()));
        // Settled: a tab the user opens later is theirs.
        let seven = [(3, MAIL), (4, NEWS), (5, NEWS), (6, NEWS), (7, NEWS)];
        m.look(&a, &listing(&seven, Some(7)));
        assert_eq!(m.verdict("a", &tab(7, NEWS, true), false), Ok(()));
    }

    #[test]
    fn the_tab_that_takes_a_closed_users_tab_place_is_kais_doing() {
        let mut m = Moves::default();
        let a = link(1, "a");
        m.closing("a", 3, true, false);
        m.look(&a, &listing(&[(3, MAIL), (4, NEWS)], Some(3)));
        m.look(&a, &listing(&[(4, NEWS)], Some(4)));
        assert_eq!(
            m.verdict("a", &tab(4, NEWS, true), false),
            Err(Away::Brought)
        );
        m.closing("a", 4, false, false);
        m.look(&a, &listing(&[(5, MAIL)], Some(5)));
        assert_eq!(m.verdict("a", &tab(5, MAIL, true), false), Ok(()));
    }

    fn facts(mut t: Tab, by: Option<&str>, at: i64, origin: Option<&str>, user_at: i64) -> Tab {
        t.activated_by = by.map(str::to_owned);
        t.activated_at = Some(at);
        t.user_origin = origin.map(str::to_owned);
        t.user_at = Some(user_at);
        t
    }

    /// The extension's facts decide where it has them: a tab the extension activated is Kai's
    /// until the user acts in it, the site is the one of the user's last act, and Kai's marks
    /// count only where they are newer than what the extension saw.
    #[test]
    fn the_extensions_facts_decide_before_kais_marks() {
        let m = Moves::default();
        let mail = |by, at, origin, user_at| facts(tab(3, MAIL, true), by, at, origin, user_at);
        let o = Some("https://mail.example");
        assert_eq!(
            m.verdict("a", &mail(Some("user"), 10, o, 10), false),
            Ok(())
        );
        assert_eq!(
            m.verdict("a", &mail(Some("extension"), 10, o, 5), false),
            Err(Away::Brought)
        );
        assert_eq!(
            m.verdict("a", &mail(Some("extension"), 10, o, 12), false),
            Ok(()),
            "the user acted in it since"
        );
        assert_eq!(
            m.verdict(
                "a",
                &mail(Some("user"), 10, Some("https://news.example"), 10),
                false
            ),
            Err(Away::Elsewhere),
            "a link took it elsewhere: the user has not acted on this site"
        );
        // Kai's mark of a switch is overruled by the user's later activation the extension saw.
        let mut m = Moves::default();
        m.brought("a", 3);
        let later = now_ms() + 1000;
        assert_eq!(
            m.verdict("a", &mail(Some("user"), later, o, later), false),
            Ok(())
        );
        // An older activation of the user's is not: Kai raised the tab since (its window).
        assert_eq!(
            m.verdict("a", &mail(Some("user"), 10, o, 10), false),
            Err(Away::Brought)
        );
        let mut unseen = tab(3, MAIL, true);
        unseen.activated_by = None;
        assert_eq!(
            m.verdict("a", &unseen, false),
            Err(Away::Brought),
            "the mark"
        );
    }

    /// Kai's input where the extension's span could not be opened: a tab whose last act falls into
    /// it is Kai-touched until the user acts there after it, and an activation in it is Kai's. A
    /// bracketed moment leaves the facts to the extension, but no window's focus in it counts.
    #[test]
    fn kais_unbracketed_input_is_not_the_users_act() {
        let mut m = Moves::default();
        let a = link(1, "a");
        let now = now_ms();
        let before = facts(
            tab(3, MAIL, true),
            Some("user"),
            now - 5000,
            Some("https://mail.example"),
            now - 9000,
        );
        assert_eq!(m.verdict("a", &before, false), Ok(()));
        let t = m.input_began(&[("a".into(), false), ("b".into(), true)]);
        let mut after = before.clone();
        after.user_at = Some(now_ms());
        after.user_origin = Some("https://evil.example".into());
        m.input_ended(t, now_ms() + 50);
        let l = Listing {
            tabs: vec![after.clone()],
            user_tab: Some(3),
            windows: Vec::new(),
        };
        m.look(&a, &l);
        assert_eq!(m.verdict("a", &after, false), Err(Away::Typed), "Kai's key");
        assert_eq!(m.users_origin("a", &after), None);
        // In the bracketed profile the extension kept its facts itself.
        assert_eq!(m.verdict("b", &before, false), Ok(()));
        // The user's own act after the moment counts.
        after.user_at = Some(now_ms() + 1000);
        assert_eq!(m.verdict("a", &after, false), Err(Away::Elsewhere));
        after.user_origin = Some("https://mail.example".into());
        assert_eq!(m.verdict("a", &after, false), Ok(()));
        // An activation in the moment is Kai's.
        let mut popup = tab(9, NEWS, true);
        popup.activated_by = Some("user".into());
        popup.activated_at = Some(now + 1);
        assert_eq!(m.verdict("a", &popup, false), Err(Away::Brought));
        // A window that got the focus in any of Kai's moments does not decide the profile.
        let mut w = Listing {
            windows: vec![
                Window {
                    id: 1,
                    focused: false,
                    focused_at: Some(now + 1),
                },
                Window {
                    id: 2,
                    focused: false,
                    focused_at: Some(now - 60_000),
                },
            ],
            ..l
        };
        m.discount("b", &mut w);
        assert_eq!(w.windows[0].focused_at, None);
        assert_eq!(w.windows[1].focused_at, Some(now - 60_000));
    }

    /// The trusted press: a tab its key opened is Kai's, and the tab that is the user's after it
    /// and was not before, or of the profile Kai brought to the front, is brought.
    #[test]
    fn what_kais_input_brought_to_the_front_is_kais() {
        let seen = |profile: &str, l: Listing| Seen {
            link: link(1, profile),
            listing: Ok(l),
        };
        let focused = |tabs: &[(i64, &str)], user: i64, focused: bool| Listing {
            tabs: tabs
                .iter()
                .map(|(i, url)| {
                    let mut t = tab(*i, url, *i == user);
                    t.window = 1;
                    t
                })
                .collect(),
            user_tab: Some(user),
            windows: vec![Window {
                id: 1,
                focused,
                focused_at: None,
            }],
        };
        let before = [
            seen("a", focused(&[(3, MAIL)], 3, true)),
            seen("b", focused(&[(5, NEWS)], 5, false)),
        ];
        let at = now_ms();
        let mut m = Moves::default();
        let opened = [
            seen("a", focused(&[(3, MAIL)], 3, false)),
            seen("b", focused(&[(5, NEWS), (6, NEWS)], 6, true)),
        ];
        m.raised(&before, &opened, at);
        assert_eq!(
            m.verdict("b", &tab(6, NEWS, true), false),
            Err(Away::Opened)
        );
        assert_eq!(m.verdict("a", &tab(3, MAIL, true), false), Ok(()));
        // Profile b came to the front with the same tab: brought.
        let mut m = Moves::default();
        let raised = [
            seen("a", focused(&[(3, MAIL)], 3, false)),
            seen("b", focused(&[(5, NEWS)], 5, true)),
        ];
        m.raised(&before, &raised, at);
        assert_eq!(
            m.verdict("b", &tab(5, NEWS, true), false),
            Err(Away::Brought)
        );
        // Nothing changed in front: nothing marked.
        let mut m = Moves::default();
        m.raised(&before, &before, at);
        assert_eq!(m.verdict("a", &tab(3, MAIL, true), false), Ok(()));
    }

    /// kai-firefox WP10b: with the switch on, a move Kai made in a turn the owner started is the
    /// user's act: the tab Kai opened, a navigation's new site, a switch, the tab after a close.
    /// The user's own later act wins, and Kai's later moves for anyone else mark it again.
    #[test]
    fn the_owners_own_moves_through_kai_count_as_theirs() {
        let mut m = Moves::default();
        let a = link(1, "a");
        // A tab Kai opened for the owner: the extension activated it before Kai's answer came.
        let t = m.opening("a", HashSet::from([3]));
        m.opened("a", t, 9);
        let mut opened = facts(tab(9, MAIL, true), Some("extension"), now_ms() - 5, None, 0);
        opened.user_at = None;
        assert_eq!(m.verdict("a", &opened, true), Err(Away::Opened));
        m.owned("a", 9, Some(MAIL));
        assert_eq!(
            m.verdict("a", &opened, true),
            Ok(()),
            "the owner's tab, on the owner's site"
        );
        assert_eq!(
            m.verdict("a", &opened, false),
            Err(Away::Opened),
            "the switch off: Kai's own marks, as without it"
        );
        let mut elsewhere = opened.clone();
        elsewhere.url = NEWS.into();
        assert_eq!(m.verdict("a", &elsewhere, true), Err(Away::Elsewhere));
        m.look(&a, &listing(&[(3, NEWS), (9, MAIL)], Some(9)));
        assert_eq!(m.verdict("a", &opened, true), Ok(()), "kept by a look");

        // A navigation of the owner's moves the site the user last acted on there.
        let user = facts(
            tab(3, NEWS, true),
            Some("user"),
            10,
            Some("https://mail.example"),
            10,
        );
        assert_eq!(m.verdict("a", &user, true), Err(Away::Elsewhere));
        m.navigated("a", 3, MAIL);
        m.landed("a", 3);
        m.owned("a", 3, Some(NEWS));
        assert_eq!(m.verdict("a", &user, true), Ok(()));
        assert_eq!(m.verdict("a", &user, false), Err(Away::Elsewhere));
        let mut back = user.clone();
        back.url = MAIL.into();
        assert_eq!(
            m.verdict("a", &back, true),
            Err(Away::Elsewhere),
            "now anchored on the news"
        );
        // The user's own act after it decides again.
        let mut acted = back.clone();
        acted.user_at = Some(now_ms() + 1000);
        assert_eq!(m.verdict("a", &acted, true), Ok(()));
        // The owner's own switch keeps the owner's site.
        m.switching_for_owner("a", 3);
        assert_eq!(
            m.verdict("a", &user, true),
            Err(Away::Brought),
            "until it answered"
        );
        m.owned("a", 3, None);
        assert_eq!(m.verdict("a", &user, true), Ok(()));
        // Kai's later move for another caller ends the owner's act, their site with it: the
        // owner's next switch is an activation on the site the user had.
        m.brought("a", 3);
        assert_eq!(m.verdict("a", &user, true), Err(Away::Brought));
        m.switching_for_owner("a", 3);
        m.owned("a", 3, None);
        assert_eq!(m.verdict("a", &user, true), Err(Away::Elsewhere));
        m.owned("a", 3, Some(NEWS));
        assert_eq!(m.verdict("a", &user, true), Ok(()));

        // The tab Firefox shows after the owner had Kai close theirs.
        m.closing("a", 3, true, true);
        m.look(&a, &listing(&[(3, NEWS), (9, MAIL)], Some(3)));
        m.look(&a, &listing(&[(9, MAIL)], Some(9)));
        assert_eq!(m.verdict("a", &tab(9, MAIL, true), true), Ok(()));
        m.closing("a", 9, true, false);
        m.look(&a, &listing(&[(4, NEWS)], Some(4)));
        assert_eq!(
            m.verdict("a", &tab(4, NEWS, true), true),
            Err(Away::Brought)
        );
    }

    /// kai-firefox WP10b's check of the rule as written: a link Kai pressed to another site does
    /// not make that site the user's, also where the user never acted in the tab.
    #[test]
    fn a_link_kai_pressed_does_not_move_the_site() {
        let mut m = Moves::default();
        let a = link(1, "a");
        let news = tab(3, NEWS, true);
        assert_eq!(
            m.verdict("a", &news, false),
            Ok(()),
            "no act of the user's: no site"
        );
        // Every acting operation is marked as a navigation of Kai's before it is sent, and landed
        // once it answered (`super::tools::act`): one that stayed leaves no mark at the next look.
        m.navigated("a", 3, MAIL);
        m.landed("a", 3);
        m.look(&a, &listing(&[(3, MAIL)], Some(3)));
        assert!(
            m.profiles.get("a").is_none_or(|p| p.tabs.is_empty()),
            "stayed: no mark"
        );
        m.navigated("a", 3, MAIL);
        m.landed("a", 3);
        m.look(&a, &listing(&[(3, NEWS)], Some(3)));
        assert_eq!(m.verdict("a", &news, false), Err(Away::Elsewhere));
        // Back on the site it had: the mark ends.
        m.look(&a, &listing(&[(3, "https://mail.example/sent")], Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, "https://mail.example/sent", true), false),
            Ok(())
        );
        assert!(m.profiles["a"].tabs.is_empty());
    }

    /// kai-firefox WP10b's review (L2): the owner's marks count only while the switch is on, a
    /// close that failed hands the next tab to nobody, and a move of Kai's for anyone else ends
    /// the owner's site.
    #[test]
    fn the_owners_marks_are_nothing_with_the_switch_off_or_after_kais_other_moves() {
        let mut m = Moves::default();
        let a = link(1, "a");
        // The owner's tab_go took the user's tab from the mail to the news.
        let user = facts(
            tab(3, NEWS, true),
            Some("user"),
            10,
            Some("https://mail.example"),
            10,
        );
        m.navigated("a", 3, MAIL);
        m.landed("a", 3);
        m.owned("a", 3, Some(NEWS));
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        assert_eq!(m.verdict("a", &user, true), Ok(()));
        // Switched off: as without it, also for marks left from while it was on.
        assert_eq!(m.verdict("a", &user, false), Err(Away::Elsewhere));
        let mut plain = tab(3, NEWS, true);
        plain.user_at = None;
        assert_eq!(
            m.verdict("a", &plain, false),
            Err(Away::Elsewhere),
            "Kai's own navigation mark decides"
        );
        // A close of the owner's that failed: the tab the user goes to next is no one's act.
        m.closing("a", 3, true, true);
        m.close_failed("a");
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(4)));
        assert_eq!(
            m.verdict("a", &tab(4, MAIL, true), true),
            Err(Away::Brought),
            "Kai's doing at most, never the owner's"
        );
        // Kai brought the owner's tab to the front for another caller: the owner's site is gone.
        m.brought("a", 3);
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(4)));
        m.look(&a, &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        assert_eq!(
            m.verdict("a", &user, true),
            Err(Away::Elsewhere),
            "the user came back, to the site they had"
        );
    }

    #[test]
    fn marks_outlive_a_new_link_but_not_a_restart_of_firefox() {
        let mut m = Moves::default();
        let tabs = [(3, MAIL), (4, NEWS)];
        m.look(&link(1, "a"), &listing(&tabs, Some(3)));
        m.brought("a", 3);
        // The extension reloaded: a new link, the same tabs.
        m.look(&link(2, "a"), &listing(&tabs, Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, MAIL, true), false),
            Err(Away::Brought)
        );
        // Firefox restarted: not one tab as it was.
        m.look(&link(3, "a"), &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        assert_eq!(m.verdict("a", &tab(3, NEWS, true), false), Ok(()));
        // Or a new version of Firefox, whatever the tabs.
        m.brought("a", 3);
        let mut updated = link(4, "a");
        updated.firefox = "160.0".into();
        m.look(&updated, &listing(&[(3, NEWS), (4, MAIL)], Some(3)));
        assert_eq!(m.verdict("a", &tab(3, NEWS, true), false), Ok(()));
    }

    /// Kai's own restart (module doc): the marks are read back, and the first look tells the same
    /// Firefox run from a restart of it; an input still running when Kai stopped has ended.
    #[test]
    fn marks_outlive_a_restart_of_kai() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join(STORE);
        let tabs = [(3, MAIL), (4, NEWS)];
        {
            let mut m = Moves::stored(path.clone());
            m.look(&link(1, "a"), &listing(&tabs, Some(3)));
            m.brought("a", 3);
            m.navigated("a", 4, MAIL);
            m.input_began(&[("a".into(), false)]);
        }
        let mut m = Moves::stored(path.clone());
        assert_eq!(
            m.verdict("a", &tab(3, MAIL, true), false),
            Err(Away::Brought)
        );
        assert_eq!(
            m.verdict("a", &tab(4, NEWS, true), false),
            Err(Away::Elsewhere)
        );
        assert!(
            m.profiles["a"].moments.iter().all(|x| x.to.is_some()),
            "ended when read back"
        );
        // The same Firefox run: kept; a later restart of Firefox: gone.
        m.look(&link(1, "a"), &listing(&tabs, Some(3)));
        assert_eq!(
            m.verdict("a", &tab(3, MAIL, true), false),
            Err(Away::Brought)
        );
        drop(m);
        let mut m = Moves::stored(path.clone());
        m.look(&link(1, "a"), &listing(&[(1, NEWS)], Some(1)));
        assert_eq!(m.verdict("a", &tab(3, MAIL, true), false), Ok(()));
        // A file that does not read starts without marks.
        std::fs::write(&path, "garbage").unwrap();
        let m = Moves::stored(path);
        assert!(m.profiles.is_empty());
    }
}
