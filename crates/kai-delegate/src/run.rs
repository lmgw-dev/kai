//! The loop (crate doc).

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::def::{Def, Settings, Task};
use crate::message::{Message, Part, Request, ToolCall};
use crate::plan::{self, PLAN_TOOL, Plan};
use crate::prompt::{self, Parts, Read};
use crate::{Gate, Model, Owner, Tools};

/// The task was cancelled (the caller's token): nothing more ran after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// How the task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Partial,
    Failed,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Done => "done",
            Status::Partial => "partial",
            Status::Failed => "failed",
        }
    }
}

/// The delegate's answer for the main agent (K25): short, never its transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: Status,
    /// What the delegate said (its status word taken off), or why the loop stopped.
    pub text: String,
    /// The tools its calls ran with their targets (`read_window (Konsole)`), in the order of first
    /// use, with how often.
    pub used: Vec<(String, u32)>,
    /// The model turns it took.
    pub turns: u32,
}

impl Answer {
    /// `done: <text>`, and a line naming the tools used, if any.
    pub fn render(&self) -> String {
        let mut out = format!("{}: {}", self.status.word(), self.text);
        if !self.used.is_empty() {
            let used: Vec<String> = self
                .used
                .iter()
                .map(|(t, n)| match n {
                    1 => t.clone(),
                    n => format!("{t} ×{n}"),
                })
                .collect();
            out.push_str(&format!("\n(used: {})", used.join(", ")));
        }
        out
    }
}

/// Waits for `fut` unless `cancel` fires first.
async fn unless<F: Future>(cancel: &CancellationToken, fut: F) -> Result<F::Output, Cancelled> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(Cancelled),
        out = fut => Ok(out),
    }
}

/// Runs one task of `def` to its answer (crate doc). `Err(Cancelled)` once `cancel` fired: the
/// pending model request, tool call or confirmation is dropped with it.
#[allow(clippy::too_many_arguments)]
pub async fn run<M: Model, T: Tools, O: Owner>(
    def: &Def,
    task: &Task,
    model: &M,
    tools: &T,
    owner: &O,
    settings: &Settings,
    cancel: &CancellationToken,
) -> Result<Answer, Cancelled> {
    let prefix = prompt::prefix(def, tools.defs(def.tools));
    let mut reads = Vec::new();
    for c in def.context {
        let mut args = Map::new();
        for (from, to) in c.pass {
            if let Some((_, v)) = task.params.iter().find(|(n, _)| n == from) {
                args.insert((*to).to_owned(), Value::String(v.clone()));
            }
        }
        let read = match tools.check(c.tool, &Value::Object(args)) {
            Err(why) => Read::NotRun(why),
            Ok(step) => match &step.gate {
                Gate::Allow => {
                    let out = unless(cancel, tools.call(&step, cancel)).await?;
                    Read::Ran {
                        parts: out.parts,
                        error: out.error,
                    }
                }
                Gate::Confirm => Read::NotRun(format!(
                    "{} needs the owner's confirmation; propose it in a plan to read it",
                    c.tool
                )),
                Gate::Deny(why) => Read::NotRun(why.clone()),
            },
        };
        reads.push((c.heading, read));
    }
    let mut lp = Loop {
        def,
        tools,
        owner,
        cancel,
        plan: Plan::None,
        answered: false,
        used: Vec::new(),
    };
    let mut messages = vec![prompt::task_message(def, task, reads)];
    let mut turns = 0u32;
    loop {
        if settings.max_turns > 0 && turns >= settings.max_turns {
            return Ok(lp.answer(
                Status::Partial,
                format!(
                    "stopped after {turns} model turns ({} = {}) before the delegate answered",
                    settings.max_turns_key, settings.max_turns
                ),
                turns,
            ));
        }
        let request = Request {
            prefix: &prefix,
            messages: &messages,
        };
        let turn = unless(cancel, model.complete(&request, cancel)).await?;
        turns += 1;
        let turn = match turn {
            Ok(t) => t,
            Err(why) => {
                return Ok(lp.answer(
                    Status::Failed,
                    format!("the delegate's model did not answer: {why}"),
                    turns,
                ));
            }
        };
        if turn.calls.is_empty() {
            let (status, text) = final_text(turn.text.as_deref().unwrap_or_default());
            return Ok(lp.answer(status, text, turns));
        }
        messages.push(Message::Assistant {
            text: turn.text.clone(),
            calls: turn.calls.clone(),
        });
        let mut images = Parts::default();
        for call in &turn.calls {
            let (text, got) = match lp.one(call).await? {
                Handled::Result(text, got) => (text, got),
                Handled::Unasked => {
                    return Ok(lp.answer(
                        Status::Failed,
                        "Kai could not ask the owner to confirm the plan (its dialog cannot be \
                         shown); nothing of it ran"
                            .into(),
                        turns,
                    ));
                }
                Handled::Declined(steps) => {
                    return Ok(lp.answer(
                        Status::Failed,
                        format!(
                            "the owner declined the plan ({}); nothing of it ran",
                            steps.join(", ")
                        ),
                        turns,
                    ));
                }
            };
            messages.push(Message::Tool {
                call_id: call.id.clone(),
                text,
            });
            if !got.is_empty() {
                images.text(format!(
                    "The image{} {} returned (call {}):",
                    if got.len() == 1 { "" } else { "s" },
                    call.name,
                    call.id
                ));
                for p in got {
                    images.push(p);
                }
            }
        }
        // OpenAI's tool message carries text only: a tool's images follow as a user message.
        if !images.0.is_empty() {
            messages.push(Message::User(images.0));
        }
    }
}

