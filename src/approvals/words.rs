//! The spoken answer (K17, §11.1): what the recording is sent as, and how its transcript is read.
//!
//! **The match is the whole utterance** (WP19, the spec left it open): the transcript, lower-cased,
//! apostrophes dropped and every other character that is not a letter or a digit read as a space,
//! must be one of `approvals.yes` or `approvals.no` normalised the same way. "Ja." and "Yes!"
//! approve; "ja, aber nicht jetzt" and "mach es nicht" are in neither list, so the card stays and
//! says so. A word search would read "mach es nicht" as yes: an approval must not hinge on a word
//! the recogniser dropped, so only a whole answer counts, and the lists hold the short phrases
//! people say.

use crate::audio::RATE;

/// `text` as the lists are compared (module doc).
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '`' | '\u{b4}') {
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The answer `heard` gives: `Some(true)` approves, `Some(false)` declines, `None` is neither.
pub fn answer(heard: &str, yes: &[String], no: &[String]) -> Option<bool> {
    let said = normalise(heard);
    if said.is_empty() {
        return None;
    }
    let is = |list: &[String]| list.iter().any(|p| normalise(p) == said);
    match (is(yes), is(no)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        // Both cannot happen with a configuration that loaded (`ApprovalsConfig::validate`).
        _ => None,
    }
}

/// How much of `samples` (mono PCM16 at Kai's [`RATE`]) is speech by its level, in ms: the 20 ms
/// frames whose RMS is `dbfs` or louder (`approvals.speech_dbfs`). An energy check, no voice
/// detection: it tells a held key over a quiet room from a spoken word, which is what keeps the
/// recogniser from making words of near silence.
pub fn speech_ms(samples: &[i16], dbfs: i32) -> u64 {
    const FRAME_MS: u64 = 20;
    let frame = (RATE as u64 * FRAME_MS / 1000) as usize;
    // RMS ≥ 32768 · 10^(dbfs/20), compared as mean squares.
    let floor = 32768.0 * 10f64.powf(f64::from(dbfs) / 20.0);
    let floor_sq = floor * floor;
    samples
        .chunks(frame)
        .filter(|f| f.len() == frame)
        .filter(|f| {
            let sum: f64 = f.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
            sum / f.len() as f64 >= floor_sq
        })
        .count() as u64
        * FRAME_MS
}

/// `samples` (mono PCM16 at Kai's [`RATE`]) as a WAV file, what `…/transcribe` takes as
/// `audio/wav` (and reads the length of).
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::settings::ApprovalsConfig;

    fn lists() -> (Vec<String>, Vec<String>) {
        let c = ApprovalsConfig::default();
        (c.yes, c.no)
    }

    #[test]
    fn a_whole_answer_matches_and_anything_else_is_neither() {
        let (yes, no) = lists();
        for (heard, want) in [
            ("Ja.", Some(true)),
            ("Yes!", Some(true)),
            ("  yes   please ", Some(true)),
            ("Mach das.", Some(true)),
            ("Genehmigen.", Some(true)),
            ("Nein", Some(false)),
            ("No.", Some(false)),
            ("Don't!", Some(false)),
            ("don’t", Some(false)),
            ("Nein, danke.", Some(false)),
            // Neither: more than an answer, or nothing.
            ("ja, aber nicht jetzt", None),
            ("mach es nicht", None),
            ("yes no", None),
            // The filler words a recogniser makes of near silence approve nothing by default.
            ("OK", None),
            ("Okay.", None),
            ("Sure.", None),
            ("Yeah.", None),
            ("Genau.", None),
            ("Mach.", None),
            ("", None),
            ("...", None),
            ("Wie spät ist es?", None),
        ] {
            assert_eq!(answer(heard, &yes, &no), want, "{heard:?}");
        }
    }

    #[test]
    fn the_lists_are_validated() {
        let mut c = ApprovalsConfig::default();
        assert!(c.validate().is_ok());
        c.yes.push("Nein!".into());
        let (key, value, rule) = c.validate().unwrap_err();
        assert_eq!((key, value.as_str()), ("approvals.yes", "\"Nein!\""));
        assert!(rule.contains("approvals.no too"), "{rule}");
        let mut c = ApprovalsConfig::default();
        c.no.push(" ?! ".into());
        assert_eq!(c.validate().unwrap_err().0, "approvals.no");
        let c = ApprovalsConfig {
            yes: vec![],
            no: vec![],
            ..ApprovalsConfig::default()
        };
        assert!(
            c.validate().is_ok(),
            "empty lists: only clicks and keys answer"
        );
    }

    #[test]
    fn speech_is_counted_by_its_level_in_20_ms_frames() {
        let frame = (RATE / 50) as usize;
        // A quiet room (-60 dBFS RMS square wave) and a word (-20 dBFS) after it.
        let quiet = 10f64.powf(-60.0 / 20.0) * 32768.0;
        let loud = 10f64.powf(-20.0 / 20.0) * 32768.0;
        let wave = |amp: f64, frames: usize| -> Vec<i16> {
            (0..frames * frame)
                .map(|i| {
                    if i % 2 == 0 {
                        amp as i16
                    } else {
                        -(amp as i16)
                    }
                })
                .collect()
        };
        let mut rec = wave(quiet, 25);
        assert_eq!(speech_ms(&rec, -40), 0, "half a second of room noise");
        rec.extend(wave(loud, 10));
        assert_eq!(speech_ms(&rec, -40), 200);
        assert_eq!(speech_ms(&rec, -10), 0, "a stricter level");
        assert_eq!(speech_ms(&rec, -70), 700, "a level under the room");
        assert_eq!(speech_ms(&rec[..frame - 1], -70), 0, "no whole frame");
    }

    #[test]
    fn the_recording_is_a_wav_of_its_samples() {
        let w = wav(&[1, -2, 3]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), RATE);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), -2);
    }
}
