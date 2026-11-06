//! The acceptance runs' voice for Kai's own questions (kai-firefox WP10b): every question a call of
//! Kai's own asks reaches it through the real `VoiceConfirmer` beside the owner's dialog, and is
//! held, armed and answered by the real approvals bookkeeping (`crate::approvals`: `asked`, the
//! arming when its speech ended, the talk key's press, the speech check, the whole-answer match).
//! What stands in: the speech (the stage's fake text-to-speech, which hears each question as it is
//! said and answers it as heard), and the owner's microphone and the transcription, since this
//! runs headless: when the script says so ([`Ears::will_say`]), the owner holds the talk key once
//! the question was spoken, says the words (a tone loud enough for the speech check) and lets go,
//! and the transcript is those words.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::stage::Tts;
use crate::approvals::asked::Ask;
use crate::approvals::card::Look;
use crate::approvals::{Act, ApprovalSettings, Approvals, Done, Op};
use crate::voice::speak::Mouth;

/// Kai's own thread, where the answer is transcribed.
const THREAD: i64 = 4242;

pub struct Ears {
    tx: mpsc::UnboundedSender<Ask>,
    words: Arc<Mutex<VecDeque<String>>>,
    task: JoinHandle<()>,
}

impl Ears {
    pub fn start(tts: Tts) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let words: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let task = tokio::spawn(run(rx, tts, words.clone()));
        Self { tx, words, task }
    }

    /// Where `VoiceConfirmer` hands Kai's questions.
    pub fn voice(&self) -> Arc<dyn Fn(Ask) + Send + Sync> {
        let tx = self.tx.clone();
        Arc::new(move |a| {
            let _ = tx.send(a);
        })
    }

    /// The owner's next spoken answer, said once Kai's next question was spoken.
    pub fn will_say(&self, words: &str) {
        self.words.lock().unwrap().push_back(words.to_owned());
    }

    /// Spoken answers the script expected and nobody asked for.
    pub fn unsaid(&self) -> Vec<String> {
        std::mem::take(&mut *self.words.lock().unwrap()).into()
    }
}

impl Drop for Ears {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn settings() -> ApprovalSettings {
    let d = crate::approvals::settings::ApprovalsConfig::default();
    ApprovalSettings {
        yes: d.yes,
        no: d.no,
        speech_dbfs: d.speech_dbfs,
        speech_ms: d.speech_ms,
        look: Look {
            anchor: crate::overlay::placement::Anchor::Top,
            margin: 48,
            width: 560,
            arm: std::time::Duration::from_millis(600),
            output: None,
        },
    }
}

/// One second of a tone at about -20 dBFS: speech by the check's measure.
fn spoken() -> Vec<i16> {
    (0..crate::audio::RATE as usize)
        .map(|i| if i % 2 == 0 { 3277 } else { -3277 })
        .collect()
}

async fn run(mut rx: mpsc::UnboundedReceiver<Ask>, tts: Tts, words: Arc<Mutex<VecDeque<String>>>) {
    let mut a = Approvals::new(settings(), Box::new(|_| {}));
    a.observe(vec![THREAD], Some(1));
    let mut saying: Option<String> = None;
    loop {
        let acts = tokio::select! {
            ask = rx.recv() => match ask {
                Some(ask) => {
                    // Spoken now, or when the questions before it are over (`Act::Speak`).
                    if let Some(u) = a.asked(ask) {
                        tts.say(u);
                    }
                    Vec::new()
                }
                None => break,
            },
            w = a.next() => a.on_wake(w, None),
        };
        handle(&mut a, acts, &tts, &mut saying);
        // A press while the last answer is still being heard records nothing: the next wake.
        if a.voice_answers() && !words.lock().unwrap().is_empty() && a.press() {
            a.record(&spoken());
            saying = words.lock().unwrap().pop_front();
            let acts = a.release();
            handle(&mut a, acts, &tts, &mut saying);
        }
    }
}

fn handle(a: &mut Approvals, acts: Vec<Act>, tts: &Tts, saying: &mut Option<String>) {
    for act in acts {
        match act {
            Act::Transcribe { call, .. } => {
                let text = saying.take().unwrap_or_default();
                a.push(
                    Op::Hear { call },
                    Box::pin(async move { Done::Heard(Ok(text)) }),
                );
            }
            Act::Speak { id, words } => {
                if let Some(u) = a.speak_again(&id, words) {
                    tts.say(u);
                }
            }
            other => tracing::warn!(?other, "accept: the voice did not expect this"),
        }
    }
}
