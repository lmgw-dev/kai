//! Kai's delegates (ruling 21, K25, WP17): `look` and `find`, each a hosted tool whose call runs
//! `kai_delegate`'s loop with
//! - **the model**: lmgw's `/v1/chat/completions` as Kai's device ([`model`]), on the
//!   delegate's alias (`[delegate]`, [`settings`]), else the Kai conversation's (its folder's
//!   model, asked from lmgw at each call, so a change on the dashboard holds at once);
//! - **the tools**: the desktop module's entries (WP14 to WP16), called in process, each call by
//!   the caller that called the delegate, under K15's policy ([`tools`]); a plan is confirmed in
//!   PD4's dialog, once;
//! - **the user's words**: the last utterance of Kai's own voice session, for Kai's own calls
//!   ([`heard`]), and only if they came for the turn the call is in.
//!
//! The answer is the loop's short text (done, partial or failed, and the tools used); a failed one
//! is the tool's error (`isError`). The screenshots stay in the delegate's requests, which lmgw's
//! request log keeps with the rest of its transcript.
//!
//! **Listing:** a delegate is listed while the desktop serves (and the policy lists) one of the
//! tools it needs (`defs::Delegate::needs_any`), and only in K26's synchronous class until Kai has
//! asynchronous jobs (WP18): `arrange_windows` and `operate_app` plug in there ([`defs`]).
//!
//! **The policy** is the host's own, shared live (`Deps::policy`): the listing and each step's
//! decision read it, so a policy change while Kai runs (WP21a's operations) applies to the
//! delegates' calls and list at once.
//!
//! **Who looks at screenshots** (`delegate.screenshots`): with `delegate`, the host lists the
//! desktop's tools to its callers without the image tools, and `look` is the way to see
//! ([`screenshots`]). The delegates' own tools are the desktop module's, unfiltered.

pub mod defs;
mod heard;
pub mod model;
pub mod screenshots;
pub mod settings;
#[cfg(test)]
pub(crate) mod tests;
pub mod tools;

use std::sync::Arc;
use std::time::{Duration, Instant};

use kai_delegate::{Settings, Task, Utterance};
use kai_desktop::contract::{Class, delegate::AREA};
use lmgw_client::requests;
use tokio::sync::watch;

pub use heard::{heard, response_created, session_ended};
pub use screenshots::host_view;

use crate::conversation::{FolderInfo, Pick, pick};
use crate::host::confirm::{Confirmer, Dialog};
use crate::host::policy::{Caller, SharedPolicy};
use crate::host::registry::{Block, Call, Deps, Entry, Provider, Reply};
use crate::lmgw_link::Credentials;
use defs::{DELEGATES, Delegate};
use model::LmgwModel;
use settings::DelegateConfig;
use tools::{KaiTools, PlanDialog};

/// The delegates' provider (module doc).
pub struct DelegateTools {
    inner: Arc<Inner>,
}

/// What a delegate's call needs.
pub struct Inner {
    /// The desktop module's provider: the delegates' tools.
    pub desktop: Box<dyn Provider>,
    /// The host's live policy: each decision and listing reads it, so a change applies at once.
    pub policy: SharedPolicy,
    pub confirmer: Arc<dyn Confirmer>,
    pub creds: watch::Receiver<Option<Credentials>>,
    pub settings: DelegateConfig,
    /// `conversation.folder_name` and `conversation.folder_id`: the Kai conversation's folder.
    pub folder_name: String,
    pub folder_id: Option<i64>,
    /// `lmgw.timeout_s`: the folder list's answer, and connecting to lmgw.
    pub lmgw_timeout: Duration,
    /// The user's last words in Kai's own session ([`heard`]).
    pub utterance: fn() -> Option<String>,
}

/// The provider `registry::providers` lists, over the desktop module's provider.
pub fn provider(deps: &Deps, desktop: impl Provider) -> DelegateTools {
    let timeout = deps.config.lmgw.timeout();
    DelegateTools::new(Inner {
        desktop: Box::new(desktop),
        policy: deps.policy.clone(),
        confirmer: Arc::new(Dialog(deps.ui.clone())),
        creds: deps.creds.clone(),
        settings: deps.config.delegate.clone(),
        folder_name: deps.config.conversation.folder_name.clone(),
        folder_id: deps.config.conversation.folder_id,
        lmgw_timeout: timeout,
        utterance: heard::last,
    })
}

