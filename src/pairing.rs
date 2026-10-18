//! Pairing (§3.1, K13; lmgw's device link design §3, §6.2): the `lmgw-pair:` link v2, the
//! enrolment it makes, and what a pairing keeps.
//!
//! ```text
//! lmgw-pair:?v=2&name=<name>&url=<https URL>[&url=…]&fp=sha256:<64 hex>&token=lmgw-enrol-<32 hex>
//! ```
//!
//! The link is `lmgw-client`'s ([`PairingLink`], strict, https only; a v1 link is told it is from
//! a pairing before TLS), and so are the facts a dialog shows ([`PairFacts`]). The link carries a
//! one-time token while it lives: nothing here logs it, and no error echoes any value taken from
//! it.
//!
//! [`enrol`] enrols the device's key ([`TlsKey`], P-256 in software, one per token: the core keeps
//! it, so trying a link again sends the same key) with the token: `POST /devices/enrol` over TLS
//! pinned to the link's `fp`, tried on the link's URLs by the device link's rule
//! (`crate::lmgw_link::tls::race`). lmgw answers the device's
//! certificate, its CA, the dates, the URLs and the confirmation code, which `lmgw-client` checks
//! against the link and the key. [`store`] then keeps the key in the Secret Service (the item's
//! `lmgw` attribute is the CA's fingerprint, its `kind` `device-tls-key`) and the rest, all of it
//! public, in `device-link.json` beside the configuration ([`Paired`]); what it could not store yet
//! (a locked wallet, a failed write) it stores at a later call, while Kai runs the pairing from
//! memory. The owner confirms the code on lmgw's Devices card; until then lmgw answers 403
//! `device_unconfirmed`.

mod key;
#[cfg(test)]
pub(crate) mod tests;

use std::path::Path;
use std::time::Duration;

use lmgw_client::requests::{self, KeyRefused};
use lmgw_client::tls::TlsRefusal;
use serde::{Deserialize, Serialize};

pub use key::TlsKey;
pub use lmgw_client::pairing::{FactLine, Fingerprint, LinkError, PairFacts, PairingLink};

use crate::lmgw_link::tls::{self, Tls};
use crate::paths::Paths;
use crate::secret::{SecretError, SecretStore, Stored};

/// What a pairing keeps beside the configuration (`device-link.json`, device link §6.2). None of
/// it is secret: the key is in the Secret Service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paired {
    /// The device's name as the link gave it (lmgw's row; `hello` names it again).
    pub name: String,
    /// The `fp` the link pinned (`sha256:<64 hex>`): lmgw's device CA, so which gateway this is.
    /// It names the Secret Service item that holds the key.
    pub fp: String,
    /// The certificate, lmgw's CA, the dates, the URLs and the confirmation code
    /// (`lmgw_client::requests::read_enrolment`'s).
    pub credentials: requests::Credentials,
}

impl Paired {
    /// The stored pairing, `None` when there is none. A file that does not read, or does not hold
    /// together ([`Paired::check`]), is an error naming it: pair again (the next pairing replaces
    /// it).
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        let p: Paired = serde_json::from_str(&text)
            .map_err(|e| format!("{} does not read ({e}): pair again", path.display()))?;
        p.check()
            .map_err(|e| format!("{} {e}: pair again", path.display()))?;
        Ok(Some(p))
    }

    /// Whether the parts hold together as an enrolment left them (`read_enrolment` checked them
    /// then): `fp` reads and pins the CA certificate kept beside it, which reads as a root, and the
    /// device's certificate is there. Whether the key is the certificate's is checked where the
    /// key is used (`crate::lmgw_link::tls::Tls::paired`).
    pub fn check(&self) -> Result<(), String> {
        let fp = Fingerprint::parse(&self.fp).map_err(|e| format!("holds no fp ({e})"))?;
        let ca = &self.credentials.ca_certificate;
        if !fp.pins_certificate(ca) {
            return Err(
                "does not hold together: its fp does not pin the CA certificate kept with it"
                    .to_owned(),
            );
        }
        rustls::RootCertStore::empty()
            .add(rustls::pki_types::CertificateDer::from(ca.as_slice()))
            .map_err(|e| {
                format!("does not hold together: its CA certificate does not read ({e})")
            })?;
        if self.credentials.certificate.is_empty() {
            return Err("does not hold together: it holds no device certificate".to_owned());
        }
        Ok(())
    }

    /// Written atomically (public data, the configuration's own mode).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self).expect("a pairing always serializes");
        text.push('\n');
        crate::config::write_atomically(path, text.as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// The URL that names this pairing in a dialog and the tooltip: the first the link gave.
    pub fn first_url(&self) -> &str {
        &self.credentials.urls.as_slice()[0]
    }

    /// The certificate's lapse as a date (`2027-10-14`, UTC).
    pub fn not_after(&self) -> String {
        date(self.credentials.not_after_ms)
    }
}

