//! The sidebar's requests through `browser.sock` and Kai's proxy to a scripted lmgw behind the
//! tests' device listener (`crate::lmgw_link::test_pki`), with the core's part (pairing, the
//! request's bound, the turn's mark) left to the core's and the voice's tests: a stream in chunks
//! under the window and its acks with `Last-Event-ID` passed on, byte bodies both ways, `abort`
//! and the link's end closing lmgw's connection, the head's timeout, a whole answer's bound, and
//! Kai's refusals.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use super::{Client, Rig, in_firefox};
use crate::browser::sidebar::{Proxy, Relay, Request};
use crate::lmgw_link::proxy::{Limits, relay};
use crate::lmgw_link::test_pki::{Gateway, creds, read_request};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What the core does with an admitted request, minus the pairing and the mark: sent to `url`'s
/// gateway with `limits`, relayed with `ahead_mb` (module doc). The requests are kept.
pub(super) fn proxy(url: &str, limits: Limits, ahead_mb: u64) -> (Proxy, Arc<Mutex<Vec<Request>>>) {
    let creds = creds(url);
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let kept = seen.clone();
    let p: Proxy = Arc::new(move |Relay { request, reply }| {
        kept.lock().unwrap().push(request.clone());
        let creds = creds.clone();
        let answer = Box::pin(async move { relay::send(&creds, request.out, limits).await });
        tokio::spawn(reply.run(answer, ahead_mb, None, Default::default()));
    });
    (p, seen)
}

pub(super) const LIMITS: Limits = Limits {
    max_mb: 1,
    timeout_s: 5,
};

/// One HTTP/1.1 chunk of `data`.
pub(super) fn http_chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:x}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

pub(super) const SSE_HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

/// The next frame within `ms`, or `None` when none comes.
pub(super) async fn within(c: &mut Client, ms: u64) -> Option<Value> {
    tokio::time::timeout(Duration::from_millis(ms), c.recv())
        .await
        .ok()
        .flatten()
}

/// A body's frames until its `end`: the joined data (text, or base64 decoded) and the `end`.
async fn body(c: &mut Client, id: Value, text: bool) -> (Vec<u8>, Value) {
    let mut data = Vec::new();
    let mut seq = 0;
    loop {
        let f = c.recv().await.unwrap();
        assert_eq!(f["id"], id, "{f}");
        if f["t"] == "end" {
            assert_eq!(f["seq"], seq);
            return (data, f);
        }
        assert_eq!(
            (f["t"].as_str(), f["seq"].as_u64()),
            (Some("chunk"), Some(seq))
        );
        let d = f["data"].as_str().unwrap();
        if text {
            data.extend_from_slice(d.as_bytes());
        } else {
            data.extend(B64.decode(d).unwrap());
        }
        seq += 1;
    }
}

#[tokio::test]
async fn a_feed_streams_under_the_window_with_its_cursor_passed_on() {
    // About 3 MiB of events, each line numbered, in 64 KiB writes.
    let mut events = Vec::new();
    for i in 0..1500 {
        events.extend(format!("id: e{i}\ndata: {}\n\n", "x".repeat(2000)).into_bytes());
    }
    let g = Gateway::bind().await;
    let (p, seen) = proxy(&g.url, LIMITS, 1);
    let sent = events.clone();
    let (head_tx, head) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let (h, _) = read_request(&mut s).await;
        let _ = head_tx.send(h);
        s.write_all(SSE_HEAD).await.unwrap();
        for piece in sent.chunks(64 * 1024) {
            s.write_all(&http_chunk(piece)).await.unwrap();
        }
        s.write_all(b"0\r\n\r\n").await.unwrap();
        s.flush().await.unwrap();
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(
        json!({"t": "req", "id": 7, "method": "GET", "path": "/chat/api/feed",
                  "headers": {"Last-Event-ID": "e1:9:bb"}, "read": "stream"}),
    )
    .await;
    let res = c.recv().await.unwrap();
    assert_eq!(
        res,
        json!({"t": "res", "id": 7, "status": 200,
               "headers": {"content-type": "text/event-stream"}, "body": "text"})
    );
    let h = head.await.unwrap().to_ascii_lowercase();
    assert!(h.contains("last-event-id: e1:9:bb"), "{h}");
    assert!(seen.lock().unwrap()[0].out.stream);

    // Without an ack Kai stops at the window: 1 MiB, the frame that crosses it included.
    let (mut got, mut seq, mut unacked) = (Vec::new(), 0u64, 0usize);
    while let Some(f) = within(&mut c, 400).await {
        assert_eq!(
            (f["t"].as_str(), f["seq"].as_u64()),
            (Some("chunk"), Some(seq)),
            "{f}"
        );
        unacked += f.to_string().len();
        got.extend_from_slice(f["data"].as_str().unwrap().as_bytes());
        seq += 1;
    }
    assert!(
        (1024 * 1024 - 70 * 1024..=2 * 1024 * 1024).contains(&unacked),
        "Kai held at the window: {unacked} bytes in {seq} frames out"
    );
    assert!(got.len() < events.len(), "the stream is not done yet");
    // Each ack lets more come, until the end.
    let mut ended = false;
    while !ended {
        c.send(json!({"t": "ack", "id": 7, "seq": seq})).await;
        let mut moved = false;
        while let Some(f) = within(&mut c, 400).await {
            moved = true;
            if f["t"] == "end" {
                assert_eq!(f, json!({"t": "end", "id": 7, "seq": seq}), "no error");
                ended = true;
                break;
            }
            assert_eq!(f["seq"], seq, "{f}");
            got.extend_from_slice(f["data"].as_str().unwrap().as_bytes());
            seq += 1;
        }
        assert!(moved, "an ack lets more come");
    }
    assert!(got == events, "the stream as lmgw sent it");
    server.await.unwrap();
}

