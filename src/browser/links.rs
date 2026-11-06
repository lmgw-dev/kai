//! The links to Kai for Firefox: one per connected extension (each Firefox profile or instance
//! runs its own `kai-browser-host`), what each said in its `hello`, and the API the browser
//! provider (kai-firefox design §12, WP5) builds on:
//! - [`Links::list`]: who is linked, with the operations each serves (D3 lists tools by them);
//! - [`Links::op`]: send an `op` and wait for its `op_result`; a cancelled call, and one whose
//!   future is dropped while it waits, sends `cancel`;
//! - [`Links::subscribe`]: the extensions' `event` and `ask` frames as they come;
//! - [`Links::answer`]: the answer to an `ask` (the network gate's question, design §6.4);
//! - a link that comes or goes calls the host registry's [`Changes::signal`], so the host lists
//!   its tools again (D3: browser tools are listed while an extension is linked), and moves
//!   [`Links::linked`], the count the core's warning follows (design §8.3).
//!
//! Nothing here waits by a clock of its own: a call waits until its result, its cancellation or
//! the link's end ("Firefox closed while the call ran"), and the caller's deadline is its
//! cancellation (lmgw's, api L11).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::host::registry::Changes;

/// A link's number in this Kai run (never reused).
pub type LinkId = u64;

/// What an extension said in its `hello` (design §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub id: LinkId,
    /// The random id the extension keeps in its profile's `storage.local`.
    pub profile: String,
    pub ext_version: String,
    /// Firefox's version.
    pub firefox: String,
    pub protocol: u32,
    /// The operations it serves, `area.verb@version` (design §4.3).
    pub ops: Vec<String>,
    /// Firefox's main process: the parent of the `kai-browser-host` that connected (Firefox starts
    /// it as its own child), read through the peer's pidfd with the peer check
    /// (`super::server`); its windows are the desktop's windows of this pid. `None` where it
    /// could not be read.
    pub firefox_pid: Option<u32>,
}

/// Why an operation has no result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpError {
    #[error("no Firefox with Kai for Firefox is linked as link {0} (it closed)")]
    NoLink(LinkId),
    #[error("Kai for Firefox in this Firefox does not offer {op}")]
    NotServed { op: String },
    #[error("Firefox closed while the call ran")]
    Closed,
    #[error("the call was cancelled")]
    Cancelled,
    /// The extension's own error (`op_result`'s `error`), with what it says of the tab it concerns.
    #[error("{message}")]
    Failed {
        code: String,
        message: String,
        about: Option<Box<About>>,
    },
}

impl OpError {
    /// An error of the extension's that names no tab.
    pub fn failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.into(),
            about: None,
        }
    }
}

/// What an extension's error says of the tab it concerns (the contract's "an error about a tab"):
/// its id, and its title and URL as they were when the error happened; Reader view's page
/// (`reader_view`) and the URL that answered a fetch (`not_pdf`, `fetch_failed`). The messages name
/// a tab by its id only, so Kai can withhold a page its site rules keep it from
/// (`super::tools::failure`). Besides the tab: `no_option`'s `options`, the labels
/// `element.choose@1` found, an acting operation's `requests`, what its network gate held
/// before it failed, `not_get`'s `method` (how the page was requested), `bad_css`'s `dropped`
/// (what the parser did not understand), `script_error`'s `thrown` (what the script threw:
/// page text), and a style sheet's `uses` (`css_var_url`) and `fetches` (`fetch_not_approved`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub tab: Option<i64>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub page: Option<String>,
    pub response_url: Option<String>,
    pub options: Option<Vec<String>>,
    pub requests: Option<Vec<Value>>,
    pub method: Option<String>,
    pub dropped: Option<Vec<String>>,
    pub thrown: Option<Value>,
    /// `css_var_url`'s `uses`: each function that names a file through `var( )`, as the sheet
    /// writes it.
    pub uses: Option<Vec<String>>,
    /// `fetch_not_approved`'s `fetches`: the sheet's fetches from origins Kai did not pass,
    /// `{"origin", "url"}` each.
    pub fetches: Option<Vec<Value>>,
    /// `session_ended`'s `why`: what ended a reading session (`navigated`, `tab_closed`,
    /// `replaced`, `closed`).
    pub why: Option<String>,
}

impl About {
    /// The fields of an `error` object; `None` when it has none of them.
    fn of(error: &Value) -> Option<Box<Self>> {
        let text = |k: &str| error.get(k).and_then(Value::as_str).map(str::to_owned);
        let texts = |k: &str| {
            error.get(k).and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        };
        let about = Self {
            tab: error.get("tab").and_then(Value::as_i64),
            title: text("title"),
            url: text("url"),
            page: text("page"),
            response_url: text("response_url"),
            options: texts("options"),
            requests: error.get("requests").and_then(Value::as_array).cloned(),
            method: text("method"),
            dropped: texts("dropped"),
            thrown: error.get("thrown").cloned(),
            uses: texts("uses"),
            fetches: error.get("fetches").and_then(Value::as_array).cloned(),
            why: text("why"),
        };
        (about != Self::default()).then(|| Box::new(about))
    }

