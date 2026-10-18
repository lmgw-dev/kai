//! Pairing (lmgw's device link design §3, §6.2): the link v2 as Kai takes it, the device's key,
//! what a pairing keeps, and the enrolment against a mock of lmgw's device listener (a TLS server
//! under the tests' device CA, `crate::lmgw_link::test_pki`).

use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::*;
use crate::lmgw_link::test_pki::{self, Clients, FOREIGN, Gateway, PKI, Pki};
use crate::secret::MemoryStore;

pub const TOKEN: &str = "lmgw-enrol-0123456789abcdef0123456789abcdef";

/// A v2 link for the gateway `fp` pins at `url`.
pub fn link_for(fp: &Fingerprint, url: &str, name: &str) -> String {
    link_with_token(fp, url, name, TOKEN)
}

/// [`link_for`] with another one-time token (`lmgw-enrol-` and 32 hex digits).
pub fn link_with_token(fp: &Fingerprint, url: &str, name: &str, token: &str) -> String {
    let enc = |s: &str| form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!(
        "lmgw-pair:?v=2&name={}&url={}&fp={}&token={token}",
        enc(name),
        enc(url),
        enc(&fp.to_string())
    )
}

/// The tests' gateway ([`PKI`]) at `url`.
pub fn good_at(url: &str) -> String {
    link_for(&test_pki::fp(&PKI.ca), url, "desktop")
}

/// The tests' gateway on this computer.
pub fn good() -> String {
    good_at("https://127.0.0.1:8002")
}

/// Another gateway ([`FOREIGN`]) at `url`.
pub fn other_at(url: &str) -> String {
    link_for(&test_pki::fp(&FOREIGN.ca), url, "desktop")
}

/// A pairing with `pki`'s gateway at `url` as the core finds it at its start: the key in `store`,
/// the rest in `device-link.json`.
pub fn store_pairing(pki: &Pki, paths: &Paths, store: &MemoryStore, url: &str) -> Paired {
    let p = pki.paired(&[url]);
    p.save(&paths.device_link_file).unwrap();
    store
        .items
        .lock()
        .unwrap()
        .insert(p.fp.clone(), ("d".into(), pki.device_key.clone()));
    p
}

/// What lmgw's enrolment gives for `link` without a network: the pinned gateway's ([`PKI`] or
/// [`FOREIGN`]) certificate for its device key, at the link's URLs, named as the link names it.
pub fn enrolled_for(link: &PairingLink) -> Enrolled {
    let pki: &Pki = if link.fp == test_pki::fp(&FOREIGN.ca) {
        &FOREIGN
    } else {
        &PKI
    };
    let urls: Vec<&str> = link.urls.as_slice().iter().map(String::as_str).collect();
    let mut paired = pki.paired(&urls);
    paired.name = link.name.clone();
    Enrolled {
        key: pki.device_key.clone(),
        paired,
    }
}

/// The core's enrolment in the tests ([`enrolled_for`]; the key the core sends is not used).
pub fn enroller() -> crate::core::Enroller {
    std::sync::Arc::new(|link: PairingLink, _key: TlsKey| {
        Box::pin(async move { Ok(enrolled_for(&link)) })
    })
}

const WAIT: Duration = Duration::from_secs(10);

fn fresh() -> TlsKey {
    TlsKey::generate().unwrap()
}

#[test]
fn a_v2_link_parses_and_a_v1_link_is_told_it_is_from_before_tls() {
    let l = PairingLink::parse(&good_at("https://127.0.0.1:8002")).unwrap();
    assert_eq!(l.urls.as_slice(), ["https://127.0.0.1:8002"]);
    assert_eq!(l.fp, test_pki::fp(&PKI.ca));
    assert_eq!(l.token.expose(), TOKEN);
    assert!(
        !format!("{l:?}").contains(&TOKEN[11..]),
        "the token is redacted"
    );
    let v1 = "lmgw-pair:?v=1&url=http%3A%2F%2F127.0.0.1%3A8001&name=desktop&key=lmgw-device-00";
    let e = PairingLink::parse(v1).unwrap_err().to_string();
    assert!(e.contains("from a pairing before TLS"), "{e}");
    let plain = good_at("http://127.0.0.1:8001");
    let e = PairingLink::parse(&plain).unwrap_err().to_string();
    assert!(e.contains("https"), "{e}");
}

