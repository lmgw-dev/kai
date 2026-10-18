//! The device link's TLS on Kai's side (lmgw's device link design §6.2, D15). Every connection Kai
//! makes to lmgw (each request, the feed, the voice session's WebSocket, the host link) is built
//! from one `rustls::ClientConfig`: the one `lmgw_client::tls::client_config` makes of the stored
//! credentials and the device's key, which trusts lmgw's device CA alone and presents the device's
//! certificate. [`Tls`] is that constructor: reqwest gets the configuration through
//! `tls_backend_preconfigured` (https only, no proxy, no redirects), and the WebSockets a TLS
//! stream Kai makes with it ([`Tls::websocket`], `wss://` only, [`ws`]). The system's trust store
//! has no say, and nothing goes over plain http. Enrolment builds its own [`Tls`] the same way,
//! from `lmgw_client::tls::enrol_config`, pinned to the link's `fp` and with no certificate.
//!
//! D15 holds at compile time too: `clippy.toml` refuses `reqwest::Client::new` and `builder`,
//! `reqwest::ClientBuilder::new`, `reqwest::get` and tokio-tungstenite's `connect_async*`
//! everywhere but here, the widgets' fetch and the updater (each with its `#[allow]` and why), and
//! tokio-tungstenite has none of its TLS features, so it cannot make a TLS connection by itself.
//!
//! [`Reach`] is where lmgw is: the URL list the pairing gave (https only, in the owner's order),
//! which each `hello.urls` replaces, and the URL that last connected. The feed and the enrolment
//! try the list with [`race`] (device link §6.3's rule: the next URL a stagger after the previous
//! one if no connection to that one is up yet, the first answer wins) and every other connection
//! goes to the URL the feed last reached.
//!
//! [`refusal`] reads a refused handshake as `lmgw_client::tls::TlsRefusal`, whose words Kai shows
//! for a refusal of lmgw's side. An alert about the device's certificate is said in Kai's own words
//! and never acted on (the security review's B1): rustls takes a fatal alert in any state of the
//! handshake, a plaintext one sent right after the ClientHello included, before the server proved
//! anything, so whoever answers at the address can send one. Kai decides a lapse by its own date
//! ([`Tls::lapsed`]); lmgw's real refusals of a paired device come over the authenticated link (a
//! 401, the feed's `revoked`).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lmgw_client::pairing::{Fingerprint, Urls};
use lmgw_client::tls::TlsRefusal;
use rustls::crypto::CryptoProvider;
use tokio_tungstenite::tungstenite;

use super::describe;
use crate::pairing::TlsKey;

/// The crypto provider of the device link: aws-lc-rs, which rustls already builds (D15 leaves it to
/// the app).
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// The one client configuration and what is built from it (module doc). Cheap to clone.
#[derive(Clone)]
pub struct Tls {
    config: Arc<rustls::ClientConfig>,
    http: reqwest::Client,
    /// `lmgw.timeout_s`, for each connection's TCP and TLS.
    connect_timeout: Duration,
    /// The SHA-256 of the device's certificate (all zero for enrolment's, which has none): which
    /// pairing this is, for what is kept per pairing.
    identity: [u8; 32],
    /// The device's certificate's `not_after` (unix milliseconds; `None` for enrolment's), shared
    /// by the clones: the feed's `hello.cert` refreshes it ([`Tls::set_not_after_ms`]).
    not_after_ms: Option<Arc<AtomicI64>>,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls").finish_non_exhaustive()
    }
}

impl Tls {
    /// The constructor every lmgw-facing connection comes from: reqwest with `config` as its TLS
    /// ([`builder`]), connecting within `connect_timeout` (`lmgw.timeout_s`); WebSockets through
    /// [`Tls::websocket`] with the same configuration.
    pub fn new(config: rustls::ClientConfig, connect_timeout: Duration) -> Result<Self, String> {
        Self::with_identity(config, connect_timeout, [0; 32], None)
    }

    fn with_identity(
        config: rustls::ClientConfig,
        connect_timeout: Duration,
        identity: [u8; 32],
        not_after_ms: Option<i64>,
    ) -> Result<Self, String> {
        let http = builder(&config, connect_timeout)
            .build()
            .map_err(|e| format!("cannot build the HTTP client for lmgw: {}", describe(&e)))?;
        Ok(Self {
            config: Arc::new(config),
            http,
            connect_timeout,
            identity,
            not_after_ms: not_after_ms.map(|t| Arc::new(AtomicI64::new(t))),
        })
    }