#[tokio::test]
async fn bytes_go_and_come_as_they_are() {
    let upload: Vec<u8> = (0..300_000u32).map(|i| (i * 13 % 256) as u8).collect();
    let picture: Vec<u8> = (0..2_500_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let g = Gateway::bind().await;
    let (p, _) = proxy(
        &g.url,
        Limits {
            max_mb: 4,
            timeout_s: 5,
        },
        0,
    );
    let answer = picture.clone();
    let server = tokio::spawn(async move {
        // The upload.
        let (mut s, _) = g.accept().await.unwrap();
        let seen = read_request(&mut s).await;
        s.write_all(crate::lmgw_link::test_pki::answer("201 Created", r#"{"id":9}"#).as_bytes())
            .await
            .unwrap();
        drop(s);
        // The picture, with a header the sidebar reads and one it does not.
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-disposition: inline\r\n\
             set-cookie: s=1\r\nx-lmgw-fallback: other\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n",
            answer.len()
        );
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(&answer).await.unwrap();
        s.flush().await.unwrap();
        seen
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(json!({"t": "req", "id": 1, "method": "POST",
                  "path": "/chat/api/threads/12/attachments?name=a.bin",
                  "headers": {"content-type": "application/octet-stream"},
                  "body": {"base64": B64.encode(&upload)}}))
        .await;
    let res = c.recv().await.unwrap();
    assert_eq!(
        (res["status"].as_u64(), res["body"].as_str()),
        (Some(201), Some("text"))
    );
    let (json, _) = body(&mut c, json!(1), true).await;
    assert_eq!(json, br#"{"id":9}"#);

    c.send(json!({"t": "req", "id": 2, "method": "GET", "path": "/chat/api/attachments/9"}))
        .await;
    let res = c.recv().await.unwrap();
    assert_eq!(
        res,
        json!({"t": "res", "id": 2, "status": 200, "body": "base64",
               "headers": {"content-type": "image/png", "content-disposition": "inline",
                           "x-lmgw-fallback": "other"}})
    );
    let (bytes, end) = body(&mut c, json!(2), false).await;
    assert!(end.get("error").is_none());
    assert_eq!(bytes.len(), picture.len());
    assert!(bytes == picture, "the picture's bytes as they were");
    let (head, got) = server.await.unwrap();
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/octet-stream"),
        "{head}"
    );
    assert!(got == upload, "the upload's bytes as they were");
}

#[tokio::test]
async fn abort_and_the_links_end_close_lmgws_connection() {
    let g = Gateway::bind().await;
    let (p, _) = proxy(&g.url, LIMITS, 1);
    let (ended_tx, mut ended) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut s, _) = g.accept().await.unwrap();
            let ended_tx = ended_tx.clone();
            tokio::spawn(async move {
                let _ = read_request(&mut s).await;
                s.write_all(SSE_HEAD).await.unwrap();
                s.write_all(&http_chunk(b"data: 1\n\n")).await.unwrap();
                s.flush().await.unwrap();
                let mut buf = [0u8; 64];
                let r = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await;
                let _ = ended_tx.send(matches!(r, Ok(Ok(0) | Err(_))));
            });
        }
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    let send = json!({"t": "req", "id": 1, "method": "POST",
                      "path": "/chat/api/threads/12/send",
                      "headers": {"content-type": "application/json"},
                      "body": {"text": "{}"}, "read": "stream"});
    c.send(send.clone()).await;
    assert_eq!(c.recv().await.unwrap()["t"], "res");
    assert_eq!(c.recv().await.unwrap()["data"], "data: 1\n\n");
    c.send(json!({"t": "abort", "id": 1})).await;
    assert_eq!(ended.recv().await, Some(true), "lmgw saw the abort");
    // Nothing more of it: the next frame is another request's.
    c.send(json!({"t": "req", "id": 2, "method": "GET", "path": "/api/events"}))
        .await;
    let f = c.recv().await.unwrap();
    assert_eq!(
        (f["t"].as_str(), f["id"].as_i64()),
        (Some("res"), Some(2)),
        "{f}"
    );

    // A request open when the link ends ends with it.
    let mut c2 = rig.link(&[]).await;
    c2.send(send).await;
    assert_eq!(c2.recv().await.unwrap()["t"], "res");
    assert_eq!(c2.recv().await.unwrap()["t"], "chunk");
    drop(c2);
    assert_eq!(ended.recv().await, Some(true), "lmgw saw the link go");
    server.await.unwrap();
}

#[tokio::test]
async fn the_head_has_its_time_and_a_whole_answer_its_bound() {
    // No head within window.proxy_timeout_s.
    let g = Gateway::bind().await;
    let (p, _) = proxy(
        &g.url,
        Limits {
            max_mb: 1,
            timeout_s: 1,
        },
        1,
    );
    let hold = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(s);
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(
        json!({"t": "req", "id": 1, "method": "GET", "path": "/chat/api/feed",
                  "read": "stream"}),
    )
    .await;
    let res = c.recv().await.unwrap();
    assert_eq!(
        res,
        json!({"t": "res", "id": 1, "status": 504,
               "headers": {"content-type": "application/json"}, "body": "text"})
    );
    let (b, _) = body(&mut c, json!(1), true).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["code"], "timeout");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("window.proxy_timeout_s")
    );
    hold.abort();

    // A whole answer over window.proxy_max_mb.
    let g = Gateway::bind().await;
    let (p, _) = proxy(&g.url, LIMITS, 1);
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        let n = 1024 * 1024 + 1;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {n}\r\n\
             connection: close\r\n\r\n"
        );
        s.write_all(head.as_bytes()).await.unwrap();
        let _ = s.write_all(&vec![b' '; n]).await;
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(json!({"t": "req", "id": 2, "method": "GET", "path": "/chat/api/export"}))
        .await;
    let res = c.recv().await.unwrap();
    assert_eq!(res["status"], 502);
    let (b, _) = body(&mut c, json!(2), true).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["code"], "answer_too_large");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("window.proxy_max_mb (1 MB)")
    );
    server.await.unwrap();
}

