//! Kai's end of `browser.sock` (module doc of `crate::browser`): the peer check, the handshake,
//! each link's frames (the extension's split ones joined), and its writer, which splits what is
//! too long for Firefox.
//!
//! **A split the extension cannot join** is answered with `{"t":"error","code":"bad_split",
//! "split":<n>}` and no `op_result`, so the writer keeps which operation each of its splits
//! carried ([`Splits`]) and such an error fails that operation at once with the extension's
//! words, instead of leaving it to wait until lmgw gives up. Every other `error` frame of the
//! extension's is logged at `warn` the first time its code comes on a link, then counted at
//! `debug`.
//!
//! The log names links, profiles, versions and frame kinds, never an operation's arguments or
//! results, a page's text or a request's body.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use super::PROTOCOL;
use super::chunk::{BadSplit, Chunker, Joiner, Key, Piece};
use super::frame::{self, TO_FIREFOX_MAX};
use super::links::{Incoming, LinkId, LinkInfo, Links, OpError, Outbox, Outgoing};
use super::peer::{self, Peer};
use super::sidebar::{self, Open, Proxy};
use crate::config::Config;
use crate::local_socket::PeerProcess;

/// What the socket serves and whom it lets in.
pub struct Server {
    pub links: Links,
    /// The configuration file, read again at every connection for `browser.peer_scopes`; `None`:
    /// only `scopes` (the tests).
    pub config_file: Option<PathBuf>,
    /// `browser.peer_scopes` as read at Kai's start: used while the file does not load.
    pub scopes: Vec<String>,
    /// The peer process by its pid ([`peer::proc_peer`]; the tests give theirs).
    pub peer_of: fn(i32) -> Option<Peer>,
    /// Where the sidebar's admitted requests go (`sidebar::Proxy`: the core in Kai); `None`
    /// refuses them 503.
    pub sidebar: Option<Proxy>,
    /// Kai's site rules now, for the sidebar's `site` questions (`sidebar::site`); `None`: no
    /// rules (every site is `act`; the tests').
    pub sites: Option<sidebar::site::Rules>,
}

struct Shared {
    server: Server,
    said: SaidOnce,
}

/// What is logged at `warn` once and at `debug` after: the host tries again every `retry_ms`,
/// so a refused peer, a refused hello or a configuration that does not load repeats for as long as
/// it lasts.
#[derive(Default)]
struct SaidOnce(Mutex<HashSet<String>>);

impl SaidOnce {
    /// Whether `key` is said for the first time.
    fn first(&self, key: String) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(key)
    }
}

impl Server {
    /// `browser.peer_scopes` now, and why the file did not load when it did not (the scopes are
    /// then those read at Kai's start). Reads the file: called off the async runtime.
    fn scopes(&self) -> (Vec<String>, Option<String>) {
        let Some(file) = &self.config_file else {
            return (self.scopes.clone(), None);
        };
        match Config::load(file) {
            Ok(c) => (c.browser.peer_scopes, None),
            Err(e) => (self.scopes.clone(), Some(e.to_string())),
        }
    }
}

