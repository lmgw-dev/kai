//! The calls followed, driven by the frames of Kai's bound session: pure bookkeeping (the module
//! doc of `super` says the flow; this says the rules for a call that waits for an approval).
//!
//! **The frames** are read as lmgw types them (`lmgw_client::mcp_apps::{ToolReadyFrame,
//! ToolResultFrame}`). A call is the `(response, index)` its frames carry; its `call_id` (the
//! model's id, verbatim: Gemini's signed ids run to hundreds of characters, never cut) is what
//! finds it again across responses (a held call, a resumed turn). A frame of an lmgw before
//! 2026-10-09 carries neither `call_id`, `server_label`, `ui_resource` nor `needs_approval` on its
//! `ready`: the tool and the arguments find a held call then, and `/mcp`'s list names the view.
//!
//! **A call that may wait for an approval gets no input before it runs.** lmgw sends a `ready`
//! frame for every call, gated or not (client-apps §6.2), then, for a turn that stops on gated
//! calls, a `tool {event: "approval"}` frame per gated call and `done` with `pending_approvals`;
//! the ungated calls beside them wait too (the model gets every result in one block). So:
//! - A `ready` that says `needs_approval: true` is followed but shows nothing yet. One that says
//!   `needs_approval: false` runs: its view gets the input at once. Without the field (an older
//!   lmgw) the view is shown, but its input waits until Kai knows the call ran: its `result`
//!   frame.
//! - An `approval` frame matches its call by the response and its `call_id` (lmgw since
//!   2026-10-09 puts the call's id on the request, as on `done`'s `pending_approvals`); without
//!   one, or when no call of the response has it, by the tool (`<server_label>__<name>`) and the
//!   arguments (a JSON string there, an object in `ready`). It marks the call with its approval
//!   id. A `done` with `pending_approvals` keeps the response's
//!   open calls as held, uncancelled.
//! - A held call ends at `lmgw.approval.decided` (a decision another client made; that client's
//!   request runs the resumed turn, which Kai's session does not see): declined, or approved
//!   there, its view gets `tool-cancelled` saying so, without the input. Once a turn's last gated
//!   call is decided this way, its other held calls end the same way.
//! - A resumed turn that does reach Kai's session (a later Kai that answers approvals itself)
//!   announces its decided calls again under a new response: a `ready` with the held call's
//!   `call_id` (else, from an older lmgw, its tool and arguments) is that call (its view stays the
//!   one shown, no second), and a `result` whose `ready` was not seen is matched by its `call_id`
//!   (else by the tool, when one held call has it).
//! - Kai's voice session ending cancels all.

use std::collections::{HashMap, HashSet};

use lmgw_client::mcp_apps::{ToolReadyFrame, ToolResultFrame};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use super::super::app_bridge::{Outcome, Progress, RESERVED};

/// A view to look for, for one call.
pub struct Start {
    /// The tool's exposed name (`<server>__<name>`).
    pub name: String,
    pub arguments: Map<String, Value>,
    /// The call's id as lmgw's frames carry it (`hostContext.toolInfo.id`); `None` from an older
    /// lmgw.
    pub call_id: Option<String>,
    /// The `(server_label, ui_resource)` a frame of the call named: its view, read without asking
    /// `/mcp`'s list which.
    pub known: Option<(String, String)>,
    /// Ask `/mcp`'s list for the tool's view now (step 1, for an lmgw whose `ready` does not say);
    /// otherwise only [`Start::known`] or the result frame tells.
    pub look_up: bool,
    pub progress: watch::Receiver<Progress>,
    /// The order Starts were made in: a view of an older one never replaces a newer one's.
    pub seq: u64,
}

impl std::fmt::Debug for Start {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the arguments.
        write!(
            f,
            "Start({}, known: {:?}, look_up: {}, seq: {})",
            self.name, self.known, self.look_up, self.seq
        )
    }
}

/// One call followed.
struct Call {
    tx: watch::Sender<Progress>,
    /// The tool's exposed name.
    name: String,
    arguments: Map<String, Value>,
    /// The model's id for it, verbatim (never cut: it finds the call again).
    call_id: Option<String>,
    /// The view its `ready` named.
    known: Option<(String, String)>,
    /// Its approval id, once its `approval` frame (or `done`'s list) said it waits.
    approval: Option<String>,
    /// Its view was looked for (a [`Start`] went out).
    started: bool,
    /// The response that made it (its gated turn's, while held).
    origin: String,
}

impl Call {
    fn end(self, outcome: Outcome) {
        let runs = self.tx.borrow().runs;
        self.tx.send_replace(Progress::ended(runs, outcome));
    }

