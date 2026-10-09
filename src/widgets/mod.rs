//! Tool-controlled overlays (ruling 6, ruling 24, §10.1, K1, K18's no-bridge half; WP10): the
//! agent shows widgets borderless over the desktop, like the ring.
//!
//! - [`tools`]: the hosted tools `overlay_text`, `overlay_html`, `overlay_hide`, `overlay_list`
//!   (`contract/overlay.rs`), one provider line in `host::registry::providers`; their calls go to
//!   the GTK thread as `UiCmd::Widgets`.
//! - [`gtk_host`]: the widgets on the GTK thread, one layer-shell surface each, related views in
//!   one shared web process of their own, separate from the ring's (K1); [`view`] the surface and
//!   view, [`sandbox`] the sandbox's rules, [`filter`] the content filter, [`features`] the WebKit
//!   features turned off by identifier, [`input`] the input region, [`table`] the pure
//!   bookkeeping, [`kinds`] what each show tool's arguments become ([`fetch`] the one HTTP fetch
//!   of images and weather, [`image`] pictures and emotes, [`weather`] Bright Sky's data), [`settings`] `[overlay]`'s
//!   per-kind defaults.
//! - [`app_bridge`]: `overlay_app`'s MCP Apps host bridge (K18, WP12): the view's document read
//!   through lmgw's `/mcp` under Kai's key, its JSON-RPC answered for its own server only;
//!   [`thread_apps`]: the views of the tool calls in Kai's own conversation, shown by themselves
//!   with the call's input and result (`[overlay.app] auto_show`).
//! - **Nothing lingers unseen** (§10.1): the tray's Overlays menu lists them with Hide and Hide
//!   all, and the `hide-overlays` shortcut hides all ([`hide`], from any thread).
//! - **No view over Kai's dialogs** (§10.1): while a Kai confirmation is open ([`DialogOpen`]),
//!   every MCP Apps view is click-through and shrinks to Kai's strip alone, saying it waits, on an
//!   output edge away from the dialog (`view::held_place`; the owner's decision of 2026-10-09);
//!   after, it is as configured. An interactive one carries Kai's strip naming its server
//!   (`view`).

pub mod app_bridge;
pub mod features;
pub mod fetch;
pub mod filter;
pub mod gtk_host;
pub mod image;
pub mod input;
pub mod kinds;
pub mod sandbox;
pub mod settings;
pub mod table;
pub mod thread_apps;
pub mod tools;
pub mod view;
pub mod weather;

#[cfg(test)]
pub(crate) mod live_tests;

pub use gtk_host::{WidgetsCmd, start};
pub use tools::provider;

/// Hides widget `id`, or every widget: the tray's Hide and Hide all, the `hide-overlays` shortcut.
/// From any thread; nothing happens while the GTK side is not up (then nothing is shown).
pub fn hide(id: Option<u64>) {
    remote::send(WidgetsCmd::Hide { id, reply: None });
}

/// A Kai confirmation dialog is open (K15/PD4's tool dialog, which a view's own calls use too;
/// K13's pairing link): while any is, every MCP Apps view is click-through and only Kai's strip,
/// away from the dialog, so it can neither cover nor catch a click meant for the dialog nor pass
/// for it (§10.1). Held by the dialog, on the
/// GTK thread, from its build to its destroy.
pub struct DialogOpen(());

static DIALOGS_OPEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl DialogOpen {
    #[allow(clippy::new_without_default)]
    pub fn new() -> DialogOpen {
        if DIALOGS_OPEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            remote::send(WidgetsCmd::Guard(true));
        }
        DialogOpen(())
    }
}

impl Drop for DialogOpen {
    fn drop(&mut self) {
        if DIALOGS_OPEN.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            remote::send(WidgetsCmd::Guard(false));
        }
    }
}

/// What an MCP Apps view held for a Kai dialog takes from its output's top or bottom edge, in
/// application pixels: Kai's strip and `[overlay.app] margin` ([`view::held_place`]). Set by
/// [`provider`] from the configuration it reads at start, which every view's margin comes from; 0
/// before (no view exists then). The tool dialog keeps this much free above and below itself
/// (`crate::host::confirm::args_max_height`), so a centred dialog cannot meet a held strip.
pub fn held_reserve() -> u32 {
    HELD_RESERVE.load(std::sync::atomic::Ordering::SeqCst)
}

static HELD_RESERVE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// [`held_reserve`] for views with `margin` (`[overlay.app] margin`).
fn set_held_margin(margin: u32) {
    HELD_RESERVE.store(
        view::STRIP_PX.saturating_add(margin),
        std::sync::atomic::Ordering::SeqCst,
    );
}

/// The tray's Overlays entries `(id, label)`, as they change.
pub fn tray_entries() -> tokio::sync::watch::Receiver<Vec<(u64, String)>> {
    remote::TRAY.subscribe()
}

/// The GTK side's channel for the tray and the shortcut, and the tray's list: set while the widgets
/// run (from [`start`] to their quit), so neither needs a path through the core.
mod remote {
    use std::sync::{LazyLock, Mutex};

    use tokio::sync::watch;

    use super::gtk_host::WidgetsCmd;

    static TX: Mutex<Option<async_channel::Sender<WidgetsCmd>>> = Mutex::new(None);

    pub static TRAY: LazyLock<watch::Sender<Vec<(u64, String)>>> =
        LazyLock::new(|| watch::channel(Vec::new()).0);

    pub fn set(tx: async_channel::Sender<WidgetsCmd>) {
        *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    }

    pub fn clear() {
        TX.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    pub fn send(cmd: WidgetsCmd) {
        if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.try_send(cmd);
        }
    }

    pub fn tell_tray(entries: Vec<(u64, String)>) {
        TRAY.send_if_modified(|now| {
            let changed = *now != entries;
            *now = entries;
            changed
        });
    }
}
