//! What the Settings page offers where a setting names a thing Kai can list (the owner's rule of
//! 2026-10-19: never a raw identifier to type): the microphones and speakers the tray's menus
//! list, with the names PipeWire gives them, and the screens, by make and model with the
//! connector in brackets. `GET /api/devices`. A list that cannot be read is empty, and the page
//! then offers only the unset choice and the current value.

use serde::{Deserialize, Serialize};

use crate::audio::DeviceList;
use crate::overlay::placement::WlOutput;

/// One thing the owner can pick: the identifier the configuration stores and the name it is shown
/// under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub name: String,
    pub label: String,
}

/// The answer to `GET /api/devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Devices {
    pub microphones: Vec<Choice>,
    pub speakers: Vec<Choice>,
    pub screens: Vec<Choice>,
}

impl Devices {
    /// The audio devices the core knows; the screens are added by [`Self::with_screens`].
    pub fn from_audio(list: &DeviceList) -> Self {
        let choices = |devices: &[crate::audio::Device]| {
            devices
                .iter()
                .map(|d| Choice {
                    name: d.name.clone(),
                    label: d.label.clone(),
                })
                .collect()
        };
        Self {
            microphones: choices(&list.inputs),
            speakers: choices(&list.outputs),
            screens: Vec::new(),
        }
    }

    pub fn with_screens(self, outputs: &[WlOutput]) -> Self {
        Self {
            screens: screens(outputs),
            ..self
        }
    }
}

/// The compositor's outputs by what they are: "Test Monitor 27 (DP-1)", or the connector alone when
/// the compositor says no make or model.
pub fn screens(outputs: &[WlOutput]) -> Vec<Choice> {
    outputs
        .iter()
        .map(|o| {
            let what = format!("{} {}", o.make, o.model);
            let what = what.trim();
            Choice {
                name: o.name.clone(),
                label: if what.is_empty() {
                    o.name.clone()
                } else {
                    format!("{what} ({})", o.name)
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(name: &str, make: &str, model: &str) -> WlOutput {
        WlOutput {
            name: name.into(),
            x: 0,
            y: 0,
            make: make.into(),
            model: model.into(),
        }
    }

    #[test]
    fn screens_are_named_by_what_they_are_with_the_connector_in_brackets() {
        let got = screens(&[out("DP-1", "Test", "Monitor 27"), out("HDMI-A-1", "", "")]);
        assert_eq!(got[0].label, "Test Monitor 27 (DP-1)");
        assert_eq!(got[0].name, "DP-1");
        assert_eq!(got[1].label, "HDMI-A-1");
    }
}
