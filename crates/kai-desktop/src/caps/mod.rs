//! The capability traits (K23): what a desktop provider offers, one trait per capability. Kai's
//! `src/desktop/` probes which provider serves each capability on this desktop (`fdo`, the
//! freedesktop base, and `plasma`; `gnome` and `wlroots` later) and the tools call the chosen
//! provider through these traits only, so a tool never knows which desktop it runs on.
//!
//! The traits are plain Rust with no runtime: a method returns a boxed `Send` future ([`Fut`]), so
//! a provider can be held as `Arc<dyn Windows>`. Each trait carries the methods of the tools built
//! so far; a later work package adds the methods its tools need to its capability's trait (the
//! trait's doc names them).

pub mod accessibility;
pub mod audio;
pub mod capture;
pub mod clipboard;
pub mod input;
pub mod launch;
pub mod media;
pub mod notify;
pub mod search;
pub mod session;
pub mod windows;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// A provider's answer: a boxed future, so the traits stay object-safe.
pub type Fut<'a, T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;

/// The eleven capabilities of K23, in the order the capability table shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Windows,
    Capture,
    Accessibility,
    Launch,
    Search,
    Clipboard,
    Media,
    Audio,
    Session,
    Notify,
    Input,
}

impl Capability {
    pub const ALL: [Capability; 11] = [
        Capability::Windows,
        Capability::Capture,
        Capability::Accessibility,
        Capability::Launch,
        Capability::Search,
        Capability::Clipboard,
        Capability::Media,
        Capability::Audio,
        Capability::Session,
        Capability::Notify,
        Capability::Input,
    ];

    /// Its word in the configuration (`[desktop]`), the log and the tray.
    pub fn key(self) -> &'static str {
        match self {
            Capability::Windows => "windows",
            Capability::Capture => "capture",
            Capability::Accessibility => "accessibility",
            Capability::Launch => "launch",
            Capability::Search => "search",
            Capability::Clipboard => "clipboard",
            Capability::Media => "media",
            Capability::Audio => "audio",
            Capability::Session => "session",
            Capability::Notify => "notify",
            Capability::Input => "input",
        }
    }
}

/// Why a provider could not do what a tool asked. Its text is what the model reads (K22: a short
/// sentence, never an argument echoed back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The desktop has the feature, but this build of Kai holds no grant for it (ruling 16,
    /// §11.1): `what` names the grant and who gives it ("KWin grants <interface> per .desktop
    /// file"). The tool stays listed (K23).
    NotGranted { what: String },
    /// The request cannot be served as asked (no such window, no such desktop): said to the model,
    /// which can ask again differently.
    Refused(String),
    /// The desktop did not answer, or answered with an error.
    Failed(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NotGranted { what } => write!(f, "needs the installed Kai: {what}"),
            Failure::Refused(m) | Failure::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_its_own_key() {
        let keys: std::collections::HashSet<&str> =
            Capability::ALL.iter().map(|c| c.key()).collect();
        assert_eq!(keys.len(), 11);
    }

    #[test]
    fn a_missing_grant_reads_as_section_11_1_says() {
        let f = Failure::NotGranted {
            what: "KWin grants org_kde_plasma_window_management per .desktop file".into(),
        };
        assert_eq!(
            f.to_string(),
            "needs the installed Kai: KWin grants org_kde_plasma_window_management per .desktop \
             file"
        );
    }
}
