//! The device link's TLS (lmgw's device link design D15, §6.2): every connection Kai makes to lmgw
//! comes from the one configuration, trusts lmgw's device CA alone and presents the device's
//! certificate; a refused handshake reads in `lmgw-client`'s words; the URL rule. The host link's
//! refusal of another CA is `crate::host::tests::runs`'.

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::*;
use crate::lmgw_link::test_pki::{self, Clients, FOREIGN, Gateway, PKI, Pki};
use crate::lmgw_link::{
    FeedLink, FeedSettings, LinkEvent, LmgwLink, RealtimeLink, RealtimeSettings, mcp, proxy,
};
use crate::voice::link::{SessionEvent, VoiceLink};

const WAIT: Duration = Duration::from_secs(10);

/// The gateway's next handshake, which Kai must have broken off (it sent nothing).
async fn refused_at(g: &Gateway) {
    let r = tokio::time::timeout(WAIT, g.accept())
        .await
        .expect("Kai connected");
    assert!(r.is_err(), "the handshake with another CA completed");
}

/// D15: the requests (reqwest), the WebSocket connector, the feed, the voice session's requests and
/// its WebSocket, Kai's `/mcp` client and the settings window's proxy all refuse a server whose
/// certificate lmgw's device CA did not sign, before they send anything.
#[tokio::test]
async fn every_connection_refuses_a_server_under_another_ca() {
    let g = Gateway::bind_as(&FOREIGN, Clients::None).await;
    let creds = PKI.creds(&g.url);
    let authority = TlsRefusal::AuthorityChanged.to_string();

    // A request, and the WebSocket connector both WebSocket links use.
    let req = creds.http().get(format!("{}/v1/models", g.url)).send();
    let (e, ()) = tokio::join!(req, refused_at(&g));
    assert_eq!(refusal(&e.unwrap_err()), Some(TlsRefusal::AuthorityChanged));
    let request = format!("{}/v1/realtime", g.url.replacen("https", "wss", 1))
        .into_client_request()
        .unwrap();
    let ws = creds.tls.websocket(request, None, false, WAIT);
    let (e, ()) = tokio::join!(ws, refused_at(&g));
    assert_eq!(refusal(&e.unwrap_err()), Some(TlsRefusal::AuthorityChanged));

    // The feed: its note says why, and it tries again (the next URL may be the owner's lmgw).
    let mut feed = FeedLink::new(FeedSettings {
        retry_max: Duration::from_secs(30),
        timeout: Duration::from_secs(5),
        stagger: Duration::from_millis(250),
    });
    let (tx, mut events) = mpsc::unbounded_channel();
    feed.start(creds.clone(), tx);
    refused_at(&g).await;
    match tokio::time::timeout(WAIT, events.recv()).await.unwrap() {
        Some(LinkEvent::Down { note, .. }) => {
            assert!(note.contains(&authority), "{note}")
        }
        other => panic!("not a Down: {other:?}"),
    }
    feed.stop();

    // The voice session's requests and its WebSocket.
    let mut voice = RealtimeLink::new(RealtimeSettings {
        timeout: Duration::from_secs(5),
        keepalive: Duration::from_secs(15),
        send_queue: Duration::from_secs(30),
    });
    voice.set_credentials(Some(creds.clone()));
    let (e, ()) = tokio::join!(voice.current(1, false), refused_at(&g));
    let e = e.unwrap_err();
    assert!(e.message.contains(&authority), "{e}");
    let (tx, mut session) = mpsc::unbounded_channel();
    let _socket = voice.open(5, false, tx);
    refused_at(&g).await;
    match tokio::time::timeout(WAIT, session.recv()).await.unwrap() {
        Some(SessionEvent::Refused { message, .. }) => {
            assert!(message.contains(&authority), "{message}")
        }
        other => panic!("not a refusal: {other:?}"),
    }

    // Kai's /mcp client and the settings window's proxy.
    let bound = mcp::Bound {
        bytes: 1 << 20,
        says: "overlay.app_max_mb (test)".into(),
    };
    let open = mcp::McpSession::open(creds.clone(), Duration::from_secs(5), bound);
    let (e, ()) = tokio::join!(open, refused_at(&g));
    let e = e.expect_err("refused").text();
    assert!(e.contains(&authority), "{e}");
    let forward = proxy::Forward {
        method: proxy::Method::Get,
        path: "/chat/api/profiles".into(),
        body: Vec::new(),
    };
    let limits = proxy::Limits {
        max_mb: 1,
        timeout_s: 5,
    };
    let (answer, ()) = tokio::join!(proxy::send(&creds, forward, limits), refused_at(&g));
    let body = String::from_utf8(answer.body).unwrap();
    assert_eq!(answer.status, 502);
    assert!(body.contains("authority changed"), "{body}");
}

