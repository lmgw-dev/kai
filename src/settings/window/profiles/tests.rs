//! The Personality page's routes: exactly the profile editor's requests reach lmgw, after the
//! window's origin checks, and the page reads the conversation's facts as Kai writes them.

use serde_json::json;

use super::super::route::{Answer, Incoming, Routed, route};
use super::Conversation;
use crate::lmgw_link::proxy::{Forward, Method};

use super::super::tests::wire;

fn incoming<'a>(method: &'a str, uri: &'a str, body: &'a [u8]) -> Incoming<'a> {
    Incoming {
        method,
        uri,
        origin: Some("kai://settings"),
        fetch_site: Some("same-origin"),
        body,
    }
}

fn routed(method: &str, path: &str, body: &[u8]) -> Routed {
    route(&incoming(method, &format!("kai://settings{path}"), body))
}

fn refused(r: Routed) -> Answer {
    match r {
        Routed::Answer(a) => a,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn exactly_the_editors_requests_are_forwarded() {
    let body = br#"{"name":"Calm"}"#;
    for (method, path, lmgw, how) in [
        (
            "GET",
            "/lmgw/chat/api/profiles",
            "/chat/api/profiles",
            Method::Get,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles",
            "/chat/api/profiles",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/7",
            "/chat/api/profiles/7",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/7/reset",
            "/chat/api/profiles/7/reset",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/12/delete",
            "/chat/api/profiles/12/delete",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/preview",
            "/chat/api/profiles/preview",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/test",
            "/chat/api/profiles/test",
            Method::Post,
        ),
        (
            "POST",
            "/lmgw/chat/api/profiles/speak",
            "/chat/api/profiles/speak",
            Method::Post,
        ),
        ("GET", "/lmgw/v1/models", "/v1/models", Method::Get),
        (
            "GET",
            "/lmgw/v1/audio/voices?model=pocket-tts",
            "/v1/audio/voices?model=pocket-tts",
            Method::Get,
        ),
    ] {
        let sent: &[u8] = if how == Method::Post { body } else { b"" };
        match routed(method, path, sent) {
            Routed::Proxy(f) => assert_eq!(
                f,
                Forward {
                    method: how,
                    path: lmgw.into(),
                    body: sent.to_vec(),
                },
                "{method} {path}"
            ),
            other => panic!("{method} {path}: {other:?}"),
        }
    }
}

#[test]
fn the_voices_query_is_written_again_as_lmgw_reads_it() {
    let Routed::Proxy(f) = routed("GET", "/lmgw/v1/audio/voices?model=tts%2Fa%20b%26c", b"") else {
        panic!("not forwarded");
    };
    assert_eq!(f.path, "/v1/audio/voices?model=tts%2Fa+b%26c");
}

#[test]
fn everything_else_under_the_prefix_is_not_found_and_reaches_nothing() {
    for (method, path) in [
        // lmgw's own route the kit does not call.
        ("GET", "/lmgw/chat/api/profiles/7"),
        // Other methods of the admitted paths.
        ("POST", "/lmgw/v1/models"),
        ("POST", "/lmgw/v1/audio/voices?model=x"),
        ("GET", "/lmgw/chat/api/profiles/preview"),
        ("GET", "/lmgw/chat/api/profiles/7/delete"),
        ("PUT", "/lmgw/chat/api/profiles"),
        ("DELETE", "/lmgw/chat/api/profiles/7"),
        // Other routes of lmgw's.
        ("GET", "/lmgw/chat/api/threads"),
        ("POST", "/lmgw/chat/api/folders/3"),
        ("GET", "/lmgw/chat/api/feed"),
        ("POST", "/lmgw/v1/chat/completions"),
        ("POST", "/lmgw/v1/audio/speech"),
        ("POST", "/lmgw/api/op/key_create"),
        ("GET", "/lmgw/api/status"),
        ("GET", "/lmgw/mcp"),
        ("GET", "/lmgw/"),
        ("GET", "/lmgw"),
        // Shapes of the admitted paths that are not the kit's.
        ("GET", "/lmgw/chat/api/profiles/"),
        ("GET", "/lmgw//chat/api/profiles"),
        ("POST", "/lmgw/chat/api/profiles/-1"),
        ("POST", "/lmgw/chat/api/profiles/abc/delete"),
        ("POST", "/lmgw/chat/api/profiles/7/other"),
        ("POST", "/lmgw/chat/api/profiles/7/delete/x"),
        ("POST", "/lmgw/chat/api/profiles/99999999999999999999"),
        ("POST", "/lmgw/chat/api/%70rofiles"),
        ("GET", "/lmgw/chat/api/profiles?all=1"),
        ("GET", "/lmgw/v1/models?x=1"),
        ("GET", "/lmgw/v1/audio/voices"),
        ("GET", "/lmgw/v1/audio/voices?model="),
        ("GET", "/lmgw/v1/audio/voices?model=a&model=b"),
        ("GET", "/lmgw/v1/audio/voices?model=a&x=1"),
        ("GET", "/lmgw/v1/audio/voices?voice=a"),
    ] {
        let a = refused(routed(method, path, br#"{}"#));
        assert_eq!(a.status, 404, "{method} {path}");
        let v: serde_json::Value = serde_json::from_slice(&a.body).unwrap();
        assert_eq!(v["code"], "not_found", "{method} {path}");
        assert!(a.csp.is_none());
    }
    // A GET carries no body; a POST's is JSON.
    assert_eq!(
        refused(routed("GET", "/lmgw/chat/api/profiles", b"{}")).status,
        404
    );
    let a = refused(routed("POST", "/lmgw/chat/api/profiles", b"name=x"));
    assert_eq!(a.status, 400);
    assert!(String::from_utf8_lossy(&a.body).contains("invalid_json"));
    // A path that only begins like the prefix is a page path, not lmgw's.
    assert!(matches!(
        routed("GET", "/lmgwx/v1/models", b""),
        Routed::Answer(Answer { status: 404, .. })
    ));
}

#[test]
fn the_windows_origin_checks_come_first() {
    let path = "kai://settings/lmgw/chat/api/profiles";
    for (uri, origin, site) in [
        ("kai://app/lmgw/chat/api/profiles", None, None),
        ("kai://other/lmgw/v1/models", None, None),
        (path, Some("kai://app"), None),
        (path, Some("null"), None),
        (path, None, Some("cross-site")),
        (path, None, Some("same-site")),
    ] {
        let req = Incoming {
            method: "POST",
            uri,
            origin,
            fetch_site: site,
            body: b"{}",
        };
        assert_eq!(
            refused(route(&req)).status,
            403,
            "{uri} {origin:?} {site:?}"
        );
    }
    // A navigation the owner started, or a request naming no origin, is the window's.
    let req = Incoming {
        method: "GET",
        uri: path,
        origin: None,
        fetch_site: Some("none"),
        body: b"",
    };
    assert!(matches!(route(&req), Routed::Proxy(_)));
}

#[test]
fn the_page_reads_the_conversations_facts_as_kai_writes_them() {
    assert!(matches!(
        routed("GET", "/api/conversation", b""),
        Routed::Conversation
    ));
    assert_eq!(
        refused(routed("POST", "/api/conversation", b"")).status,
        405
    );
    let facts = Conversation {
        paired: true,
        why: None,
        gateway: Some("http://127.0.0.1:8001".into()),
        folder: Some("Kai".into()),
        thread_id: Some(41),
        model: Some("gemma4-12b".into()),
        profile: Some("Default (global: Concise)".into()),
    };
    let read: wire::Conversation =
        serde_json::from_value(serde_json::to_value(&facts).unwrap()).unwrap();
    assert_eq!(
        read,
        wire::Conversation {
            paired: true,
            why: None,
            gateway: Some("http://127.0.0.1:8001".into()),
            folder: Some("Kai".into()),
            thread_id: Some(41),
            model: Some("gemma4-12b".into()),
            profile: Some("Default (global: Concise)".into()),
        }
    );
    let unpaired: wire::Conversation = serde_json::from_value(json!({
        "paired": false, "why": "Kai is not paired", "gateway": null, "folder": null,
        "thread_id": null, "model": null, "profile": null
    }))
    .unwrap();
    assert!(!unpaired.paired);
}