impl DelegateTools {
    pub fn new(inner: Inner) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }
}

#[cfg(test)]
impl DelegateTools {
    /// The desktop's entries the policy lists (a test's `KaiTools`).
    fn inner_entries(&self) -> Vec<Entry> {
        self.inner.desktop_tools()
    }
}

impl Provider for DelegateTools {
    fn entries(&self) -> Vec<Entry> {
        let desk = self.inner.desktop_tools();
        DELEGATES
            .iter()
            // K26: an asynchronous delegate waits for WP18's jobs.
            .filter(|d| d.spec.class == Class::Sync)
            .filter(|d| {
                d.needs_any
                    .iter()
                    .any(|n| desk.iter().any(|e| e.spec.name == *n))
            })
            .map(|d: &'static Delegate| {
                let inner = self.inner.clone();
                Entry {
                    area: AREA.name,
                    spec: d.spec,
                    handler: Arc::new(move |call| {
                        let inner = inner.clone();
                        Box::pin(async move { inner.run_task(d, call).await })
                    }),
                    prepare: None,
                }
            })
            .collect()
    }
}

fn failed(text: String) -> Reply {
    Reply::error(format!("failed: {text}"))
}

impl Inner {
    /// The desktop's entries the policy lists.
    fn desktop_tools(&self) -> Vec<Entry> {
        let policy = self.policy.read().expect("never poisoned");
        self.desktop
            .entries()
            .into_iter()
            .filter(|e| policy.listed(e.spec))
            .collect()
    }

