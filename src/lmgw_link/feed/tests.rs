//! The feed task against a scripted HTTP server behind the tests' device listener on loopback
//! (`crate::lmgw_link::test_pki`; real time, waits of a few seconds: the backoff starts at 1 s), and
//! the mapping of its events.

use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::server::TlsStream;

use super::*;
use crate::lmgw_link::AdminLevel;
use crate::lmgw_link::test_pki::{self, Gateway};

const SSE_HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\n\r\n";

fn hello(name: &str, keepalive_s: u32) -> String {
    let h = json!({"epoch": "e1", "cursor": "e1:4:aa", "keepalive_s": keepalive_s,
        "retention_days": 7, "principal": {"kind": "device", "name": name},
        "hosts_label": "kai", "self_admin": "read_only",
        "hold": {"active": false, "fallback_alias": null},
        "voice": [{"thread_id": 812, "by": "device 'phone'"}],
        "turns": [{"thread_id": 812, "by": format!("device '{name}'"), "voice": true}]});
    format!("event: hello\ndata: {h}\n\n")
}

fn refusal(status: &str, code: &str, message: &str) -> String {
    let body = json!({"code": code, "message": message}).to_string();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Accepts one connection and reads its request's head.
async fn accept(l: &Gateway) -> (TlsStream<TcpStream>, String) {
    let (mut s, _) = tokio::time::timeout(Duration::from_secs(10), l.accept())
        .await
        .expect("a connection")
        .unwrap();
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0, "the request ended early");
        head.extend_from_slice(&buf[..n]);
    }
    (s, String::from_utf8(head).unwrap())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

struct Rig {
    link: FeedLink,
    events: mpsc::UnboundedReceiver<LinkEvent>,
    listener: Gateway,
}

async fn rig(retry_max_s: u64) -> Rig {
    let listener = Gateway::bind().await;
    let link = start(&[&listener.url], retry_max_s);
    let (link, events) = link;
    Rig {
        link,
        events,
        listener,
    }
}

/// The feed of a pairing with the tests' gateway at `urls`.
fn start(urls: &[&str], retry_max_s: u64) -> (FeedLink, mpsc::UnboundedReceiver<LinkEvent>) {
    let mut link = FeedLink::new(FeedSettings {
        retry_max: Duration::from_secs(retry_max_s),
        timeout: Duration::from_secs(2),
        stagger: Duration::from_millis(250),
    });
    let (tx, events) = mpsc::unbounded_channel();
    let p = test_pki::PKI.paired(urls);
    let creds = Credentials {
        reach: crate::lmgw_link::tls::Reach::new(p.credentials.urls.clone()),
        tls: test_pki::PKI.creds(urls[0]).tls,
    };
    link.start(creds, tx);
    (link, events)
}

/// The next event but what a `hello` says about the device ([`LinkEvent::Device`], which
/// [`hellos_device_facts_come_right_before_connected`] checks).
async fn next(events: &mut mpsc::UnboundedReceiver<LinkEvent>) -> LinkEvent {
    loop {
        match next_any(events).await {
            LinkEvent::Device(_) => continue,
            e => return e,
        }
    }
}

async fn next_any(events: &mut mpsc::UnboundedReceiver<LinkEvent>) -> LinkEvent {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("a link event")
        .expect("the link runs")
}

