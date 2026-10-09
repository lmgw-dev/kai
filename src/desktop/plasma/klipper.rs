//! The clipboard on Plasma: Klipper's D-Bus interface (`org.kde.klipper` on `/klipper`, which
//! plasmashell's clipboard applet owns in Plasma 6): `setClipboardContents` for WP16b's
//! `clipboard_set` (Klipper keeps it in its history like any copy), `getClipboardContents` for
//! `clipboard_get`,
//! `getClipboardHistoryMenu` for `clipboard_history` (newest first; Klipper keeps as many as its
//! own history size, a setting of the clipboard applet).
//!
//! *Built against the documented interface and a fake;* whether a non-text entry (an image) comes
//! back empty or as a description, and whether the history call answers every entry, is to be
//! verified on a live Plasma session (`spikes/p3/owner-checks.sh b`).

use std::time::Duration;

use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::{Failure, Fut};

use crate::desktop::call;
use crate::host::registry::SessionBus;

pub const KLIPPER: &str = "org.kde.klipper";
const PATH: &str = "/klipper";
const IFACE: &str = "org.kde.klipper.klipper";

pub struct Klipper {
    pub bus: SessionBus,
    pub timeout: Duration,
}

impl Clipboard for Klipper {
    fn set<'a>(&'a self, text: &'a str) -> Fut<'a, ()> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            call::<_, ()>(
                &bus,
                KLIPPER,
                PATH,
                IFACE,
                "setClipboardContents",
                &(text,),
                self.timeout,
            )
            .await
        })
    }

    fn reads(&self) -> bool {
        true
    }

    fn text(&self) -> Fut<'_, String> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            call(
                &bus,
                KLIPPER,
                PATH,
                IFACE,
                "getClipboardContents",
                &(),
                self.timeout,
            )
            .await
        })
    }

    fn history(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            call(
                &bus,
                KLIPPER,
                PATH,
                IFACE,
                "getClipboardHistoryMenu",
                &(),
                self.timeout,
            )
            .await
        })
    }
}
