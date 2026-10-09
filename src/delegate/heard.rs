//! The user's words for the turn a delegate is called in (K25: the delegate gets them verbatim):
//! the turn's user message as lmgw put it into the thread (`lmgw.chat.user`), recorded by the
//! voice's event handling. Only Kai's own calls get it: a call from another principal (the
//! dashboard, the phone, an agent) comes from a conversation Kai does not hear, and lmgw stamps no
//! utterance on a call (api §5.5), so its delegate gets the parameter alone and its prompt says so.
//!
//! **Whose turn.** The user message of a turn that the model hears as audio races its reply, and
//! one that comes after the reply's start belongs to that reply, so a value is the current turn's
//! only if it came after the response began (or names that response). A response's start drops
//! what was kept from before, and so does the end of the session: a call with nothing for its turn
//! gets none, never the previous turn's words.
//!
//! Kept in memory only, never logged.

use std::sync::Mutex;

/// What is known of the current turn's words.
#[derive(Debug, Default)]
pub struct Heard {
    /// The response now running, as `response.created` named it.
    response: Option<String>,
    /// The words, and the response they named (heard turns name theirs).
    words: Option<(Option<String>, String)>,
}

impl Heard {
    /// A response began: the words kept so far are a past turn's unless they name this response.
    pub fn response_created(&mut self, id: &str) {
        if self
            .words
            .as_ref()
            .is_none_or(|(r, _)| r.as_deref() != Some(id))
        {
            self.words = None;
        }
        self.response = Some(id.to_owned());
    }

    /// The turn's user message came, naming its response when it was heard as audio. One that
    /// names a response other than the running one is a late event of a past turn.
    pub fn user(&mut self, text: &str, response: Option<&str>) {
        if let (Some(named), Some(now)) = (response, self.response.as_deref())
            && named != now
        {
            return;
        }
        self.words = Some((response.map(str::to_owned), text.to_owned()));
    }

    /// The session ended: no turn is running.
    pub fn session_ended(&mut self) {
        *self = Self::default();
    }

    /// The current turn's words, if they have come.
    pub fn words(&self) -> Option<String> {
        self.words.as_ref().map(|(_, w)| w.clone())
    }
}

static HEARD: Mutex<Option<Heard>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Heard) -> R) -> R {
    f(HEARD
        .lock()
        .expect("never poisoned")
        .get_or_insert_with(Heard::default))
}

/// The voice session's turn carried `text` as its user message (`response`: the heard response it
/// names, if any).
pub fn heard(text: &str, response: Option<&str>) {
    with(|h| h.user(text, response));
}

/// A response of the voice session began.
pub fn response_created(id: &str) {
    with(|h| h.response_created(id));
}

/// The voice session ended.
pub fn session_ended() {
    with(Heard::session_ended);
}

/// The current turn's words, if they have come.
pub fn last() -> Option<String> {
    with(|h| h.words())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_a_past_turn_are_not_this_turns() {
        let mut h = Heard::default();
        h.response_created("r1");
        h.user("first", None);
        assert_eq!(h.words().as_deref(), Some("first"));
        // The next turn's response began; its user message has not come (the race).
        h.response_created("r2");
        assert_eq!(h.words(), None, "nothing came for r2 yet");
        h.user("second", Some("r2"));
        assert_eq!(h.words().as_deref(), Some("second"));
    }

    #[test]
    fn a_user_message_that_named_the_response_before_it_began_is_kept() {
        let mut h = Heard::default();
        h.user("early", Some("r2"));
        h.response_created("r2");
        assert_eq!(h.words().as_deref(), Some("early"));
    }

    #[test]
    fn a_late_user_message_of_a_past_response_is_dropped() {
        let mut h = Heard::default();
        h.response_created("r2");
        h.user("late first", Some("r1"));
        assert_eq!(h.words(), None);
    }

    #[test]
    fn nothing_outlives_the_session() {
        let mut h = Heard::default();
        h.response_created("r1");
        h.user("words", Some("r1"));
        h.session_ended();
        assert_eq!(h.words(), None);
        // The next session's first message is its own.
        h.user("again", None);
        assert_eq!(h.words().as_deref(), Some("again"));
    }
}
