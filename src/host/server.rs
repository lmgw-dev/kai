//! rmcp's server role on the link (PD2): the tool list, and each call through K15's policy, the
//! owner's confirmation (PD4) and the tool's handler, with lmgw's cancellation honoured at every
//! step (api §5.4: a cancelled call never runs, and a running one is stopped).
//!
//! The log names the tool, the caller and what happened, never an argument or a result (rmcp's own
//! traces, which carry whole messages, are capped in `crate::logging`).

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use kai_desktop::contract::ToolSpec;
use lmgw_client::mcp_host::{CallMeta, HostLimits};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, CancelTaskParams, CancelTaskResult, ContentBlock,
    CreateTaskResult, ErrorCode, GetTaskParams, GetTaskPayloadParams, GetTaskPayloadResult,
    GetTaskResult, Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, TaskSupport, Tool, ToolAnnotations, ToolExecution,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::confirm::{ConfirmRequest, Confirmer};
use super::limits::{LinkLimits, answer_len};
use super::policy::{Caller, Decision, Policy, SharedPolicy};
use super::registry::{Args, Block, Call, Entry, Origin, Registry, Reply};

/// The JSON-RPC code of the answer to a call lmgw cancelled (it no longer waits for one; MCP asks
/// for none, rmcp sends one anyway).
const CANCELLED: ErrorCode = ErrorCode(-32800);

/// The offered tools: the registry's entries the policy lists, and their MCP rendering.
#[derive(Default)]
pub struct Listed {
    pub entries: Vec<Entry>,
    pub tools: Vec<Tool>,
}

impl Listed {
    pub fn build(registry: &Registry, policy: &Policy) -> Self {
        Self::of(registry.entries(), policy)
    }

    /// What the policy lists of the entries the providers offer.
    fn of(offered: Vec<Entry>, policy: &Policy) -> Self {
        let entries: Vec<Entry> = offered
            .into_iter()
            .filter(|e| policy.listed(e.spec))
            .collect();
        let tools = entries.iter().map(|e| render(e.spec)).collect();
        Self { entries, tools }
    }

    fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.iter().find(|e| e.spec.name == name).cloned()
    }
}

/// A spec as MCP's `Tool`: the name as is (lmgw lists it as `<label>__<name>`), the description,
/// the input schema and every annotation stated.
pub fn render(spec: &ToolSpec) -> Tool {
    let mut tool = Tool::new(spec.name, spec.description, Arc::new(spec.input_schema()));
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(spec.hints.read_only)
            .destructive(spec.hints.destructive)
            .idempotent(spec.hints.idempotent)
            .open_world(spec.hints.open_world),
    );
    // K26's open-ended class is called as an MCP task (WP18; lmgw's MCP Tasks design T3).
    if spec.class == kai_desktop::contract::Class::Async {
        tool = tool.with_execution(ToolExecution::new().with_task_support(TaskSupport::Required));
    }
    tool
}

/// What bounds one call: its cancellation (lmgw's `notifications/cancelled`, the link's end, a CLI
/// caller gone), lmgw's deadline (`lmgw/timeout_ms` from the call's arrival, the confirmation
/// included; `None`: no deadline) and the room its result has within the link's message limit.
pub struct Bounds {
    pub cancel: tokio_util::sync::CancellationToken,
    pub deadline: Option<Instant>,
    pub timeout_ms: Option<u64>,
    pub room: Option<u64>,
    pub room_says: String,
    /// `Call::out`.
    pub out: Option<super::registry::Out>,
}

/// Why a call ended without its handler's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Cancelled; when, for the log ("before it ran").
    Cancelled(&'static str),
    /// Refused: the reason the caller reads (K15's, the owner's, the tool's own check).
    Refused(String),
}

