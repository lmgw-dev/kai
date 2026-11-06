//! Kai's own questions answered by voice (kai-firefox WP10b, *design decision*): the questions
//! of Kai's own dialog (K15's, PD4) that a call of Kai's own asks while it is prepared or runs (the
//! browser's focus rule, `element_file`, the commit guard's verdicts and its network gate) are
//! spoken by Kai and answered as the approval card's calls of Kai's own are (K17), so a hands-free
//! and eyes-free user can get through them. The dialog shows as before; whichever answers first
//! decides, and the other goes.
//!
//! - **Which questions.** Those the asker gives words to speak ([`Confirmer::ask_spoken`]) for a
//!   call of Kai's own device (`Caller::Own`: its voice session, its typed turn, the sidebar), as
//!   K17 takes only Kai's own calls. Another caller's question shows the dialog alone.
//! - **The question is spoken** in Kai's voice ([`crate::voice::speak`]'s `Kind::Question`),
//!   Kai's words first and the page's text last, framed as the page's ("The page calls the link:
//!   Order now", design §7.2). It plays while the call waits, so not at the quiet moment Kai's other
//!   speech waits for, but once nothing else is heard (Kai's reply, the talk key) and never under do
//!   not disturb or with PipeWire down.
//! - **One question at a time** (kai-firefox WP10b's review, M2): the oldest waiting question is
//!   spoken; the others wait in line, each spoken once the one before it is over (answered at the
//!   dialog or by voice, or no longer waited for), so a "yes" never answers a question the owner
//!   did not just hear.
//! - **The spoken answer is K17's**: while the question waits, Kai's voice session gets no audio
//!   (a spoken "yes" would become a turn, which ends the call's response); a press of the talk key
//!   records the answer once the question was spoken (or could not be: it ended), a press before
//!   that only stops the speech; the recording must hold speech, is transcribed in Kai's own thread
//!   (`…/transcribe`) and is matched whole against `approvals.yes` and `approvals.no`. A barge-in
//!   or a press is never a yes; only the owner's words are. An answer in neither list keeps the
//!   question and Kai says "Say yes or no"; so does a recording without speech.
//! - **The press answers what was heard last** (M2): of the card's call of Kai's own the card
//!   armed for and the question whose speech ended, the one that became answerable last; Kai's
//!   "say yes or no" makes its question that one again. A card's call not armed yet holds back no
//!   question (L4).
//! - **Nothing Kai said is the answer** (M1): the recording starts at the press, with no pre-roll,
//!   and what the microphone hears while Kai speaks (its reply, its own speech) and
//!   `browser.speech_echo_guard_ms` after is silence in it (`crate::voice`'s `drain_capture`); the
//!   answer press stops Kai's speech, another question's included.
//! - **Its end**: the dialog's answer, the voice's, or the host no longer waiting (lmgw's cancel,
//!   the deadline, the screen lock) closes both: the dialog's channel is dropped, the question's
//!   speech is cancelled and the voice forgets it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{Act, Approvals};
use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::voice::link::Refusal;
use crate::voice::speak::{Kind, Said, Spoken, Text, Utterance, Words};

/// One of Kai's own questions, on its way to the voice ([`VoiceConfirmer`]).
pub struct Ask {
    /// `kai_ask_<n>`: never an lmgw approval's id.
    pub id: String,
    /// The question in Kai's words (module doc).
    pub words: Words,
    /// The voice's answer.
    pub answer: oneshot::Sender<bool>,
    /// Fires when the question is over, however it ended.
    pub over: CancellationToken,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Kai's dialog with a voice beside it (module doc).
pub struct VoiceConfirmer {
    dialog: Arc<dyn Confirmer>,
    voice: Arc<dyn Fn(Ask) + Send + Sync>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl VoiceConfirmer {
    /// `voice` hands a question to Kai's voice (the core's `Command::Ask`; the tests' fakes).
    pub fn new(dialog: Arc<dyn Confirmer>, voice: Arc<dyn Fn(Ask) + Send + Sync>) -> Self {
        Self { dialog, voice }
    }

    /// The dialog, and the core's voice.
    pub fn to_core(
        dialog: Arc<dyn Confirmer>,
        cmds: tokio::sync::mpsc::UnboundedSender<crate::core::Command>,
    ) -> Self {
        Self::new(
            dialog,
            Arc::new(move |a| {
                let _ = cmds.send(crate::core::Command::Ask(Box::new(a)));
            }),
        )
    }
}

impl Confirmer for VoiceConfirmer {
    fn ask(&self, req: ConfirmRequest) {
        self.dialog.ask(req);
    }

