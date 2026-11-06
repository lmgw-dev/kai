//! The sidebar's requests at their edges, through `browser.sock` to a scripted lmgw: an ack for more
//! pieces than Kai sent, a turn route's mark against lmgw's answer (a refusal drops it at once, a
//! turn whose stream ended keeps it for the feed), and Kai leaving the gateway before lmgw answered.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::sidebar::{LIMITS, SSE_HEAD, http_chunk, proxy, within};
use super::{Client, Rig};
use crate::browser::sidebar::turns::SidebarTurns;
use crate::browser::sidebar::{Proxy, Relay};
use crate::lmgw_link::proxy::relay;
use crate::lmgw_link::test_pki::{Gateway, answer, creds, read_request};
use crate::voice::{By, FeedFact};

/// The chunk frames that come until Kai holds (none for 400 ms): their count and bytes; `None`
/// when the `end` came.
async fn until_held(c: &mut Client, mut seq: u64) -> Option<(u64, usize)> {
    let mut bytes = 0;
    while let Some(f) = within(c, 400).await {
        if f["t"] == "end" {
            return None;
        }
        assert_eq!(f["seq"], seq, "{f}");
        bytes += f.to_string().len();
        seq += 1;
    }
    Some((seq, bytes))
}

/// L5: an ack for more pieces than Kai sent (`u64::MAX`) counts the pieces sent: Kai's window
/// stays, and the stream goes on a window at a time.
#[tokio::test]
async fn an_ack_beyond_the_pieces_sent_keeps_the_window() {
    let mut events = Vec::new();
    for i in 0..1500 {
        events.extend(format!("id: e{i}\ndata: {}\n\n", "x".repeat(2000)).into_bytes());
    }
    let g = Gateway::bind().await;
    let (p, _) = proxy(&g.url, LIMITS, 1);
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(SSE_HEAD).await.unwrap();
        for piece in events.chunks(64 * 1024) {
            if s.write_all(&http_chunk(piece)).await.is_err() {
                return;
            }
        }
        let _ = s.write_all(b"0\r\n\r\n").await;
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(
        json!({"t": "req", "id": 7, "method": "GET", "path": "/chat/api/feed",
                  "read": "stream"}),
    )
    .await;
    assert_eq!(c.recv().await.unwrap()["status"], 200);
    let (seq, _) = until_held(&mut c, 0).await.expect("held at the window");
    c.send(json!({"t": "ack", "id": 7, "seq": u64::MAX})).await;
    let (seq2, bytes) = until_held(&mut c, seq)
        .await
        .expect("held again: the ack counted the pieces sent");
    assert!(seq2 > seq, "the ack let more come");
    assert!(
        bytes <= 2 * 1024 * 1024,
        "{bytes} bytes after the ack: one window"
    );
    c.send(json!({"t": "abort", "id": 7})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

fn own() -> By {
    By {
        name: "device 'desktop'".into(),
        own: true,
        sidebar: false,
    }
}

fn started(thread_id: i64) -> FeedFact {
    FeedFact::TurnStarted {
        thread_id,
        by: own(),
        voice: false,
    }
}

/// L1: a turn route lmgw refused started no turn, so its mark goes with the answer; one whose
/// stream ended before the feed's `turn.started` was read keeps its mark for it.
#[tokio::test]
async fn a_turn_routes_mark_goes_with_a_refusal_and_waits_for_the_feed_otherwise() {
    let g = Gateway::bind().await;
    let creds = creds(&g.url);
    let turns = SidebarTurns::default();
    let marks = turns.clone();
    let p: Proxy = Arc::new(move |Relay { request, reply }| {
        let mark = request.turn.map(|(t, start)| marks.mark(t, start));
        let creds = creds.clone();
        let answer = Box::pin(async move { relay::send(&creds, request.out, LIMITS).await });
        tokio::spawn(reply.run(answer, 1, mark, CancellationToken::new()));
    });
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(
            answer(
                "409 Conflict",
                r#"{"code":"turn_running","message":"a turn runs"}"#,
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        drop(s);
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        s.write_all(SSE_HEAD).await.unwrap();
        s.write_all(&http_chunk(b"event: done\ndata: {}\n\n"))
            .await
            .unwrap();
        s.write_all(b"0\r\n\r\n").await.unwrap();
        s.flush().await.unwrap();
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    for (id, thread) in [(1, 12), (2, 14)] {
        c.send(json!({"t": "req", "id": id, "method": "POST",
                      "path": format!("/chat/api/threads/{thread}/answer"),
                      "headers": {"content-type": "application/json"},
                      "body": {"text": "{}"}, "read": "stream"}))
            .await;
        loop {
            let f: Value = c.recv().await.unwrap();
            if f["t"] == "end" {
                break;
            }
        }
    }
    let by = |f: Option<FeedFact>| match f {
        Some(FeedFact::TurnStarted { by, .. }) => by,
        other => panic!("{other:?}"),
    };
    assert_eq!(by(turns.fact(started(12))), own(), "refused: no mark");
    assert_eq!(
        by(turns.fact(started(14))),
        By::sidebar(),
        "the turn's start, read after its stream ended"
    );
    server.await.unwrap();
}

/// L7: Kai leaves the gateway (`Core::drop_requests` cancels the token) while lmgw has not
/// answered: the request ends with `gateway_left`, and lmgw's connection closes.
#[tokio::test]
async fn leaving_the_gateway_before_its_answer_ends_the_request_with_an_error() {
    let g = Gateway::bind().await;
    let creds = creds(&g.url);
    let left = CancellationToken::new();
    let leave = left.clone();
    let p: Proxy = Arc::new(move |Relay { request, reply }| {
        let creds = creds.clone();
        let answer = Box::pin(async move { relay::send(&creds, request.out, LIMITS).await });
        tokio::spawn(reply.run(answer, 1, None, left.clone()));
    });
    let (got_tx, got) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut s, _) = g.accept().await.unwrap();
        let _ = read_request(&mut s).await;
        let _ = got_tx.send(());
        // No answer; the client's end closes the connection.
        let mut rest = Vec::new();
        let _ = s.read_to_end(&mut rest).await;
    });
    let rig = Rig::with_sidebar(p).await;
    let mut c = rig.link(&[]).await;
    c.send(
        json!({"t": "req", "id": 3, "method": "GET", "path": "/chat/api/feed",
                  "read": "stream"}),
    )
    .await;
    got.await.unwrap();
    leave.cancel();
    let end = c.recv().await.unwrap();
    assert_eq!(
        (end["t"].as_str(), end["id"].as_u64(), end["seq"].as_u64()),
        (Some("end"), Some(3), Some(0))
    );
    assert_eq!(end["error"]["code"], "gateway_left", "{end}");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("lmgw's connection closed")
        .unwrap();
}