    /// The URLs it names: the tab's, Reader view's page, the one that answered.
    pub fn urls(&self) -> impl Iterator<Item = &str> {
        [&self.url, &self.page, &self.response_url]
            .into_iter()
            .filter_map(|u| u.as_deref())
    }
}

/// A frame the extension sent on its own, for the subscribers, and a link that came.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A link whose `hello` was welcomed (after its `welcome` went): events are off on it until
    /// Kai sends `events.config@1` (`super::announce`).
    Linked { link: LinkId },
    /// `event` (design §7.1): its `kind` and the frame's other fields.
    Event {
        link: LinkId,
        kind: String,
        fields: Map<String, Value>,
    },
    /// `ask` (the network gate holds a request, design §6.4): its `id`, to answer with
    /// [`Links::answer`], and the frame's other fields (`tab`, `method`, `url`, `body`).
    Ask {
        link: LinkId,
        id: Value,
        fields: Map<String, Value>,
    },
}

/// One frame for a link's writer: its JSON text, and the operation it carries (an `op`'s id), so
/// that a split of it the extension cannot join fails that operation (`super::server`).
#[derive(Debug)]
pub struct Outgoing {
    pub text: String,
    pub op: Option<u64>,
}

impl From<String> for Outgoing {
    fn from(text: String) -> Self {
        Self { text, op: None }
    }
}

/// The frames a link's writer sends (it splits those over the limit, `super::chunk`).
pub type Outbox = mpsc::UnboundedSender<Outgoing>;

/// Every link (module doc). Cheap to clone.
#[derive(Clone)]
pub struct Links(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    changes: Changes,
    /// How many links there are ([`Links::linked`]).
    count: watch::Sender<usize>,
    /// Operation ids, one counter for every link.
    next_op: AtomicU64,
}

#[derive(Default)]
struct State {
    next_link: LinkId,
    links: BTreeMap<LinkId, Entry>,
    subscribers: Vec<mpsc::UnboundedSender<Incoming>>,
}

struct Entry {
    info: LinkInfo,
    out: Outbox,
    /// The operations waiting for their `op_result`.
    pending: HashMap<u64, oneshot::Sender<Result<Value, OpError>>>,
}

/// An operation that waits for its result ([`Links::op`]): when it goes without one (cancelled, or
/// its future dropped), its entry is removed and the extension is sent `cancel`, if the call still
/// waited.
struct Waiting<'a> {
    links: &'a Links,
    link: LinkId,
    id: u64,
    armed: bool,
}

impl Waiting<'_> {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut st = self.links.state();
        if let Some(entry) = st.links.get_mut(&self.link)
            && entry.pending.remove(&self.id).is_some()
        {
            let _ = entry
                .out
                .send(json!({"t": "cancel", "id": self.id}).to_string().into());
        }
    }
}

impl Links {
    /// `changes` is the host registry's signal (`Deps::changes`).
    pub fn new(changes: Changes) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            changes,
            count: watch::channel(0).0,
            next_op: AtomicU64::new(1),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How many extensions are linked, now and as it moves.
    pub fn linked(&self) -> watch::Receiver<usize> {
        self.0.count.subscribe()
    }

