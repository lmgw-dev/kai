//! Who links: the socket's mode, the peer check (and its live configuration), `hello` against
//! `welcome` and `refused`, frames before the hello, the link's facts and the registry's signal,
//! and a refusal logged once.

use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::*;
use crate::browser::frame::TO_FIREFOX_MAX;
use crate::logging::test_support::Capture;

#[tokio::test]
async fn a_firefox_says_hello_and_is_linked() {
    let mut rig = Rig::start(in_firefox).await;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&rig.socket), 0o600);
    assert_eq!(mode(rig.socket.parent().unwrap()), 0o700);
    let before = *rig.changed.borrow_and_update();

    let c = rig.link(&["tabs.list@1", "page.outline@1"]).await;
    rig.links_are(1).await;
    let info = rig.links.list().remove(0);
    assert_eq!(info.profile, "c0ffee");
    assert_eq!(info.ext_version, "0.1.0");
    assert_eq!(info.firefox, "159.0");
    assert_eq!(info.protocol, 1);
    assert_eq!(info.ops, ["tabs.list@1", "page.outline@1"]);
    assert!(
        *rig.changed.borrow_and_update() > before,
        "a new link signals the host's registry"
    );

    // A second Firefox (another profile) is a second link.
    let c2 = rig.link(&["tabs.list@1"]).await;
    rig.links_are(2).await;
    let ids: Vec<u64> = rig.links.list().iter().map(|l| l.id).collect();
    assert!(ids[0] < ids[1], "{ids:?}");

    // Firefox quits: the link goes, and the registry hears it.
    let mark = *rig.changed.borrow_and_update();
    drop(c);
    rig.links_are(1).await;
    assert!(*rig.changed.borrow_and_update() > mark);
    assert_eq!(rig.links.list()[0].id, ids[1]);
    drop(c2);
    rig.links_are(0).await;
}

#[tokio::test]
async fn a_mismatched_protocol_or_an_unreadable_first_frame_is_refused() {
    let rig = Rig::start(in_firefox).await;
    let cases = [
        (
            super::hello(2, &[]),
            "update Kai: Kai for Firefox speaks protocol 2, this Kai protocol 1",
        ),
        (
            super::hello(0, &[]),
            "update the extension: it speaks protocol 0, this Kai protocol 1",
        ),
        (
            json!({"id": 1, "ok": null}),
            "must be its hello, not a frame without a type",
        ),
        (
            json!({"t": "hello", "protocol": 1}),
            "could not read the extension's hello",
        ),
    ];
    for (first, says) in cases {
        let mut c = Client::connect(&rig.socket).await;
        c.send(first).await;
        let refused = c.recv().await.unwrap();
        assert_eq!(refused["t"], "refused");
        let reason = refused["reason"].as_str().unwrap();
        assert!(reason.contains(says), "{reason}");
        assert_eq!(c.recv().await, None, "Kai closes after refusing");
    }
    // Not JSON at all.
    let mut c = Client::connect(&rig.socket).await;
    frame::write(&mut c.wr, b"hello?").await.unwrap();
    let refused = c.recv().await.unwrap();
    assert!(refused["reason"].as_str().unwrap().contains("first frame"));
    assert!(rig.links.list().is_empty());
}

/// A typed frame before the hello is a late one of the profile's earlier link, which the host
/// passed on as this connection began: read past, and the hello after it links.
#[tokio::test]
async fn frames_before_the_hello_are_read_past() {
    let rig = Rig::start(in_firefox).await;
    let mut c = Client::connect(&rig.socket).await;
    c.send(json!({"t": "op_result", "id": 1, "ok": null})).await;
    c.send(json!({"t": "event", "kind": "loaded"})).await;
    c.send(json!({"t": "later", "x": [1, 2]})).await;
    c.send(super::hello(1, &["tabs.list@1"])).await;
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "welcome", "kai_version": crate::VERSION, "protocol": 1,
               "takes": crate::browser::server::TAKES})
    );
    rig.links_are(1).await;
    assert_eq!(rig.links.list()[0].ops, ["tabs.list@1"]);
}

