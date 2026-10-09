//! The plan (K25): the calls a delegate's task needs the owner's confirmation for, proposed at once
//! and confirmed once.
//!
//! - The delegate calls [`PLAN_TOOL`] with its steps, each a tool and its arguments.
//! - Each step is checked as a call would be ([`crate::Tools::check`]). A malformed or refused step
//!   sends the plan back unasked, naming it; a plan with no step that needs a confirmation is
//!   answered "go ahead" unasked. Neither counts as the task's plan.
//! - Otherwise the owner is asked once ([`crate::Owner`]). Confirmed, each confirmed step runs once
//!   when the delegate calls it with the same arguments; a call that needs a confirmation and
//!   matches no unspent step is refused. Declined, the task ends failed.
//! - One plan per task: a second proposal after the owner answered is refused unasked.

use serde_json::{Value, json};

use crate::Step;
use crate::message::ToolDef;

/// The plan tool's name in the delegate's tool list.
pub const PLAN_TOOL: &str = "propose_plan";

pub(crate) fn def() -> ToolDef {
    ToolDef {
        name: PLAN_TOOL.into(),
        description: "Propose every call that needs the owner's confirmation, at once; the owner \
                      confirms the plan once."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string"},
                            "arguments": {"type": "object"}
                        },
                        "required": ["tool"]
                    }
                }
            },
            "required": ["steps"],
            "additionalProperties": false
        }),
    }
}

/// The proposal's steps as `(tool, arguments)`.
pub(crate) fn proposed(arguments: &Value) -> Result<Vec<(String, Value)>, String> {
    let steps = arguments
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{PLAN_TOOL} needs steps (a list of {{tool, arguments}})"))?;
    if steps.is_empty() {
        return Err(format!("{PLAN_TOOL}: the plan has no steps"));
    }
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let tool = s
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{PLAN_TOOL}: step {} names no tool", i + 1))?;
            let args = s.get("arguments").cloned().unwrap_or(Value::Null);
            Ok((tool.to_owned(), args))
        })
        .collect()
}

/// The task's plan.
#[derive(Debug, Default)]
pub(crate) enum Plan {
    #[default]
    None,
    /// The owner confirmed these steps; `true` once a step ran.
    Confirmed(Vec<(Step, bool)>),
}

impl Plan {
    /// Takes the confirmed step `step` is (same tool, same arguments), unless it ran already.
    pub fn take(&mut self, step: &Step) -> bool {
        let Plan::Confirmed(steps) = self else {
            return false;
        };
        match steps
            .iter_mut()
            .find(|(s, spent)| !*spent && s.tool == step.tool && s.args == step.args)
        {
            Some((_, spent)) => {
                *spent = true;
                true
            }
            None => false,
        }
    }
}
