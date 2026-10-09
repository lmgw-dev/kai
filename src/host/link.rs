//! The link to lmgw's `/mcp/host` (api §5.1 to §5.4): the WebSocket under the device key, its
//! frames as rmcp's transport ([`LinkTransport`]), the reconnects and the closes.
//!
//! - **One JSON-RPC message per text message**, both ways, no batches. A message that does not
//!   read as MCP's client side is answered with an error to its id (when it has one) and said in
//!   the log by its size only.
//! - **lmgw's limits bind, no limit of Kai's own:** tungstenite's are lifted. Kai writes a message
//!   larger than the link's frame limit as a text frame and continuation frames within it
//!   ([`super::limits::frames`]), so only `mcp.host_max_message_mb` binds a message; a tool's
//!   result over that is the call's error (`server.rs`). The limits are what lmgw announces in its
//!   `initialize` (`lmgw/host_limits`), until then and with an lmgw that announces none `[host]`'s
//!   fallbacks. An overrun anyway is lmgw's close naming the setting, which the tooltip says.
//! - **Dead links are found:** a ping every `host.keepalive_s`; nothing from lmgw for two intervals,
//!   or a write it does not take in that long, ends the link (lmgw pings on its own interval,
//!   `mcp.host_ping_interval_s`, and tungstenite answers it).
//! - **Reconnects by itself** (PD2: it warms no model, so K5's drop rules do not apply): after 1 s,
//!   doubling to `lmgw.retry_max_s`, starting over only once a link was up (lmgw's `initialize`
//!   answered): a link that ends before it, at the handshake or by a close, backs off like a
//!   refused one, so a gateway that takes the upgrade and closes at once is not hit every second.
//!   A 403 `host_not_granted` tries again every `lmgw.retry_max_s`, and at once when the feed's
//!   `hello` names a label.
//! - **One run at a time** (the fence): a stopped run is cancelled at every wait, the handshake
//!   included, so a late upgrade of the old run never reaches lmgw to take over the new link (4000);
//!   a new run waits for the old one's socket to close first (at most `lmgw.timeout_s`, then it is
//!   aborted). The peer the server notifies is tagged with the run's generation
//!   ([`super::server::PeerSlot`]): a stopped run neither sets nor clears it.
//! - **Closes** (`lmgw_client::realtime::close_kind`): 4003 is a revocation and goes the feed's
//!   path (the core, §3.3); 4000 (another link of this device took over) waits for the next
//!   feed `hello`, so two Kais of one key do not take the link from each other in turns; 1001 (lmgw
//!   stopping) and every other close reconnect after the backoff. A 401 at the handshake is a
//!   revocation too.
//! - **The calls end with the link:** every call in flight is cancelled when it drops.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lmgw_client::mcp_host::{self, CloseKind, close_kind};
use lmgw_client::requests;
use rmcp::model::{ClientJsonRpcMessage, ErrorData, RequestId, ServerJsonRpcMessage};
use rmcp::service::{RoleServer, serve_server_with_ct};
use rmcp::transport::Transport;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::CancellationToken;

use super::limits::{LinkLimits, frames};
use super::server::{Server, Shared};
use super::{HostEvent, HostStatus};
use crate::lmgw_link::{Credentials, KeyLoss, describe};

/// The first reconnect waits this long; each further one twice the last, up to
/// `lmgw.retry_max_s` (the feed's backoff, §3.2).
const FIRST_RETRY: Duration = Duration::from_secs(1);

/// The reason of Kai's own close (1000).
pub const CLOSE_REASON: &str = "Kai closed the tool link";

/// What the link needs from the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSettings {
    /// `lmgw.retry_max_s`.
    pub retry_max: Duration,
    /// `lmgw.timeout_s`: the handshake's answer, and lmgw's `initialize` after it.
    pub timeout: Duration,
    /// `host.keepalive_s`.
    pub keepalive: Duration,
    /// `host.fallback_max_message_mb` and `host.fallback_max_frame_mb`: the limits until lmgw
    /// announces its own, and with an lmgw that announces none.
    pub fallback: LinkLimits,
}

/// rmcp's transport over the link: the frames the pump read, and the messages it writes.
pub struct LinkTransport {
    incoming: mpsc::UnboundedReceiver<ClientJsonRpcMessage>,
    outgoing: mpsc::UnboundedSender<ServerJsonRpcMessage>,
}

