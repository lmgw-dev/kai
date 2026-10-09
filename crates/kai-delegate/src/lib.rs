//! K25's delegate loop (ruling 21), client-neutral: a domain tool with one plain-language parameter
//! (`look {question, window?}`, `find {what}`) becomes a task-tuned prompt for a second model, which
//! works with the client's fine-grained tools and answers the main agent in a few words.
//!
//! **The context** ([`Request`]):
//! - a **static part** first ([`Prefix`]): the delegate's instructions and examples, the answer's
//!   form, the plan rule, and its tools. It depends on the delegate and the tools the client offers
//!   it, nothing else, so it is byte-identical across calls and the model server's prompt cache in
//!   the delegate's slot holds it;
//! - a **dynamic part** last ([`run`]): what the client read for the task just now (the window
//!   list, a screenshot: [`Def::context`]), the call's parameters, and the user's last utterance
//!   verbatim, at the very end. Then the loop's own turns.
//!
//! The delegate never sees the main conversation: nothing of it is an input here. A screenshot it
//! takes stays in its own requests; the main agent gets [`Answer::render`]'s text only.
//!
//! **Its tools** are the client's ([`Tools`]); each call is checked by the client's policy
//! ([`Gate`]) as a call by whoever called the delegate. A call the policy wants confirmed runs only
//! as a step of **a plan** the delegate proposes ([`PLAN_TOOL`]) and the owner confirms once
//! ([`Owner`]), not each step. One plan per task: a second one is refused without asking.
//!
//! **The answer** ([`Answer`]): done, partial or failed, with what the delegate said it did and the
//! tools it used. Every stop that is not the model's own says why: the model's error, a declined
//! plan, the step setting ([`Settings::max_turns`]).
//!
//! **Sync and async** (K26): [`run`] is one task's whole loop; the client decides whether its tool
//! answers when the loop ends (`look`, `find`) or answers "started" at once and delivers the loop's
//! answer later (`arrange_windows`, `operate_app`, once the client has asynchronous jobs). Nothing
//! here changes for that.
//!
//! No I/O and no runtime: the model, the tools and the owner's confirmation are traits; the caller's
//! cancellation ([`CancellationToken`]) ends the loop at any wait, and the pending model request,
//! tool call or confirmation with it.

mod def;
mod message;
mod plan;
mod prompt;
mod run;

pub use def::{ContextStep, Def, Settings, Task, Utterance};
pub use message::{Message, Part, Prefix, Request, ToolCall, ToolDef, Turn};
pub use plan::PLAN_TOOL;
pub use prompt::prefix;
pub use run::{Answer, Cancelled, Status, run};
pub use tokio_util::sync::CancellationToken;

use serde_json::{Map, Value};

/// The delegate's model: one completion of [`Request`] (the client's chat endpoint under its key).
pub trait Model: Send + Sync {
    /// The model's next turn. The future is dropped when the task is cancelled; `cancel` is the
    /// same signal for work the client spawned. An `Err` is the reason in words (the gateway's
    /// error, a refusal), said in the failed answer.
    fn complete(
        &self,
        request: &Request<'_>,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Turn, String>> + Send;
}

/// What the client's policy says about one call (K15, by the delegate's caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    /// Runs only as a step of the confirmed plan.
    Confirm,
    /// Refused; the reason goes to the model as the call's error.
    Deny(String),
}

/// One checked call: the tool, its arguments as the client reads them (an omitted optional one
/// left out, so a plan's step and the later call compare equal), the policy's word, and what the
/// owner sees of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tool: String,
    pub args: Map<String, Value>,
    pub gate: Gate,
    /// The call for the owner's confirmation: the tool and its arguments in compact lines.
    pub shown: String,
    /// The tool's annotations call it destructive.
    pub destructive: bool,
    /// What the call is aimed at, in a few words (the window, the query): the answer's used line
    /// names it beside the tool. Empty: the tool alone.
    pub target: String,
}

/// A tool's result: text and image parts, and whether it is an error the model reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub parts: Vec<Part>,
    pub error: bool,
}

impl Outcome {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![Part::Text(text.into())],
            error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            parts: vec![Part::Text(text.into())],
            error: true,
        }
    }
}

/// The client's tools, as the delegate may use them.
pub trait Tools: Send + Sync {
    /// The definitions of those of `names` the client offers now, in `names`' order (the static
    /// part's list: it must be the same for the same offer).
    fn defs(&self, names: &[&str]) -> Vec<ToolDef>;

    /// `tool` called with `arguments` (the model's JSON object; `Value::Null` for none): checked
    /// against the tool's parameters and the policy. `Err` is the reason the call is malformed (no
    /// such tool for this delegate, an argument that does not fit), returned to the model.
    fn check(&self, tool: &str, arguments: &Value) -> Result<Step, String>;

    /// Runs a step [`Tools::check`] made, whose gate the loop has passed. Dropped when the task is
    /// cancelled; `cancel` is the same signal for work the client spawned.
    fn call(&self, step: &Step, cancel: &CancellationToken)
    -> impl Future<Output = Outcome> + Send;
}

/// The owner, asked once per task (K25).
pub trait Owner: Send + Sync {
    /// Whether the owner confirms the plan `steps` of the delegate `delegate`: `Some(false)` for a
    /// refusal, `None` for a question that could not be shown (the owner was not asked). Dropped
    /// when the task is cancelled (a dialog closes then).
    fn confirm_plan(
        &self,
        delegate: &str,
        steps: &[Step],
    ) -> impl Future<Output = Option<bool>> + Send;
}
