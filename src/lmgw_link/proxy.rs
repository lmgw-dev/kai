//! The configuration window's requests to lmgw (WP21b's Personality page): the kit's profile editor
//! calls lmgw's `/chat/api/profiles*`, `/v1/models` and `/v1/audio/voices`, and Kai's scheme
//! handler hands each one it admits (`crate::settings::window::profiles`) to the core as a
//! [`Forward`]. The core sends it here over the device link (lmgw's device link design §6.2), so
//! the page holds no credential and opens no connection of its own.
//!
//! - **Out:** the method, the path and query as the window admitted them, a POST's JSON body, and
//!   nothing of the page's own headers; the device's certificate authenticates it.
//! - **Back:** lmgw's status and body as they are, with its content type reduced to the few the
//!   page reads (JSON, `audio/wav` from Speak, plain text); none of lmgw's other headers.
//! - **Bounds:** `window.proxy_max_mb` for the request (checked by the core before it is sent) and
//!   for lmgw's answer, `window.proxy_timeout_s` for the whole exchange; each refusal names its key.
//!   Kai's own refusals are in lmgw's error shape (`{code, message}`), so the kit shows them as it
//!   shows lmgw's.
//!
//! Firefox's sidebar (kai-firefox design §8.2) goes through the same device link with the same
//! bounds and refusals, plus streams and byte bodies ([`relay`]).

use std::time::Duration;

use super::{Credentials, describe};
use crate::config::WindowConfig;

/// The methods the page's requests use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One request of the page, admitted by the window, for lmgw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub method: Method,
    /// The path and query as lmgw serves them (`/chat/api/profiles/3`, `/v1/audio/voices?model=x`).
    pub path: String,
    /// A POST's JSON body as the page sent it; empty for a GET.
    pub body: Vec<u8>,
}

/// What goes back to the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxied {
    pub status: u16,
    pub mime: &'static str,
    pub body: Vec<u8>,
}

impl Proxied {
    /// Kai's own refusal, in lmgw's error shape.
    pub fn refused(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            mime: "application/json",
            body: serde_json::json!({ "code": code, "message": message })
                .to_string()
                .into_bytes(),
        }
    }
}

/// `window.proxy_max_mb` and `window.proxy_timeout_s` (0: no bound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_mb: u64,
    pub timeout_s: u64,
}

impl Limits {
    pub fn from_config(c: &WindowConfig) -> Self {
        Self {
            max_mb: c.proxy_max_mb,
            timeout_s: c.proxy_timeout_s,
        }
    }

    fn max_bytes(&self) -> Option<u64> {
        (self.max_mb > 0).then(|| self.max_mb.saturating_mul(1024 * 1024))
    }

    /// The core's check of the page's request: `None` when it may go.
    pub fn refuse_request(&self, forward: &Forward) -> Option<Proxied> {
        self.refuse_body(forward.body.len())
    }

    /// The check of a request body of `n` bytes: `None` when it may go.
    pub fn refuse_body(&self, n: usize) -> Option<Proxied> {
        let max = self.max_bytes()?;
        let n = n as u64;
        (n > max).then(|| {
            Proxied::refused(
                413,
                "body_limit",
                &format!(
                    "the request is {n} bytes, more than window.proxy_max_mb ({} MB) lets Kai \
                     forward to lmgw",
                    self.max_mb
                ),
            )
        })
    }

    /// `work` within `window.proxy_timeout_s` (0: no bound); past it, the 504 naming the key.
    async fn bounded<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, Proxied>>,
    ) -> Result<T, Proxied> {
        match self.timeout_s {
            0 => work.await,
            s => tokio::time::timeout(Duration::from_secs(s), work)
                .await
                .unwrap_or_else(|_| {
                    Err(Proxied::refused(
                        504,
                        "timeout",
                        &format!("lmgw did not answer within {s} s (window.proxy_timeout_s)"),
                    ))
                }),
        }
    }
}

/// The page's type for lmgw's `Content-Type`: JSON, a WAV, plain text, or opaque bytes.
pub fn mime_of(content_type: Option<&str>) -> &'static str {
    let essence = content_type
        .and_then(|c| c.split(';').next())
        .map(|c| c.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match essence.as_str() {
        "application/json" => "application/json",
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => "audio/wav",
        "text/plain" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Sends `forward` to the paired lmgw and returns its answer for the page (module doc). Never fails:
/// what went wrong is a [`Proxied::refused`].
pub async fn send(creds: &Credentials, forward: Forward, limits: Limits) -> Proxied {
    let client = creds.http();
    let url = format!("{}{}", creds.url().trim_end_matches('/'), forward.path);
    let rb = match forward.method {
        Method::Get => client.get(&url),
        Method::Post => client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(forward.body),
    };
    let answer = limits
        .bounded(exchange(rb, limits))
        .await
        .unwrap_or_else(|refused| refused);
    tracing::debug!(
        path = %forward.path.split('?').next().unwrap_or_default(),
        status = answer.status,
        "settings window: a request forwarded to lmgw"
    );
    answer
}

async fn exchange(rb: reqwest::RequestBuilder, limits: Limits) -> Result<Proxied, Proxied> {
    let resp = rb.send().await.map_err(|e| unreachable(&e))?;
    let status = resp.status().as_u16();
    let mime = mime_of(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );
    let body = read_bounded(resp, limits).await?;
    Ok(Proxied { status, mime, body })
}

/// lmgw could not be reached (a refused or failed connection, a TLS refusal), in its words.
fn unreachable(e: &reqwest::Error) -> Proxied {
    Proxied::refused(502, "lmgw_unreachable", &super::tls::unreachable(e))
}

/// lmgw's answer stopped before its end.
fn broke_off(e: &reqwest::Error) -> String {
    format!("lmgw's answer broke off: {}", describe(e))
}

/// `resp`'s whole body, refused (502 `answer_too_large`, naming `window.proxy_max_mb`) as soon as
/// it is known to be over the bound: by its `content-length`, else as it arrives.
async fn read_bounded(mut resp: reqwest::Response, limits: Limits) -> Result<Vec<u8>, Proxied> {
    let max = limits.max_bytes();
    let too_big = |n: u64| {
        Proxied::refused(
            502,
            "answer_too_large",
            &format!(
                "lmgw's answer is {n} bytes or more, more than window.proxy_max_mb ({} MB) lets \
                 Kai take",
                limits.max_mb
            ),
        )
    };
    if let (Some(max), Some(n)) = (max, resp.content_length())
        && n > max
    {
        return Err(too_big(n));
    }
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let n = (body.len() + chunk.len()) as u64;
                if max.is_some_and(|m| n > m) {
                    return Err(too_big(n));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(e) => return Err(Proxied::refused(502, "lmgw_broke_off", &broke_off(&e))),
        }
    }
}

pub mod relay;

#[cfg(test)]
mod tests;