#[derive(Debug, thiserror::Error)]
#[error("the tool link is closed")]
pub struct LinkClosed;

impl Transport<RoleServer> for LinkTransport {
    type Error = LinkClosed;

    fn send(
        &mut self,
        item: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(self.outgoing.send(item).map_err(|_| LinkClosed))
    }

    fn receive(&mut self) -> impl Future<Output = Option<ClientJsonRpcMessage>> + Send {
        self.incoming.recv()
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.incoming.close();
        Ok(())
    }
}

/// How one link ended, and whether lmgw's `initialize` came first (the backoff starts over only
/// then).
struct Ended {
    end: End,
    linked: bool,
}

/// How one link ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// lmgw's close frame, as sent.
    Closed { code: u16, reason: String },
    /// No close frame: what broke it.
    Dropped(String),
    /// Kai stopped the link.
    Stopped,
}

/// The pump: the socket's frames to rmcp and rmcp's messages to the socket, the keep-alive, Kai's
/// close on `stop`.
async fn pump<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    incoming: mpsc::UnboundedSender<ClientJsonRpcMessage>,
    mut outgoing: mpsc::UnboundedReceiver<ServerJsonRpcMessage>,
    keepalive: Duration,
    limits: Arc<RwLock<LinkLimits>>,
    stop: CancellationToken,
) -> End
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let dead_after = keepalive.saturating_mul(2);
    let secs = dead_after.as_secs();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + keepalive, keepalive);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    let unwritten = || {
        End::Dropped(format!(
            "lmgw took nothing on the tool link for {secs} s (two host.keepalive_s intervals): the \
             link is dead"
        ))
    };
    macro_rules! write {
        ($msg:expr) => {
            match tokio::time::timeout(dead_after, sink.send($msg)).await {
                Err(_) => break unwritten(),
                Ok(Err(e)) => break End::Dropped(describe(&e)),
                Ok(Ok(())) => {}
            }
        };
    }
    let end = loop {
        tokio::select! {
            biased;
            // Kai's stop, or rmcp done with the link (its calls cancelled with the stop): Kai's
            // close, 1000.
            () = stop.cancelled() => break End::Stopped,
            m = outgoing.recv() => match m {
                Some(m) => match serde_json::to_string(&m) {
                    Ok(text) => {
                        let max = limits.read().expect("never poisoned").frame_bytes();
                        let mut failed = None;
                        for frame in frames(text, max) {
                            match tokio::time::timeout(dead_after, sink.send(frame)).await {
                                Err(_) => failed = Some(unwritten()),
                                Ok(Err(e)) => failed = Some(End::Dropped(describe(&e))),
                                Ok(Ok(())) => {}
                            }
                            if failed.is_some() {
                                break;
                            }
                        }
                        if let Some(end) = failed {
                            break end;
                        }
                    }
                    Err(e) => tracing::error!("host: a message Kai cannot write as JSON: {e}"),
                },
                None => break End::Stopped,
            },
            _ = ping.tick() => {
                if heard.elapsed() >= dead_after {
                    break End::Dropped(format!(
                        "lmgw sent nothing on the tool link for {secs} s (two host.keepalive_s \
                         intervals): the link is dead"
                    ));
                }
                write!(Message::Ping(Vec::new().into()));
            }
            m = stream.next() => {
                if matches!(m, Some(Ok(_))) {
                    heard = tokio::time::Instant::now();
                }
                match m {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientJsonRpcMessage>(&text) {
                        Ok(msg) => {
                            if incoming.send(msg).is_err() {
                                break End::Stopped;
                            }
                        }
                        Err(_) => {
                            tracing::warn!(bytes = text.len(), "host: lmgw sent a frame that is not an MCP client message; answered with an error");
                            if let Some(id) = request_id(&text) {
                                let answer = ServerJsonRpcMessage::error(
                                    ErrorData::invalid_request(
                                        "Kai cannot read this message as one JSON-RPC message of MCP's client side",
                                        None,
                                    ),
                                    Some(id),
                                );
                                if let Ok(text) = serde_json::to_string(&answer) {
                                    write!(Message::Text(text.into()));
                                }
                            }
                        }
                    },
                    Some(Ok(Message::Binary(b))) => {
                        tracing::warn!(bytes = b.len(), "host: lmgw sent a binary frame; skipped");
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let (code, reason) = match frame {
                            Some(f) => (u16::from(f.code), f.reason.as_str().to_owned()),
                            None => (CloseCode::Status.into(), String::new()),
                        };
                        break End::Closed { code, reason };
                    }
                    Some(Ok(_)) => {}
                    Some(Err(tungstenite::Error::ConnectionClosed)) | None => {
                        break End::Dropped("lmgw ended the connection without a close frame".into());
                    }
                    Some(Err(e)) => break End::Dropped(describe(&e)),
                }
            }
        }
    };
    if end == End::Stopped {
        let frame = CloseFrame {
            code: CloseCode::Normal,
            reason: CLOSE_REASON.into(),
        };
        let _ = tokio::time::timeout(dead_after, sink.send(Message::Close(Some(frame)))).await;
    }
    // The answer to a close lmgw began, best effort.
    let _ = tokio::time::timeout(dead_after, sink.flush()).await;
    end
}