/// Now, as the certificate's dates count it (unix milliseconds, the system clock).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A unix time in milliseconds as a UTC date.
pub fn date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("{0}")]
    Link(#[from] LinkError),
    /// The device's key could not be made.
    #[error("{0}")]
    Key(String),
    /// The enrolment: lmgw's refusal in its words, a handshake lmgw-client refused, or no answer.
    #[error("{0}")]
    Enrol(String),
}

/// A finished enrolment, nothing stored yet.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub key: TlsKey,
    pub paired: Paired,
}

/// What the Devices card shows of the enrolling client (`client`, free text): Kai's version, the
/// computer's host name and the system's name (`Kai <version> on <host name> (<PRETTY_NAME>)`; the
/// review's decision 9).
pub fn client_text() -> String {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim().trim_matches('"').to_owned())
            })
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux".to_owned());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    match host {
        Some(host) => format!("Kai {} on {host} ({os})", crate::VERSION),
        None => format!("Kai {} ({os})", crate::VERSION),
    }
}

/// Enrols `key` with `link` (module doc): its public half sent with the link's token over TLS
/// pinned to the link's `fp`, lmgw's answer checked by `lmgw-client`. `timeout` is
/// `lmgw.timeout_s` (the connection, the answer and its body), `stagger` `lmgw.url_stagger_ms`.
/// Nothing is stored here ([`store`]).
///
/// The caller sends the same key again when it tries the same link again (lmgw's device link §3.3,
/// D8's lost answer): while the token lives, lmgw answers the key that used it what it answered it
/// (the same certificate, `confirmed` as it stands), and only another key meets 410
/// `pairing_token_used`, which marks the row contested. The race's tries may each reach lmgw, and
/// each sends the same key, so a second POST is such a repeat.
pub async fn enrol(
    link: &PairingLink,
    key: &TlsKey,
    client: &str,
    timeout: Duration,
    stagger: Duration,
) -> Result<Enrolled, PairError> {
    let spki = key.public_key().map_err(PairError::Key)?;
    let tls = Tls::enrolment(link.fp, timeout).map_err(PairError::Enrol)?;
    let req = requests::enrol(&link.token, &spki, Some(client));
    let secs = timeout.as_secs();
    let attempt = |base: &str, answered: tls::Answered| {
        // Each try's client is its own, so the race sees when its connection is up (§6.3).
        let http = tls.racing(answered);
        let url = req.url(base);
        let req = &req;
        async move {
            let http = http.map_err(|e| {
                format!(
                    "cannot build the HTTP client for lmgw: {}",
                    crate::lmgw_link::describe(&e)
                )
            })?;
            // The request carries the token: never logged.
            let mut rb = http.post(url);
            for h in &req.headers {
                if let Ok((name, value)) = crate::lmgw_link::header(h) {
                    rb = rb.header(name, value);
                }
            }
            let rb = rb.body(req.body.clone().unwrap_or_default());
            let resp = rb.send().await.map_err(|e| match tls::enrol_refusal(&e) {
                Some(TlsRefusal::Unproven) => UNPROVEN.to_owned(),
                Some(r) => r.to_string(),
                None => format!("lmgw is not reachable: {}", crate::lmgw_link::describe(&e)),
            })?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| {
                format!(
                    "lmgw's answer to the enrolment broke off: {}",
                    crate::lmgw_link::describe(&e)
                )
            })?;
            Ok::<_, String>((status, body))
        }
    };
    let urls = link.urls.order(None);
    let raced = tokio::time::timeout(timeout, tls::race(urls, stagger, attempt)).await;
    let (url, (status, body)) = match raced {
        Err(_) => {
            return Err(PairError::Enrol(format!(
                "lmgw did not answer the enrolment within {secs} s (lmgw.timeout_s)"
            )));
        }
        Ok(Err(failed)) => return Err(PairError::Enrol(said(failed))),
        Ok(Ok(won)) => won,
    };
    tracing::info!(%url, status, "pairing: lmgw answered the enrolment");
    let credentials = requests::read_enrolment(status, &body, link, &spki).map_err(|e| {
        PairError::Enrol(match requests::key_refused(&e) {
            Some(KeyRefused::TokenUsedElsewhere) => format!(
                "someone else used this code: delete the device on lmgw's Devices card \
                 (lmgw: {})",
                e.message
            ),
            _ => format!("lmgw refused the enrolment: {} ({})", e.message, e.code),
        })
    })?;
    Ok(Enrolled {
        key: key.clone(),
        paired: Paired {
            name: link.name.clone(),
            fp: link.fp.to_string(),
            credentials,
        },
    })
}

