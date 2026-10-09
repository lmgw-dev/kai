//! One run at a time and its backoff: a link that ends before lmgw's `initialize` backs off
//! (doubling) instead of starting over every second; a stopped run's handshake never completes
//! after the new run's (lmgw would close the new link with 4000), and the new run's peer is the
//! one lmgw is told about.

use std::time::Instant;

use serde_json::json;
use tokio::io::AsyncReadExt;

use super::*;
use crate::host::settings::ToolState;

fn creds(url: &str) -> Credentials {
    Credentials {
        url: url.to_owned(),
        key: DeviceKey::parse(KEY).unwrap(),
    }
}

async fn offline_note(r: &mut Rig) -> String {
    match r.status(|s| matches!(s, HostStatus::Offline { .. })).await {
        HostStatus::Offline { note } => note,
        _ => unreachable!(),
    }
}

/// lmgw takes the upgrade and closes before `initialize`: the waits grow (1 s, 2 s, …) as for a
/// refusal; only a link lmgw initialized starts the backoff over.
#[tokio::test]
async fn a_link_that_ends_before_initialize_backs_off() {
    let mut r = rig_with(PolicyConfig::default(), settings(8)).await;
    let mut c = r.fake.accept().await;
    c.close(1011, "the gateway is not ready").await;
    let first = offline_note(&mut r).await;
    assert!(first.ends_with("next try in 1 s"), "{first}");
    let mut c = r.fake.accept().await;
    let second_at = Instant::now();
    c.close(1011, "the gateway is not ready").await;
    let second = offline_note(&mut r).await;
    assert!(second.ends_with("next try in 2 s"), "{second}");
    // It waits what it said before it links again.
    let mut c = r.fake.accept().await;
    let gap = second_at.elapsed();
    assert!(
        gap >= Duration::from_millis(1900),
        "linked again after {gap:?}"
    );
    c.initialize().await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    // A link that was up: the backoff starts over.
    c.close(1011, "no pong within mcp.host_ping_interval_s")
        .await;
    let after_up = offline_note(&mut r).await;
    assert!(after_up.ends_with("next try in 1 s"), "{after_up}");
    r.fake.accept().await;
}

/// The fence's handshake half: a run stopped while lmgw has not answered its upgrade drops the
/// handshake at once, before the new run connects, so its upgrade can never complete after the
/// new one's.
#[tokio::test]
async fn a_stopped_runs_handshake_never_completes() {
    let mut r = rig().await;
    let mut old = r.fake.tcp(WAIT).await.expect("Kai linked");
    read_head(&mut old).await;
    // A restart (a re-pairing) while lmgw has not answered.
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&r.fake.url), tx);
    // At once: the stopped run did not sit out its handshake (lmgw.timeout_s is 5 s here).
    let new = r
        .fake
        .tcp(Duration::from_secs(2))
        .await
        .expect("Kai linked again at once");
    // The old connection is closed by the time the new upgrade arrives (before the fix it waited
    // for lmgw's answer up to lmgw.timeout_s).
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_millis(500), old.read(&mut byte))
        .await
        .expect("the stopped run's connection is still open");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    let mut c = FakeHost::upgrade(new).await;
    c.initialize().await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    r.fake.none_within(Duration::from_millis(1500)).await;
}

/// The fence's peer half: after a restart the new link is the one told `list_changed`; the old
/// run's end did not wipe its peer.
#[tokio::test]
async fn after_a_restart_the_new_link_hears_list_changed() {
    let mut r = rig().await;
    let mut old = r.linked().await;
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&r.fake.url), tx);
    assert_eq!(
        old.closed_by_kai().await,
        (1000, crate::host::link::CLOSE_REASON.to_owned())
    );
    let mut c = r.linked().await;
    let mut off = PolicyConfig::default();
    off.tools.insert("poke".into(), ToolState::Off);
    assert!(r.host.set_policy(Policy::new(&off)));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    let names: Vec<Value> = c.list().await.iter().map(|t| t["name"].clone()).collect();
    assert_eq!(
        names,
        [
            json!("echo"),
            json!("hang"),
            json!("own_only"),
            json!("prep"),
            json!("ctx")
        ]
    );
}

/// A restart forgets the old pairing's identity: until the new feed says `hello`, a call from the
/// old device name is a device's, not Kai's own.
#[tokio::test]
async fn a_restart_forgets_the_old_identity() {
    let mut r = rig().await;
    r.host.feed_up("desktop", Some("kai"));
    let mut c = r.linked().await;
    let own = json!({"kind": "device", "name": "desktop"});
    let id = c.call("own_only", json!({}), own.clone(), 60_000).await;
    assert!(!text_of(&c.answer(id, &mut Vec::new()).await).1);
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&r.fake.url), tx);
    c.closed_by_kai().await;
    let mut c = r.linked().await;
    let id = c.call("own_only", json!({}), own, 60_000).await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error && text.contains("not to device 'desktop'"), "{text}");
    assert_eq!(r.ran.all(), ["own_only by Kai itself"]);
}

/// `Deps::creds`: the providers see each start's credentials (a re-pairing's new ones), and none
/// once the link stopped (a revocation, quit).
#[tokio::test]
async fn the_providers_see_the_current_credentials() {
    let mut r = rig().await;
    let url = |r: &Rig| r.creds.borrow().as_ref().map(|c| c.url.clone());
    assert_eq!(url(&r), Some(r.fake.url.clone()));
    let other = FakeHost::new().await;
    let (tx, events) = mpsc::unbounded_channel();
    r.events = events;
    r.host.start(creds(&other.url), tx);
    assert_eq!(url(&r), Some(other.url.clone()), "a re-pairing's");
    assert_eq!(
        r.creds.borrow().as_ref().map(|c| c.key.expose().to_owned()),
        Some(KEY.to_owned())
    );
    r.host.stop();
    assert_eq!(url(&r), None);
}
