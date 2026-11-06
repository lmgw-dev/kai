//! The warning of kai-firefox design §8.3 (the owner's decision 9: warn, prevent nothing): page text
//! reaches the model through the browser tools (and the sidebar's attachments), and a page can
//! carry instructions. While Kai's lmgw admin level is above `off` and a Firefox is linked, Kai
//! says so in a notification once, when Firefox first links, and again whenever the level is
//! raised; the settings window shows the line while it holds. Nothing is refused or hidden.
//!
//! Kai learns its level from its feed: `hello.self_admin` and `state.self_admin`, the level the
//! owner set on lmgw's Devices card capped by lmgw's own self-admin level (`crate::lmgw_link`'s
//! `AdminLevel`). Until the feed said it, the level counts as `off`; while the feed is down the
//! last level said holds.

use crate::lmgw_link::AdminLevel;

/// The level's reach, for "raised": `off` 0, `read_only` 1, `full` 2; a level a newer lmgw adds
/// counts as 1 (it is more than `off`, its reach unknown).
fn rank(level: &AdminLevel) -> u8 {
    match level {
        AdminLevel::Off => 0,
        AdminLevel::ReadOnly => 1,
        AdminLevel::Full => 2,
        other if other.as_str().trim().is_empty() => 0,
        _ => 1,
    }
}

/// The warning's text for `level` (design §8.3's words).
pub fn text(level: &AdminLevel) -> String {
    let (can, reach) = match level {
        AdminLevel::Full => (
            "can change lmgw (admin level: full)".to_owned(),
            "lmgw's configuration and the tools that run programs",
        ),
        AdminLevel::ReadOnly => (
            "can read lmgw's configuration (admin level: read only)".to_owned(),
            "lmgw's configuration",
        ),
        other => (
            format!(
                "has lmgw's admin tools (admin level: {})",
                other.as_str().replace('_', " ")
            ),
            "lmgw's admin tools",
        ),
    };
    format!(
        "Kai reads web pages in Firefox and {can}. A page's text can steer the model, and at this \
         level it reaches {reach}. This may be unwise."
    )
}

/// The notification's summary.
pub const SUMMARY: &str = "Kai: web pages and lmgw's admin tools";

/// Tells the core how many Firefox extensions are linked, now and at every change, until the
/// core or the links are gone.
pub async fn forward(
    mut linked: tokio::sync::watch::Receiver<usize>,
    cmds: tokio::sync::mpsc::UnboundedSender<crate::core::Command>,
) {
    loop {
        let n = *linked.borrow_and_update();
        if cmds.send(crate::core::Command::BrowserLinked(n)).is_err()
            || linked.changed().await.is_err()
        {
            return;
        }
    }
}

/// The warning's state (module doc).
#[derive(Debug, Default)]
pub struct AdminWarning {
    level: AdminLevel,
    linked: bool,
    /// The rank last warned at; lowered with the level, so a raise warns again.
    warned: u8,
}

impl AdminWarning {
    /// The level as the feed says it now; the notification's text when one is due.
    pub fn level(&mut self, level: AdminLevel) -> Option<String> {
        self.level = level;
        self.due()
    }

    /// Whether a Firefox is linked now; the notification's text when one is due.
    pub fn linked(&mut self, linked: bool) -> Option<String> {
        self.linked = linked;
        self.due()
    }

    fn due(&mut self) -> Option<String> {
        let rank = rank(&self.level);
        self.warned = self.warned.min(rank);
        (self.linked && rank > self.warned).then(|| {
            self.warned = rank;
            text(&self.level)
        })
    }

    /// The settings window's line, while the warning holds.
    pub fn line(&self) -> Option<String> {
        (self.linked && rank(&self.level) > 0).then(|| text(&self.level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_warns_once_per_link_and_again_at_each_raise() {
        let mut w = AdminWarning::default();
        assert_eq!(w.linked(true), None, "at off nothing is said");
        assert_eq!(w.line(), None);
        let full = w.level(AdminLevel::Full).unwrap();
        assert!(
            full.contains("can change lmgw (admin level: full)"),
            "{full}"
        );
        assert!(full.ends_with("This may be unwise."));
        assert_eq!(w.line().as_deref(), Some(full.as_str()));
        // Firefox quits and starts again: said once.
        assert_eq!(w.linked(false), None);
        assert_eq!(w.line(), None);
        assert_eq!(w.linked(true), None);
        assert_eq!(w.level(AdminLevel::Full), None, "the same level again");
        // Lowered, then raised: said again.
        assert_eq!(w.level(AdminLevel::ReadOnly), None);
        assert!(w.line().unwrap().contains("read only"));
        assert!(w.level(AdminLevel::Full).is_some());
        // Raised while no Firefox is linked: said when one links.
        assert_eq!(w.level(AdminLevel::Off), None);
        assert_eq!(w.linked(false), None);
        assert_eq!(w.level(AdminLevel::ReadOnly), None);
        let ro = w.linked(true).unwrap();
        assert!(ro.contains("can read lmgw's configuration"), "{ro}");
        assert!(w.level(AdminLevel::Full).is_some());
        // A level a newer lmgw adds counts as above off.
        let mut w = AdminWarning::default();
        w.linked(true);
        let newer = w.level(AdminLevel::Unknown("ask_first".into())).unwrap();
        assert!(newer.contains("admin level: ask first"), "{newer}");
    }
}
