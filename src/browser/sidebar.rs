//! Firefox's sidebar's requests (kai-firefox design §8.2 "Kai's proxy", §8.3; the wire is
//! kai-firefox's `docs/protocol/operations.md`, "The sidebar's requests"): the sidebar is lmgw's
//! Chat, and its every request comes as a `req` frame from the extension's background, which takes
//! them from the extension's own sidebar page only. Kai sends it to lmgw over its device link with
//! its own certificate (decision 9: lmgw sees the sidebar as Kai) and relays the answer.
//!
//! - **What goes** ([`admit`]): exactly the Chat's requests by method and path; the headers
//!   `content-type` and `last-event-id` (the feed's cursor, passed on as it came); a body as text or
//!   base64 inside the `req` frame, which the extension splits like any long frame. Anything else
//!   is refused with a `res` in lmgw's error shape before anything goes to lmgw.
//! - **What comes back** ([`reply`]): a `res` head (lmgw's status and the headers the Chat reads:
//!   `content-type`, `content-disposition`, `retry-after`, every `x-lmgw-*`), the body as `chunk`
//!   frames keyed by the request's `id`, and an `end` (with an `error` when lmgw's stream broke
//!   off). A whole answer is read first, under `window.proxy_max_mb`; a stream (`read: "stream"`:
//!   the send stream, the feed, read-aloud) is relayed as it comes, unbounded, its head bounded by
//!   `window.proxy_timeout_s`.
//! - **Flow control** ([`reply`]): Firefox's native messaging has no backpressure, so Kai keeps at
//!   most `browser.sidebar_ahead_mb` of one answer's chunk frames unacknowledged; the sidebar page
//!   acknowledges each piece as it takes it from its port (`ack`, cumulative), whole answers and
//!   streams alike, and Kai reads no more of lmgw's answer meanwhile. An ack counts at most the
//!   pieces Kai sent.
//! - **Cancel:** `abort` ends the request (lmgw sees the connection go: a send's turn stops) and
//!   Kai sends nothing more for it; a link that ends ends all of its requests. Leaving the gateway
//!   (a re-pair, a revocation) ends them with `end` and an `error` (`gateway_left`).
//! - **Turns** ([`turns`]): a request that starts a turn marks its thread, so Kai's voice tells the
//!   sidebar's turns from its own (design §8.3, K16 and K17).
//!
//! The log names a request's method and path without its query, never a header's value or a body.

pub mod admit;
pub mod reply;
pub mod site;
pub mod turns;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{Map, Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::chunk::{Chunker, Key};
use super::frame::TO_FIREFOX_MAX;
use super::links::{LinkId, Outbox};
use crate::lmgw_link::proxy::Method;
use crate::lmgw_link::proxy::relay::Outbound;
pub use reply::Reply;

/// The headers the sidebar may send (module doc), lower case.
pub const REQUEST_HEADERS: &[&str] = &["content-type", "last-event-id"];

/// One sidebar request, parsed and admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// What goes to lmgw.
    pub out: Outbound,
    /// The thread whose turn it starts, and how ([`admit::Admitted::turn`]).
    pub turn: Option<(i64, turns::Start)>,
}

/// A request for the core (`crate::core::Command::Sidebar`): what to send lmgw, and where the
/// answer goes.
pub struct Relay {
    pub request: Request,
    pub reply: Reply,
}

/// What hands an admitted request on: the core's command channel in Kai (`main`), a run of its own
/// in the tests.
pub type Proxy = Arc<dyn Fn(Relay) + Send + Sync>;

/// One link's open requests, by their `id`'s JSON text: what `ack` and `abort` reach. The server
/// keeps one per connection and [`Open::close`]s it when the link ends.
#[derive(Clone, Default)]
pub struct Open(Arc<Mutex<HashMap<String, Ctl>>>);

struct Ctl {
    acked: watch::Sender<u64>,
    cancel: CancellationToken,
    /// The answer's pieces Kai sent so far: what an `ack` may count at most.
    sent: Arc<AtomicU64>,
}

/// Removes its request from [`Open`] when the request ends (its [`Reply`] goes).
pub(super) struct Done {
    open: Open,
    key: String,
}

impl Drop for Done {
    fn drop(&mut self) {
        self.open.lock().remove(&self.key);
    }
}

impl Open {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Ctl>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new request `key`: its acknowledgements, its cancellation and its count of pieces sent,
    /// or `None` while one of that `id` is open.
    fn begin(&self, key: &str) -> Option<(reply::Acks, Done)> {
        let mut m = self.lock();
        if m.contains_key(key) {
            return None;
        }
        let (acked, rx) = watch::channel(0);
        let cancel = CancellationToken::new();
        let sent = Arc::new(AtomicU64::new(0));
        m.insert(
            key.to_owned(),
            Ctl {
                acked,
                cancel: cancel.clone(),
                sent: sent.clone(),
            },
        );
        Some((
            reply::Acks {
                acked: rx,
                cancel,
                sent,
            },
            Done {
                open: self.clone(),
                key: key.to_owned(),
            },
        ))
    }

    /// An `ack` frame: `seq` chunks of the answer `id` taken. A smaller count than before, or one
    /// for no open request, changes nothing; a count above the pieces Kai sent counts those (it
    /// would take the window away from the request) and is logged.
    pub fn ack(&self, link: LinkId, frame: &Map<String, Value>) {
        let (Some(id), Some(seq)) = (frame.get("id"), frame.get("seq").and_then(Value::as_u64))
        else {
            tracing::warn!(
                link,
                "browser: an ack without an id and a whole-number seq; ignored"
            );
            return;
        };
        if let Some(c) = self.lock().get(&id.to_string()) {
            let sent = c.sent.load(Ordering::Acquire);
            let seq = if seq > sent {
                tracing::warn!(
                    link,
                    seq,
                    sent,
                    "browser: an ack for more pieces than Kai sent; counted as the pieces sent"
                );
                sent
            } else {
                seq
            };
            c.acked.send_if_modified(|n| {
                let more = seq > *n;
                if more {
                    *n = seq;
                }
                more
            });
        }
    }

