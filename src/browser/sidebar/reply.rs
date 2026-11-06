//! One sidebar request's answer on the link (module doc of `super`): the `res` head, the body's
//! `chunk` frames under the window, and the `end`.
//!
//! **The body's encoding** (`res.body`): `"text"` for a whole JSON or `text/*` answer that is
//! UTF-8, and for a streamed `text/event-stream` or JSON answer, decoded as SSE's own decoding does
//! (an invalid sequence becomes U+FFFD; a character split between two of lmgw's chunks is kept
//! whole); `"base64"` for everything else (pictures, recordings, exports), each piece decoding on
//! its own. Text pieces are cut between characters, so a piece's `data` is always whole text.
//!
//! **The window** ([`Window`]): with `browser.sidebar_ahead_mb` above 0, Kai sends a chunk only
//! while the chunk frames the sidebar has not acknowledged, this one included, stay within it
//! (one frame always goes, so a frame of up to 1 MiB never stalls); the `ack`'s `seq` is how many
//! chunks the sidebar took, counted like `end`'s. The page acks each piece as it takes it from its
//! port, for a whole answer as for a stream: a whole answer larger than the window would wait
//! forever for an ack that came only once it was complete. Nothing else waits: the head and the
//! `end` go at once. While the window is full Kai reads no more of a stream from lmgw, so lmgw's
//! connection carries the pressure.
//!
//! **Leaving the gateway** (a re-pair, a revocation: the core's `left` token) ends the request with
//! `end` and the error `gateway_left`, before its head as after it, so the sidebar shows it and asks
//! again; an `abort` and the link's end end it without a word, since nobody reads it any more.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::Done;
use super::turns::Mark;
use crate::browser::chunk::{Chunker, Key, NoRoom};
use crate::browser::frame::TO_FIREFOX_MAX;
use crate::browser::links::Outbox;
use crate::lmgw_link::proxy::Proxied;
use crate::lmgw_link::proxy::relay::{Answer, Body};
use crate::voice::link::Pending;

/// What a request's link says to its answer: the sidebar's acks, the request's cancellation (an
/// `abort`, the link's end), and the count of pieces sent, which bounds an ack (`super::Open::ack`).
pub(super) struct Acks {
    pub acked: watch::Receiver<u64>,
    pub cancel: CancellationToken,
    pub sent: Arc<AtomicU64>,
}

/// Where one request's answer goes (module doc).
pub struct Reply {
    id: Value,
    out: Outbox,
    acks: Acks,
    /// Kai left the gateway the request went to (module doc); none until [`Self::run`].
    left: CancellationToken,
    _open: Option<Done>,
}

/// Why a reply stops before its end.
enum Stopped {
    /// An `abort`, the link's end: nothing more goes.
    Quiet,
    /// Kai left the gateway: an `end` with the error goes.
    Left,
}

impl Reply {
    pub(super) fn new(id: Value, out: Outbox, acks: Acks, open: Option<Done>) -> Self {
        Self {
            id,
            out,
            acks,
            left: CancellationToken::new(),
            _open: open,
        }
    }

    /// A reply whose frames go to `out`, acknowledged through `acked` and cancelled by `cancel`,
    /// outside any link (the core's and the voice's tests).
    #[cfg(test)]
    pub fn detached(
        id: Value,
        out: Outbox,
        acked: watch::Receiver<u64>,
        cancel: CancellationToken,
    ) -> Self {
        let acks = Acks {
            acked,
            cancel,
            sent: Arc::default(),
        };
        Self::new(id, out, acks, None)
    }

    /// Kai's own refusal, in lmgw's error shape.
    pub fn refuse(self, status: u16, code: &str, message: &str) {
        self.refused(Proxied::refused(status, code, message));
    }

    /// A refusal as the settings window's proxy makes them, whole and at once.
    pub fn refused(self, p: Proxied) {
        let mut headers = Map::new();
        headers.insert("content-type".into(), p.mime.into());
        self.head(p.status, headers, true);
        let mut c = Chunker::new(Key::Id(self.id.clone()), TO_FIREFOX_MAX);
        // `super::request` checked the id's room.
        let Ok(pieces) = c.text(&String::from_utf8_lossy(&p.body)) else {
            return;
        };
        for f in pieces {
            let _ = self.out.send(f.into());
        }
        let _ = self.out.send(end(c, None).into());
    }