/// Before the hello a frame may be at most Firefox's limit for a message, which a hello never
/// reaches: a longer one is refused by its length, naming it, before Kai reads its body.
#[tokio::test]
async fn a_frame_before_the_hello_over_firefoxs_limit_is_refused_unread() {
    let rig = Rig::start(in_firefox).await;
    let mut c = Client::connect(&rig.socket).await;
    let len = 2 * TO_FIREFOX_MAX as u32;
    c.wr.write_all(&len.to_le_bytes()).await.unwrap();
    c.wr.write_all(br#"{"t":"hello","#).await.unwrap();
    let refused = c.recv().await.unwrap();
    assert_eq!(refused["t"], "refused");
    let reason = refused["reason"].as_str().unwrap();
    assert!(reason.contains("a frame of 2097152 bytes"), "{reason}");
    assert!(reason.contains("at most 1048576 bytes"), "{reason}");
    // Kai closed with the body unread: the end, or the kernel's reset for the unread bytes.
    assert!(
        !matches!(frame::read(&mut c.rd).await, Ok(Some(_))),
        "nothing after the refusal"
    );
    assert!(rig.links.list().is_empty());

    // At the limit itself the frame is read (and refused for what it is).
    let mut c = Client::connect(&rig.socket).await;
    let mut body = br#"{"t":"hello","pad":""#.to_vec();
    body.resize(TO_FIREFOX_MAX - 2, b'a');
    body.extend_from_slice(br#""}"#);
    frame::write(&mut c.wr, &body).await.unwrap();
    let reason = c.recv().await.unwrap()["reason"].clone();
    assert!(
        reason
            .as_str()
            .unwrap()
            .contains("could not read the extension's hello"),
        "{reason}"
    );
}

#[tokio::test]
async fn a_process_outside_firefoxs_scope_is_refused_before_its_hello() {
    let rig = Rig::start(in_terminal).await;
    for _ in 0..2 {
        let mut c = Client::connect(&rig.socket).await;
        // Kai answers at once, before the extension says anything.
        let refused = c.recv().await.unwrap();
        assert_eq!(refused["t"], "refused");
        let reason = refused["reason"].as_str().unwrap();
        assert!(
            reason.contains("app-org.kde.konsole-61734.scope"),
            "{reason}"
        );
        assert!(reason.contains("started from a terminal"), "{reason}");
        assert_eq!(c.recv().await, None);
    }
    assert!(rig.links.list().is_empty());
}

/// `browser.peer_scopes` is read again at each connection: a pattern the owner adds lets the
/// next one in.
#[tokio::test]
async fn the_peer_scopes_are_read_at_each_connection() {
    let rig = Rig::with(in_terminal, Some("")).await;
    let mut c = Client::connect(&rig.socket).await;
    assert_eq!(c.recv().await.unwrap()["t"], "refused");

    let file = rig.dir.path().join("config.toml");
    std::fs::write(
        &file,
        "[browser]\npeer_scopes = [\"app-org.kde.konsole-*.scope\"]\n",
    )
    .unwrap();
    let _linked = rig.link(&[]).await;
    rig.links_are(1).await;

    // A file that does not load: the scopes Kai read at its start (the defaults) apply.
    std::fs::write(&file, "[browser]\npeer_scopes = 3\n").unwrap();
    let mut c = Client::connect(&rig.socket).await;
    assert_eq!(c.recv().await.unwrap()["t"], "refused");

    // An empty list lets no Firefox in, and says so.
    std::fs::write(&file, "[browser]\npeer_scopes = []\n").unwrap();
    let mut c = Client::connect(&rig.socket).await;
    let refused = c.recv().await.unwrap();
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("list of Firefox's app scopes is empty")
    );
}

/// The host connects again every `retry_ms`, and Kai refuses again: a refusal is logged at `warn`
/// the first time and at `debug` after, for the peer check, the hello and a configuration that
/// does not load.
#[tokio::test]
async fn a_refusal_is_logged_once_then_at_debug() {
    // While a single dispatcher exists tracing works a callsite's interest out from the default of
    // the thread that registers it (`has_just_one`), so a callsite another test's thread registered
    // first would stay disabled here; a second live dispatcher makes it ask every dispatcher (as
    // `delegate::tests::a_trace_log_carries_nothing_private`).
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let out = Capture::default();
    let _log =
        tracing::subscriber::set_default(crate::logging::subscriber("debug", out.clone(), false));
    let rig = Rig::start(in_terminal).await;
    for _ in 0..3 {
        let mut c = Client::connect(&rig.socket).await;
        assert_eq!(c.recv().await.unwrap()["t"], "refused");
        assert_eq!(c.recv().await, None);
    }
    let rig = Rig::with(in_firefox, Some("[browser]\npeer_scopes = 3\n")).await;
    for _ in 0..2 {
        let mut c = Client::connect(&rig.socket).await;
        c.send(super::hello(2, &[])).await;
        assert_eq!(c.recv().await.unwrap()["t"], "refused");
        assert_eq!(c.recv().await, None);
    }
    let text = out.text();
    let lines = |what: &str, level: &str| {
        text.lines()
            .filter(|l| l.contains(what) && l.contains(level))
            .count()
    };
    for (what, warn, debug) in [
        ("browser: a connection refused (later", 1, 0),
        ("browser: a connection refused again", 0, 2),
        ("refused at its handshake", 1, 0),
        ("refused again at the handshake", 0, 1),
        ("the configuration does not load", 1, 1),
    ] {
        assert_eq!(lines(what, " WARN "), warn, "{what} at warn: {text}");
        assert_eq!(lines(what, " DEBUG "), debug, "{what} at debug: {text}");
    }
}