/// lmgw's device link §7.2: a device's `hello` says what lmgw holds about it (its URLs, its
/// certificate's dates, its dashboard), which the feed passes on as `lmgw-client`'s reader kept it
/// (the `https` URLs, each once; the dates in unix milliseconds), right before the connection is
/// said; a `hello` without them says none.
#[tokio::test]
async fn hellos_device_facts_come_right_before_connected() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    let mut h: serde_json::Value = serde_json::from_str(
        hello("desktop", 15)
            .trim_start_matches("event: hello\ndata: ")
            .trim_end(),
    )
    .unwrap();
    h["urls"] = json!([
        "https://192.0.2.1:8788",
        "http://192.0.2.1:8787",
        "https://192.0.2.1:8788/",
        "https://198.51.100.20:8788"
    ]);
    h["cert"] =
        json!({"not_after": "2027-10-14T14:03:11Z", "notice_after": "2027-06-14T14:03:11Z"});
    h["dashboard_url"] = json!("http://198.51.100.20:8787/");
    s.write_all(format!("event: hello\ndata: {h}\n\n").as_bytes())
        .await
        .unwrap();
    let LinkEvent::Device(facts) = next_any(&mut r.events).await else {
        panic!("the device's facts first");
    };
    assert_eq!(
        facts.urls.as_ref().map(|u| u.as_slice().to_vec()),
        Some(vec![
            "https://192.0.2.1:8788".to_owned(),
            "https://198.51.100.20:8788".to_owned()
        ]),
        "the https ones, each once"
    );
    assert_eq!(
        facts.cert,
        Some(lmgw_client::requests::CertDates {
            not_after_ms: 1_823_522_591_000,
            notice_after_ms: 1_812_981_791_000,
        })
    );
    assert_eq!(
        facts.dashboard.as_deref(),
        Some("http://198.51.100.20:8787/")
    );
    assert!(matches!(
        next_any(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    // The next connection's hello carries none: none said.
    drop(s);
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    loop {
        match next_any(&mut r.events).await {
            LinkEvent::Device(facts) => {
                assert_eq!(
                    facts,
                    DeviceFacts {
                        urls: None,
                        cert: None,
                        dashboard: None
                    }
                );
                break;
            }
            LinkEvent::Down { .. } | LinkEvent::Fact(_) => {}
            other => panic!("{other:?}"),
        }
    }
    r.link.stop();
}

fn down(e: LinkEvent) -> String {
    match e {
        LinkEvent::Down { note, .. } => note,
        other => panic!("not Down: {other:?}"),
    }
}

fn by(name: &str, own: bool) -> By {
    By {
        name: name.into(),
        own,
    }
}

#[tokio::test]
async fn hello_names_the_device_marks_kais_own_and_a_drop_resumes_with_the_cursor() {
    let mut r = rig(30).await;
    let (mut s, head) = accept(&r.listener).await;
    assert!(
        head.starts_with("GET /chat/api/feed HTTP/1.1\r\n"),
        "{head}"
    );
    assert_eq!(
        header(&head, "authorization"),
        None,
        "the device's certificate authenticates it"
    );
    assert_eq!(header(&head, "accept"), Some("text/event-stream"));
    assert_eq!(
        header(&head, "last-event-id"),
        None,
        "a new run starts from now"
    );
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: Some("kai".into()),
            admin: AdminLevel::ReadOnly,
        }
    );
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Fact(FeedFact::Live {
            voice: vec![(812, by("device 'phone'", false))],
            turns: vec![(812, by("device 'desktop'", true))],
        }),
        "Kai's own turn is marked own"
    );
    s.write_all(
        b"id: e1:9:bb\nevent: turn.started\ndata: {\"thread_id\":812,\"by\":\"the dashboard\",\
          \"voice\":false}\n\n: keep-alive\n\nevent: hold\ndata: {\"active\":true,\
          \"fallback_alias\":null}\n\n",
    )
    .await
    .unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Fact(FeedFact::TurnStarted {
            thread_id: 812,
            by: by("the dashboard", false)
        })
    );
    assert_eq!(next(&mut r.events).await, LinkEvent::Hold { active: true });
    // lmgw ends the stream: TLS's close_notify, then the connection closes.
    s.shutdown().await.unwrap();
    drop(s);
    let note = down(next(&mut r.events).await);
    assert!(
        note.contains("lmgw closed the feed")
            && note.contains("next try in 1 s")
            && note.contains("lmgw.retry_max_s"),
        "{note}"
    );
    let (_s, head) = accept(&r.listener).await;
    assert_eq!(
        header(&head, "last-event-id"),
        Some("e1:9:bb"),
        "resumed from the last record"
    );
    r.link.stop();
}

#[tokio::test]
async fn a_refusal_is_retried_and_a_401_is_a_revocation_after_which_the_link_stops() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(refusal("404 Not Found", "not_found", "no such route").as_bytes())
        .await
        .unwrap();
    let note = down(next(&mut r.events).await);
    assert!(
        note.contains("HTTP 404, not_found: no such route") && note.contains("next try in 1 s"),
        "{note}"
    );
    let (mut s, _) = accept(&r.listener).await;
    let words = "device 'desktop' is disabled — enable it on Usage → Keys";
    s.write_all(refusal("401 Unauthorized", "device_disabled", words).as_bytes())
        .await
        .unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Revoked {
            reason: words.into(),
            loss: Some(KeyLoss::Disabled),
        },
        "the server's words, verbatim; a disabled device is enabled again (review KAI-3 K3-6)"
    );
    // It stops retrying.
    let again = tokio::time::timeout(Duration::from_millis(2500), r.listener.accept()).await;
    assert!(again.is_err(), "the link connected again after a 401");
    assert!(r.events.recv().await.is_none(), "the run ended");
}

#[tokio::test]
async fn the_revoked_event_ends_the_link() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    next(&mut r.events).await;
    let msg = "the key of device 'desktop' was replaced — pair it again";
    let rec = json!({"reason": "rotated", "message": msg, "kind": "key_unknown"});
    s.write_all(format!("event: revoked\ndata: {rec}\n\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Revoked {
            reason: msg.into(),
            loss: Some(KeyLoss::PairAgain)
        }
    );
    let again = tokio::time::timeout(Duration::from_millis(1500), r.listener.accept()).await;
    assert!(
        again.is_err(),
        "the link connected again after a revocation"
    );
}

