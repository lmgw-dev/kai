//! The `plasma` provider (K23): KWin and Plasma's own services, chosen before `fdo` where its probe
//! passes. Its probe needs a Plasma session (`XDG_CURRENT_DESKTOP` names KDE) and, per capability:
//! - Windows: `org.kde.KWin` on the session bus ([`windows`]). The facts come from the window
//!   management protocol where KWin grants this binary `org_kde_plasma_window_management` (ruling
//!   16; a `.desktop` naming its path, WP13 (d)), else from KWin scripting, which needs no grant;
//!   the actions always from scripting. The capability table says which;
//! - Session: the screen saver service `org.freedesktop.ScreenSaver` ([`session`]);
//! - Notify: the notification server's `Inhibited` property ([`session`]);
//! - Capture: KWin's ScreenShot2 ([`capture`]; KWin grants it per `.desktop` file);
//! - Search: Baloo's index through `baloosearch6` where it is installed ([`baloo`]), the recent
//!   files from `fdo`'s list;
//! - Clipboard: Klipper (`org.kde.klipper`, [`klipper`]);
//! - Input: KWin's fake input where this build holds the grant (WP16c, [`input`]).

pub mod baloo;
pub mod capture;
pub mod cookie;
pub mod input;
pub mod klipper;
pub mod kwin_script;
pub mod night_light;
pub mod protocol;
pub mod session;
pub mod windows;
pub mod wm;

use std::sync::Arc;

use kai_desktop::caps::Capability;
use kai_desktop::caps::session::Features;

use super::fdo::recent::RecentList;
use super::probe::{Offer, Probed, ProviderName, Service};
use super::{Env, has_owner, property};
use crate::host::registry::SessionBus;
use kwin_script::KwinScript;
use night_light::NightLightControl;
use session::{
    NOTIFICATIONS, NOTIFICATIONS_PATH, PlasmaNotify, PlasmaSession, brightness_displays,
};
use windows::PlasmaWindows;
use wm::{Unavailable, Wm};

/// The restricted interface of the window management protocol (ruling 16).
pub const WINDOW_MANAGEMENT: &str = "org_kde_plasma_window_management";

pub async fn probe(env: &Env) -> Probed {
    let caps = [
        Capability::Windows,
        Capability::Session,
        Capability::Notify,
        Capability::Capture,
        Capability::Search,
        Capability::Clipboard,
        Capability::Input,
    ];
    let all_absent = |why: String| Probed {
        provider: ProviderName::Plasma,
        offers: caps
            .iter()
            .map(|&c| Offer::absent(c, why.clone()))
            .collect(),
    };
    if !env.is_plasma() {
        return all_absent(format!(
            "not a Plasma session (XDG_CURRENT_DESKTOP={:?})",
            env.current_desktop
        ));
    }
    let Some(session) = &env.session else {
        return all_absent("no session bus".into());
    };
    let bus = match session.get().await {
        Ok(b) => b,
        Err(e) => return all_absent(e),
    };
    let t = env.timeout;
    let windows = match has_owner(&bus, "org.kde.KWin", t).await {
        Ok(true) => {
            let script = KwinScript::new(session.clone(), env.runtime_dir.join("kwin"), t);
            let (wm, note) = window_management(env);
            Offer::ready(
                Service::Windows(Arc::new(PlasmaWindows::new(wm, script, session.clone(), t))),
                note,
            )
        }
        Ok(false) => Offer::absent(Capability::Windows, "KWin is not on the session bus"),
        Err(e) => Offer::absent(Capability::Windows, e),
    };
    let lock = match has_owner(&bus, "org.freedesktop.ScreenSaver", t).await {
        Ok(true) => {
            let features = Features {
                brightness: brightness_displays(&bus, t)
                    .await
                    .is_ok_and(|d| !d.is_empty()),
                night_light: NightLightControl::available(&bus, t).await,
            };
            Offer::ready(
                Service::Session(Arc::new(PlasmaSession {
                    bus: session.clone(),
                    system: env.system.clone(),
                    timeout: t,
                    features,
                    night: NightLightControl::new(session.clone(), t),
                })),
                "the screen saver service; the battery from UPower; the brightness from \
                 PowerDevil, Night Light from KWin",
            )
        }
        Ok(false) => Offer::absent(
            Capability::Session,
            "no screen saver service (org.freedesktop.ScreenSaver) on the session bus",
        ),
        Err(e) => Offer::absent(Capability::Session, e),
    };
    let dnd = match property::<bool>(
        &bus,
        NOTIFICATIONS,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS,
        "Inhibited",
        t,
    )
    .await
    {
        Ok(_) => Offer::ready(
            Service::Notify(Arc::new(PlasmaNotify::new(session.clone(), t))),
            "the notification server's Inhibited",
        ),
        Err(e) => Offer::absent(
            Capability::Notify,
            format!("the notification server has no do-not-disturb state ({e})"),
        ),
    };
    let mut offers = vec![windows, lock, dnd];
    offers.extend(read_offers(env, session, &bus).await);
    offers.push(input::offer(env).await);
    Probed {
        provider: ProviderName::Plasma,
        offers,
    }
}

