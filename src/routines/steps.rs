//! A steps routine's run (K28): its fixed tool calls in order, no model. Each is a call of Kai's
//! own by the routine (`routine:<name>`), through the host's one path (`crate::host::local`): K15's
//! policy for that caller, the owner's confirmation where it asks, the handler. The first step that
//! fails ends the run; the result names it and what ran before it.

use serde_json::Map;

use super::def::{Routine, Step, fill};
use crate::host::local::Local;
use crate::host::policy::Caller;
use crate::host::registry::{Block, Call, Reply};
use crate::host::settings::Named;

pub async fn run(local: &Local, routine: &Routine, steps: &[Step], call: &Call) -> Reply {
    let caller = Caller::Named(Named::routine(&routine.name));
    let declared: Vec<&str> = routine.spec.params.iter().map(|p| p.name).collect();
    let params: Map<String, serde_json::Value> = declared
        .iter()
        .filter_map(|n| call.args.value(n).map(|v| ((*n).to_owned(), v.clone())))
        .collect();
    let mut blocks = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let tool = step.tool.as_str();
        let Some(entry) = local.entries().into_iter().find(|e| e.spec.name == tool) else {
            return failed(
                blocks,
                format!(
                    "step {n}: Kai offers no tool {tool} now (switched off, or not on this \
                     desktop)"
                ),
            );
        };
        let args = fill(&step.args, &params, &declared);
        let reply = local
            .call(&entry, Some(args), &caller, call.cancel.child_token())
            .await;
        let mut first = true;
        for b in reply.blocks {
            match b {
                Block::Text(t) if first => {
                    first = false;
                    blocks.push(Block::Text(format!("{tool}: {t}")));
                }
                other => blocks.push(other),
            }
        }
        if first {
            blocks.push(Block::Text(format!("{tool}: done")));
        }
        if reply.error {
            return failed(
                blocks,
                format!("step {n} ({tool}) failed; the rest did not run"),
            );
        }
    }
    Reply {
        blocks,
        error: false,
    }
}

fn failed(mut blocks: Vec<Block>, why: String) -> Reply {
    blocks.push(Block::Text(why));
    Reply {
        blocks,
        error: true,
    }
}