    /// A paired device's: lmgw's device CA as the only root, the device's certificate with `key`
    /// (`lmgw_client::tls::client_config`; TLS 1.3 only). Refused when the stored CA does not read,
    /// or when `key` is not the certificate's.
    pub fn paired(
        stored: &lmgw_client::requests::Credentials,
        key: &TlsKey,
        connect_timeout: Duration,
    ) -> Result<Self, String> {
        let provider = provider();
        let signing = key.signing_key(&provider)?;
        let config = lmgw_client::tls::client_config(provider, stored, signing)
            .map_err(|e| format!("the stored device link does not hold together: {e}"))?;
        use sha2::Digest;
        let identity = sha2::Sha256::digest(&stored.certificate).into();
        Self::with_identity(config, connect_timeout, identity, Some(stored.not_after_ms))
    }

    /// Enrolment's: pinned to the link's `fp`, no client certificate
    /// (`lmgw_client::tls::enrol_config`).
    pub fn enrolment(fp: Fingerprint, connect_timeout: Duration) -> Result<Self, String> {
        let config = lmgw_client::tls::enrol_config(provider(), fp)
            .map_err(|e| format!("cannot set up TLS for the enrolment: {e}"))?;
        Self::new(config, connect_timeout)
    }

    /// The HTTP client (a handle: clones share its connections).
    pub fn http(&self) -> reqwest::Client {
        self.http.clone()
    }

    /// An HTTP client of this configuration for one attempt of [`race`], with connections of its
    /// own: `answered` says when its connection is up ([`race`]'s module doc).
    pub fn racing(&self, answered: Answered) -> Result<reqwest::Client, reqwest::Error> {
        builder(&self.config, self.connect_timeout)
            .connector_layer(race::OnConnect(answered))
            .build()
    }

    /// A WebSocket to `request`'s `wss://` URL over this configuration ([`ws`]): refused before
    /// any connection for another scheme; `nodelay` for the TCP socket; a refused upgrade's body
    /// read for at most `body_within` after its head.
    pub async fn websocket(
        &self,
        request: tungstenite::handshake::client::Request,
        config: Option<tungstenite::protocol::WebSocketConfig>,
        nodelay: bool,
        body_within: Duration,
    ) -> Result<(ws::Socket, tungstenite::handshake::client::Response), tungstenite::Error> {
        ws::connect(self.config.clone(), request, config, nodelay, body_within).await
    }

    /// Which pairing this is (module doc of `crate::widgets::app_bridge::lists`).
    pub fn identity(&self) -> &[u8; 32] {
        &self.identity
    }

    /// The device's certificate's `not_after` as Kai keeps it (`None` for enrolment's): stored
    /// with the pairing, then as lmgw's last `hello.cert` said it.
    pub fn not_after_ms(&self) -> Option<i64> {
        self.not_after_ms
            .as_ref()
            .map(|t| t.load(Ordering::Relaxed))
    }

    /// lmgw's record of the certificate's `not_after` (the feed's `hello.cert`, lmgw's device link
    /// §7.2), for every clone of this configuration. Nothing for enrolment's.
    pub fn set_not_after_ms(&self, not_after_ms: i64) {
        if let Some(t) = &self.not_after_ms {
            t.store(not_after_ms, Ordering::Relaxed);
        }
    }

    /// The device's certificate lapsed at `now_ms` by Kai's own date (lmgw's device link §5.3):
    /// the one way Kai decides a lapse (module doc).
    pub fn lapsed(&self, now_ms: i64) -> bool {
        self.not_after_ms().is_some_and(|t| now_ms >= t)
    }
}

/// D15's one place for lmgw's HTTP clients (the module doc, `clippy.toml`): `config` as its TLS,
/// https only, no proxy (Kai talks to its gateway directly, and a proxy would hold back the feed),
/// no redirects (an answer that points elsewhere is lmgw's answer, never followed), TCP and TLS
/// within `connect_timeout`.
#[allow(clippy::disallowed_methods)]
fn builder(config: &rustls::ClientConfig, connect_timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .tls_backend_preconfigured(config.clone())
}

/// Where lmgw is (module doc): the pairing's URLs and the one that last connected. Clones share it,
/// so the feed's reach is every connection's.
#[derive(Clone)]
pub struct Reach(Arc<Mutex<Reached>>);

struct Reached {
    urls: Urls,
    last_good: Option<String>,
}

impl std::fmt::Debug for Reach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = self.lock();
        f.debug_struct("Reach")
            .field("urls", &r.urls.as_slice())
            .field("last_good", &r.last_good)
            .finish()
    }
}

