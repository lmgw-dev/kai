//! `routine_create {goal}` (K28, K25): a delegate that writes a draft routine, a bash script by
//! default, on `routines.create_alias` (a capable coding model), else the Kai conversation's alias.
//!
//! The loop's one tool is `draft_write {path, content}`, a tool of this delegate only (not hosted):
//! it writes `<name>/routine.toml` or `<name>/<file>` into the routines' folder, only into a folder
//! this task made (an existing routine is never touched) and never under a name Kai's configuration
//! has a switch for (`routines.enabled.<name>`, even one whose folder is gone: a new folder under
//! that name is someone else's routine to the switch), read again at every write: a draft the owner
//! switched on while the task runs takes no more writes. `routine_enable` refuses to switch on a
//! name the task writes until it has ended ([`Drafting`]). Every file but `routine.toml` is made
//! executable. The task message carries the goal, Kai's tools as they are offered now (what the
//! script may call with `kai tool`), and the routines that exist. The draft is off: a routine is on
//! only once the owner switched it on (`routines.enabled`), after seeing it in full.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kai_delegate::{
    CancellationToken, Def, Gate, Outcome, Settings, Step, Task, ToolDef, Utterance,
};
use serde_json::{Value, json};

use super::Inner;
use super::def::{self, FILE, plain_name};
use crate::delegate::model::LmgwModel;
use crate::delegate::tools::PlanDialog;
use crate::delegate::{Conversation, conversation_alias};
use crate::host::local::Local;
use crate::host::registry::{Block, Call, Reply};

const WRITE: &str = "draft_write";

pub const CREATE: Def = Def {
    name: "routine_create",
    instructions: "Write a routine of Kai's for the goal below: a folder holding routine.toml and \
                   an executable bash script. Write each file with draft_write. The draft stays off \
                   until the owner reads it and switches it on.\n\n\
                   routine.toml:\n\
                   description = \"<one line: what it does>\"\n\
                   kind = \"script\"\n\
                   script = \"run.sh\"\n\
                   (optional) at most two [[params]] tables, each with name, kind (text, integer, \
                   number or boolean), optional = true where it may be left out, and description \
                   where the name is not enough; read_only = true if it changes nothing; \
                   destructive = true if what it does cannot be undone simply; open_world = true \
                   if it uses the network.\n\n\
                   run.sh: starts with #!/bin/bash and set -euo pipefail. Its parameters arrive as \
                   $1 and $2 in the order of [[params]] (an omitted optional one is empty). What it \
                   prints on stdout is its result, short; messages for a person go to stderr. Its \
                   exit code says done (0) or failed. It reaches Kai's tools with: kai tool <name> \
                   --<param> <value> (the tool's answer on stdout; when the tool fails, its reason \
                   on stderr and exit code 1), and a model with: kai model <alias> < prompt-file \
                   (the answer on stdout). It never needs a key or a password.\n\n\
                   The folder's name is the routine's name: lowercase letters, digits and single \
                   underscores, not one of the existing names. Paths are <name>/routine.toml and \
                   <name>/run.sh.",
    examples: &["goal: tell me how full my home disk is\n\
         -> draft_write {path: disk_usage/routine.toml, content: description = \"Say how full the \
         home file system is.\"\\nkind = \"script\"\\nscript = \"run.sh\"\\nread_only = true\\n}\n\
         -> draft_write {path: disk_usage/run.sh, content: #!/bin/bash\\nset -euo pipefail\\ndf \
         -h --output=pcent,avail \"$HOME\" | tail -n 1\\n}\n\
         done: Drafted disk_usage."],
    tools: &[WRITE],
    context: &[],
};

/// The names the running `routine_create` tasks write, one entry per task and name.
#[derive(Debug, Clone, Default)]
pub struct Drafting(Arc<Mutex<Vec<String>>>);

impl Drafting {
    pub fn contains(&self, name: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|n| n == name)
    }

    fn add(&self, name: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(name.to_owned());
    }

    fn remove(&self, name: &str) {
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = all.iter().position(|n| n == name) {
            all.swap_remove(i);
        }
    }
}

