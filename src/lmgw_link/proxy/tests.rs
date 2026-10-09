//! The window's forwarding to lmgw against a scripted gateway on loopback: what goes out (the
//! bearer, the body, nothing of the page's), what comes back (lmgw's status and body, a reduced
//! content type, no other header), and the bounds.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::pairing::DeviceKey;
use crate::pairing::tests::KEY;

fn creds(url: &str) -> Credentials {
    Credentials {
        url: format!("{url}/"),
        key: DeviceKey::parse(KEY).unwrap(),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

const LIMITS: Limits = Limits {
    max_mb: 1,
    timeout_s: 5,
};

/// One request's head (lowercased header names kept as sent) and body.
async fn read_request(s: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the request ended early");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(buf[..at].to_vec()).unwrap();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while buf.len() < at + 4 + len {
                let n = s.read(&mut chunk).await.unwrap();
                assert!(n > 0, "the body ended early");
                buf.extend_from_slice(&chunk[..n]);
            }
            return (head, buf[at + 4..at + 4 + len].to_vec());
        }
    }
}

/// A gateway that answers one request with `head` (status line and headers) and `body`, and
/// hands back what it was asked.
async fn gateway(
    head: &'static str,
    body: Vec<u8>,
) -> (String, tokio::task::JoinHandle<(String, Vec<u8>)>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let seen = read_request(&mut s).await;
        let r = format!(
            "{head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(r.as_bytes()).await.unwrap();
        s.write_all(&body).await.unwrap();
        seen
    });
    (url, task)
}

