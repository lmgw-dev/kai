//! The freedesktop Session: logind and UPower on the system bus.
//!
//! The session is the user's graphical one (`/org/freedesktop/login1/user/self`'s `Display`), so
//! it is found from an app scope too, where `session/auto` finds none. Idleness is logind's
//! `IdleHint` with `IdleSinceHint`: a desktop that never sets the hint reads as not idle, so the
//! seconds are said only while the hint is set.
//!
//! The actions (WP16b): `Lock` on that session, `Suspend` on the manager (not interactive: no
//! polkit prompt, a refusal is the answer), and the brightness of the first backlight in sysfs
//! through the session's `SetBrightness` (logind writes the device, Kai needs no rights on it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kai_desktop::caps::session::{Battery, Charge, Features, Lock, NightLight, Session};
use kai_desktop::caps::{Failure, Fut};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::desktop::{SystemBus, call, property};

const LOGIN1: &str = "org.freedesktop.login1";
const UPOWER: &str = "org.freedesktop.UPower";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";

pub struct Logind {
    pub system: SystemBus,
    pub timeout: Duration,
    /// `/sys/class/backlight`.
    pub backlight: PathBuf,
}

/// The graphical session's object path.
pub async fn session_path(bus: &zbus::Connection, timeout: Duration) -> Result<String, Failure> {
    let (_, path): (String, OwnedObjectPath) = property(
        bus,
        LOGIN1,
        "/org/freedesktop/login1/user/self",
        "org.freedesktop.login1.User",
        "Display",
        timeout,
    )
    .await?;
    Ok(if path.as_str() == "/" {
        "/org/freedesktop/login1/session/auto".to_owned()
    } else {
        path.to_string()
    })
}

/// `Suspend` on logind's manager, not interactive.
pub async fn suspend(system: &SystemBus, timeout: Duration) -> Result<(), Failure> {
    let bus = system.get().await.map_err(Failure::Failed)?;
    call::<_, ()>(
        &bus,
        LOGIN1,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        "Suspend",
        &(false,),
        timeout,
    )
    .await
}

/// A backlight in sysfs: its name for logind and its highest step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    pub name: String,
    pub max: u32,
}

/// The backlight to drive in `dir`: a firmware one before a platform one before a raw one (the
/// kernel's own order of preference), then by name. `None` where there is none.
pub fn find_backlight(dir: &Path) -> Option<Backlight> {
    let rank = |kind: &str| match kind.trim() {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    };
    let mut found: Vec<(u8, String, u32)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let max: u32 = std::fs::read_to_string(e.path().join("max_brightness"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let kind = std::fs::read_to_string(e.path().join("type")).unwrap_or_default();
            (max > 0).then(|| {
                (
                    rank(&kind),
                    e.file_name().to_string_lossy().into_owned(),
                    max,
                )
            })
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .next()
        .map(|(_, name, max)| Backlight { name, max })
}

/// The raw step for `percent` of `max`: never 0, which would turn many panels off.
pub fn raw_step(percent: u32, max: u32) -> u32 {
    let raw = (u64::from(percent.min(100)) * u64::from(max) + 50) / 100;
    (raw as u32).clamp(1, max.max(1))
}

/// The percent a raw step is, rounded.
pub fn percent_of(raw: u32, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    ((u64::from(raw) * 100 + u64::from(max) / 2) / u64::from(max)) as u32
}

impl Session for Logind {
    fn features(&self) -> Features {
        Features {
            brightness: find_backlight(&self.backlight).is_some(),
            night_light: false,
        }
    }

    fn lock_screen(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let bus = self.system.get().await.map_err(Failure::Failed)?;
            let path = session_path(&bus, self.timeout).await?;
            call::<_, ()>(
                &bus,
                LOGIN1,
                &path,
                "org.freedesktop.login1.Session",
                "Lock",
                &(),
                self.timeout,
            )
            .await
        })
    }

    fn suspend(&self) -> Fut<'_, ()> {
        Box::pin(suspend(&self.system, self.timeout))
    }

    fn set_brightness(&self, percent: u32) -> Fut<'_, u32> {
        Box::pin(async move {
            let light = find_backlight(&self.backlight)
                .ok_or_else(|| Failure::Refused("this machine has no screen backlight".into()))?;
            let raw = raw_step(percent, light.max);
            let bus = self.system.get().await.map_err(Failure::Failed)?;
            let path = session_path(&bus, self.timeout).await?;
            call::<_, ()>(
                &bus,
                LOGIN1,
                &path,
                "org.freedesktop.login1.Session",
                "SetBrightness",
                &("backlight", light.name.as_str(), raw),
                self.timeout,
            )
            .await?;
            Ok(percent_of(raw, light.max))
        })
    }

    fn set_night_light(&self, _on: bool) -> Fut<'_, NightLight> {
        Box::pin(async {
            Err(Failure::Refused(
                "this desktop has no Night Light Kai can control".into(),
            ))
        })
    }

    fn lock(&self) -> Fut<'_, Lock> {
        Box::pin(async move {
            let bus = self.system.get().await.map_err(Failure::Failed)?;
            let path = session_path(&bus, self.timeout).await?;
            let props: HashMap<String, OwnedValue> = call(
                &bus,
                LOGIN1,
                &path,
                "org.freedesktop.DBus.Properties",
                "GetAll",
                &("org.freedesktop.login1.Session",),
                self.timeout,
            )
            .await?;
            let flag = |k: &str| props.get(k).and_then(|v| bool::try_from(v).ok());
            let since = props
                .get("IdleSinceHint")
                .and_then(|v| u64::try_from(v).ok());
            let idle_s = match (flag("IdleHint"), since) {
                (Some(true), Some(us)) if us > 0 => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_micros() as u64)
                        .unwrap_or(us);
                    Some(now.saturating_sub(us) / 1_000_000)
                }
                _ => None,
            };
            Ok(Lock {
                locked: flag("LockedHint"),
                idle_s,
            })
        })
    }

    fn battery(&self) -> Fut<'_, Option<Battery>> {
        Box::pin(upower_battery(Some(&self.system), self.timeout))
    }
}

/// UPower's display device (the battery the panel shows): `None` when the machine has none.
pub async fn upower_battery(
    system: Option<&SystemBus>,
    timeout: Duration,
) -> Result<Option<Battery>, Failure> {
    let system = system.ok_or_else(|| Failure::Failed("no system bus".into()))?;
    let bus = system.get().await.map_err(Failure::Failed)?;
    let props: HashMap<String, OwnedValue> = call(
        &bus,
        UPOWER,
        DISPLAY_DEVICE,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &("org.freedesktop.UPower.Device",),
        timeout,
    )
    .await?;
    if props.get("IsPresent").and_then(|v| bool::try_from(v).ok()) != Some(true) {
        return Ok(None);
    }
    let percent = props
        .get("Percentage")
        .and_then(|v| f64::try_from(v).ok())
        .unwrap_or(0.0);
    // UPower's Device.State: 1 charging, 2 discharging, 3 empty, 4 fully charged, 5 pending
    // charge, 6 pending discharge.
    let charge = match props.get("State").and_then(|v| u32::try_from(v).ok()) {
        Some(1) => Charge::Charging,
        Some(2) => Charge::Discharging,
        Some(4) => Charge::Full,
        Some(3) => Charge::Other("empty".into()),
        Some(5) => Charge::Other("not charging (pending charge)".into()),
        Some(6) => Charge::Other("pending discharge".into()),
        _ => Charge::Other("state unknown".into()),
    };
    Ok(Some(Battery { percent, charge }))
}
