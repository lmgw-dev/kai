//! The browser tools' provider (kai-firefox design D3; kai K23): one line in the host registry's
//! `providers`, after every other, so a list that changes when Firefox starts or quits leaves the
//! cached prefix of Kai's other tools alone.
//!
//! **What is listed** (D3, decided by the owner 2026-11-03): a tool of `contract::browser` that is
//! built (`super::tools::route`) and whose operations one linked extension serves in full; the
//! union over the linked profiles. The eight tools that exist only for hands and eyes are listed
//! only while the hands-and-eyes switch is on (`browser.hands_and_eyes`), and `firefox_ui` and
//! `firefox_press` only where the desktop offers accessibility (kai-firefox WP12). A link that comes or
//! goes signals the host's registry (`super::links`), and so does a change of the switch (the
//! host's policy, `HostLink::set_policy`), so the host lists again and lmgw gets `list_changed`
//! only when the list changed.
//!
//! Listing is also where the policy reaches the announcements (`super::announce`): every write of
//! the host's policy lists again, so `entries` hands the browser's part to them, and they turn
//! the extensions' events on or off.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use kai_desktop::contract::browser::{AREA, hands_and_eyes_only};

use super::announce::Announcer;
use super::links::{LinkInfo, Links};
use super::refs::{Refs, STORE};
use super::spans::Spans;
use super::tools::trusted::Desk;
use super::tools::{self, Inner, Route};
use crate::host::confirm::{Confirmer, Dialog};
use crate::host::policy::SharedPolicy;
use crate::host::registry::{Deps, Entry, Provider};
use crate::voice::speak::Mouth;

/// The provider (module doc).
pub struct BrowserTools {
    inner: Arc<Inner>,
}

impl BrowserTools {
    /// `refs`: Kai's numbers for the extensions' element refs (`super::refs`), kept in Kai's state
    /// directory; `confirmer`: Kai's dialog for the questions a call asks while it runs (the
    /// commit guard's, the trusted key's); `desk`: the trusted path's desktop; `spans`: Kai for
    /// Firefox's input spans and the focus rule's marks, shared with the desktop's input tools.
    /// `mouth`: where Kai's speech without a model goes (the core's, `crate::voice::speak`).
    pub fn new(
        links: Links,
        policy: SharedPolicy,
        refs: Refs,
        confirmer: Arc<dyn Confirmer>,
        desk: Arc<dyn Desk>,
        spans: Spans,
        mouth: Arc<dyn Mouth>,
    ) -> Self {
        Self::built(
            links,
            policy,
            refs,
            tools::route,
            confirmer,
            desk,
            spans,
            mouth,
        )
    }

    /// With another route table and numbers kept nowhere, asking nobody and without a desktop (the
    /// tests' listing).
    #[cfg(test)]
    pub fn with_routes(links: Links, policy: SharedPolicy, routes: fn(&str) -> Route) -> Self {
        struct Nobody;
        impl Confirmer for Nobody {
            fn ask(&self, _: crate::host::confirm::ConfirmRequest) {}
        }
        let spans = Spans::new(
            links.clone(),
            policy.clone(),
            super::moves::Moves::default(),
        );
        Self::built(
            links,
            policy,
            Refs::default(),
            routes,
            Arc::new(Nobody),
            Arc::new(super::tools::trusted::NoDesk),
            spans,
            Arc::new(Mute),
        )
    }

    /// The tests' provider: their routes, confirmer, desk and mouth.
    #[cfg(test)]
    pub fn with_parts(
        links: Links,
        policy: SharedPolicy,
        routes: fn(&str) -> Route,
        confirmer: Arc<dyn Confirmer>,
        desk: Arc<dyn Desk>,
        spans: Spans,
        mouth: Arc<dyn Mouth>,
    ) -> Self {
        Self::built(
            links,
            policy,
            Refs::default(),
            routes,
            confirmer,
            desk,
            spans,
            mouth,
        )
    }

