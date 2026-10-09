//! "The approval card holds the keyboard" (WP19): an in-process flag the card's GTK side sets at
//! its `focus-in` (layer-shell `OnDemand`: KWin moves the keyboard to it at the map and when it is
//! clicked) and clears at its `focus-out` and when the card goes; a card KWin never gives the
//! keyboard never sets it. Kai's own input tools read it before every key they send
//! (`desktop::tools::input`'s focus guard): KWin's window list does not name layer-shell surfaces,
//! so the window it calls active can be another while the keys would land on the card.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

/// The flag; [`Holder::global`] is the one the card sets, a default one is a test's own.
#[derive(Clone, Default)]
pub struct Holder(Arc<AtomicBool>);

static GLOBAL: LazyLock<Holder> = LazyLock::new(Holder::default);

impl Holder {
    /// The card's flag.
    pub fn global() -> Self {
        GLOBAL.clone()
    }

    pub fn holds(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn set(&self, on: bool) {
        self.0.store(on, Ordering::SeqCst);
    }
}