    /// The call a frame of `call_id` (`name`, `arguments`) announces again: by the id when both
    /// have one, else (an older lmgw) by the tool and the arguments.
    fn is(&self, call_id: Option<&str>, name: &str, arguments: &Map<String, Value>) -> bool {
        match (self.call_id.as_deref(), call_id) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => self.name == name && self.arguments == *arguments,
        }
    }
}

/// The calls followed, by `(response_id, index)`, and those held for a decision.
#[derive(Default)]
pub struct Flow {
    calls: HashMap<(String, u64), Call>,
    /// The open calls of turns that stopped on approvals.
    held: Vec<Call>,
    /// Responses that said `stop` before their `done`.
    stopped: HashSet<String>,
    seq: u64,
}

/// A tool lmgw serves itself (`lmgw__`, `docs__`, `kb__`) or one without a label has no view of
/// an MCP server's.
pub fn candidate(name: &str) -> bool {
    name.split_once("__")
        .is_some_and(|(label, own)| !own.is_empty() && !RESERVED.contains(&label))
}

/// The result frame's result as MCP's `CallToolResult`: its `content` blocks as the server sent
/// them (from an lmgw before 2026-10-09, which sends none, `output` as one text block), its
/// `structuredContent` and its `isError`.
pub fn call_tool_result(f: &ToolResultFrame) -> Value {
    let content = match &f.content {
        Some(blocks) => Value::Array(blocks.clone()),
        None if f.output.is_empty() => json!([]),
        None => json!([{"type": "text", "text": f.output}]),
    };
    let mut r = json!({"content": content, "isError": f.is_error});
    if let Some(s) = f.structured_content.as_ref().filter(|v| !v.is_null()) {
        r["structuredContent"] = s.clone();
    }
    r
}

/// The `(server_label, ui_resource)` a frame names, when it names both.
fn named_view(label: Option<&str>, ui: Option<&str>) -> Option<(String, String)> {
    Some((label?.to_owned(), ui?.to_owned()))
}

/// A frame's call id: `None` for an absent or empty one.
fn id_of(call_id: Option<String>) -> Option<String> {
    call_id.filter(|id| !id.is_empty())
}

/// An approval request (an `approval` frame, an entry of `done`'s `pending_approvals`): the
/// exposed names its tool may have (lmgw strips the label's `<prefix>__` when it is the label),
/// its arguments as an object, its id.
struct Request {
    names: [String; 2],
    arguments: Option<Map<String, Value>>,
    id: String,
    /// The call's id, when lmgw's request carries it (since 2026-10-09): it finds the call
    /// without the tool and the arguments.
    call_id: Option<String>,
}

impl Request {
    fn of(data: &Value) -> Option<Request> {
        let name = data["name"].as_str()?;
        let label = data["server_label"].as_str().unwrap_or_default();
        Some(Request {
            names: [format!("{label}__{name}"), name.to_owned()],
            arguments: match &data["arguments"] {
                Value::String(s) => serde_json::from_str::<Value>(s)
                    .ok()
                    .and_then(|v| v.as_object().cloned()),
                Value::Object(o) => Some(o.clone()),
                _ => None,
            },
            id: data["approval_request_id"].as_str()?.to_owned(),
            call_id: id_of(data["call_id"].as_str().map(str::to_owned)),
        })
    }

    fn names(&self, call: &Call) -> bool {
        self.names.contains(&call.name)
    }
}

impl Flow {
    /// One `lmgw.chat.frame`; `auto` is `[overlay.app] auto_show`. A call to look for a view for
    /// comes back.
    pub fn frame(
        &mut self,
        response: &str,
        event: &str,
        data: &Value,
        auto: bool,
    ) -> Option<Start> {
        match event {
            "tool" => self.tool(response, data, auto),
            "stop" => {
                self.stopped.insert(response.to_owned());
                None
            }
            "done" => {
                let stopped = self.stopped.remove(response);
                let pending = data["pending_approvals"]
                    .as_array()
                    .filter(|p| !p.is_empty());
                if let Some(pending) = pending
                    && !stopped
                {
                    for r in pending.iter().filter_map(Request::of) {
                        self.mark(response, &r);
                    }
                    self.hold(response);
                    return None;
                }
                let why = if stopped {
                    "the turn was stopped"
                } else if data["aborted"] == true {
                    "the turn was aborted"
                } else {
                    "the turn ended without the call's result"
                };
                self.cancel(|(r, _)| r == response, why);
                None
            }
            _ => None,
        }
    }

    fn start(&mut self, call: &Call, look_up: bool) -> Start {
        self.seq += 1;
        Start {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            call_id: call.call_id.clone(),
            known: call.known.clone(),
            look_up,
            progress: call.tx.subscribe(),
            seq: self.seq,
        }
    }

