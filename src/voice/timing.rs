//! `lmgw.response.timing` (chat-voice §8.6) as the tooltip's timing line (§7) and the fallbacks
//! that answered (K21).
//!
//! ```json
//! {"response_id": "resp_…", "message_id": 812, "end_of_turn_ms": 400, "asr_ms": 30,
//!  "first_token_ms": 200, "first_clause_ms": 100, "first_audio_ms": 40, "total_ms": 4000,
//!  "to_first_audio_ms": 800, "cold": ["tts"],
//!  "models": {"asr": {"alias": "…", "answered_by": null}, "chat": {"alias": "…", "answered_by": "…"},
//!             "tts": {"alias": "…", "answered_by": null, "voice": "…"}}}
//! ```

use serde_json::Value;

/// The stages in the order the line names them.
pub const STAGES: [&str; 3] = ["asr", "chat", "tts"];

/// A stage answered by another alias than the one asked for (`answered_by`, K21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub stage: String,
    pub alias: String,
    pub answered_by: String,
}

/// The stages a fallback answered, in [`STAGES`] order.
pub fn fallbacks(t: &Value) -> Vec<Fallback> {
    STAGES
        .iter()
        .filter_map(|stage| {
            let m = &t["models"][*stage];
            let alias = m["alias"].as_str().unwrap_or_default();
            let by = m["answered_by"]
                .as_str()
                .filter(|b| !b.is_empty() && *b != alias)?;
            Some(Fallback {
                stage: (*stage).to_owned(),
                alias: alias.to_owned(),
                answered_by: by.to_owned(),
            })
        })
        .collect()
}

/// The line (§7): "first audio 800 ms (turn end 400, ASR 30, first token 200, clause 100, audio
/// 40), chat gemma4-e4b, cold: tts", naming `answered_by` where a fallback answered. `None` when the
/// event carries no stage time at all.
pub fn line(t: &Value) -> Option<String> {
    let ms = |k: &str| t[k].as_u64();
    let parts: Vec<String> = [
        ("turn end", "end_of_turn_ms"),
        ("ASR", "asr_ms"),
        ("first token", "first_token_ms"),
        ("clause", "first_clause_ms"),
        ("audio", "first_audio_ms"),
    ]
    .iter()
    .filter_map(|(name, key)| ms(key).map(|v| format!("{name} {v}")))
    .collect();
    let head = match ms("to_first_audio_ms") {
        Some(total) if parts.is_empty() => format!("first audio {total} ms"),
        Some(total) => format!("first audio {total} ms ({})", parts.join(", ")),
        None if parts.is_empty() => return None,
        None => format!("no audio ({})", parts.join(", ")),
    };
    let mut line = head;
    if let Some(alias) = t["models"]["chat"]["alias"]
        .as_str()
        .filter(|a| !a.is_empty())
    {
        line.push_str(&format!(", chat {alias}"));
    }
    for f in fallbacks(t) {
        line.push_str(&format!(
            ", {} answered by {} (not {})",
            stage_name(&f.stage),
            f.answered_by,
            f.alias
        ));
    }
    let cold: Vec<&str> = t["cold"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !cold.is_empty() {
        line.push_str(&format!(", cold: {}", cold.join(", ")));
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

    #[test]
    fn the_line_is_section_7s() {
        let t = json!({"response_id": "r", "end_of_turn_ms": 400, "asr_ms": 30,
            "first_token_ms": 200, "first_clause_ms": 100, "first_audio_ms": 40,
            "to_first_audio_ms": 800, "cold": ["tts"],
            "models": {"asr": {"alias": "parakeet", "answered_by": null},
                       "chat": {"alias": "gemma4-e4b", "answered_by": null},
                       "tts": {"alias": "supertonic", "answered_by": null}}});
        assert_eq!(
            line(&t).unwrap(),
            "first audio 800 ms (turn end 400, ASR 30, first token 200, clause 100, audio 40), \
             chat gemma4-e4b, cold: tts"
        );
        assert!(fallbacks(&t).is_empty());
    }

    #[test]
    fn a_fallback_is_named_in_the_line_and_listed() {
        let t = json!({"to_first_audio_ms": 900, "asr_ms": 40,
            "models": {"asr": {"alias": "parakeet", "answered_by": "openai/whisper-1"},
                       "chat": {"alias": "gemma4-e4b", "answered_by": "gemma4-e4b"},
                       "tts": {"alias": "supertonic"}}});
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
        assert_eq!(line(&json!({"models": {}})), None);
    }
}
