//! The tests' device link: a device CA, server leaves and device certificates made with rcgen as
//! lmgw makes them (lmgw's device link design §2: P-256, the CA with `pathlen 0`, leaves for
//! `serverAuth` naming 127.0.0.1 and `localhost`, device certificates for `clientAuth`), the
//! credentials a paired Kai has, and [`Gateway`], a loopback TLS listener as lmgw's device listener
//! serves (TLS 1.3, the client's certificate checked against the CA). The tests' servers accept
//! through it and speak HTTP or WebSocket over the stream it hands over.

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use lmgw_client::pairing::Fingerprint;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PublicKeyData, date_time_ymd,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use super::Credentials;
use super::tls::{Reach, Tls};
use crate::pairing::{Paired, TlsKey};

pub type Ca = CertifiedIssuer<'static, KeyPair>;

/// A device CA named as lmgw names its own; `cn` tells them apart.
pub fn ca(cn: &str) -> Ca {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    dn.push(DnType::OrganizationName, "lmgw");
    p.distinguished_name = dn;
    p.not_before = date_time_ymd(2026, 1, 1);
    p.not_after = date_time_ymd(9999, 12, 31);
    CertifiedIssuer::self_signed(p, KeyPair::generate().unwrap()).unwrap()
}

/// The `fp` that pins `ca`.
pub fn fp(ca: &Ca) -> Fingerprint {
    Fingerprint::of_spki(&ca.key().subject_public_key_info())
}

fn leaf_params(
    names: &[&str],
    eku: ExtendedKeyUsagePurpose,
    years: (i32, i32),
) -> CertificateParams {
    let mut p =
        CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![eku];
    p.use_authority_key_identifier_extension = true;
    p.not_before = date_time_ymd(years.0, 1, 1);
    p.not_after = date_time_ymd(years.1, 1, 1);
    p
}

/// A device's public key as enrolment sends it (a DER SubjectPublicKeyInfo of a P-256 key), as
/// rcgen signs for it: lmgw's enrolment, in the mock gateways.
pub struct DevicePub(pub Vec<u8>);

impl PublicKeyData for DevicePub {
    fn der_bytes(&self) -> &[u8] {
        // The uncompressed point closes a canonical P-256 SubjectPublicKeyInfo.
        &self.0[self.0.len() - 65..]
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

/// A device certificate for the public key `spki`, valid for `years`, as lmgw issues it.
pub fn device_cert(ca: &Ca, spki: &[u8], years: (i32, i32)) -> Vec<u8> {
    leaf_params(&[], ExtendedKeyUsagePurpose::ClientAuth, years)
        .signed_by(&DevicePub(spki.to_vec()), ca)
        .unwrap()
        .der()
        .to_vec()
}

/// A server leaf for loopback (`127.0.0.1`, `localhost`) and its key, as rustls serves it.
fn server_leaf(ca: &Ca) -> (Vec<u8>, KeyPair) {
    let key = KeyPair::generate().unwrap();
    let der = leaf_params(
        &["127.0.0.1", "localhost"],
        ExtendedKeyUsagePurpose::ServerAuth,
        (2020, 2100),
    )
    .signed_by(&key, ca)
    .unwrap()
    .der()
    .to_vec();
    (der, key)
}

/// One gateway's PKI: its CA, its loopback leaf, and a device paired with it.
pub struct Pki {
    pub ca: Ca,
    leaf: (Vec<u8>, KeyPair),
    pub device_key: TlsKey,
    pub device_cert: Vec<u8>,
}

impl Pki {
    pub fn new(cn: &str) -> Self {
        let ca = ca(cn);
        let leaf = server_leaf(&ca);
        let device_key = TlsKey::generate().unwrap();
        let device_cert = device_cert(&ca, &device_key.public_key().unwrap(), (2020, 2100));
        Self {
            ca,
            leaf,
            device_key,
            device_cert,
        }
    }

    /// What a pairing with this gateway at `urls` keeps (the code is the device key's).
    pub fn paired(&self, urls: &[&str]) -> Paired {
        let urls: Vec<String> = urls.iter().map(|u| (*u).to_owned()).collect();
        Paired {
            name: "desktop".into(),
            fp: fp(&self.ca).to_string(),
            credentials: lmgw_client::requests::Credentials {
                device_id: 7,
                certificate: self.device_cert.clone(),
                ca_certificate: self.ca.der().to_vec(),
                not_after_ms: 4_102_444_800_000,
                notice_after_ms: 4_000_000_000_000,
                urls: urls.try_into().unwrap(),
                confirm_code: lmgw_client::requests::confirm_code(
                    &self.device_key.public_key().unwrap(),
                ),
            },
        }
    }

    /// A paired Kai's credentials for this gateway at `url` (`https://…`).
    pub fn creds(&self, url: &str) -> Credentials {
        let p = self.paired(&[url]);
        Credentials {
            reach: Reach::new(p.credentials.urls.clone()),
            tls: Tls::paired(&p.credentials, &self.device_key, Duration::from_secs(10)).unwrap(),
        }
    }

    /// A TLS 1.3 server configuration with this gateway's leaf; `clients`: the device CA every
    /// client's certificate must chain to, or none asked for.
    pub fn server_config(&self, clients: Clients) -> rustls::ServerConfig {
        let provider = super::tls::provider();
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap();
        let builder = match clients {
            Clients::None => builder.with_no_client_auth(),
            Clients::Required | Clients::Optional => {
                let mut roots = rustls::RootCertStore::empty();
                roots.add(self.ca.der().clone()).unwrap();
                let v = rustls::server::WebPkiClientVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider,
                );
                let v = match clients {
                    Clients::Optional => v.allow_unauthenticated(),
                    _ => v,
                };
                builder.with_client_cert_verifier(v.build().unwrap())
            }
        };
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(self.leaf.1.serialize_der().into());
        builder
            .with_single_cert(vec![self.leaf.0.clone().into(), self.ca.der().clone()], key)
            .unwrap()
    }
}

/// What a test gateway asks of its clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clients {
    /// A certificate under the device CA, as lmgw's device listener while no token is pending.
    Required,
    /// A certificate if there is one, as while a token is pending (enrolment).
    Optional,
    /// None asked for.
    None,
}