/// What every link run's server shares with the host: kept across reconnects.
pub struct Shared {
    pub registry: Registry,
    pub listed: RwLock<Listed>,
    pub policy: SharedPolicy,
    /// Kai's own device name (the feed's `hello.principal.name`): its calls are [`Caller::Own`].
    pub own_device: RwLock<Option<String>>,
    /// The label lmgw lists Kai's tools under (`hello.hosts_label`), for the confirmation.
    pub label: RwLock<Option<String>>,
    pub confirmer: Arc<dyn Confirmer>,
    /// The current link's peer, for `notifications/tools/list_changed`, fenced by the run's
    /// generation.
    pub peer: Mutex<PeerSlot>,
    /// The link's limits as last known: `[host]`'s fallbacks, then what the latest lmgw announced
    /// (the CLI bounds a request by its message limit).
    pub limits: RwLock<LinkLimits>,
    /// The screen lock: while locked every call is refused, and a call on its way is stopped
    /// (`crate::screen_lock`).
    pub screen: crate::screen_lock::ScreenLock,
    /// K26's jobs (WP18, `super::tasks`): kept across reconnects, as lmgw follows them across links.
    pub jobs: crate::jobs::Jobs,
}

/// The current link's peer, tagged with the generation of the run that set it. The host moves the
/// generation on at every stop and start; a run sets and clears the peer only while the
/// generation is still its own, so a stopped run that ends late never wipes a newer link's peer
/// (nor sets its own over it).
pub struct PeerSlot<P = Peer<RoleServer>> {
    generation: u64,
    peer: Option<P>,
}

impl<P> Default for PeerSlot<P> {
    fn default() -> Self {
        Self {
            generation: 0,
            peer: None,
        }
    }
}

impl<P: Clone> PeerSlot<P> {
    /// Fences every run so far off (a stop, or a start's new run): their peer is gone, and the
    /// new generation is returned.
    pub fn next(&mut self) -> u64 {
        self.generation += 1;
        self.peer = None;
        self.generation
    }

    /// The peer of the run of `generation`, if that is still the current one.
    pub fn set(&mut self, generation: u64, peer: P) -> bool {
        let ours = generation == self.generation;
        if ours {
            self.peer = Some(peer);
        }
        ours
    }

    /// The run of `generation` ended: its peer goes, a newer run's stays.
    pub fn clear(&mut self, generation: u64) -> bool {
        let ours = generation == self.generation;
        if ours {
            self.peer = None;
        }
        ours
    }

    pub fn get(&self) -> Option<P> {
        self.peer.clone()
    }
}

impl Shared {
    pub fn new(
        registry: Registry,
        policy: SharedPolicy,
        confirmer: Arc<dyn Confirmer>,
        screen: crate::screen_lock::ScreenLock,
        jobs: crate::jobs::Jobs,
    ) -> Self {
        let listed = Listed::build(&registry, &policy.read().expect("never poisoned"));
        Self {
            registry,
            listed: RwLock::new(listed),
            policy,
            own_device: RwLock::default(),
            label: RwLock::default(),
            confirmer,
            peer: Mutex::new(PeerSlot::default()),
            limits: RwLock::new(LinkLimits::fallback(0, 0)),
            screen,
            jobs,
        }
    }

    /// Lists the tools again (a provider's change, a policy change). When the list changed and a
    /// link is up, lmgw gets `notifications/tools/list_changed` and lists again (K22: only then).
    /// A routine the providers no longer offer (switched off, its folder gone) ends its running
    /// jobs; a tool the policy no longer lists (K15) ends none (WP18).
    pub fn relist(&self) -> bool {
        // Built inside the write lock (order: listed, then policy), so a relister that is late
        // cannot put a list it built before another's change over that change's.
        let mut listed = self.listed.write().expect("never poisoned");
        let offered = self.registry.entries();
        self.jobs
            .routines_gone(&offered.iter().map(|e| e.spec.name).collect());
        let fresh = Listed::of(offered, &self.policy.read().expect("never poisoned"));
        if fresh.tools == listed.tools {
            return false;
        }
        tracing::info!(
            tools = fresh.tools.len(),
            was = listed.tools.len(),
            "host: the tool list changed"
        );
        *listed = fresh;
        drop(listed);
        if let Some(peer) = self.peer.lock().expect("never poisoned").get() {
            tokio::spawn(async move {
                if let Err(e) = peer.notify_tool_list_changed().await {
                    tracing::warn!("host: tools/list_changed did not go out: {e}");
                }
            });
        }
        true
    }

