//! The prompt's two parts: the static one ([`prefix`]) and the task message, the start of the
//! dynamic one ([`task_message`]).

use crate::def::{Def, Task, Utterance};
use crate::message::{Message, Part, Prefix, ToolDef};
use crate::plan;

/// What every delegate is told after its own instructions: the task's frame, the answer's form, the
/// plan rule.
const FRAME: &str = "You do one task for Kai's main assistant, then answer it. You see this task \
only, not the conversation it came from. Use the tools to look before you act; never guess what a \
tool can tell you.

When you are done, reply without a tool call, in one to three short sentences that start with one \
word:
- done: the task is done; give the answer, or say what you did;
- partial: part of it is done; say what is done and what is not;
- failed: it could not be done; say why.

Some calls need the owner's confirmation; Kai tells you which. Such a call runs only as a step of a \
plan: call propose_plan once with every such call (its tool and arguments) before making them, and \
the owner confirms the whole plan once. Calls that need no confirmation do not belong in a plan. \
There is one plan per task.";

/// The static part for `def` with the tools the client offers it (`tools`, from
/// [`crate::Tools::defs`]) and the plan tool last. Depends on nothing else, so it is
/// byte-identical across calls while the offer stays.
pub fn prefix(def: &Def, tools: Vec<ToolDef>) -> Prefix {
    let mut system = format!(
        "You are Kai's {} delegate. {}\n\n{FRAME}",
        def.name, def.instructions
    );
    if !def.examples.is_empty() {
        system.push_str("\n\nExamples:");
        for e in def.examples {
            system.push_str("\n\n");
            system.push_str(e);
        }
    }
    let mut tools = tools;
    tools.push(plan::def());
    Prefix { system, tools }
}

/// What a context step brought: its result, or why it did not run.
pub(crate) enum Read {
    Ran { parts: Vec<Part>, error: bool },
    NotRun(String),
}

/// The task message, the dynamic part's start: the reads, the parameters, the user's last
/// utterance verbatim at the very end.
pub(crate) fn task_message(def: &Def, task: &Task, reads: Vec<(&str, Read)>) -> Message {
    let mut parts = Parts::default();
    if !reads.is_empty() {
        parts.text("Read for this task just now:".into());
        for (heading, read) in reads {
            match read {
                Read::Ran { parts: got, error } => {
                    parts.text(if error {
                        format!("\n## {heading} (the read failed)")
                    } else {
                        format!("\n## {heading}")
                    });
                    for p in got {
                        parts.push(p);
                    }
                }
                Read::NotRun(why) => parts.text(format!("\n## {heading}\n(not read: {why})")),
            }
        }
        parts.text(String::new());
    }
    parts.text(format!("Task ({}):", def.name));
    for (name, value) in &task.params {
        parts.text(format!("{name}: {value}"));
    }
    match &task.utterance {
        Utterance::Heard(words) => {
            parts.text("\nThe user's last words, verbatim:".into());
            parts.text(words.clone());
        }
        Utterance::Unknown(why) => {
            parts.text(format!("\nThe user's words are not known here ({why})."));
        }
    }
    Message::User(parts.0)
}

/// Parts with adjacent texts joined by line breaks.
#[derive(Default)]
pub(crate) struct Parts(pub Vec<Part>);

impl Parts {
    pub fn text(&mut self, t: String) {
        if let Some(Part::Text(last)) = self.0.last_mut() {
            last.push('\n');
            last.push_str(&t);
        } else {
            self.0.push(Part::Text(t));
        }
    }

    pub fn push(&mut self, p: Part) {
        match p {
            Part::Text(t) => self.text(t),
            image => self.0.push(image),
        }
    }
}
