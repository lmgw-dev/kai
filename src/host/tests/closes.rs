//! How the link ends and what follows (api §5.1, §5.3, §1.6, §1.8): a takeover waits for the
//! feed, a revocation goes the feed's path, an oversize close is said and the link comes back, a
//! missing grant is said and tried again, Kai's own stop is a 1000.

use serde_json::json;

use super::*;
use crate::lmgw_link::KeyLoss;

/// 4000: another link of this device took over. Kai does not take it back by itself; the feed's
/// next `hello` links again.
#[tokio::test]
async fn a_takeover_waits_for_the_feed() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.close(4000, "another connection of device 'desktop' took over")
        .await;
    let s = r
        .status(|s| matches!(s, HostStatus::TakenOver { .. }))
        .await;
    let line = crate::host::tools_line(&s, Some("kai"));
    assert!(
        line.contains("took them over (another connection of device 'desktop' took over)"),
        "{line}"
    );
    r.fake.none_within(Duration::from_millis(2500)).await;
    r.host.feed_up("desktop", Some("kai"));
    let mut c = r.fake.accept().await;
    c.initialize().await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
}

/// 4003: a revocation, read by its kind like the feed's; the link stops.
#[tokio::test]
async fn a_revocation_goes_the_feeds_path() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.close(4003, "device_disabled: device 'desktop' was disabled")
        .await;
    loop {
        match r.event().await {
            HostEvent::Revoked { reason, loss } => {
                assert_eq!(reason, "device_disabled: device 'desktop' was disabled");
                assert_eq!(loss, Some(KeyLoss::Disabled));
                break;
            }
            HostEvent::Status(_) => {}
        }
    }
    r.fake.none_within(Duration::from_millis(2500)).await;
}

/// A 4003 of a kind this build does not know: said (the core says lmgw's sentence), and the link
/// tries again after its backoff.
#[tokio::test]
async fn an_unknown_revocation_kind_is_said_and_retried() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.close(4003, "key_paused: device 'desktop' was paused")
        .await;
    loop {
        if let HostEvent::Revoked { loss, .. } = r.event().await {
            assert_eq!(loss, None);
            break;
        }
    }
    r.fake.accept().await;
}

/// lmgw's size settings: an overrun closes the link with a reason naming the setting, which the
/// tooltip says; the link comes back after the backoff.
#[tokio::test]
async fn an_oversize_close_is_said_and_the_link_comes_back() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let reason = "a message of 40 MB exceeds mcp.host_max_message_mb (32)";
    c.close(1009, reason).await;
    let s = r.status(|s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("(1009): a message of 40 MB exceeds mcp.host_max_message_mb (32)")
            && line.contains("next try in 1 s"),
        "{line}"
    );
    let mut c = r.fake.accept().await;
    c.initialize().await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
}

/// 403 `host_not_granted`: "not granted", tried again every `lmgw.retry_max_s`, and at once on the
/// feed's next `hello`.
#[tokio::test]
async fn no_grant_is_said_and_the_feed_s_hello_tries_again() {
    let mut r = rig_with(PolicyConfig::default(), settings(30)).await;
    r.fake
        .refuse(
            403,
            json!({"code": "host_not_granted", "message": "device 'desktop' hosts no tools"}),
        )
        .await;
    let s = r
        .status(|s| matches!(s, HostStatus::NotGranted { .. }))
        .await;
    assert_eq!(
        s,
        HostStatus::NotGranted {
            retry_s: 30,
            message: "device 'desktop' hosts no tools".into()
        }
    );
    // lmgw's own words: it says which of its reasons it is (no label, no row, the row off).
    assert_eq!(
        crate::host::tools_line(&s, None),
        "tools: not granted (device 'desktop' hosts no tools; Kai tries again every 30 s and \
         when the feed reconnects)"
    );
    let empty = HostStatus::NotGranted {
        retry_s: 30,
        message: " ".into(),
    };
    assert!(
        crate::host::tools_line(&empty, None)
            .starts_with("tools: not granted (lmgw does not let this device host tools;")
    );
    r.fake.none_within(Duration::from_millis(1500)).await;
    r.host.feed_up("desktop", Some("kai"));
    let mut c = r.fake.accept().await;
    c.initialize().await;
    assert_eq!(
        r.status(|s| matches!(s, HostStatus::Linked { .. })).await,
        HostStatus::Linked { tools: 6 }
    );
}