/// The id of a request Kai could not read, so its error reaches it.
fn request_id(text: &str) -> Option<RequestId> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("method")?;
    serde_json::from_value(v.get("id")?.clone()).ok()
}

/// Why a connect failed.
enum Refused {
    /// 403 (`host_not_granted`; an `Origin` refusal cannot happen, Kai sends none).
    NotGranted(String),
    /// 401: the key opens nothing (§3.3).
    Key(String, KeyLoss),
    Other(String),
}

/// The TLS a `wss:` gateway gets (the feed's: the system's trust store through the platform
/// verifier).
fn tls() -> Result<Arc<rustls::ClientConfig>, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| b.with_platform_verifier())
        .map_err(|e| format!("cannot set up TLS for the tool link: {e}"))?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// One link run (started by the core, aborted or stopped by it).
pub struct Run {
    pub creds: Credentials,
    pub settings: LinkSettings,
    pub shared: Arc<Shared>,
    pub events: mpsc::UnboundedSender<HostEvent>,
    /// The feed's `hello`s, counted: a link that waits for one tries again at the next.
    pub kick: watch::Receiver<u64>,
    pub stop: CancellationToken,
    /// This run's generation ([`super::server::PeerSlot`]).
    pub generation: u64,
    /// The run this one replaces: its socket closes before this one links.
    pub prior: Option<JoinHandle<()>>,
}

impl Run {
    fn status(&self, s: HostStatus) -> bool {
        self.events.send(HostEvent::Status(s)).is_ok()
    }