#[tokio::test]
async fn a_post_goes_with_the_bearer_and_the_answer_comes_back_without_lmgws_headers() {
    let (url, seen) = gateway(
        "HTTP/1.1 201 Created\r\ncontent-type: application/json; charset=utf-8\r\n\
         set-cookie: s=1\r\nx-lmgw-fallback: other",
        br#"{"id":7,"name":"Calm"}"#.to_vec(),
    )
    .await;
    let forward = Forward {
        method: Method::Post,
        path: "/chat/api/profiles".into(),
        body: br#"{"name":"Calm"}"#.to_vec(),
    };
    let answer = send(&client(), &creds(&url), forward, LIMITS).await;
    let (head, body) = seen.await.unwrap();
    assert!(
        head.starts_with("POST /chat/api/profiles HTTP/1.1"),
        "{head}"
    );
    let lower = head.to_lowercase();
    assert!(
        lower.contains(&format!("authorization: bearer {}", KEY.to_lowercase())),
        "the device key is the bearer: {head}"
    );
    assert!(lower.contains("content-type: application/json"), "{head}");
    assert_eq!(body, br#"{"name":"Calm"}"#);
    // lmgw's status, body and (reduced) type; no header of lmgw's reaches the page, and the key
    // is nowhere in what does.
    assert_eq!(answer.status, 201);
    assert_eq!(answer.mime, "application/json");
    assert_eq!(answer.body, br#"{"id":7,"name":"Calm"}"#);
    assert!(!String::from_utf8_lossy(&answer.body).contains(KEY));
}

#[tokio::test]
async fn a_get_carries_its_query_and_no_body() {
    let (url, seen) = gateway(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json",
        br#"{"voices":[]}"#.to_vec(),
    )
    .await;
    let forward = Forward {
        method: Method::Get,
        path: "/v1/audio/voices?model=tts%2Fa+b".into(),
        body: Vec::new(),
    };
    let answer = send(&client(), &creds(&url), forward, LIMITS).await;
    let (head, body) = seen.await.unwrap();
    assert!(
        head.starts_with("GET /v1/audio/voices?model=tts%2Fa+b HTTP/1.1"),
        "{head}"
    );
    assert!(body.is_empty());
    assert_eq!((answer.status, answer.mime), (200, "application/json"));
}

#[tokio::test]
async fn speak_passes_the_wav_through_and_a_refusal_keeps_lmgws_words() {
    let wav = b"RIFF\x24\0\0\0WAVEfmt ".to_vec();
    let (url, _) = gateway("HTTP/1.1 200 OK\r\ncontent-type: audio/wav", wav.clone()).await;
    let forward = Forward {
        method: Method::Post,
        path: "/chat/api/profiles/speak".into(),
        body: b"{}".to_vec(),
    };
    let answer = send(&client(), &creds(&url), forward.clone(), LIMITS).await;
    assert_eq!(
        (answer.status, answer.mime, answer.body),
        (200, "audio/wav", wav)
    );
    // R1: a device refused editing a profile in admin use reads lmgw's own sentence.
    let refusal = br#"{"code":"profile_in_admin_use","message":"used by 1 admin thread"}"#.to_vec();
    let (url, _) = gateway(
        "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json",
        refusal.clone(),
    )
    .await;
    let answer = send(&client(), &creds(&url), forward, LIMITS).await;
    assert_eq!((answer.status, answer.body), (403, refusal));
}

#[tokio::test]
async fn an_answer_over_the_bound_is_refused_naming_the_key() {
    let (url, _) = gateway(
        "HTTP/1.1 200 OK\r\ncontent-type: audio/wav",
        vec![0u8; 1024 * 1024 + 1],
    )
    .await;
    let forward = Forward {
        method: Method::Get,
        path: "/v1/models".into(),
        body: Vec::new(),
    };
    let answer = send(&client(), &creds(&url), forward, LIMITS).await;
    assert_eq!(answer.status, 502);
    let v: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
    assert_eq!(v["code"], "answer_too_large");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("window.proxy_max_mb (1 MB)"),
        "{v}"
    );
}

#[tokio::test]
async fn a_request_over_the_bound_is_refused_before_it_goes() {
    let big = Forward {
        method: Method::Post,
        path: "/chat/api/profiles".into(),
        body: vec![b' '; 1024 * 1024 + 1],
    };
    let r = LIMITS.refuse_request(&big).expect("refused");
    assert_eq!(r.status, 413);
    assert!(String::from_utf8_lossy(&r.body).contains("window.proxy_max_mb (1 MB)"));
    let fits = Forward {
        body: vec![b' '; 1024 * 1024],
        ..big.clone()
    };
    assert_eq!(LIMITS.refuse_request(&fits), None);
    // 0: no bound.
    let none = Limits {
        max_mb: 0,
        timeout_s: 0,
    };
    assert_eq!(none.refuse_request(&big), None);
}

#[tokio::test]
async fn a_silent_lmgw_is_given_up_naming_the_key_and_a_closed_one_says_so() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let hold = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(s);
    });
    let forward = Forward {
        method: Method::Get,
        path: "/chat/api/profiles".into(),
        body: Vec::new(),
    };
    let quick = Limits {
        max_mb: 1,
        timeout_s: 1,
    };
    let answer = send(&client(), &creds(&url), forward.clone(), quick).await;
    hold.abort();
    assert_eq!(answer.status, 504);
    assert!(String::from_utf8_lossy(&answer.body).contains("window.proxy_timeout_s"));
    // Nothing listens: lmgw is not reachable.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gone = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    let answer = send(&client(), &creds(&gone), forward, quick).await;
    assert_eq!(answer.status, 502);
    assert!(String::from_utf8_lossy(&answer.body).contains("lmgw_unreachable"));
}

#[test]
fn only_the_types_the_page_reads_pass() {
    assert_eq!(mime_of(Some("application/json")), "application/json");
    assert_eq!(
        mime_of(Some("Application/JSON; charset=utf-8")),
        "application/json"
    );
    assert_eq!(mime_of(Some("audio/x-wav")), "audio/wav");
    assert_eq!(mime_of(Some("text/plain; charset=utf-8")), "text/plain");
    for other in [
        Some("text/html"),
        Some("image/svg+xml"),
        Some("text/javascript"),
        None,
    ] {
        assert_eq!(mime_of(other), "application/octet-stream", "{other:?}");
    }
}
