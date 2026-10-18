//! The device link's WebSockets (the voice session's, the host link's; lmgw's device link design
//! §6.2, D15): a TLS stream Kai makes from the one configuration ([`super::Tls`]), `wss://` only
//! and refused before any connection otherwise, and tokio-tungstenite's client handshake over it.
//! tokio-tungstenite builds no TLS of its own here (none of its TLS features is on), so a
//! WebSocket to lmgw can come from nowhere else.
//!
//! A refused upgrade's body is read to its end before it is handed over: tungstenite hands over
//! only the bytes that came with the head (review KAI-4 K4-5), and lmgw's code
//! (`device_unconfirmed`, `host_not_granted`) may come after it. The stream is therefore
//! [`Shared`] between the handshake, which drops its handle with a refusal, and Kai.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::error::UrlError;
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::tungstenite::http::header::{CONTENT_LENGTH, TRANSFER_ENCODING};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error, http};

/// A WebSocket over the device link.
pub type Socket = WebSocketStream<Shared<TlsStream<TcpStream>>>;

/// A stream the handshake and Kai share (module doc). Each poll holds the lock for its length;
/// once the handshake is over only one handle is left (the socket's, or after a refusal Kai's).
pub struct Shared<S>(Arc<Mutex<S>>);

impl<S> std::fmt::Debug for Shared<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl<S> Clone for Shared<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<S> Shared<S> {
    fn new(s: S) -> Self {
        Self(Arc::new(Mutex::new(s)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, S> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Shared<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Shared<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.lock()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_shutdown(cx)
    }
}

/// Connects `request` (module doc): `wss://` only, TCP to its host and port (443 by default) with
/// `nodelay`, TLS with `config`, the client handshake with `ws`. A refused upgrade comes back as
/// tungstenite's `Error::Http` with its whole body, read for at most `body_within` after the head.
pub(super) async fn connect(
    config: Arc<rustls::ClientConfig>,
    request: Request,
    ws: Option<WebSocketConfig>,
    nodelay: bool,
    body_within: Duration,
) -> Result<(Socket, Response), Error> {
    let uri = request.uri();
    if uri.scheme_str() != Some("wss") {
        return Err(Error::Url(UrlError::UnsupportedUrlScheme));
    }
    let host = uri
        .host()
        .ok_or(Error::Url(UrlError::NoHostName))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(443);
    let name = ServerName::try_from(host.clone()).map_err(|_| {
        Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("'{host}' is no name a certificate can carry"),
        ))
    })?;
    let tcp = TcpStream::connect((host.as_str(), port)).await?;
    tcp.set_nodelay(nodelay)?;
    let tls = tokio_rustls::TlsConnector::from(config)
        .connect(name, tcp)
        .await?;
    let stream = Shared::new(tls);
    match tokio_tungstenite::client_async_with_config(request, stream.clone(), ws).await {
        Err(Error::Http(mut resp)) => {
            // The handshake dropped its handle with the refusal: the stream is Kai's alone.
            if let Ok(only) = Arc::try_unwrap(stream.0) {
                let mut s = only.into_inner().unwrap_or_else(PoisonError::into_inner);
                read_rest(&mut s, &mut resp, body_within).await;
            }
            Err(Error::Http(resp))
        }
        other => other,
    }
}

/// The rest of a refused upgrade's body (module doc): up to its `Content-Length`, a chunked one to
/// its last chunk, any other to the connection's end; what came within `within` is the body.
async fn read_rest<S: AsyncRead + Unpin>(
    s: &mut S,
    resp: &mut http::Response<Option<Vec<u8>>>,
    within: Duration,
) {
    let headers = resp.headers();
    let length: Option<usize> = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let chunked = headers.get_all(TRANSFER_ENCODING).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.to_ascii_lowercase().contains("chunked"))
    });
    let body = resp.body_mut().get_or_insert_with(Vec::new);
    let complete = |b: &[u8]| match length {
        Some(n) => b.len() >= n,
        None if chunked => b == b"0\r\n\r\n" || b.ends_with(b"\r\n0\r\n\r\n"),
        None => false,
    };
    let read = async {
        let mut buf = [0u8; 4096];
        while !complete(body) {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => body.extend_from_slice(&buf[..n]),
            }
        }
    };
    let _ = tokio::time::timeout(within, read).await;
}
