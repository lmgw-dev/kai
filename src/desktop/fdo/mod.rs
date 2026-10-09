//! The `fdo` provider (K23): the freedesktop base, chosen where no desktop-specific provider's probe
//! passes. In this build it serves:
//! - Session: logind on the system bus (the user's graphical session's `LockedHint`, `IdleHint`)
//!   and UPower's display device for the battery ([`session`]);
//! - Media: MPRIS players on the session bus ([`media`]);
//! - Notify: the notification server on the session bus, for `notify` and `ask` ([`notify`]); it
//!   has no do-not-disturb state;
//! - Audio: `pactl` where the audio server answers ([`audio`]);
//! - Clipboard: `wl-copy` on a Wayland session ([`clipboard`]);
//! - Accessibility: AT-SPI, where the session bus has `org.a11y.Bus` ([`atspi`]);
//! - Input: the RemoteDesktop portal with a stored restore token (WP16c, [`input`]);
//! - Search: the recently used files only ([`recent`]; no file index, so `find_files` is not
//!   listed where it serves).
//! - Launch: the user's service manager on the session bus for the app scopes (K14), desktop
//!   entries and `mimeapps.list` ([`launch`]).

pub mod atspi;
pub mod audio;
pub mod clipboard;
pub mod input;
pub mod launch;
pub mod media;
pub mod notify;
pub mod recent;
pub mod session;

use std::sync::Arc;

use kai_desktop::caps::Capability;

use super::probe::{Offer, Probed, ProviderName, Service};
use super::{Env, has_owner};
use audio::Pactl;
use clipboard::WlCopy;
use media::Mpris;
use notify::FdoNotify;
use session::Logind;

pub async fn probe(env: &Env) -> Probed {
    let t = env.timeout;
    let session = match &env.system {
        None => Offer::absent(Capability::Session, "no system bus"),
        // A connection of the probe's own: the services connect on their first call, in the
        // runtime that serves them.
        Some(system) => match system.connect().await {
            Err(e) => Offer::absent(Capability::Session, e),
            Ok(bus) => match has_owner(&bus, "org.freedesktop.login1", t).await {
                Ok(true) => Offer::ready(
                    Service::Session(Arc::new(Logind {
                        system: system.clone(),
                        timeout: t,
                        backlight: env.backlight.clone(),
                    })),
                    "logind; the battery from UPower; the brightness from sysfs",
                ),
                Ok(false) => Offer::absent(Capability::Session, "logind is not on the system bus"),
                Err(e) => Offer::absent(Capability::Session, e),
            },
        },
    };
    let media = match &env.session {
        None => Offer::absent(Capability::Media, "no session bus"),
        Some(bus) => match bus.get().await {
            Ok(_) => Offer::ready(
                Service::Media(Arc::new(Mpris {
                    bus: bus.clone(),
                    timeout: t,
                })),
                "MPRIS",
            ),
            Err(e) => Offer::absent(Capability::Media, e),
        },
    };
    let notify = match &env.session {
        None => Offer::absent(Capability::Notify, "no session bus"),
        Some(bus) => match bus.get().await {
            Err(e) => Offer::absent(Capability::Notify, e),
            Ok(conn) => match has_owner(&conn, "org.freedesktop.Notifications", t).await {
                Ok(true) => Offer::ready(
                    Service::Notify(Arc::new(FdoNotify {
                        bus: bus.clone(),
                        timeout: t,
                    })),
                    "the notification server; no do-not-disturb state",
                ),
                Ok(false) => Offer::absent(
                    Capability::Notify,
                    "no notification server (org.freedesktop.Notifications) on the session bus",
                ),
                Err(e) => Offer::absent(Capability::Notify, e),
            },
        },
    };
    let audio = match &env.pactl {
        None => Offer::absent(Capability::Audio, "pactl is not installed"),
        Some(program) => {
            let pactl = Pactl {
                program: program.clone(),
                timeout: t,
            };
            // The default output is a read-only question: it also tells a running audio server
            // from none.
            match pactl.default_output().await {
                Ok(_) => Offer::ready(Service::Audio(Arc::new(pactl)), "pactl (PipeWire's pulse)"),
                Err(e) => Offer::absent(Capability::Audio, format!("the audio server: {e}")),
            }
        }
    };
    let clipboard = match (&env.wl_copy, env.wayland) {
        (_, false) => Offer::absent(Capability::Clipboard, "no Wayland display"),
        (None, true) => Offer::absent(
            Capability::Clipboard,
            "wl-copy (wl-clipboard) is not installed",
        ),
        (Some(program), true) => Offer::ready(
            Service::Clipboard(Arc::new(WlCopy {
                program: program.clone(),
                timeout: t,
            })),
            "wl-copy",
        ),
    };
    let launch = launch::probe(env).await;
    let mut offers = vec![session, media, notify, audio, clipboard, launch];
    offers.extend(read_offers(env).await);
    offers.push(input::offer(env).await);
    Probed {
        provider: ProviderName::Fdo,
        offers,
    }
}

/// WP15's capabilities: Accessibility, Search.
async fn read_offers(env: &Env) -> Vec<Offer> {
    let t = env.timeout;
    let access = match &env.session {
        None => Offer::absent(Capability::Accessibility, "no session bus"),
        Some(session) => match session.get().await {
            Err(e) => Offer::absent(Capability::Accessibility, e),
            Ok(bus) => match a11y_bus_exists(&bus, t).await {
                Ok(true) => Offer::ready(
                    Service::Accessibility(Arc::new(atspi::Atspi {
                        bus: session.clone(),
                        timeout: t,
                        concurrency: env.read.a11y_concurrency,
                        switch: env.a11y.clone(),
                    })),
                    "AT-SPI",
                ),
                Ok(false) => Offer::absent(
                    Capability::Accessibility,
                    "no accessibility bus (org.a11y.Bus) on the session bus",
                ),
                Err(e) => Offer::absent(Capability::Accessibility, e),
            },
        },
    };
    let search = if env.read.data_home.as_os_str().is_empty() {
        Offer::absent(Capability::Search, "no XDG data directory")
    } else {
        Offer::ready(
            Service::Search(Arc::new(recent::FdoSearch {
                recent: recent::RecentList::in_data_home(&env.read.data_home),
            })),
            "the recent files from recently-used.xbel; no file index",
        )
    };
    vec![access, search]
}

/// `org.a11y.Bus` runs or can be started on the session bus (it is D-Bus activated; the probe
/// starts nothing).
async fn a11y_bus_exists(bus: &zbus::Connection, t: std::time::Duration) -> Result<bool, String> {
    if has_owner(bus, atspi::A11Y_BUS, t).await? {
        return Ok(true);
    }
    let names: Vec<String> = super::call(
        bus,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListActivatableNames",
        &(),
        t,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(names.iter().any(|n| n == atspi::A11Y_BUS))
}
