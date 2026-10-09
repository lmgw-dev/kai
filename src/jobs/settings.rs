//! `[jobs]` (WP18, ruling 22, K26; `crate::config` holds the field): when Kai speaks a job's result,
//! and how long it keeps an ended job whose delivery lmgw never confirmed.
//!
//! ```toml
//! [jobs]                  # Kai's asynchronous jobs (K26) and their results (ruling 22)
//! quiet_ms = 1500         # a job's result is spoken only after this long with nothing going on:
//!                         # no talk key held, no speech heard, no reply open or playing, no call
//!                         # waiting for an approval, no turn of another client running in the
//!                         # thread, the screen unlocked, not muted; anything of it starts the wait
//!                         # again. 0: at the first such moment
//! keep_ended_s = 86400    # an ended job whose result lmgw has not confirmed (fetched, and said to
//!                         # have entered its thread) is dropped this long after its end or its
//!                         # latest fetch; the tray and the jobs tool say until when. 0: kept until
//!                         # Kai quits
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct JobsConfig {
    pub quiet_ms: u64,
    pub keep_ended_s: u64,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            quiet_ms: 1500,
            keep_ended_s: super::KEEP_ENDED_DEFAULT.as_secs(),
        }
    }
}

impl JobsConfig {
    /// `jobs.quiet_ms`.
    pub fn quiet(&self) -> Duration {
        Duration::from_millis(self.quiet_ms)
    }

    /// `jobs.keep_ended_s` (zero: until Kai quits).
    pub fn keep_ended(&self) -> Duration {
        Duration::from_secs(self.keep_ended_s)
    }
}
