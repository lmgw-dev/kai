//! The trusted path's desktop (kai-firefox design §6.3, `super::tools::trusted::Desk`) over Kai's
//! desktop module (K23, K24): the window control brings Firefox's window to the front, and the
//! Input capability presses the key the way `key_press` does (`crate::desktop::tools::input`: one
//! input call at a time, the focus guard on the window and the approval card).
//!
//! **Which window is Firefox's** ([`firefox_window`]): the windows of Firefox's main process,
//! which Kai knows from the link (`LinkInfo::firefox_pid`: the parent of the `kai-browser-host`
//! that connected, read through its pidfd), so the window is the linked profile's; where the
//! desktop names no window of that process (a pid Kai could not read, a sandbox's own pid
//! namespace), every window whose app id names Firefox (`org.mozilla.firefox`, `firefox`,
//! `firefox-esr`…). Private windows are left out (their title ends in Firefox's "Private
//! Browsing", English and German as Firefox writes them): the extension never reaches them. Of
//! the rest, the one whose title is the tab's title and Firefox's separator (`"<title> — "`, as
//! Firefox titles a window after its active tab); with one window that is the one; with several
//! and none or more than one of that title (the active one aside), Kai cannot tell and says so,
//! rather than guess. The focus check that follows asks the extension itself, so a wrong window
//! is never pressed in. The extension's window ids are not the desktop's.

use std::time::Duration;

use kai_desktop::caps::windows::{Action, Window};
use serde_json::json;

use super::tools::trusted::{Desk, Fut, Key, Turn};
use crate::desktop::DesktopTools;
use crate::desktop::resolve::window_line;
use crate::desktop::tools::Services;
use crate::host::confirm::quoted;
use crate::host::registry::{Args, Call};

/// How often Kai looks again whether Firefox's window has the focus after it asked for it.
const FOCUS_POLL: Duration = Duration::from_millis(25);

/// The desktop's part of the trusted path (module doc), and the screen lock the key checks.
pub struct DesktopDesk {
    pub desktop: DesktopTools,
    pub screen: crate::screen_lock::ScreenLock,
}

impl DesktopDesk {
    fn services(&self) -> Result<Services, String> {
        self.desktop.services().ok_or_else(|| {
            "Kai has not finished looking at the desktop's capabilities yet; ask again in a moment"
                .to_owned()
        })
    }
}

/// A Private Browsing window, by the title Firefox gives it (module doc).
fn private(w: &Window) -> bool {
    let t = w.title.to_lowercase();
    t.ends_with("private browsing") || t.ends_with("(privater modus)")
}

/// The window of the Firefox whose main process is `pid` and whose active tab is titled `title`
/// (module doc).
pub fn firefox_window<'a>(
    windows: &'a [Window],
    pid: Option<u32>,
    title: &str,
) -> Result<&'a Window, String> {
    let of_pid: Vec<&Window> = match pid {
        Some(p) => windows.iter().filter(|w| w.pid == p).collect(),
        None => Vec::new(),
    };
    let firefox: Vec<&Window> = if of_pid.is_empty() {
        windows
            .iter()
            .filter(|w| w.app.to_lowercase().contains("firefox"))
            .collect()
    } else {
        of_pid
    };
    let normal: Vec<&Window> = firefox.into_iter().filter(|w| !private(w)).collect();
    let prefix = format!("{title} — ");
    let titled: Vec<&Window> = normal
        .iter()
        .copied()
        .filter(|w| !title.is_empty() && w.title.starts_with(&prefix))
        .collect();
    match (titled.as_slice(), normal.as_slice()) {
        ([one], _) => Ok(one),
        ([], [one]) => Ok(one),
        ([], []) => Err(
            "the desktop lists no window of this Firefox (Private Browsing windows aside)".into(),
        ),
        (several, _) => {
            if let [active] = several
                .iter()
                .filter(|w| w.active)
                .collect::<Vec<_>>()
                .as_slice()
            {
                return Ok(active);
            }
            let among = if several.is_empty() { &normal } else { several };
            let lines: Vec<String> = among.iter().map(|w| window_line(w)).collect();
            Err(format!(
                "Kai cannot tell which of Firefox's windows shows the tab {} (the desktop lists: \
                 {}); bring it to the front and ask again",
                quoted(title),
                lines.join("; ")
            ))
        }
    }
}

impl Desk for DesktopDesk {
    fn overlay_holds(&self) -> bool {
        crate::approvals::keyboard::Holder::global().holds()
    }

    fn locked(&self) -> bool {
        self.screen.is_locked()
    }

    fn turn<'a>(&'a self, call: &'a Call) -> Fut<'a, Result<Turn, String>> {
        Box::pin(async move {
            let services = self.services()?;
            let turn =
                crate::desktop::tools::input::take_turn(&services, call.deadline, call.timeout_ms)
                    .await?;
            Ok(Box::new(turn) as Turn)
        })
    }