    /// Waits for lmgw's `answer` and relays it (module doc), until its end, the `abort`, the
    /// link's end or Kai leaving the gateway (`left`). `mark` (the turn this request starts,
    /// `super::turns`) ends with it, at once when lmgw refused the request.
    pub async fn run(
        mut self,
        answer: Pending<Result<Answer, Proxied>>,
        ahead_mb: u64,
        mark: Option<Mark>,
        left: CancellationToken,
    ) {
        self.left = left;
        let answer = tokio::select! {
            biased;
            () = self.acks.cancel.cancelled() => return,
            () = self.left.cancelled() => {
                let c = Chunker::new(Key::Id(self.id.clone()), TO_FIREFOX_MAX);
                let _ = self.out.send(end(c, Some(gateway_left())).into());
                return;
            }
            a = answer => a,
        };
        match answer {
            Err(p) => self.refused(p),
            Ok(a) => {
                // lmgw refuses a turn route before a turn begins: a refusal started none.
                let mark = match mark {
                    Some(m) if !(200..300).contains(&a.status) => {
                        m.no_turn();
                        None
                    }
                    m => m,
                };
                self.relay(a, ahead_mb).await;
                drop(mark);
            }
        }
    }

    /// Whichever stops the reply first (module doc).
    async fn stopped(&self) -> Stopped {
        tokio::select! {
            biased;
            () = self.acks.cancel.cancelled() => Stopped::Quiet,
            () = self.left.cancelled() => Stopped::Left,
        }
    }

    fn head(&self, status: u16, headers: Map<String, Value>, text: bool) {
        let frame = json!({
            "t": "res",
            "id": self.id,
            "status": status,
            "headers": headers,
            "body": if text { "text" } else { "base64" },
        });
        let _ = self.out.send(frame.to_string().into());
    }

    async fn relay(mut self, a: Answer, ahead_mb: u64) {
        let headers = for_sidebar(&a.headers);
        let kind = headers
            .get("content-type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let text = match &a.body {
            Body::Whole(b) => textual(&kind) && std::str::from_utf8(b).is_ok(),
            Body::Stream(_) => streamed_text(&kind),
        };
        self.head(a.status, headers, text);
        let mut w = Writer {
            chunker: Chunker::new(Key::Id(self.id.clone()), TO_FIREFOX_MAX),
            window: Window::new(ahead_mb),
            text,
            carry: Utf8Carry::default(),
        };
        let error = match self.body(a.body, &mut w).await {
            Ok(error) => error,
            Err(Stopped::Left) => Some(gateway_left()),
            Err(Stopped::Quiet) => return,
        };
        let _ = self.out.send(end(w.chunker, error).into());
    }

    /// The body's chunk frames; `Ok(Some(error))` when lmgw's stream broke off.
    async fn body(&mut self, body: Body, w: &mut Writer) -> Result<Option<Value>, Stopped> {
        match body {
            // A whole text answer is UTF-8 already (checked above): cut as it is, not copied.
            Body::Whole(b) => match std::str::from_utf8(&b) {
                Ok(text) if w.text => w.text_frames(text, self).await?,
                _ => w.push(&b, self).await?,
            },
            Body::Stream(mut s) => loop {
                let item = tokio::select! {
                    biased;
                    why = self.stopped() => return Err(why),
                    i = s.next() => i,
                };
                match item {
                    Some(Ok(b)) => w.push(&b, self).await?,
                    Some(Err(why)) => {
                        w.flush(self).await?;
                        return Ok(Some(json!({"code": "lmgw_broke_off", "message": why})));
                    }
                    None => break,
                }
            },
        }
        w.flush(self).await?;
        Ok(None)
    }
}

/// The `end`'s error when Kai left the gateway (module doc).
fn gateway_left() -> Value {
    json!({
        "code": "gateway_left",
        "message": "Kai left the lmgw this request went to (a new pairing, or its key was \
                    revoked); ask again once Kai is linked",
    })
}

fn end(c: Chunker, error: Option<Value>) -> String {
    let mut frame: Map<String, Value> = serde_json::from_str(&c.end()).unwrap_or_default();
    if let Some(e) = error {
        frame.insert("error".into(), e);
    }
    Value::Object(frame).to_string()
}

