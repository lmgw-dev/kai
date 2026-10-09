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
    r.host.start(
        Credentials {
            url,
            key: DeviceKey::parse(KEY).unwrap(),
        },
        tx,
    );
    let s = r.status(|s| matches!(s, HostStatus::Offline { .. })).await;
    let line = crate::host::tools_line(&s, None);
    assert!(
        line.contains("lmgw is not reachable") && line.contains("next try in 1 s"),
        "{line}"
    );
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
