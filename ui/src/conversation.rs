//! The Personality page's header, apart from the DOM: what Kai's conversation talks with and where
//! (`GET /api/conversation`, [`Conversation`]). Plain functions, tested natively.

use crate::wire::Conversation;

/// The profile Kai's conversation talks with, as the tray's Personality radio checks it.
pub fn talks_with(c: &Conversation) -> String {
    c.profile
        .clone()
        .unwrap_or_else(|| "no profile: lmgw lists none yet".to_owned())
}

/// Where it talks: the folder, the thread and the model, each said when Kai does not know it yet
/// (what the editor's Test and Preview then assemble as).
pub fn facts(c: &Conversation) -> String {
    let folder = match &c.folder {
        Some(f) => format!("folder {f}"),
        None => "the folder is not found yet".to_owned(),
    };
    let thread = match c.thread_id {
        Some(t) => format!("thread #{t}"),
        None => "no thread yet (Preview and Test assemble as a new thread)".to_owned(),
    };
    let model = match &c.model {
        Some(m) => format!("model {m}"),
        None => "the folder names no model".to_owned(),
    };
    format!("{folder} · {thread} · {model}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv() -> Conversation {
        Conversation {
            paired: true,
            why: None,
            gateway: Some("http://127.0.0.1:8001".into()),
            folder: Some("Kai".into()),
            thread_id: Some(41),
            model: Some("gemma4-12b".into()),
            profile: Some("Default (global: Concise)".into()),
        }
    }

    #[test]
    fn the_header_says_the_profile_and_where_it_talks() {
        let c = conv();
        assert_eq!(talks_with(&c), "Default (global: Concise)");
        assert_eq!(facts(&c), "folder Kai · thread #41 · model gemma4-12b");
    }

    #[test]
    fn what_kai_does_not_know_yet_is_said() {
        let c = Conversation {
            folder: None,
            thread_id: None,
            model: None,
            profile: None,
            ..conv()
        };
        assert_eq!(talks_with(&c), "no profile: lmgw lists none yet");
        let f = facts(&c);
        assert!(f.contains("folder is not found yet"), "{f}");
        assert!(f.contains("assemble as a new thread"), "{f}");
        assert!(f.contains("names no model"), "{f}");
    }
}