/// lmgw's device link §3.5: 403 `device_unconfirmed` is no missing grant: the tool link waits for
/// the owner's Confirm and links at the feed's next `hello`.
#[tokio::test]
async fn an_unconfirmed_device_waits_for_the_feeds_hello() {
    let mut r = rig_with(PolicyConfig::default(), settings(30)).await;
    r.fake
        .refuse(
            403,
            json!({"code": "device_unconfirmed", "message": "confirm 7F3A-91C2 on the Devices card"}),
        )
        .await;
    let s = r.status(|s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("waiting for you to confirm this device on lmgw's Devices card"),
        "{line}"
    );
    r.fake.none_within(Duration::from_millis(1500)).await;
    r.host.feed_up("desktop", Some("kai"));
    let mut c = r.fake.accept().await;
    c.initialize().await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
}

/// A `device_unconfirmed` 403 whose body comes after the head is still read by its code (the
/// security review's nit): the body is read to its end before the status decides.
#[tokio::test]
async fn a_late_body_still_decides_the_403() {
    let mut r = rig_with(PolicyConfig::default(), settings(30)).await;
    r.fake
        .refuse_after(
            403,
            json!({"code": "device_unconfirmed", "message": "confirm 7F3A-91C2 on the Devices card"}),
            Some(Duration::from_millis(300)),
        )
        .await;
    let s = loop {
        match r.event().await {
            HostEvent::Status(s @ HostStatus::Offline { .. }) => break s,
            HostEvent::Status(s @ HostStatus::NotGranted { .. }) => panic!("read as {s:?}"),
            _ => {}
        }
    };
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("waiting for you to confirm this device"),
        "{line}"
    );
}

/// A credential beside the certificate (400 `credential_on_device_listener`) opens nothing either.
#[tokio::test]
async fn a_credential_on_the_device_listener_is_a_revocation() {
    let mut r = rig().await;
    r.fake
        .refuse(
            400,
            json!({"code": "credential_on_device_listener", "message": "send no Authorization"}),
        )
        .await;
    loop {
        if let HostEvent::Revoked { reason, loss } = r.event().await {
            assert_eq!(loss, Some(crate::lmgw_link::KeyLoss::PairAgain));
            assert_eq!(reason, "send no Authorization");
            break;
        }
    }
}

/// 401 at the handshake: the key opens nothing (§3.3), read by its code.
#[tokio::test]
async fn a_401_is_a_revocation() {
    let mut r = rig().await;
    r.fake
        .refuse(
            401,
            json!({"code": "device_key_unknown", "message": "this device key is unknown"}),
        )
        .await;
    loop {
        if let HostEvent::Revoked { reason, loss } = r.event().await {
            assert_eq!(reason, "this device key is unknown");
            assert_eq!(loss, Some(KeyLoss::PairAgain));
            break;
        }
    }
    r.fake.none_within(Duration::from_millis(2500)).await;
}

/// lmgw away: the backoff is said with its next try, and the link comes once lmgw answers.
#[tokio::test]
async fn an_unreachable_lmgw_is_retried_with_the_backoff() {
    let fake = FakeHost::new().await;
    let url = fake.url.clone();
    drop(fake);
    let mut r = rig().await;
    // A rig's own fake, and the host pointed at the closed port instead.
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&url), tx);
    let s = r.status(|s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("lmgw is not reachable") && line.contains("next try in 1 s"),
        "{line}"
    );
}

