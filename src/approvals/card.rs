//! What the approval card shows (§11.1), and what goes between the voice and the GTK side
//! ([`super::overlay`]). Built here, without a display, and tested; the page only puts the texts
//! into its elements as text.

use std::borrow::Cow;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::json_shown;
use crate::overlay::placement::Anchor;

/// One call as the card shows it. Its `Debug` leaves the arguments out, so no log line can carry
/// them.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct CardModel {
    /// The call's `approval_request_id`: an answer quotes it, so an answer meant for a call that
    /// has gone meanwhile answers nothing.
    pub call: String,
    /// "Approve kailive__window_close?"
    pub heading: String,
    /// Who asked, and where: "asked by the dashboard in thread 'Plans'".
    pub asked: String,
    /// The arguments in K22's compact lines (`name: value`), never shortened.
    pub args: Vec<String>,
    /// "call 1 of 2 of this reply", when the reply waits on more than one.
    pub count: Option<String>,
    /// What the owner should know: a refusal, an answer heard that is neither yes nor no.
    pub note: Option<String>,
    /// While Kai records or transcribes the spoken answer: the buttons wait.
    pub busy: Option<String>,
    /// How to answer.
    pub hint: String,
    /// Said once a key reached the card before it armed for this call: Enter alone does not
    /// approve it then.
    pub typed: String,
}

impl std::fmt::Debug for CardModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardModel")
            .field("call", &self.call)
            .field("heading", &self.heading)
            .field("arguments", &self.args.len())
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

/// Where and how the card sits, and when it arms (`[approvals]`, `overlay.output`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub anchor: Anchor,
    pub margin: u32,
    pub width: u32,
    pub arm: std::time::Duration,
    pub output: Option<String>,
}

/// The owner's answer on the card (a click, Enter or Esc), for the call it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAnswer {
    pub call: String,
    pub approve: bool,
}

/// What the card's GTK side tells the approvals, each for the call it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardEvent {
    Answer(CardAnswer),
    /// The card armed: its keys and clicks answer now, and for a call of Kai's own the talk key
    /// records the spoken answer.
    Armed {
        call: String,
    },
    /// The card waits again before it answers (a key or a click before it armed, the keyboard
    /// arriving again).
    Unarmed {
        call: String,
    },
    /// The card cannot show the call (no layer-shell; its page ended twice): why.
    Lost {
        call: String,
        why: String,
    },
}

pub enum CardCmd {
    /// Show `model` (the card is created when there is none; a new call arms it again).
    Show {
        model: Box<CardModel>,
        look: Look,
        events: mpsc::UnboundedSender<CardEvent>,
    },
    /// No call waits any more: the card goes.
    Hide,
}

impl std::fmt::Debug for CardCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CardCmd::Show { model, .. } => write!(f, "Show({model:?})"),
            CardCmd::Hide => write!(f, "Hide"),
        }
    }
}

/// The call's arguments (a JSON string, as the model wrote them) in K22's compact lines: an
/// object's fields as `name: value` by name, a text as it is unless it holds a line break or
/// a bidi or format character (then JSON-quoted with those escaped, `confirm::shown_text`), any
/// other value as compact JSON. Arguments that are no object, or no JSON, are one line as sent.
pub fn arg_lines(arguments: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                let shown = match v {
                    Value::String(s) => shown_text(s).into_owned(),
                    other => json_shown(other),
                };
                format!("{}: {shown}", shown_text(k))
            })
            .collect(),
        Ok(Value::Null) => Vec::new(),
        _ if arguments.trim().is_empty() => Vec::new(),
        Ok(other) => vec![format!("arguments: {}", json_shown(&other))],
        Err(_) => vec![format!("arguments: {}", shown_text(arguments))],
    }
}

/// A name from lmgw (a label, a tool, a principal, a title) as the card shows it: escaped as the
/// dialog escapes a caller's text.
pub fn name(s: &str) -> Cow<'_, str> {
    shown_text(s)
}

/// A name inside a sentence of the card's (a device's, a thread's title): `'name'`, or JSON-quoted
/// and escaped when it holds a quote, a backslash or a deceptive character, so it cannot end its
/// quotes and go on as the card's own words ("x' in thread 'Plans").
pub fn quoted_name(s: &str) -> String {
    if s.contains(['\'', '"', '\\']) || matches!(shown_text(s), Cow::Owned(_)) {
        quoted(s)
    } else {
        format!("'{s}'")
    }
}