    /// An `abort` frame: the request ends, and nothing more is sent for it.
    pub fn abort(&self, link: LinkId, frame: &Map<String, Value>) {
        let Some(id) = frame.get("id") else {
            tracing::warn!(link, "browser: an abort without an id; ignored");
            return;
        };
        match self.lock().get(&id.to_string()) {
            Some(c) => c.cancel.cancel(),
            None => tracing::debug!(link, "browser: an abort for no open request (it ended)"),
        }
    }

    /// The link ended: every open request ends.
    pub fn close(&self) {
        for (_, c) in self.lock().drain() {
            c.cancel.cancel();
        }
    }
}

/// A `req` frame of `link` (module doc): refused at once, or handed to `proxy`.
pub fn request(
    link: LinkId,
    frame: &Map<String, Value>,
    out: &Outbox,
    open: &Open,
    proxy: Option<&Proxy>,
) {
    let Some(id) = frame.get("id").cloned() else {
        tracing::warn!(link, "browser: a req without an id; ignored");
        return;
    };
    if let Err(e) = Chunker::new(Key::Id(id.clone()), TO_FIREFOX_MAX).check() {
        tracing::warn!(
            link,
            "browser: a req whose id leaves no room for its answer: {e}"
        );
        let _ = out.send(
            json!({
                "t": "error",
                "code": "id_too_long",
                "size": e.key,
                "limit": e.limit,
                "message": format!("Kai cannot answer a req with this id: {e}"),
            })
            .to_string()
            .into(),
        );
        return;
    }
    let key = id.to_string();
    let Some((acks, done)) = open.begin(&key) else {
        tracing::warn!(link, "browser: a req whose id is open already; ignored");
        let _ = out.send(
            json!({
                "t": "error",
                "code": "duplicate_req",
                "id": id,
                "message": "a request with this id is open on this link; the second was not sent",
            })
            .to_string()
            .into(),
        );
        return;
    };
    let reply = Reply::new(id, out.clone(), acks, Some(done));
    let request = match parse(frame) {
        Ok(r) => r,
        Err((status, code, message)) => {
            tracing::info!(link, status, code, "browser: a sidebar request refused");
            return reply.refuse(status, code, &message);
        }
    };
    match proxy {
        Some(p) => p(Relay { request, reply }),
        None => reply.refuse(503, "no_proxy", "this Kai forwards no sidebar requests"),
    }
}

/// The `req` frame's fields as a [`Request`] (module doc), or Kai's refusal of it.
fn parse(frame: &Map<String, Value>) -> Result<Request, admit::Refused> {
    let bad = |message: String| (400, "bad_req", message);
    let text = |k: &str| frame.get(k).and_then(Value::as_str);
    let (Some(method), Some(path)) = (text("method"), text("path")) else {
        return Err(bad("a req needs its method and path as text".into()));
    };
    let admitted = admit::admit(method, path)?;
    let mut headers = Vec::new();
    match frame.get("headers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(h)) => {
            for (name, value) in h {
                let lower = name.to_ascii_lowercase();
                if !REQUEST_HEADERS.contains(&lower.as_str()) {
                    return Err((
                        400,
                        "header_not_admitted",
                        format!(
                            "Kai forwards only the headers {} for the sidebar; {name} is not one",
                            REQUEST_HEADERS.join(" and ")
                        ),
                    ));
                }
                let Some(value) = value.as_str() else {
                    return Err(bad(format!("the header {name}'s value is not text")));
                };
                if reqwest::header::HeaderValue::from_str(value).is_err() {
                    return Err(bad(format!(
                        "the header {name}'s value has characters a header cannot carry"
                    )));
                }
                if headers.iter().any(|(k, _)| *k == lower) {
                    return Err(bad(format!("the header {lower} comes twice")));
                }
                headers.push((lower, value.to_owned()));
            }
        }
        Some(_) => {
            return Err(bad(
                "a req's headers are an object of names and texts".into()
            ));
        }
    }
    let body = match frame.get("body") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(b)) => match (b.get("text"), b.get("base64"), b.len()) {
            (Some(Value::String(t)), None, 1) => t.clone().into_bytes(),
            (None, Some(Value::String(d)), 1) => base64::engine::general_purpose::STANDARD
                .decode(d)
                .map_err(|e| bad(format!("the body's base64 does not decode: {e}")))?,
            _ => {
                return Err(bad(
                    "a req's body is {\"text\": …} or {\"base64\": …}, or none".into(),
                ));
            }
        },
        Some(_) => {
            return Err(bad(
                "a req's body is {\"text\": …} or {\"base64\": …}, or none".into(),
            ));
        }
    };
    if admitted.method == Method::Get && !body.is_empty() {
        return Err(bad("a GET carries no body".into()));
    }
    let stream = match frame.get("read") {
        None | Some(Value::Null) => false,
        Some(Value::String(r)) if r == "whole" => false,
        Some(Value::String(r)) if r == "stream" => true,
        Some(other) => {
            return Err(bad(format!(
                "a req's read is \"whole\" or \"stream\", not {other}"
            )));
        }
    };
    Ok(Request {
        out: Outbound {
            method: admitted.method,
            path: admitted.path,
            headers,
            body,
            stream,
        },
        turn: admitted.turn,
    })
}