/// The tests' gateway and paired device (made once: key generation and signing cost time).
pub static PKI: LazyLock<Pki> = LazyLock::new(|| Pki::new("lmgw devices test"));

/// Another gateway, whose CA the paired device does not trust.
pub static FOREIGN: LazyLock<Pki> = LazyLock::new(|| Pki::new("lmgw devices foreign"));

/// A paired Kai's credentials for [`PKI`]'s gateway at `url`.
pub fn creds(url: &str) -> Credentials {
    PKI.creds(url)
}

/// A loopback TLS listener as lmgw's device listener serves (module doc).
pub struct Gateway {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    pub url: String,
}

impl Gateway {
    /// [`PKI`]'s gateway, a client certificate required.
    pub async fn bind() -> Self {
        Self::bind_as(&PKI, Clients::Required).await
    }

    pub async fn bind_as(pki: &Pki, clients: Clients) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        Self {
            listener,
            acceptor: TlsAcceptor::from(Arc::new(pki.server_config(clients))),
            url,
        }
    }

    /// The next connection's TLS stream, once its handshake is done (the client's failure to
    /// verify this gateway, or this gateway's refusal of its certificate, is the error).
    pub async fn accept(&self) -> std::io::Result<(TlsStream<TcpStream>, SocketAddr)> {
        let (tcp, peer) = self.listener.accept().await?;
        let tls = self.acceptor.accept(tcp).await?;
        Ok((tls, peer))
    }
}

/// TLS's alert numbers (RFC 8446 §6) a [`Squatter`] sends.
pub const BAD_CERTIFICATE: u8 = 42;
pub const CERTIFICATE_EXPIRED: u8 = 45;
pub const UNKNOWN_CA: u8 = 48;
pub const CERTIFICATE_REQUIRED: u8 = 116;

/// A process squatting on lmgw's port (the security review's B1): plain TCP that answers each
/// ClientHello with a fatal alert in the clear, before it proved anything.
pub struct Squatter {
    listener: TcpListener,
    pub url: String,
}

impl Squatter {
    pub async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        Self { listener, url }
    }

    /// The next connection: its ClientHello's record read, then `alert` (TLS's number) sent as a
    /// plaintext fatal alert record, and the connection closed.
    pub async fn answer(&self, alert: u8) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut s, _) = self.listener.accept().await.unwrap();
        let mut head = [0u8; 5];
        s.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 22, "a handshake record (the ClientHello)");
        let mut hello = vec![0u8; usize::from(u16::from_be_bytes([head[3], head[4]]))];
        s.read_exact(&mut hello).await.unwrap();
        // content type 21 (alert), legacy version 3.3, length 2, level 2 (fatal), the alert.
        s.write_all(&[21, 3, 3, 0, 2, 2, alert]).await.unwrap();
        let _ = s.shutdown().await;
    }
}

/// One request's head and body (by its `Content-Length`), read off `s`.
pub async fn read_request<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> (String, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the request ended early");
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8(buf[..end].to_vec()).unwrap();
    let len: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the body ended early");
        body.extend_from_slice(&chunk[..n]);
    }
    (head, body)
}

/// A whole HTTP answer with a JSON body, closing the connection.
pub fn answer(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// lmgw's enrolment as `pki`'s gateway answers it at `url`: a certificate for the key the request
/// sends, the CA, the dates, the URL and the key's code (lmgw's device link §3.4).
pub fn enrolment_answer(pki: &Pki, url: &str, body: &[u8]) -> String {
    let req: lmgw_client::types::device_link::EnrolRequest = serde_json::from_slice(body).unwrap();
    let spki = lmgw_client::base64::decode(&req.public_key).unwrap();
    let cert = device_cert(&pki.ca, &spki, (2020, 2100));
    serde_json::json!({
        "device": {"id": 7, "name": "desktop"},
        "certificate": lmgw_client::base64::encode(&cert),
        "ca_certificate": lmgw_client::base64::encode(pki.ca.der()),
        "not_after": "2100-01-01T00:00:00Z",
        "notice_after": "2099-01-01T00:00:00Z",
        "urls": [url],
        "confirm_code": lmgw_client::requests::confirm_code(&spki),
        "confirmed": false,
    })
    .to_string()
}
