//! The sidebar's exchange against a scripted gateway behind the tests' device listener on loopback
//! (`crate::lmgw_link::test_pki`): a stream's chunks as lmgw sends them with `Last-Event-ID`
//! passed on, the head's bound and no bound on a stream's length, byte bodies both ways, the whole
//! answer's bound, and a dropped stream closing lmgw's connection.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use super::*;
use crate::lmgw_link::test_pki::{Gateway, creds, read_request};

const LIMITS: Limits = Limits {
    max_mb: 1,
    timeout_s: 5,
};

fn get(path: &str, stream: bool) -> Outbound {
    Outbound {
        method: Method::Get,
        path: path.into(),
        headers: Vec::new(),
        body: Vec::new(),
        stream,
    }
}

/// One HTTP/1.1 chunk of `data`.
fn chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:x}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

#[tokio::test]
async fn a_stream_comes_as_lmgw_sends_it_with_the_cursor_passed_on() {
    let g = Gateway::bind().await;
    let creds = creds(&g.url);
    let (go_on, wait) = oneshot::channel::<()>();
    let (seen_tx, seen) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let (head, _) = read_request(&mut s).await;
        let _ = seen_tx.send(head);
        s.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-lmgw-fallback: other\r\n\
              set-cookie: s=1\r\ntransfer-encoding: chunked\r\n\r\n",
        )
        .await
        .unwrap();
        s.write_all(&chunk(b"id: e1:9:bb\nevent: hello\ndata: {}\n\n"))
            .await
            .unwrap();
        s.flush().await.unwrap();
        // The rest only once the test has the first chunk.
        wait.await.unwrap();
        s.write_all(&chunk(b": keep-alive\n\n")).await.unwrap();
        s.write_all(b"0\r\n\r\n").await.unwrap();
        s.flush().await.unwrap();
    });
    let mut out = get("/chat/api/feed", true);
    out.headers = vec![("last-event-id".into(), "e1:9:bb".into())];
    let answer = send(&creds, out, LIMITS).await.unwrap();
    let head = seen.await.unwrap();
    assert!(head.starts_with("GET /chat/api/feed HTTP/1.1"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("last-event-id: e1:9:bb"),
        "the cursor goes to lmgw as it came: {head}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "{head}"
    );
    assert_eq!(answer.status, 200);
    let names: Vec<&str> = answer.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        names.contains(&"content-type") && names.contains(&"x-lmgw-fallback"),
        "{names:?}"
    );
    let Body::Stream(mut body) = answer.body else {
        panic!("a stream was asked for");
    };
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(
        first, b"id: e1:9:bb\nevent: hello\ndata: {}\n\n",
        "before lmgw sent more"
    );
    go_on.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(c) = body.next().await {
        rest.extend(c.unwrap());
    }
    assert_eq!(rest, b": keep-alive\n\n");
    server.await.unwrap();
}