/// api §1.6: a `revoked` of a kind this build does not know, or with none (an older gateway's),
/// goes to the core with lmgw's sentence and no loss (the core asks lmgw once, §3.3). The feed
/// then tries again after its backoff, said in a `Down`, and a `hello` before such an end never
/// starts the backoff over (review KAI-4 K4-7): a gateway that ends every feed so is met after 1,
/// then 2 s.
#[tokio::test]
async fn a_revoked_event_of_an_unknown_kind_is_tried_again_after_the_backoff() {
    for rec in [
        json!({"reason": "paused", "message": "device 'desktop' was paused", "kind": "key_paused"}),
        json!({"reason": "disabled", "message": "device 'desktop' was paused"}),
    ] {
        let mut r = rig(30).await;
        let mut waits = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = accept(&r.listener).await;
            s.write_all(SSE_HEAD).await.unwrap();
            s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
            assert!(matches!(
                next(&mut r.events).await,
                LinkEvent::Connected { .. }
            ));
            next(&mut r.events).await;
            s.write_all(format!("event: revoked\ndata: {rec}\n\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(
                next(&mut r.events).await,
                LinkEvent::Revoked {
                    reason: "device 'desktop' was paused".into(),
                    loss: None
                },
                "{rec}: its words are not read, not even an old reason's"
            );
            let note = down(next(&mut r.events).await);
            assert!(
                note.starts_with("lmgw ended the feed: device 'desktop' was paused; next try in"),
                "{rec}: {note}"
            );
            let s = note.split("next try in ").nth(1).unwrap();
            waits.push(s.split(' ').next().unwrap().to_owned());
            let early = tokio::time::timeout(Duration::from_millis(700), r.listener.accept()).await;
            assert!(early.is_err(), "{rec}: tried again before its backoff");
        }
        assert_eq!(waits, ["1", "2"], "{rec}: the hello did not start it over");
        r.link.stop();
    }
}

#[tokio::test]
async fn silence_for_two_keepalive_intervals_is_a_dead_link_and_the_backoff_starts_over() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 1).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    next(&mut r.events).await;
    // A keep-alive comment is a sign of life (1 s into the 2 s: a wide margin under a loaded test
    // run, review KAI-2 K2-25).
    tokio::time::sleep(Duration::from_millis(1000)).await;
    s.write_all(b": keep-alive\n\n").await.unwrap();
    let t0 = tokio::time::Instant::now();
    let ev = next(&mut r.events).await;
    let after = t0.elapsed();
    assert!(
        matches!(ev, LinkEvent::Down { dead: true, .. }),
        "the dead-link verdict, which also ends a bound voice session: {ev:?}"
    );
    let note = down(ev);
    assert!(
        note.contains("sent nothing for 2 s (two hello.keepalive_s intervals)")
            && note.contains("next try in 1 s"),
        "{note}"
    );
    assert!(
        after >= Duration::from_millis(1800),
        "the keep-alive reset the timer: {after:?}"
    );
    let (_s, _) = accept(&r.listener).await;
    r.link.stop();
}

/// A stream that sends nothing at all after its answer is bounded by `lmgw.timeout_s`, and says so.
#[tokio::test]
async fn no_byte_within_lmgw_timeout_s_is_a_drop() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    let note = down(next(&mut r.events).await);
    assert!(
        note.contains("lmgw sent no hello, nor any byte, within 2 s (lmgw.timeout_s)")
            && note.contains("next try in 1 s"),
        "{note}"
    );
    r.link.stop();
}

/// lmgw a50141d (api §2.1): a feed opened while a level change goes live begins with a keep-alive
/// comment, and `hello` follows within one keep-alive interval, which may be longer than
/// `lmgw.timeout_s` (15 s against 10 by default). Every byte is progress: gaps of 2.5 s, past
/// `lmgw.timeout_s` (2 s here) but within two intervals (with none stated yet, `lmgw.timeout_s`
/// stands in: 4 s), keep the request alive for 5 s, and the `hello` after them brings it up.
#[tokio::test]
async fn keep_alive_comments_before_hello_keep_the_feed_alive() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    let t0 = tokio::time::Instant::now();
    s.write_all(b": keep-alive\n\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    s.write_all(b": keep-alive\n\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: Some("kai".into()),
            admin: AdminLevel::ReadOnly,
        },
        "up, with no drop before it"
    );
    assert!(t0.elapsed() >= Duration::from_secs(5), "{:?}", t0.elapsed());
    next(&mut r.events).await;
    r.link.stop();
}

/// A stream that began but sends nothing more is still bounded: two keep-alive intervals, with
/// `lmgw.timeout_s` standing in until lmgw states one, then the interval of its last `hello`.
#[tokio::test]
async fn a_feed_that_began_without_hello_and_falls_silent_is_dropped_after_two_intervals() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(b": keep-alive\n\n").await.unwrap();
    let t0 = tokio::time::Instant::now();
    let note = down(next(&mut r.events).await);
    let after = t0.elapsed();
    assert!(
        note.contains(
            "lmgw began the feed but sent no hello and nothing else for 4 s (two keep-alive \
             intervals of 2 s: lmgw.timeout_s, until lmgw states its keep-alive in a hello)"
        ),
        "{note}"
    );
    assert!(after >= Duration::from_millis(3800), "{after:?}");
    // The next try states 1 s in its hello, then lmgw closes it; the try after that began, and its
    // silence is two of lmgw's stated intervals.
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 1).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    next(&mut r.events).await;
    // lmgw ends the stream: TLS's close_notify, then the connection closes.
    s.shutdown().await.unwrap();
    drop(s);
    let note = down(next(&mut r.events).await);
    assert!(note.contains("lmgw closed the feed"), "{note}");
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(b": keep-alive\n\n").await.unwrap();
    let note = down(next(&mut r.events).await);
    assert!(
        note.contains(
            "sent no hello and nothing else for 2 s (two keep-alive intervals of 1 s: \
             hello.keepalive_s, as lmgw last stated it)"
        ),
        "{note}"
    );
    r.link.stop();
}

