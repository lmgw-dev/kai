//! The four echo modes (§6.2) and what each means for the tray and the session.

use serde::{Deserialize, Serialize};

/// The owner's choice. Against the Chat's names (chat-voice §12.1): `device` = `device`, `app`
/// takes the place of `browser` (a canceller in the client), `headphones` = `not_needed`,
/// `half_duplex` = `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoMode {
    /// The input device cancels echo against the default output's monitor (K8). Kai must play on
    /// the default output.
    #[default]
    Device,
    /// Kai hosts PipeWire's echo-cancel module and routes the defaults through it (K20).
    App,
    /// Nothing to cancel.
    Headphones,
    /// The session ignores the microphone while Kai speaks; only the key interrupts.
    HalfDuplex,
}

impl EchoMode {
    pub const ALL: [EchoMode; 4] = [
        EchoMode::Device,
        EchoMode::App,
        EchoMode::Headphones,
        EchoMode::HalfDuplex,
    ];

    /// The config value.
    pub fn key(self) -> &'static str {
        match self {
            EchoMode::Device => "device",
            EchoMode::App => "app",
            EchoMode::Headphones => "headphones",
            EchoMode::HalfDuplex => "half_duplex",
        }
    }

    /// The tray's radio label (§7).
    pub fn label(self) -> &'static str {
        match self {
            EchoMode::Device => "Input device cancels echo",
            EchoMode::App => "Kai cancels echo",
            EchoMode::Headphones => "Headphones",
            EchoMode::HalfDuplex => "Half duplex",
        }
    }
}

/// Where app AEC stands (only meaningful while the mode is `app`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AecPhase {
    #[default]
    Off,
    /// The module is coming up.
    Starting,
    /// The defaults run through Kai's canceller.
    Active,
    /// It could not come up or went away; the reason names the setting where one applies.
    Failed(String),
}

/// What the echo mode amounts to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoStatus {
    pub chosen: EchoMode,
    /// What the session runs with: `app` falls back to `half_duplex` when it failed.
    pub effective: EchoMode,
    /// The tooltip line, if there is something to say (§7).
    pub note: Option<String>,
    /// `device` with an output that is not the default: the warning offers half duplex (§6.2).
    pub offer_half_duplex: bool,
}

impl EchoStatus {
    #[cfg_attr(not(test), allow(dead_code))] // read by the voice session (WP4)
    /// The session's `half_duplex` (WP4 sends it in `session.update`; `echo_tail_ms` comes from
    /// lmgw's settings).
    pub fn half_duplex(&self) -> bool {
        self.effective == EchoMode::HalfDuplex
    }
}

/// The status for a chosen mode, the AEC phase, Kai's output pick (node name, and the label the
/// menu shows for it) and the system's default output.
pub fn status(
    chosen: EchoMode,
    aec: &AecPhase,
    output_pick: Option<(&str, &str)>,
    default_output: Option<&str>,
) -> EchoStatus {
    let mut s = EchoStatus {
        chosen,
        effective: chosen,
        note: None,
        offer_half_duplex: false,
    };
    match chosen {
        EchoMode::Device => {
            if let Some((pick, label)) = output_pick
                && Some(pick) != default_output
            {
                s.note = Some(format!(
                    "echo: Kai plays on '{label}', not the default output, so the input device \
                     cannot cancel Kai's voice; half duplex avoids hearing itself"
                ));
                s.offer_half_duplex = true;
            }
        }
        EchoMode::App => match aec {
            AecPhase::Off | AecPhase::Starting => {
                s.note = Some("echo: Kai's echo cancellation is starting".into());
            }
            AecPhase::Active => {
                s.note = Some(
                    "echo: Kai cancels echo; the default devices run through it until Kai quits"
                        .into(),
                );
            }
            AecPhase::Failed(reason) => {
                s.effective = EchoMode::HalfDuplex;
                s.note = Some(format!(
                    "echo: Kai's echo cancellation failed: {reason}; half duplex instead"
                ));
            }
        },
        EchoMode::Headphones | EchoMode::HalfDuplex => {}
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_mode_on_the_default_output_says_nothing() {
        let s = status(EchoMode::Device, &AecPhase::Off, None, Some("spk"));
        assert_eq!(
            (s.effective, s.note, s.offer_half_duplex),
            (EchoMode::Device, None, false)
        );
        let s = status(
            EchoMode::Device,
            &AecPhase::Off,
            Some(("spk", "Speakers")),
            Some("spk"),
        );
        assert_eq!(s.note, None);
        assert!(!s.half_duplex());
    }

    #[test]
    fn device_mode_on_another_output_warns_and_offers_half_duplex() {
        let s = status(
            EchoMode::Device,
            &AecPhase::Off,
            Some(("hdmi", "HDMI out")),
            Some("spk"),
        );
        assert_eq!(s.effective, EchoMode::Device, "a warning, not a switch");
        assert!(s.offer_half_duplex);
        assert!(
            s.note
                .unwrap()
                .contains("'HDMI out', not the default output")
        );
        // Unknown default (no session manager): the pick cannot be the default.
        assert!(
            status(
                EchoMode::Device,
                &AecPhase::Off,
                Some(("hdmi", "HDMI out")),
                None
            )
            .offer_half_duplex
        );
    }

    #[test]
    fn app_mode_falls_back_to_half_duplex_with_the_reason() {
        let reason = "the echo-cancel nodes did not appear within 4 s (aec.ready_timeout_s)";
        let s = status(
            EchoMode::App,
            &AecPhase::Failed(reason.into()),
            Some(("hdmi", "HDMI out")),
            Some("spk"),
        );
        assert_eq!(
            (s.chosen, s.effective),
            (EchoMode::App, EchoMode::HalfDuplex)
        );
        assert!(s.half_duplex());
        let note = s.note.unwrap();
        assert!(
            note.contains(reason) && note.contains("half duplex instead"),
            "{note}"
        );
        assert!(!s.offer_half_duplex, "the output warning is device mode's");
    }

    #[test]
    fn app_mode_while_starting_and_active_is_full_duplex() {
        for phase in [AecPhase::Starting, AecPhase::Active] {
            let s = status(EchoMode::App, &phase, None, Some("spk"));
            assert_eq!(s.effective, EchoMode::App);
            assert!(!s.half_duplex());
            assert!(s.note.is_some());
        }
    }

    #[test]
    fn headphones_and_half_duplex_are_what_they_say() {
        let s = status(
            EchoMode::Headphones,
            &AecPhase::Off,
            Some(("x", "X")),
            Some("y"),
        );
        assert_eq!(
            (s.effective, s.half_duplex(), s.note),
            (EchoMode::Headphones, false, None)
        );
        let s = status(EchoMode::HalfDuplex, &AecPhase::Off, None, None);
        assert!(s.half_duplex());
    }

    #[test]
    fn the_config_keys_parse() {
        #[derive(Deserialize)]
        struct T {
            m: EchoMode,
        }
        for m in EchoMode::ALL {
            let t: T = toml::from_str(&format!("m = {:?}", m.key())).unwrap();
            assert_eq!(t.m, m);
        }
    }
}