    fn ask_spoken(&self, req: ConfirmRequest, spoken: Words) {
        let ConfirmRequest {
            tool,
            description,
            caller,
            lines,
            destructive,
            deadline,
            options,
            notes,
            reply,
        } = req;
        let (to_dialog, from_dialog) = oneshot::channel();
        let (to_ask, from_voice) = oneshot::channel();
        let over = CancellationToken::new();
        let id = format!("kai_ask_{}", NEXT.fetch_add(1, Ordering::Relaxed));
        tracing::info!(question = %id, tool = %tool, "approvals: Kai's question goes to the voice too");
        (self.voice)(Ask {
            id: id.clone(),
            words: spoken,
            answer: to_ask,
            over: over.clone(),
        });
        self.dialog.ask(ConfirmRequest {
            tool,
            description,
            caller,
            lines,
            destructive,
            deadline,
            options,
            notes,
            reply: to_dialog,
        });
        tokio::spawn(decide(id, reply, from_dialog, from_voice, over));
    }
}

/// The first answer of the dialog's and the voice's goes to the asker (module doc). The voice going
/// away leaves the dialog; the dialog going away (it could not be shown) is the asker's "not
/// shown", as without a voice.
async fn decide(
    id: String,
    mut reply: oneshot::Sender<bool>,
    mut dialog: oneshot::Receiver<bool>,
    voice: oneshot::Receiver<bool>,
    over: CancellationToken,
) {
    let mut voice = Some(voice);
    let answer = loop {
        tokio::select! {
            a = &mut dialog => break a.ok(),
            v = async {
                match voice.as_mut() {
                    Some(v) => v.await,
                    None => std::future::pending().await,
                }
            } => match v {
                Ok(yes) => {
                    tracing::info!(question = %id, yes, "approvals: Kai's question answered by voice");
                    break Some(yes);
                }
                Err(_) => voice = None,
            },
            () = reply.closed() => break None,
        }
    };
    over.cancel();
    drop(dialog);
    if let Some(a) = answer {
        let _ = reply.send(a);
    }
}

/// A question the voice holds.
pub(super) struct Waiting {
    id: String,
    tag: u64,
    /// The question in Kai's words, spoken when its turn in line comes (module doc).
    words: Words,
    /// It was handed to the speech.
    spoken: bool,
    answer: Option<oneshot::Sender<bool>>,
    over: CancellationToken,
    /// Its spoken question (or Kai's "say yes or no") last ended (heard, cut, failed), by the
    /// approvals' count of what became answerable: a press answers it from then on.
    armed: Option<u64>,
}

/// Kai's words when the answer was neither yes nor no, or held no speech.
fn again(heard: Option<&str>) -> Words {
    match heard.map(str::trim).filter(|h| !h.is_empty()) {
        // The owner's own words are not repeated: the speech says what Kai needs.
        Some(_) => Words::new(
            "Kai heard neither yes nor no. Hold the talk key and say yes or no.",
            "Kai hat weder ja noch nein gehört. Halte die Sprechtaste und sag ja oder nein.",
        ),
        None => Words::new(
            "Kai heard no answer. Hold the talk key and say yes or no.",
            "Kai hat keine Antwort gehört. Halte die Sprechtaste und sag ja oder nein.",
        ),
    }
}

impl Approvals {
    /// A question of Kai's own (module doc): kept, and the utterance that speaks it for the voice
    /// when no other question is in line before it; else it waits for its turn ([`Self::ask_over`]
    /// hands it over then, as an [`Act::Speak`]).
    pub fn asked(&mut self, a: Ask) -> Option<Utterance> {
        self.ask_tags += 1;
        let tag = self.ask_tags;
        let over = a.over.clone();
        let id = a.id.clone();
        self.overs.push(Box::pin(async move {
            over.cancelled_owned().await;
            id
        }));
        let first = self.asks.is_empty();
        tracing::info!(question = %a.id, first, "approvals: Kai asks by voice too");
        self.asks.push(Waiting {
            id: a.id.clone(),
            tag,
            words: a.words.clone(),
            spoken: first,
            answer: Some(a.answer),
            over: a.over.clone(),
            armed: None,
        });
        first.then(|| self.question(tag, a.over, a.words))
    }

