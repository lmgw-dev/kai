//! `[routines]` (K28; `crate::config` holds the field): which routines are on, the alias
//! `routine_create` writes with, and how long a run from the tray or a key may take.
//!
//! ```toml
//! [routines]              # K28: the routines in $XDG_DATA_HOME/kai/routines/<name>/
//! create_alias = "qwen3-coder"  # routine_create's model (a capable coding model); absent: the
//!                         # Kai conversation's alias (its folder's model), asked from lmgw at each
//!                         # call
//! run_timeout_s = 0       # a run started from the tray's Routines menu or a routine's key is
//!                         # stopped after this many seconds (its scope killed, the notification
//!                         # says so); 0: no deadline, it runs until it ends or the tray's Stop
//! [routines.enabled]      # a routine is offered only while it is on here: routine_enable and the
//! backup = true           # tray show it in full first (routine.toml and every other file at the
//!                         # top of its folder), then write `true`. The latest version of the folder
//!                         # always runs: no pinning (owner's decision, 2026-10-09). A new one (a
//!                         # draft of routine_create, one written by hand or by any agent) is off.
//!                         # A true whose folder is gone becomes false (a new folder of that name
//!                         # is off until shown). An older `"sha256:…"` value (WP20's pins) reads
//!                         # as on; config_set refuses these keys, except false
//! ```
//!
//! The switches live in Kai's configuration, not in the routine's directory: the directory is meant
//! to be written by agents and other tools (K28), so nothing in it can switch itself on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RoutinesConfig {
    pub create_alias: Option<String>,
    /// The deadline of a run from the tray or a key, in seconds; 0: none.
    pub run_timeout_s: u64,
    /// A routine's switch by its name (module doc).
    pub enabled: BTreeMap<String, Switch>,
}

/// One routine's switch as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Switch {
    /// `false`: off. `true`: on.
    Flag(bool),
    /// `"sha256:<hex>"`: WP20's pin, read as on (the digest is no longer checked). Never written.
    Pin(String),
}

/// The table of the switches (`routines.enabled.<name>`), which only the routine operation writes.
pub const ENABLED_TABLE: &str = "routines.enabled";

/// The tray's and keys' deadline, read live like the switches.
pub const RUN_TIMEOUT_KEY: &str = "routines.run_timeout_s";

/// The old pin's word before its digest.
const PIN_PREFIX: &str = "sha256:";

/// A pin as WP20 wrote it: `sha256:` and 64 lowercase hex digits.
fn is_pin(text: &str) -> bool {
    text.strip_prefix(PIN_PREFIX).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl RoutinesConfig {
    /// The file switched `name` on (`true`, or an old pin; a `false` is no switch on).
    pub fn switched_on(&self, name: &str) -> bool {
        matches!(
            self.enabled.get(name),
            Some(Switch::Pin(_) | Switch::Flag(true))
        )
    }

    /// The file has a key for `name`, whatever its value (`routine_create` leaves such a name
    /// alone; the tray lists one without a folder to forget it).
    pub fn has_switch(&self, name: &str) -> bool {
        self.enabled.contains_key(name)
    }

    /// `crate::config`'s check: a key, its value and the rule it breaks.
    pub fn validate(&self) -> Result<(), (&'static str, String, String)> {
        if self
            .create_alias
            .as_deref()
            .is_some_and(|v| v.trim().is_empty())
        {
            return Err((
                "routines.create_alias",
                "\"\"".into(),
                "an lmgw alias, or leave the key out for the Kai conversation's".into(),
            ));
        }
        if let Some(bad) = self.enabled.keys().find(|n| !super::def::plain_name(n)) {
            return Err((
                "routines.enabled",
                format!("{bad:?}"),
                "each key is a routine's name: lowercase letters, digits and single underscores"
                    .into(),
            ));
        }
        if let Some(Switch::Pin(bad)) = self
            .enabled
            .values()
            .find(|s| matches!(s, Switch::Pin(p) if !is_pin(p)))
        {
            return Err((
                "routines.enabled",
                format!("{bad:?}"),
                "a switch is true or false (an old pin is sha256: and 64 hex digits)".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn the_section_reads_from_the_configuration() {
        let parse = |t: &str| crate::config::Config::parse(t, std::path::Path::new("kai.toml"));
        let c = parse(&format!(
            "[routines]\ncreate_alias = \"coder\"\nrun_timeout_s = 30\n[routines.enabled]\n\
             backup = \"{PIN}\"\nold = true\nx = false\n"
        ))
        .unwrap();
        assert_eq!(c.routines.create_alias.as_deref(), Some("coder"));
        assert_eq!(c.routines.run_timeout_s, 30);
        assert!(c.routines.switched_on("backup"), "an old pin reads as on");
        assert!(c.routines.switched_on("old"));
        assert!(!c.routines.switched_on("x") && c.routines.has_switch("x"));
        assert!(!c.routines.switched_on("other"), "off unless switched on");
        assert!(!c.routines.has_switch("other"));
        assert_eq!(parse("").unwrap().routines, RoutinesConfig::default());
        assert_eq!(parse("").unwrap().routines.run_timeout_s, 0, "no deadline");
        for (text, says) in [
            (
                "[routines]\ncreate_alias = \" \"\n".to_owned(),
                "routines.create_alias",
            ),
            (
                "[routines.enabled]\n\"Bad-Name\" = true\n".to_owned(),
                "routines.enabled",
            ),
            ("[routines]\nenable = []\n".to_owned(), "enable"),
            (
                "[routines.enabled]\nx = \"sha256:abc\"\n".to_owned(),
                "64 hex digits",
            ),
            (
                format!("[routines.enabled]\nx = \"{}\"\n", PIN.to_uppercase()),
                "64 hex digits",
            ),
        ] {
            let e = parse(&text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
