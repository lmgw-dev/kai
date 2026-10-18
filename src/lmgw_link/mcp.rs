//! Kai's own MCP client of lmgw's `/mcp` (api §7, WP12): the MCP Apps bridge's `resources/read`
//! and `tools/call`, as Kai's device over the device link, so lmgw's scope for the device decides
//! what Kai reaches (a tool out of scope is lmgw's `-32601`, a resource out of reach its `-32002`,
//! each with why).
//!
//! - **One session per view** ([`McpSession::open`]): `initialize` declares the MCP Apps
//!   extension (`capabilities.extensions["io.modelcontextprotocol/ui"]`), without which `/mcp`
//!   lists no app-only tool (api §7.5); then `notifications/initialized`. A session lmgw no longer
//!   knows (a 404 after its restart) is opened again once and the request sent again; requests that
//!   meet the 404 together open one session, not one each (the first opens it, the others take
//!   it). The session is ended with `DELETE /mcp` when the view goes ([`McpSession::close`]), and
//!   by its drop when nobody closed it (an open cancelled half way), so lmgw keeps no session of
//!   Kai's that Kai forgot.
//! - **Bounds:** connecting is bounded by `lmgw.timeout_s`; an answer's body by the `max` the
//!   caller gives (`overlay.app_max_mb`), named in the error. Kai puts no deadline of its own on an
//!   answer: lmgw bounds each server by its row's `timeout_ms`, and the caller's own deadline (the
//!   tool call's, the view's hide) drops the request.
//! - **Errors:** a JSON-RPC error lmgw answers (its own or a server's, passed on with its code) is
//!   [`McpError::Rpc`]; anything else (unreachable, a refusal of the device, an answer that does
//!   not read or is too large) is [`McpError::Link`] in Kai's words. No request or answer is
//!   logged.

use std::sync::Mutex;
use std::time::Duration;

use lmgw_client::mcp_apps;
use lmgw_client::requests;
use serde_json::{Value, json};

use super::{Credentials, describe};

/// The MCP revision Kai asks `/mcp` for (lmgw's newest, api §7.2).
pub const PROTOCOL: &str = "2025-11-25";

/// `/mcp`'s path from the gateway's root.
const PATH: &str = "/mcp";

/// A request's failure.
#[derive(Debug, Clone, PartialEq)]
pub enum McpError {
    /// lmgw's JSON-RPC error, or a server's passed on through it.
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    /// No JSON-RPC answer: Kai's words.
    Link(String),
}

impl McpError {
    /// The error as one line (a tool's answer, the log).
    pub fn text(&self) -> String {
        match self {
            McpError::Rpc { code, message, .. } => format!("{message} ({code})"),
            McpError::Link(s) => s.clone(),
        }
    }
}

/// The size an answer may have, and the key that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub bytes: u64,
    /// The setting as an error names it (`overlay.app_max_mb = 64`).
    pub says: String,
}

/// One session with `/mcp`.
pub struct McpSession {
    creds: Credentials,
    connect: Duration,
    bound: Bound,
    /// `Mcp-Session-Id`, replaced when the session is opened again.
    id: Mutex<String>,
    next: std::sync::atomic::AtomicU64,
    /// Held while the session is opened again: one re-open for requests that met the 404 together.
    reopening: tokio::sync::Mutex<()>,
    /// [`McpSession::close`] ran (the drop then sends nothing).
    closed: std::sync::atomic::AtomicBool,
}

impl Drop for McpSession {
    /// A session nobody closed is ended in the background (`DELETE /mcp`), if a runtime is there.
    fn drop(&mut self) {
        let session = self.session();
        if session.is_empty() || self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::debug!("mcp: a session dropped with no runtime to end it on lmgw");
            return;
        };
        let req = self
            .creds
            .http()
            .delete(self.url())
            .header("Mcp-Session-Id", session.as_str());
        let connect = self.connect;
        rt.spawn(async move {
            if !matches!(tokio::time::timeout(connect, req.send()).await, Ok(Ok(_))) {
                tracing::debug!("mcp: ending a dropped /mcp session did not reach lmgw");
            }
        });
    }
}

impl std::fmt::Debug for McpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key.
        f.debug_struct("McpSession")
            .field("url", &self.creds.url())
            .finish_non_exhaustive()
    }
}