    /// The links now, in the order they came.
    pub fn list(&self) -> Vec<LinkInfo> {
        self.state()
            .links
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Runs `op` with `args` on `link` and waits for its result: until the extension answers,
    /// `cancel` fires or the future is dropped (the extension is sent `cancel`; a result after it
    /// is dropped), or the link ends.
    pub async fn op(
        &self,
        link: LinkId,
        op: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, OpError> {
        self.op_as(self.reserve(), link, op, args, cancel).await
    }

    /// A new operation id, for [`Links::op_as`]: the network gate's questions name the operation
    /// whose gate holds the request by it (`ask`'s `op`), so the browser tools know it before they
    /// send the operation.
    pub fn reserve(&self) -> u64 {
        self.0.next_op.fetch_add(1, Ordering::Relaxed)
    }

    /// [`Links::op`] with the id [`Links::reserve`] gave.
    pub async fn op_as(
        &self,
        id: u64,
        link: LinkId,
        op: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, OpError> {
        if cancel.is_cancelled() {
            return Err(OpError::Cancelled);
        }
        let frame = json!({"t": "op", "id": id, "op": op, "args": args}).to_string();
        self.op_frame(id, link, op, frame, cancel).await
    }

    /// [`Links::op_as`] with the `op` frame as its whole JSON text (`{"t":"op","id":<id>,"op":<op>,
    /// "args":{…}}`), which the caller wrote: a file's bytes go into it as they are read
    /// (`element_file`), so no second copy of them is made.
    pub async fn op_frame(
        &self,
        id: u64,
        link: LinkId,
        op: &str,
        frame: String,
        cancel: &CancellationToken,
    ) -> Result<Value, OpError> {
        if cancel.is_cancelled() {
            return Err(OpError::Cancelled);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.state();
            let entry = st.links.get_mut(&link).ok_or(OpError::NoLink(link))?;
            if !entry.info.ops.iter().any(|o| o == op) {
                return Err(OpError::NotServed { op: op.to_owned() });
            }
            let out = Outgoing {
                text: frame,
                op: Some(id),
            };
            if entry.out.send(out).is_err() {
                return Err(OpError::Closed);
            }
            entry.pending.insert(id, tx);
        }
        let waiting = Waiting {
            links: self,
            link,
            id,
            armed: true,
        };
        tokio::select! {
            biased;
            r = rx => {
                // Answered, or the link ended: either took the entry.
                waiting.disarm();
                r.unwrap_or(Err(OpError::Closed))
            }
            // `waiting` withdraws the call as it goes.
            () = cancel.cancelled() => Err(OpError::Cancelled),
        }
    }

    /// The calls of `link` waiting for their result.
    #[cfg(test)]
    pub(super) fn waiting(&self, link: LinkId) -> usize {
        self.state().links.get(&link).map_or(0, |e| e.pending.len())
    }

    /// The `event` and `ask` frames of every link from now on. An `ask` that comes while nobody
    /// subscribes is answered `allow: false` at once (nothing in Kai could ask the owner).
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Incoming> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state().subscribers.push(tx);
        rx
    }

    /// Answers the `ask` `id` of `link`.
    pub fn answer(&self, link: LinkId, id: Value, allow: bool) -> Result<(), OpError> {
        let st = self.state();
        let entry = st.links.get(&link).ok_or(OpError::NoLink(link))?;
        entry
            .out
            .send(
                json!({"t": "answer", "id": id, "allow": allow})
                    .to_string()
                    .into(),
            )
            .map_err(|_| OpError::Closed)
    }

    /// A link whose `hello` was welcomed (`super::server`); its frames go to `out`.
    pub(super) fn add(&self, mut info: LinkInfo, out: Outbox) -> LinkId {
        let id = {
            let mut st = self.state();
            st.next_link += 1;
            let id = st.next_link;
            info.id = id;
            st.links.insert(
                id,
                Entry {
                    info,
                    out,
                    pending: HashMap::new(),
                },
            );
            id
        };
        self.counted();
        self.0.changes.signal();
        self.incoming(Incoming::Linked { link: id });
        id
    }

    fn counted(&self) {
        let n = self.state().links.len();
        self.0
            .count
            .send_if_modified(|c| std::mem::replace(c, n) != n);
    }

    /// The link ended: its waiting operations fail with [`OpError::Closed`].
    pub(super) fn remove(&self, link: LinkId) {
        let gone = self.state().links.remove(&link);
        if let Some(entry) = gone {
            for (_, tx) in entry.pending {
                let _ = tx.send(Err(OpError::Closed));
            }
            self.counted();
            self.0.changes.signal();
        }
    }

    /// An `op_result` of `link`: `{id, ok}` or `{id, error: {code, message, …}}`.
    pub(super) fn op_result(&self, link: LinkId, mut frame: Map<String, Value>) {
        let Some(id) = frame.get("id").and_then(Value::as_u64) else {
            tracing::warn!(link, "browser: an op_result without a numeric id; ignored");
            return;
        };
        let waiting = self
            .state()
            .links
            .get_mut(&link)
            .and_then(|e| e.pending.remove(&id));
        let Some(tx) = waiting else {
            tracing::debug!(
                link,
                id,
                "browser: an op_result for no waiting call (cancelled?)"
            );
            return;
        };
        let result = match (frame.remove("error"), frame.remove("ok")) {
            (Some(error), _) => Err(OpError::Failed {
                code: error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
                    .to_owned(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_owned),
                about: About::of(&error),
            }),
            (None, Some(ok)) => Ok(ok),
            (None, None) => Err(OpError::failed(
                "no_result",
                "Kai for Firefox answered the call with neither a result nor an error",
            )),
        };
        let _ = tx.send(result);
    }

    /// The operation `id` of `link` ends with `error` without a result of the extension's (a
    /// split of it the extension could not join); `false` when it no longer waits.
    pub(super) fn fail(&self, link: LinkId, id: u64, error: OpError) -> bool {
        let waiting = self
            .state()
            .links
            .get_mut(&link)
            .and_then(|e| e.pending.remove(&id));
        waiting.is_some_and(|tx| tx.send(Err(error)).is_ok())
    }

    /// An `event` or `ask` of a link, to every subscriber.
    pub(super) fn incoming(&self, item: Incoming) {
        let ask = match &item {
            Incoming::Ask { link, id, .. } => Some((*link, id.clone())),
            Incoming::Event { .. } | Incoming::Linked { .. } => None,
        };
        let delivered = {
            let mut st = self.state();
            st.subscribers.retain(|s| s.send(item.clone()).is_ok());
            !st.subscribers.is_empty()
        };
        if !delivered && let Some((link, id)) = ask {
            tracing::info!(
                link,
                "browser: the extension's network gate asked, and nothing in Kai answers it yet; \
                 the request is refused"
            );
            let _ = self.answer(link, id, false);
        }
    }
}