    fn tool(&mut self, response: &str, data: &Value, auto: bool) -> Option<Start> {
        match data["event"].as_str()? {
            "approval" => {
                if let Some(r) = Request::of(data) {
                    self.mark(response, &r);
                }
                None
            }
            "ready" if auto => match ToolReadyFrame::deserialize(data) {
                Ok(f) => self.ready(response, f),
                Err(e) => {
                    tracing::debug!("app view: a ready frame that does not read: {e}");
                    None
                }
            },
            "result" => match ToolResultFrame::deserialize(data) {
                Ok(f) => self.result(response, f, auto),
                Err(e) => {
                    tracing::debug!("app view: a result frame that does not read: {e}");
                    None
                }
            },
            _ => None,
        }
    }

    fn ready(&mut self, response: &str, f: ToolReadyFrame) -> Option<Start> {
        let key = (response.to_owned(), f.index as u64);
        let name = f.name.as_str();
        if !candidate(name) || self.calls.contains_key(&key) {
            return None;
        }
        let arguments = f.arguments.as_object().cloned().unwrap_or_default();
        let call_id = id_of(f.call_id);
        let own = name.split_once("__").map(|(_, o)| o).unwrap_or_default();
        let kais = kai_desktop::contract::find(own).is_some();
        // The view the frame names (an lmgw since 2026-10-09; never for Kai's own tools). An lmgw
        // that says `needs_approval` names every view there is, so only an older one's call asks
        // `/mcp`'s list.
        let known =
            named_view(f.server_label.as_deref(), f.ui_resource.as_deref()).filter(|_| !kais);
        let look_up = !kais && known.is_none() && f.needs_approval.is_none();
        // A resumed turn's decided call, announced again: the call held, not a new one. Its
        // verdict is not in the frame (lmgw says `needs_approval: false` for every decided call),
        // so its input still waits for its result.
        if let Some(i) = self
            .held
            .iter()
            .position(|c| c.is(call_id.as_deref(), name, &arguments))
        {
            let mut call = self.held.remove(i);
            if call.known.is_none() {
                call.known = known;
            }
            let start = (!call.started).then(|| {
                call.started = true;
                self.start(&call, look_up)
            });
            self.calls.insert(key, call);
            return start;
        }
        let needs = f.needs_approval;
        let (tx, _) = watch::channel(Progress {
            runs: needs == Some(false),
            outcome: None,
        });
        let call = Call {
            tx,
            name: name.to_owned(),
            arguments,
            call_id,
            known,
            approval: None,
            started: needs != Some(true),
            origin: response.to_owned(),
        };
        let start = call.started.then(|| self.start(&call, look_up));
        self.calls.insert(key, call);
        start
    }

    fn result(&mut self, response: &str, f: ToolResultFrame, auto: bool) -> Option<Start> {
        let ui = named_view(f.server_label.as_deref(), f.ui_resource.as_deref());
        let call_id = id_of(f.call_id.clone());
        let name = f.name.clone();
        let outcome = Outcome::Result {
            result: call_tool_result(&f),
            ui: ui.clone(),
        };
        let found = self
            .calls
            .remove(&(response.to_owned(), f.index as u64))
            .or_else(|| {
                // A held call's result whose `ready` was not seen: the held call of that id, else
                // (an older lmgw) the one held call of that tool.
                let at = match &call_id {
                    Some(id) => self
                        .held
                        .iter()
                        .position(|c| c.call_id.as_deref() == Some(id)),
                    None => {
                        let mut same = self.held.iter().enumerate().filter(|(_, c)| c.name == name);
                        match (same.next(), same.next()) {
                            (Some((i, _)), None) => Some(i),
                            _ => None,
                        }
                    }
                };
                at.map(|i| self.held.remove(i))
            });
        if let Some(mut call) = found {
            let start = (auto && !call.started && ui.is_some()).then(|| {
                call.started = true;
                call.known = ui.clone();
                self.start(&call, false)
            });
            call.end(outcome);
            return start;
        }
        // A result whose ready was not seen (Kai bound mid-turn): only its own word.
        if !auto || !candidate(&name) || ui.is_none() {
            return None;
        }
        let (tx, _) = watch::channel(Progress::ended(true, outcome));
        let call = Call {
            tx,
            name,
            arguments: Map::new(),
            call_id,
            known: ui,
            approval: None,
            started: true,
            origin: response.to_owned(),
        };
        Some(self.start(&call, false))
    }