/// What one POST answered.
enum Answer {
    /// A JSON-RPC message (the response).
    Message(Value),
    /// 202: a notification was taken.
    Accepted,
    /// 404: lmgw does not know the session (it restarted).
    UnknownSession,
}

impl McpSession {
    /// Opens a session as an MCP Apps host. `connect` is `lmgw.timeout_s`.
    pub async fn open(
        creds: Credentials,
        connect: Duration,
        bound: Bound,
    ) -> Result<McpSession, McpError> {
        let s = McpSession {
            creds,
            connect,
            bound,
            id: Mutex::new(String::new()),
            next: std::sync::atomic::AtomicU64::new(1),
            reopening: tokio::sync::Mutex::new(()),
            closed: std::sync::atomic::AtomicBool::new(false),
        };
        s.initialize().await?;
        Ok(s)
    }

    fn session(&self) -> String {
        self.id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `initialize` and `notifications/initialized`; the id is replaced only once lmgw gave a new
    /// one (requests meanwhile still name the old).
    async fn initialize(&self) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": PROTOCOL,
            "capabilities": {
                "extensions": {mcp_apps::EXTENSION: {"mimeTypes": [mcp_apps::MIME_TYPE]}}
            },
            "clientInfo": {"name": "kai", "version": env!("CARGO_PKG_VERSION")},
        });
        let (answer, session) = self
            .post(&self.envelope("initialize", params, true), None)
            .await?;
        let Answer::Message(m) = answer else {
            return Err(McpError::Link(
                "lmgw's /mcp did not answer initialize".into(),
            ));
        };
        result(m)?;
        let Some(session) = session else {
            return Err(McpError::Link(
                "lmgw's /mcp answered initialize with no Mcp-Session-Id".into(),
            ));
        };
        *self.id.lock().unwrap_or_else(|e| e.into_inner()) = session;
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.post(&note, Some(&self.session())).await?;
        Ok(())
    }

    fn envelope(&self, method: &str, params: Value, request: bool) -> Value {
        if request {
            let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        } else {
            json!({"jsonrpc": "2.0", "method": method, "params": params})
        }
    }

    /// A request and its result; a session lmgw forgot is opened again once.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        for attempt in 0..2 {
            let body = self.envelope(method, params.clone(), true);
            let used = self.session();
            match self.post(&body, Some(&used)).await?.0 {
                Answer::Message(m) => return result(m),
                Answer::UnknownSession if attempt == 0 => self.reopen(&used).await?,
                Answer::UnknownSession => break,
                Answer::Accepted => {
                    return Err(McpError::Link(format!(
                        "lmgw's /mcp answered {method} with no result"
                    )));
                }
            }
        }
        Err(McpError::Link(
            "lmgw's /mcp does not keep Kai's session".into(),
        ))
    }

    /// Opens the session again after lmgw forgot `stale`, unless another request did already.
    async fn reopen(&self, stale: &str) -> Result<(), McpError> {
        let _one = self.reopening.lock().await;
        if self.session() != stale {
            return Ok(());
        }
        tracing::info!("mcp: lmgw no longer knows Kai's /mcp session; opening another");
        self.initialize().await
    }

    /// Ends the session (`DELETE /mcp`); a failure is only logged.
    pub async fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let session = self.session();
        if session.is_empty() {
            return;
        }
        let req = self.creds.http().delete(self.url());
        let sent = tokio::time::timeout(
            self.connect,
            req.header("Mcp-Session-Id", session.as_str()).send(),
        )
        .await;
        if !matches!(sent, Ok(Ok(_))) {
            tracing::debug!("mcp: ending Kai's /mcp session did not reach lmgw");
        }
    }

    fn url(&self) -> String {
        format!("{}{PATH}", self.creds.url().trim_end_matches('/'))
    }

    /// POSTs one JSON-RPC message; the answer and the session id lmgw set.
    async fn post(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> Result<(Answer, Option<String>), McpError> {
        let mut rb = self
            .creds
            .http()
            .post(self.url())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(s) = session.filter(|s| !s.is_empty()) {
            rb = rb
                .header("Mcp-Session-Id", s)
                .header("MCP-Protocol-Version", PROTOCOL);
        }
        let secs = self.connect.as_secs();
        // Only connecting is bounded (the client's `connect_timeout`, `lmgw.timeout_s`): lmgw
        // sends the answer's head with the whole answer, after the tool ran (module doc).
        let resp = rb.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                McpError::Link(format!(
                    "lmgw is not reachable within {secs} s (lmgw.timeout_s): {}",
                    super::tls::why(&e)
                ))
            } else {
                McpError::Link(format!(
                    "lmgw's /mcp did not answer: {}",
                    super::tls::why(&e)
                ))
            }
        })?;
        let status = resp.status().as_u16();
        let set = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        let text = self.body(resp).await?;
        match status {
            202 => return Ok((Answer::Accepted, set)),
            404 if session.is_some() => return Ok((Answer::UnknownSession, set)),
            200..=299 => {}
            _ => {
                let e = requests::read_refusal(status, &text);
                return Err(McpError::Link(format!(
                    "lmgw's /mcp refused Kai ({status} {}): {}",
                    e.code, e.message
                )));
            }
        }
        let message = if sse { first_event(&text) } else { Some(text) }
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(Value::is_object)
            .ok_or_else(|| {
                McpError::Link("lmgw's /mcp answered something that is not JSON-RPC".into())
            })?;
        Ok((Answer::Message(message), set))
    }

    /// The body as text, streamed against the bound.
    async fn body(&self, mut resp: reqwest::Response) -> Result<String, McpError> {
        let too_big = |n: u64| {
            McpError::Link(format!(
                "lmgw's answer is larger than {} ({n} bytes or more)",
                self.bound.says
            ))
        };
        if let Some(n) = resp.content_length().filter(|n| *n > self.bound.bytes) {
            return Err(too_big(n));
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| McpError::Link(format!("lmgw's answer broke off: {}", describe(&e))))?
        {
            if (out.len() + chunk.len()) as u64 > self.bound.bytes {
                return Err(too_big((out.len() + chunk.len()) as u64));
            }
            out.extend_from_slice(&chunk);
        }
        String::from_utf8(out).map_err(|_| McpError::Link("lmgw's answer is not UTF-8".into()))
    }
}