/// The headers the sidebar reads (module doc of `super`), names in lower case; a repeated one's
/// values joined by `, `.
fn for_sidebar(headers: &[(String, String)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in headers {
        let k = k.to_ascii_lowercase();
        let read = matches!(
            k.as_str(),
            "content-type" | "content-disposition" | "retry-after"
        ) || k.starts_with("x-lmgw-");
        if !read {
            continue;
        }
        match m.get_mut(&k) {
            Some(Value::String(s)) => {
                s.push_str(", ");
                s.push_str(v);
            }
            _ => {
                m.insert(k, Value::String(v.clone()));
            }
        }
    }
    m
}

fn essence(kind: &str) -> String {
    kind.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn json_type(e: &str) -> bool {
    e == "application/json" || (e.starts_with("application/") && e.ends_with("+json"))
}

/// A whole answer of this type goes as text when it is UTF-8.
fn textual(kind: &str) -> bool {
    let e = essence(kind);
    e.starts_with("text/") || json_type(&e)
}

/// A streamed answer of this type goes as text (module doc).
fn streamed_text(kind: &str) -> bool {
    let e = essence(kind);
    e == "text/event-stream" || json_type(&e)
}

/// A body's chunk frames as they are made.
struct Writer {
    chunker: Chunker,
    window: Window,
    text: bool,
    carry: Utf8Carry,
}

impl Writer {
    async fn push(&mut self, bytes: &[u8], r: &mut Reply) -> Result<(), Stopped> {
        if self.text {
            let text = self.carry.push(bytes);
            self.text_frames(&text, r).await
        } else {
            let mut rest = bytes;
            while !rest.is_empty() {
                let f = self.chunker.bytes_piece(&mut rest).map_err(no_room)?;
                self.send(f, r).await?;
            }
            Ok(())
        }
    }

    /// What a text body still holds back (an unfinished character) goes as U+FFFD.
    async fn flush(&mut self, r: &mut Reply) -> Result<(), Stopped> {
        let rest = self.carry.finish();
        self.text_frames(&rest, r).await
    }

    async fn text_frames(&mut self, text: &str, r: &mut Reply) -> Result<(), Stopped> {
        let mut rest = text;
        while !rest.is_empty() {
            let f = self.chunker.piece(&mut rest).map_err(no_room)?;
            self.send(f, r).await?;
        }
        Ok(())
    }

    async fn send(&mut self, frame: String, r: &mut Reply) -> Result<(), Stopped> {
        let seq = self.chunker.count() - 1;
        self.window.room(frame.len() as u64, r).await?;
        self.window.sent(seq, frame.len() as u64);
        // Counted before it goes, so an ack of it is never above the count.
        r.acks.sent.store(seq + 1, Ordering::Release);
        r.out.send(frame.into()).map_err(|_| Stopped::Quiet)
    }
}

fn no_room(_: NoRoom) -> Stopped {
    // `super::request` checked that the id leaves room.
    Stopped::Quiet
}

/// The chunk frames sent and not acknowledged yet (module doc).
pub(super) struct Window {
    /// `browser.sidebar_ahead_mb` in bytes; `None`: no bound.
    ahead: Option<u64>,
    /// (seq, bytes) of each frame not acknowledged, oldest first.
    inflight: VecDeque<(u64, u64)>,
    unacked: u64,
}

impl Window {
    pub(super) fn new(ahead_mb: u64) -> Self {
        Self {
            ahead: (ahead_mb > 0).then(|| ahead_mb.saturating_mul(1024 * 1024)),
            inflight: VecDeque::new(),
            unacked: 0,
        }
    }

    /// The sidebar took the first `taken` chunks.
    pub(super) fn acked(&mut self, taken: u64) {
        while let Some(&(seq, n)) = self.inflight.front() {
            if seq >= taken {
                break;
            }
            self.inflight.pop_front();
            self.unacked -= n;
        }
    }

    pub(super) fn fits(&self, len: u64) -> bool {
        self.ahead
            .is_none_or(|max| self.unacked == 0 || self.unacked + len <= max)
    }

    pub(super) fn sent(&mut self, seq: u64, len: u64) {
        self.inflight.push_back((seq, len));
        self.unacked += len;
    }

    /// Waits until a frame of `len` bytes fits: the sidebar acknowledged enough, or never (the
    /// request was aborted, the link ended, Kai left the gateway).
    async fn room(&mut self, len: u64, r: &mut Reply) -> Result<(), Stopped> {
        loop {
            self.acked(*r.acks.acked.borrow_and_update());
            if self.fits(len) {
                return Ok(());
            }
            let mut acked = r.acks.acked.clone();
            tokio::select! {
                biased;
                why = r.stopped() => return Err(why),
                changed = acked.changed() => changed.map_err(|_| Stopped::Quiet)?,
            }
        }
    }
}

/// A text stream's bytes as text, a character split between two chunks kept whole (module doc).
#[derive(Default)]
pub(super) struct Utf8Carry(Vec<u8>);

impl Utf8Carry {
    /// The text of what came so far, an unfinished character at the end held back.
    pub(super) fn push(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest: &[u8] = &self.0;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    out.push_str(std::str::from_utf8(good).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{fffd}');
                            rest = &bad[n..];
                        }
                        None => {
                            rest = bad;
                            break;
                        }
                    }
                }
            }
        }
        let tail = rest.to_vec();
        self.0 = tail;
        out
    }

    /// The end: a character still unfinished is U+FFFD.
    pub(super) fn finish(&mut self) -> String {
        if self.0.is_empty() {
            String::new()
        } else {
            self.0.clear();
            "\u{fffd}".to_owned()
        }
    }
}