#[tokio::test]
async fn kai_refuses_what_it_does_not_forward_and_nothing_goes_to_lmgw() {
    // Nothing listens there: a request that went would be a 502.
    let gone = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("https://{}", l.local_addr().unwrap())
    };
    let (p, seen) = proxy(&gone, LIMITS, 1);
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    let refusal = |res: &Value, status: u64| {
        assert_eq!(
            (
                res["t"].as_str(),
                res["status"].as_u64(),
                res["body"].as_str()
            ),
            (Some("res"), Some(status), Some("text")),
            "{res}"
        );
    };
    for (req, status, code) in [
        (
            json!({"t": "req", "id": 1, "method": "GET", "path": "/api/settings-full"}),
            404,
            "not_admitted",
        ),
        (
            json!({"t": "req", "id": 2, "method": "GET", "path": "/v1/models",
                   "headers": {"cookie": "s=1"}}),
            400,
            "header_not_admitted",
        ),
        (
            json!({"t": "req", "id": 3, "method": "POST", "path": "/chat/api/threads",
                   "body": "plain"}),
            400,
            "bad_req",
        ),
    ] {
        c.send(req.clone()).await;
        let res = c.recv().await.unwrap();
        refusal(&res, status);
        let (b, _) = body(&mut c, req["id"].clone(), true).await;
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["code"], code, "{v}");
    }
    assert!(seen.lock().unwrap().is_empty(), "none reached the proxy");
    // An admitted one goes, and an unreachable lmgw is a 502.
    c.send(json!({"t": "req", "id": 4, "method": "GET", "path": "/v1/models"}))
        .await;
    let res = c.recv().await.unwrap();
    refusal(&res, 502);
    let (b, _) = body(&mut c, json!(4), true).await;
    assert!(String::from_utf8_lossy(&b).contains("lmgw_unreachable"));
    // A second request under an open one's id is an error frame, and the first goes on.
    let g = Gateway::bind().await;
    let (p, _) = proxy(&g.url, LIMITS, 1);
    let hold = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(SSE_HEAD).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    let feed = json!({"t": "req", "id": 5, "method": "GET", "path": "/chat/api/feed",
                      "read": "stream"});
    c.send(feed.clone()).await;
    assert_eq!(c.recv().await.unwrap()["status"], 200);
    c.send(feed).await;
    let e = c.recv().await.unwrap();
    assert_eq!(
        (e["t"].as_str(), e["code"].as_str()),
        (Some("error"), Some("duplicate_req"))
    );
    assert_eq!(e["id"], 5);
    hold.abort();
}