impl Reach {
    pub fn new(urls: Urls) -> Self {
        Self(Arc::new(Mutex::new(Reached {
            urls,
            last_good: None,
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Reached> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The URL a connection goes to: the one that last connected, else the list's first.
    pub fn base(&self) -> String {
        let r = self.lock();
        r.urls.order(r.last_good.as_deref()).swap_remove(0)
    }

    /// The order to try them in (`Urls::order`).
    pub fn order(&self) -> Vec<String> {
        let r = self.lock();
        r.urls.order(r.last_good.as_deref())
    }

    /// The pairing's URLs, in the owner's order.
    pub fn urls(&self) -> Vec<String> {
        self.lock().urls.as_slice().to_vec()
    }

    /// lmgw's list now (the feed's `hello.urls`, lmgw's device link §7.2) replaces the pairing's,
    /// for every connection. The URL that last connected still goes first while the list holds it.
    pub fn set_urls(&self, urls: Urls) {
        self.lock().urls = urls;
    }

    /// `url` connected.
    pub fn reached(&self, url: &str) {
        let mut r = self.lock();
        if r.last_good.as_deref() != Some(url) {
            tracing::info!(%url, "lmgw link: reached lmgw at this address");
            r.last_good = Some(url.to_owned());
        }
    }
}

/// A refused handshake inside `e` (an HTTP or WebSocket client's error) as lmgw-client reads it
/// on a paired connection; `None` for any other error.
pub fn refusal(e: &(dyn std::error::Error + 'static)) -> Option<TlsRefusal> {
    lmgw_client::tls::find_tls_error(e).and_then(TlsRefusal::of)
}

/// [`refusal`] for an enrolment's connection: a `certificate_required` alert is
/// [`TlsRefusal::Unproven`] there, a failed handshake to try again that says nothing about the link
/// (an alert comes before the server proved anything).
pub fn enrol_refusal(e: &(dyn std::error::Error + 'static)) -> Option<TlsRefusal> {
    lmgw_client::tls::find_tls_error(e).and_then(TlsRefusal::of_enrolment)
}

/// The alert the server sent in `e`'s handshake, when it refused the device's certificate
/// ([`TlsRefusal::CertificateLapsed`] or [`TlsRefusal::CertificateRefused`]): TLS's name for it
/// (`certificate_expired`, `bad_certificate`, …). Never a reason to act (module doc).
pub fn certificate_alert(e: &(dyn std::error::Error + 'static)) -> Option<String> {
    match lmgw_client::tls::find_tls_error(e)? {
        rustls::Error::AlertReceived(d)
            if matches!(
                TlsRefusal::of(&rustls::Error::AlertReceived(*d)),
                Some(TlsRefusal::CertificateLapsed | TlsRefusal::CertificateRefused)
            ) =>
        {
            Some(alert_name(*d))
        }
        _ => None,
    }
}

/// TLS's name of an alert (RFC 8446 §6): rustls's variant in snake case (`BadCertificate` is
/// `bad_certificate`, `UnknownCA` `unknown_ca`).
fn alert_name(d: rustls::AlertDescription) -> String {
    let debug = format!("{d:?}");
    let mut out = String::with_capacity(debug.len() + 4);
    let mut prev_lower = false;
    for c in debug.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// What an alert about the device's certificate says (module doc): never "pair again".
fn alert_said(alert: &str) -> String {
    format!(
        "the server refused this device's certificate with TLS's {alert} alert, before it proved \
         to be your lmgw: Kai keeps the pairing and tries again"
    )
}

/// Why a connection to lmgw failed: an alert about the device's certificate in Kai's words, a
/// refusal of lmgw's side in lmgw-client's, or the error and its causes ([`describe`]).
pub fn why(e: &(dyn std::error::Error + 'static)) -> String {
    if let Some(alert) = certificate_alert(e) {
        return alert_said(&alert);
    }
    match refusal(e) {
        Some(r) => r.to_string(),
        None => describe(e),
    }
}

/// What a request that reached no lmgw says: a refused handshake as [`why`] says it, else "lmgw
/// is not reachable" and what broke.
pub fn unreachable(e: &(dyn std::error::Error + 'static)) -> String {
    if certificate_alert(e).is_some() || refusal(e).is_some() {
        return why(e);
    }
    format!("lmgw is not reachable: {}", describe(e))
}

mod race;
#[cfg(test)]
mod tests;
pub mod ws;

pub use race::{Answered, race};