/// Serves `browser.sock` at `path` until Kai's runtime ends; returns only when the socket cannot
/// be bound, with why (logged here too).
pub async fn serve(path: PathBuf, server: Server) -> std::io::Result<()> {
    let listener = match crate::local_socket::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                "browser: cannot serve {}: {e}; Kai for Firefox cannot link to this Kai",
                path.display()
            );
            return Err(e);
        }
    };
    let _socket = crate::local_socket::Unlink(path.clone());
    tracing::info!(socket = %path.display(), "browser: serving Kai for Firefox");
    let shared = Arc::new(Shared {
        server,
        said: SaidOnce::default(),
    });
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, shared.clone()));
            }
            Err(e) => {
                tracing::warn!("browser: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// One connection: the peer checked, the handshake, then its frames until it closes.
async fn connection(stream: UnixStream, shared: Arc<Shared>) {
    let admitted = admit(&stream, &shared).await;
    let (rd, wr) = stream.into_split();
    let (out, rx) = mpsc::unbounded_channel();
    let splits = Splits::default();
    let writer = tokio::spawn(write_loop(wr, rx, splits.clone()));
    let mut rd = BufReader::new(rd);
    let link = match admitted {
        Ok((unit, firefox)) => handshake(&mut rd, &out, &shared, &unit, firefox).await,
        Err(reason) => {
            let _ = out.send(json!({"t": "refused", "reason": reason}).to_string().into());
            None
        }
    };
    if let Some(id) = link {
        let linked = Linked {
            links: &shared.server.links,
            id,
        };
        // The sidebar's requests end with the link, however its task ends.
        let open = Closing(Open::default());
        let why = frames(
            &mut rd,
            id,
            &out,
            &shared.server.links,
            &splits,
            (
                &open.0,
                shared.server.sidebar.as_ref(),
                shared.server.sites.as_ref(),
            ),
        )
        .await;
        drop(open);
        drop(linked);
        tracing::info!(link = id, "browser: Kai for Firefox's link closed ({why})");
    }
    drop(out);
    let _ = writer.await;
}

/// Which operation each of a connection's splits carried (module doc): the writer adds one as it
/// splits an `op`, the reader takes it out at the operation's result or at the extension's
/// `bad_split` for it.
#[derive(Clone, Default)]
struct Splits(Arc<Mutex<HashMap<u64, u64>>>);

impl Splits {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, u64>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sent(&self, split: u64, op: u64) {
        self.lock().insert(split, op);
    }

    /// The operation answered: its split is done with.
    fn answered(&self, op: u64) {
        self.lock().retain(|_, o| *o != op);
    }

    fn take(&self, split: u64) -> Option<u64> {
        self.lock().remove(&split)
    }
}

/// A link of this connection: removed from [`Links`] however the connection's task ends (its
/// frames end, a panic, the task aborted), so no link outlives its connection.
struct Linked<'a> {
    links: &'a Links,
    id: LinkId,
}

impl Drop for Linked<'_> {
    fn drop(&mut self) {
        self.links.remove(self.id);
    }
}

/// A link's open sidebar requests ([`Open`]), ended when the link's task ends in any way.
struct Closing(Open);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The peer let in, with the unit it runs in and its parent (Firefox's main process,
/// `LinkInfo::firefox_pid`), or why not (logged here: the first time a peer is refused at `warn`,
/// again at `debug`). `/proc` and the configuration file are read on a blocking thread, not on the
/// runtime's workers.
async fn admit(stream: &UnixStream, shared: &Arc<Shared>) -> Result<(String, Option<u32>), String> {
    let cred = stream.peer_cred().map_err(|e| {
        tracing::warn!("browser: a peer's credentials cannot be read ({e}); refused");
        format!("Kai could not read who connected ({e})")
    })?;
    let process = crate::local_socket::peer_process(stream, cred.pid());
    let uid = cred.uid();
    let checked = tokio::task::spawn_blocking({
        let shared = shared.clone();
        move || check(&shared, uid, &process)
    })
    .await;
    let (result, peer, config_error) = match checked {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("browser: a peer's check did not finish ({e}); refused");
            return Err(format!("Kai's check of who connected did not finish ({e})"));
        }
    };
    if let Some(e) = config_error {
        let message = "browser: the configuration does not load; the peer check uses \
                       browser.peer_scopes as Kai read it at its start";
        if shared.said.first(format!("config\n{e}")) {
            tracing::warn!(error = %e, "{message} (said again at debug)");
        } else {
            tracing::debug!(error = %e, "{message}");
        }
    }
    let parent = peer
        .as_ref()
        .and_then(|p| p.parent)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 1);
    result.map(|unit| (unit, parent)).map_err(|reason| {
        let exe = peer.as_ref().map_or("(unknown)", Peer::exe_or_unknown);
        let cgroup = peer.as_ref().map_or("(unknown)", Peer::cgroup_line);
        if shared.said.first(format!("peer\n{uid}\n{exe}\n{cgroup}")) {
            tracing::warn!(
                uid,
                exe,
                cgroup,
                "browser: a connection refused (later ones from it are logged at debug): {reason}"
            );
        } else {
            tracing::debug!(uid, exe, cgroup, "browser: a connection refused again");
        }
        reason
    })
}

/// The peer check's blocking part: the peer read in `/proc`, the scopes in the configuration file.
/// The check's answer, the peer as read and why the file did not load.
fn check(
    shared: &Shared,
    uid: u32,
    process: &PeerProcess,
) -> (Result<String, String>, Option<Peer>, Option<String>) {
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    let peer = process.read(shared.server.peer_of);
    let (scopes, config_error) = shared.server.scopes();
    (
        peer::check(uid, me, peer.as_ref(), &scopes),
        peer,
        config_error,
    )
}

