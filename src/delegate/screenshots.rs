//! Who looks at screenshots (`delegate.screenshots`, decided by the owner 2026-10-09): the tools
//! that return images (`ToolSpec::returns_images`) as Kai's host lists them to its callers: the
//! desktop module's `see_window` and `see_screen`, and the browser's `page_see` and `element_see`
//! (kai-firefox design §5, WP7).
//!
//! - **`main`** (the default): every tool the desktop and the browser serve, the image tools among
//!   them; `look` is there too.
//! - **`delegate`**: the image tools are not listed to callers, so `look` is the way to see: its
//!   delegate gets the screenshots, the caller a short text answer, and the caller's model needs
//!   no vision and little context. The delegate's own tools are the desktop module's unfiltered
//!   (`super::Inner::desktop`), so `look` still sees; its alias must have vision (`delegate.look`,
//!   checked at each task). No tool is renamed or duplicated: the caller's list only loses the
//!   image tools, and a call of one of them is refused like any unlisted tool.
//!
//! **Kai's browser site rules in what `look` sees.** `look` has no browser tools yet, so it sees a
//! page in Firefox through `see_window` on Firefox's window or through `see_screen`, as a caller
//! does in `main`. Both follow the site rules by the tab in front of Firefox's window
//! (`crate::browser::shown`, `crate::desktop::sites`, kai-firefox WP7b): `see_window` refuses a
//! window whose tab in front is on a `none` site (or one whose page Kai cannot tell, while some
//! site is `none`), and `see_screen` blanks such windows grey. **Not covered yet:** what the
//! desktop shows of Firefox's window besides that tab's own site, a frame of a `none` site inside
//! an `act` page, a split view's second tab, the sidebar or a `none` site's title in the tab
//! strip, is in the image as it is (the answer says so); only `page_see` and `element_see` blank
//! such frames (kai-firefox design §6.4, `crate::browser::tools::see`). Kai's design records it
//! under "Later".
//!
//! Each provider with image tools is listed through a [`HostView`]: the desktop's
//! ([`host_view`]), and the browser's ([`HostView::also`]), which follows the same setting. The
//! setting applies at once: the core hands a change to the host (`ToolHost::set_screenshots`),
//! which sets it on the first view, and so on every view, and lists again (`list_changed` when the
//! list changed; K22: a changed list costs one full prefill, so it changes only when the owner
//! changes the setting).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::settings::Screenshots;
use crate::host::registry::{Entry, Provider};

/// A provider with image tools as the host lists it (module doc).
pub struct HostView {
    inner: Box<dyn Provider>,
    /// Shared by every view of one host ([`HostView::also`]).
    to_delegate: Arc<AtomicBool>,
}

impl HostView {
    pub fn new(desktop: impl Provider, seen_by: Screenshots) -> Self {
        Self {
            inner: Box::new(desktop),
            to_delegate: Arc::new(AtomicBool::new(seen_by == Screenshots::Delegate)),
        }
    }

    /// `other`'s tools under the same setting as this view's: one change sets both.
    pub fn also(&self, other: impl Provider) -> Self {
        Self {
            inner: Box::new(other),
            to_delegate: self.to_delegate.clone(),
        }
    }

    /// The provider it lists, as `Any` (`Provider::as_any`): the tests' check that the browser's
    /// provider is listed through a view.
    #[cfg(test)]
    pub fn wrapped(&self) -> Option<&dyn std::any::Any> {
        self.inner.as_any()
    }

    /// Who looks at screenshots from now on; the host lists again.
    pub fn set(&self, seen_by: Screenshots) {
        self.to_delegate
            .store(seen_by == Screenshots::Delegate, Ordering::SeqCst);
    }

    pub fn seen_by(&self) -> Screenshots {
        if self.to_delegate.load(Ordering::SeqCst) {
            Screenshots::Delegate
        } else {
            Screenshots::Main
        }
    }
}

impl Provider for HostView {
    fn entries(&self) -> Vec<Entry> {
        let all = self.inner.entries();
        match self.seen_by() {
            Screenshots::Main => all,
            Screenshots::Delegate => all.into_iter().filter(|e| !e.spec.returns_images).collect(),
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The view `registry::providers` lists in place of the desktop module's provider.
pub fn host_view(deps: &crate::host::registry::Deps, desktop: impl Provider) -> HostView {
    HostView::new(desktop, deps.config.delegate.screenshots)
}
