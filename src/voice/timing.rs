//! `lmgw.response.timing` (chat-voice §8.6) as the tooltip's timing line (§7) and the fallbacks
//! that answered (K21). The event is lmgw's typed [`VoiceTiming`].

use lmgw_client::realtime::VoiceTiming;

/// A stage answered by another alias than the one asked for (`answered_by`, K21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub stage: String,
    pub alias: String,
    pub answered_by: String,
}

/// The stages a fallback answered, in the order the line names them (ASR, chat, TTS).
pub fn fallbacks(t: &VoiceTiming) -> Vec<Fallback> {
    [
        ("asr", &t.models.asr),
        ("chat", &t.models.chat),
        ("tts", &t.models.tts),
    ]
    .into_iter()
    .filter_map(|(stage, m)| {
        let m = m.as_ref()?;
        let by = m
            .answered_by
            .as_deref()
            .filter(|b| !b.is_empty() && *b != m.alias)?;
        Some(Fallback {
            stage: stage.to_owned(),
            alias: m.alias.clone(),
            answered_by: by.to_owned(),
        })
    })
    .collect()
}

/// The line (§7): "first audio 800 ms (turn end 400, ASR 30, first token 200, clause 100, audio
/// 40), chat gemma4-e4b, cold: tts", naming `answered_by` where a fallback answered. `None` when the
/// event carries no stage time at all.
pub fn line(t: &VoiceTiming) -> Option<String> {
    let parts: Vec<String> = [
        ("turn end", t.end_of_turn_ms),
        ("ASR", t.asr_ms),
        ("first token", t.first_token_ms),
        ("clause", t.first_clause_ms),
        ("audio", t.first_audio_ms),
    ]
    .iter()
    .filter_map(|(name, ms)| ms.map(|v| format!("{name} {v}")))
    .collect();
    let head = match t.to_first_audio_ms {
        Some(total) if parts.is_empty() => format!("first audio {total} ms"),
        Some(total) => format!("first audio {total} ms ({})", parts.join(", ")),
        None if parts.is_empty() => return None,
        None => format!("no audio ({})", parts.join(", ")),
    };
    let mut line = head;
    if let Some(chat) = t.models.chat.as_ref().filter(|m| !m.alias.is_empty()) {
        line.push_str(&format!(", chat {}", chat.alias));
    }
    for f in fallbacks(t) {
        line.push_str(&format!(
            ", {} answered by {} (not {})",
            stage_name(&f.stage),
            f.answered_by,
            f.alias
        ));
    }
    if !t.cold.is_empty() {
        line.push_str(&format!(", cold: {}", t.cold.join(", ")));
    }
    Some(line)
}

/// How a stage is named in a line or a notification.
pub fn stage_name(stage: &str) -> &str {
    match stage {
        "asr" => "ASR",
        "chat" => "chat",
        "tts" => "TTS",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn timing(v: serde_json::Value) -> VoiceTiming {
        serde_json::from_value(v).expect("a VoiceTiming")
    }

    #[test]
    fn the_line_is_section_7s() {
        let t = timing(
            json!({"response_id": "r", "end_of_turn_ms": 400, "asr_ms": 30,
            "first_token_ms": 200, "first_clause_ms": 100, "first_audio_ms": 40,
            "to_first_audio_ms": 800, "cold": ["tts"],
            "models": {"asr": {"alias": "parakeet", "answered_by": null},
                       "chat": {"alias": "gemma4-e4b", "answered_by": null},
                       "tts": {"alias": "supertonic", "answered_by": null}}}),
        );
        assert_eq!(
            line(&t).unwrap(),
            "first audio 800 ms (turn end 400, ASR 30, first token 200, clause 100, audio 40), \
             chat gemma4-e4b, cold: tts"
        );
        assert!(fallbacks(&t).is_empty());
    }

    #[test]
    fn a_fallback_is_named_in_the_line_and_listed() {
        let t = timing(json!({"to_first_audio_ms": 900, "asr_ms": 40,
            "models": {"asr": {"alias": "parakeet", "answered_by": "openai/whisper-1"},
                       "chat": {"alias": "gemma4-e4b", "answered_by": "gemma4-e4b"},
                       "tts": {"alias": "supertonic"}}}));
        assert_eq!(
            line(&t).unwrap(),
            "first audio 900 ms (ASR 40), chat gemma4-e4b, ASR answered by openai/whisper-1 \
             (not parakeet)"
        );
        assert_eq!(
            fallbacks(&t),
            vec![Fallback {
                stage: "asr".into(),
                alias: "parakeet".into(),
                answered_by: "openai/whisper-1".into()
            }],
            "answered_by equal to the alias is no fallback"
        );
        assert_eq!(line(&VoiceTiming::default()), None);
    }
}
