//! Frames both ways, unread: only the extension's `config` is the host's.

use serde_json::json;

use super::*;

#[test]
fn frames_pass_both_ways_unchanged() {
    let (_dir, _kai, mut conn, mut host) = linked();
    let hello = br#"{"t":"hello","protocol":1,"ops":["tabs.list@1"],"profile":"c0ffee"}"#;
    host.send_raw(hello);
    assert_eq!(conn.recv_raw().unwrap(), hello);

    // Whatever the extension sends goes byte for byte: no JSON, a nested "config", spacing.
    for body in [
        &b"not json at all"[..],
        br#"{"t":"op_result","id":3,"ok":{"t":"config","retry_ms":1}}"#,
        br#"{"t":"chunk","split":1,"seq":0,"data":"{\"t\":\"config\","}"#,
        b"{ \"t\" : \"event\" ,\n \"kind\":\"loaded\" }",
        b"",
    ] {
        host.send_raw(body);
        assert_eq!(conn.recv_raw().unwrap(), body);
    }

    let welcome = br#"{"t":"welcome","kai_version":"0.2.0","protocol":1}"#;
    conn.send_raw(welcome);
    assert_eq!(host.recv_raw(), welcome);
    conn.send_raw(b"{\"t\":\"op\",\"id\":1,\"op\":\"tabs.list@1\",\"args\":{}}  ");
    assert_eq!(
        host.recv_raw(),
        b"{\"t\":\"op\",\"id\":1,\"op\":\"tabs.list@1\",\"args\":{}}  "
    );
    // A frame Kai sends with the host's own type is still Kai's: the host reads nothing of Kai's.
    conn.send(json!({"t": "config", "retry_ms": 1}));
    assert_eq!(host.recv(), json!({"t": "config", "retry_ms": 1}));
    host.quiet(Duration::from_millis(100));
}

#[test]
fn config_is_answered_and_a_bad_one_is_said() {
    let (_dir, _kai, mut conn, mut host) = linked();
    host.send(json!({"t": "config"}));
    assert!(host.kai(), "a config without retry_ms is answered too");
    for bad in [json!(0), json!(-5), json!(1.5), json!("2000")] {
        host.send(json!({"t": "config", "retry_ms": bad}));
        let e = host.recv();
        assert_eq!(e["t"], "error");
        assert_eq!(e["code"], "bad_config");
        let message = e["message"].as_str().unwrap();
        assert!(message.contains("the host keeps 50"), "{message}");
        assert!(host.kai());
    }
    // None of it reached Kai.
    host.send(json!({"t": "event", "kind": "x"}));
    assert_eq!(conn.recv(), json!({"t": "event", "kind": "x"}));
}