/// The WebSocket takes `wss://` only: a `ws://` URL is refused before any connection.
#[tokio::test]
async fn no_websocket_goes_over_plain_ws() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let request = format!("ws://{}/v1/realtime", l.local_addr().unwrap())
        .into_client_request()
        .unwrap();
    let e = PKI
        .creds("https://127.0.0.1:1")
        .tls
        .websocket(request, None, false, WAIT)
        .await;
    assert!(e.is_err(), "a plain WebSocket was opened");
    let seen = tokio::time::timeout(Duration::from_millis(300), l.accept()).await;
    assert!(seen.is_err(), "a plain connection was made");
}

/// lmgw's answer that points elsewhere is never followed (no redirects on lmgw's client).
#[tokio::test]
async fn a_redirect_is_not_followed() {
    let g = Gateway::bind().await;
    let creds = PKI.creds(&g.url);
    let server = async {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = test_pki::read_request(&mut s).await;
        s.write_all(
            b"HTTP/1.1 302 Found\r\nlocation: https://127.0.0.1:1/elsewhere\r\n\
              content-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    };
    let (resp, ()) = tokio::join!(
        creds.http().get(format!("{}/v1/models", g.url)).send(),
        server
    );
    assert_eq!(resp.unwrap().status(), 302);
}

/// The constructor's client takes https only: a plain URL is refused before any connection.
#[tokio::test]
async fn nothing_goes_over_plain_http() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/models", l.local_addr().unwrap());
    let e = PKI
        .creds("https://127.0.0.1:1")
        .http()
        .get(&url)
        .send()
        .await;
    assert!(e.is_err(), "a plain URL was sent");
    let seen = tokio::time::timeout(Duration::from_millis(300), l.accept()).await;
    assert!(seen.is_err(), "a plain connection was made");
}

/// A paired connection reaches lmgw's device listener with the device's certificate, and nothing
/// else authenticates it: no credential header goes with it.
#[tokio::test]
async fn a_paired_request_presents_the_device_certificate_and_no_header() {
    let g = Gateway::bind().await;
    let creds = PKI.creds(&g.url);
    let server = async {
        let (mut s, _) = g.accept().await.unwrap();
        let certs = s.get_ref().1.peer_certificates().unwrap().to_vec();
        let (head, _) = test_pki::read_request(&mut s).await;
        s.write_all(test_pki::answer("200 OK", "{}").as_bytes())
            .await
            .unwrap();
        (certs, head)
    };
    let (resp, (certs, head)) = tokio::join!(
        creds.http().get(format!("{}/v1/models", g.url)).send(),
        server
    );
    assert_eq!(resp.unwrap().status(), 200);
    assert_eq!(certs[0].as_ref(), PKI.device_cert.as_slice());
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "{head}"
    );
}

