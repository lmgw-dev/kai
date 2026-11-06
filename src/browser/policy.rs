//! The browser's part of K15's policy (kai-firefox design §6.4), read from `[browser]` with the
//! rest of the host's policy (`crate::host::policy::Policy::of`), so the providers and the host
//! read one live value and a change of either lists the tools again:
//! - **the hands-and-eyes switch**: K15's default of the tools of `contract::browser::BY_SWITCH`
//!   becomes `allow_trusted` where `policy.tools` sets no state for them, and the tools that exist
//!   only for hands and eyes are listed (`super::provider`);
//! - **the focus rule's exception** (`act_in_background`), which `super::tools` reads;
//! - **the site rules** ([`SiteRules`]), checked before every call;
//! - the reading tools' settings, read at each call: `element_point`'s ring (`point_ms`) and
//!   `pdftotext`'s time, text and memory on a PDF (`pdf_timeout_s`, `pdf_text_max_mb`,
//!   `pdf_memory_max_mb`);
//! - the acting tools' settings, read at each call: the commit guard (`commit_guard`,
//!   `guard_words`, `guard_window_ms`), when a press takes the trusted path (`trusted_press`) and
//!   how long Firefox still counts input as Kai's after Kai's own keys (`input_settle_ms`);
//! - the desktop's captures under the site rules: how long a tab brought to the front counts as
//!   not settled (`see_settle_ms`, `super::shown`);
//! - the announcements (`announce`, and their quiet period, how long
//!   `browser_context` keeps them and how much page text one speaks), which `super::announce`
//!   follows, and reading aloud's clean-up rules and voices (`read`), read at each paragraph.

use kai_desktop::contract::browser::by_switch;

use super::settings::{BrowserConfig, LangVoice, ReadAloud, TrustedPress, lang_voices};
use super::sites::SiteRules;
use crate::host::settings::ToolState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPolicy {
    pub hands_and_eyes: bool,
    pub act_in_background: bool,
    pub sites: SiteRules,
    pub point_ms: u64,
    pub pdf_timeout_s: u64,
    pub pdf_text_max_mb: u64,
    pub pdf_memory_max_mb: u64,
    pub commit_guard: bool,
    pub guard_words: Vec<String>,
    pub guard_window_ms: u64,
    pub input_settle_ms: u64,
    pub see_settle_ms: u64,
    pub trusted_press: TrustedPress,
    /// `browser.announce` as set; [`BrowserPolicy::announcing`] is this alone.
    pub announce: bool,
    pub announce_quiet_ms: u64,
    pub announce_keep_s: u64,
    pub announce_max_chars: u64,
    pub announce_max_age_s: u64,
    pub read: ReadAloud,
    /// `browser.read.voices`, read (an entry that does not read was refused with the
    /// configuration).
    pub voices: Vec<LangVoice>,
}

/// `[browser]`'s defaults: the switch and the exception off, no site rules.
impl Default for BrowserPolicy {
    fn default() -> Self {
        Self::new(&BrowserConfig::default())
    }
}

impl BrowserPolicy {
    pub fn new(c: &BrowserConfig) -> Self {
        Self {
            hands_and_eyes: c.hands_and_eyes,
            act_in_background: c.act_in_background,
            sites: SiteRules::of(&c.sites),
            point_ms: c.point_ms,
            pdf_timeout_s: c.pdf_timeout_s,
            pdf_text_max_mb: c.pdf_text_max_mb,
            pdf_memory_max_mb: c.pdf_memory_max_mb,
            commit_guard: c.commit_guard,
            guard_words: c.guard_words.clone(),
            guard_window_ms: c.guard_window_ms,
            input_settle_ms: c.input_settle_ms,
            see_settle_ms: c.see_settle_ms,
            trusted_press: c.trusted_press,
            announce: c.announce,
            announce_quiet_ms: c.announce_quiet_ms,
            announce_keep_s: c.announce_keep_s,
            announce_max_chars: c.announce_max_chars,
            announce_max_age_s: c.announce_max_age_s,
            read: c.read.clone(),
            voices: lang_voices(&c.read.voices).unwrap_or_default(),
        }
    }