#[test]
fn the_key_is_p256_kept_as_pem_and_never_shown() {
    let key = TlsKey::generate().unwrap();
    let spki = key.public_key().unwrap();
    // id-ecPublicKey with prime256v1, then the uncompressed point: canonical as lmgw requires.
    assert_eq!(spki.len(), 91);
    assert_eq!(spki[26], 0x04);
    let pem = key.to_pem();
    assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\n"), "{}", *pem);
    assert!(pem.lines().all(|l| l.len() <= 64));
    assert_eq!(
        pem.len(),
        pem.capacity(),
        "sized up front, never reallocated"
    );
    assert_eq!(TlsKey::from_pem(&pem).unwrap(), key);
    assert!(TlsKey::from_pem("not a key").is_err());
    assert_eq!(format!("{key:?}"), "TlsKey(<redacted>)");
    assert_ne!(TlsKey::generate().unwrap(), key, "a fresh key each time");
}

#[test]
fn what_a_pairing_keeps_reads_back_and_a_bad_file_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kai").join("device-link.json");
    assert_eq!(Paired::load(&path), Ok(None));
    let p = PKI.paired(&["https://127.0.0.1:8002"]);
    p.save(&path).unwrap();
    assert_eq!(Paired::load(&path), Ok(Some(p.clone())));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("PRIVATE KEY"), "public data only");
    std::fs::write(&path, "{").unwrap();
    let e = Paired::load(&path).unwrap_err();
    assert!(
        e.contains("device-link.json") && e.contains("pair again"),
        "{e}"
    );
    let mut bad = serde_json::to_value(&p).unwrap();
    bad["fp"] = "sha256:00".into();
    std::fs::write(&path, bad.to_string()).unwrap();
    assert!(Paired::load(&path).is_err());
}

#[test]
fn dates_are_utc_days() {
    assert_eq!(date(0), "1970-01-01");
    assert_eq!(date(4_102_444_800_000), "2100-01-01");
    assert_eq!(date(1_823_097_791_000), "2027-10-09");
    assert_eq!(date(951_782_400_000), "2000-02-29");
}