/// Without a proxy (a Kai whose core does not take them) the sidebar's requests are refused 503,
/// and frames Kai does not know are read past.
#[tokio::test]
async fn without_a_proxy_the_sidebars_requests_are_refused() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&[]).await;
    c.send(json!({"t": "req", "id": "s1", "method": "GET",
                  "path": "/chat/api/threads", "headers": {}, "body": null}))
        .await;
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "res", "id": "s1", "status": 503,
               "headers": {"content-type": "application/json"}, "body": "text"})
    );
    let (b, _) = body(&mut c, json!("s1"), true).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["code"], "no_proxy");
    c.send(json!({"t": "ack", "id": "s1", "seq": 1})).await;
    c.send(json!({"t": "abort", "id": "s1"})).await;
    c.send(json!({"t": "later", "x": 1})).await;
    c.send(super::hello(1, &[])).await;
    c.send(json!({"t": "req", "id": 2, "method": "GET", "path": "/v1/models"}))
        .await;
    assert_eq!(c.recv().await.unwrap()["id"], 2);
}

/// The sidebar's site question (kai-firefox design §8.1, the site strip and "Attach this page"):
/// answered by Kai itself with the rules the tools apply, read again for every question (a change
/// of the settings shows at once), nothing sent to lmgw; one that does not read is an `error`.
#[tokio::test]
async fn a_site_question_is_answered_with_kais_rules_now() {
    use crate::browser::settings::Sites;
    use crate::browser::sites::SiteRules;

    let none: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec!["mail.example".into()]));
    let now = none.clone();
    let rules: crate::browser::sidebar::site::Rules = Arc::new(move || {
        SiteRules::read(&Sites {
            act: Vec::new(),
            read: vec!["news.example".into()],
            none: now.lock().unwrap().clone(),
        })
        .unwrap()
    });
    let rig = Rig::with_sites(rules).await;
    let mut c = rig.link(&[]).await;
    c.send(json!({"t": "site", "id": 41, "urls": [
        "https://mail.example/inbox", "https://news.example/", "https://shop.example/", ""
    ]}))
    .await;
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "site", "id": 41, "rules": [
            {"site": "mail.example", "rule": "none"},
            {"site": "news.example", "rule": "read"},
            {"site": "shop.example", "rule": "act"},
            {"site": "", "rule": "none"},
        ]})
    );
    // The owner moved mail.example off none: the next question reads the rules again.
    none.lock().unwrap().clear();
    c.send(json!({"t": "site", "id": 42, "urls": ["https://mail.example/inbox", ""]}))
        .await;
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "site", "id": 42, "rules": [
            {"site": "mail.example", "rule": "act"},
            {"site": "", "rule": "read"},
        ]})
    );
    c.send(json!({"t": "site", "id": 43})).await;
    let e = c.recv().await.unwrap();
    assert_eq!(
        (e["t"].as_str(), e["code"].as_str(), e["id"].as_u64()),
        (Some("error"), Some("bad_site"), Some(43))
    );
}

/// A frame Kai does not take (a newer extension's) that carries an id is answered with an `error`
/// frame `unknown_frame` with that id, so the extension ends the question instead of waiting; one
/// without an id is dropped as before. The welcome lists the frames Kai takes (`takes`).
#[tokio::test]
async fn an_unknown_frame_with_an_id_is_answered_with_an_error() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&[]).await;
    c.send(json!({"t": "later_question", "id": 44, "urls": []}))
        .await;
    let e = c.recv().await.unwrap();
    assert_eq!(
        (e["t"].as_str(), e["code"].as_str(), e["id"].as_u64()),
        (Some("error"), Some("unknown_frame"), Some(44))
    );
    let said = e["message"].as_str().unwrap();
    assert!(
        said.contains("later_question") && said.contains(crate::VERSION),
        "{said}"
    );
    c.send(json!({"t": "later_note", "x": 1})).await;
    c.send(json!({"t": "later_question", "id": null})).await;
    assert_eq!(
        within(&mut c, 300).await,
        None,
        "nothing for a frame without an id"
    );
    assert!(crate::browser::server::TAKES.contains(&"site"));
}
