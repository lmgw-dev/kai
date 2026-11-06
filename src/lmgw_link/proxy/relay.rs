//! Firefox's sidebar's requests to lmgw (kai-firefox design §8.2, "Kai's proxy"): what
//! `crate::browser::sidebar` admitted, sent over the device link, so lmgw sees the sidebar as Kai
//! (decision 9) and the extension holds no credential.
//!
//! Beyond the settings window's buffered JSON ([`super::send`]):
//! - **Out:** GET or POST, the path and query as admitted, the headers the sidebar may send (its
//!   allow-list is the caller's) and a body of any bytes.
//! - **Back:** lmgw's status and every header (the caller keeps what the sidebar may read), and
//!   the body **whole** ([`Body::Whole`]: read here under `window.proxy_max_mb`, refused 502
//!   `answer_too_large` past it) or **streamed** ([`Body::Stream`]: lmgw's chunks as they come,
//!   no bound on their sum, since nothing holds them; the caller's window paces them).
//! - **Time:** `window.proxy_timeout_s` bounds a whole answer's exchange, as for the settings
//!   window, and only the head of a streamed one (the feed stays open for hours).
//!
//! Dropping the [`send`] future or a [`Body::Stream`] ends the request: reqwest closes its stream,
//! so lmgw sees the client gone (a send stream's turn stops, as the dashboard's Stop does).

use std::fmt;

use futures_util::stream::{self, BoxStream, StreamExt};

use super::{Limits, Method, Proxied, broke_off, read_bounded, unreachable};
use crate::lmgw_link::Credentials;

/// One sidebar request for lmgw, as admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    pub method: Method,
    /// The path and query as lmgw serves them.
    pub path: String,
    /// Names in lower case, values as the sidebar sent them.
    pub headers: Vec<(String, String)>,
    /// Empty: none.
    pub body: Vec<u8>,
    /// Relay the answer as it comes, not whole.
    pub stream: bool,
}

/// An answer's body.
pub enum Body {
    Whole(Vec<u8>),
    /// lmgw's chunks as they arrive; an `Err` (its words) ends it: the answer broke off.
    Stream(BoxStream<'static, Result<Vec<u8>, String>>),
}

/// lmgw's answer: its status, its headers (names in lower case, a repeated one once per value)
/// and its body.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let body = match &self.body {
            Body::Whole(b) => format!("Whole({} bytes)", b.len()),
            Body::Stream(_) => "Stream".to_owned(),
        };
        f.debug_struct("Answer")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &body)
            .finish()
    }
}

/// Sends `out` to the paired lmgw (module doc): its answer, or Kai's refusal in lmgw's error shape
/// (`lmgw_unreachable`, `timeout`, `answer_too_large`, `lmgw_broke_off`).
pub async fn send(creds: &Credentials, out: Outbound, limits: Limits) -> Result<Answer, Proxied> {
    let client = creds.http();
    let url = format!("{}{}", creds.url().trim_end_matches('/'), out.path);
    let mut rb = match out.method {
        Method::Get => client.get(&url),
        Method::Post => client.post(&url),
    };
    for (name, value) in &out.headers {
        rb = rb.header(name, value);
    }
    if out.method == Method::Post {
        rb = rb.body(out.body);
    }
    let path = out.path.split('?').next().unwrap_or_default().to_owned();
    let answer = if out.stream {
        // The bound is the head's: the body runs as long as lmgw sends.
        limits
            .bounded(async { rb.send().await.map_err(|e| unreachable(&e)) })
            .await
            .map(|resp| Answer {
                status: resp.status().as_u16(),
                headers: headers(&resp),
                body: Body::Stream(chunks(resp)),
            })
    } else {
        limits
            .bounded(async {
                let resp = rb.send().await.map_err(|e| unreachable(&e))?;
                let (status, headers) = (resp.status().as_u16(), headers(&resp));
                let body = read_bounded(resp, limits).await?;
                Ok(Answer {
                    status,
                    headers,
                    body: Body::Whole(body),
                })
            })
            .await
    };
    tracing::debug!(
        %path,
        stream = out.stream,
        status = answer.as_ref().map_or_else(|r| r.status, |a| a.status),
        "sidebar: a request forwarded to lmgw"
    );
    answer
}

fn headers(resp: &reqwest::Response) -> Vec<(String, String)> {
    resp.headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect()
}

/// `resp`'s body as it arrives (module doc).
fn chunks(resp: reqwest::Response) -> BoxStream<'static, Result<Vec<u8>, String>> {
    stream::unfold(Some(resp), |resp| async move {
        let mut resp = resp?;
        match resp.chunk().await {
            Ok(Some(c)) => Some((Ok(c.to_vec()), Some(resp))),
            Ok(None) => None,
            Err(e) => Some((Err(broke_off(&e)), None)),
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests;