/// The first `data:` event of an event stream (lmgw answers one response per POST).
fn first_event(text: &str) -> Option<String> {
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            data.push(d.strip_prefix(' ').unwrap_or(d));
        } else if line.is_empty() && !data.is_empty() {
            break;
        }
    }
    (!data.is_empty()).then(|| data.join("\n"))
}

/// A response's `result`, or its `error` as [`McpError::Rpc`].
fn result(mut m: Value) -> Result<Value, McpError> {
    if let Some(e) = m.get("error").filter(|e| e.is_object()) {
        return Err(McpError::Rpc {
            code: e.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("an error without a message")
                .to_owned(),
            data: e.get("data").cloned(),
        });
    }
    match m.get_mut("result") {
        Some(r) => Ok(r.take()),
        None => Err(McpError::Link(
            "lmgw's /mcp answered with neither a result nor an error".into(),
        )),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A loopback `/mcp` for the tests behind the tests' device listener
    //! (`crate::lmgw_link::test_pki`): one JSON-RPC message per POST, answered by a closure; every
    //! request kept (method, params, the session header, an Authorization header should one come).

    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::lmgw_link::test_pki::Gateway;

    /// One request as the fake saw it.
    #[derive(Debug, Clone)]
    pub struct Seen {
        pub http: String,
        pub method: String,
        pub params: Value,
        pub session: Option<String>,
        pub bearer: Option<String>,
        pub ui_extension: bool,
    }

    /// The answer to a request: `Ok(result)`, `Err((code, message))`, a raw HTTP status with no
    /// body (`Status`), or none ever (`Hang`: the connection stays open).
    pub enum Reply {
        Ok(Value),
        Err(i64, String),
        Status(u16),
        Hang,
    }

    pub type Answer = Arc<dyn Fn(&str, &Value) -> Reply + Send + Sync>;

    pub struct Fake {
        pub url: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
        /// The session id the next initialize hands out ("s1", "s2", …); set the counter to forget
        /// every earlier one.
        pub sessions: Arc<Mutex<u32>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Fake {
        pub fn methods(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.method.clone())
                .collect()
        }
    }

    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    pub async fn serve(answer: Answer) -> Fake {
        let listener = Gateway::bind().await;
        let url = listener.url.clone();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let sessions = Arc::new(Mutex::new(0u32));
        let (log, counter) = (seen.clone(), sessions.clone());
        let task = tokio::spawn(async move {
            loop {
                // A handshake that failed (a client that went) ends only its connection.
                let Ok((mut sock, _)) = listener.accept().await else {
                    continue;
                };
                let (log, counter, answer) = (log.clone(), counter.clone(), answer.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let (head, body) = loop {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                        let len: usize = header(&head, "content-length")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        while buf.len() < end + 4 + len {
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        break (head, buf[end + 4..end + 4 + len].to_vec());
                    };
                    let http = head.split(' ').next().unwrap_or_default().to_owned();
                    let msg: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let method = msg["method"].as_str().unwrap_or_default().to_owned();
                    let params = msg["params"].clone();
                    let session = header(&head, "mcp-session-id").map(str::to_owned);
                    let known = {
                        let n = *counter.lock().unwrap();
                        session
                            .as_deref()
                            .and_then(|s| s.strip_prefix('s'))
                            .and_then(|s| s.parse::<u32>().ok())
                            .is_some_and(|s| s == n)
                    };
                    log.lock().unwrap().push(Seen {
                        http: http.clone(),
                        method: method.clone(),
                        ui_extension:
                            params["capabilities"]["extensions"]["io.modelcontextprotocol/ui"]
                                .is_object(),
                        params: params.clone(),
                        session: session.clone(),
                        bearer: header(&head, "authorization").map(str::to_owned),
                    });
                    let respond = |status: &str, extra: &str, body: &str| {
                        format!(
                            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: \
                             application/json\r\n{extra}Content-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let out = if http == "DELETE" {
                        respond("204 No Content", "", "")
                    } else if method == "initialize" {
                        let n = {
                            let mut c = counter.lock().unwrap();
                            *c += 1;
                            *c
                        };
                        let body = json!({"jsonrpc": "2.0", "id": msg["id"], "result": {
                            "protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "fake-lmgw", "version": "0"}}});
                        respond(
                            "200 OK",
                            &format!("Mcp-Session-Id: s{n}\r\n"),
                            &body.to_string(),
                        )
                    } else if !known {
                        respond("404 Not Found", "", "")
                    } else if msg.get("id").is_none() {
                        respond("202 Accepted", "", "")
                    } else {
                        match answer(&method, &params) {
                            Reply::Ok(r) => respond(
                                "200 OK",
                                "",
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": r})
                                    .to_string(),
                            ),
                            Reply::Err(code, message) => respond(
                                "200 OK",
                                "",
                                &json!({"jsonrpc": "2.0", "id": msg["id"],
                                    "error": {"code": code, "message": message}})
                                .to_string(),
                            ),
                            Reply::Status(s) => respond(&format!("{s} X"), "", ""),
                            Reply::Hang => {
                                std::future::pending::<()>().await;
                                return;
                            }
                        }
                    };
                    let _ = sock.write_all(out.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Fake {
            url,
            seen,
            sessions,
            task,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::fake::{Reply, serve};
    use super::*;
    use crate::lmgw_link::test_pki::creds;

    fn bound(bytes: u64) -> Bound {
        Bound {
            bytes,
            says: "overlay.app_max_mb (test)".into(),
        }
    }

    async fn open(url: &str, b: u64) -> Result<McpSession, McpError> {
        McpSession::open(creds(url), Duration::from_secs(5), bound(b)).await
    }

    #[tokio::test]
    async fn a_session_declares_the_apps_extension_as_kais_device() {
        let f = serve(Arc::new(|m, p| match m {
            "tools/list" => Reply::Ok(json!({"tools": []})),
            "resources/read" if p["uri"] == "ui://fx__a/b" => {
                Reply::Err(-32002, "out of reach: x".into())
            }
            _ => Reply::Err(-32601, "no".into()),
        }))
        .await;
        let s = open(&f.url, 1 << 20).await.unwrap();
        assert_eq!(
            s.request("tools/list", json!({})).await,
            Ok(json!({"tools": []}))
        );
        assert_eq!(
            s.request("resources/read", json!({"uri": "ui://fx__a/b"}))
                .await,
            Err(McpError::Rpc {
                code: -32002,
                message: "out of reach: x".into(),
                data: None
            }),
            "lmgw's error passes on with its code"
        );
        s.close().await;
        let seen = f.seen.lock().unwrap().clone();
        let methods: Vec<&str> = seen.iter().map(|s| s.method.as_str()).collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "resources/read",
                ""
            ]
        );
        assert!(seen[0].ui_extension, "the apps host's extension (api §7.5)");
        assert_eq!(
            seen[0].params["capabilities"]["extensions"]["io.modelcontextprotocol/ui"]["mimeTypes"],
            json!(["text/html;profile=mcp-app"])
        );
        assert_eq!(seen[0].session, None);
        assert!(seen[1..].iter().all(|s| s.session.as_deref() == Some("s1")));
        assert!(
            seen.iter().all(|s| s.bearer.is_none()),
            "the device's certificate authenticates it"
        );
        assert_eq!(seen[4].http, "DELETE");
    }

    #[tokio::test]
    async fn a_forgotten_session_is_opened_again_once() {
        let f = serve(Arc::new(|_, _| Reply::Ok(json!({"ok": 1})))).await;
        let s = open(&f.url, 1 << 20).await.unwrap();
        // lmgw restarted: it knows no earlier session.
        *f.sessions.lock().unwrap() = 7;
        assert_eq!(s.request("ping", json!({})).await, Ok(json!({"ok": 1})));
        assert_eq!(
            f.methods(),
            [
                "initialize",
                "notifications/initialized",
                "ping",
                "initialize",
                "notifications/initialized",
                "ping"
            ]
        );
    }

    /// Requests that meet lmgw's 404 together open one new session, not one each (each further
    /// one would be a session lmgw keeps and Kai never ends).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_requests_after_a_restart_open_one_session() {
        let f = serve(Arc::new(|_, _| Reply::Ok(json!({"ok": 1})))).await;
        let s = Arc::new(open(&f.url, 1 << 20).await.unwrap());
        *f.sessions.lock().unwrap() = 7;
        let mut all = tokio::task::JoinSet::new();
        for _ in 0..6 {
            let s = s.clone();
            all.spawn(async move { s.request("ping", json!({})).await });
        }
        while let Some(r) = all.join_next().await {
            assert_eq!(r.unwrap(), Ok(json!({"ok": 1})));
        }
        let opened = f.methods().iter().filter(|m| *m == "initialize").count();
        assert_eq!(
            opened,
            2,
            "the first session and one more: {:?}",
            f.methods()
        );
        assert_eq!(s.session(), "s8");
    }

    /// A session dropped without [`McpSession::close`] (an open cancelled half way) is ended on
    /// lmgw all the same; a closed one is not ended twice.
    #[tokio::test]
    async fn a_dropped_session_is_ended() {
        let f = serve(Arc::new(|_, _| Reply::Ok(json!({})))).await;
        let s = open(&f.url, 1 << 20).await.unwrap();
        drop(s);
        let deletes = |f: &super::fake::Fake| {
            f.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.http == "DELETE")
                .count()
        };
        for _ in 0..200 {
            if deletes(&f) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(deletes(&f), 1, "{:?}", f.methods());
        let s = open(&f.url, 1 << 20).await.unwrap();
        s.close().await;
        drop(s);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(deletes(&f), 2, "closed, then dropped: one DELETE");
    }

    #[tokio::test]
    async fn an_answer_over_the_bound_is_refused_naming_the_setting() {
        let big = "x".repeat(4000);
        let f = serve(Arc::new(move |_, _| Reply::Ok(json!({"text": big})))).await;
        let s = open(&f.url, 2000).await.unwrap();
        let e = s.request("resources/read", json!({})).await.unwrap_err();
        let McpError::Link(text) = e else {
            panic!("{e:?}")
        };
        assert!(
            text.contains("larger than overlay.app_max_mb (test)"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn refusals_and_an_absent_lmgw_are_said() {
        let f = serve(Arc::new(|_, _| Reply::Status(500))).await;
        let s = open(&f.url, 1 << 20).await.unwrap();
        let e = s.request("tools/list", json!({})).await.unwrap_err();
        assert!(e.text().contains("refused Kai (500"), "{}", e.text());
        drop(f);
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let e = open(&format!("https://127.0.0.1:{port}"), 1 << 20)
            .await
            .unwrap_err();
        assert!(
            e.text().starts_with("lmgw is not reachable"),
            "{}",
            e.text()
        );
    }

    #[test]
    fn an_event_stream_answer_reads() {
        assert_eq!(
            first_event("event: message\ndata: {\"a\":1}\n\n").as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(first_event(": keep-alive\n\n"), None);
    }
}