/// Device link §3.4: the enrolment goes over TLS pinned to the link's `fp` without a certificate,
/// sends the token and the new key's public half, and keeps what lmgw answered; the certificate
/// then opens lmgw's device listener with that key, which answers `device_unconfirmed` until the
/// owner confirms.
#[tokio::test]
async fn an_enrolment_gets_a_certificate_that_opens_the_device_listener() {
    let g = Gateway::bind_as(&PKI, Clients::Optional).await;
    let link = PairingLink::parse(&good_at(&g.url)).unwrap();
    let server = async {
        let (mut s, _) = g.accept().await.unwrap();
        assert!(
            s.get_ref().1.peer_certificates().is_none(),
            "no certificate at enrolment"
        );
        let (head, body) = test_pki::read_request(&mut s).await;
        assert!(
            head.starts_with("POST /devices/enrol HTTP/1.1\r\n"),
            "{head}"
        );
        let sent: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["token"], TOKEN);
        assert!(sent["client"].as_str().unwrap().starts_with("Kai "));
        let answer = test_pki::enrolment_answer(&PKI, &g.url, &body);
        s.write_all(test_pki::answer("201 Created", &answer).as_bytes())
            .await
            .unwrap();
    };
    let client = client_text();
    let key = TlsKey::generate().unwrap();
    let (enrolled, ()) = tokio::join!(
        enrol(&link, &key, &client, WAIT, Duration::from_millis(250)),
        server
    );
    let enrolled = enrolled.unwrap();
    let p = &enrolled.paired;
    assert_eq!(p.name, "desktop");
    assert_eq!(p.fp, test_pki::fp(&PKI.ca).to_string());
    assert_eq!(p.first_url(), g.url);
    assert_eq!(
        p.credentials.confirm_code,
        requests::confirm_code(&enrolled.key.public_key().unwrap())
    );

    // The certificate with the new key, against the device listener.
    let listener = Gateway::bind().await;
    let tls = Tls::paired(&p.credentials, &enrolled.key, WAIT).unwrap();
    for (status, code) in [("403 Forbidden", "device_unconfirmed"), ("200 OK", "")] {
        let server = async {
            let (mut s, _) = listener.accept().await.unwrap();
            let (head, _) = test_pki::read_request(&mut s).await;
            assert!(!head.to_ascii_lowercase().contains("authorization"));
            let body = serde_json::json!({"code": code, "message": "Confirm it"}).to_string();
            s.write_all(test_pki::answer(status, &body).as_bytes())
                .await
                .unwrap();
        };
        let (resp, ()) = tokio::join!(
            tls.http()
                .get(format!("{}/chat/api/feed", listener.url))
                .send(),
            server
        );
        let resp = resp.unwrap();
        let status = resp.status().as_u16();
        let e = requests::read_refusal(status, &resp.text().await.unwrap());
        if code.is_empty() {
            assert_eq!(status, 200);
        } else {
            assert_eq!(
                requests::key_refused(&e),
                Some(KeyRefused::Unconfirmed),
                "until the owner confirms"
            );
        }
    }

    // Stored: the key in the wallet under the gateway's fingerprint, the rest beside the
    // configuration.
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under_config_home(dir.path());
    let wallet = MemoryStore::default();
    let mut done = StoredParts::default();
    assert_eq!(store(&wallet, &paths, &enrolled, &mut done).await, Ok(()));
    assert_eq!(
        done,
        StoredParts {
            key: true,
            file: true
        }
    );
    assert_eq!(
        wallet.items.lock().unwrap()[&p.fp].1,
        enrolled.key,
        "the key, under the CA's fingerprint"
    );
    assert_eq!(
        Paired::load(&paths.device_link_file).unwrap().as_ref(),
        Some(p)
    );
    // A locked wallet: nothing is stored, and a later call stores it all.
    wallet.items.lock().unwrap().clear();
    wallet.set_locked(true);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under_config_home(dir.path());
    let mut done = StoredParts::default();
    assert_eq!(
        store(&wallet, &paths, &enrolled, &mut done).await,
        Err(NotStored::WalletLocked)
    );
    assert_eq!(done, StoredParts::default());
    assert_eq!(
        Paired::load(&paths.device_link_file),
        Ok(None),
        "nothing stored"
    );
    wallet.set_locked(false);
    assert_eq!(store(&wallet, &paths, &enrolled, &mut done).await, Ok(()));
    assert!(done.key && done.file);
    // A file that cannot be written after the wallet took the key (S4): the key stays stored,
    // and the next call writes only the file.
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.device_link_file.join("in-the-way")).unwrap();
    let mut done = StoredParts::default();
    let e = store(&wallet, &paths, &enrolled, &mut done)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, NotStored::File(w) if w.contains("cannot write")),
        "{e}"
    );
    assert_eq!(
        done,
        StoredParts {
            key: true,
            file: false
        }
    );
    std::fs::remove_dir_all(&paths.device_link_file).unwrap();
    wallet.set_locked(true);
    assert_eq!(
        store(&wallet, &paths, &enrolled, &mut done).await,
        Ok(()),
        "the wallet is not asked again"
    );
    assert_eq!(
        Paired::load(&paths.device_link_file).unwrap().as_ref(),
        Some(p)
    );
}

/// A server that does not hold the key the link pins is not the lmgw it pairs with: the token
/// never reaches it.
#[tokio::test]
async fn an_enrolment_with_another_lmgw_sends_nothing() {
    let g = Gateway::bind_as(&FOREIGN, Clients::Optional).await;
    let link = PairingLink::parse(&good_at(&g.url)).unwrap();
    let (e, accepted) = tokio::join!(
        enrol(
            &link,
            &PKI.device_key,
            "test",
            WAIT,
            Duration::from_millis(250)
        ),
        g.accept()
    );
    assert!(accepted.is_err(), "the handshake completed");
    let e = e.unwrap_err().to_string();
    assert!(e.contains("not the lmgw this link pairs with"), "{e}");
}