/// What an extension's `hello` carries (design §4.2). Fields a later extension adds are read past.
#[derive(Deserialize)]
struct Hello {
    ext_version: String,
    protocol: u32,
    ops: Vec<String>,
    firefox: String,
    profile: String,
}

/// A frame that came before the link.
#[derive(Debug)]
enum First {
    /// A welcome `hello`: the link it asks for.
    Hello(LinkInfo),
    /// A frame of another type (named), read past: a late frame of the profile's earlier link,
    /// which the host passed on as this connection began.
    Other(String),
}

/// What a frame before the link is, or why the connection is refused (said in the `refused`
/// frame): a frame that is no JSON object with a type, a `hello` Kai cannot read, or one of
/// another protocol.
fn first(body: &[u8]) -> Result<First, String> {
    let frame: Value = serde_json::from_slice(body)
        .map_err(|e| format!("Kai could not read the extension's first frame: {e}"))?;
    match frame.get("t").and_then(Value::as_str) {
        Some("hello") => {}
        Some(other) => return Ok(First::Other(other.to_owned())),
        None => {
            return Err(
                "the extension's first frame must be its hello, not a frame without a type".into(),
            );
        }
    }
    let h: Hello = serde_json::from_value(frame)
        .map_err(|e| format!("Kai could not read the extension's hello: {e}"))?;
    if h.protocol > PROTOCOL {
        return Err(format!(
            "update Kai: Kai for Firefox speaks protocol {}, this Kai protocol {PROTOCOL}",
            h.protocol
        ));
    }
    if h.protocol < PROTOCOL {
        return Err(format!(
            "update the extension: it speaks protocol {}, this Kai protocol {PROTOCOL}",
            h.protocol
        ));
    }
    Ok(First::Hello(LinkInfo {
        id: 0,
        profile: h.profile,
        ext_version: h.ext_version,
        firefox: h.firefox,
        protocol: h.protocol,
        ops: h.ops,
        firefox_pid: None,
    }))
}

/// The frames before the link: a welcomed `hello` makes it. A frame of another type is read past
/// (counted, at `debug`); one longer than Firefox's limit for a message, which a `hello` never
/// is, is refused before Kai reads it.
async fn handshake(
    rd: &mut BufReader<OwnedReadHalf>,
    out: &Outbox,
    shared: &Shared,
    unit: &str,
    firefox_pid: Option<u32>,
) -> Option<LinkId> {
    let mut skipped = 0u64;
    loop {
        let len = match frame::read_len(rd).await {
            Ok(Some(len)) => len,
            Ok(None) => {
                tracing::debug!(skipped, "browser: a connection closed before its hello");
                return None;
            }
            Err(e) => {
                tracing::warn!("browser: a connection's first frame did not arrive whole: {e}");
                return None;
            }
        };
        if len as usize > TO_FIREFOX_MAX {
            refuse(
                shared,
                out,
                format!(
                    "the extension sent a frame of {len} bytes before its hello; until the hello \
                     Kai takes frames of at most {TO_FIREFOX_MAX} bytes (Firefox's limit for one \
                     message), and did not read it"
                ),
            );
            return None;
        }
        let body = match frame::read_body(rd, len).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("browser: a connection's first frame did not arrive whole: {e}");
                return None;
            }
        };
        let info = match first(&body) {
            Ok(First::Hello(info)) => LinkInfo {
                firefox_pid,
                ..info
            },
            Ok(First::Other(t)) => {
                skipped += 1;
                tracing::debug!(
                    t,
                    skipped,
                    "browser: a frame before the extension's hello (a late one of its earlier \
                     link?); read past"
                );
                continue;
            }
            Err(reason) => {
                refuse(shared, out, reason);
                return None;
            }
        };
        let _ = out.send(
            json!({"t": "welcome", "kai_version": crate::VERSION, "protocol": PROTOCOL, "takes": TAKES})
                .to_string()
                .into(),
        );
        let (profile, version, firefox, ops) = (
            info.profile.clone(),
            info.ext_version.clone(),
            info.firefox.clone(),
            info.ops.len(),
        );
        let id = shared.server.links.add(info, out.clone());
        tracing::info!(
            link = id,
            profile,
            extension = version,
            firefox,
            ops,
            unit,
            firefox_pid,
            skipped,
            "browser: Kai for Firefox linked"
        );
        return Some(id);
    }
}