#[tokio::test]
async fn the_backoff_doubles_up_to_lmgw_retry_max_s() {
    // A port nothing listens on: every try is refused at once.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}", l.local_addr().unwrap());
    drop(l);
    let (mut link, mut events) = start(&[&url], 2);
    let mut waits = Vec::new();
    for _ in 0..3 {
        let note = down(next(&mut events).await);
        assert!(note.contains("lmgw is not reachable"), "{note}");
        let s = note.split("next try in ").nth(1).unwrap();
        waits.push(s.split(' ').next().unwrap().to_owned());
    }
    assert_eq!(waits, ["1", "2", "2"]);
    link.stop();
}

/// Device link §3.5: lmgw's 403 `device_unconfirmed` is said (the tray shows Kai's own code) and
/// asked again on the backoff; once the owner confirmed, the feed comes up.
#[tokio::test]
async fn an_unconfirmed_device_is_said_and_asked_again() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    let words = "confirm 7F3A-91C2 on lmgw's Devices card";
    s.write_all(refusal("403 Forbidden", "device_unconfirmed", words).as_bytes())
        .await
        .unwrap();
    match next(&mut r.events).await {
        LinkEvent::Unconfirmed { note } => {
            assert!(note.contains("Kai asks again in 1 s"), "{note}")
        }
        other => panic!("not Unconfirmed: {other:?}"),
    }
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    r.link.stop();
}

/// The device link's refusals of the device (device link §4.7) end the link: a lapsed
/// certificate, no certificate, a credential beside it.
#[tokio::test]
async fn the_device_listeners_refusals_end_the_link() {
    for (status, code, loss) in [
        (
            "401 Unauthorized",
            "device_certificate_expired",
            KeyLoss::Lapsed,
        ),
        (
            "401 Unauthorized",
            "client_certificate_required",
            KeyLoss::PairAgain,
        ),
        (
            "400 Bad Request",
            "credential_on_device_listener",
            KeyLoss::PairAgain,
        ),
    ] {
        let mut r = rig(30).await;
        let (mut s, _) = accept(&r.listener).await;
        s.write_all(refusal(status, code, "lmgw's words").as_bytes())
            .await
            .unwrap();
        assert_eq!(
            next(&mut r.events).await,
            LinkEvent::Revoked {
                reason: "lmgw's words".into(),
                loss: Some(loss),
            },
            "{code}"
        );
    }
}

/// The feed of a pairing with the tests' gateway at `url`, whose stored certificate is `cert` and
/// whose `not_after` by Kai's date is `not_after_ms`.
fn start_with(
    url: &str,
    cert: Option<Vec<u8>>,
    not_after_ms: i64,
) -> (FeedLink, mpsc::UnboundedReceiver<LinkEvent>) {
    let mut p = test_pki::PKI.paired(&[url]);
    if let Some(c) = cert {
        p.credentials.certificate = c;
    }
    p.credentials.not_after_ms = not_after_ms;
    let creds = Credentials {
        reach: crate::lmgw_link::tls::Reach::new(p.credentials.urls.clone()),
        tls: crate::lmgw_link::tls::Tls::paired(
            &p.credentials,
            &test_pki::PKI.device_key,
            Duration::from_secs(5),
        )
        .unwrap(),
    };
    let mut link = FeedLink::new(FeedSettings {
        retry_max: Duration::from_secs(30),
        timeout: Duration::from_secs(5),
        stagger: Duration::from_millis(250),
    });
    let (tx, events) = mpsc::unbounded_channel();
    link.start(creds, tx);
    (link, events)
}

/// The security review's B1: a port squatter's plaintext alert right after the ClientHello, before
/// it proved anything, never ends the feed: each is said, by TLS's name, and the feed tries again
/// on its backoff.
#[tokio::test]
async fn a_plaintext_alert_never_ends_the_feed() {
    let squatter = test_pki::Squatter::bind().await;
    let (mut link, mut events) = start(&[&squatter.url], 1);
    for (alert, name) in [
        (test_pki::BAD_CERTIFICATE, "bad_certificate"),
        (test_pki::UNKNOWN_CA, "unknown_ca"),
        (test_pki::CERTIFICATE_EXPIRED, "certificate_expired"),
    ] {
        tokio::time::timeout(Duration::from_secs(10), squatter.answer(alert))
            .await
            .expect("the feed tried again");
        let note = down(next(&mut events).await);
        assert!(
            note.contains(name) && note.contains("tries again") && !note.contains("pair again"),
            "{note}"
        );
        // A lapse an alert claims while Kai's date holds the certificate is said as the clocks
        // disagreeing.
        if alert == test_pki::CERTIFICATE_EXPIRED {
            assert!(note.contains("holds until 2100-01-01"), "{note}");
        }
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        squatter.answer(test_pki::BAD_CERTIFICATE),
    )
    .await
    .expect("the feed still tries");
    link.stop();
}