/// D8's lost answer (lmgw's device link §3.3, its note of 2026-10-15): lmgw took the token for the
/// key and issued its certificate, but the connection broke before the answer arrived. The same key
/// asked again gets that certificate and code; another key would meet 410 `pairing_token_used`.
#[tokio::test]
async fn the_same_key_asked_again_gets_the_answer_that_was_lost() {
    let g = Gateway::bind_as(&PKI, Clients::Optional).await;
    let link = PairingLink::parse(&good_at(&g.url)).unwrap();
    let key = fresh();
    let public_key = |body: &[u8]| {
        serde_json::from_slice::<serde_json::Value>(body).unwrap()["public_key"].clone()
    };
    let first = async {
        let (mut s, _) = g.accept().await.unwrap();
        let (_, body) = test_pki::read_request(&mut s).await;
        let answer = test_pki::enrolment_answer(&PKI, &g.url, &body);
        // The connection breaks before the answer.
        drop(s);
        (public_key(&body), answer)
    };
    let (lost, (used_by, answer)) = tokio::join!(
        enrol(&link, &key, "test", WAIT, Duration::from_millis(250)),
        first
    );
    assert!(lost.is_err(), "no answer arrived");
    // lmgw's repeat: the key that used the token gets the same answer, any other 410.
    let repeat = |body: &[u8]| {
        if public_key(body) == used_by {
            test_pki::answer("201 Created", &answer)
        } else {
            let refusal = serde_json::json!({"code": "pairing_token_used", "message": "used"});
            test_pki::answer("410 Gone", &refusal.to_string())
        }
    };
    for (k, same) in [(key.clone(), true), (fresh(), false)] {
        let server = async {
            let (mut s, _) = g.accept().await.unwrap();
            let (_, body) = test_pki::read_request(&mut s).await;
            let reply = repeat(&body);
            s.write_all(reply.as_bytes()).await.unwrap();
        };
        let (got, ()) = tokio::join!(
            enrol(&link, &k, "test", WAIT, Duration::from_millis(250)),
            server
        );
        if same {
            let got = got.unwrap();
            let issued: serde_json::Value = serde_json::from_str(&answer).unwrap();
            assert_eq!(
                lmgw_client::base64::encode(&got.paired.credentials.certificate),
                issued["certificate"].as_str().unwrap(),
                "the certificate issued for the first try"
            );
            assert_eq!(got.key, key);
            assert_eq!(
                got.paired.credentials.confirm_code,
                requests::confirm_code(&key.public_key().unwrap())
            );
        } else {
            let e = got.unwrap_err().to_string();
            assert!(e.contains("someone else used this code"), "{e}");
        }
    }
}

/// While no token is pending lmgw asks every connection for a certificate (device link §3.3), so an
/// ended link meets a `certificate_required` alert; so does a squatter that sends one. An alert
/// comes before the server proved anything (its note of 2026-10-15): a failed handshake to try
/// again, a new link suggested, never stated.
#[tokio::test]
async fn a_certificate_required_alert_is_a_handshake_to_try_again() {
    let g = Gateway::bind_as(&PKI, Clients::Required).await;
    let link = PairingLink::parse(&good_at(&g.url)).unwrap();
    let (e, _) = tokio::join!(
        enrol(
            &link,
            &PKI.device_key,
            "test",
            WAIT,
            Duration::from_millis(250)
        ),
        g.accept()
    );
    let e = e.unwrap_err().to_string();
    assert!(e.contains("certificate_required"), "{e}");
    assert!(
        e.contains("may have ended") && e.contains("something else may be answering"),
        "both readings, neither stated: {e}"
    );
    assert!(
        e.contains("Try again; if it keeps failing, make a new link"),
        "{e}"
    );
    assert!(!e.contains("has ended"), "{e}");
    // A process squatting on the address says the same, and is answered the same.
    let squatter = test_pki::Squatter::bind().await;
    let link = PairingLink::parse(&good_at(&squatter.url)).unwrap();
    let (said, ()) = tokio::join!(
        enrol(
            &link,
            &PKI.device_key,
            "test",
            WAIT,
            Duration::from_millis(250)
        ),
        squatter.answer(test_pki::CERTIFICATE_REQUIRED)
    );
    assert_eq!(said.unwrap_err().to_string(), e);
}

/// lmgw's refusals of an enrolment: a token another key used is the design's warning; any other in
/// lmgw's words with its code.
#[tokio::test]
async fn an_enrolment_lmgw_refuses_says_why() {
    for (status, code, message, want) in [
        (
            "410 Gone",
            "pairing_token_used",
            "the token was used",
            "someone else used this code: delete the device on lmgw's Devices card",
        ),
        (
            "410 Gone",
            "pairing_token_expired",
            "the token expired at 14:32 (devices.token_minutes)",
            "devices.token_minutes",
        ),
    ] {
        let g = Gateway::bind_as(&PKI, Clients::Optional).await;
        let link = PairingLink::parse(&good_at(&g.url)).unwrap();
        let server = async {
            let (mut s, _) = g.accept().await.unwrap();
            test_pki::read_request(&mut s).await;
            let body = serde_json::json!({"code": code, "message": message}).to_string();
            s.write_all(test_pki::answer(status, &body).as_bytes())
                .await
                .unwrap();
        };
        let (e, ()) = tokio::join!(
            enrol(
                &link,
                &PKI.device_key,
                "test",
                WAIT,
                Duration::from_millis(250)
            ),
            server
        );
        let e = e.unwrap_err().to_string();
        assert!(e.contains(want), "{code}: {e}");
    }
}

