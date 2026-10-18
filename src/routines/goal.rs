//! A goal routine's run (K28): K25's delegate loop (`kai_delegate::run`, as `look` and `find` run
//! it) with the routine's goal as its instructions, the tools its `routine.toml` lists (those Kai
//! offers now), its parameters as the task's, on its alias, else the Kai conversation's. Each of the
//! loop's calls is the routine's own (`routine:<name>`) under K15's policy; a call the policy
//! wants confirmed runs as a step of a plan the owner confirms once (PD4's dialog).

use kai_delegate::{Def, Settings, Task, Utterance};

use super::Inner;
use super::def::{Routine, intern, intern_names};
use crate::delegate::model::LmgwModel;
use crate::delegate::tools::{KaiTools, PlanDialog};
use crate::delegate::{Conversation, conversation_alias};
use crate::host::local::Local;
use crate::host::policy::Caller;
use crate::host::registry::{Block, Call, Reply};
use crate::host::settings::Named;

pub async fn run(
    inner: &Inner,
    local: &Local,
    routine: &Routine,
    goal: &str,
    tools: &[String],
    alias: Option<&str>,
    call: &Call,
) -> Reply {
    let name = routine.name.as_str();
    let failed = |why: String| Reply::error(format!("failed: {why}"));
    let Some(creds) = inner.creds.borrow().clone() else {
        return failed(format!(
            "Kai is not linked to lmgw, so the routine {name} has no model"
        ));
    };
    let alias = match alias {
        Some(a) => a.to_owned(),
        None => {
            let conversation = Conversation {
                creds: &creds,
                folder_name: &inner.config.conversation.folder_name,
                folder_id: inner.config.conversation.folder_id,
                timeout: inner.config.lmgw.timeout(),
            };
            let who = format!("the routine {name}");
            let set = format!("set alias in the routine's {}", super::def::FILE);
            let asked = conversation_alias(&conversation, &who, &set);
            match tokio::select! {
                a = asked => a,
                () = call.cancel.cancelled() => return Reply::error(format!("the routine {name} was cancelled")),
            } {
                Ok(a) => a,
                Err(why) => return failed(why),
            }
        }
    };
    let names: Vec<&'static str> = tools.iter().map(|t| intern(t)).collect();
    let def = Def {
        name: intern(name),
        instructions: intern(&format!(
            "You run the owner's routine {name}. Its goal: {goal}"
        )),
        examples: &[],
        tools: intern_names(names.clone()),
        context: &[],
    };
    let caller = Caller::Named(Named::routine(name));
    let entries = local
        .entries()
        .into_iter()
        .filter(|e| names.contains(&e.spec.name))
        .collect();
    let kai_tools = KaiTools {
        delegate: def.name,
        entries,
        policy: inner.policy.clone(),
        caller: caller.clone(),
        deadline: call.deadline,
        timeout_ms: call.timeout_ms,
    };
    let owner = PlanDialog {
        confirmer: inner.confirmer.clone(),
        caller,
        task: goal.to_owned(),
        alias: alias.clone(),
        deadline: call.deadline,
    };
    let task = Task {
        params: routine
            .spec
            .params
            .iter()
            .filter_map(|p| {
                call.args.value(p.name).map(|v| {
                    (
                        p.name.to_owned(),
                        match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    )
                })
            })
            .collect(),
        utterance: Utterance::Unknown(
            "a routine runs its own goal, not the conversation's words".into(),
        ),
    };
    let settings = Settings {
        max_turns: inner.config.delegate.max_turns,
        max_turns_key: "delegate.max_turns".into(),
    };
    let model = LmgwModel::new(creds.clone(), alias.clone());
    tracing::info!(routine = name, %alias, "routines: a goal started");
    match kai_delegate::run(
        &def,
        &task,
        &model,
        &kai_tools,
        &owner,
        &settings,
        &call.cancel,
    )
    .await
    {
        Ok(a) => {
            tracing::info!(
                routine = name,
                status = a.status.word(),
                turns = a.turns,
                answered_by = ?model.answered_by(),
                "routines: a goal answered"
            );
            let mut text = a.render();
            if let Some(note) = model.note() {
                text.push('\n');
                text.push_str(&note);
            }
            Reply {
                blocks: vec![Block::Text(text)],
                error: a.status == kai_delegate::Status::Failed,
            }
        }
        Err(kai_delegate::Cancelled) => Reply::error(format!("the routine {name} was cancelled")),
    }
}