/// Whether Kai's configuration has a switch for a name now.
pub type Switched = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// The delegate's tool: writes the draft's files.
pub struct DraftTools {
    pub dir: PathBuf,
    /// The folders this task made: only these take files.
    made: Mutex<BTreeSet<String>>,
    /// Asked at every write: a name Kai's configuration has a switch for is never written.
    switched: Switched,
    /// Where this task's names are said while it runs ([`DraftTools::said`]); taken out when it
    /// ends (dropped).
    drafting: Drafting,
    /// The names this task has said in `drafting`.
    said: Mutex<BTreeSet<String>>,
}

impl DraftTools {
    pub fn new(dir: PathBuf, switched: Switched, drafting: Drafting) -> Self {
        Self {
            dir,
            made: Mutex::default(),
            switched,
            drafting,
            said: Mutex::default(),
        }
    }

    pub fn made(&self) -> Vec<String> {
        self.made
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// `path` as `(routine, file)`, or why it is not a draft's file.
    fn target(&self, path: &str) -> Result<(String, String), String> {
        let Some((name, file)) = path.split_once('/') else {
            return Err(format!(
                "{WRITE}: path is <name>/<file>, like backup/run.sh"
            ));
        };
        if !plain_name(name) {
            return Err(format!(
                "{WRITE}: {name:?} is no routine name (lowercase letters, digits, single \
                 underscores)"
            ));
        }
        if kai_desktop::contract::find(name).is_some() {
            return Err(format!("{WRITE}: {name} is one of Kai's own tools"));
        }
        if (self.switched)(name) {
            return Err(format!(
                "{WRITE}: Kai's configuration has a switch for a routine {name} \
                 (routines.enabled.{name}); choose another name"
            ));
        }
        if file.is_empty() || file.contains('/') || file.starts_with('.') {
            return Err(format!(
                "{WRITE}: {file:?} is no file name in the routine's folder"
            ));
        }
        let ours = self
            .made
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(name);
        if !ours && self.dir.join(name).exists() {
            return Err(format!(
                "{WRITE}: a routine {name} exists already; choose another name"
            ));
        }
        Ok((name.to_owned(), file.to_owned()))
    }
}

impl kai_delegate::Tools for DraftTools {
    fn defs(&self, names: &[&str]) -> Vec<ToolDef> {
        names
            .iter()
            .filter(|n| **n == WRITE)
            .map(|_| ToolDef {
                name: WRITE.into(),
                description: "Write one file of the draft routine.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "<name>/routine.toml or <name>/run.sh"},
                        "content": {"type": "string", "description": "the whole file"}
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }),
            })
            .collect()
    }

    fn check(&self, tool: &str, arguments: &Value) -> Result<Step, String> {
        if tool != WRITE {
            return Err(format!("routine_create has no tool '{tool}'"));
        }
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{WRITE} needs path (a text)"))?;
        if !arguments.get("content").is_some_and(Value::is_string) {
            return Err(format!("{WRITE} needs content (a text)"));
        }
        self.target(path)?;
        Ok(Step {
            tool: WRITE.into(),
            args: arguments.as_object().cloned().unwrap_or_default(),
            gate: Gate::Allow,
            shown: format!("{WRITE} ({path})"),
            destructive: false,
            target: path.to_owned(),
        })
    }

    async fn call(&self, step: &Step, _cancel: &CancellationToken) -> Outcome {
        let path = step
            .args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = step
            .args
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (name, file) = match self.target(path) {
            Ok(t) => t,
            Err(why) => return Outcome::error(why),
        };
        // Said before the folder exists, so routine_enable never sees it unsaid.
        if self
            .said
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.clone())
        {
            self.drafting.add(&name);
        }
        let folder = self.dir.join(&name);
        match write_file(&folder, &file, content) {
            Ok(()) => {
                self.made
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name);
                Outcome::text(format!("written {path} ({} bytes)", content.len()))
            }
            Err(e) => Outcome::error(format!("{WRITE}: {path}: {e}")),
        }
    }
}

impl Drop for DraftTools {
    /// The task ended, however it ended (a cancel drops it): its names may be switched on.
    fn drop(&mut self) {
        for name in self
            .said
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            self.drafting.remove(name);
        }
    }
}

