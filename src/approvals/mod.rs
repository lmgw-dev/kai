//! Approvals (WP19; §11.1, K17, api §6, PD4's hand-over): a call that waits for the owner's
//! approval shows as a card over the desktop, and the owner answers by click, by Enter and Esc, or,
//! for a call of Kai's own conversation, by voice.
//!
//! **Where calls come from.** The feed's `approval.requested` (every thread the device sees) and
//! the bound session's `mcp_approval_request` items, one call by its `approval_request_id` however
//! many ways it came. The feed's `approval.decided`, the session's `lmgw.approval.decided`, a
//! thread deleted, and the gateway left (a re-pair, a revocation) take calls away. Calls that
//! waited before Kai's feed started are not shown (the feed starts with no cursor). A feed
//! `resync` reads the waiting calls of every thread Kai knows calls of, and of Kai's own
//! conversation, again (`GET /chat/api/threads/{id}`'s `pending_approvals`) and rebuilds them.
//!
//! **The card** ([`card`], [`overlay`]) shows one call at a time, the oldest first: the tool as
//! the thread names it (`<label>__<name>`), who asked (the starter of the thread's last turn, from
//! the feed's `turn.started`, by kind: "device 'phone'", never a bare name) and where, the
//! arguments in K22's compact lines, and "call 1 of 2" when the reply waits on more than one.
//! lmgw needs a verdict for every call of a reply at once (api §6.3), and every waiting call of a
//! thread is its last reply's, so the verdicts of a thread are kept until its last call has one,
//! then sent together. A card that cannot show (no layer-shell, its page ended twice) says so in
//! the tooltip and a notification; the call is the owner's to answer in lmgw.
//!
//! **Where the verdicts go.** Through the bound session when it is bound to the thread and showed
//! every one of its calls: an `mcp_approval_response` each, then `response.create`, so the resumed
//! reply is spoken in the session (§11.1, K17). Otherwise through `POST
//! /chat/api/threads/{id}/approvals` with Kai's device key, whose resumed turn streams to Kai's
//! request and is read to its end (a request that stopped reading would stop the turn). Kai
//! approves as its own device: lmgw refuses an approval beyond the device's reach (403
//! `approval_out_of_scope`) and the card says so; declining stays open. Sent verdicts keep their
//! calls until lmgw took them: the route's answer, or for the session its `approval.decided`
//! (feed or session) or the `completed` end of the response that carried them. The session's
//! `response.create` carries an `event_id` (`kai_approvals_<n>`), and an `error` echoing it is
//! lmgw's refusal of it, whatever its code (an active response, the GPU hold, a model's error);
//! so is an `error` with an approval code. A refusal, a response that ended otherwise (failed,
//! cancelled, incomplete): the thread's waiting calls are read again, and what still waits is
//! asked again. A session that ended first sends its verdicts by the route instead, where lmgw's
//! `approval_decided` means it took them in the session.
//!
//! **The spoken answer (K17)** is for **Kai's own calls** only: a call its bound session showed,
//! or one in the conversation's thread. While one of those waits, Kai sends the session no audio
//! (no append, no commit: a spoken "yes" would become a user message, which declines the call),
//! nor while its verdict went through the session and lmgw has not created the response that
//! carries it (a turn committed then would be taken for it). A talk-key hold that began while
//! the gate was up stays silent and commits nothing to its release, also when the gate lifts
//! meanwhile. A call an ended session showed is Kai's own only through the conversation's thread.
//! A press of the talk key records the answer only when the card shows such a call and the card
//! said it armed for it ([`card::CardEvent::Armed`]); any other press is the talk key's ordinary
//! press. The recording must hold speech (`approvals.speech_dbfs`, `approvals.speech_ms`), goes
//! through `…/transcribe` of the call's thread, and its words are matched against `approvals.yes`
//! and `approvals.no` ([`words`]). An answer in neither list keeps the card and says "say yes or
//! no, or click". Calls of other threads (a phone's turn, the dashboard's) show on the card and
//! are answered there; they hold back no audio and leave the talk key as it is.
//!
//! **K15 (PD4's hand-over).** A thread call approved here reaches Kai's own tools with
//! `_meta["lmgw/approval"]` naming Kai's device as approver, a trusted approver: Kai's dialog
//! does not ask again (`host::policy`), K24's confirmed-by-default tools included (a call approved
//! in lmgw names its window, so the card's arguments show the target; a draft),
//! only `routine_enable` switching a routine on asks again. Calls with no F keep the dialog.

pub mod card;
pub mod keyboard;
pub mod overlay;
pub mod settings;
pub mod words;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use lmgw_client::requests::approvals::{ApprovalDecision, ApprovalRequest, ApprovalsRequest, code};
use tokio::sync::mpsc;

use crate::voice::link::{Pending, Refusal};
use card::{CardCmd, CardEvent, CardModel, Look};

/// What the approvals need from the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalSettings {
    pub yes: Vec<String>,
    pub no: Vec<String>,
    /// A 20 ms frame of the spoken answer counts as speech at this RMS level or above (dBFS).
    pub speech_dbfs: i32,
    /// The spoken answer needs at least this much speech, in ms, or it is not sent.
    pub speech_ms: u64,
    pub look: Look,
}

impl ApprovalSettings {
    pub fn from_config(c: &crate::config::Config) -> Self {
        Self {
            yes: c.approvals.yes.clone(),
            no: c.approvals.no.clone(),
            speech_dbfs: c.approvals.speech_dbfs,
            speech_ms: c.approvals.speech_ms,
            look: Look {
                anchor: c.approvals.anchor,
                margin: c.approvals.margin,
                width: c.approvals.width,
                arm: c.approvals.arm(),
                output: c.overlay.output.clone(),
            },
        }
    }
}

/// The feed's `approval.requested`: a call of `thread_id` waits. Its `Debug` leaves the arguments
/// out, so no log line can carry them.
#[derive(Clone, PartialEq, Eq)]
pub struct Requested {
    pub thread_id: i64,
    pub request: ApprovalRequest,
}

