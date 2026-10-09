//! MCP Apps views for the tool calls of Kai's own conversation (decided by the owner, 2026-10-09;
//! §10.3, Q4): a call whose tool names a UI resource shows that view by itself, and the view gets
//! the call's input and result as MCP Apps' host does (SEP-1865, revision
//! [`lmgw_client::mcp_apps::REVISION`]).
//!
//! **Where the calls come from.** Kai's turns run in its bound realtime session, which relays the
//! Chat turn's frames as `lmgw.chat.frame` (`crate::voice`'s one-line hook, [`frame`]): a `tool`
//! frame's `ready` carries the call's name, arguments, `call_id`, `server_label`, `needs_approval`
//! and `ui_resource`, its `result` the MCP result's `content` blocks as the server sent them,
//! `structured_content`, `is_error`, `call_id`, `server_label` and `ui_resource` (lmgw's
//! client-apps §7.6; `lmgw_client::mcp_apps::{ToolReadyFrame, ToolResultFrame}`), and the turn
//! ends with `done` (after a `stop` when it was stopped). An lmgw before 2026-10-09 sent the
//! `ready` with name and arguments only and the `result` with the text alone: the steps below say
//! what Kai does then. Turns of other principals in Kai's thread (the phone, the dashboard) are
//! not relayed to Kai's session (lmgw: live mirroring of another client's turn is Later), so their
//! calls show nothing here.
//!
//! **The flow** (`[overlay.app] auto_show`, on by default):
//! 1. At `ready`, a call whose frame names its view (`server_label`, `ui_resource`) and does not
//!    wait for an approval has that view read and shown at once, while the tool runs: the spec's
//!    order (the host renders the view beside the call). A `ready` that says `needs_approval:
//!    true` shows nothing yet; one that names no view shows nothing (lmgw names every view there
//!    is). From an older lmgw (no `needs_approval`), Kai asks `/mcp` (under its key, a session of
//!    the view's own) for the tool list and shows the view its `_meta.ui.resourceUri` names; the
//!    list as last fetched answers for a tool without a view (`app_bridge::lists`). A tool of
//!    Kai's own contract (`kai__overlay_text`) is neither looked up nor shown at `ready` (its
//!    result frame still counts).
//! 2. Once the view said `ui/notifications/initialized` **and Kai knows the call runs** (a `ready`
//!    that says `needs_approval: false`, else its `result` frame: a call may wait for an
//!    approval, `flow`), it gets `ui/notifications/tool-input` with the frame's arguments, then
//!    `…/tool-result` (MCP's `CallToolResult`: the frame's `content` blocks, `structuredContent`,
//!    `isError`; from an older lmgw the text as one block) or `…/tool-cancelled` when the call
//!    ended without its result (`stop`, an aborted turn, a decision in another client, the voice
//!    session ending), with the reason; a call that never ran gets the cancel alone.
//! 3. When neither the `ready` nor the list named a view but the result frame does, the view is
//!    shown then, with the input and the result at once.
//!
//! Calls are told apart by their response and index, and found again across responses (a held
//! call, a resumed turn) by `call_id`, kept verbatim (`flow`).
//!
//! A view shown here is placed by `[overlay.app]` like `overlay_app`'s, answers its bridge's rules
//! (K18, K15 as `app:<server>`), and replaces the one shown here before it (one at a time: they
//! would cover each other at the same place; the newest call's stays, whichever show finishes
//! last, and none looked for in an ended voice session shows); `overlay_app`'s own shows are never
//! replaced. Its `overlay_list` line says which call it was shown for. Nothing of the call's
//! arguments or result is logged.

mod flow;

use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::{oneshot, watch};

pub use self::flow::{Flow, Start};
use super::app_bridge::{Apps, CallView, Outcome, Progress};
use super::gtk_host::WidgetsCmd;
use super::kinds::{Body, Kind};
use crate::config::OverlayConfig;
use crate::ui::UiCmd;
use serde_json::Value;

/// What the flow needs to show a view: set by the widgets' provider ([`set`]).
pub struct Ctx {
    pub apps: Arc<Apps>,
    pub ui: async_channel::Sender<UiCmd>,
    pub overlay: Arc<OverlayConfig>,
}

#[derive(Default)]
struct State {
    flow: Flow,
    ctx: Option<Arc<Ctx>>,
    /// The view shown here last, `(its Start's seq, its widget)`: the one a newer one replaces.
    shown: Option<(u64, u64)>,
    /// Kai's voice sessions ended so far: a view looked for in an earlier one is not shown.
    generation: u64,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The widgets' provider hands over what a view needs (each start of the host).
pub fn set(ctx: Ctx) {
    state().ctx = Some(Arc::new(ctx));
}

/// A frame of Kai's bound session (`lmgw.chat.frame`), from `crate::voice`.
pub fn frame(response: &str, event: &str, data: &Value) {
    if !matches!(event, "tool" | "stop" | "done") {
        return;
    }
    let (start, ctx, generation) = {
        let mut st = state();
        let Some(ctx) = st.ctx.clone() else { return };
        let auto = ctx.overlay.app.auto_show == Some(true);
        (
            st.flow.frame(response, event, data, auto),
            ctx,
            st.generation,
        )
    };
    let Some(start) = start else { return };
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(follow(ctx, start, generation));
        }
        Err(_) => tracing::warn!("app view: a call's view needs Kai's runtime; not shown"),
    }
}