    /// The job table Kai's voice says in whether its continuation for jobs' results runs
    /// (`crate::jobs::Jobs::continuing`): meanwhile no call's move is the owner's own for the
    /// focus rule (`super::tools::owners_move`, kai-firefox WP10b's review, M3). Set before the
    /// provider is shared.
    pub fn with_jobs(mut self, jobs: crate::jobs::Jobs) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("the provider is not shared yet")
            .jobs = jobs;
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn built(
        links: Links,
        policy: SharedPolicy,
        refs: Refs,
        routes: fn(&str) -> Route,
        confirmer: Arc<dyn Confirmer>,
        desk: Arc<dyn Desk>,
        spans: Spans,
        mouth: Arc<dyn Mouth>,
    ) -> Self {
        let announcer = Announcer::new(links.clone(), policy.clone(), mouth.clone());
        Self {
            inner: Arc::new(Inner {
                announcer,
                mouth,
                reading: Mutex::default(),
                links,
                policy,
                spans,
                refs: Mutex::new(refs),
                routes,
                confirmer,
                desk,
                gates: Mutex::default(),
                following: Default::default(),
                sheets: Mutex::default(),
                jobs: crate::jobs::Jobs::default(),
            }),
        }
    }

    /// The entries for the extensions `linked`, with the switch `hands_and_eyes` (module doc).
    pub fn entries_for(&self, linked: &[LinkInfo], hands_and_eyes: bool) -> Vec<Entry> {
        let served: Vec<HashSet<&str>> = linked
            .iter()
            .map(|l| l.ops.iter().map(String::as_str).collect())
            .collect();
        // Firefox's own UI is read through the desktop's accessibility (kai-firefox WP12).
        let access = self.inner.desk.access().is_some();
        AREA.tools
            .iter()
            .filter(|spec| hands_and_eyes || !hands_and_eyes_only(spec.name))
            .filter(|spec| access || !needs_access(spec.name))
            .filter_map(|spec| match (self.inner.routes)(spec.name) {
                Route::Built { ops, reach, run }
                    if served.iter().any(|s| ops.iter().all(|op| s.contains(op))) =>
                {
                    let inner = self.inner.clone();
                    Some(Entry {
                        area: AREA.name,
                        spec,
                        handler: Arc::new(move |call| run(inner.clone(), call)),
                        prepare: tools::prepared(spec, reach)
                            .then(|| tools::preparer(self.inner.clone(), spec, reach)),
                    })
                }
                _ => None,
            })
            .collect()
    }
}

impl Provider for BrowserTools {
    fn entries(&self) -> Vec<Entry> {
        let browser = self.inner.browser();
        // The announcements follow the policy the host lists with (module doc).
        self.inner.announcer.follow(&browser);
        self.entries_for(&self.inner.links.list(), browser.hands_and_eyes)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// `name` reads or presses through the desktop's accessibility (kai-firefox WP12): listed only
/// where the desktop offers it.
pub fn needs_access(name: &str) -> bool {
    matches!(name, "firefox_ui" | "firefox_press")
}

/// A mouth that says nothing (the tests' listing).
#[cfg(test)]
struct Mute;

#[cfg(test)]
impl Mouth for Mute {
    fn say(&self, _: crate::voice::speak::Utterance) {}
}

/// The provider `registry::providers` lists (through the view of who looks at screenshots,
/// `crate::delegate::screenshots::HostView`, for `page_see` and `element_see`); Kai's numbers for the refs are kept in Kai's state
/// directory when there is one, its questions go to Kai's dialog (and, for a call of Kai's own, to
/// its voice: `crate::approvals::asked`), the trusted path uses the
/// desktop module's services (`desktop`, the provider the registry lists) and the screen lock,
/// and `spans` are shared with the desktop's input tools.
pub fn provider(deps: &Deps, desktop: crate::desktop::DesktopTools, spans: Spans) -> BrowserTools {
    let refs = deps
        .state_dir
        .as_ref()
        .map_or_else(Refs::default, |d| Refs::stored(d.join(STORE)));
    BrowserTools::new(
        deps.browser.clone(),
        deps.policy.clone(),
        refs,
        Arc::new(crate::approvals::asked::VoiceConfirmer::to_core(
            Arc::new(Dialog(deps.ui.clone())),
            deps.cmds.clone(),
        )),
        Arc::new(super::desk::DesktopDesk {
            desktop,
            screen: deps.screen.clone(),
        }),
        spans,
        Arc::new(deps.cmds.clone()),
    )
    .with_jobs(deps.jobs.clone())
}
