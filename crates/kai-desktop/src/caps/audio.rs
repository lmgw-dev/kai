//! The Audio capability (K23): the output volume and mute for `volume_set` and `mute` (WP16b).

use super::Fut;

/// One output device, as a provider names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// What the provider addresses it by (the sink's name); the tools never show it.
    pub id: String,
    /// What the owner knows it as ("Speakers", "Headphones").
    pub description: String,
    /// The default output: the one `device` omitted means.
    pub default: bool,
}

pub trait Audio: Send + Sync {
    /// The output devices, the default among them.
    fn outputs(&self) -> Fut<'_, Vec<Device>>;

    /// Sets the volume of the output `id` to `percent` of full scale.
    fn set_volume<'a>(&'a self, id: &'a str, percent: u32) -> Fut<'a, ()>;

    /// Mutes or unmutes the output `id`.
    fn set_mute<'a>(&'a self, id: &'a str, on: bool) -> Fut<'a, ()>;
}
