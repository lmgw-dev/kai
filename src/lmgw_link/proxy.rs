//! The configuration window's requests to lmgw (WP21b's Personality page): the kit's profile editor
//! calls lmgw's `/chat/api/profiles*`, `/v1/models` and `/v1/audio/voices`, and Kai's scheme
//! handler hands each one it admits (`crate::settings::window::profiles`) to the core as a
//! [`Forward`]. The core sends it here with the device key, so the page never holds the key and
//! opens no connection of its own.
//!
//! - **Out:** the method, the path and query as the window admitted them, a POST's JSON body, the
//!   device key as the bearer (a sensitive header, never logged), and nothing of the page's own
//!   headers.
//! - **Back:** lmgw's status and body as they are, with its content type reduced to the few the
//!   page reads (JSON, `audio/wav` from Speak, plain text); none of lmgw's other headers.
//! - **Bounds:** `window.proxy_max_mb` for the request (checked by the core before it is sent) and
//!   for lmgw's answer, `window.proxy_timeout_s` for the whole exchange; each refusal names its key.
//!   Kai's own refusals are in lmgw's error shape (`{code, message}`), so the kit shows them as it
//!   shows lmgw's.

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
        let max = self.max_bytes()?;
        let n = forward.body.len() as u64;
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

/// Sends `forward` to the paired lmgw with the device key and returns its answer for the page
/// (module doc). Never fails: what went wrong is a [`Proxied::refused`].
pub async fn send(
    client: &reqwest::Client,
    creds: &Credentials,
    forward: Forward,
    limits: Limits,
) -> Proxied {
    let url = format!("{}{}", creds.url.trim_end_matches('/'), forward.path);
    let mut bearer =
        match reqwest::header::HeaderValue::from_str(&format!("Bearer {}", creds.key.expose())) {
            Ok(v) => v,
            Err(_) => return Proxied::refused(500, "kai_key", "the device key cannot be sent"),
        };
    bearer.set_sensitive(true);
    let rb = match forward.method {
        Method::Get => client.get(&url),
        Method::Post => client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(forward.body),
    }
    .header(reqwest::header::AUTHORIZATION, bearer);
    let exchange = exchange(rb, limits);
    let answer = match limits.timeout_s {
        0 => exchange.await,
        s => match tokio::time::timeout(Duration::from_secs(s), exchange).await {
            Ok(a) => a,
            Err(_) => Proxied::refused(
                504,
                "timeout",
                &format!("lmgw did not answer within {s} s (window.proxy_timeout_s)"),
            ),
        },
    };
    tracing::debug!(
        path = %forward.path.split('?').next().unwrap_or_default(),
        status = answer.status,
        "settings window: a request forwarded to lmgw"
    );
    answer
}

async fn exchange(rb: reqwest::RequestBuilder, limits: Limits) -> Proxied {
    let mut resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            return Proxied::refused(
                502,
                "lmgw_unreachable",
                &format!("lmgw is not reachable: {}", describe(&e)),
            );
        }
    };
    let status = resp.status().as_u16();
    let mime = mime_of(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );
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
        return too_big(n);
    }
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let n = (body.len() + chunk.len()) as u64;
                if max.is_some_and(|m| n > m) {
                    return too_big(n);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                return Proxied::refused(
                    502,
                    "lmgw_broke_off",
                    &format!("lmgw's answer broke off: {}", describe(&e)),
                );
            }
        }
    }
    Proxied { status, mime, body }
}

#[cfg(test)]
mod tests;