/// lmgw's `certificate_expired` alert for a lapsed device certificate (device link §5.3) reads as
/// lmgw-client's lapse, but Kai says it in its own words, without "pair again", and acts on it
/// nowhere (the security review's B1): an alert proves nothing about who sent it.
#[tokio::test]
async fn an_alert_about_the_certificate_is_said_without_pair_again() {
    let g = Gateway::bind().await;
    let mut p = PKI.paired(&[&g.url]);
    p.credentials.certificate =
        test_pki::device_cert(&PKI.ca, &PKI.device_key.public_key().unwrap(), (2020, 2021));
    let tls = Tls::paired(&p.credentials, &PKI.device_key, WAIT).unwrap();
    assert!(!tls.lapsed(crate::pairing::now_ms()), "Kai's date holds it");
    let (r, _) = tokio::join!(
        tls.http().get(format!("{}/v1/models", g.url)).send(),
        g.accept()
    );
    let e = r.unwrap_err();
    assert_eq!(refusal(&e), Some(TlsRefusal::CertificateLapsed));
    assert_eq!(
        certificate_alert(&e).as_deref(),
        Some("certificate_expired")
    );
    for said in [why(&e), unreachable(&e)] {
        assert!(
            said.contains("certificate_expired") && said.contains("tries again"),
            "{said}"
        );
        assert!(
            !said.contains("pair again") && !said.contains("pair the device"),
            "{said}"
        );
    }
}

/// Kai's own date decides a lapse: from `not_after` on, not before.
#[test]
fn the_date_decides_a_lapse() {
    let mut p = PKI.paired(&["https://127.0.0.1:1"]);
    p.credentials.not_after_ms = 2_000;
    let tls = Tls::paired(&p.credentials, &PKI.device_key, WAIT).unwrap();
    assert!(!tls.lapsed(1_999));
    assert!(tls.lapsed(2_000));
    assert_eq!(tls.not_after_ms(), Some(2_000));
    let enrolment = Tls::enrolment(test_pki::fp(&PKI.ca), WAIT).unwrap();
    assert!(!enrolment.lapsed(i64::MAX), "enrolment's has no date");
}

/// TLS's names of the alerts Kai says.
#[test]
fn alerts_go_by_tls_names() {
    use rustls::AlertDescription as A;
    for (d, name) in [
        (A::BadCertificate, "bad_certificate"),
        (A::UnknownCA, "unknown_ca"),
        (A::CertificateExpired, "certificate_expired"),
        (A::CertificateRequired, "certificate_required"),
    ] {
        assert_eq!(alert_name(d), name);
    }
}

/// A stored certificate that is not the key's is refused when the configuration is built: every
/// handshake would fail.
#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let other = Pki::new("lmgw devices other");
    let p = PKI.paired(&["https://127.0.0.1:1"]);
    let e = Tls::paired(&p.credentials, &other.device_key, WAIT).unwrap_err();
    assert!(e.contains("does not hold together"), "{e}");
}

/// Device link §6.3's rule: the next URL a stagger after the previous one unless that one's
/// connection is up or it ended, at once after a failure; the first answer wins; all failed, each
/// URL's error in the list's order.
#[tokio::test(start_paused = true)]
async fn the_urls_are_raced_in_order_with_a_stagger() {
    let urls = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let stagger = Duration::from_millis(250);
    // The first hangs before its connection is up: the second starts after the stagger and wins.
    let t0 = tokio::time::Instant::now();
    let won = race(urls(&["a", "b"]), stagger, |u: &str, _| {
        let u = u.to_owned();
        async move {
            if u == "a" {
                std::future::pending::<()>().await;
            }
            Ok::<_, String>(u)
        }
    })
    .await;
    assert_eq!(won, Ok(("b".to_owned(), "b".to_owned())));
    assert_eq!(t0.elapsed(), stagger);
    // The first fails at once: the second starts at once.
    let t0 = tokio::time::Instant::now();
    let won = race(urls(&["a", "b"]), stagger, |u: &str, _| {
        let u = u.to_owned();
        async move {
            if u == "a" {
                Err("refused".to_owned())
            } else {
                Ok(u)
            }
        }
    })
    .await;
    assert_eq!(won, Ok(("b".to_owned(), "b".to_owned())));
    assert_eq!(t0.elapsed(), Duration::ZERO);
    // A slow first answer still wins when it comes before the second's.
    let won = race(urls(&["a", "b"]), stagger, |u: &str, _| {
        let u = u.to_owned();
        async move {
            let ms = if u == "a" { 300 } else { 500 };
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok::<_, String>(u)
        }
    })
    .await;
    assert_eq!(won.unwrap().0, "a");
    // All fail: each URL's error, in the list's order.
    let lost = race(urls(&["a", "b", "c"]), stagger, |u: &str, _| {
        let u = u.to_owned();
        async move {
            let ms = if u == "a" { 900 } else { 10 };
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Err::<(), _>(format!("{u} failed"))
        }
    })
    .await;
    assert_eq!(
        lost.unwrap_err(),
        [
            ("a".to_owned(), "a failed".to_owned()),
            ("b".to_owned(), "b failed".to_owned()),
            ("c".to_owned(), "c failed".to_owned()),
        ]
    );
}