    pub async fn run(mut self) {
        if let Some(prior) = self.prior.take() {
            // The old run closes its socket first (its stop is cancelled): lmgw never sees its
            // upgrade after this one's.
            let abort = prior.abort_handle();
            tokio::select! {
                biased;
                () = self.stop.cancelled() => {
                    abort.abort();
                    return;
                }
                r = tokio::time::timeout(self.settings.timeout, prior) => {
                    if r.is_err() {
                        tracing::warn!(
                            "host: the previous tool link did not close within lmgw.timeout_s; \
                             it is dropped"
                        );
                        abort.abort();
                    }
                }
            }
        }
        let mut kick = self.kick.clone();
        let mut delay = FIRST_RETRY;
        let retry_max = self.settings.retry_max.max(FIRST_RETRY);
        loop {
            if self.stop.is_cancelled() || !self.status(HostStatus::Connecting) {
                return;
            }
            let wait = match self.connect().await {
                Err(Refused::Key(reason, loss)) => {
                    tracing::warn!(%reason, ?loss, "host: the device key opens nothing (401)");
                    let _ = self.events.send(HostEvent::Revoked {
                        reason,
                        loss: Some(loss),
                    });
                    return;
                }
                Err(Refused::NotGranted(message)) => {
                    tracing::info!(%message, "host: this device has no hosting grant (403)");
                    self.status(HostStatus::NotGranted {
                        retry_s: retry_max.as_secs(),
                        message,
                    });
                    Wait::KickOr(retry_max)
                }
                Err(Refused::Other(why)) => {
                    let next = Wait::Backoff(delay);
                    self.status(HostStatus::Offline {
                        note: format!("{why}; next try in {} s", delay.as_secs()),
                    });
                    delay = (delay * 2).min(retry_max);
                    next
                }
                Ok(Ended {
                    end: End::Stopped, ..
                }) => return,
                Ok(Ended { end, linked }) => {
                    // Only a link lmgw initialized starts the backoff over; one that ended before
                    // backs off like a refusal.
                    if linked {
                        delay = FIRST_RETRY;
                    } else {
                        tracing::info!(?end, "host: the link ended before lmgw initialized it");
                    }
                    let next = self.after(end, delay);
                    delay = (delay * 2).min(retry_max);
                    match next {
                        Some(w) => w,
                        None => return,
                    }
                }
            };
            // Only a `hello` from now on counts.
            kick.borrow_and_update();
            let kicked = async {
                if kick.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            match wait {
                Wait::Backoff(d) => tokio::select! {
                    () = tokio::time::sleep(d) => {}
                    () = self.stop.cancelled() => return,
                },
                Wait::KickOr(d) => tokio::select! {
                    () = tokio::time::sleep(d) => {}
                    () = kicked => tracing::info!("host: the feed said hello; linking now"),
                    () = self.stop.cancelled() => return,
                },
                Wait::Kick => tokio::select! {
                    () = kicked => tracing::info!("host: the feed said hello; linking now"),
                    () = self.stop.cancelled() => return,
                },
            }
        }
    }

    /// What an ended link means, `delay` being the backoff's next wait; `None`: the run ends (a
    /// revocation of a kind Kai knows).
    fn after(&self, end: End, delay: Duration) -> Option<Wait> {
        let (code, reason) = match end {
            End::Closed { code, reason } => (code, reason),
            End::Dropped(why) => {
                tracing::warn!(%why, "host: the tool link dropped");
                self.status(HostStatus::Offline {
                    note: format!("{why}; next try in {} s", delay.as_secs()),
                });
                return Some(Wait::Backoff(delay));
            }
            End::Stopped => return None,
        };
        tracing::info!(code, %reason, "host: lmgw closed the tool link");
        match close_kind(code, &reason) {
            CloseKind::Revoked { kind } => {
                let loss = KeyLoss::of_kind(&kind);
                let _ = self.events.send(HostEvent::Revoked {
                    reason: reason.clone(),
                    loss,
                });
                if loss.is_some() {
                    return None;
                }
                // A kind this build does not know: the core says it; the link tries again.
                self.status(HostStatus::Offline {
                    note: format!("lmgw ended the tool link: {reason}"),
                });
                Some(Wait::Backoff(self.settings.retry_max.max(FIRST_RETRY)))
            }
            CloseKind::TakenOver => {
                self.status(HostStatus::TakenOver { reason });
                Some(Wait::Kick)
            }
            CloseKind::ShuttingDown => {
                self.status(HostStatus::Offline {
                    note: format!(
                        "lmgw is stopping or restarting; next try in {} s",
                        delay.as_secs()
                    ),
                });
                Some(Wait::Backoff(delay))
            }
            _ => {
                let said = if reason.is_empty() {
                    format!("lmgw closed the tool link ({code})")
                } else {
                    format!("lmgw closed the tool link ({code}): {reason}")
                };
                self.status(HostStatus::Offline {
                    note: format!("{said}; next try in {} s", delay.as_secs()),
                });
                Some(Wait::Backoff(delay))
            }
        }
    }

    /// One link: the handshake, rmcp's `initialize`, then the pump until it ends.
    async fn connect(&self) -> Result<Ended, Refused> {
        let stopped = Ended {
            end: End::Stopped,
            linked: false,
        };
        let req = requests::mcp_host().bearer(self.creds.key.expose());
        let url = req
            .ws_url(&self.creds.url)
            .map_err(|e| Refused::Other(format!("{} ({})", e.message, e.code)))?;
        let connector = if url.starts_with("wss://") {
            Connector::Rustls(tls().map_err(Refused::Other)?)
        } else {
            Connector::Plain
        };
        let mut request = url.as_str().into_client_request().map_err(|e| {
            Refused::Other(format!(
                "the tool link's address '{url}' does not work: {e}"
            ))
        })?;
        for h in &req.headers {
            let (name, value) = crate::lmgw_link::header(h).map_err(Refused::Other)?;
            request.headers_mut().insert(name, value);
        }
        tracing::info!(%url, "host: linking to lmgw");
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let secs = self.settings.timeout.as_secs();
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            request,
            Some(config),
            true,
            Some(connector),
        );
        // A stop drops the handshake where it is: an old run's upgrade never completes after a
        // new run's (lmgw would close the new link with 4000).
        let handshake = tokio::select! {
            biased;
            () = self.stop.cancelled() => return Ok(stopped),
            r = tokio::time::timeout(self.settings.timeout, connect) => r,
        };
        let ws = match handshake {
            Err(_) => {
                return Err(Refused::Other(format!(
                    "lmgw did not answer the tool link within {secs} s (lmgw.timeout_s)"
                )));
            }
            Ok(Err(tungstenite::Error::Http(resp))) => {
                let status = resp.status().as_u16();
                let body = crate::lmgw_link::refusal_body(&resp);
                let e = requests::read_refusal(status, &body);
                // The status decides: the body may come after the head tungstenite read.
                return Err(match status {
                    401 => Refused::Key(e.message, KeyLoss::of_code(&e.code)),
                    // Kai sends no Origin; should a proxy add one, lmgw says so.
                    403 if e.code == mcp_host::CROSS_ORIGIN_REFUSED => Refused::Other(format!(
                        "lmgw refused the tool link for an Origin header Kai did not send: {}",
                        e.message
                    )),
                    // `host_not_granted`, or a 403 whose body came after the head.
                    403 => {
                        if e.code != mcp_host::HOST_NOT_GRANTED {
                            tracing::info!(code = %e.code, "host: a 403 without lmgw's code; read as no grant");
                        }
                        Refused::NotGranted(e.message)
                    }
                    _ => Refused::Other(format!(
                        "lmgw refused the tool link: {} ({status})",
                        e.message
                    )),
                });
            }
            Ok(Err(e)) => {
                return Err(Refused::Other(format!(
                    "lmgw is not reachable: {}",
                    describe(&e)
                )));
            }
            Ok(Ok((ws, _))) => ws,
        };
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let transport = LinkTransport {
            incoming: in_rx,
            outgoing: out_tx,
        };
        // The calls of this link: cancelled when it ends, whatever ends it.
        let calls = self.stop.child_token();
        let _cancel_calls = calls.clone().drop_guard();
        // This link's limits: the fallbacks until lmgw's `initialize` announces its own.
        let limits = Arc::new(RwLock::new(self.settings.fallback));
        let pump = pump(
            ws,
            in_tx,
            out_rx,
            self.settings.keepalive,
            limits.clone(),
            self.stop.clone(),
        );
        tokio::pin!(pump);
        let server = Server {
            shared: self.shared.clone(),
            limits,
        };
        let serve = serve_server_with_ct(server, transport, calls.clone());
        // The pump carries the stop: it ends `Stopped` (with Kai's 1000) the moment the run is
        // stopped, and is polled first.
        let running = tokio::select! {
            biased;
            end = &mut pump => return Ok(Ended { end, linked: false }),
            r = tokio::time::timeout(self.settings.timeout, serve) => match r {
                Err(_) => {
                    // The socket goes with this function: Kai closes it and tries again.
                    return Err(Refused::Other(format!(
                        "lmgw sent no initialize on the tool link within {secs} s (lmgw.timeout_s)"
                    )));
                }
                Ok(Err(e)) => {
                    // The error may carry the message lmgw sent: it is not logged.
                    let _ = e;
                    return Err(Refused::Other(
                        "lmgw's initialize on the tool link failed".into(),
                    ));
                }
                Ok(Ok(running)) => running,
            },
        };
        // Fenced: a run stopped meanwhile sets no peer (its pump ends at once with the stop).
        let ours = self
            .shared
            .peer
            .lock()
            .expect("never poisoned")
            .set(self.generation, running.peer().clone());
        if ours {
            let tools = self.shared.tool_count();
            tracing::info!(tools, "host: linked; lmgw initialized the tool link");
            self.status(HostStatus::Linked { tools });
        }
        let end = pump.await;
        // Only this run's own peer: a newer run's stays.
        self.shared
            .peer
            .lock()
            .expect("never poisoned")
            .clear(self.generation);
        calls.cancel();
        drop(running);
        Ok(Ended { end, linked: ours })
    }
}

/// What the run waits for before it links again.
enum Wait {
    Backoff(Duration),
    /// The feed's next `hello`, or this long.
    KickOr(Duration),
    /// The feed's next `hello` only (a takeover).
    Kick,
}