/// lmgw-client's [`TlsRefusal::Unproven`] at enrolment (lmgw's device link §3.3, its note of
/// 2026-10-15): a `certificate_required` alert. lmgw sends it while no pairing token lives, but an
/// alert is unauthenticated and may come before the server proved anything, so whoever answers at
/// the address can send one: a failed handshake to try again, which says nothing about the link. A
/// new link is suggested, never stated. A try again sends the same key (`crate::core`'s enrolment
/// keeps one per token), so an answer lost on the way comes back as it was.
const UNPROVEN: &str = "the handshake failed: the server answered with TLS's certificate_required \
     alert before it proved to be the lmgw this link pairs with, so something else may be \
     answering at that address, or the pairing link may have ended (closed on lmgw's Devices card, \
     expired or used). Try again; if it keeps failing, make a new link on lmgw's Devices card";

/// The race's failures as one sentence: one URL's error, or each URL's.
fn said(failed: Vec<(String, String)>) -> String {
    match failed.as_slice() {
        [(_, one)] => one.clone(),
        many => format!(
            "lmgw did not answer at any of the link's addresses: {}",
            many.iter()
                .map(|(u, e)| format!("{u}: {e}"))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// What of an enrolment is stored: the key in the Secret Service, the rest in `device-link.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoredParts {
    pub key: bool,
    pub file: bool,
}

/// Why an enrolment is not stored (yet): Kai runs it from memory meanwhile and tries again.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotStored {
    /// The wallet is locked; no unlock was asked for.
    #[error("KWallet is locked: unlock it")]
    WalletLocked,
    #[error("the wallet is not available ({0})")]
    Wallet(SecretError),
    /// `device-link.json` could not be written.
    #[error("{0}")]
    File(String),
}

/// Stores what of `enrolled` `done` does not hold yet (§3.1 step 3): the key as the Secret Service
/// item for its gateway (replacing an earlier key for the same gateway), then the public data.
/// `done` says how far it got; the first part that could not be stored says why.
pub async fn store<S: SecretStore>(
    store: &S,
    paths: &Paths,
    enrolled: &Enrolled,
    done: &mut StoredParts,
) -> Result<(), NotStored> {
    let p = &enrolled.paired;
    if !done.key {
        let label = format!("Kai: lmgw device link key '{}' ({})", p.name, p.first_url());
        match store.store(&p.fp, &label, &enrolled.key).await {
            Ok(Stored::Stored) => done.key = true,
            Ok(Stored::Locked) => return Err(NotStored::WalletLocked),
            Err(e) => return Err(NotStored::Wallet(e)),
        }
    }
    if !done.file {
        p.save(&paths.device_link_file).map_err(NotStored::File)?;
        done.file = true;
    }
    Ok(())
}