/// A stored pairing that does not hold together is refused at load: an `fp` that does not pin
/// the CA certificate kept with it, a CA certificate that does not read, no device certificate.
#[test]
fn a_stored_pairing_that_does_not_hold_together_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device-link.json");
    let good = PKI.paired(&["https://127.0.0.1:8002"]);
    good.save(&path).unwrap();
    assert_eq!(Paired::load(&path).unwrap(), Some(good.clone()));
    let mut other_fp = good.clone();
    other_fp.fp = test_pki::fp(&FOREIGN.ca).to_string();
    let mut no_ca = good.clone();
    no_ca.credentials.ca_certificate = b"not a certificate".to_vec();
    let mut foreign_ca = good.clone();
    foreign_ca.credentials.ca_certificate = FOREIGN.ca.der().to_vec();
    let mut no_cert = good.clone();
    no_cert.credentials.certificate.clear();
    let mut bad_fp = good.clone();
    bad_fp.fp = "sha256:00".into();
    for (p, says) in [
        (other_fp, "does not pin the CA"),
        (no_ca, "does not pin the CA"),
        (foreign_ca, "does not pin the CA"),
        (no_cert, "no device certificate"),
        (bad_fp, "holds no fp"),
    ] {
        p.save(&path).unwrap();
        let e = Paired::load(&path).unwrap_err();
        assert!(e.contains(says) && e.contains("pair again"), "{e}");
    }
}

/// The review's decision 9: the Devices card names the client by Kai's version, the computer's
/// host name and the system's name.
#[test]
fn the_client_names_kai_the_host_and_the_system() {
    let text = client_text();
    assert!(
        text.starts_with(&format!("Kai {} ", crate::VERSION)),
        "{text}"
    );
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    assert!(text.contains(&format!(" on {} (", host.trim())), "{text}");
    assert!(text.ends_with(')'), "{text}");
}

/// Not a unit test: the enrolment the live checks that hand a pairing to a live test of their own
/// run against a dev lmgw (`scripts/live-check-window.sh`, `scripts/live-check-widgets.sh`, through
/// `scripts/live-device-link.sh`'s `enrol_files`). `KAI_LIVE_ENROL` names a directory under a
/// `target` directory whose `link` (mode 600: the script's `key_create` link, never in argv) is
/// enrolled with a new key; it writes `device-link.json`, `key.pem` (mode 600) and `code`, the
/// confirmation code the script confirms with `device_confirm`, and removes `link`. Ignored by
/// default, and refused outside a `target` directory.
#[tokio::test]
#[ignore]
async fn live_enrol() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = std::path::PathBuf::from(
        std::env::var_os("KAI_LIVE_ENROL")
            .expect("KAI_LIVE_ENROL names the live check's directory"),
    );
    assert!(
        dir.components().any(|c| c.as_os_str() == "target"),
        "KAI_LIVE_ENROL={} is outside a target directory",
        dir.display()
    );
    let text = std::fs::read_to_string(dir.join("link")).expect("the link file");
    let link = PairingLink::parse(text.trim()).expect("a v2 pairing link");
    let key = TlsKey::generate().unwrap();
    let enrolled = enrol(
        &link,
        &key,
        &client_text(),
        Duration::from_secs(10),
        Duration::from_millis(250),
    )
    .await
    .unwrap_or_else(|e| panic!("the enrolment: {e}"));
    enrolled.paired.save(&dir.join("device-link.json")).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join("key.pem"))
        .unwrap()
        .write_all(enrolled.key.to_pem().as_bytes())
        .unwrap();
    std::fs::write(
        dir.join("code"),
        format!("{}\n", enrolled.paired.credentials.confirm_code),
    )
    .unwrap();
    // The token is used: the link goes.
    std::fs::remove_file(dir.join("link")).unwrap();
}
