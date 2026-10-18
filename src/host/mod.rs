//! Kai's hosted MCP server (ruling 17, K2, §10.1, §11.1, PD2): Kai connects out to lmgw's
//! `/mcp/host` over the device link and serves its tools there; lmgw lists them under the label of
//! the device's hosting grant (`<label>__<tool>`) to the principals api §5.6 lets reach it, and
//! forwards their calls with who is calling (`_meta`, api §5.5).
//!
//! - the wire's facts (the route, its refusals and closes, the `_meta` on every call, the limits
//!   in `initialize`) are lmgw's: `lmgw_client::mcp_host`;
//! - [`link`]: the WebSocket, rmcp's transport over it, reconnects and closes;
//! - [`server`]: rmcp's server role: the list and each call through the policy, the confirmation
//!   and the handler, cancellation honoured;
//! - [`registry`]: the tools, from the contract (`kai_desktop::contract`) and the providers that
//!   serve them; how later work packages add tools;
//! - [`limits`]: the link's size limits (announced by lmgw, else `[host]`'s fallbacks), the frames
//!   a message is split into, the room a tool's result has;
//! - [`policy`]: K15; [`confirm`]: PD4's dialog; [`settings`]: `[host]` and `[policy]`.
//!
//! The core starts and stops it exactly where it starts and stops the feed ([`ToolHost`]), and
//! follows its [`HostEvent`]s: the tooltip's tools line, and a revocation said on the link (a
//! 4003, a 401) goes the feed's path (§3.3).

pub mod confirm;
pub mod limits;
pub mod link;
#[cfg(feature = "live-test")]
mod live;
pub mod local;
pub mod policy;
pub mod registry;
pub mod server;
pub mod settings;
pub mod tasks;

#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::lmgw_link::{Credentials, KeyLoss};
use link::{LinkSettings, Run};
use policy::Policy;
use registry::{Provider, Registry};
use server::Shared;

/// The seam the core drives (like `LmgwLink`).
pub trait ToolHost: Send + 'static {
    /// Starts the link for `creds`, replacing any earlier run. Events go to `events` until
    /// [`ToolHost::stop`] or the next start.
    fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<HostEvent>);
    /// Closes the link (Kai's close, 1000); its calls are cancelled.
    fn stop(&mut self);
    /// The feed's `hello`: Kai's own device name (its calls are Kai's own, K15) and the label lmgw
    /// lists the tools under. A link that waits for the grant or after a takeover tries again.
    fn feed_up(&mut self, device: &str, label: Option<&str>);
    /// A new policy (WP21a's operations): when it changes the list, lmgw is told once
    /// (`list_changed`).
    fn set_policy(&mut self, policy: Policy);
    /// Who looks at screenshots (`delegate.screenshots`, `crate::delegate::screenshots`): the
    /// list changes as a policy change does.
    fn set_screenshots(&mut self, seen_by: crate::delegate::settings::Screenshots);
}

/// What the host reports to the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    Status(HostStatus),
    /// A 4003 close or a 401 at the handshake (§3.3): `loss` as the feed reads it; `None` for a
    /// kind this build does not know (the core says lmgw's sentence; the link tries again, and
    /// the feed finds whether the key still opens).
    Revoked {
        reason: String,
        loss: Option<KeyLoss>,
    },
}

/// The link's state, for the tooltip's tools line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Connecting,
    /// lmgw initialized the link; it lists the tools.
    Linked {
        tools: usize,
    },
    /// 403 `host_not_granted`: the device may not host tools (no hosting label, no row on lmgw's
    /// MCP page yet, or its row switched off there); `message` is lmgw's, which says which.
    NotGranted {
        retry_s: u64,
        message: String,
    },
    /// Not linked: why, and when it tries again.
    Offline {
        note: String,
    },
    /// Another link of this device took over (4000); Kai links again at the feed's next `hello`.
    TakenOver {
        reason: String,
    },
}