#[tokio::test]
async fn the_timeout_bounds_a_streams_head_and_not_its_length() {
    // No head within the bound: the 504 naming the key.
    let g = Gateway::bind().await;
    let creds_silent = creds(&g.url);
    let hold = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(s);
    });
    let quick = Limits {
        max_mb: 1,
        timeout_s: 1,
    };
    let refused = send(&creds_silent, get("/chat/api/feed", true), quick)
        .await
        .unwrap_err();
    hold.abort();
    assert_eq!(refused.status, 504);
    assert!(String::from_utf8_lossy(&refused.body).contains("window.proxy_timeout_s"));

    // The head at once, then silence for longer than the bound: the stream stays open.
    let g = Gateway::bind().await;
    let creds_slow = creds(&g.url);
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
              transfer-encoding: chunked\r\n\r\n",
        )
        .await
        .unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1600)).await;
        s.write_all(&chunk(b"event: late\ndata: {}\n\n"))
            .await
            .unwrap();
        s.write_all(b"0\r\n\r\n").await.unwrap();
        s.flush().await.unwrap();
    });
    let answer = send(&creds_slow, get("/chat/api/feed", true), quick)
        .await
        .unwrap();
    let Body::Stream(mut body) = answer.body else {
        panic!("a stream");
    };
    assert_eq!(
        body.next().await.unwrap().unwrap(),
        b"event: late\ndata: {}\n\n",
        "1.6 s after the head, past the 1 s bound"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn bytes_go_and_come_back_exactly() {
    let upload: Vec<u8> = (0..70_000u32).map(|i| (i * 31 % 256) as u8).collect();
    let picture: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let g = Gateway::bind().await;
    let creds = creds(&g.url);
    let answer_bytes = picture.clone();
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let seen = read_request(&mut s).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\n\
             content-disposition: attachment; filename=\"a.png\"\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n",
            answer_bytes.len()
        );
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(&answer_bytes).await.unwrap();
        s.flush().await.unwrap();
        seen
    });
    let out = Outbound {
        method: Method::Post,
        path: "/chat/api/threads/12/attachments?name=a%20b.wav".into(),
        headers: vec![("content-type".into(), "audio/wav".into())],
        body: upload.clone(),
        stream: false,
    };
    let answer = send(&creds, out, LIMITS).await.unwrap();
    let (head, body) = server.await.unwrap();
    assert!(
        head.starts_with("POST /chat/api/threads/12/attachments?name=a%20b.wav HTTP/1.1"),
        "{head}"
    );
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: audio/wav"),
        "{head}"
    );
    assert_eq!(body, upload, "the upload's bytes as they were");
    assert_eq!(answer.status, 200);
    let Body::Whole(got) = answer.body else {
        panic!("a whole answer was asked for");
    };
    assert_eq!(got, picture);
    assert!(answer.headers.contains(&(
        "content-disposition".into(),
        "attachment; filename=\"a.png\"".into()
    )));
}

#[tokio::test]
async fn a_whole_answer_over_the_bound_is_refused_and_a_stream_is_not() {
    let big = vec![b'x'; 1024 * 1024 + 1];
    for stream in [false, true] {
        let g = Gateway::bind().await;
        let creds = creds(&g.url);
        let body = big.clone();
        let server = tokio::spawn(async move {
            let (mut s, _) = g.accept().await.unwrap();
            let _ = read_request(&mut s).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            let _ = s.write_all(&body).await;
            let _ = s.flush().await;
        });
        let answer = send(&creds, get("/chat/api/export", stream), LIMITS).await;
        if stream {
            let Body::Stream(mut b) = answer.unwrap().body else {
                panic!("a stream");
            };
            let mut n = 0;
            while let Some(c) = b.next().await {
                n += c.unwrap().len();
            }
            assert_eq!(
                n,
                big.len(),
                "a stream is not bounded by window.proxy_max_mb"
            );
        } else {
            let refused = answer.unwrap_err();
            assert_eq!(refused.status, 502);
            let text = String::from_utf8_lossy(&refused.body).into_owned();
            assert!(
                text.contains("answer_too_large") && text.contains("window.proxy_max_mb (1 MB)"),
                "{text}"
            );
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn a_dropped_stream_closes_lmgws_connection() {
    let g = Gateway::bind().await;
    let creds = creds(&g.url);
    let (closed_tx, closed) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
              transfer-encoding: chunked\r\n\r\n",
        )
        .await
        .unwrap();
        s.write_all(&chunk(b"data: 1\n\n")).await.unwrap();
        s.flush().await.unwrap();
        // Kai reads nothing more: the next read sees the connection end.
        let mut buf = [0u8; 64];
        let ended = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await;
        let _ = closed_tx.send(matches!(ended, Ok(Ok(0)) | Ok(Err(_))));
    });
    let answer = send(&creds, get("/chat/api/threads/12/send", true), LIMITS)
        .await
        .unwrap();
    let Body::Stream(mut body) = answer.body else {
        panic!("a stream");
    };
    assert_eq!(body.next().await.unwrap().unwrap(), b"data: 1\n\n");
    drop(body);
    assert!(closed.await.unwrap(), "lmgw saw the connection end");
    server.await.unwrap();
}