    pub fn tool_count(&self) -> usize {
        self.listed.read().expect("never poisoned").tools.len()
    }

    fn shown_name(&self, name: &str) -> String {
        match self.label.read().expect("never poisoned").as_deref() {
            Some(label) => format!("{label}__{name}"),
            None => name.to_owned(),
        }
    }

    /// One call after its arguments were read, the one path every call takes, lmgw's and Kai's
    /// own (the routines' CLI, the tray; `super::local`): K15's decision for `caller` (and
    /// `approver`, F's), the tool's preparation, the owner's confirmation (PD4) and the handler,
    /// each step ending early when `bounds.cancel` fires or `bounds.deadline` passes. While the
    /// screen is locked the call is refused, and a lock while it is on its way stops it
    /// (`crate::screen_lock`).
    pub async fn dispatch(
        &self,
        entry: &Entry,
        args: Args,
        caller: &Caller,
        approver: Option<&Caller>,
        bounds: Bounds,
    ) -> Result<Reply, Stop> {
        let spec = entry.spec;
        let tool = spec.name;
        let Bounds {
            cancel: ct,
            deadline,
            timeout_ms,
            room,
            room_says,
            out,
        } = bounds;
        let waits = || timeout_ms.unwrap_or_default().div_ceil(1000);
        if ct.is_cancelled() {
            return Err(Stop::Cancelled("before it ran"));
        }
        if self.screen.is_locked() {
            tracing::info!(tool, caller = %caller.describe(), "host: refused: the screen is locked");
            return Err(Stop::Refused(crate::screen_lock::REFUSAL.to_owned()));
        }
        let locked = |when: &str| {
            tracing::info!(tool, when, "host: the screen locked; the call is stopped");
            Stop::Refused(format!(
                "{}; the screen locked {when}, so {tool} was stopped",
                crate::screen_lock::REFUSAL
            ))
        };
        // F's approval counts only where it may (`policy::approval_applies`: every call, K24's
        // confirmed-by-default tools included, but switching a routine on, whose dialog shows the
        // routine and offers to grant what it asks).
        let approver = approver.filter(|_| super::policy::approval_applies(spec, &args));
        let origin = Origin {
            approved_in_lmgw: approver.is_some(),
            caller: Some(caller.clone()),
        };
        let decision = self
            .policy
            .read()
            .expect("never poisoned")
            .decide_call(spec, &args, caller, approver);
        if let Decision::Deny(why) = &decision {
            tracing::info!(tool, caller = %caller.describe(), %why, "host: denied (K15)");
            return Err(Stop::Refused(why.clone()));
        }
        let until_deadline = || async move {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        // What the tool resolves first (`open`'s application), shown in the dialog and run as
        // shown; its refusal asks nobody.
        let prepared = match &entry.prepare {
            None => None,
            Some(prepare) => {
                tokio::select! {
                    p = prepare(&args, origin) => match p {
                        Ok(p) => Some(p),
                        Err(why) => {
                            tracing::info!(tool, "host: refused before the confirmation (the tool's own check)");
                            return Err(Stop::Refused(why));
                        }
                    },
                    () = ct.cancelled() => return Err(Stop::Cancelled("before it ran")),
                    () = self.screen.locked() => return Err(locked("before it ran")),
                    () = until_deadline() => {
                        return Err(Stop::Refused(format!(
                            "{tool} could not be prepared within lmgw's wait ({} s, \
                             lmgw/timeout_ms); it did not run",
                            waits()
                        )));
                    }
                }
            }
        };
        match decision {
            Decision::Deny(_) => unreachable!("refused above"),
            Decision::Allow => {}
            Decision::Confirm { why } => {
                tracing::info!(tool, caller = %caller.describe(), why, "host: asking the owner (PD4)");
                let (reply, answer) = oneshot::channel();
                let std_deadline = deadline.map(|d| {
                    std::time::Instant::now() + d.saturating_duration_since(Instant::now())
                });
                let mut lines = args.lines(spec);
                if let Some(p) = &prepared {
                    lines.extend(p.lines.iter().cloned());
                }
                self.confirmer.ask(ConfirmRequest {
                    tool: self.shown_name(tool),
                    description: spec.description.to_owned(),
                    caller: caller.describe(),
                    lines,
                    destructive: spec.hints.destructive,
                    deadline: std_deadline,
                    options: prepared
                        .as_ref()
                        .map(|p| p.options.clone())
                        .unwrap_or_default(),
                    notes: prepared
                        .as_ref()
                        .map(|p| p.notes.clone())
                        .unwrap_or_default(),
                    reply,
                });
                // Dropping `answer` (any arm but the first) closes the dialog.
                let answered = tokio::select! {
                    a = answer => a.ok(),
                    () = ct.cancelled() => return Err(Stop::Cancelled("while the owner was asked")),
                    () = self.screen.locked() => return Err(locked("while the owner was asked")),
                    () = until_deadline() => {
                        tracing::info!(tool, "host: the owner did not answer in time; the call did not run");
                        return Err(Stop::Refused(format!(
                            "the owner did not confirm the call in time (lmgw waits {} s, \
                             lmgw/timeout_ms); it did not run",
                            waits()
                        )));
                    }
                };
                match answered {
                    Some(true) => tracing::info!(tool, "host: the owner allowed the call"),
                    Some(false) => {
                        tracing::info!(tool, "host: the owner denied the call");
                        return Err(Stop::Refused(format!(
                            "the owner denied this call of {tool}"
                        )));
                    }
                    None => {
                        tracing::warn!(
                            tool,
                            "host: the confirmation could not be shown; the call did not run"
                        );
                        return Err(Stop::Refused(format!(
                            "Kai could not ask the owner about this call of {tool} (its dialog \
                             cannot be shown); it did not run"
                        )));
                    }
                }
            }
        }
        if ct.is_cancelled() {
            return Err(Stop::Cancelled("before it ran"));
        }
        if self.screen.is_locked() {
            return Err(locked("before it ran"));
        }
        let call_ct = ct.child_token();
        let call = Call {
            spec,
            args,
            caller: caller.clone(),
            // A child of lmgw's token: the lock cancels it too, as lmgw's cancel would.
            cancel: call_ct.clone(),
            deadline,
            timeout_ms,
            room,
            room_says,
            prepared,
            out,
        };
        let started = Instant::now();
        let reply = tokio::select! {
            r = (entry.handler)(call) => r,
            () = ct.cancelled() => return Err(Stop::Cancelled("while it ran; it was stopped")),
            () = self.screen.locked() => {
                call_ct.cancel();
                return Err(locked("while it ran"));
            }
        };
        tracing::info!(
            tool,
            caller = %caller.describe(),
            error = reply.error,
            ms = started.elapsed().as_millis() as u64,
            "host: answered"
        );
        Ok(reply)
    }
}

/// One link run's server.
#[derive(Clone)]
pub struct Server {
    pub shared: Arc<Shared>,
    /// This link's limits: `[host]`'s fallbacks until lmgw's `initialize` announces its own (the
    /// pump reads the frame limit here).
    pub limits: Arc<RwLock<LinkLimits>>,
}

fn cancelled(tool: &str, caller: &Caller, when: &str) -> Result<CallToolResult, McpError> {
    tracing::info!(tool, caller = %caller.describe(), "host: lmgw cancelled the call {when}");
    Err(McpError::new(
        CANCELLED,
        format!("the call of {tool} was cancelled"),
        None,
    ))
}

fn refused(text: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
}

/// A result that would be a larger message than the link takes becomes the call's error naming the
/// limit (the one place every tool's result passes), so it never closes the link.
pub(super) fn within(
    tool: &str,
    result: CallToolResult,
    id: &rmcp::model::RequestId,
    limits: &LinkLimits,
) -> CallToolResult {
    let Some(max) = limits.message else {
        return result;
    };
    let len = answer_len(id, &result);
    if len <= max {
        return result;
    }
    tracing::warn!(
        tool,
        bytes = len,
        max,
        "host: the result is larger than the link's message limit; the call answers with an error"
    );
    CallToolResult::error(vec![ContentBlock::text(format!(
        "the result of {tool} was {len} bytes as a message, more than the tool link takes (the \
         message limit is {}); it was not sent",
        limits.message_says()
    ))])
}

pub(super) fn result(reply: Reply) -> CallToolResult {
    let content = reply
        .blocks
        .into_iter()
        .map(|b| match b {
            Block::Text(t) => ContentBlock::text(t),
            Block::Image { mime, base64 } => ContentBlock::image(base64, mime),
        })
        .collect();
    if reply.error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_tasks_with(super::tasks::capability())
                .build(),
        );
        info.server_info = Implementation::new("kai", crate::VERSION);
        info
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // lmgw's announcement, wherever rmcp put the `_meta`.
        let mut meta = context.meta.0.clone();
        if let Some(m) = &request.meta {
            for (k, v) in &m.0 {
                meta.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let announced = HostLimits::from_meta(&meta);
        {
            let mut limits = self.limits.write().expect("never poisoned");
            match &announced {
                Some(h) => {
                    *limits = LinkLimits::announced(h, &limits);
                    tracing::info!(
                        max_message_bytes = h.max_message_bytes,
                        max_frame_bytes = h.max_frame_bytes,
                        ping_interval_s = h.ping_interval_s,
                        call_timeout_ms = h.call_timeout_ms,
                        "host: lmgw announced the link's limits"
                    );
                }
                None => tracing::info!(
                    message = ?limits.message,
                    frame = ?limits.frame,
                    "host: lmgw announced no limits; host.fallback_max_message_mb and \
                     host.fallback_max_frame_mb apply"
                ),
            }
            *self.shared.limits.write().expect("never poisoned") = *limits;
        }
        context.peer.set_peer_info(request);
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self
            .shared
            .listed
            .read()
            .expect("never poisoned")
            .tools
            .clone();
        tracing::info!(tools = tools.len(), "host: lmgw lists the tools");
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let a = match self.arrive(request, &context) {
            Ok(a) => a,
            Err(answer) => return answer,
        };
        let tool = a.entry.spec.name;
        let limits = a.limits;
        let reply = match self
            .shared
            .dispatch(
                &a.entry,
                a.args,
                &a.caller,
                a.approver.as_ref(),
                Bounds {
                    cancel: context.ct.clone(),
                    deadline: a.deadline,
                    timeout_ms: a.meta.timeout_ms,
                    room: limits.text_room(&context.id),
                    room_says: limits.message_says(),
                    out: None,
                },
            )
            .await
        {
            Ok(reply) => reply,
            Err(Stop::Cancelled(when)) => return cancelled(tool, &a.caller, when),
            Err(Stop::Refused(why)) => return refused(why),
        };
        Ok(within(tool, result(reply), &context.id, &limits))
    }

    // K26's open-ended class as MCP tasks (WP18, `super::tasks`).

    async fn enqueue_task(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CreateTaskResult, McpError> {
        super::tasks::enqueue(self, request, context).await
    }

    async fn get_task_info(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        super::tasks::get(&self.shared, &request.task_id)
    }

    async fn get_task_result(
        &self,
        request: GetTaskPayloadParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskPayloadResult, McpError> {
        super::tasks::result(self, &request.task_id, &context).await
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CancelTaskResult, McpError> {
        super::tasks::cancel(&self.shared, &request.task_id)
    }
}

/// A call as it arrived, read: its entry, arguments, caller and approver, lmgw's stamp, its
/// deadline and the link's limits (`call_tool` and `enqueue_task` alike).
pub(super) struct Arrived {
    pub entry: Entry,
    pub args: Args,
    pub caller: Caller,
    pub approver: Option<Caller>,
    pub meta: CallMeta,
    pub deadline: Option<Instant>,
    pub limits: LinkLimits,
}

impl Server {
    /// Reads a call (a tool Kai offers, lmgw's stamp, the arguments); a call that does not read is
    /// answered with `Err`.
    #[allow(clippy::result_large_err)]
    pub(super) fn arrive(
        &self,
        request: CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<Arrived, Result<CallToolResult, McpError>> {
        let arrived = Instant::now();
        let s = &self.shared;
        let name = request.name.to_string();
        let Some(entry) = s.listed.read().expect("never poisoned").entry(&name) else {
            tracing::warn!(tool = %name, "host: a call for a tool Kai does not offer");
            return Err(Err(McpError::invalid_params(
                format!("Kai offers no tool '{name}'"),
                None,
            )));
        };
        let spec = entry.spec;
        let tool = spec.name;
        // lmgw's stamp (api §5.5), wherever rmcp put it: the request context's meta, or the
        // params' own `_meta`.
        let mut meta = context.meta.0.clone();
        if let Some(m) = &request.meta {
            for (k, v) in &m.0 {
                meta.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let meta = CallMeta::from_meta(&meta);
        let own = s.own_device.read().expect("never poisoned").clone();
        let caller = Caller::from_principal(meta.caller.as_ref(), own.as_deref());
        let approver = meta
            .approval
            .as_ref()
            .filter(|a| a.decision == "approved")
            .map(|a| Caller::from_principal(Some(&a.by), own.as_deref()));
        tracing::info!(
            tool,
            caller = %caller.describe(),
            approved_by = approver.as_ref().map(Caller::describe),
            timeout_ms = meta.timeout_ms,
            task = request.task.is_some(),
            "host: a call"
        );
        let args = match Args::read(spec, request.arguments) {
            Ok(a) => a,
            Err(why) => {
                tracing::info!(
                    tool,
                    "host: refused: the arguments do not fit the tool's parameters"
                );
                return Err(refused(why));
            }
        };
        let limits = *self.limits.read().expect("never poisoned");
        let deadline = meta
            .timeout_ms
            .map(|ms| arrived + Duration::from_millis(ms));
        Ok(Arrived {
            entry,
            args,
            caller,
            approver,
            meta,
            deadline,
            limits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PeerSlot;

    /// The fence: a stopped run that ends after a newer one linked neither clears nor sets the
    /// newer one's peer.
    #[test]
    fn a_stopped_run_never_touches_a_newer_runs_peer() {
        let mut slot = PeerSlot::<&str>::default();
        let old = slot.next();
        assert!(slot.set(old, "old"));
        assert_eq!(slot.get(), Some("old"));
        // The host stops the old run and starts a new one.
        slot.next();
        let new = slot.next();
        assert_eq!(slot.get(), None, "a stop drops the old peer");
        assert!(slot.set(new, "new"));
        // The old run ends late: its clear is not ours, its late set neither.
        assert!(!slot.clear(old));
        assert!(!slot.set(old, "old again"));
        assert_eq!(slot.get(), Some("new"));
        assert!(slot.clear(new));
        assert_eq!(slot.get(), None);
    }
}