    fn activate<'a>(
        &'a self,
        firefox: Option<u32>,
        title: &'a str,
    ) -> Fut<'a, Result<Window, String>> {
        Box::pin(async move {
            let services = self.services()?;
            let windows = services.windows.as_ref().ok_or(
                "this desktop offers Kai no window control, so Kai cannot bring Firefox to the \
                 front for a real key",
            )?;
            let snap = windows.snapshot().await.map_err(|e| e.to_string())?;
            let w = firefox_window(&snap.windows, firefox, title)?.clone();
            if w.active {
                return Ok(w);
            }
            windows
                .act(&w.id, &Action::Activate)
                .await
                .map_err(|e| e.to_string())?;
            let end = tokio::time::Instant::now() + services.focus_wait;
            loop {
                let now = windows.active().await.map_err(|e| e.to_string())?;
                if now.as_ref().is_some_and(|a| a.id == w.id) {
                    return Ok(w);
                }
                if tokio::time::Instant::now() >= end {
                    return Err(format!(
                        "Firefox's window {} did not take the keyboard focus within {} s \
                         (dbus.timeout_s); the desktop may keep it from taking it",
                        window_line(&w),
                        services.focus_wait.as_secs_f64()
                    ));
                }
                tokio::time::sleep(FOCUS_POLL).await;
            }
        })
    }

    fn access(
        &self,
    ) -> Option<std::sync::Arc<dyn kai_desktop::caps::accessibility::Accessibility>> {
        self.desktop.services()?.accessibility
    }

    fn press<'a>(&'a self, window: &'a Window, key: Key) -> Fut<'a, Result<(), String>> {
        Box::pin(async move {
            let services = self.services()?;
            let spec = kai_desktop::contract::find("key_press")
                .map(|(_, s)| s)
                .ok_or("this Kai has no key_press")?;
            let keys = match key {
                Key::Enter => "enter",
                Key::Space => "space",
            };
            let args = Args::read(spec, json!({"keys": keys}).as_object().cloned())?;
            if services.input.is_none() {
                return Err(
                    "this desktop offers Kai no keyboard input (Kai's Input capability)".into(),
                );
            }
            // The browser tool that sends the trusted press has applied the site rules to the
            // element itself (its tab and its frame, `focus.check@1` right before); the desktop's
            // checks of a window's tab in front and its focused frame are not for it.
            let services = crate::desktop::tools::Services {
                sites: None,
                ..services
            };
            crate::desktop::tools::input::act(
                &services,
                "key_press",
                &args,
                crate::desktop::tools::input::Confirmed::Focus(window.clone()),
            )
            .await
            .map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str, app: &str, title: &str, pid: u32, active: bool) -> Window {
        Window {
            id: id.into(),
            app: app.into(),
            title: title.into(),
            pid,
            active,
            ..Default::default()
        }
    }

    #[test]
    fn firefoxs_window_is_found_by_its_process_and_its_tabs_title() {
        let ff = "org.mozilla.firefox";
        let all = [
            w(
                "1",
                "org.kde.konsole",
                "Checkout — Mozilla Firefox",
                7,
                false,
            ),
            w("2", ff, "Checkout — Mozilla Firefox", 100, false),
            w("3", ff, "News — Mozilla Firefox", 100, true),
            w(
                "4",
                ff,
                "Checkout — Mozilla Firefox Private Browsing",
                100,
                false,
            ),
            // Another profile's Firefox, the same title.
            w("5", ff, "Checkout — Mozilla Firefox", 200, false),
        ];
        let id = |pid, title| firefox_window(&all, pid, title).map(|w| w.id.clone());
        assert_eq!(
            id(Some(100), "Checkout"),
            Ok("2".into()),
            "not private, not 200's"
        );
        assert_eq!(id(Some(200), "Checkout"), Ok("5".into()));
        assert_eq!(id(Some(200), "Mail"), Ok("5".into()), "its only window");
        assert_eq!(id(Some(100), "News"), Ok("3".into()));
        // An exact prefix: a tab titled "Check" is not "Checkout"'s window.
        let e = id(Some(100), "Check").unwrap_err();
        assert!(e.contains("Kai cannot tell"), "{e}");
        // The pid unknown, or no window of it: by the app id.
        let e = id(None, "Checkout").unwrap_err();
        assert!(e.contains("Kai cannot tell"), "two profiles' windows: {e}");
        assert_eq!(id(Some(999), "News"), Ok("3".into()));
        assert_eq!(id(None, "News"), Ok("3".into()));
        assert!(firefox_window(&all[..1], None, "Checkout").is_err());
        assert!(
            firefox_window(&all[3..4], Some(100), "Checkout").is_err(),
            "private only"
        );
        let twins = [
            w("2", "firefox", "Checkout — Mozilla Firefox", 1, false),
            w("4", "firefox", "Checkout — Mozilla Firefox", 1, true),
        ];
        assert_eq!(firefox_window(&twins, Some(1), "Checkout").unwrap().id, "4");
        let de = [
            w(
                "6",
                ff,
                "Kasse — Mozilla Firefox (Privater Modus)",
                3,
                false,
            ),
            w("7", ff, "Kasse — Mozilla Firefox", 3, false),
        ];
        assert_eq!(firefox_window(&de, Some(3), "Kasse").unwrap().id, "7");
    }
}