/// WP15's capabilities: Capture, Search, Clipboard.
async fn read_offers(env: &Env, session: &SessionBus, bus: &zbus::Connection) -> Vec<Offer> {
    let t = env.timeout;
    let capture = match capture::serves(bus, t).await {
        Ok(true) => Offer::ready(
            Service::Capture(Arc::new(capture::KwinCapture {
                bus: session.clone(),
                timeout: t,
            })),
            "KWin's ScreenShot2 (granted per .desktop file: a build without the grant says so at \
             its first capture)",
        ),
        Ok(false) => Offer::absent(Capability::Capture, "KWin serves no ScreenShot2"),
        Err(e) => Offer::absent(Capability::Capture, e),
    };
    let search = match &env.read.baloosearch {
        Some(program) => Offer::ready(
            Service::Search(Arc::new(baloo::Baloo {
                program: program.clone(),
                config_home: env.read.config_home.clone(),
                home: env.read.home.clone(),
                bus: session.clone(),
                timeout: t,
                recent: RecentList::in_data_home(&env.read.data_home),
            })),
            "Baloo's index through baloosearch6; the recent files from recently-used.xbel",
        ),
        None => Offer::absent(Capability::Search, "no baloosearch6 on PATH"),
    };
    let clipboard = match has_owner(bus, klipper::KLIPPER, t).await {
        Ok(true) => Offer::ready(
            Service::Clipboard(Arc::new(klipper::Klipper {
                bus: session.clone(),
                timeout: t,
            })),
            "Klipper",
        ),
        Ok(false) => Offer::absent(
            Capability::Clipboard,
            "Klipper (org.kde.klipper) is not on the session bus",
        ),
        Err(e) => Offer::absent(Capability::Clipboard, e),
    };
    vec![capture, search, clipboard]
}

/// The window management protocol's connection where KWin grants it, and the table's note.
fn window_management(env: &Env) -> (Option<Wm>, String) {
    let scripting = "KWin scripting for the window facts and the actions";
    if !env.wayland {
        return (
            None,
            format!("{scripting}; the window management protocol not tried (no Wayland display)"),
        );
    }
    match Wm::start(env.timeout) {
        Ok(wm) => (
            Some(wm),
            "the window management protocol for the window facts, KWin scripting for the actions"
                .into(),
        ),
        Err(Unavailable::NotGranted) => (
            None,
            format!(
                "{scripting}; the window management protocol is not granted (KWin grants \
                 {WINDOW_MANAGEMENT} per .desktop file)"
            ),
        ),
        Err(Unavailable::Failed(e)) => (
            None,
            format!("{scripting}; the window management protocol failed: {e}"),
        ),
    }
}
