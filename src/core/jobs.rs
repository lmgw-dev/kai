//! K26's job tools on the core (PD3: a job's announce; WP18): `jobs`, `job_cancel` and
//! `job_announce` read and change the host's job table (`crate::jobs`), which the voice reads when
//! a result enters the chat. A job is named by its short id or its task id. The owner's callers act
//! on any job, every other caller only on its own (`crate::jobs::may_act`): refused before anyone
//! is asked, and the dialog names the job's tool, its state and who started it.

use std::sync::Arc;

use super::Core;
use super::calls::CoreCall;
use crate::audio::AudioPort;
use crate::host::registry::{Args, Origin, Prepared, Preparer, Reply};
use crate::jobs::{Jobs, Refusal, may_act};
use crate::lmgw_link::LmgwLink;
use crate::secret::SecretStore;
use crate::voice::VoiceLink;

/// `job_cancel`'s and `job_announce`'s preparation (module doc); `None` for the other tools.
pub(super) fn preparer(tool: &str, jobs: &Jobs) -> Option<Preparer> {
    if !matches!(tool, "job_cancel" | "job_announce") {
        return None;
    }
    let jobs = jobs.clone();
    Some(Arc::new(move |args: &Args, origin: Origin| {
        let id = args.text("id").unwrap_or_default().trim().to_owned();
        let found = match jobs.view(&id) {
            None => Err(Refusal::Unknown.words(&id)),
            Some(v) if origin.caller.as_ref().is_some_and(|c| !may_act(c, &v.by)) => {
                Err(Refusal::NotYours(v.caller.clone()).words(&id))
            }
            Some(v) => Ok(Prepared {
                lines: vec![format!(
                    "job {}: {}, {}, started by {}",
                    v.id,
                    v.tool,
                    if v.running { "running" } else { v.status },
                    v.caller
                )],
                state: Arc::new(()),
                notes: Vec::new(),
                options: Vec::new(),
            }),
        };
        Box::pin(async move { found })
    }))
}

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    pub(super) async fn jobs_tool(&mut self, c: CoreCall) {
        let id = c.call.args.text("id").unwrap_or_default().trim().to_owned();
        let actor = &c.call.caller;
        let reply = match c.call.spec.name {
            "jobs" => Reply::text(crate::jobs::listing(&self.jobs)),
            "job_cancel" => match self.jobs.cancel_as(&id, &actor.describe(), actor) {
                Ok(_) => Reply::text(format!(
                    "job {id} is cancelled; its result says so when it reaches the chat"
                )),
                Err(r) => Reply::error(r.words(&id)),
            },
            "job_announce" => {
                let on = c.call.args.boolean("on").unwrap_or(true);
                match self.jobs.set_announce_as(&id, on, actor) {
                    Ok(()) if on => Reply::text(format!(
                        "Kai speaks job {id}'s result when it reaches the chat, at a quiet moment"
                    )),
                    Ok(()) => Reply::text(format!(
                        "Kai notifies job {id}'s result when it reaches the chat, and does not \
                         speak it"
                    )),
                    Err(r) => Reply::error(r.words(&id)),
                }
            }
            other => Reply::error(format!("Kai's jobs area has no tool {other}")),
        };
        let _ = c.reply.send(reply);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::host::policy::Caller;
    use crate::host::settings::Named;
    use crate::jobs::{Delivery, Start};

    fn device(name: &str) -> Caller {
        Caller::Named(Named {
            kind: "device".into(),
            name: name.into(),
        })
    }

    /// `job_cancel`'s dialog names the job's tool, its state and who started it; another caller's
    /// job is refused before anyone is asked, and so is one Kai does not know.
    #[tokio::test]
    async fn the_dialog_names_the_job_and_another_callers_is_refused_first() {
        let jobs = Jobs::default();
        let id = jobs
            .start(Start {
                tool: "run_command",
                label: None,
                by: device("phone"),
                delivery: Delivery::Thread(Some(1)),
                routine: false,
            })
            .unwrap()
            .id;
        let spec = kai_desktop::contract::find("job_cancel").unwrap().1;
        let prepare = preparer("job_cancel", &jobs).unwrap();
        let args = |id: &str| Args::read(spec, json!({"id": id}).as_object().cloned()).unwrap();
        let origin = |c: Caller| Origin {
            caller: Some(c),
            ..Origin::default()
        };
        let p = prepare(&args(&id), origin(Caller::Owner)).await.unwrap();
        assert_eq!(
            p.lines,
            [format!(
                "job {id}: run_command, running, started by device 'phone'"
            )]
        );
        assert!(prepare(&args(&id), origin(device("phone"))).await.is_ok());
        let refused = prepare(&args(&id), origin(device("laptop")))
            .await
            .err()
            .unwrap();
        assert!(refused.contains("started by device 'phone'"), "{refused}");
        let unknown = prepare(&args("zzzz"), origin(Caller::Owner))
            .await
            .err()
            .unwrap();
        assert_eq!(unknown, "Kai has no job zzzz");
        assert!(preparer("jobs", &jobs).is_none());
    }
}
