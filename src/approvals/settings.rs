//! `[approvals]` (WP19, §11.1, K17; `crate::config` holds the field): the approval card's place, its
//! arming delay, and the phrases a spoken answer is matched against.
//!
//! ```toml
//! [approvals]             # a call that waits for an approval (api §6): the card, and the spoken answer
//! arm_ms = 600            # the card answers no key, no click and no spoken answer until this long
//!                         # has passed with no key and no click reaching it, counted again when
//!                         # the keyboard comes to it (focus-in): the keyboard moves to the card
//!                         # the moment it shows (layer-shell OnDemand), so a key meant for another
//!                         # window, or a burst of typing, must not answer it; every key or click
//!                         # before then starts the wait again, and the card says it is not armed
//!                         # yet. Once a key reached the card before it armed, Enter alone does not
//!                         # approve that call (Ctrl+Enter, a click or the voice does). 0: armed at
//!                         # once
//! anchor = "top"          # where the card sits on the overlays' output (overlay.output, else
//!                         # KWin's active one): top-left | top | top-right | left | center | right
//!                         # | bottom-left | bottom | bottom-right
//! margin = 48             # px from the anchored edges
//! width = 560             # px; the height is the card's content, at most the output's height less
//!                         # the margins (the arguments then scroll, never shortened)
//! speech_dbfs = -40       # the spoken answer must hold speech: a 20 ms frame counts at this RMS
//! speech_ms = 150         # level (dBFS) or above, and the recording needs this many ms of it, or
//!                         # it is not sent and the card says "Kai heard no speech" (ASR makes
//!                         # words of near silence: "Okay.", "Ja."). 0: no check. Not yet tuned
//!                         # against real microphones
//! yes = ["yes", "ja", …]  # a spoken answer approves when the whole of it, lower-cased and with
//! no = ["no", "nein", …]  # its punctuation gone, is one of these phrases (`super::words`); one in
//!                         # neither list keeps the card and says so. A phrase in both lists is an
//!                         # error. The defaults leave out the filler words a recogniser makes of
//!                         # a cough or a sigh ("ok", "okay", "sure", "yeah", "genau", "mach")
//! ```

use serde::{Deserialize, Serialize};

use crate::overlay::placement::Anchor;

/// The default phrases that approve, German and English (§11.1): only ones that say yes on
/// their own, never a filler word a recogniser makes of near silence or a cough.
pub const YES: &[&str] = &[
    "yes",
    "yes please",
    "approve",
    "approved",
    "go ahead",
    "do it",
    "ja",
    "ja bitte",
    "jawohl",
    "genehmigen",
    "genehmigt",
    "freigeben",
    "mach das",
    "mach es",
];

/// The default phrases that decline, German and English (§11.1).
pub const NO: &[&str] = &[
    "no",
    "nope",
    "no thanks",
    "decline",
    "deny",
    "cancel",
    "stop",
    "don't",
    "do not",
    "nein",
    "nee",
    "nein danke",
    "abbrechen",
    "ablehnen",
    "lass es",
    "lass das",
    "stopp",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ApprovalsConfig {
    pub arm_ms: u64,
    pub anchor: Anchor,
    pub margin: u32,
    pub width: u32,
    pub speech_dbfs: i32,
    pub speech_ms: u64,
    pub yes: Vec<String>,
    pub no: Vec<String>,
}

impl Default for ApprovalsConfig {
    fn default() -> Self {
        Self {
            arm_ms: 600,
            anchor: Anchor::Top,
            margin: 48,
            width: 560,
            speech_dbfs: -40,
            speech_ms: 150,
            yes: YES.iter().map(|s| (*s).to_owned()).collect(),
            no: NO.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

impl ApprovalsConfig {
    /// A phrase that says nothing once normalised, or one in both lists, is an error naming it
    /// (`crate::config`'s shape: the key, the value, the rule).
    pub fn validate(&self) -> Result<(), (&'static str, String, String)> {
        if self.width == 0 {
            return Err((
                "approvals.width",
                "0".into(),
                "must be at least 1 (px)".into(),
            ));
        }
        if !(-120..=0).contains(&self.speech_dbfs) {
            return Err((
                "approvals.speech_dbfs",
                self.speech_dbfs.to_string(),
                "must be between -120 and 0 (dBFS)".into(),
            ));
        }
        for (key, list) in [("approvals.yes", &self.yes), ("approvals.no", &self.no)] {
            if let Some(p) = list.iter().find(|p| super::words::normalise(p).is_empty()) {
                return Err((
                    key,
                    format!("{p:?}"),
                    "each phrase needs a word (punctuation is ignored)".into(),
                ));
            }
        }
        let no: Vec<String> = self.no.iter().map(|p| super::words::normalise(p)).collect();
        if let Some(p) = self
            .yes
            .iter()
            .find(|p| no.contains(&super::words::normalise(p)))
        {
            return Err((
                "approvals.yes",
                format!("{p:?}"),
                "is in approvals.no too: a phrase in both lists would mean both".into(),
            ));
        }
        Ok(())
    }

    pub fn arm(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.arm_ms)
    }
}
