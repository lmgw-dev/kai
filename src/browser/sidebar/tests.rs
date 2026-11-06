//! The sidebar's request frames as Kai reads them, the window's arithmetic and a text stream's
//! characters across chunks. The requests through `browser.sock` to a scripted lmgw are
//! `crate::browser::tests::sidebar`; the turns are `turns`' own tests and the voice's.

use serde_json::{Map, Value, json};

use super::reply::{Utf8Carry, Window};
use super::*;

fn frame(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn a_request_reads_its_headers_body_and_mode() {
    let r = parse(&frame(json!({
        "t": "req", "id": 1, "method": "POST",
        "path": "/chat/api/threads/12/send",
        "headers": {"Content-Type": "application/json"},
        "body": {"text": "{\"content\":\"hi\"}"},
        "read": "stream",
        "later": "a field a later extension adds is read past",
    })))
    .unwrap();
    assert_eq!(r.out.method, Method::Post);
    assert_eq!(r.out.path, "/chat/api/threads/12/send");
    assert_eq!(
        r.out.headers,
        vec![("content-type".to_owned(), "application/json".to_owned())]
    );
    assert_eq!(r.out.body, br#"{"content":"hi"}"#);
    assert!(r.out.stream);
    assert_eq!(r.turn, Some((12, turns::Start::New)));

    let r = parse(&frame(json!({
        "id": 2, "method": "POST", "path": "/chat/api/threads/12/attachments?name=a.bin",
        "headers": {"content-type": "application/octet-stream"},
        "body": {"base64": "AAEC/w=="},
    })))
    .unwrap();
    assert_eq!(r.out.body, [0, 1, 2, 255]);
    assert!(!r.out.stream, "whole by default");
    assert_eq!(r.turn, None);

    let r = parse(&frame(json!({
        "id": 3, "method": "GET", "path": "/chat/api/feed",
        "headers": {"Last-Event-ID": "e1:9:bb"}, "body": null, "read": "whole",
    })))
    .unwrap();
    assert_eq!(
        r.out.headers,
        vec![("last-event-id".to_owned(), "e1:9:bb".to_owned())]
    );
    assert!(r.out.body.is_empty());
}

#[test]
fn a_request_kai_does_not_forward_says_why() {
    let refused = |v: Value| parse(&frame(v)).unwrap_err();
    let (s, c, m) = refused(json!({"id": 1, "method": "GET", "path": "/api/settings-full"}));
    assert_eq!((s, c), (404, "not_admitted"));
    assert!(m.contains("GET /api/settings-full"), "{m}");
    let (s, c, m) = refused(json!({"id": 1, "method": "GET", "path": "/v1/models",
                                   "headers": {"authorization": "Bearer x"}}));
    assert_eq!((s, c), (400, "header_not_admitted"));
    assert!(m.contains("authorization"), "{m}");
    for (v, says) in [
        (json!({"id": 1, "path": "/v1/models"}), "method and path"),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models", "headers": ["x"]}),
            "object",
        ),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models",
                   "headers": {"content-type": 3}}),
            "not text",
        ),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models",
                   "headers": {"last-event-id": "a\nb"}}),
            "cannot carry",
        ),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models",
                   "headers": {"Content-Type": "a", "content-type": "b"}}),
            "twice",
        ),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models", "body": {"text": "x"}}),
            "GET carries no body",
        ),
        (
            json!({"id": 1, "method": "POST", "path": "/chat/api/threads", "body": "x"}),
            "or none",
        ),
        (
            json!({"id": 1, "method": "POST", "path": "/chat/api/threads",
                   "body": {"text": "x", "base64": "eA=="}}),
            "or none",
        ),
        (
            json!({"id": 1, "method": "POST", "path": "/chat/api/threads",
                   "body": {"base64": "%%%"}}),
            "does not decode",
        ),
        (
            json!({"id": 1, "method": "GET", "path": "/v1/models", "read": "blob"}),
            "\"whole\" or \"stream\"",
        ),
    ] {
        let (s, c, m) = refused(v.clone());
        assert_eq!((s, c), (400, "bad_req"), "{v}");
        assert!(m.contains(says), "{v}: {m}");
    }
}

#[test]
fn the_window_lets_one_frame_go_and_holds_the_rest_until_acked() {
    let mib = 1024 * 1024;
    let mut w = Window::new(2);
    // A frame of the limit always goes when nothing is out.
    assert!(w.fits(mib));
    w.sent(0, mib);
    assert!(w.fits(mib), "2 MiB: a second fits");
    w.sent(1, mib);
    assert!(!w.fits(1), "full");
    // The sidebar took the first: room for one more.
    w.acked(1);
    assert!(w.fits(mib));
    w.sent(2, mib);
    assert!(!w.fits(1));
    // An ack beyond what was sent, or one going back, is read as what it can mean.
    w.acked(0);
    assert!(!w.fits(1));
    w.acked(99);
    assert!(w.fits(2 * mib));
    // 0: no bound.
    let mut free = Window::new(0);
    for i in 0..100 {
        assert!(free.fits(mib));
        free.sent(i, mib);
    }
}

#[test]
fn a_text_stream_keeps_a_character_split_between_chunks_whole() {
    let text = "grüße 😀 – €";
    let bytes = text.as_bytes();
    for cut in 0..=bytes.len() {
        let mut c = Utf8Carry::default();
        let mut out = c.push(&bytes[..cut]);
        out.push_str(&c.push(&bytes[cut..]));
        out.push_str(&c.finish());
        assert_eq!(out, text, "cut at {cut}");
    }
    // Invalid bytes become U+FFFD, as SSE's decoding makes them; an unfinished character at the
    // end too.
    let mut c = Utf8Carry::default();
    assert_eq!(c.push(b"a\xffb\xe2\x82"), "a\u{fffd}b");
    assert_eq!(c.finish(), "\u{fffd}");
}
