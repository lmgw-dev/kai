//! `[delegate]` (K25, §16 Q13; `crate::config` holds the field): the alias each delegate's model
//! calls go to, and the loop's one bound.
//!
//! ```toml
//! [delegate]              # K25: the delegates' models (§16 Q13)
//! look = "qwen3-vl"       # look's alias (it reads screenshots: a vision model); absent: the
//!                         # Kai conversation's alias (its folder's model), asked from lmgw at
//!                         # each call
//! find = "gemma4-12b"     # find's alias; absent: the same
//! max_turns = 20          # the most model turns one delegate task takes; a task that reaches it
//!                         # ends "partial" and its answer names this key; 0: no bound (lmgw's
//!                         # timeout for the call and the model's context still end it, each said)
//! screenshots = "main"    # who looks at screenshots: "main", Kai's callers (see_window and
//!                         # see_screen are listed to them); "delegate", only the look delegate
//!                         # (they are not listed to callers, look answers in text: the caller's
//!                         # model needs no vision and little context; look's alias needs vision)
//! ```
//!
//! `screenshots` was decided by the owner 2026-10-09; it takes effect at once (the host lists the
//! tools again, `list_changed`), see `super::screenshots`.
//!
//! An alias is any of lmgw's (`GET /v1/models`): the conversation's row with a second slot (no
//! extra VRAM; without kv-unified llama-server divides the context across the slots, with it a
//! large delegate context competes with the conversation), or a smaller model. Which is better is
//! the owner's open question (§16 Q13).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DelegateConfig {
    pub look: Option<String>,
    pub find: Option<String>,
    pub max_turns: u32,
    pub screenshots: Screenshots,
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self {
            look: None,
            find: None,
            max_turns: 20,
            screenshots: Screenshots::Main,
        }
    }
}

/// `delegate.screenshots`: who looks at screenshots (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Screenshots {
    /// Kai's callers see them: `see_window` and `see_screen` are listed to them (and `look` too).
    #[default]
    Main,
    /// Only the `look` delegate sees them; its answer is text.
    Delegate,
}

/// The key, for the configuration operations' effect (it applies at once).
pub const SCREENSHOTS_KEY: &str = "delegate.screenshots";

impl DelegateConfig {
    /// The alias set for the delegate `name`, if any.
    pub fn alias(&self, name: &str) -> Option<&str> {
        match name {
            "look" => self.look.as_deref(),
            "find" => self.find.as_deref(),
            _ => None,
        }
    }

    /// `crate::config`'s check: a key, its value and the rule it breaks.
    pub fn validate(&self) -> Result<(), (&'static str, String, String)> {
        for (key, value) in [("delegate.look", &self.look), ("delegate.find", &self.find)] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err((
                    key,
                    "\"\"".into(),
                    "an lmgw alias, or leave the key out for the Kai conversation's".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_reads_from_the_configuration() {
        let parse = |t: &str| crate::config::Config::parse(t, std::path::Path::new("kai.toml"));
        let c = parse("[delegate]\nlook = \"vl\"\nmax_turns = 0\n").unwrap();
        assert_eq!(c.delegate.alias("look"), Some("vl"));
        assert_eq!(c.delegate.alias("find"), None);
        assert_eq!(c.delegate.max_turns, 0);
        assert_eq!(c.delegate.screenshots, Screenshots::Main, "the default");
        assert_eq!(parse("").unwrap().delegate, DelegateConfig::default());
        assert_eq!(DelegateConfig::default().max_turns, 20);
        let c = parse("[delegate]\nscreenshots = \"delegate\"\n").unwrap();
        assert_eq!(c.delegate.screenshots, Screenshots::Delegate);
        for (text, says) in [
            ("[delegate]\nlook = \" \"\n", "delegate.look"),
            ("[delegate]\nloook = \"x\"\n", "loook"),
            ("[delegate]\nmax_turns = -1\n", "max_turns"),
            ("[delegate]\nscreenshots = \"look\"\n", "screenshots"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
