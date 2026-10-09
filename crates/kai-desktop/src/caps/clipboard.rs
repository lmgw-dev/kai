//! The Clipboard capability (K23): `clipboard_set` (WP16b), `clipboard_get` and
//! `clipboard_history` (WP15). On Plasma it is Klipper (`src/desktop/plasma/klipper.rs`), which
//! does all three; the freedesktop `wl-copy` only sets, so where it serves the two reading tools
//! are not listed ([`Clipboard::reads`]).

use super::{Failure, Fut};

/// The Clipboard capability (methods: module doc).
pub trait Clipboard: Send + Sync {
    /// Puts `text` on the clipboard, replacing what it holds.
    fn set<'a>(&'a self, text: &'a str) -> Fut<'a, ()>;

    /// This provider reads the clipboard and its history (`clipboard_get`, `clipboard_history`
    /// are listed).
    fn reads(&self) -> bool {
        false
    }

    /// The text on the clipboard; empty when it holds none.
    fn text(&self) -> Fut<'_, String> {
        Box::pin(async { Err(Failure::Refused("this clipboard cannot be read".into())) })
    }

    /// The clipboard's history, newest first (the current content first).
    fn history(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async { Err(Failure::Refused("this clipboard keeps no history".into())) })
    }
}