    /// Announcements are on: `browser.announce` alone (design §7.2). The hands-and-eyes switch
    /// does not turn them on; the settings window writes `browser.announce` itself when that
    /// switch is turned on there.
    pub fn announcing(&self) -> bool {
        self.announce
    }

    /// The state the switch gives `tool` where `policy.tools` gives none.
    pub fn default_state(&self, tool: &str) -> Option<ToolState> {
        (self.hands_and_eyes && by_switch(tool)).then_some(ToolState::AllowTrusted)
    }
}

#[cfg(test)]
mod tests {
    use kai_desktop::contract::find;

    use super::*;
    use crate::host::policy::{Caller, Decision, Policy};
    use crate::host::settings::{Named, PolicyConfig};

    fn decide(p: &Policy, tool: &str, caller: &Caller) -> Decision {
        p.decide(find(tool).unwrap().1, caller, None)
    }

    /// §6.4: the switch makes the acting tools of tabs, hands and reading aloud allow_trusted, but
    /// not attaching a file, Firefox's features, styles or scripts.
    #[test]
    fn the_switch_lets_trusted_callers_act_except_with_files_features_and_scripts() {
        let off = Policy::new(&PolicyConfig::default());
        let on = Policy::new(&PolicyConfig::default()).with_browser(&BrowserConfig {
            hands_and_eyes: true,
            ..Default::default()
        });
        let key = Caller::Named(Named {
            kind: "key".into(),
            name: "laptop".into(),
        });
        for tool in [
            "tab_close",
            "element_press",
            "page_key",
            "read_skip",
            "hints_show",
        ] {
            assert!(
                matches!(decide(&off, tool, &Caller::Owner), Decision::Confirm { .. }),
                "{tool}"
            );
            assert_eq!(decide(&on, tool, &Caller::Owner), Decision::Allow, "{tool}");
            assert_eq!(decide(&on, tool, &Caller::Own), Decision::Allow, "{tool}");
            assert_eq!(
                decide(&on, tool, &key),
                Decision::Confirm {
                    why: "browser.hands_and_eyes lets trusted callers only act unasked"
                },
                "{tool}"
            );
        }
        for tool in [
            "element_file",
            "page_zoom",
            "reader_view",
            "page_css",
            "page_script",
            "page_script_main",
        ] {
            assert!(
                matches!(decide(&on, tool, &Caller::Owner), Decision::Confirm { .. }),
                "{tool} stays asked"
            );
        }
        assert_eq!(decide(&on, "tab_list", &Caller::Owner), Decision::Allow);
        let defaults = BrowserPolicy::default();
        assert_eq!(
            (
                defaults.point_ms,
                defaults.pdf_timeout_s,
                defaults.pdf_text_max_mb,
                defaults.pdf_memory_max_mb
            ),
            (3000, 60, 1024, 4096),
            "§5.4's 3000 ms"
        );
        assert!(!defaults.hands_and_eyes && !defaults.act_in_background);
        assert!(!defaults.announcing());
        let switch_only = BrowserPolicy::new(&BrowserConfig {
            hands_and_eyes: true,
            ..Default::default()
        });
        assert!(
            !switch_only.announcing(),
            "the hands-and-eyes switch does not turn announcements on"
        );
        let announce_only = BrowserPolicy::new(&BrowserConfig {
            announce: true,
            ..Default::default()
        });
        assert!(announce_only.announcing());
        assert!(defaults.commit_guard, "§6.4: on by default");
        assert_eq!(defaults.guard_window_ms, 2000);
        assert_eq!(defaults.input_settle_ms, 1000);
        assert_eq!(defaults.see_settle_ms, 500);
        assert_eq!(
            defaults.guard_words,
            crate::browser::settings::GUARD_WORDS,
            "the list the contract ships"
        );
        assert_eq!(defaults.trusted_press, TrustedPress::WhenNeeded);
        assert_eq!(defaults.sites, SiteRules::default());
    }
}