/// Who asked, from the feed's `by` (lmgw names authors "the dashboard" or "device '<name>'",
/// client-apps §1.7), by kind: the dashboard as the owner's, a device as `device '<name>'` with its
/// name quoted ([`quoted_name`]), anything else JSON-quoted as lmgw named it. A device named
/// "Kai's voice session" or "the owner" reads as a device.
pub fn asker(by: &str) -> String {
    // lmgw's `chat_feed::BY_ADMIN`.
    if by == "the dashboard" {
        return "the owner (the dashboard)".to_owned();
    }
    if let Some(name) = by
        .strip_prefix("device '")
        .and_then(|r| r.strip_suffix('\''))
    {
        return format!("device {}", quoted_name(name));
    }
    format!("a principal lmgw names {}", quoted(by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_compact_lines_never_shortened() {
        let long = "x".repeat(5000);
        let args = serde_json::json!({
            "window": "Firefox",
            "percent": 40,
            "urls": ["a", "b"],
            "text": "line one\nline two",
            "long": long,
        })
        .to_string();
        let lines = arg_lines(&args);
        assert!(lines.contains(&"window: Firefox".to_owned()), "{lines:?}");
        assert!(lines.contains(&"percent: 40".to_owned()));
        assert!(lines.contains(&r#"urls: ["a","b"]"#.to_owned()));
        assert!(lines.contains(&r#"text: "line one\nline two""#.to_owned()));
        assert!(lines.iter().any(|l| l.len() == "long: ".len() + 5000));
        // A forged line or a reordered one is escaped, a key too.
        let lines = arg_lines("{\"a\\u202eb\": \"c\\u202ed\"}");
        assert_eq!(lines, vec!["\"a\\u202eb\": \"c\\u202ed\"".to_owned()]);
        assert!(arg_lines("{}").is_empty());
        assert!(arg_lines("").is_empty());
        assert_eq!(arg_lines("[1,2]"), vec!["arguments: [1,2]".to_owned()]);
        assert_eq!(
            arg_lines("not json\n"),
            vec![r#"arguments: "not json\n""#.to_owned()]
        );
    }

    /// Who asked reads by kind: a device's name cannot pass for Kai's session or the owner, nor
    /// end its quotes and go on as the card's words.
    #[test]
    fn the_asker_reads_by_kind() {
        assert_eq!(asker("the dashboard"), "the owner (the dashboard)");
        assert_eq!(asker("device 'phone'"), "device 'phone'");
        assert_eq!(
            asker("device 'Kai's voice session'"),
            "device \"Kai's voice session\""
        );
        assert_eq!(asker("device 'the owner'"), "device 'the owner'");
        assert_eq!(
            asker("device 'x' in thread 'Plans'"),
            "device \"x' in thread 'Plans\""
        );
        assert_eq!(asker("the owner"), "a principal lmgw names \"the owner\"");
        assert_eq!(
            asker("Kai's voice session"),
            "a principal lmgw names \"Kai's voice session\""
        );
        assert_eq!(asker("device 'a\u{202e}b'"), "device \"a\\u202eb\"");
        assert_eq!(quoted_name("Plans"), "'Plans'");
        assert_eq!(
            quoted_name("Plans' asked by the owner"),
            "\"Plans' asked by the owner\""
        );
    }

    #[test]
    fn no_debug_line_carries_the_arguments() {
        let m = CardModel {
            call: "mcpr_1".into(),
            heading: "Approve x__y?".into(),
            asked: "asked by the owner".into(),
            args: vec!["text: secret-argument".into()],
            count: None,
            note: None,
            busy: None,
            hint: String::new(),
            typed: String::new(),
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let cmd = CardCmd::Show {
            model: Box::new(m.clone()),
            look: Look {
                anchor: Anchor::Top,
                margin: 0,
                width: 1,
                arm: std::time::Duration::ZERO,
                output: None,
            },
            events: tx,
        };
        assert!(!format!("{m:?}{cmd:?}").contains("secret-argument"));
    }
}