/// The tooltip's tools line (§15.1.4 WP8: linked as `<label>`, not granted, offline).
pub fn tools_line(status: &HostStatus, label: Option<&str>) -> String {
    match status {
        HostStatus::Connecting => "tools: linking to lmgw".into(),
        HostStatus::Linked { tools } => {
            let n = match tools {
                1 => "1 tool".to_owned(),
                n => format!("{n} tools"),
            };
            match label {
                Some(l) => format!("tools: linked as {l} ({n})"),
                None => format!("tools: linked ({n})"),
            }
        }
        HostStatus::NotGranted { retry_s, message } => {
            let said = if message.trim().is_empty() {
                "lmgw does not let this device host tools"
            } else {
                message.trim()
            };
            format!(
                "tools: not granted ({said}; Kai tries again every {retry_s} s and when the feed \
                 reconnects)"
            )
        }
        HostStatus::Offline { note } => format!("tools: offline ({note})"),
        HostStatus::TakenOver { reason } => format!(
            "tools: another connection of this device took them over ({reason}); Kai links again \
             when its feed reconnects"
        ),
    }
}

/// Kai's hosted server (module doc).
pub struct HostLink {
    settings: LinkSettings,
    shared: Arc<Shared>,
    /// The feed's `hello`s, counted (a run waiting for one sees the next).
    kick: watch::Sender<u64>,
    /// The providers' change signal ([`registry::Changes`]).
    changes: Option<watch::Receiver<u64>>,
    run: Option<(CancellationToken, JoinHandle<()>)>,
    /// The current credentials, for the providers (`registry::Deps::creds`).
    creds: watch::Sender<Option<Credentials>>,
    /// The last stopped run, still closing its socket: the next run waits for it.
    ending: Option<JoinHandle<()>>,
    relister: Option<JoinHandle<()>>,
}

impl HostLink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: LinkSettings,
        policy: policy::SharedPolicy,
        providers: Vec<Box<dyn Provider>>,
        changes: watch::Receiver<u64>,
        confirmer: Arc<dyn confirm::Confirmer>,
        creds: watch::Sender<Option<Credentials>>,
        screen: crate::screen_lock::ScreenLock,
        jobs: crate::jobs::Jobs,
    ) -> Self {
        let shared = Arc::new(Shared::new(
            Registry::new(providers),
            policy,
            confirmer,
            screen,
            jobs,
        ));
        *shared.limits.write().expect("never poisoned") = settings.fallback;
        shared
            .jobs
            .set_notifier(tasks::notifier(Arc::downgrade(&shared)));
        let offered: std::collections::HashSet<&str> = shared
            .registry
            .entries()
            .iter()
            .map(|e| e.spec.name)
            .collect();
        for key in shared
            .policy
            .read()
            .expect("never poisoned")
            .unknown_names(&offered)
        {
            tracing::warn!("host: {key}: Kai offers no such tool; the setting does nothing");
        }
        for name in shared
            .policy
            .read()
            .expect("never poisoned")
            .app_tool_names()
        {
            tracing::info!(
                "host: policy.tools.{name}: an MCP app's tool, checked when its server's view opens"
            );
        }
        Self {
            settings,
            shared,
            kick: watch::channel(0).0,
            changes: Some(changes),
            run: None,
            creds,
            ending: None,
            relister: None,
        }
    }

    /// K26's jobs (WP18): the core's tools and the voice read the same table.
    pub fn jobs(&self) -> crate::jobs::Jobs {
        self.shared.jobs.clone()
    }

    /// The host for Kai's own calls (K28: the routines' CLI, the tray's routines).
    pub fn local(&self) -> local::Local {
        local::Local::new(&self.shared)
    }

    /// The settings `[policy]` names that no tool has ("policy.tools.x"), for the tooltip.
    pub fn unknown_policy_names(&self) -> Vec<String> {
        let entries = self.shared.registry.entries();
        let offered = entries.iter().map(|e| e.spec.name).collect();
        self.shared
            .policy
            .read()
            .expect("never poisoned")
            .unknown_names(&offered)
    }

    /// The core's commands: from then on a trusted caller's confirmation offers "always allow"
    /// (`server::Shared::always_option`), which the core writes.
    pub fn set_core(&self, cmds: mpsc::UnboundedSender<crate::core::Command>) {
        let _ = self.shared.core.set(cmds);
    }

    /// A new policy (WP21a's operations): when it changes the list (`off`, `config_tools`), lmgw
    /// is told.
    pub fn set_policy(&self, policy: Policy) -> bool {
        *self.shared.policy.write().expect("never poisoned") = policy;
        self.shared.relist()
    }

    /// Who looks at screenshots: the desktop's view sets it, and when the list changed lmgw is
    /// told (`crate::delegate::screenshots`).
    pub fn set_screenshots(&self, seen_by: crate::delegate::settings::Screenshots) -> bool {
        let Some(view) = self
            .shared
            .registry
            .find::<crate::delegate::screenshots::HostView>()
        else {
            return false;
        };
        view.set(seen_by);
        self.shared.relist()
    }

    /// The tool list as lmgw would send it to a model under `label`, as OpenAI's `tools` array
    /// (`kai --tool-list`, `just tool-cost`).
    pub fn tool_list_json(&self, label: &str) -> serde_json::Value {
        let listed = self.shared.listed.read().expect("never poisoned");
        serde_json::Value::Array(
            listed
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": format!("{label}__{}", t.name),
                            "description": t.description,
                            "parameters": serde_json::Value::Object((*t.input_schema).clone()),
                        }
                    })
                })
                .collect(),
        )
    }
}

