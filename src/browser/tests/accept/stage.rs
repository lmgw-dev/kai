//! What the acceptance runs' scripted model stands on: its calls through Kai's host (the path
//! lmgw's calls take, as Kai's own voice session's, or another caller's), the owner who answers
//! Kai's questions as the script says, at the dialog or by voice (`super::voice`), the speech that
//! hears Kai's announcements and questions without playing them, and the reading of Kai's answer
//! lines a model would do.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::launcher::Launcher;
use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::host::local::Local;
use crate::host::policy::Caller;
use crate::host::registry::Block;
use crate::voice::speak::{Kind, Mouth, Said as SpeechSaid, Spoken, Utterance};

/// lmgw's wait on a call of Kai's tools (`lmgw/timeout_ms`, the MCP row's): what the calls here
/// are bounded by.
pub const LMGW_TIMEOUT: Duration = Duration::from_secs(120);

/// One question Kai asked the owner while the script ran.
#[derive(Debug, Clone)]
pub struct Question {
    pub tool: String,
    pub caller: String,
    pub lines: Vec<String>,
    pub notes: Vec<String>,
    pub yes: bool,
    /// The dialog was left to the voice, which answered (`super::voice`).
    pub by_voice: bool,
}

impl Question {
    pub fn said(&self) -> String {
        let mut s = format!(
            "{} for {} ({})",
            self.tool,
            self.caller,
            match (self.by_voice, self.yes) {
                (true, _) => "left to the voice",
                (false, true) => "yes",
                (false, false) => "no",
            }
        );
        for l in self.lines.iter().chain(&self.notes) {
            s.push_str(&format!("\n      | {l}"));
        }
        s
    }
}

/// The owner at Kai's dialog (K15, PD4): answers yes to the questions the script expects, in
/// order, each by the start of the dialog's tool line, and no to every other. An expected question
/// written `voice:<tool line>` is left open at the dialog: the owner answers it by voice
/// (`super::voice`), and the dialog closes when the voice's answer went.
#[derive(Default)]
pub struct Owner {
    asked: Mutex<Vec<Question>>,
    expected: Mutex<VecDeque<String>>,
    /// The dialogs left to the voice, open until Kai closes them.
    open: Mutex<Vec<tokio::sync::oneshot::Sender<bool>>>,
}

impl Owner {
    /// The questions the next steps expect (module doc).
    pub fn expect(&self, tools: &[&str]) {
        *self.expected.lock().unwrap() = tools.iter().map(|t| (*t).to_owned()).collect();
    }

    /// The questions asked since the last look, and the expected ones that were not asked.
    pub fn asked(&self) -> (Vec<Question>, Vec<String>) {
        let asked = std::mem::take(&mut *self.asked.lock().unwrap());
        let missing = std::mem::take(&mut *self.expected.lock().unwrap());
        (asked, missing.into())
    }
}

impl Confirmer for Owner {
    fn ask(&self, req: ConfirmRequest) {
        let mut expected = self.expected.lock().unwrap();
        let front = expected.front().cloned().unwrap_or_default();
        let by_voice = front
            .strip_prefix("voice:")
            .is_some_and(|t| req.tool.starts_with(t));
        let yes = !by_voice && !front.is_empty() && req.tool.starts_with(front.as_str());
        if yes || by_voice {
            expected.pop_front();
        }
        drop(expected);
        self.asked.lock().unwrap().push(Question {
            tool: req.tool.clone(),
            caller: req.caller.clone(),
            lines: req.lines.clone(),
            notes: req.notes.clone(),
            yes: yes || by_voice,
            by_voice,
        });
        if by_voice {
            self.open.lock().unwrap().push(req.reply);
        } else {
            let _ = req.reply.send(yes);
        }
    }
}

/// One utterance as the speech handled it (an announcement, a notice, or one of Kai's own
/// questions: `Kind::Question`).
#[derive(Debug, Clone)]
pub struct Heard {
    pub kind: Kind,
    /// Kai's English words (the page's text inside them).
    pub text: String,
    /// The gate said Firefox had the focused window (`None`: no gate, a notice).
    pub focused: Option<bool>,
    /// Why the gate dropped it.
    pub dropped: Option<String>,
}

/// Kai's speech with a fake text-to-speech (the browser module's seam, `crate::voice::speak`'s
/// [`Mouth`]): each utterance's gate is run as the speech runs it when it makes and plays one, its
/// text kept, and it is answered as started and heard; nothing is made or played. The owner of a
/// hands-and-eyes setup talks with Kai in conversation mode, so Firefox without the focus does not
/// drop one (that is `voice::speak`'s rule with conversation mode on).
#[derive(Clone, Default)]
pub struct Tts(Arc<Mutex<Vec<Heard>>>);