/// Refuses the connection before its link with `reason` (logged at `warn` the first time, at
/// `debug` after: the host connects again every `retry_ms`).
fn refuse(shared: &Shared, out: &Outbox, reason: String) {
    if shared.said.first(format!("hello\n{reason}")) {
        tracing::warn!(
            "browser: a connection refused at its handshake (later ones for the same reason are \
             logged at debug): {reason}"
        );
    } else {
        tracing::debug!("browser: refused again at the handshake: {reason}");
    }
    let _ = out.send(json!({"t": "refused", "reason": reason}).to_string().into());
}

/// The link's frames until it closes; why it ended. The extension's split frames are joined
/// first (`chunk::Joiner`); one still open when the link closes is dropped with it.
async fn frames(
    rd: &mut BufReader<OwnedReadHalf>,
    link: LinkId,
    out: &Outbox,
    links: &Links,
    splits: &Splits,
    sidebar: Sidebar<'_>,
) -> String {
    let mut joiner = Joiner::default();
    let mut errors = Errors::default();
    let why = loop {
        let body = match frame::read(rd).await {
            Ok(Some(b)) => b,
            Ok(None) => break "Firefox closed it".to_owned(),
            Err(e) => break format!("a frame did not arrive whole: {e}"),
        };
        let mut frame = match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(frame)) => frame,
            _ => {
                if let Some(e) = joiner.interrupted(None, &Map::new()) {
                    bad_split(out, link, &e);
                }
                tracing::warn!(
                    link,
                    bytes = body.len(),
                    "browser: a frame that is no JSON object; ignored"
                );
                continue;
            }
        };
        let t = frame
            .remove("t")
            .and_then(|t| t.as_str().map(str::to_owned));
        if let Some(e) = joiner.interrupted(t.as_deref(), &frame) {
            bad_split(out, link, &e);
        }
        let (t, frame) = match t.as_deref() {
            Some(piece @ ("chunk" | "end")) => match joiner.piece(piece, &frame) {
                Ok(Piece::Taken) => continue,
                Ok(Piece::Frame(mut whole)) => {
                    let t = whole
                        .remove("t")
                        .and_then(|t| t.as_str().map(str::to_owned));
                    (t, whole)
                }
                Err(e) => {
                    bad_split(out, link, &e);
                    continue;
                }
            },
            _ => (t, frame),
        };
        match t.as_deref() {
            Some("error") => errors.frame(link, &frame, links, splits),
            Some("op_result") => {
                if let Some(id) = frame.get("id").and_then(Value::as_u64) {
                    splits.answered(id);
                }
                links.op_result(link, frame);
            }
            other => dispatch(link, other, frame, out, links, sidebar),
        }
    };
    if let Some(split) = joiner.open() {
        tracing::debug!(
            link,
            split,
            "browser: a split frame the link left unfinished, dropped"
        );
    }
    why
}

/// A split the extension sent that cannot be joined: logged, and said in an `error` frame.
fn bad_split(out: &Outbox, link: LinkId, e: &BadSplit) {
    tracing::warn!(link, split = e.split, "browser: a split frame dropped: {e}");
    let _ = out.send(
        json!({"t": "error", "code": "bad_split", "split": e.split, "message": e.message})
            .to_string()
            .into(),
    );
}

/// A link's `error` frames, counted by code (module doc).
#[derive(Default)]
struct Errors(HashMap<String, u64>);

impl Errors {
    fn frame(&mut self, link: LinkId, frame: &Map<String, Value>, links: &Links, splits: &Splits) {
        let code = frame
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("(none)")
            .to_owned();
        let message = frame
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let split = frame.get("split").and_then(Value::as_u64);
        if code == "bad_split"
            && let Some(op) = split.and_then(|s| splits.take(s))
        {
            let message = if message.is_empty() {
                format!("split {} could not be joined", split.unwrap_or_default())
            } else {
                message
            };
            let failed = links.fail(link, op, OpError::failed(code, message));
            tracing::warn!(
                link,
                split,
                op,
                failed,
                "browser: the extension could not join an operation Kai sent in pieces; the \
                 operation failed"
            );
            return;
        }
        let n = self.0.entry(code.clone()).or_default();
        *n += 1;
        if *n == 1 {
            tracing::warn!(
                link,
                code,
                split,
                "browser: the extension reported an error: {message} (more of this code on this \
                 link are counted at debug)"
            );
        } else {
            tracing::debug!(
                link,
                code,
                count = *n,
                "browser: the extension reported an error"
            );
        }
    }
}