impl std::fmt::Debug for Requested {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Requested")
            .field("thread_id", &self.thread_id)
            .field("call", &self.request.approval_request_id)
            .field("label", &self.request.server_label)
            .field("tool", &self.request.name)
            .finish_non_exhaustive()
    }
}

/// One call that waits.
#[derive(Clone, PartialEq, Eq)]
struct Call {
    id: String,
    thread_id: i64,
    label: String,
    name: String,
    arguments: String,
    /// The model's id of the call (lmgw since 2026-10-09: the id the call's `ready` and `result`
    /// frames carry, so the call and its MCP App view, `crate::widgets::thread_apps`, are the same
    /// call). Never the card's key (`id` is): a gateway or an `mcp_approval_request` item without
    /// it leaves it `None`, and a later announcement of the call fills it in.
    call_id: Option<String>,
    /// The bound session that showed it (an `mcp_approval_request` item), by the voice's count of
    /// sessions: its verdict goes through that session while it is still the bound one.
    in_session: Option<u64>,
    verdict: Option<bool>,
}

/// How a thread's verdicts went.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Via {
    /// Through the approvals route; `resent`: the verdicts went through a session first, which
    /// ended before lmgw said it took them (lmgw may have: its `approval_decided` is then no
    /// refusal, `route_answered`).
    Route { resent: bool },
    /// Through session `no`: `create` is the `event_id` of the `response.create` that carries them
    /// (an `error` echoing it is lmgw's refusal of it); `response` is the response it created,
    /// once lmgw said `response.created`.
    Session {
        no: u64,
        create: String,
        response: Option<String>,
    },
}

/// A thread's verdicts on their way: their calls stay until lmgw took them.
#[derive(Debug, Clone)]
struct Sending {
    thread_id: i64,
    ids: Vec<String>,
    via: Via,
}

/// What a request on its way is for.
#[derive(Debug)]
pub enum Op {
    /// The thread's verdicts, sent through the route.
    Decide { thread_id: i64, ids: Vec<String> },
    /// The spoken answer to call `call`, transcribed.
    Hear { call: String },
    /// The thread's waiting calls, read again.
    Reread { thread_id: i64 },
}

#[derive(Debug)]
pub enum Done {
    Decided(Result<(), Refusal>),
    Heard(Result<String, Refusal>),
    /// The thread's waiting calls (`GET /chat/api/threads/{id}`'s `pending_approvals`).
    Pending(Result<Vec<ApprovalRequest>, Refusal>),
}

/// What [`Approvals::next`] woke for.
#[derive(Debug)]
pub enum Wake {
    Card(CardEvent),
    Done(Op, Done),
}

/// What the voice carries out for the approvals (it holds the session and the link).
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    /// The thread's verdicts through the bound session: an `mcp_approval_response` each, then
    /// `response.create` with `event_id` `create`.
    Session {
        thread_id: i64,
        decisions: Vec<ApprovalDecision>,
        create: String,
    },
    /// The thread's verdicts through `POST /chat/api/threads/{id}/approvals`.
    Route {
        thread_id: i64,
        body: ApprovalsRequest,
    },
    /// The spoken answer to `call`, as a WAV, through `POST /chat/api/threads/{id}/transcribe`.
    Transcribe {
        thread_id: i64,
        call: String,
        wav: Vec<u8>,
    },
    /// The thread's waiting calls, read again (`GET /chat/api/threads/{id}`).
    Reread { thread_id: i64 },
}

/// Who the card says asked, by thread: the starter of its last turn, as the feed names authors.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asker {
    name: String,
    own: bool,
}

/// A thread's calls read again: what lmgw said to the answer that asked for it (none: a resync).
#[derive(Debug, Clone)]
struct Reread {
    refusal: Option<Refusal>,
}

type Sink = Box<dyn FnMut(CardCmd) + Send>;

pub struct Approvals {
    s: ApprovalSettings,
    calls: Vec<Call>,
    asked_by: HashMap<i64, Asker>,
    titles: HashMap<i64, String>,
    /// Verdicts on their way (the route, or a session).
    sending: Vec<Sending>,
    /// Threads whose waiting calls are being read again.
    rereading: HashMap<i64, Reread>,
    /// Kai's own threads now: the bound session's and the conversation's (the voice says them).
    own: Vec<i64>,
    /// The bound session that is created now, by number (the voice says it).
    live: Option<u64>,
    /// The call the card said it armed for, until it says otherwise or shows another.
    armed: Option<String>,
    /// Calls the card cannot show (no layer-shell, its page ended twice): their threads are the
    /// owner's to answer in lmgw.
    lost: HashSet<String>,
    /// The spoken answer being recorded (the talk key is held), and the call it answers.
    recording: Option<(String, Vec<i16>)>,
    /// The call whose spoken answer is being transcribed.
    hearing: Option<String>,
    /// What the card says under the call, and for which call.
    note: Option<(String, String)>,
    ops: FuturesUnordered<Pending<(Op, Done)>>,
    events_tx: mpsc::UnboundedSender<CardEvent>,
    events_rx: mpsc::UnboundedReceiver<CardEvent>,
    sink: Sink,
    /// What the card was last told (`None`: hidden).
    shown: Option<CardModel>,
    /// Desktop notifications to send: (summary, body).
    notices: Vec<(String, String)>,
    /// Numbers the `response.create`s that carry verdicts (their `event_id`).
    creates: u64,
    /// The screen is locked (`crate::screen_lock`): the card is hidden, its calls stay waiting, no
    /// answer is taken.
    locked: bool,
}