/// Device link D15: the tool link's WebSocket refuses a server under another CA before it sends
/// anything, says so, and tries again (the next try may reach the owner's lmgw).
#[tokio::test]
async fn the_tool_link_refuses_a_server_under_another_ca() {
    use crate::lmgw_link::test_pki::{Clients, FOREIGN};
    let foreign = Gateway::bind_as(&FOREIGN, Clients::None).await;
    let mut r = rig().await;
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&foreign.url), tx);
    let handshake = tokio::time::timeout(WAIT, foreign.accept()).await.unwrap();
    assert!(handshake.is_err(), "the handshake completed");
    let s = r.status(|s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("device authority changed") && line.contains("next try in 1 s"),
        "{line}"
    );
}

/// The next status the predicate takes; a revocation on the way fails the test.
async fn unrevoked(r: &mut Rig, want: impl Fn(&HostStatus) -> bool) -> HostStatus {
    loop {
        match r.event().await {
            HostEvent::Status(s) if want(&s) => return s,
            HostEvent::Status(_) => {}
            HostEvent::Revoked { reason, loss } => panic!("revoked: {reason} ({loss:?})"),
        }
    }
}

/// The security review's B1: a port squatter's plaintext alert right after the ClientHello never
/// ends the tool link: said by TLS's name, and tried again on the backoff.
#[tokio::test]
async fn a_plaintext_alert_never_ends_the_tool_link() {
    use crate::lmgw_link::test_pki::{self, Squatter};
    let squatter = Squatter::bind().await;
    let mut r = rig().await;
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&squatter.url), tx);
    for (alert, name) in [
        (test_pki::BAD_CERTIFICATE, "bad_certificate"),
        (test_pki::UNKNOWN_CA, "unknown_ca"),
        (test_pki::CERTIFICATE_EXPIRED, "certificate_expired"),
    ] {
        tokio::time::timeout(WAIT, squatter.answer(alert))
            .await
            .expect("the tool link tried again");
        let s = unrevoked(&mut r, |s| matches!(s, HostStatus::Offline { .. })).await;
        let line = crate::host::tools_line(&s, None);
        assert!(
            line.contains(name) && line.contains("next try in") && !line.contains("pair again"),
            "{line}"
        );
    }
    tokio::time::timeout(WAIT, squatter.answer(test_pki::BAD_CERTIFICATE))
        .await
        .expect("the tool link still tries");
}

/// lmgw's own `certificate_expired` alert (a certificate past its date) is no revocation of the
/// tool link either: said and tried again; the lapse is the feed's to find, by Kai's date.
#[tokio::test]
async fn a_certificate_alert_from_lmgw_is_said_and_tried_again() {
    use crate::lmgw_link::test_pki::{PKI, device_cert};
    let mut r = rig().await;
    let mut p = PKI.paired(&[&r.fake.url]);
    p.credentials.certificate =
        device_cert(&PKI.ca, &PKI.device_key.public_key().unwrap(), (2020, 2021));
    let lapsed = Credentials {
        reach: crate::lmgw_link::tls::Reach::new(p.credentials.urls.clone()),
        tls: crate::lmgw_link::tls::Tls::paired(&p.credentials, &PKI.device_key, WAIT).unwrap(),
    };
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(lapsed, tx);
    let _ = tokio::time::timeout(WAIT, r.fake.listener.accept()).await;
    let s = unrevoked(&mut r, |s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(line.contains("certificate_expired"), "{line}");
    let again = tokio::time::timeout(WAIT, r.fake.listener.accept()).await;
    assert!(again.is_ok(), "the tool link tried again");
}

/// Kai's stop closes the link with 1000 and its reason; the calls in flight end with it.
#[tokio::test]
async fn kais_stop_is_a_normal_close() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.call("hang", json!({}), owner(), 60_000).await;
    r.ran.until("hang started").await;
    r.host.stop();
    assert_eq!(
        c.closed_by_kai().await,
        (1000, crate::host::link::CLOSE_REASON.to_owned())
    );
    r.ran.until("hang dropped").await;
    r.fake.none_within(Duration::from_millis(1500)).await;
}