/// The extension's frames Kai takes, listed in its `welcome` (`takes`), as the extension's `hello`
/// lists the operations it serves: an additive frame does not raise [`PROTOCOL`], so this is how
/// the extension tells a Kai older than a frame (kai-firefox's sidebar asks `site` only of a Kai
/// that lists it). Every type [`dispatch`] and the read loop take; a frame of another type with an
/// `id` is answered with an `error` frame `unknown_frame`.
pub(crate) const TAKES: &[&str] = &[
    "hello",
    "error",
    "op_result",
    "event",
    "ask",
    "req",
    "ack",
    "abort",
    "site",
    "chunk",
    "end",
];

/// What a link's sidebar frames reach: its open requests, the proxy, Kai's site rules.
type Sidebar<'a> = (
    &'a Open,
    Option<&'a Proxy>,
    Option<&'a sidebar::site::Rules>,
);

/// One whole frame of the extension's (its type `t`, its other fields `frame`).
fn dispatch(
    link: LinkId,
    t: Option<&str>,
    mut frame: Map<String, Value>,
    out: &Outbox,
    links: &Links,
    (open, proxy, sites): Sidebar<'_>,
) {
    match t {
        Some("event") => {
            // One event per frame (the contract's Events), in the link's `seq` order.
            let kind = frame
                .remove("kind")
                .and_then(|k| k.as_str().map(str::to_owned))
                .unwrap_or_default();
            links.incoming(Incoming::Event {
                link,
                kind,
                fields: frame,
            });
        }
        Some("ask") => match frame.remove("id") {
            Some(id) => links.incoming(Incoming::Ask {
                link,
                id,
                fields: frame,
            }),
            None => tracing::warn!(link, "browser: an ask without an id; ignored"),
        },
        Some("req") => sidebar::request(link, &frame, out, open, proxy),
        Some("ack") => open.ack(link, &frame),
        Some("abort") => open.abort(link, &frame),
        Some("site") => {
            let rules = sites.map(|now| now()).unwrap_or_default();
            let _ = out.send(sidebar::site::answer(&frame, &rules).to_string().into());
        }
        Some("hello") => tracing::warn!(link, "browser: a second hello on a link; ignored"),
        other => {
            let t = other.unwrap_or("(none)");
            tracing::debug!(link, t, "browser: a frame Kai does not know; ignored");
            // One with an id is a question the extension waits on (a newer extension's): it hears
            // that this Kai does not take it, instead of waiting for an answer that never comes.
            if let Some(id) = frame.get("id").filter(|id| !id.is_null()) {
                let _ = out.send(
                    json!({
                        "t": "error",
                        "code": "unknown_frame",
                        "id": id,
                        "message": format!("Kai {} does not take a {t} frame: update Kai", crate::VERSION),
                    })
                    .to_string()
                    .into(),
                );
            }
        }
    }
}

/// Writes the link's frames in order, each within Firefox's limit, until every sender is gone or
/// the socket fails: a longer one as chunk frames (`chunk::Chunker`), cut and written one at a
/// time, so a long frame (a file's bytes) is held once, not twice. A split `op` is kept in `sent`
/// (module doc).
async fn write_loop(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    sent: Splits,
) {
    let mut splits = 0u64;
    while let Some(Outgoing { text: f, op }) = rx.recv().await {
        let written = if f.len() <= TO_FIREFOX_MAX {
            frame::write(&mut wr, f.as_bytes()).await
        } else {
            splits += 1;
            if let Some(op) = op {
                sent.sent(splits, op);
            }
            let mut c = Chunker::new(Key::Split(splits), TO_FIREFOX_MAX);
            let mut rest = f.as_str();
            let mut written = Ok(());
            while !rest.is_empty() && written.is_ok() {
                match c.piece(&mut rest) {
                    Ok(piece) => written = frame::write(&mut wr, piece.as_bytes()).await,
                    Err(e) => {
                        // A split key is a number: not at Firefox's limit.
                        tracing::error!("browser: a frame of {} bytes was not sent: {e}", f.len());
                        break;
                    }
                }
            }
            match written {
                Ok(()) if rest.is_empty() => frame::write(&mut wr, c.end().as_bytes()).await,
                other => other,
            }
        };
        if let Err(e) = written {
            tracing::debug!("browser: a frame for the extension was not taken: {e}");
            return;
        }
    }
    let _ = wr.shutdown().await;
}