    async fn run_task(&self, d: &'static Delegate, call: Call) -> Reply {
        let started = Instant::now();
        let name = d.def.name;
        let caller = call.caller.clone();
        let params = d
            .spec
            .params
            .iter()
            .filter_map(|p| {
                call.args
                    .text(p.name)
                    .map(|v| (p.name.to_owned(), v.to_owned()))
            })
            .collect();
        let utterance = if caller == Caller::Own {
            match (self.utterance)() {
                Some(words) => Utterance::Heard(words),
                None => Utterance::Unknown("not transcribed yet".into()),
            }
        } else {
            Utterance::Unknown(format!(
                "the call came from {}, whose conversation Kai does not hear",
                caller.describe()
            ))
        };
        let task = Task { params, utterance };
        let Some(creds) = self.creds.borrow().clone() else {
            return failed(format!(
                "Kai is not linked to lmgw, so the {name} delegate has no model"
            ));
        };
        let alias = tokio::select! {
            a = self.alias(name, &creds) => a,
            () = call.cancel.cancelled() => return Reply::error(format!("the {name} task was cancelled")),
        };
        let alias = match alias {
            Ok(a) => a,
            Err(why) => {
                tracing::info!(delegate = name, caller = %caller.describe(), "delegate: no alias");
                return failed(why);
            }
        };
        if d.vision {
            let seen = tokio::select! {
                s = self.vision(&alias, &creds) => s,
                () = call.cancel.cancelled() => return Reply::error(format!("the {name} task was cancelled")),
            };
            if seen == Some(false) {
                tracing::info!(delegate = name, %alias, "delegate: the alias has no vision");
                return failed(format!(
                    "the alias '{alias}' takes no images, and {name} works from screenshots; \
                     set delegate.{name} to a vision alias"
                ));
            }
        }
        let model = LmgwModel::new(creds.clone(), alias.clone());
        let tools = KaiTools {
            delegate: name,
            entries: self
                .desktop_tools()
                .into_iter()
                .filter(|e| d.def.tools.contains(&e.spec.name))
                .collect(),
            policy: self.policy.clone(),
            caller: caller.clone(),
            deadline: call.deadline,
            timeout_ms: call.timeout_ms,
        };
        let owner = PlanDialog {
            confirmer: self.confirmer.clone(),
            caller: caller.clone(),
            task: d
                .spec
                .params
                .first()
                .and_then(|p| call.args.text(p.name))
                .unwrap_or_default()
                .to_owned(),
            alias: alias.clone(),
            deadline: call.deadline,
        };
        let settings = Settings {
            max_turns: self.settings.max_turns,
            max_turns_key: "delegate.max_turns".into(),
        };
        tracing::info!(delegate = name, caller = %caller.describe(), %alias, "delegate: a task");
        match kai_delegate::run(
            d.def,
            &task,
            &model,
            &tools,
            &owner,
            &settings,
            &call.cancel,
        )
        .await
        {
            Ok(a) => {
                tracing::info!(
                    delegate = name,
                    status = a.status.word(),
                    turns = a.turns,
                    calls = a.used.iter().map(|(_, n)| n).sum::<u32>(),
                    ms = started.elapsed().as_millis() as u64,
                    answered_by = ?model.answered_by(),
                    "delegate: answered"
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
            Err(kai_delegate::Cancelled) => {
                tracing::info!(delegate = name, "delegate: the task was cancelled");
                Reply::error(format!("the {name} task was cancelled"))
            }
        }
    }

    /// Whether lmgw lists `alias` with vision (`capabilities.vision` of `/v1/models`): `None` when
    /// lmgw does not say or cannot be asked, and the task goes on.
    async fn vision(&self, alias: &str, creds: &Credentials) -> Option<bool> {
        let req = model::models();
        let (http, url) = (creds.http(), creds.url());
        let asked = model::send(&http, &url, req);
        match tokio::time::timeout(self.lmgw_timeout, asked).await {
            Ok(Ok((status, body))) if (200..300).contains(&status) => {
                model::vision_of(&body, alias)
            }
            _ => None,
        }
    }

    /// The delegate's alias: `delegate.<name>`, else the Kai conversation's folder's model.
    async fn alias(&self, name: &str, creds: &Credentials) -> Result<String, String> {
        if let Some(a) = self.settings.alias(name) {
            return Ok(a.to_owned());
        }
        conversation_alias(
            &Conversation {
                creds,
                folder_name: &self.folder_name,
                folder_id: self.folder_id,
                timeout: self.lmgw_timeout,
            },
            &format!("the {name} delegate"),
            &format!("set delegate.{name}"),
        )
        .await
    }
}

/// Where the Kai conversation's alias is asked for.
pub struct Conversation<'a> {
    pub creds: &'a Credentials,
    /// `conversation.folder_name` and `conversation.folder_id`.
    pub folder_name: &'a str,
    pub folder_id: Option<i64>,
    /// `lmgw.timeout_s`.
    pub timeout: Duration,
}

/// The Kai conversation's alias (its folder's model), asked from lmgw now. `who` needs it ("the
/// look delegate", "routine_create"); `set` is the setting that names one instead ("set
/// delegate.look"), said in every error.
pub async fn conversation_alias(
    c: &Conversation<'_>,
    who: &str,
    set: &str,
) -> Result<String, String> {
    let req = requests::folders();
    let secs = c.timeout.as_secs();
    let (http, url) = (c.creds.http(), c.creds.url());
    let asked = model::send(&http, &url, req);
    let (status, body) = match tokio::time::timeout(c.timeout, asked).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(e)) => {
            return Err(format!(
                "Kai could not ask lmgw for the Kai conversation's alias ({e}); {set}"
            ));
        }
        Err(_) => {
            return Err(format!(
                "lmgw did not list the folders within {secs} s (lmgw.timeout_s), so the Kai \
                     conversation's alias is not known; {set}"
            ));
        }
    };
    let list = requests::read_folders(status, &body).map_err(|e| {
        format!(
            "lmgw refused the folder list ({}: {}); {set}",
            e.code, e.message
        )
    })?;
    let infos: Vec<FolderInfo> = list.folders.iter().map(FolderInfo::from).collect();
    let folder = match pick(&infos, c.folder_name, c.folder_id) {
        Pick::Use { folder, .. } => folder,
        Pick::Create => {
            return Err(format!(
                "lmgw has no folder '{}' (conversation.folder_name), so {who} has no alias; {set}",
                c.folder_name
            ));
        }
    };
    folder.model.ok_or_else(|| {
        format!(
            "the Kai conversation's folder '{}' has no model, so {who} has no alias; {set} or the \
             folder's model",
            folder.name
        )
    })
}