/// §6.3: a URL whose connection is up is not raced by the next one however slowly it answers; one
/// that fails after its connection was up starts the next at once.
#[tokio::test(start_paused = true)]
async fn a_url_whose_connection_is_up_is_not_raced() {
    let urls = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let stagger = Duration::from_millis(250);
    let started = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = started.clone();
    let t0 = tokio::time::Instant::now();
    let won = race(urls(&["a", "b"]), stagger, move |u: &str, answered| {
        seen.lock().unwrap().push(u.to_owned());
        let u = u.to_owned();
        async move {
            answered.up();
            tokio::time::sleep(stagger * 4).await;
            Ok::<_, String>(u)
        }
    })
    .await;
    assert_eq!(won.unwrap().0, "a");
    assert_eq!(t0.elapsed(), stagger * 4);
    assert_eq!(*started.lock().unwrap(), ["a"], "the second never started");
    // Up, then failed: the second starts then.
    let t0 = tokio::time::Instant::now();
    let won = race(urls(&["a", "b"]), stagger, |u: &str, answered: Answered| {
        let u = u.to_owned();
        async move {
            if u == "a" {
                answered.up();
                tokio::time::sleep(stagger * 2).await;
                return Err("reset".to_owned());
            }
            Ok(u)
        }
    })
    .await;
    assert_eq!(won.unwrap().0, "b");
    assert_eq!(t0.elapsed(), stagger * 2);
}

/// The per-try client says when its TCP and TLS are up: a gateway reached but slow to answer is
/// not raced by the next URL, which sees no connection.
#[tokio::test]
async fn a_reached_gateway_that_answers_slowly_is_not_raced() {
    let slow = Gateway::bind().await;
    let next = Gateway::bind().await;
    let creds = PKI.creds(&slow.url);
    let urls = vec![slow.url.clone(), next.url.clone()];
    let stagger = Duration::from_millis(100);
    let raced = race(urls, stagger, |base: &str, answered| {
        let http = creds.tls.racing(answered);
        let url = format!("{base}/v1/models");
        async move { http?.get(url).send().await }
    });
    let server = async {
        let (mut s, _) = slow.accept().await.unwrap();
        let _ = test_pki::read_request(&mut s).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        s.write_all(test_pki::answer("200 OK", "{}").as_bytes())
            .await
            .unwrap();
        s
    };
    let watch_next = tokio::time::timeout(Duration::from_millis(900), next.accept());
    let (won, _s, seen) = tokio::join!(raced, server, watch_next);
    let (url, resp) = won.unwrap();
    assert_eq!(url, slow.url);
    assert_eq!(resp.status(), 200);
    assert!(seen.is_err(), "the next URL was raced");
}

#[test]
fn the_reach_goes_to_the_url_that_last_connected() {
    let urls: Urls = vec![
        "https://192.0.2.1:8788".to_owned(),
        "https://198.51.100.20:8788".to_owned(),
    ]
    .try_into()
    .unwrap();
    let r = Reach::new(urls);
    assert_eq!(r.base(), "https://192.0.2.1:8788");
    let shared = r.clone();
    shared.reached("https://198.51.100.20:8788");
    assert_eq!(r.base(), "https://198.51.100.20:8788", "clones share it");
    assert_eq!(
        r.order(),
        ["https://198.51.100.20:8788", "https://192.0.2.1:8788"]
    );
    assert_eq!(
        r.urls(),
        ["https://192.0.2.1:8788", "https://198.51.100.20:8788"]
    );
}
