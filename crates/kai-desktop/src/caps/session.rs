//! The Session capability (K23): the screen lock, idleness and the battery for `desktop_context`
//! (WP14), and the actions of `lock_screen`, `suspend`, `brightness_set` and `night_light`
//! (WP16b).

use super::Fut;

/// The screen lock and idleness. A part the provider cannot read is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lock {
    pub locked: Option<bool>,
    /// Seconds since the last input.
    pub idle_s: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charge {
    Charging,
    Discharging,
    Full,
    /// Not charging while on mains (a charge limit), or a state the provider names otherwise.
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub percent: f64,
    pub charge: Charge,
}

/// What a provider can do beyond the lock, idleness and the battery: a tool whose feature it lacks
/// is not listed (K23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    pub brightness: bool,
    pub night_light: bool,
}

/// Night Light after `night_light`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightLight {
    /// The screen is tinted now (or will be at its schedule): Kai did not hold it paused, and
    /// nothing else does.
    pub on: bool,
}

pub trait Session: Send + Sync {
    fn lock(&self) -> Fut<'_, Lock>;

    fn features(&self) -> Features;

    /// Locks the screen.
    fn lock_screen(&self) -> Fut<'_, ()>;

    /// Suspends the machine to RAM.
    fn suspend(&self) -> Fut<'_, ()>;

    /// Sets the screen brightness to `percent` of its range; the percent it really is now (a panel
    /// has a lowest step above off, and steps of its own).
    fn set_brightness(&self, percent: u32) -> Fut<'_, u32>;

    /// Resumes or pauses Night Light.
    fn set_night_light(&self, on: bool) -> Fut<'_, NightLight>;

    /// The battery; `None` when the machine has none.
    fn battery(&self) -> Fut<'_, Option<Battery>>;
}
