//! `[launch]` (WP16a; `crate::config` holds the field): the owner's own bounds on `run_command`
//! (none by default: the call's deadline and the link's message limit bound it) and how an
//! application that asks for a terminal is started.
//!
//! ```toml
//! [launch]                # open, app_launch, run_command (K14, K24)
//! command_timeout_s = 0   # an optional time limit for run_command below the
//!                         # call's: a command always stops at its call's deadline (lmgw's
//!                         # lmgw/timeout_ms, the confirmation included) less the time its answer
//!                         # needs, and the result names that; this stops it sooner, its scope
//!                         # killed and its output so far returned, naming this setting. 0: none
//! output_max_mb = 0       # an optional bound on what of a command's output its result
//!                         # carries, below the link's: the result always fits
//!                         # lmgw's message limit (the start and the end of what does not fit,
//!                         # measured as the JSON the link carries, the cut said and the limit
//!                         # named); this bounds it sooner, naming this setting. 0: none
//! # terminal = ["konsole", "-e"]  # runs an application whose desktop entry asks for a terminal
//!                         # (Terminal=true): its command line follows. Unset (the default):
//!                         # Plasma's configured terminal (kdeglobals' [General]
//!                         # TerminalApplication, then -e), else xdg-terminal-exec, else Konsole on
//!                         # Plasma; an error naming this key only when none is installed
//!                         # (`super::terminal`). Set, it always wins. A command Kai runs: the
//!                         # config tools change it only with policy.widen_by_tools.run_command (K30)
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Every default is "none of the owner's own": no time limit, no output bound, the automatic
/// terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LaunchConfig {
    pub command_timeout_s: u64,
    pub output_max_mb: u64,
    /// `None`: the automatic order of `super::terminal`.
    pub terminal: Option<Vec<String>>,
}

impl LaunchConfig {
    /// `None`: no timeout of Kai's own.
    pub fn command_timeout(&self) -> Option<Duration> {
        (self.command_timeout_s > 0).then(|| Duration::from_secs(self.command_timeout_s))
    }

    /// The setting with its value, as a result names it.
    pub fn timeout_setting(&self) -> String {
        format!("launch.command_timeout_s = {}", self.command_timeout_s)
    }

    /// `None`: no bound.
    pub fn output_max(&self) -> Option<u64> {
        (self.output_max_mb > 0).then(|| self.output_max_mb.saturating_mul(1024 * 1024))
    }

    pub fn output_setting(&self) -> String {
        format!("launch.output_max_mb = {}", self.output_max_mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_reads_from_the_configuration() {
        let parse = |t: &str| crate::config::Config::parse(t, std::path::Path::new("kai.toml"));
        let c = parse(
            "[launch]\ncommand_timeout_s = 0\noutput_max_mb = 1\nterminal = [\"konsole\", \"-e\"]\n",
        )
        .unwrap();
        assert_eq!(c.launch.command_timeout(), None);
        assert_eq!(c.launch.output_max(), Some(1024 * 1024));
        assert_eq!(
            c.launch.terminal.as_deref(),
            Some(&["konsole".to_owned(), "-e".to_owned()][..])
        );
        let d = parse("").unwrap().launch;
        assert_eq!(d, LaunchConfig::default());
        assert_eq!(d.terminal, None, "unset: the automatic order");
        assert_eq!(
            (d.command_timeout(), d.output_max()),
            (None, None),
            "none of the owner's own"
        );
        let c = parse("[launch]\ncommand_timeout_s = 50\n").unwrap().launch;
        assert_eq!(c.command_timeout(), Some(Duration::from_secs(50)));
        assert_eq!(c.timeout_setting(), "launch.command_timeout_s = 50");
        let e = parse("[launch]\ntimeout = 3\n").unwrap_err().to_string();
        assert!(e.contains("timeout"), "{e}");
    }
}
