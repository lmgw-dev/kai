//! Plasma's Session and Notify parts for `desktop_context`: the lock and idleness from the screen
//! saver service Plasma runs (`org.freedesktop.ScreenSaver` on `/ScreenSaver`: `GetActive`,
//! `GetSessionIdleTime` in seconds), the battery from UPower (`fdo`), and do not disturb from
//! Plasma's notification server (its `Inhibited` property, which the freedesktop specification
//! does not have).

use std::collections::HashMap;
use std::time::Duration;

use kai_desktop::caps::notify::{Asked, Dnd, Notify};
use kai_desktop::caps::session::{Battery, Features, Lock, NightLight, Session};
use kai_desktop::caps::{Failure, Fut};
use zbus::zvariant::Value;

use super::cookie::{Cookie, UNBOUNDED};
use super::night_light::NightLightControl;
use crate::desktop::fdo::notify;
use crate::desktop::fdo::session::{percent_of, raw_step, suspend, upower_battery};
use crate::desktop::{SystemBus, call, property};
use crate::host::registry::SessionBus;

pub struct PlasmaSession {
    pub bus: SessionBus,
    pub system: Option<SystemBus>,
    pub timeout: Duration,
    /// What the probe found beyond the lock: PowerDevil's screen brightness, KWin's Night Light.
    pub features: Features,
    pub night: NightLightControl,
}

/// PowerDevil's screen brightness service: one display object per screen it can drive.
const BRIGHTNESS: &str = "org.kde.ScreenBrightness";
const BRIGHTNESS_PATH: &str = "/org/kde/ScreenBrightness";

/// The names of the displays whose brightness PowerDevil can set.
pub async fn brightness_displays(
    bus: &zbus::Connection,
    timeout: Duration,
) -> Result<Vec<String>, Failure> {
    property(
        bus,
        BRIGHTNESS,
        BRIGHTNESS_PATH,
        BRIGHTNESS,
        "DisplaysDBusNames",
        timeout,
    )
    .await
}

const SAVER: &str = "org.freedesktop.ScreenSaver";

impl Session for PlasmaSession {
    fn features(&self) -> Features {
        self.features
    }

    fn lock_screen(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            call::<_, ()>(
                &bus,
                SAVER,
                "/ScreenSaver",
                SAVER,
                "Lock",
                &(),
                self.timeout,
            )
            .await
        })
    }

    fn suspend(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let system = self
                .system
                .as_ref()
                .ok_or_else(|| Failure::Failed("no system bus".into()))?;
            suspend(system, self.timeout).await
        })
    }

    fn set_brightness(&self, percent: u32) -> Fut<'_, u32> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let t = self.timeout;
            let displays = brightness_displays(&bus, t).await?;
            if displays.is_empty() {
                return Err(Failure::Refused("no screen's brightness can be set".into()));
            }
            // Every screen PowerDevil drives, each to the same share of its own range; the answer
            // is the first one's.
            let mut first = None;
            for name in &displays {
                let path = format!("{BRIGHTNESS_PATH}/{name}");
                let iface = "org.kde.ScreenBrightness.Display";
                let max: i32 = property(&bus, BRIGHTNESS, &path, iface, "MaxBrightness", t).await?;
                let max = u32::try_from(max).unwrap_or(0);
                let raw = raw_step(percent, max);
                call::<_, ()>(
                    &bus,
                    BRIGHTNESS,
                    &path,
                    iface,
                    "SetBrightness",
                    &(raw as i32, 0u32),
                    t,
                )
                .await?;
                first.get_or_insert(percent_of(raw, max));
            }
            Ok(first.unwrap_or(percent))
        })
    }

    fn set_night_light(&self, on: bool) -> Fut<'_, NightLight> {
        self.night.set(on)
    }

    fn lock(&self) -> Fut<'_, Lock> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let locked: bool = call(
                &bus,
                SAVER,
                "/ScreenSaver",
                SAVER,
                "GetActive",
                &(),
                self.timeout,
            )
            .await?;
            let idle: u32 = call(
                &bus,
                SAVER,
                "/ScreenSaver",
                SAVER,
                "GetSessionIdleTime",
                &(),
                self.timeout,
            )
            .await?;
            Ok(Lock {
                locked: Some(locked),
                idle_s: Some(u64::from(idle)),
            })
        })
    }

    fn battery(&self) -> Fut<'_, Option<Battery>> {
        Box::pin(upower_battery(self.system.as_ref(), self.timeout))
    }
}

pub struct PlasmaNotify {
    pub bus: SessionBus,
    pub timeout: Duration,
    /// The cookie of Kai's own inhibition (do not disturb), while there is one. The server drops
    /// it with Kai's connection.
    pub inhibit: Cookie,
}

impl PlasmaNotify {
    pub fn new(bus: SessionBus, timeout: Duration) -> Self {
        Self {
            bus,
            timeout,
            inhibit: Cookie::new(timeout),
        }
    }
}

pub const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
pub const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

impl Notify for PlasmaNotify {
    fn can_dnd(&self) -> bool {
        true
    }

    fn set_dnd(&self, on: bool) -> Fut<'_, Dnd> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let t = self.timeout;
            let conn = bus.clone();
            if on {
                self.inhibit
                    .hold(async move {
                        let hints: HashMap<&str, Value<'_>> = HashMap::new();
                        call::<_, u32>(
                            &conn,
                            NOTIFICATIONS,
                            NOTIFICATIONS_PATH,
                            NOTIFICATIONS,
                            "Inhibit",
                            &(crate::APP_ID, "do not disturb, asked through Kai", hints),
                            UNBOUNDED,
                        )
                        .await
                    })
                    .await?;
            } else {
                self.inhibit
                    .release(move |c| async move {
                        call::<_, ()>(
                            &conn,
                            NOTIFICATIONS,
                            NOTIFICATIONS_PATH,
                            NOTIFICATIONS,
                            "UnInhibit",
                            &(c,),
                            UNBOUNDED,
                        )
                        .await
                    })
                    .await?;
            }
            let on: bool = property(
                &bus,
                NOTIFICATIONS,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS,
                "Inhibited",
                t,
            )
            .await?;
            Ok(Dnd { on })
        })
    }

    fn show<'a>(&'a self, title: &'a str, body: &'a str) -> Fut<'a, ()> {
        Box::pin(notify::show(&self.bus, title, body, self.timeout))
    }

    fn ask<'a>(&'a self, question: &'a str, choices: &'a [String]) -> Fut<'a, Asked> {
        Box::pin(async move {
            // Kai's own do not disturb counts as well as the owner's: both make `Inhibited` true.
            let under_dnd = self.dnd().await?;
            notify::ask(&self.bus, question, choices, under_dnd, self.timeout).await
        })
    }

    fn dnd(&self) -> Fut<'_, bool> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            property(
                &bus,
                NOTIFICATIONS,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS,
                "Inhibited",
                self.timeout,
            )
            .await
        })
    }
}
