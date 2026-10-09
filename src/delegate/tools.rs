//! The delegate's tools: the desktop module's entries (WP14 to WP16), called in process, not
//! through lmgw, each call by the caller that called the delegate (K15: its policy applies as if
//! that caller made the call itself, with no approval of its own: F approved the delegate, not
//! its steps), and its plan confirmed in PD4's dialog.
//!
//! The log names the delegate, the tool, the caller and what happened, never an argument or a
//! result (as the host's does).

use std::sync::Arc;
use std::time::Instant;

use kai_delegate::{CancellationToken, Gate, Outcome, Part, Step, ToolDef};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::host::policy::{Caller, Decision, SharedPolicy};
use crate::host::registry::{Args, Block, Call, Entry, Origin, Reply};

/// The tools of one delegate task.
pub struct KaiTools {
    /// The delegate's name, for the log and the errors.
    pub delegate: &'static str,
    /// The desktop's entries the policy lists (not `off`), whichever the delegate may use.
    pub entries: Vec<Entry>,
    /// The host's live policy: read at each check.
    pub policy: SharedPolicy,
    pub caller: Caller,
    /// The delegate's call's deadline and `lmgw/timeout_ms`, handed to each of its calls (lmgw's
    /// cancel of the delegate's call cancels them too).
    pub deadline: Option<tokio::time::Instant>,
    pub timeout_ms: Option<u64>,
}

impl KaiTools {
    fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.spec.name == name)
    }
}

/// A reply's blocks as the loop's parts.
pub fn outcome(reply: Reply) -> Outcome {
    Outcome {
        parts: reply
            .blocks
            .into_iter()
            .map(|b| match b {
                Block::Text(t) => Part::Text(t),
                Block::Image { mime, base64 } => Part::Image { mime, base64 },
            })
            .collect(),
        error: reply.error,
    }
}

impl kai_delegate::Tools for KaiTools {
    fn defs(&self, names: &[&str]) -> Vec<ToolDef> {
        names
            .iter()
            .filter_map(|n| self.entry(n))
            .map(|e| ToolDef {
                name: e.spec.name.into(),
                description: e.spec.description.into(),
                parameters: Value::Object(e.spec.input_schema()),
            })
            .collect()
    }

    fn check(&self, tool: &str, arguments: &Value) -> Result<Step, String> {
        let Some(e) = self.entry(tool) else {
            return Err(format!(
                "the {} delegate has no tool '{tool}'",
                self.delegate
            ));
        };
        let raw = match arguments {
            Value::Null => None,
            Value::Object(m) => Some(m.clone()),
            _ => return Err(format!("{tool}: the arguments must be an object")),
        };
        let args = Args::read(e.spec, raw.clone())?;
        // The arguments as `Args::read` keeps them (a `null` is an omitted one), so a plan's step
        // and the later call compare equal.
        let kept: Map<String, Value> = raw
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, v)| !v.is_null())
            .collect();
        let decision =
            self.policy
                .read()
                .expect("never poisoned")
                .decide(e.spec, &self.caller, None);
        let gate = match decision {
            Decision::Allow => Gate::Allow,
            Decision::Confirm { .. } => Gate::Confirm,
            Decision::Deny(why) => Gate::Deny(why),
        };
        let lines = args.lines(e.spec);
        // The values only: `read_window (Konsole)` in the answer's used line.
        let target = lines
            .iter()
            .map(|l| l.split_once(": ").map_or(l.as_str(), |(_, v)| v))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Step {
            target,
            tool: tool.to_owned(),
            shown: if lines.is_empty() {
                tool.to_owned()
            } else {
                format!("{tool} ({})", lines.join(", "))
            },
            args: kept,
            gate,
            destructive: e.spec.hints.destructive,
        })
    }

    async fn call(&self, step: &Step, cancel: &CancellationToken) -> Outcome {
        let Some(e) = self.entry(&step.tool) else {
            return Outcome::error(format!("no tool '{}'", step.tool));
        };
        let args = match Args::read(e.spec, Some(step.args.clone())) {
            Ok(a) => a,
            Err(why) => return Outcome::error(why),
        };
        // A tool that resolves before its confirmation (`open`'s application) resolves here, at the
        // call; a delegate's plan shows its arguments only (the delegates that use such a tool come
        // with WP18).
        let prepared = match &e.prepare {
            None => None,
            Some(prepare) => match prepare(&args, Origin::default()).await {
                Ok(p) => Some(p),
                Err(why) => return Outcome::error(why),
            },
        };
        let started = Instant::now();
        let reply = (e.handler)(Call {
            deadline: self.deadline,
            timeout_ms: self.timeout_ms,
            // The result goes to the delegate's model, not over the link: no message limit.
            room: None,
            room_says: String::new(),
            prepared,
            ..Call::new(e.spec, args, self.caller.clone(), cancel.clone())
        })
        .await;
        tracing::info!(
            delegate = self.delegate,
            tool = e.spec.name,
            caller = %self.caller.describe(),
            error = reply.error,
            ms = started.elapsed().as_millis() as u64,
            "delegate: a call answered"
        );
        outcome(reply)
    }
}

/// The owner's confirmation of a plan: PD4's dialog, the steps as its argument lines, the
/// delegate's task and model in its description, and the time left of the delegate's call as the
/// host's dialog shows it. lmgw's cancel of the delegate's call drops the question, which closes
/// the dialog.
pub struct PlanDialog {
    pub confirmer: Arc<dyn Confirmer>,
    pub caller: Caller,
    /// The delegate's task line (`look`'s question, `find`'s description).
    pub task: String,
    /// The alias the delegate thinks with.
    pub alias: String,
    /// The delegate's call's deadline (`lmgw/timeout_ms`).
    pub deadline: Option<tokio::time::Instant>,
}

impl kai_delegate::Owner for PlanDialog {
    async fn confirm_plan(&self, delegate: &str, steps: &[Step]) -> Option<bool> {
        let (reply, answer) = oneshot::channel();
        tracing::info!(
            delegate,
            steps = steps.len(),
            caller = %self.caller.describe(),
            "delegate: asking the owner to confirm a plan (PD4)"
        );
        self.confirmer.ask(ConfirmRequest {
            tool: format!("{delegate} (a plan)"),
            description: if self.task.is_empty() {
                format!(
                    "Kai's {delegate} delegate (model {}) asks to make these calls, each once:",
                    self.alias
                )
            } else {
                format!(
                    "Kai's {delegate} delegate (model {}) asks to make these calls, each once, \
                     for: {}",
                    self.alias, self.task
                )
            },
            caller: self.caller.describe(),
            lines: steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {}", i + 1, s.shown))
                .collect(),
            destructive: steps.iter().any(|s| s.destructive),
            deadline: self.deadline.map(|d| {
                std::time::Instant::now() + d.saturating_duration_since(tokio::time::Instant::now())
            }),
            options: Vec::new(),
            notes: Vec::new(),
            reply,
        });
        // A dropped reply is a dialog that could not be shown: the owner was not asked.
        let answered = answer.await.ok();
        match answered {
            Some(yes) => tracing::info!(
                delegate,
                confirmed = yes,
                "delegate: the owner answered the plan"
            ),
            None => tracing::warn!(
                delegate,
                "delegate: the plan's confirmation could not be shown; nothing of it ran"
            ),
        }
        answered
    }
}
