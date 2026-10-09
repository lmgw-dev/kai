//! `[desktop]` (K23; `crate::config` holds the field): which provider serves a capability, where
//! the owner wants another than the probe's choice.
//!
//! ```toml
//! [desktop]               # K23: per capability, the provider; absent: the probe's choice
//! windows = "plasma"      # plasma | fdo | none (none: no provider, its tools are not listed)
//! session = "fdo"         # keys: windows, capture, accessibility, launch, search, clipboard,
//!                         # media, audio, session, notify, input
//! ```
//!
//! A provider named here is used only if its probe passes; otherwise the capability has none, and
//! the capability table (the log, the tray) says why.

use kai_desktop::caps::Capability;
use serde::{Deserialize, Serialize};

/// A provider's name in the configuration and the capability table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Plasma,
    Fdo,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DesktopConfig {
    pub windows: Option<Choice>,
    pub capture: Option<Choice>,
    pub accessibility: Option<Choice>,
    pub launch: Option<Choice>,
    pub search: Option<Choice>,
    pub clipboard: Option<Choice>,
    pub media: Option<Choice>,
    pub audio: Option<Choice>,
    pub session: Option<Choice>,
    pub notify: Option<Choice>,
    pub input: Option<Choice>,
}

impl DesktopConfig {
    /// The owner's choice for `cap`, if any.
    pub fn choice(&self, cap: Capability) -> Option<Choice> {
        match cap {
            Capability::Windows => self.windows,
            Capability::Capture => self.capture,
            Capability::Accessibility => self.accessibility,
            Capability::Launch => self.launch,
            Capability::Search => self.search,
            Capability::Clipboard => self.clipboard,
            Capability::Media => self.media,
            Capability::Audio => self.audio,
            Capability::Session => self.session,
            Capability::Notify => self.notify,
            Capability::Input => self.input,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_reads_from_the_configuration() {
        let parse = |t: &str| crate::config::Config::parse(t, std::path::Path::new("kai.toml"));
        let c = parse("[desktop]\nwindows = \"fdo\"\nnotify = \"none\"\n").unwrap();
        assert_eq!(c.desktop.choice(Capability::Windows), Some(Choice::Fdo));
        assert_eq!(c.desktop.choice(Capability::Notify), Some(Choice::None));
        assert_eq!(c.desktop.choice(Capability::Media), None);
        assert_eq!(parse("").unwrap().desktop, DesktopConfig::default());
        for (text, says) in [
            ("[desktop]\nwindows = \"gnome\"\n", "gnome"),
            ("[desktop]\nwindow = \"plasma\"\n", "window"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