/// lmgw's own `certificate_expired` alert for a certificate past its date (device link §5.3) while
/// Kai's date still holds it: said, with Kai's date, and tried again (the clocks disagree).
#[tokio::test]
async fn an_alert_against_kais_date_is_said_and_tried_again() {
    let g = Gateway::bind().await;
    let old = test_pki::device_cert(
        &test_pki::PKI.ca,
        &test_pki::PKI.device_key.public_key().unwrap(),
        (2020, 2021),
    );
    let (mut link, mut events) = start_with(&g.url, Some(old), 4_102_444_800_000);
    let _ = g.accept().await;
    let note = down(next(&mut events).await);
    assert!(
        note.contains("certificate_expired") && note.contains("holds until 2100-01-01"),
        "{note}"
    );
    let again = tokio::time::timeout(Duration::from_secs(10), g.accept()).await;
    assert!(again.is_ok(), "the feed tried again");
    link.stop();
}

/// A certificate past its date by Kai's own clock (device link §5.3) ends the feed before it
/// tries: lapsed, with the date.
#[tokio::test]
async fn a_certificate_lapsed_by_kais_date_ends_the_feed_untried() {
    let g = Gateway::bind().await;
    let (_link, mut events) = start_with(&g.url, None, 86_400_000);
    match next(&mut events).await {
        LinkEvent::Revoked { reason, loss } => {
            assert_eq!(loss, Some(KeyLoss::Lapsed));
            assert!(reason.contains("1970-01-02"), "{reason}");
        }
        other => panic!("not Revoked: {other:?}"),
    }
    let tried = tokio::time::timeout(Duration::from_millis(500), g.accept()).await;
    assert!(tried.is_err(), "the feed tried a lapsed certificate");
}

/// Device link §6.3: with several URLs the feed tries the next when one does not answer, and every
/// other connection goes to the one that did.
#[tokio::test]
async fn the_feed_reaches_lmgw_at_the_url_that_answers() {
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_url = format!("https://{}", dead.local_addr().unwrap());
    drop(dead);
    let g = Gateway::bind().await;
    let p = test_pki::PKI.paired(&[&dead_url, &g.url]);
    let reach = crate::lmgw_link::tls::Reach::new(p.credentials.urls.clone());
    let creds = Credentials {
        reach: reach.clone(),
        tls: test_pki::PKI.creds(&g.url).tls,
    };
    assert_eq!(reach.base(), dead_url);
    let mut link = FeedLink::new(FeedSettings {
        retry_max: Duration::from_secs(30),
        timeout: Duration::from_secs(5),
        stagger: Duration::from_millis(250),
    });
    let (tx, mut events) = mpsc::unbounded_channel();
    link.start(creds, tx);
    let (mut s, _) = accept(&g).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut events).await,
        LinkEvent::Connected { .. }
    ));
    assert_eq!(reach.base(), g.url);
    link.stop();
}