/// The model's final text: its status word taken off (none: done, as it answered without a call).
fn final_text(text: &str) -> (Status, String) {
    let text = text.trim();
    if text.is_empty() {
        return (
            Status::Failed,
            "the delegate's model answered nothing".into(),
        );
    }
    for status in [Status::Done, Status::Partial, Status::Failed] {
        let word = status.word();
        let Some(head) = text.get(..word.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(word) {
            continue;
        }
        let rest = &text[word.len()..];
        if rest.is_empty() || rest.starts_with([':', ' ', '\n', '.', ',', '-']) {
            let rest = rest
                .trim_start_matches(|c: char| {
                    matches!(c, ':' | '.' | ',' | '-') || c.is_whitespace()
                })
                .trim_end();
            let rest = if rest.is_empty() {
                "(nothing more said)"
            } else {
                rest
            };
            return (status, rest.to_owned());
        }
    }
    (Status::Done, text.to_owned())
}

/// What one call came to.
enum Handled {
    /// The tool message's text, and the images for the following user message.
    Result(String, Vec<Part>),
    /// The owner declined the plan: its steps, shown.
    Declined(Vec<String>),
    /// The plan's question could not be shown: the owner was not asked.
    Unasked,
}

struct Loop<'a, T, O> {
    def: &'a Def,
    tools: &'a T,
    owner: &'a O,
    cancel: &'a CancellationToken,
    plan: Plan,
    /// The owner answered a plan of this task (once per task).
    answered: bool,
    used: Vec<(String, u32)>,
}

fn arguments(call: &ToolCall) -> Result<Value, String> {
    if call.arguments.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&call.arguments)
        .map_err(|e| format!("the arguments of {} are not JSON: {e}", call.name))
}

fn said(text: String) -> Handled {
    Handled::Result(text, Vec::new())
}