impl Mouth for Tts {
    fn say(&self, u: Utterance) {
        let gate = u.gate.as_ref().map(|g| g());
        let (focused, dropped) = match &gate {
            None => (None, None),
            Some(Ok(f)) => (Some(*f), None),
            Some(Err(d)) => (None, Some(d.clone())),
        };
        if let Some(r) = &u.reply {
            match &dropped {
                None => {
                    let _ = r.send((u.tag, SpeechSaid::Started));
                    let _ = r.send((u.tag, SpeechSaid::Done(Spoken::Heard)));
                }
                Some(d) => {
                    let _ = r.send((u.tag, SpeechSaid::Done(Spoken::Dropped(d.clone()))));
                }
            }
        }
        self.0.lock().unwrap().push(Heard {
            kind: u.kind,
            text: u.text.en().to_owned(),
            focused,
            dropped: dropped.map(|d| d.to_string()),
        });
    }
}

impl Tts {
    pub fn all(&self) -> Vec<Heard> {
        self.0.lock().unwrap().clone()
    }

    /// Waits until an utterance holding every one of `parts` was heard; it.
    pub async fn heard(&self, parts: &[&str], within: Duration) -> Result<Heard, String> {
        let end = tokio::time::Instant::now() + within;
        loop {
            if let Some(h) = self
                .all()
                .into_iter()
                .find(|h| h.dropped.is_none() && parts.iter().all(|p| h.text.contains(p)))
            {
                return Ok(h);
            }
            if tokio::time::Instant::now() > end {
                return Err(format!(
                    "nothing holding {parts:?} was spoken within {within:?}; spoken: {:?}",
                    self.all().iter().map(|h| &h.text).collect::<Vec<_>>()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A tool's answer as the model reads it.
#[derive(Debug, Clone)]
pub struct Answer {
    pub error: bool,
    pub text: String,
}

/// One call the scripted model made, for the report.
#[derive(Debug, Clone)]
pub struct Made {
    pub tool: String,
    pub args: Value,
    pub answer: Answer,
}

/// A step's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
}

#[derive(Debug, Clone)]
pub struct Step {
    pub run: &'static str,
    pub name: &'static str,
    pub outcome: Outcome,
    pub calls: Vec<Made>,
    pub questions: Vec<Question>,
    pub heard: Vec<Heard>,
}

/// The scripted model's place: Kai's host, the owner, the speech, the launcher and the report.
pub struct Stage {
    pub local: Local,
    pub owner: Arc<Owner>,
    pub tts: Tts,
    pub launcher: Launcher,
    /// The voice Kai's own questions reach (`super::voice`).
    pub ears: super::voice::Ears,
    /// Who the next calls are (default: Kai's own voice session).
    pub caller: Caller,
    /// The fixture site's base URLs: its own origin and the other one (localhost).
    pub site: String,
    pub other: String,
    /// The run folder (Kai's side): the socket, the file the form gets, the report.
    pub dir: PathBuf,
    pub steps: Vec<Step>,
    calls: Vec<Made>,
    heard_before: usize,
}

impl Stage {
    pub fn new(
        local: Local,
        owner: Arc<Owner>,
        tts: Tts,
        ears: super::voice::Ears,
        launcher: Launcher,
        dir: PathBuf,
    ) -> Self {
        let s = |k: &str| {
            launcher.ready[k]
                .as_str()
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_owned()
        };
        let (site, other) = (s("site"), s("other"));
        Self {
            local,
            owner,
            tts,
            ears,
            caller: Caller::Own,
            launcher,
            site,
            other,
            dir,
            steps: Vec::new(),
            calls: Vec::new(),
            heard_before: 0,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.site, path.trim_start_matches('/'))
    }

    /// One of the model's tool calls, as Kai's own voice session's call through lmgw (or
    /// [`Stage::caller`]'s): the host's dispatch (K15's policy, the tool's preparation, the owner's
    /// confirmation, the handler) with lmgw's deadline.
    pub async fn call(&mut self, tool: &str, args: Value) -> Answer {
        let answer = match self
            .local
            .entries()
            .into_iter()
            .find(|e| e.spec.name == tool)
        {
            None => Answer {
                error: true,
                text: format!("{tool} is not listed"),
            },
            Some(entry) => {
                let raw = args.as_object().cloned();
                let call = self
                    .local
                    .call_timed(&entry, raw, &self.caller, LMGW_TIMEOUT);
                match tokio::time::timeout(LMGW_TIMEOUT + Duration::from_secs(10), call).await {
                    Err(_) => Answer {
                        error: true,
                        text: format!("{tool} did not answer within lmgw's wait"),
                    },
                    Ok(r) => Answer {
                        error: r.error,
                        text: r
                            .blocks
                            .iter()
                            .map(|b| match b {
                                Block::Text(t) => t.clone(),
                                Block::Image { .. } => "(an image)".into(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                }
            }
        };
        self.calls.push(Made {
            tool: tool.to_owned(),
            args,
            answer: answer.clone(),
        });
        answer
    }

    /// A call whose answer must not be an error.
    pub async fn ok(&mut self, tool: &str, args: Value) -> Result<String, String> {
        let a = self.call(tool, args).await;
        if a.error {
            Err(format!("{tool} failed: {}", a.text))
        } else {
            Ok(a.text)
        }
    }

    /// Records a step: its calls, the owner's questions and what was spoken since the last one.
    /// Questions the step expected and did not get fail it, and so do ones it did not expect.
    pub fn record(&mut self, run: &'static str, name: &'static str, r: Result<Outcome, String>) {
        let (questions, missing) = self.owner.asked();
        let all = self.tts.all();
        let heard = all[self.heard_before.min(all.len())..].to_vec();
        self.heard_before = all.len();
        let mut outcome = match r {
            Ok(o) => o,
            Err(why) => Outcome::Fail(why),
        };
        let unsaid = self.ears.unsaid();
        if outcome == Outcome::Pass {
            if !missing.is_empty() {
                outcome = Outcome::Fail(format!("Kai did not ask the owner {missing:?}"));
            } else if let Some(q) = questions.iter().find(|q| !q.yes) {
                outcome = Outcome::Fail(format!("Kai asked the owner unexpectedly: {}", q.said()));
            } else if !unsaid.is_empty() {
                outcome = Outcome::Fail(format!(
                    "the owner's spoken answers were never asked for: {unsaid:?}"
                ));
            }
        }
        self.steps.push(Step {
            run,
            name,
            outcome,
            calls: std::mem::take(&mut self.calls),
            questions,
            heard,
        });
    }

    /// Every tool the model called in the whole run.
    pub fn tools_called(&self) -> Vec<String> {
        self.steps
            .iter()
            .flat_map(|s| s.calls.iter().map(|c| c.tool.clone()))
            .collect()
    }
}

/// One element line of an answer: `<ref> <role> "<name>"` and what follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub r: i64,
    pub role: String,
    pub name: String,
    /// The whole line.
    pub line: String,
}

/// The element lines of `text`, in order (Kai's compact lines: the list marks, a heading's `#`s
/// and indentation before the ref skipped).
pub fn lines(text: &str) -> Vec<Line> {
    text.lines().filter_map(line).collect()
}

fn line(l: &str) -> Option<Line> {
    let t = l.trim_start().trim_start_matches(['-', '#', '*', ' ', '>']);
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    let r: i64 = digits.parse().ok()?;
    let rest = t[digits.len()..].strip_prefix(' ')?;
    // A role is one word, a heading's level after it.
    let q = rest.find(" \"")?;
    let role = rest[..q].to_owned();
    let mut words = role.split(' ');
    let word = words.next()?;
    let level = words.next();
    if word.is_empty()
        || !word.chars().all(|c| c.is_ascii_lowercase())
        || level.is_some_and(|l| !l.chars().all(|c| c.is_ascii_digit()))
        || words.next().is_some()
    {
        return None;
    }
    let name = quoted(&rest[q + 1..])?;
    Some(Line {
        r,
        role,
        name,
        line: l.to_owned(),
    })
}

/// The JSON-quoted string `s` starts with.
fn quoted(s: &str) -> Option<String> {
    let mut escaped = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return serde_json::from_str(&s[..=i]).ok(),
            _ => escaped = false,
        }
    }
    None
}

/// The first element of `text` whose role starts with `role` and whose name `name` matches.
pub fn find(text: &str, role: &str, name: impl Fn(&str) -> bool) -> Result<Line, String> {
    lines(text)
        .into_iter()
        .find(|l| l.role.starts_with(role) && name(&l.name))
        .ok_or_else(|| format!("no {role} like that in:\n{text}"))
}

/// The element named exactly `name`, of `role`.
pub fn named(text: &str, role: &str, name: &str) -> Result<Line, String> {
    find(text, role, |n| n == name)
}

/// `text` holds every one of `parts`, else why not.
pub fn holds(what: &str, text: &str, parts: &[&str]) -> Result<(), String> {
    match parts.iter().find(|p| !text.contains(**p)) {
        None => Ok(()),
        Some(p) => Err(format!("{what} does not say {p:?}:\n{text}")),
    }
}

#[cfg(test)]
mod reading {
    use super::*;

    #[test]
    fn element_lines_are_read_as_a_model_reads_them() {
        let text = "\"Shop\" http://x/ (tab 3)\n12 textbox \"Full name\" = \"Ada\"\n  - 14 link \
                    \"Say \\\"hi\\\"\" http://x/a\n## 15 heading 2 \"Headlamp Pro\"\nnot a line\n1 match \
                    for \"Battery\", highlighted:";
        let got = lines(text);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!((got[0].r, got[0].role.as_str()), (12, "textbox"));
        assert_eq!(got[1].name, "Say \"hi\"");
        assert_eq!((got[2].r, got[2].role.as_str()), (15, "heading 2"));
    }
}
