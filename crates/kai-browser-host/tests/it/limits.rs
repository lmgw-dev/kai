//! Firefox's 1 MiB toward the extension: up to it a frame passes, past it the host says so and
//! the link goes on; toward Kai there is no such limit.

use serde_json::json;

use super::*;

const MIB: usize = 1024 * 1024;

/// A JSON object of exactly `len` bytes.
fn frame_of(len: usize) -> Vec<u8> {
    let head = br#"{"t":"op","pad":""#;
    let tail = br#""}"#;
    let mut f = head.to_vec();
    f.resize(len - tail.len(), b'a');
    f.extend_from_slice(tail);
    assert_eq!(f.len(), len);
    let _: Value = serde_json::from_slice(&f).unwrap();
    f
}

#[test]
fn a_frame_from_kai_over_firefoxs_limit_is_not_passed_on() {
    let (_dir, _kai, mut conn, host) = linked();
    let at_limit = frame_of(MIB);
    conn.send_raw(&at_limit);
    assert_eq!(host.recv_raw(), at_limit, "1 MiB itself passes");

    conn.send_raw(&frame_of(MIB + 1));
    let e = host.recv();
    assert_eq!(e["t"], "error");
    assert_eq!(e["code"], "frame_too_large");
    assert_eq!(e["size"], MIB + 1);
    assert_eq!(e["limit"], MIB);
    assert!(e["message"].as_str().unwrap().contains("1048577 bytes"));
    host.logs("Kai sent a frame of 1048577 bytes");

    // The link goes on, in step: the next frame is read from its start.
    conn.send(json!({"t": "welcome", "protocol": 1}));
    assert_eq!(host.recv(), json!({"t": "welcome", "protocol": 1}));
    conn.send_raw(&frame_of(5 * MIB));
    assert_eq!(host.recv()["size"], 5 * MIB);
    conn.send(json!({"t": "cancel", "id": 1}));
    assert_eq!(host.recv(), json!({"t": "cancel", "id": 1}));
}

#[test]
fn a_long_frame_toward_kai_passes_whole() {
    let (_dir, _kai, mut conn, mut host) = linked();
    let long = frame_of(5 * MIB + 3);
    host.send_raw(&long);
    assert_eq!(conn.recv_raw().unwrap(), long);
}