    /// The utterance of question `tag`'s words.
    fn question(&self, tag: u64, over: CancellationToken, words: Words) -> Utterance {
        Utterance {
            kind: Kind::Question,
            text: Text::Kai(words),
            voice: None,
            gate: None,
            cancel: over,
            tag,
            reply: Some(self.said_tx.clone()),
        }
    }

    /// The utterance for an [`Act::Speak`] the approvals asked for.
    pub fn speak_again(&self, id: &str, words: Words) -> Option<Utterance> {
        let w = self.asks.iter().find(|w| w.id == id)?;
        Some(self.question(w.tag, w.over.clone(), words))
    }

    /// A question of Kai's own waits: Kai's voice session gets no audio (module doc).
    pub(super) fn asks_wait(&self) -> bool {
        !self.asks.is_empty()
    }

    /// The question a press may answer now, with when it became answerable (module doc): the one
    /// whose speech ended last.
    pub(super) fn ask_armed(&self) -> Option<(u64, &str)> {
        self.asks
            .iter()
            .filter_map(|w| w.armed.map(|at| (at, w.id.as_str())))
            .max_by_key(|(at, _)| *at)
    }

    pub(super) fn is_ask(&self, id: &str) -> bool {
        self.asks.iter().any(|w| w.id == id)
    }

    /// What the speech said of question `tag`: each end of its speech (the question, Kai's "say
    /// yes or no") arms it as what became answerable last (module doc).
    pub(super) fn ask_said(&mut self, tag: u64, said: Said) {
        let Said::Done(spoken) = said else {
            return;
        };
        self.arms += 1;
        let at = self.arms;
        if let Some(w) = self.asks.iter_mut().find(|w| w.tag == tag) {
            if let Spoken::Failed(f) = &spoken {
                tracing::info!(question = %w.id, why = %f, "approvals: Kai's question could not be spoken; the dialog shows it");
            }
            w.armed = Some(at);
        }
    }

    /// Question `id` is over (module doc): the next in line is spoken now.
    pub(super) fn ask_over(&mut self, id: &str) -> Vec<Act> {
        let before = self.asks.len();
        self.asks.retain(|w| w.id != id);
        if self.asks.len() == before {
            return Vec::new();
        }
        tracing::debug!(question = %id, "approvals: Kai's question is over");
        self.drop_answer_to(id);
        match self.asks.first_mut().filter(|w| !w.spoken) {
            Some(next) => {
                next.spoken = true;
                tracing::info!(question = %next.id, "approvals: the next of Kai's questions is spoken");
                vec![Act::Speak {
                    id: next.id.clone(),
                    words: next.words.clone(),
                }]
            }
            None => Vec::new(),
        }
    }

    /// The thread Kai's spoken answer to a question of its own is transcribed in: Kai's own
    /// (the bound session's, else the conversation's).
    pub(super) fn ask_thread(&self) -> Option<i64> {
        self.own.first().copied()
    }

    /// The recording for question `id` held no speech, or no thread can transcribe it.
    pub(super) fn ask_not_heard(&mut self, id: &str) -> Vec<Act> {
        vec![Act::Speak {
            id: id.to_owned(),
            words: again(None),
        }]
    }