#[test]
fn state_resync_turns_voice_and_folder_current() {
    let mut t = Translate::default();
    let h = hello("desktop", 15);
    let data = h.lines().nth(1).unwrap().strip_prefix("data: ").unwrap();
    let Step::Events(evs) = t.event(FeedEvent::parse("hello", data).unwrap()) else {
        panic!()
    };
    assert_eq!(evs.len(), 2);
    // A `state` says the device's admin-tools level as it is now (lmgw sends one after every move).
    let state = json!({"self_admin": "full", "hold": {"active": true, "fallback_alias": "cloud/x"},
        "voice": [], "turns": [{"thread_id": 5, "by": "device 'desktop'", "voice": false}],
        "reason": null});
    assert_eq!(
        t.event(FeedEvent::parse("state", &state.to_string()).unwrap()),
        Step::Events(vec![
            LinkEvent::Hold { active: true },
            LinkEvent::AdminTools(AdminLevel::Full),
            LinkEvent::Fact(FeedFact::Live {
                voice: vec![],
                turns: vec![(5, by("device 'desktop'", true))]
            })
        ])
    );
    assert_eq!(
        t.event(FeedEvent::parse("resync", r#"{"reason":"restored"}"#).unwrap()),
        Step::Events(vec![LinkEvent::Fact(FeedFact::Resync)])
    );
    assert_eq!(
        t.event(
            FeedEvent::parse(
                "turn.done",
                r#"{"thread_id":5,"message_id":null,"saved":false,"code":"superseded"}"#
            )
            .unwrap()
        ),
        Step::Events(vec![LinkEvent::Fact(FeedFact::TurnDone { thread_id: 5 })])
    );
    let ended = json!({"thread_id": 5, "by": "device 'desktop'", "reason": "taken_over",
        "taken_over_by": "device 'phone'"});
    assert_eq!(
        t.event(FeedEvent::parse("voice.ended", &ended.to_string()).unwrap()),
        Step::Events(vec![LinkEvent::Fact(FeedFact::VoiceEnded {
            thread_id: 5,
            by: by("device 'desktop'", true)
        })])
    );
    // §4: the folder events go to the core, which follows its own folder's.
    let fc = json!({"folder_id": 3, "thread_id": 9, "previous_thread_id": 5, "reason": "idle",
        "by": null});
    assert_eq!(
        t.event(FeedEvent::parse("folder.current", &fc.to_string()).unwrap()),
        Step::Events(vec![LinkEvent::Folder(FolderEvent::Current {
            folder_id: 3,
            thread_id: Some(9),
            previous: Some(5),
            reason: "idle".into()
        })])
    );
    assert_eq!(
        t.event(
            FeedEvent::parse(
                "revoked",
                r#"{"reason":"expired","message":"","kind":"key_expired"}"#
            )
            .unwrap()
        ),
        Step::Revoked("expired".into(), Some(KeyLoss::Expired)),
        "the reason when the message is empty; an expired key's expiry is moved (lmgw 7d6d18c)"
    );
    // What the event asks is its `kind` (api §1.6), each of lmgw's; never its words.
    for (kind, loss) in [
        ("device_disabled", Some(KeyLoss::Disabled)),
        ("key_expired", Some(KeyLoss::Expired)),
        ("key_unknown", Some(KeyLoss::PairAgain)),
        ("revoked", Some(KeyLoss::PairAgain)),
        ("key_paused", None),
    ] {
        let rec = json!({"reason": "disabled", "message": "device 'desktop' was disabled",
            "kind": kind});
        assert_eq!(
            t.event(FeedEvent::parse("revoked", &rec.to_string()).unwrap()),
            Step::Revoked("device 'desktop' was disabled".into(), loss),
            "{kind}"
        );
    }
    // An event without a kind (an older gateway's) is of an unknown kind, whatever its reason.
    assert_eq!(
        t.event(
            FeedEvent::parse(
                "revoked",
                r#"{"reason":"disabled","message":"device 'desktop' was disabled"}"#
            )
            .unwrap()
        ),
        Step::Revoked("device 'desktop' was disabled".into(), None),
        "no kind: lmgw's sentence, and Kai asks lmgw once (§3.3)"
    );
}

#[test]
fn folder_and_thread_events_become_the_conversations_facts() {
    let mut t = Translate::default();
    let ev =
        |name: &str, data: serde_json::Value| FeedEvent::parse(name, &data.to_string()).unwrap();
    let folder = json!({"id": 3, "name": "Kai", "sort": 1,
        "defaults": {"model_alias": "kai-chat"}, "ongoing": {"idle_minutes": 120,
        "current_thread_id": 9}, "archive_days": null, "purge_days": 365, "by": "the dashboard"});
    assert_eq!(
        t.event(ev("folder.updated", folder)),
        Step::Events(vec![LinkEvent::Folder(FolderEvent::Changed(FolderInfo {
            id: 3,
            name: "Kai".into(),
            model: Some("kai-chat".into()),
            idle_minutes: Some(120),
            archive_days: None,
            purge_days: Some(365),
            tool_labels: Vec::new(),
            profile_id: None,
        }))])
    );
    let plain =
        json!({"id": 4, "name": "Kai", "defaults": {}, "ongoing": null, "by": "device 'desktop'"});
    let Step::Events(evs) = t.event(ev("folder.created", plain)) else {
        panic!()
    };
    assert!(
        matches!(&evs[..], [LinkEvent::Folder(FolderEvent::Changed(f))] if f.id == 4 && f.idle_minutes.is_none() && f.model.is_none())
    );
    for (name, data) in [
        (
            "folder.deleted",
            json!({"folder_id": 3, "deleted": true, "by": "the dashboard"}),
        ),
        (
            "folder.updated",
            json!({"folder_id": 3, "deleted": true, "by": null}),
        ),
    ] {
        assert_eq!(
            t.event(ev(name, data)),
            Step::Events(vec![LinkEvent::Folder(FolderEvent::Gone(3))]),
            "{name}"
        );
    }
    let thread = json!({"id": 9, "title": "Morgen", "archived_at": null, "folder_id": 3,
        "last_message_at": 1760000000, "profile_id": 2, "by": null});
    assert_eq!(
        t.event(ev("thread.updated", thread)),
        Step::Events(vec![
            LinkEvent::Profile(ProfileEvent::Thread {
                thread_id: 9,
                profile_id: Some(2),
            }),
            LinkEvent::Fact(FeedFact::ThreadChanged {
                thread_id: 9,
                title: "Morgen".into(),
                archived: false,
                last_message_at: Some(1_760_000_000),
            }),
        ])
    );
    let archived = json!({"id": 9, "title": "Morgen", "archived_at": "2026-10-07 10:00:00",
        "folder_id": null, "by": "the dashboard"});
    assert_eq!(
        t.event(ev("thread.updated", archived)),
        Step::Events(vec![
            LinkEvent::Profile(ProfileEvent::Thread {
                thread_id: 9,
                profile_id: None,
            }),
            LinkEvent::Fact(FeedFact::ThreadChanged {
                thread_id: 9,
                title: "Morgen".into(),
                archived: true,
                last_message_at: None,
            }),
        ])
    );
    for (name, data) in [
        (
            "thread.deleted",
            json!({"thread_id": 9, "deleted": true, "by": "the dashboard"}),
        ),
        (
            "thread.updated",
            json!({"thread_id": 9, "deleted": true, "by": null}),
        ),
    ] {
        assert_eq!(
            t.event(ev(name, data)),
            Step::Events(vec![LinkEvent::Fact(FeedFact::ThreadGone { thread_id: 9 })]),
            "{name}"
        );
    }
}

/// WP22: lmgw's `profile.*` events (and the list repeated after a `resync`) reach the core as
/// profile events, by id and name.
#[test]
fn profile_events_become_the_trays_profiles() {
    let mut t = Translate::default();
    let ev =
        |name: &str, data: serde_json::Value| FeedEvent::parse(name, &data.to_string()).unwrap();
    for (name, data, want) in [
        (
            "profile.created",
            json!({"id": 3, "name": "Calm", "by": null}),
            ProfileEvent::Upsert {
                id: 3,
                name: "Calm".into(),
            },
        ),
        (
            "profile.updated",
            json!({"id": 3, "name": "Calmer", "by": "the dashboard"}),
            ProfileEvent::Upsert {
                id: 3,
                name: "Calmer".into(),
            },
        ),
        (
            "profile.deleted",
            json!({"id": 3, "name": "Calmer", "by": "the dashboard"}),
            ProfileEvent::Removed { id: 3 },
        ),
    ] {
        assert_eq!(
            t.event(ev(name, data)),
            Step::Events(vec![LinkEvent::Profile(want)]),
            "{name}"
        );
    }
}

/// WP18: `device.revoked` of a device becomes the jobs' cancel; one that names no device is skipped.
#[test]
fn device_revoked_names_the_deleted_device() {
    let mut t = Translate::default();
    let ev =
        |name: &str, data: serde_json::Value| FeedEvent::parse(name, &data.to_string()).unwrap();
    assert_eq!(
        t.event(ev(
            "device.revoked",
            json!({"device": {"kind": "device", "name": "phone"}, "by": "the dashboard"})
        )),
        Step::Events(vec![LinkEvent::DeviceRevoked {
            name: "phone".into()
        }])
    );
    assert_eq!(
        t.event(ev(
            "device.revoked",
            json!({"device": {"kind": "owner", "name": "x"}, "by": null})
        )),
        Step::Events(Vec::new())
    );
}

/// WP19 and device link §7.3: the approval card's facts come from the reader's open set, never
/// from the `approval.*` events themselves. `hello`'s set is handed over whole at `caught_up`; a
/// request and its decision in the catch-up change nothing; after `caught_up` a request adds and a
/// decision withdraws.
#[test]
fn the_cards_facts_follow_the_feeds_open_set() {
    let call = |id: &str| {
        json!({"thread_id": 7, "message_id": 40, "approval_request_id": id,
               "server_label": "kai", "name": "clipboard_set", "arguments": "{\"text\":\"x\"}",
               "call_id": "call_7", "thread_title": "Kai"})
    };
    let decided = |id: &str| {
        json!({"thread_id": 7, "message_id": 40, "approval_request_id": id,
               "approve": false, "by": "device 'phone'", "not_run": true})
    };
    let requested = |id: &str| crate::approvals::Requested {
        thread_id: 7,
        request: lmgw_client::requests::approvals::ApprovalRequest {
            approval_request_id: id.into(),
            server_label: "kai".into(),
            name: "clipboard_set".into(),
            arguments: r#"{"text":"x"}"#.into(),
            call_id: Some("call_7".into()),
        },
    };
    let mut h: serde_json::Value = serde_json::from_str(
        hello("desktop", 15)
            .trim()
            .strip_prefix("event: hello\ndata: ")
            .unwrap(),
    )
    .unwrap();
    h["open_approvals"] = json!([call("mcpr_1")]);
    let stream = format!(
        "event: hello\ndata: {h}\n\n\
         id: e1:5:bb\nevent: approval.requested\ndata: {}\n\n\
         id: e1:6:cc\nevent: approval.decided\ndata: {}\n\n\
         event: caught_up\ndata: {}\n\n\
         id: e1:7:dd\nevent: approval.requested\ndata: {}\n\n\
         id: e1:8:ee\nevent: approval.decided\ndata: {}\n\n",
        call("mcpr_0"),
        decided("mcpr_0"),
        json!({"cursor": "e1:6:cc"}),
        call("mcpr_2"),
        decided("mcpr_1"),
    );
    let mut t = Translate::default();
    let mut facts = Vec::new();
    for item in FeedReader::new(None).push(stream.as_bytes()) {
        match item {
            FeedItem::Event(ev) => match t.event(ev) {
                Step::Events(evs) => facts.extend(evs.into_iter().filter(|e| {
                    matches!(e, LinkEvent::Fact(f) if matches!(f,
                            FeedFact::ApprovalsOpen(_)
                            | FeedFact::ApprovalRequested(_)
                            | FeedFact::ApprovalsWithdrawn(_)))
                })),
                Step::Revoked(..) => panic!("no revocation"),
            },
            FeedItem::Approvals(c) => facts.extend(Translate::approvals(c)),
            _ => {}
        }
    }
    assert_eq!(
        facts,
        vec![
            LinkEvent::Fact(FeedFact::ApprovalsOpen(vec![requested("mcpr_1")])),
            LinkEvent::Fact(FeedFact::ApprovalRequested(requested("mcpr_2"))),
            LinkEvent::Fact(FeedFact::ApprovalsWithdrawn(vec!["mcpr_1".into()])),
        ]
    );
    // Their Debug carries no argument.
    assert!(!format!("{facts:?}").contains("text"));
}

#[tokio::test]
async fn every_device_401_ends_the_link_with_its_message_and_what_it_asks() {
    for (code, words, loss) in [
        (
            "device_key_unknown",
            "this device key is no longer valid (rotated or deleted) — pair the device again",
            KeyLoss::PairAgain,
        ),
        // lmgw 7d6d18c: the expiry may be moved, so the key is kept.
        (
            "key_expired",
            "device 'desktop' expired on 2026-10-07",
            KeyLoss::Expired,
        ),
        // Review KAI-3 K3-9.
        (
            "device_disabled",
            "device 'desktop' is disabled — enable it on Usage → Keys",
            KeyLoss::Disabled,
        ),
    ] {
        let mut r = rig(30).await;
        let (mut s, _) = accept(&r.listener).await;
        s.write_all(refusal("401 Unauthorized", code, words).as_bytes())
            .await
            .unwrap();
        assert_eq!(
            next(&mut r.events).await,
            LinkEvent::Revoked {
                reason: words.into(),
                loss: Some(loss)
            },
            "{code}: lmgw's words, verbatim"
        );
        assert!(r.events.recv().await.is_none(), "{code}: the run ended");
    }
}

#[tokio::test]
async fn an_unreadable_event_reloads_as_for_resync_and_an_unknown_one_is_skipped() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    s.write_all(hello("desktop", 15).as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    next(&mut r.events).await;
    // A newer gateway's event, then a known one whose data does not read, then a hold.
    s.write_all(
        b"id: e1:5:cc\nevent: thread.pinned\ndata: {\"thread_id\":5}\n\n\
          id: e1:6:dd\nevent: thread.updated\ndata: [1]\n\n\
          event: hold\ndata: {\"active\":true,\"fallback_alias\":null}\n\n",
    )
    .await
    .unwrap();
    assert_eq!(
        next(&mut r.events).await,
        LinkEvent::Fact(FeedFact::Resync),
        "the unknown event is skipped; the unreadable one reloads"
    );
    assert_eq!(next(&mut r.events).await, LinkEvent::Hold { active: true });
    r.link.stop();
}

/// Review KAI-3 K3-3: lmgw sends its catch-up right behind `hello`; an unreadable event in the
/// same chunk still reloads.
#[tokio::test]
async fn an_unreadable_event_in_hellos_chunk_reloads_too() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    let chunk = format!(
        "{}id: e1:5:cc\nevent: thread.updated\ndata: [1]\n\n",
        hello("desktop", 15)
    );
    s.write_all(chunk.as_bytes()).await.unwrap();
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Connected { .. }
    ));
    assert!(matches!(
        next(&mut r.events).await,
        LinkEvent::Fact(FeedFact::Live { .. })
    ));
    assert_eq!(next(&mut r.events).await, LinkEvent::Fact(FeedFact::Resync));
    r.link.stop();
}

/// Review KAI-3 K3-4: a `hello` that does not read is said at once as version skew, not as a
/// missing hello after `lmgw.timeout_s`; the link retries.
#[tokio::test]
async fn an_unreadable_hello_says_so_at_once_and_retries() {
    let mut r = rig(30).await;
    let (mut s, _) = accept(&r.listener).await;
    s.write_all(SSE_HEAD).await.unwrap();
    let t0 = tokio::time::Instant::now();
    s.write_all(b"event: hello\ndata: [1]\n\n").await.unwrap();
    let note = down(next(&mut r.events).await);
    assert!(
        note.contains("lmgw's hello does not read (this Kai may be older than lmgw)")
            && note.contains("next try in 1 s"),
        "{note}"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "said before lmgw.timeout_s (2 s here)"
    );
    let (_s, _) = accept(&r.listener).await;
    r.link.stop();
}