impl LinkSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            retry_max: c.lmgw.retry_max(),
            timeout: c.lmgw.timeout(),
            keepalive: c.host.keepalive(),
            fallback: c.host.fallback_limits(),
        }
    }
}

impl ToolHost for HostLink {
    fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<HostEvent>) {
        self.stop();
        if self.relister.is_none()
            && let Some(mut changes) = self.changes.take()
        {
            let shared = self.shared.clone();
            self.relister = Some(tokio::spawn(async move {
                while changes.changed().await.is_ok() {
                    shared.relist();
                }
            }));
        }
        // A new pairing's identity comes with its feed's `hello`; until then no call is Kai's own
        // and no label is shown.
        *self.shared.own_device.write().expect("never poisoned") = None;
        *self.shared.label.write().expect("never poisoned") = None;
        let generation = self.shared.peer.lock().expect("never poisoned").next();
        self.creds.send_replace(Some(creds.clone()));
        let stop = CancellationToken::new();
        let run = Run {
            creds,
            settings: self.settings,
            shared: self.shared.clone(),
            events,
            kick: self.kick.subscribe(),
            stop: stop.clone(),
            generation,
            prior: self.ending.take(),
        };
        self.run = Some((stop, tokio::spawn(run.run())));
    }

    fn stop(&mut self) {
        if let Some((stop, task)) = self.run.take() {
            // The run closes its socket (1000) and ends; its calls are cancelled with it. The
            // next run waits for that.
            stop.cancel();
            self.ending = Some(task);
        }
        // Fenced: the stopped run's peer is gone, and it sets or clears none from now on.
        self.shared.peer.lock().expect("never poisoned").next();
        self.creds.send_replace(None);
    }

    fn feed_up(&mut self, device: &str, label: Option<&str>) {
        *self.shared.own_device.write().expect("never poisoned") = Some(device.to_owned());
        *self.shared.label.write().expect("never poisoned") = label.map(str::to_owned);
        self.shared.jobs.set_label(label.map(str::to_owned));
        self.kick.send_modify(|n| *n += 1);
    }

    fn set_policy(&mut self, policy: Policy) {
        HostLink::set_policy(self, policy);
    }

    fn set_screenshots(&mut self, seen_by: crate::delegate::settings::Screenshots) {
        HostLink::set_screenshots(self, seen_by);
    }
}

impl Drop for HostLink {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.relister.take() {
            t.abort();
        }
    }
}