/// Writes `file` into `folder` (made if missing), executable unless it is `routine.toml`; through a
/// new file renamed in place, so the watcher never reads half of it.
fn write_file(folder: &Path, file: &str, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(folder)?;
    let mode = if file == FILE { 0o644 } else { 0o755 };
    let tmp = folder.join(format!(".{file}.draft-{}", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, folder.join(file))
}

/// Kai's tools as the task message lists them: one line each, `name(params): description`.
fn catalog(local: &Local) -> String {
    local
        .entries()
        .iter()
        .map(|e| {
            let params: Vec<String> = e
                .spec
                .params
                .iter()
                .map(|p| {
                    let opt = if p.required { "" } else { "?" };
                    format!("--{}{opt} <{}>", p.name, p.kind.noun())
                })
                .collect();
            format!(
                "{} {}: {}",
                e.spec.name,
                params.join(" "),
                e.spec.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run(inner: &Arc<Inner>, local: &Local, dir: &Path, call: &Call) -> Reply {
    let goal = call.args.text("goal").unwrap_or_default().to_owned();
    let failed = |why: String| Reply::error(format!("failed: {why}"));
    let Some(creds) = inner.creds.borrow().clone() else {
        return failed("Kai is not linked to lmgw, so routine_create has no model".into());
    };
    let alias = match &inner.config.routines.create_alias {
        Some(a) => a.clone(),
        None => {
            let conversation = Conversation {
                creds: &creds,
                folder_name: &inner.config.conversation.folder_name,
                folder_id: inner.config.conversation.folder_id,
                timeout: inner.config.lmgw.timeout(),
            };
            let asked = conversation_alias(
                &conversation,
                "routine_create",
                "set routines.create_alias (a capable coding model)",
            );
            match tokio::select! {
                a = asked => a,
                () = call.cancel.cancelled() => return Reply::error("routine_create was cancelled"),
            } {
                Ok(a) => a,
                Err(why) => return failed(why),
            }
        }
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        return failed(format!("the routines' folder cannot be made: {e}"));
    }
    let state = inner.state();
    let switched: BTreeSet<String> = state.switches.enabled.keys().cloned().collect();
    let existing: Vec<String> = state
        .loaded
        .iter()
        .map(|l| l.name.clone())
        .chain(switched.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let task = Task {
        params: vec![
            ("goal".into(), goal.clone()),
            (
                "Kai's tools (kai tool <name> --<param> <value>)".into(),
                catalog(local),
            ),
            (
                "Existing routines (not these names)".into(),
                if existing.is_empty() {
                    "none".into()
                } else {
                    existing.join(", ")
                },
            ),
        ],
        utterance: Utterance::Unknown("the goal is the whole task".into()),
    };
    let live = inner.clone();
    let tools = DraftTools::new(
        dir.to_owned(),
        Box::new(move |name| live.state().switches.has_switch(name)),
        inner.drafting.clone(),
    );
    let owner = PlanDialog {
        confirmer: inner.confirmer.clone(),
        caller: call.caller.clone(),
        task: goal,
        alias: alias.clone(),
        deadline: call.deadline,
    };
    let settings = Settings {
        max_turns: inner.config.delegate.max_turns,
        max_turns_key: "delegate.max_turns".into(),
    };
    let model = LmgwModel::new(creds.clone(), alias.clone());
    tracing::info!(caller = %call.caller.describe(), %alias, "routines: routine_create started");
    let answer = kai_delegate::run(
        &CREATE,
        &task,
        &model,
        &tools,
        &owner,
        &settings,
        &call.cancel,
    )
    .await;
    let made = tools.made();
    let answer = match answer {
        Ok(a) => a,
        Err(kai_delegate::Cancelled) => {
            return Reply::error(format!(
                "routine_create was cancelled{}",
                if made.is_empty() {
                    String::new()
                } else {
                    format!(" (written so far, off: {})", made.join(", "))
                }
            ));
        }
    };
    tracing::info!(
        status = answer.status.word(),
        drafts = made.len(),
        "routines: routine_create answered"
    );
    let mut lines = vec![answer.render()];
    for name in &made {
        let loaded = def::load(&dir.join(name));
        lines.push(match loaded.routine {
            Ok(r) => format!(
                "draft {name} ({}): {}; off until the owner switches it on (routine_enable or the \
                 tray), which shows it in full",
                r.body.word(),
                r.spec.description
            ),
            Err(why) => format!("draft {name} does not load yet: {why}"),
        });
    }
    if made.is_empty() {
        lines.push("no draft was written".into());
    }
    if let Some(note) = model.note() {
        lines.push(note);
    }
    Reply {
        blocks: vec![Block::Text(lines.join("\n"))],
        error: made.is_empty() || answer.status == kai_delegate::Status::Failed,
    }
}