    /// The transcript of the answer to question `id` (module doc).
    pub(super) fn ask_heard(&mut self, id: &str, r: Result<String, Refusal>) -> Vec<Act> {
        if self.hearing.as_deref() == Some(id) {
            self.hearing = None;
        }
        let text = match r {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(question = %id, status = e.status, code = ?e.code, message = %e.message, "approvals: the spoken answer to Kai's question was not transcribed");
                return self.ask_not_heard(id);
            }
        };
        match super::words::answer(&text, &self.s.yes, &self.s.no) {
            Some(yes) => {
                if let Some(a) = self
                    .asks
                    .iter_mut()
                    .find(|w| w.id == id)
                    .and_then(|w| w.answer.take())
                {
                    let _ = a.send(yes);
                }
                // The asker ends it ([`decide`]): `over` fires and the voice forgets it.
                Vec::new()
            }
            None => {
                tracing::info!(question = %id, chars = text.chars().count(), "approvals: the spoken answer to Kai's question is neither yes nor no");
                vec![Act::Speak {
                    id: id.to_owned(),
                    words: again(Some(&text)),
                }]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Held(Mutex<Vec<oneshot::Sender<bool>>>);

    impl Confirmer for Held {
        fn ask(&self, req: ConfirmRequest) {
            self.0.lock().unwrap().push(req.reply);
        }
    }

    fn request(reply: oneshot::Sender<bool>) -> ConfirmRequest {
        ConfirmRequest {
            tool: "element_press".into(),
            description: String::new(),
            caller: "Kai itself".into(),
            lines: Vec::new(),
            destructive: false,
            deadline: None,
            options: Vec::new(),
            notes: Vec::new(),
            reply,
        }
    }

    fn pair() -> (Arc<Held>, Arc<Mutex<Vec<Ask>>>, VoiceConfirmer) {
        let dialog = Arc::new(Held(Mutex::new(Vec::new())));
        let asks = Arc::new(Mutex::new(Vec::new()));
        let to = asks.clone();
        let c = VoiceConfirmer::new(
            dialog.clone(),
            Arc::new(move |a| to.lock().unwrap().push(a)),
        );
        (dialog, asks, c)
    }

    /// The voice's answer goes to the asker and closes the dialog; the dialog's answer ends the
    /// voice's question; the asker no longer waiting ends both.
    #[tokio::test]
    async fn the_first_answer_decides_and_ends_the_other() {
        let (dialog, asks, c) = pair();
        let (reply, answer) = oneshot::channel();
        c.ask_spoken(request(reply), Words::new("May Kai?", "Darf Kai?"));
        let a = asks.lock().unwrap().pop().unwrap();
        assert!(a.id.starts_with("kai_ask_"));
        a.answer.send(true).unwrap();
        assert_eq!(answer.await, Ok(true));
        a.over.cancelled().await;
        let mut held = dialog.0.lock().unwrap().pop().unwrap();
        held.closed().await;

        let (reply, answer) = oneshot::channel();
        c.ask_spoken(request(reply), Words::new("May Kai?", "Darf Kai?"));
        let a = asks.lock().unwrap().pop().unwrap();
        dialog.0.lock().unwrap().pop().unwrap().send(false).unwrap();
        assert_eq!(answer.await, Ok(false));
        a.over.cancelled().await;

        let (reply, answer) = oneshot::channel();
        c.ask_spoken(request(reply), Words::new("May Kai?", "Darf Kai?"));
        let a = asks.lock().unwrap().pop().unwrap();
        drop(answer);
        a.over.cancelled().await;
        let mut held = dialog.0.lock().unwrap().pop().unwrap();
        held.closed().await;
    }

    /// A voice that goes away leaves the dialog to answer; a plain question never reaches it.
    #[tokio::test]
    async fn the_dialog_answers_when_the_voice_goes() {
        let (dialog, asks, c) = pair();
        let (reply, answer) = oneshot::channel();
        c.ask_spoken(request(reply), Words::new("May Kai?", "Darf Kai?"));
        drop(asks.lock().unwrap().pop().unwrap().answer);
        tokio::task::yield_now().await;
        dialog.0.lock().unwrap().pop().unwrap().send(true).unwrap();
        assert_eq!(answer.await, Ok(true));
        let (reply, _answer) = oneshot::channel();
        c.ask(request(reply));
        assert!(asks.lock().unwrap().is_empty());
        assert_eq!(dialog.0.lock().unwrap().len(), 1);
    }
}