impl Approvals {
    /// `sink` carries the card's commands to the GTK side ([`overlay::send`] in Kai).
    pub fn new(s: ApprovalSettings, sink: Sink) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            s,
            calls: Vec::new(),
            asked_by: HashMap::new(),
            titles: HashMap::new(),
            sending: Vec::new(),
            rereading: HashMap::new(),
            own: Vec::new(),
            live: None,
            armed: None,
            lost: HashSet::new(),
            recording: None,
            hearing: None,
            note: None,
            ops: FuturesUnordered::new(),
            events_tx,
            events_rx,
            sink,
            shown: None,
            notices: Vec::new(),
            creates: 0,
            locked: false,
        }
    }

    /// The screen locked or unlocked (`crate::screen_lock`, the owner's decision of 2026-10-09):
    /// while locked the card is hidden and takes no answer (a spoken answer being recorded is
    /// dropped); the calls stay waiting, for other clients and for the card after the unlock,
    /// where it shows (and arms) afresh.
    pub fn set_locked(&mut self, locked: bool) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if locked {
            if self.recording.take().is_some() {
                tracing::info!(
                    "approvals: the screen locked; the spoken answer being recorded is dropped"
                );
            }
            self.armed = None;
        }
        self.sync();
    }

    /// Notifications for the voice to send.
    pub fn take_notices(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.notices)
    }

    /// The card as it was last told (the tests' look).
    #[cfg(test)]
    pub fn card(&self) -> Option<&CardModel> {
        self.shown.as_ref()
    }

    /// A call is known (waiting, or its verdict on its way).
    #[cfg(test)]
    pub fn waiting(&self) -> bool {
        !self.calls.is_empty()
    }

    /// The next event from the card, or a request that came back. Cancel-safe.
    pub async fn next(&mut self) -> Wake {
        tokio::select! {
            Some(e) = self.events_rx.recv() => Wake::Card(e),
            Some((op, done)) = self.ops.next(), if !self.ops.is_empty() => Wake::Done(op, done),
        }
    }

    /// A request on its way (the voice made it from an [`Act`]).
    pub fn push(&mut self, op: Op, fut: Pending<Done>) {
        self.ops.push(Box::pin(async move { (op, fut.await) }));
    }

    // ------------------------------------------------------------- what the voice says and reads

    /// The voice's view after every event: Kai's own threads (the bound session's and the
    /// conversation's) and the bound session that is created now. Verdicts that went through a
    /// session that is no longer the live one and were not taken go by the route.
    pub fn observe(&mut self, own: Vec<i64>, live: Option<u64>) -> Vec<Act> {
        let mut acts = Vec::new();
        self.live = live;
        let ended: Vec<usize> = self
            .sending
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.via, Via::Session { no, .. } if Some(no) != live))
            .map(|(i, _)| i)
            .collect();
        for i in ended.into_iter().rev() {
            let s = self.sending.remove(i);
            tracing::info!(
                thread = s.thread_id,
                calls = s.ids.len(),
                "approvals: the session that carried the verdicts ended before lmgw took them; \
                 they go through the approvals route"
            );
            acts.extend(self.route_as(s.thread_id, &s.ids, true));
        }
        if own != self.own {
            self.own = own;
            self.sync();
        }
        acts
    }

    /// A call of Kai's own waits (K17): no audio goes to the session. Not while the card cannot
    /// show it (nothing would tell the owner why Kai stopped listening). A verdict on its way
    /// through the session holds the gate until lmgw created the response that carries it: a turn
    /// committed before then would be taken as that response (or, with words, decline the call).
    pub fn gates_audio(&self) -> bool {
        let lost = self.lost_threads();
        self.calls.iter().any(|c| {
            self.mine(c)
                && (!self.in_flight(c) || self.awaits_response(c))
                && !lost.contains(&c.thread_id)
        })
    }

    /// A press of the talk key now records the spoken answer (K17): the card shows a call of Kai's
    /// own and said it armed for it.
    pub fn voice_answers(&self) -> bool {
        !self.locked
            && self
                .current()
                .is_some_and(|c| self.mine(c) && self.armed.as_deref() == Some(c.id.as_str()))
    }

    /// Session `no` showed a call that waits, or carried verdicts lmgw has not taken yet: the
    /// session stays (no grace close) until then.
    pub fn waiting_in_session(&self, no: u64) -> bool {
        self.calls.iter().any(|c| c.in_session == Some(no))
            || self
                .sending
                .iter()
                .any(|s| matches!(s.via, Via::Session { no: n, .. } if n == no))
    }

    /// The talk key records the spoken answer now.
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The tooltip's line while a call waits.
    pub fn note(&self) -> Option<String> {
        if let Some(c) = self.calls.iter().find(|c| self.lost.contains(&c.id)) {
            return Some(format!(
                "an approval waits for {} in {}, and the approval card cannot show it: answer it \
                 in lmgw",
                tool_name(c),
                self.place(c.thread_id)
            ));
        }
        let c = self.current()?;
        if self.locked {
            return Some(format!(
                "an approval waits for {} in {}: the card shows again once the screen is \
                 unlocked (or answer it in lmgw)",
                tool_name(c),
                self.place(c.thread_id)
            ));
        }
        Some(if self.mine(c) {
            format!(
                "an approval waits for {}: answer on the card, or hold the talk key and say yes \
                 or no",
                tool_name(c)
            )
        } else {
            format!(
                "an approval waits for {} in {}: answer on the card",
                tool_name(c),
                self.place(c.thread_id)
            )
        })
    }

    // ------------------------------------------------------------- facts

    /// A call waits: the feed's `approval.requested` (`in_session` `None`) or session `no`'s
    /// `mcp_approval_request` (`Some(no)`).
    pub fn requested(&mut self, thread_id: i64, req: ApprovalRequest, in_session: Option<u64>) {
        match self
            .calls
            .iter_mut()
            .find(|c| c.id == req.approval_request_id)
        {
            Some(c) => {
                if in_session.is_some() {
                    c.in_session = in_session;
                }
                if c.call_id.is_none() {
                    c.call_id = req.call_id.filter(|id| !id.is_empty());
                }
            }
            None => {
                tracing::info!(
                    call = %req.approval_request_id,
                    thread = thread_id,
                    label = %req.server_label,
                    tool = %req.name,
                    session = ?in_session,
                    "approvals: a call waits"
                );
                self.calls.push(Call {
                    id: req.approval_request_id,
                    thread_id,
                    label: req.server_label,
                    name: req.name,
                    arguments: req.arguments,
                    call_id: req.call_id.filter(|id| !id.is_empty()),
                    in_session,
                    verdict: None,
                });
            }
        }
        self.sync();
    }

    /// The bound session's `lmgw.chat.frame` relaying a turn's `approval` frame carries the call's
    /// id beside the `approval_request_id` its `mcp_approval_request` item (which has none) was
    /// shown under: the card's call learns it.
    pub fn call_id_seen(&mut self, approval_request_id: &str, call_id: &str) {
        if let Some(c) = self
            .calls
            .iter_mut()
            .find(|c| c.id == approval_request_id && c.call_id.is_none())
            && !call_id.is_empty()
        {
            c.call_id = Some(call_id.to_owned());
        }
    }

    /// The model's id of the call waiting under `approval_request_id`, when lmgw gave it: what
    /// finds the call's MCP App view.
    pub fn call_id_of(&self, approval_request_id: &str) -> Option<&str> {
        self.calls
            .iter()
            .find(|c| c.id == approval_request_id)
            .and_then(|c| c.call_id.as_deref())
    }

    /// A call was decided (the feed's `approval.decided`, the session's `lmgw.approval.decided`):
    /// it goes, also when its verdict was Kai's own on its way.
    pub fn decided(&mut self, id: &str, approve: bool, by: Option<&str>) {
        let before = self.calls.len();
        self.calls.retain(|c| c.id != id);
        if self.calls.len() < before {
            tracing::info!(call = %id, approve, by = ?by, "approvals: decided; the call goes");
            self.forget_call(id);
            self.sync();
        }
    }

    /// The feed's `turn.started`: who started the thread's last turn (the card's "asked by").
    pub fn turn_started(&mut self, thread_id: i64, name: &str, own: bool) {
        self.asked_by.insert(
            thread_id,
            Asker {
                name: name.to_owned(),
                own,
            },
        );
    }

    /// A thread's title as the feed shows it now.
    pub fn thread_title(&mut self, thread_id: i64, title: &str) {
        self.titles.insert(thread_id, title.to_owned());
        if self.calls.iter().any(|c| c.thread_id == thread_id) {
            self.sync();
        }
    }

    /// The thread was deleted: its calls go with it.
    pub fn thread_gone(&mut self, thread_id: i64) {
        self.asked_by.remove(&thread_id);
        self.titles.remove(&thread_id);
        self.rereading.remove(&thread_id);
        self.drop_thread(thread_id, "the thread was deleted");
    }

    /// The feed's `resync`: what it did not deliver may have changed any thread's calls. The
    /// threads Kai knows calls of, and Kai's own, are read again.
    pub fn resync(&mut self) -> Vec<Act> {
        let mut threads: Vec<i64> = self.calls.iter().map(|c| c.thread_id).collect();
        threads.extend(self.own.iter().copied());
        threads.sort_unstable();
        threads.dedup();
        tracing::info!(
            threads = threads.len(),
            "approvals: the feed resynced; the waiting calls are read again"
        );
        let acts = threads
            .into_iter()
            .flat_map(|t| self.reread(t, None))
            .collect();
        self.sync();
        acts
    }

    /// The gateway was left (a re-pair, a revocation): its calls are not Kai's to answer any more,
    /// and nothing on its way to it is waited for.
    pub fn forget_all(&mut self) {
        if !self.calls.is_empty() {
            tracing::info!(
                calls = self.calls.len(),
                "approvals: the gateway was left; its calls go"
            );
        }
        self.calls.clear();
        self.sending.clear();
        self.rereading.clear();
        self.ops = FuturesUnordered::new();
        self.recording = None;
        self.hearing = None;
        self.note = None;
        self.armed = None;
        self.lost.clear();
        self.asked_by.clear();
        self.titles.clear();
        self.sync();
    }

    // ------------------------------------------------------------- the session's answers

    /// Session `no` created a response: the first one after verdicts it carried is theirs (the
    /// session gets no audio until then, [`Self::gates_audio`], and a refused create is said by an
    /// `error` echoing its id, [`Self::session_refused`]).
    pub fn session_response_created(&mut self, no: u64, response_id: &str) {
        if let Some(s) = self
            .sending
            .iter_mut()
            .find(|s| matches!(&s.via, Via::Session { no: n, response: None, .. } if *n == no))
            && let Via::Session { response, .. } = &mut s.via
        {
            *response = Some(response_id.to_owned());
        }
    }

    /// Session `no`'s response ended with `status`. The one that carried verdicts: lmgw took them
    /// when it completed. Any other end (failed, cancelled, incomplete) leaves it open whether
    /// lmgw decided, so the thread is read again (what still waits is asked again).
    pub fn session_response_done(&mut self, no: u64, response_id: &str, status: &str) -> Vec<Act> {
        let Some(i) = self.sending.iter().position(|s| {
            matches!(&s.via, Via::Session { no: n, response: Some(r), .. } if *n == no && r == response_id)
        }) else {
            return Vec::new();
        };
        let s = self.sending.remove(i);
        let acts = if status == "completed" {
            tracing::info!(
                thread = s.thread_id,
                "approvals: lmgw took the verdicts (session)"
            );
            self.calls.retain(|c| !s.ids.contains(&c.id));
            for id in &s.ids {
                self.forget_call(id);
            }
            Vec::new()
        } else {
            tracing::warn!(
                thread = s.thread_id,
                status,
                "approvals: the response that carried the verdicts did not complete; the thread is \
                 read again"
            );
            self.clear_verdicts(&s.ids);
            self.reread(
                s.thread_id,
                Some(Refusal::no_answer(format!(
                    "the voice session's reply to the answer ended {status}"
                ))),
            )
        };
        self.sync();
        acts
    }

    /// Session `no` said an `error` about the verdicts it carried: one echoing the `event_id` of
    /// their `response.create` (`create`, whatever its code: an active response, the GPU hold, a
    /// model's error), or one with an approval code (`approval_decided`, …; `create` `None`).
    /// Refused: the thread's waiting calls are read again.
    pub fn session_refused(&mut self, no: u64, create: Option<&str>, e: Refusal) -> Vec<Act> {
        let Some(i) = self.sending.iter().position(|s| match &s.via {
            Via::Session {
                no: n, create: c, ..
            } => *n == no && create.is_none_or(|id| id == c),
            Via::Route { .. } => false,
        }) else {
            return Vec::new();
        };
        let s = self.sending.remove(i);
        let acts = self.refused(s.thread_id, &s.ids, e);
        self.sync();
        acts
    }

    // ------------------------------------------------------------- answers

    /// The voice's wake: an event of the card, or a request that came back. `session`: the
    /// created session now, its thread and its number.
    pub fn on_wake(&mut self, w: Wake, session: Option<(i64, u64)>) -> Vec<Act> {
        let acts = match w {
            Wake::Card(CardEvent::Answer(a)) => {
                self.answer(&a.call, a.approve, "the card", session)
            }
            Wake::Card(CardEvent::Armed { call }) => {
                if self.current().is_some_and(|c| c.id == call) {
                    tracing::debug!(call = %call, "approvals: the card armed");
                    self.armed = Some(call);
                }
                Vec::new()
            }
            Wake::Card(CardEvent::Unarmed { call }) => {
                if self.armed.as_deref() == Some(call.as_str()) {
                    tracing::debug!(call = %call, "approvals: the card waits again before it answers");
                    self.armed = None;
                }
                Vec::new()
            }
            Wake::Card(CardEvent::Lost { call, why }) => {
                self.card_lost(call, &why);
                Vec::new()
            }
            Wake::Done(Op::Decide { thread_id, ids }, Done::Decided(r)) => {
                self.route_answered(thread_id, &ids, r)
            }
            Wake::Done(Op::Hear { call }, Done::Heard(r)) => self.heard(&call, r, session),
            Wake::Done(Op::Reread { thread_id }, Done::Pending(r)) => {
                self.reread_answered(thread_id, r);
                Vec::new()
            }
            Wake::Done(op, done) => {
                tracing::warn!(
                    ?op,
                    ?done,
                    "approvals: an answer that does not fit its request"
                );
                Vec::new()
            }
        };
        self.sync();
        acts
    }

    /// The owner's verdict on `call`, from `how` (the card, the voice).
    fn answer(
        &mut self,
        call: &str,
        approve: bool,
        how: &str,
        session: Option<(i64, u64)>,
    ) -> Vec<Act> {
        if self.locked {
            tracing::info!(call = %call, how, "approvals: an answer while the screen is locked; ignored");
            return Vec::new();
        }
        if self.current().map(|c| c.id.as_str()) != Some(call) {
            tracing::info!(call = %call, "approvals: an answer for a call the card no longer shows; ignored");
            return Vec::new();
        }
        self.recording = None;
        self.drop_answer_to(call);
        let c = self
            .calls
            .iter_mut()
            .find(|c| c.id == call)
            .expect("the current call");
        c.verdict = Some(approve);
        let thread_id = c.thread_id;
        tracing::info!(call = %call, thread = thread_id, approve, how, "approvals: the owner answered");
        let batch: Vec<&Call> = self
            .calls
            .iter()
            .filter(|c| c.thread_id == thread_id && !self.in_flight(c))
            .collect();
        if batch.iter().any(|c| c.verdict.is_none()) {
            // The reply's other calls first: lmgw takes a reply's verdicts together.
            return Vec::new();
        }
        let ids: Vec<String> = batch.iter().map(|c| c.id.clone()).collect();
        let through_session = session.and_then(|(t, no)| {
            (t == thread_id && batch.iter().all(|c| c.in_session == Some(no))).then_some(no)
        });
        match through_session {
            Some(no) => {
                tracing::info!(
                    thread = thread_id,
                    calls = ids.len(),
                    "approvals: the verdicts go through the bound session"
                );
                let decisions = self.decisions(&ids);
                self.creates += 1;
                let create = format!("kai_approvals_{}", self.creates);
                // Kept until lmgw took them (`session_response_done`, `decided`).
                self.sending.push(Sending {
                    thread_id,
                    ids,
                    via: Via::Session {
                        no,
                        create: create.clone(),
                        response: None,
                    },
                });
                vec![Act::Session {
                    thread_id,
                    decisions,
                    create,
                }]
            }
            None => {
                tracing::info!(
                    thread = thread_id,
                    calls = ids.len(),
                    "approvals: the verdicts go through the approvals route"
                );
                self.route(thread_id, &ids)
            }
        }
    }

    /// The verdicts of `ids` (all given) through the approvals route.
    fn route(&mut self, thread_id: i64, ids: &[String]) -> Vec<Act> {
        self.route_as(thread_id, ids, false)
    }

    /// [`Self::route`]; `resent`: they went through a session first ([`Via::Route`]).
    fn route_as(&mut self, thread_id: i64, ids: &[String], resent: bool) -> Vec<Act> {
        let decisions = self.decisions(ids);
        if decisions.is_empty() {
            return Vec::new();
        }
        self.sending.push(Sending {
            thread_id,
            ids: decisions
                .iter()
                .map(|d| d.approval_request_id.clone())
                .collect(),
            via: Via::Route { resent },
        });
        vec![Act::Route {
            thread_id,
            body: ApprovalsRequest {
                decisions,
                speak: false,
            },
        }]
    }

    /// The verdicts Kai holds of `ids` (a call that went meanwhile has none).
    fn decisions(&self, ids: &[String]) -> Vec<ApprovalDecision> {
        self.calls
            .iter()
            .filter(|c| ids.contains(&c.id))
            .filter_map(|c| {
                let approve = c.verdict?;
                Some(ApprovalDecision {
                    approval_request_id: c.id.clone(),
                    approve,
                    reason: (!approve).then(|| "declined on Kai".to_owned()),
                })
            })
            .collect()
    }

    /// The route's answer for `thread_id`'s verdicts.
    fn route_answered(
        &mut self,
        thread_id: i64,
        ids: &[String],
        r: Result<(), Refusal>,
    ) -> Vec<Act> {
        // Its ids, less any decided meanwhile (`forget_call`).
        let mut resent = false;
        self.sending.retain(|s| {
            let this = s.thread_id == thread_id
                && matches!(s.via, Via::Route { .. })
                && s.ids.iter().all(|i| ids.contains(i));
            if this {
                resent |= s.via == Via::Route { resent: true };
            }
            !this
        });
        // Verdicts a session carried before it ended: lmgw may have taken them there (the resumed
        // reply had started), so "decided already" is that, not a refusal.
        let r = match r {
            Err(e) if resent && e.code() == code::APPROVAL_DECIDED => {
                tracing::info!(thread = thread_id, %e, "approvals: the verdicts the ended session carried were decided already; taken");
                Ok(())
            }
            r => r,
        };
        match r {
            Ok(()) => {
                tracing::info!(thread = thread_id, "approvals: lmgw took the verdicts");
                self.calls.retain(|c| !ids.contains(&c.id));
                for id in ids {
                    self.forget_call(id);
                }
                Vec::new()
            }
            Err(e) => self.refused(thread_id, ids, e),
        }
    }

    /// lmgw did not take `thread_id`'s verdicts on `ids`. Beyond the device's reach: the calls
    /// stay and the card says so. Anything else (decided by someone else, moved on, gone, a call
    /// Kai never saw missing from the answer, no answer in time though lmgw may have decided): the
    /// thread's waiting calls are read again before anything is asked again.
    fn refused(&mut self, thread_id: i64, ids: &[String], e: Refusal) -> Vec<Act> {
        tracing::warn!(thread = thread_id, status = e.status, code = ?e.code, message = %e.message, "approvals: the verdicts were refused");
        self.clear_verdicts(ids);
        if e.code() == code::APPROVAL_OUT_OF_SCOPE {
            if let Some(first) = self.calls.iter().find(|c| c.thread_id == thread_id) {
                self.note = Some((
                    first.id.clone(),
                    format!(
                        "lmgw refused Kai's approval: {}. Decline here, or approve in lmgw.",
                        e.message
                    ),
                ));
            }
            return Vec::new();
        }
        self.reread(thread_id, Some(e))
    }

    /// Reads `thread_id`'s waiting calls again (one request at a time per thread).
    fn reread(&mut self, thread_id: i64, refusal: Option<Refusal>) -> Vec<Act> {
        match self.rereading.get_mut(&thread_id) {
            Some(r) => {
                if refusal.is_some() {
                    r.refusal = refusal;
                }
                Vec::new()
            }
            None => {
                self.rereading.insert(thread_id, Reread { refusal });
                vec![Act::Reread { thread_id }]
            }
        }
    }

    /// The thread's waiting calls as lmgw lists them now: the ones that went go, new ones come,
    /// and what still waits is asked again.
    fn reread_answered(&mut self, thread_id: i64, r: Result<Vec<ApprovalRequest>, Refusal>) {
        let Some(rr) = self.rereading.remove(&thread_id) else {
            return;
        };
        let list = match r {
            Ok(l) => l,
            Err(e) if e.status == 404 => {
                // Gone, or out of the device's reach: nothing of it is Kai's to answer.
                tracing::info!(thread = thread_id, %e, "approvals: the thread is not there to read");
                self.drop_thread(thread_id, "the thread could not be read (404)");
                return;
            }
            Err(e) => {
                tracing::warn!(thread = thread_id, %e, "approvals: the thread's waiting calls could not be read");
                if let Some(first) = self
                    .calls
                    .iter()
                    .find(|c| c.thread_id == thread_id && !self.in_flight(c))
                {
                    let why = match &rr.refusal {
                        Some(r) => format!("lmgw did not take the answer: {r}, and "),
                        None => String::new(),
                    };
                    self.note = Some((
                        first.id.clone(),
                        format!(
                            "{why}Kai could not read the thread's waiting calls: {e}. Answer \
                             again, or in lmgw."
                        ),
                    ));
                }
                return;
            }
        };
        let listed: HashSet<&str> = list
            .iter()
            .map(|r| r.approval_request_id.as_str())
            .collect();
        let gone: Vec<String> = self
            .calls
            .iter()
            .filter(|c| {
                c.thread_id == thread_id && !self.in_flight(c) && !listed.contains(c.id.as_str())
            })
            .map(|c| c.id.clone())
            .collect();
        self.calls.retain(|c| !gone.contains(&c.id));
        for id in &gone {
            self.forget_call(id);
        }
        let mut new = 0;
        for req in list {
            if !self.calls.iter().any(|c| c.id == req.approval_request_id) {
                new += 1;
                self.calls.push(Call {
                    id: req.approval_request_id,
                    thread_id,
                    label: req.server_label,
                    name: req.name,
                    arguments: req.arguments,
                    call_id: req.call_id.filter(|id| !id.is_empty()),
                    in_session: None,
                    verdict: None,
                });
            }
        }
        let first = self
            .calls
            .iter()
            .find(|c| c.thread_id == thread_id && !self.in_flight(c))
            .map(|c| c.id.clone());
        tracing::info!(
            thread = thread_id,
            gone = gone.len(),
            new,
            waits = first.is_some(),
            "approvals: the thread's waiting calls, read again"
        );
        match (rr.refusal, first) {
            (Some(e), Some(first)) => {
                self.note = Some((
                    first,
                    format!("lmgw did not take the answer: {e}. Answer again."),
                ));
            }
            // lmgw said why (decided by someone else, moved on): nothing of it waits now.
            (Some(e), None) if e.code.is_some() => self.notices.push((
                "Kai: the approval was not taken".to_owned(),
                format!("lmgw did not take the answer: {e}; nothing of it waits any more"),
            )),
            // No answer, but nothing waits now: lmgw took it (or someone decided meanwhile).
            (Some(e), None) => {
                tracing::info!(thread = thread_id, %e, "approvals: no answer came, but nothing waits any more")
            }
            (None, _) => {}
        }
    }

    // ------------------------------------------------------------- the spoken answer (K17)

    /// The talk key went down while [`Self::voice_answers`]: the answer is recorded, starting
    /// with `preroll` (what the microphone heard just before). `false` when nothing records (the
    /// card does not ask for the voice now, or the last answer is still being transcribed).
    pub fn press(&mut self, preroll: Vec<i16>) -> bool {
        if self.hearing.is_some() {
            tracing::info!(
                "approvals: the last spoken answer is still being heard; the press waits"
            );
            return false;
        }
        if !self.voice_answers() {
            return false;
        }
        let call = self.current().map(|c| c.id.clone()).expect("checked");
        tracing::info!(call = %call, "approvals: recording the spoken answer (K17)");
        self.recording = Some((call, preroll));
        self.sync();
        true
    }

    /// What the microphone heard while the talk key records.
    pub fn record(&mut self, samples: &[i16]) {
        if let Some((_, rec)) = self.recording.as_mut() {
            rec.extend_from_slice(samples);
        }
    }

    /// The talk key came up: the recording goes to be transcribed in the call's thread, if it
    /// holds speech.
    pub fn release(&mut self) -> Vec<Act> {
        let Some((call, samples)) = self.recording.take() else {
            return Vec::new();
        };
        let Some(c) = self.calls.iter().find(|c| c.id == call) else {
            self.sync();
            return Vec::new();
        };
        let thread_id = c.thread_id;
        let ms = samples.len() as u64 * 1000 / crate::audio::RATE as u64;
        let speech = words::speech_ms(&samples, self.s.speech_dbfs);
        if speech < self.s.speech_ms {
            // ASR makes words of near silence ("Okay.", "Ja."): a recording with no speech in it
            // is no answer.
            tracing::info!(call = %call, ms, speech_ms = speech, "approvals: the spoken answer holds no speech; not sent");
            self.note = Some((
                call,
                format!(
                    "Kai heard no speech ({speech} ms at or above {} dBFS, approvals.speech_ms \
                     is {}): say yes or no, or click.",
                    self.s.speech_dbfs, self.s.speech_ms
                ),
            ));
            self.sync();
            return Vec::new();
        }
        tracing::info!(call = %call, ms, speech_ms = speech, "approvals: the spoken answer goes to be transcribed");
        self.hearing = Some(call.clone());
        self.note = None;
        self.sync();
        vec![Act::Transcribe {
            thread_id,
            call,
            wav: words::wav(&samples),
        }]
    }

    /// The recording was given up (the stuck-key guard, the session ending under it).
    pub fn cancel_recording(&mut self) {
        if self.recording.take().is_some() {
            self.sync();
        }
    }

    fn heard(
        &mut self,
        call: &str,
        r: Result<String, Refusal>,
        session: Option<(i64, u64)>,
    ) -> Vec<Act> {
        if self.hearing.as_deref() == Some(call) {
            self.hearing = None;
        }
        if self.current().map(|c| c.id.as_str()) != Some(call) {
            tracing::info!(call = %call, "approvals: heard an answer for a call that went meanwhile");
            return Vec::new();
        }
        let text = match r {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(call = %call, status = e.status, code = ?e.code, message = %e.message, "approvals: the spoken answer was not transcribed");
                self.note = Some((
                    call.to_owned(),
                    format!("Kai could not hear the answer: {e}. Say it again, or click."),
                ));
                return Vec::new();
            }
        };
        match words::answer(&text, &self.s.yes, &self.s.no) {
            Some(approve) => self.answer(call, approve, "voice", session),
            None => {
                // The owner's own words: shown on the card, never logged.
                tracing::info!(call = %call, chars = text.chars().count(), "approvals: the spoken answer is neither yes nor no");
                let heard = text.trim();
                self.note = Some((
                    call.to_owned(),
                    if heard.is_empty() {
                        "Kai heard nothing: say yes or no, or click.".to_owned()
                    } else {
                        format!("Heard “{}”: say yes or no, or click.", card::name(heard))
                    },
                ));
                Vec::new()
            }
        }
    }

    // ------------------------------------------------------------- the card

    /// A call of Kai's own: one the live bound session showed (not an ended one's), or one in
    /// its conversation's thread.
    fn mine(&self, c: &Call) -> bool {
        c.in_session.is_some_and(|n| Some(n) == self.live) || self.own.contains(&c.thread_id)
    }

    /// The call's verdict is on its way.
    fn in_flight(&self, c: &Call) -> bool {
        self.sending.iter().any(|s| s.ids.contains(&c.id))
    }

    /// The call's verdict went through a session whose `response.create` lmgw has not answered
    /// with `response.created` yet.
    fn awaits_response(&self, c: &Call) -> bool {
        self.sending
            .iter()
            .any(|s| matches!(s.via, Via::Session { response: None, .. }) && s.ids.contains(&c.id))
    }

    /// Threads with a call the card cannot show: their calls are left to lmgw.
    fn lost_threads(&self) -> HashSet<i64> {
        self.calls
            .iter()
            .filter(|c| self.lost.contains(&c.id))
            .map(|c| c.thread_id)
            .collect()
    }

    /// The call the card shows: the oldest without a verdict that is not on its way, whose thread
    /// can show and is not read again after a refusal (a resync's reading keeps the card as it
    /// is: hiding it would make the card go and arm anew for nothing).
    fn current(&self) -> Option<&Call> {
        let lost = self.lost_threads();
        self.calls.iter().find(|c| {
            c.verdict.is_none()
                && !self.in_flight(c)
                && !self
                    .rereading
                    .get(&c.thread_id)
                    .is_some_and(|r| r.refusal.is_some())
                && !lost.contains(&c.thread_id)
        })
    }

    /// The card cannot show `call`: the owner answers it in lmgw.
    fn card_lost(&mut self, call: String, why: &str) {
        let Some(c) = self.calls.iter().find(|c| c.id == call) else {
            return;
        };
        tracing::warn!(call = %call, why, "approvals: the card cannot show the call; it is left to lmgw");
        self.notices.push((
            "Kai: an approval waits".to_owned(),
            format!(
                "{} waits for an approval in {}, and the approval card cannot show it ({why}): \
                 answer it in lmgw",
                tool_name(c),
                self.place(c.thread_id)
            ),
        ));
        if self.armed.as_deref() == Some(call.as_str()) {
            self.armed = None;
        }
        if self.recording.as_ref().is_some_and(|(id, _)| *id == call) {
            self.recording = None;
        }
        self.lost.insert(call);
        // The GTK side has no card now: the next call that can show is shown afresh.
        self.shown = None;
    }

    fn clear_verdicts(&mut self, ids: &[String]) {
        for c in self.calls.iter_mut().filter(|c| ids.contains(&c.id)) {
            c.verdict = None;
        }
    }

    /// Everything about a call that went.
    fn forget_call(&mut self, id: &str) {
        self.drop_answer_to(id);
        self.lost.remove(id);
        for s in &mut self.sending {
            s.ids.retain(|i| i != id);
        }
        self.sending.retain(|s| !s.ids.is_empty());
    }

    fn drop_answer_to(&mut self, id: &str) {
        if self.note.as_ref().is_some_and(|(c, _)| c == id) {
            self.note = None;
        }
        if self.recording.as_ref().is_some_and(|(c, _)| c == id) {
            self.recording = None;
        }
        if self.hearing.as_deref() == Some(id) {
            self.hearing = None;
        }
        if self.armed.as_deref() == Some(id) {
            self.armed = None;
        }
    }

    fn drop_thread(&mut self, thread_id: i64, why: &str) {
        let gone: Vec<String> = self
            .calls
            .iter()
            .filter(|c| c.thread_id == thread_id)
            .map(|c| c.id.clone())
            .collect();
        if gone.is_empty() {
            return;
        }
        tracing::info!(
            thread = thread_id,
            calls = gone.len(),
            why,
            "approvals: the thread's calls go"
        );
        self.calls.retain(|c| c.thread_id != thread_id);
        for id in &gone {
            self.forget_call(id);
        }
        self.sync();
    }

    /// Where a thread's call waits, as the card and the tooltip say it.
    fn place(&self, thread_id: i64) -> String {
        match self.titles.get(&thread_id) {
            Some(t) if !t.trim().is_empty() => format!("thread {}", card::quoted_name(t)),
            _ => format!("thread {thread_id}"),
        }
    }

    fn model(&self) -> Option<CardModel> {
        if self.locked {
            return None;
        }
        let c = self.current()?;
        let siblings: Vec<&Call> = self
            .calls
            .iter()
            .filter(|o| o.thread_id == c.thread_id && !self.in_flight(o))
            .collect();
        let count = (siblings.len() > 1).then(|| {
            let n = siblings.iter().position(|o| o.id == c.id).unwrap_or(0) + 1;
            format!("call {n} of {} this reply waits on", siblings.len())
        });
        let who = match self.asked_by.get(&c.thread_id) {
            Some(a) if a.own => "Kai's voice session".to_owned(),
            Some(a) => card::asker(&a.name),
            None if c.in_session.is_some() => "Kai's voice session".to_owned(),
            None => "a turn".to_owned(),
        };
        let mine = self.mine(c);
        let busy = if self.recording.as_ref().is_some_and(|(id, _)| *id == c.id) {
            Some("Listening: say yes or no, then let go of the talk key.".to_owned())
        } else if self.hearing.as_deref() == Some(c.id.as_str()) {
            Some("Hearing the answer…".to_owned())
        } else {
            None
        };
        Some(CardModel {
            call: c.id.clone(),
            heading: format!("Approve {}?", tool_name(c)),
            asked: format!("asked by {who} in {}", self.place(c.thread_id)),
            args: card::arg_lines(&c.arguments),
            count,
            note: self
                .note
                .as_ref()
                .filter(|(id, _)| *id == c.id)
                .map(|(_, n)| n.clone()),
            busy,
            hint: if mine {
                "Enter approves, Esc declines; or hold the talk key and say yes or no.".into()
            } else {
                "Enter approves, Esc declines.".into()
            },
            typed: if mine {
                "A key reached the card before it armed, so Enter alone does not approve this \
                 call: Ctrl+Enter, a click or your voice does."
                    .into()
            } else {
                "A key reached the card before it armed, so Enter alone does not approve this \
                 call: Ctrl+Enter or a click does."
                    .into()
            },
        })
    }

    /// Tells the card what it shows now, when that changed.
    fn sync(&mut self) {
        let now = self.model();
        if self
            .armed
            .as_deref()
            .is_some_and(|a| now.as_ref().map(|m| m.call.as_str()) != Some(a))
        {
            self.armed = None;
        }
        if now == self.shown {
            return;
        }
        let cmd = match &now {
            Some(m) => CardCmd::Show {
                model: Box::new(m.clone()),
                look: self.s.look.clone(),
                events: self.events_tx.clone(),
            },
            None => CardCmd::Hide,
        };
        tracing::debug!(?cmd, "approvals: the card");
        (self.sink)(cmd);
        self.shown = now;
    }
}

/// The tool as the thread's model sees it: `<label>__<name>`, escaped for the card.
fn tool_name(c: &Call) -> String {
    let name = if c.label.is_empty() {
        c.name.clone()
    } else {
        format!("{}__{}", c.label, c.name)
    };
    card::name(&name).into_owned()
}
