//! Who looks at screenshots (`delegate.screenshots`, decided by the owner 2026-10-09): the desktop
//! module's tools as Kai's host lists them to its callers.
//!
//! - **`main`** (the default): every tool the desktop serves, `see_window` and `see_screen` among
//!   them; `look` is there too.
//! - **`delegate`**: the image tools (`ToolSpec::returns_images`: `see_window` and `see_screen`) are not listed to callers, so `look` is the
//!   way to see: its delegate gets the screenshots, the caller a short text answer, and the
//!   caller's model needs no vision and little context. The delegate's own tools are the desktop
//!   module's unfiltered (`super::Inner::desktop`), so `look` still sees; its alias must have vision
//!   (`delegate.look`, checked at each task). No tool is renamed or duplicated: the caller's list
//!   only loses two entries, and a call of one of them is refused like any unlisted tool.
//!
//! The setting applies at once: the core hands a change to the host (`ToolHost::set_screenshots`),
//! which sets it here and lists again (`list_changed` when the list changed; K22: a changed list
//! costs one full prefill, so it changes only when the owner changes the setting).

use std::sync::atomic::{AtomicBool, Ordering};

use super::settings::Screenshots;
use crate::host::registry::{Entry, Provider};

/// The desktop module's provider as the host lists it (module doc).
pub struct HostView {
    desktop: Box<dyn Provider>,
    to_delegate: AtomicBool,
}

impl HostView {
    pub fn new(desktop: impl Provider, seen_by: Screenshots) -> Self {
        Self {
            desktop: Box::new(desktop),
            to_delegate: AtomicBool::new(seen_by == Screenshots::Delegate),
        }
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
        let all = self.desktop.entries();
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