/// `lmgw.approval.decided` of Kai's bound session, from `crate::voice`: another client decided a
/// call Kai's turn waited on ([`Flow::decided`]).
pub fn approval_decided(id: &str, call_id: Option<&str>, approve: bool, by: Option<&str>) {
    state().flow.decided(id, call_id, approve, by);
}

/// Kai's voice session ended, from `crate::voice`: its calls are cancelled, a view still being
/// looked for is not shown, and `/mcp`'s tool list is fetched afresh next time.
pub fn session_ended() {
    let mut st = state();
    st.flow.session_ended();
    st.generation += 1;
    if let Some(ctx) = &st.ctx {
        ctx.apps.lists.forget();
    }
}

/// Whether the view of Start `seq`, looked for in session `generation`, may show: its session is
/// still Kai's, and no newer call's view is shown.
fn may_show(st: &State, seq: u64, generation: u64) -> bool {
    st.generation == generation && st.shown.is_none_or(|(s, _)| s < seq)
}

/// Hides widget `id` (a view replaced, or one that came too late).
async fn hide(ctx: &Ctx, id: u64) -> bool {
    let (reply, answer) = oneshot::channel();
    let hide = WidgetsCmd::Hide {
        id: Some(id),
        reply: Some(reply),
    };
    ctx.ui.send(UiCmd::Widgets(hide)).await.is_ok()
        && matches!(answer.await, Ok(Ok(hidden)) if !hidden.is_empty())
}

/// Looks for the call's view and shows it (module doc).
async fn follow(ctx: Arc<Ctx>, start: Start, generation: u64) {
    let look = match super::tools::look(Kind::App, None, &ctx.overlay) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("app view: {e}");
            return;
        }
    };
    let size = super::tools::app_size(&look);
    let Start {
        name,
        arguments,
        call_id,
        known,
        look_up,
        mut progress,
        seq,
    } = start;
    let call = |known, progress: &watch::Receiver<Progress>| CallView {
        name: name.clone(),
        arguments: arguments.clone(),
        call_id: call_id.clone(),
        progress: progress.clone(),
        known,
    };
    let mut opened = Ok(None);
    if known.is_some() || look_up {
        opened = ctx
            .apps
            .open_for_call(call(known.clone(), &progress), size)
            .await;
    }
    match &opened {
        // The frame named the view: it should have opened.
        Err(e) if known.is_some() => {
            tracing::warn!(tool = name, "app view: the call's view was not opened: {e}")
        }
        Err(e) => tracing::debug!(tool = name, "app view: the call's tool list: {e}"),
        Ok(_) => {}
    }
    if !matches!(opened, Ok(Some(_))) {
        // The result frame names the view, if it has one (and not the one that just failed).
        let named = match progress.wait_for(|p| p.outcome.is_some()).await {
            Ok(p) => match &p.outcome {
                Some(Outcome::Result { ui: Some(ui), .. }) => Some(ui.clone()),
                _ => None,
            },
            Err(_) => None,
        };
        let Some(named) = named.filter(|n| known.as_ref() != Some(n)) else {
            return;
        };
        opened = ctx
            .apps
            .open_for_call(call(Some(named), &progress), size)
            .await;
    }
    let mut body = match opened {
        Ok(Some(b)) => b,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(tool = name, "app view: the call's view was not opened: {e}");
            return;
        }
    };
    if !may_show(&state(), seq, generation) {
        // Dropped: its bridge ends, and with it its /mcp session.
        tracing::debug!(
            tool = name,
            "app view: a newer call's view is shown, or Kai's voice session ended; not shown"
        );
        return;
    }
    body.notes
        .insert(0, format!("shown for {name} in Kai's conversation"));
    let shown = match super::tools::present(&ctx.ui, Kind::App, Body::App(body), look).await {
        Ok(shown) => shown,
        Err(e) => {
            tracing::warn!(tool = name, "app view: the call's view: {e}");
            return;
        }
    };
    // Shows finish out of order: the newest call's view stays, whichever showed last.
    let replaced = {
        let mut st = state();
        if may_show(&st, seq, generation) {
            Ok(st.shown.replace((seq, shown.id)).map(|(_, id)| id))
        } else {
            Err(())
        }
    };
    match replaced {
        Ok(before) => {
            tracing::info!(
                tool = name,
                widget = shown.id,
                "app view: shown for a call in Kai's conversation"
            );
            if let Some(old) = before
                && hide(&ctx, old).await
            {
                tracing::debug!(widget = old, "app view: replaced by the next call's");
            }
        }
        Err(()) => {
            hide(&ctx, shown.id).await;
            tracing::debug!(
                widget = shown.id,
                "app view: hidden at once, a newer call's view showed first (or the session ended)"
            );
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod call_id_tests;
