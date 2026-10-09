//! KWin's Night Light (`org.kde.KWin.NightLight` on the session bus): `night_light` of WP16b.
//!
//! The schedule and temperature are the user's settings, so Kai does not change them. It pauses
//! and resumes: `inhibit` pauses Night Light until `uninhibit` with the cookie it returned (or
//! until the connection that asked goes away, which KWin treats as the end of the pause), the
//! way Plasma's own "pause" does. Night Light switched off in the settings cannot be resumed by
//! Kai, and the tool says so.

use std::time::Duration;

use kai_desktop::caps::session::NightLight;
use kai_desktop::caps::{Failure, Fut};

use super::cookie::{Cookie, UNBOUNDED};
use crate::desktop::{call, property};
use crate::host::registry::SessionBus;

pub const KWIN: &str = "org.kde.KWin";
pub const PATH: &str = "/org/kde/KWin/NightLight";
pub const IFACE: &str = "org.kde.KWin.NightLight";

pub struct NightLightControl {
    pub bus: SessionBus,
    pub timeout: Duration,
    /// The cookie of Kai's own pause, while there is one.
    pub cookie: Cookie,
}

impl NightLightControl {
    pub fn new(bus: SessionBus, timeout: Duration) -> Self {
        Self {
            bus,
            timeout,
            cookie: Cookie::new(timeout),
        }
    }

    /// KWin has Night Light (the probe's question).
    pub async fn available(bus: &zbus::Connection, timeout: Duration) -> bool {
        property::<bool>(bus, KWIN, PATH, IFACE, "available", timeout)
            .await
            .unwrap_or(false)
    }

    /// Resumes (`on`) or pauses Night Light.
    pub fn set(&self, on: bool) -> Fut<'_, NightLight> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let t = self.timeout;
            let enabled: bool = property(&bus, KWIN, PATH, IFACE, "enabled", t).await?;
            let conn = bus.clone();
            if on {
                self.cookie
                    .release(move |c| async move {
                        call::<_, ()>(&conn, KWIN, PATH, IFACE, "uninhibit", &(c,), UNBOUNDED).await
                    })
                    .await?;
                if !enabled {
                    return Err(Failure::Refused(
                        "Night Light is switched off in the system settings; Kai only pauses \
                         and resumes it"
                            .into(),
                    ));
                }
            } else {
                self.cookie
                    .hold(async move {
                        call::<_, u32>(&conn, KWIN, PATH, IFACE, "inhibit", &(), UNBOUNDED).await
                    })
                    .await?;
            }
            let paused: bool = property(&bus, KWIN, PATH, IFACE, "inhibited", t).await?;
            Ok(NightLight {
                on: enabled && !paused,
            })
        })
    }
}