    /// Request `r` of `response` waits for an approval: its call (the tool, then the arguments
    /// when they read) gets its id.
    fn mark(&mut self, response: &str, r: &Request) {
        // By the call's id when the request carries it and a call of the response has it (the
        // same request is marked twice, by its frame and by `done`: the same call both times).
        if let Some(id) = &r.call_id
            && let Some(call) = self
                .calls
                .iter_mut()
                .find(|((resp, _), c)| resp == response && c.call_id.as_deref() == Some(id))
                .map(|(_, c)| c)
        {
            call.approval = Some(r.id.clone());
            return;
        }
        // Else (an lmgw before 2026-10-09, or a call whose `ready` carried no id) by the tool and
        // the arguments.
        let mut open: Vec<_> = self
            .calls
            .iter_mut()
            .filter(|((resp, _), c)| resp == response && c.approval.is_none() && r.names(c))
            .collect();
        open.sort_by_key(|((_, i), _)| *i);
        let at = open
            .iter()
            .position(|(_, c)| r.arguments.as_ref() == Some(&c.arguments))
            .or((!open.is_empty()).then_some(0));
        if let Some(at) = at {
            open[at].1.approval = Some(r.id.clone());
        }
    }

    /// `response`'s turn stopped on approvals: its open calls wait, held.
    fn hold(&mut self, response: &str) {
        let keys: Vec<_> = self
            .calls
            .keys()
            .filter(|(r, _)| r == response)
            .cloned()
            .collect();
        let mut held: Vec<_> = keys
            .into_iter()
            .filter_map(|k| self.calls.remove(&k).map(|c| (k.1, c)))
            .collect();
        held.sort_by_key(|(i, _)| *i);
        self.held.extend(held.into_iter().map(|(_, c)| c));
    }

    /// `lmgw.approval.decided`: another client decided the call waiting under `id`; `by` names
    /// who (`None`: nobody, its reply went). The resumed turn runs in that client's request.
    ///
    /// The call is found by the approval id its `approval` frame marked it with; failing that (the
    /// frame was not matched), by `call_id`, the model's id the approval card learned (lmgw since
    /// 2026-10-09).
    pub fn decided(&mut self, id: &str, call_id: Option<&str>, approve: bool, by: Option<&str>) {
        let Some(i) = self
            .held
            .iter()
            .position(|c| c.approval.as_deref() == Some(id))
            .or_else(|| {
                let id = call_id.filter(|id| !id.is_empty())?;
                self.held
                    .iter()
                    .position(|c| c.call_id.as_deref() == Some(id))
            })
        else {
            return;
        };
        let call = self.held.remove(i);
        let origin = call.origin.clone();
        let why = match (approve, by) {
            (false, Some(by)) => format!("the call was declined by {by}"),
            (false, None) => "the call never runs (its reply was edited or deleted)".to_owned(),
            (true, by) => format!(
                "the call was approved by {}; it runs in that client's turn, whose result Kai \
                 does not see",
                by.unwrap_or("another client")
            ),
        };
        call.end(Outcome::Cancelled(why));
        // The turn's last gated call decided: the calls held beside it go on there too.
        if !self
            .held
            .iter()
            .any(|c| c.origin == origin && c.approval.is_some())
        {
            let (gone, kept): (Vec<_>, Vec<_>) =
                self.held.drain(..).partition(|c| c.origin == origin);
            self.held = kept;
            for c in gone {
                c.end(Outcome::Cancelled(
                    "its turn was decided in another client, which runs it; Kai does not see \
                     its result"
                        .into(),
                ));
            }
        }
    }

    /// Every call followed that `which` picks ends without its result.
    fn cancel(&mut self, which: impl Fn(&(String, u64)) -> bool, why: &str) {
        let keys: Vec<_> = self.calls.keys().filter(|k| which(k)).cloned().collect();
        for k in keys {
            if let Some(c) = self.calls.remove(&k) {
                c.end(Outcome::Cancelled(why.to_owned()));
            }
        }
    }

    /// Kai's voice session ended: no frame of its calls comes any more.
    pub fn session_ended(&mut self) {
        let why = "Kai's voice session ended";
        self.cancel(|_| true, why);
        for c in self.held.drain(..) {
            c.end(Outcome::Cancelled(why.into()));
        }
        self.stopped.clear();
    }

    /// Calls followed now, held ones included (the tests).
    #[cfg(test)]
    pub fn following(&self) -> usize {
        self.calls.len() + self.held.len()
    }

    /// `(call_id, approval id)` of every call followed or held, by call id then approval (the
    /// tests).
    #[cfg(test)]
    pub fn marks(&self) -> Vec<(Option<String>, Option<String>)> {
        let mut m: Vec<_> = self
            .calls
            .values()
            .chain(&self.held)
            .map(|c| (c.call_id.clone(), c.approval.clone()))
            .collect();
        m.sort();
        m
    }
}