impl<T: Tools, O: Owner> Loop<'_, T, O> {
    fn answer(&self, status: Status, text: String, turns: u32) -> Answer {
        Answer {
            status,
            text,
            used: self.used.clone(),
            turns,
        }
    }

    async fn one(&mut self, call: &ToolCall) -> Result<Handled, Cancelled> {
        let args = match arguments(call) {
            Ok(a) => a,
            Err(e) => return Ok(said(format!("error: {e}"))),
        };
        if call.name == PLAN_TOOL {
            return self.propose(&args).await;
        }
        let step = match self.tools.check(&call.name, &args) {
            Ok(s) => s,
            Err(e) => return Ok(said(format!("error: {e}"))),
        };
        match &step.gate {
            Gate::Deny(why) => return Ok(said(format!("error: refused: {why}"))),
            Gate::Allow => {}
            Gate::Confirm if self.plan.take(&step) => {}
            Gate::Confirm if self.answered => {
                return Ok(said(format!(
                    "error: this call of {} is not a step of the confirmed plan (or ran already), \
                     and the owner confirms one plan per task; it did not run. Finish with what \
                     you have.",
                    step.tool
                )));
            }
            Gate::Confirm => {
                return Ok(said(format!(
                    "error: {} needs the owner's confirmation; it did not run. Propose it with \
                     {PLAN_TOOL}, together with every other call that needs one.",
                    step.tool
                )));
            }
        }
        let out = unless(self.cancel, self.tools.call(&step, self.cancel)).await?;
        let label = if step.target.is_empty() {
            step.tool.clone()
        } else {
            format!("{} ({})", step.tool, step.target)
        };
        match self.used.iter_mut().find(|(t, _)| *t == label) {
            Some((_, n)) => *n += 1,
            None => self.used.push((label, 1)),
        }
        let mut text = Vec::new();
        let mut images = Vec::new();
        for p in out.parts {
            match p {
                Part::Text(t) => text.push(t),
                image => images.push(image),
            }
        }
        let mut text = text.join("\n");
        if !images.is_empty() {
            let n = images.len();
            let line = if n == 1 {
                "(its image follows in the next message)".to_owned()
            } else {
                format!("({n} images follow in the next message)")
            };
            text = if text.is_empty() {
                line
            } else {
                format!("{text}\n{line}")
            };
        }
        if out.error {
            text = format!("error: {text}");
        }
        Ok(Handled::Result(text, images))
    }

    async fn propose(&mut self, args: &Value) -> Result<Handled, Cancelled> {
        if self.answered {
            return Ok(said(format!(
                "error: the owner answered this task's plan already, and confirms one plan per \
                 task; {PLAN_TOOL} was not shown. Finish with what you have."
            )));
        }
        let proposed = match plan::proposed(args) {
            Ok(p) => p,
            Err(e) => return Ok(said(format!("error: {e}"))),
        };
        let mut asks = Vec::new();
        for (i, (tool, a)) in proposed.iter().enumerate() {
            if tool == PLAN_TOOL {
                return Ok(said(format!(
                    "error: step {}: {PLAN_TOOL} is no step; the plan was not shown",
                    i + 1
                )));
            }
            let step = match self.tools.check(tool, a) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(said(format!(
                        "error: step {}: {e}; the plan was not shown to the owner",
                        i + 1
                    )));
                }
            };
            match &step.gate {
                Gate::Deny(why) => {
                    return Ok(said(format!(
                        "error: step {} ({tool}) is refused: {why}; the plan was not shown to the \
                         owner. Propose it again without that step.",
                        i + 1
                    )));
                }
                Gate::Allow => {}
                Gate::Confirm => asks.push(step),
            }
        }
        if asks.is_empty() {
            return Ok(said(
                "No step of this plan needs the owner's confirmation: make the calls.".into(),
            ));
        }
        self.answered = true;
        match unless(self.cancel, self.owner.confirm_plan(self.def.name, &asks)).await? {
            Some(true) => {}
            Some(false) => {
                return Ok(Handled::Declined(
                    asks.iter().map(|s| s.tool.clone()).collect(),
                ));
            }
            None => return Ok(Handled::Unasked),
        }
        self.plan = Plan::Confirmed(asks.into_iter().map(|s| (s, false)).collect());
        Ok(said(
            "The owner confirmed the plan: make its calls as proposed, each once.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_word_is_read_off_the_final_text() {
        for (text, status, rest) in [
            ("done: it is 14:02", Status::Done, "it is 14:02"),
            ("Partial - two of three", Status::Partial, "two of three"),
            ("FAILED: no window", Status::Failed, "no window"),
            ("failed", Status::Failed, "(nothing more said)"),
            (
                "The clock says 14:02.",
                Status::Done,
                "The clock says 14:02.",
            ),
            ("doneness is relative", Status::Done, "doneness is relative"),
            (
                "  ",
                Status::Failed,
                "the delegate's model answered nothing",
            ),
            ("ü", Status::Done, "ü"),
        ] {
            assert_eq!(final_text(text), (status, rest.to_owned()), "{text}");
        }
    }

    #[test]
    fn the_answer_is_short_and_names_the_tools_used() {
        let a = Answer {
            status: Status::Partial,
            text: "read the editor, not the terminal".into(),
            used: vec![("see_window".into(), 1), ("read_window".into(), 2)],
            turns: 3,
        };
        assert_eq!(
            a.render(),
            "partial: read the editor, not the terminal\n(used: see_window, read_window ×2)"
        );
        let a = Answer { used: vec![], ..a };
        assert_eq!(a.render(), "partial: read the editor, not the terminal");
    }
}
