//! The realtime adapter against a scripted gateway behind the tests' device listener on loopback
//! (`crate::lmgw_link::test_pki`): a WebSocket server (tokio-tungstenite's own accept) for the bound
//! session, raw HTTP for `current`, the folders and the handshake's refusals.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::*;
use crate::lmgw_link::test_pki::{self, Gateway};
use crate::voice::link::{CHAT_THREAD_BOUND, ErrorFacts, ServerEvent, TurnDetection};

const WAIT: Duration = Duration::from_secs(10);

fn link(url: &str) -> RealtimeLink {
    link_with(
        url,
        RealtimeSettings {
            timeout: Duration::from_secs(2),
            keepalive: Duration::from_secs(15),
            send_queue: Duration::from_secs(30),
        },
    )
}

fn link_with(url: &str, settings: RealtimeSettings) -> RealtimeLink {
    let mut l = RealtimeLink::new(settings);
    l.set_credentials(Some(test_pki::creds(url)));
    l
}

/// The gateway's TLS stream (the device listener's).
type Stream = tokio_rustls::server::TlsStream<TcpStream>;

async fn listener() -> (Gateway, String) {
    let l = Gateway::bind().await;
    let url = l.url.clone();
    (l, url)
}

async fn next(events: &mut mpsc::UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("a session event")
        .expect("the session's task runs")
}

/// What the handshake carried: the path with its query, and an Authorization header (none: the
/// device's certificate authenticates it).
type Seen = (String, Option<String>);

/// Accepts one WebSocket and reports the handshake's path and Authorization header.
#[allow(clippy::result_large_err)] // tungstenite's callback type
async fn accept_ws(l: &Gateway) -> (WebSocketStream<Stream>, Seen) {
    let (s, _) = tokio::time::timeout(WAIT, l.accept())
        .await
        .expect("a connection")
        .unwrap();
    let (tx, rx) = oneshot::channel();
    let ws = tokio_tungstenite::accept_hdr_async(s, move |req: &Request, resp: Response| {
        let auth = req
            .headers()
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_owned());
        let _ = tx.send((req.uri().to_string(), auth));
        Ok(resp)
    })
    .await
    .unwrap();
    (ws, rx.await.unwrap())
}

async fn text(ws: &mut WebSocketStream<Stream>) -> Value {
    loop {
        match tokio::time::timeout(WAIT, ws.next())
            .await
            .expect("a frame")
            .expect("the socket is open")
            .unwrap()
        {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Close(_) => panic!("closed instead of a text frame"),
            _ => {}
        }
    }
}

/// Reads one HTTP request (head and body) from `s`.
async fn read_request(s: &mut Stream) -> (String, String) {
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
            let body = String::from_utf8(buf[at + 4..at + 4 + len].to_vec()).unwrap();
            return (head, body);
        }
    }
}

async fn answer(s: &mut Stream, status: &str, body: &str) {
    let r = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(r.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn a_session_moves_text_frames_both_ways_and_ends_with_the_servers_close() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut socket = link.open(812, true, tx);
    // Sent before the 101: they wait, in order.
    socket.send(ClientEvent::SessionUpdate {
        turn_detection: Some(TurnDetection::SemanticVad),
        half_duplex: false,
    });
    socket.send(ClientEvent::append_pcm16(&[1, 2, 3]));
    let (mut ws, (path, auth)) = accept_ws(&l).await;
    assert_eq!(path, "/v1/realtime?chat_thread=812");
    assert_eq!(auth, None, "the device's certificate authenticates it");
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    let first = text(&mut ws).await;
    assert_eq!(first["type"], "session.update");
    assert_eq!(
        first["session"]["audio"]["input"]["turn_detection"]["type"],
        "semantic_vad"
    );
    assert_eq!(text(&mut ws).await["type"], "input_audio_buffer.append");
    // Server events arrive typed; one that is no event is skipped.
    ws.send(Message::Text(
        json!({"type": "session.created", "session": {"audio": {"input": {"turn_detection": null}},
            "lmgw": {"resolved": {"chat_thread": {"id": 812, "title": "Kai"}}}}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text("not json".into())).await.unwrap();
    ws.send(Message::Text(
        json!({"type": "input_audio_buffer.speech_started", "audio_start_ms": 10})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    match next(&mut events).await {
        SessionEvent::Server(ServerEvent::SessionCreated(f)) => {
            assert_eq!(f.thread.map(|t| t.id), Some(812));
            assert_eq!(f.turn_detection, None);
        }
        other => panic!("not session.created: {other:?}"),
    }
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Server(ServerEvent::SpeechStarted)
    );
    socket.send(ClientEvent::Commit);
    assert_eq!(text(&mut ws).await["type"], "input_audio_buffer.commit");
    // A takeover: the server's close frame, its code and reason.
    ws.close(Some(CloseFrame {
        code: CloseCode::Library(4000),
        reason: "voice mode moved to the dashboard".into(),
    }))
    .await
    .unwrap();
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Closed {
            code: 4000,
            reason: "voice mode moved to the dashboard".into()
        }
    );
    assert!(
        tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap()
            .is_none(),
        "Closed is the last event"
    );
}

/// WP19 (M1): a tagged `response.create` carries its `event_id`, and an `error` echoing one comes
/// as `ErrorFor` with it; an `error` with no id (or an empty one) stays a plain server event.
#[tokio::test]
async fn a_tagged_event_carries_its_id_and_an_error_echoing_one_says_it() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut socket = link.open(812, true, tx);
    socket.send_tagged(ClientEvent::ResponseCreate, "kai_approvals_1".into());
    socket.send(ClientEvent::ResponseCreate);
    let (mut ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    assert_eq!(
        text(&mut ws).await,
        json!({"type": "response.create", "event_id": "kai_approvals_1"})
    );
    assert_eq!(text(&mut ws).await, json!({"type": "response.create"}));
    let error = |id: Value| {
        json!({"type": "error", "event_id": "event_9", "error": {"type": "invalid_request_error",
            "code": "conversation_already_has_active_response", "message": "busy", "param": null,
            "event_id": id}})
        .to_string()
    };
    for id in [json!("kai_approvals_1"), Value::Null, json!("")] {
        ws.send(Message::Text(error(id).into())).await.unwrap();
    }
    let facts = ErrorFacts {
        kind: "invalid_request_error".into(),
        code: Some("conversation_already_has_active_response".into()),
        message: "busy".into(),
    };
    assert_eq!(
        next(&mut events).await,
        SessionEvent::ErrorFor {
            event_id: "kai_approvals_1".into(),
            error: facts.clone(),
        }
    );
    for _ in 0..2 {
        assert_eq!(
            next(&mut events).await,
            SessionEvent::Server(ServerEvent::Error(facts.clone()))
        );
    }
}

#[tokio::test]
async fn the_clients_close_goes_after_what_was_sent_and_ends_once_lmgw_answers_it() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut socket = link.open(5, true, tx);
    let (mut ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    socket.send(ClientEvent::Truncate {
        item_id: "item_1".into(),
        content_index: 0,
        audio_end_ms: 1494,
    });
    socket.close();
    socket.send(ClientEvent::Commit);
    let t = text(&mut ws).await;
    assert_eq!(
        (t["type"].as_str(), t["audio_end_ms"].as_u64()),
        (Some("conversation.item.truncate"), Some(1494))
    );
    // The close comes next (nothing sent after it), and lmgw answers it after a while (its
    // journal drains first).
    match tokio::time::timeout(WAIT, ws.next()).await.unwrap() {
        // A normal close that says whose it is.
        Some(Ok(Message::Close(f))) => assert_eq!(
            f.map(|f| (u16::from(f.code), f.reason.as_str().to_owned())),
            Some((1000, super::CLOSE_REASON.to_owned()))
        ),
        other => panic!("not the close: {other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(events.try_recv().is_err(), "nothing before lmgw's answer");
    // lmgw answers (tungstenite's server queued it at the read above, echoing the close; a flush
    // sends it).
    ws.flush().await.unwrap();
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Closed {
            code: 1000,
            reason: super::CLOSE_REASON.into()
        }
    );
}

#[tokio::test]
async fn a_drop_without_a_close_frame_is_1006() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(5, true, tx);
    let (ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    drop(ws);
    match next(&mut events).await {
        SessionEvent::Closed { code, .. } => assert_eq!(code, CLOSE_DROPPED),
        other => panic!("not Closed: {other:?}"),
    }
}

#[tokio::test]
async fn a_refusal_before_the_101_reads_the_handshakes_error() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(12, true, tx);
    let (mut s, _) = l.accept().await.unwrap();
    let (head, _) = read_request(&mut s).await;
    assert!(head.starts_with("GET /v1/realtime?chat_thread=12 HTTP/1.1"));
    answer(
        &mut s,
        "404 Not Found",
        &json!({"error": {"message": "chat thread 12 not found", "type": "invalid_request_error",
            "param": null, "code": "chat_thread_not_found"}})
        .to_string(),
    )
    .await;
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Refused {
            status: 404,
            code: Some("chat_thread_not_found".into()),
            message: "chat thread 12 not found".into()
        }
    );
}

/// lmgw sends its bind's refusals chunked; tungstenite hands the body over as it came, so Kai joins
/// the chunks before it reads the code (found live against lmgw 25ef1cc: a 404 read as `http_404`).
#[tokio::test]
async fn a_chunked_refusal_before_the_101_reads_its_code() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(3, false, tx);
    let (mut s, _) = l.accept().await.unwrap();
    read_request(&mut s).await;
    let body =
        json!({"error": {"code": "chat_thread_not_found", "message": "there is no chat thread 3",
        "param": null, "type": "invalid_request_error"}})
        .to_string();
    let (a, b) = body.split_at(40);
    let r = format!(
        "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\n\
         transfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{a}\r\n{:x};ext=1\r\n{b}\r\n0\r\n\r\n",
        a.len(),
        b.len()
    );
    s.write_all(r.as_bytes()).await.unwrap();
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Refused {
            status: 404,
            code: Some("chat_thread_not_found".into()),
            message: "there is no chat thread 3".into()
        }
    );
}

/// Review KAI-4 K4-5: tungstenite hands over only the body bytes that came with the head; the
/// device link's WebSocket reads the rest (`tls::ws`), so a body written after the head still
/// carries lmgw's code. A body that never comes carries none (never a guessed one), and the voice
/// decides by the status, which always came.
#[tokio::test]
async fn a_refusal_whose_body_comes_after_the_head_is_read_by_its_code() {
    let body = json!({"error": {"code": "device_disabled", "message": "device 'kai' is disabled",
        "param": null, "type": "invalid_request_error"}})
    .to_string();
    let head = "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\n\
         transfer-encoding: chunked\r\nconnection: close\r\n\r\n";
    for late_body in [true, false] {
        let (l, url) = listener().await;
        let mut link = link(&url);
        let (tx, mut events) = mpsc::unbounded_channel();
        let _socket = link.open(3, false, tx);
        let (mut s, _) = l.accept().await.unwrap();
        read_request(&mut s).await;
        s.write_all(head.as_bytes()).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        if late_body {
            let _ = s
                .write_all(format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len()).as_bytes())
                .await;
        } else {
            let _ = s.shutdown().await;
        }
        let want = late_body.then(|| "device_disabled".to_owned());
        match next(&mut events).await {
            SessionEvent::Refused {
                status: 401, code, ..
            } if code == want => {}
            other => panic!("not a 401 with {want:?}: {other:?}"),
        }
    }
}

#[test]
fn dechunk_joins_chunks_and_leaves_what_does_not_read() {
    assert_eq!(
        super::dechunk(b"3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n").as_deref(),
        Some(&b"abcde"[..])
    );
    // What came, though the last chunk did not.
    assert_eq!(
        super::dechunk(b"3\r\nabc\r\n").as_deref(),
        Some(&b"abc"[..])
    );
    assert_eq!(super::dechunk(b"{\"error\": 1}"), None);
    assert_eq!(super::dechunk(b"5\r\nab"), None);
}

#[tokio::test]
async fn an_unreachable_gateway_is_a_refusal_that_says_why() {
    let (l, url) = listener().await;
    drop(l);
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(12, true, tx);
    match next(&mut events).await {
        SessionEvent::Refused {
            status: 0, message, ..
        } => assert!(message.starts_with("lmgw is not reachable: "), "{message}"),
        other => panic!("not Refused: {other:?}"),
    }
}

#[tokio::test]
async fn a_close_before_the_handshake_was_answered_gives_it_up() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut socket = link.open(12, true, tx);
    // The connection is taken and never answered.
    let (_held, _) = l.accept().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    socket.close();
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Closed {
            code: 1000,
            reason: "closed before lmgw answered the handshake".into()
        }
    );
}

#[tokio::test]
async fn current_posts_to_the_folder_and_reads_the_thread() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let pending = link.current(3, true);
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let (head, body) = read_request(&mut s).await;
        answer(
            &mut s,
            "200 OK",
            &json!({"thread": {"id": 41, "title": "Morgen", "voice_resolved": {},
                "last_message_at": 1760000000},
                "rolled_over": true, "reason": "requested", "note": "a new thread was asked for"})
            .to_string(),
        )
        .await;
        (head, body)
    });
    assert_eq!(
        pending.await,
        Ok(Current {
            thread: ThreadRef {
                id: 41,
                title: "Morgen".into()
            },
            rolled_over: true,
            last_message_at: Some(1_760_000_000),
        })
    );
    let (head, body) = server.await.unwrap();
    assert!(head.starts_with("POST /chat/api/folders/3/current HTTP/1.1"));
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "{head}"
    );
    assert_eq!(body, r#"{"new":true}"#);
}

#[tokio::test]
async fn currents_refusal_keeps_its_status_and_code() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let pending = link.current(3, false);
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        read_request(&mut s).await;
        answer(
            &mut s,
            "409 Conflict",
            &json!({"code": "not_ongoing", "message": "folder 'Kai' is not an ongoing conversation"})
                .to_string(),
        )
        .await;
    });
    let e = pending.await.unwrap_err();
    assert_eq!(
        e,
        Refusal {
            status: 409,
            code: Some("not_ongoing".into()),
            message: "folder 'Kai' is not an ongoing conversation".into()
        }
    );
    assert_eq!(
        e.to_string(),
        "folder 'Kai' is not an ongoing conversation (not_ongoing)"
    );
    let mut unpaired = link_unpaired(&url);
    let e = unpaired.current(3, false).await.unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (0, "Kai is not paired"));
}

fn link_unpaired(url: &str) -> RealtimeLink {
    let mut l = link(url);
    l.set_credentials(None);
    l
}

#[tokio::test]
async fn current_is_bounded_by_lmgw_timeout_s() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let pending = link.current(3, false);
    let _server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        read_request(&mut s).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let e = pending.await.unwrap_err();
    assert_eq!(e.status, 0);
    assert!(e.message.contains("within 2 s (lmgw.timeout_s)"), "{e}");
}

/// WP22: the profiles are read with `GET /chat/api/profiles`, and a folder's profile is written
/// with `defaults_patch` alone; the answer says whether the current thread took it.
#[tokio::test]
async fn the_profiles_are_read_and_a_folders_profile_is_patched() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let listed = link.profiles();
    let set = link.set_folder_profile(3, Some(2));
    let unset = link.set_folder_profile(3, None);
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (mut s, _) = l.accept().await.unwrap();
            let (head, body) = read_request(&mut s).await;
            let reply = if head.starts_with("GET ") {
                json!({"profiles": [{"id": 2, "name": "Calm"}], "default_profile_id": 2})
            } else {
                json!({"id": 3, "name": "Kai", "defaults": {"profile_id": 2},
                       "applied": {"thread_id": 9, "fields": ["profile_id"]}})
            };
            answer(&mut s, "200 OK", &reply.to_string()).await;
            seen.push((head, body));
        }
        seen
    });
    let list = listed.await.unwrap();
    assert_eq!(
        (list.profiles[0].name.as_str(), list.default_profile_id),
        ("Calm", Some(2))
    );
    let patched = set.await.unwrap();
    assert_eq!(patched.folder.defaults.profile_id, Some(2));
    assert_eq!(patched.applied.unwrap().thread_id, 9);
    unset.await.unwrap();
    let seen = server.await.unwrap();
    let mut posts: Vec<_> = seen
        .iter()
        .filter(|(h, _)| h.starts_with("POST "))
        .collect();
    posts.sort_by_key(|(_, b)| b.clone());
    assert!(
        posts
            .iter()
            .all(|(h, _)| h.starts_with("POST /chat/api/folders/3 "))
    );
    let bodies: Vec<&str> = posts.iter().map(|(_, b)| b.as_str()).collect();
    assert_eq!(
        bodies,
        [
            r#"{"defaults_patch":{"profile_id":2}}"#,
            r#"{"defaults_patch":{"profile_id":null}}"#
        ]
    );
    assert!(
        seen.iter()
            .any(|(h, _)| h.starts_with("GET /chat/api/profiles "))
    );
}

#[tokio::test]
async fn the_folders_are_listed_and_a_new_one_carries_only_its_name() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let listed = link.folders();
    let created = link.create_folder("Kai");
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = l.accept().await.unwrap();
            let (head, body) = read_request(&mut s).await;
            let reply = if head.starts_with("GET ") {
                json!({"folders": [
                    {"id": 3, "name": "Kai", "defaults": {"model_alias": "kai-chat"},
                     "ongoing": {"idle_minutes": 120, "current_thread_id": 9},
                     "archive_days": null, "purge_days": 365},
                    {"id": 4, "name": "Work", "defaults": {}, "ongoing": null}]})
            } else {
                json!({"id": 7, "name": "Kai", "defaults": {}, "ongoing": null})
            };
            answer(&mut s, "200 OK", &reply.to_string()).await;
            seen.push((head, body));
        }
        seen
    });
    let folders = listed.await.unwrap();
    assert_eq!(
        folders[0],
        FolderInfo {
            id: 3,
            name: "Kai".into(),
            model: Some("kai-chat".into()),
            idle_minutes: Some(120),
            archive_days: None,
            purge_days: Some(365),
            tool_labels: Vec::new(),
            profile_id: None,
        }
    );
    assert_eq!(folders[1].idle_minutes, None);
    assert_eq!(folders[1].model, None);
    let f = created.await.unwrap();
    assert_eq!((f.id, f.name.as_str(), f.idle_minutes), (7, "Kai", None));
    let seen = server.await.unwrap();
    let (get, _) = seen.iter().find(|(h, _)| h.starts_with("GET ")).unwrap();
    assert!(get.starts_with("GET /chat/api/folders HTTP/1.1"), "{get}");
    assert!(!get.to_ascii_lowercase().contains("authorization"), "{get}");
    let (post, body) = seen.iter().find(|(h, _)| h.starts_with("POST ")).unwrap();
    assert!(
        post.starts_with("POST /chat/api/folders HTTP/1.1"),
        "{post}"
    );
    // No model, no ongoing, no retention: those are the owner's (§4; a device's retention is
    // refused).
    assert_eq!(body, r#"{"name":"Kai"}"#);
}

/// Review KAI-2 K2-13: Kai's own bind asks lmgw not to take the thread over, and lmgw's 409 is
/// read with its code; the owner's bind takes over (no `takeover`).
#[tokio::test]
async fn kais_own_bind_never_takes_over() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(12, false, tx);
    let (mut s, _) = l.accept().await.unwrap();
    let (head, _) = read_request(&mut s).await;
    assert!(
        head.starts_with("GET /v1/realtime?chat_thread=12&takeover=never HTTP/1.1"),
        "{head}"
    );
    answer(
        &mut s,
        "409 Conflict",
        &json!({"error": {"message": "voice is in use on device 'phone'",
            "type": "invalid_request_error", "param": null, "code": "chat_thread_bound"}})
        .to_string(),
    )
    .await;
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Refused {
            status: 409,
            code: Some(CHAT_THREAD_BOUND.into()),
            message: "voice is in use on device 'phone'".into()
        }
    );
    let (tx, _events) = mpsc::unbounded_channel();
    let _socket = link.open(12, true, tx);
    let (_ws, (path, _)) = accept_ws(&l).await;
    assert_eq!(path, "/v1/realtime?chat_thread=12");
}

/// Review KAI-2 K2-6: the outgoing queue holds `voice.send_queue_s` of audio. Past it appends are
/// dropped, said once; control events always go, in order.
#[tokio::test]
async fn the_outgoing_queue_is_bounded_for_audio_only() {
    let (l, url) = listener().await;
    let mut link = link_with(
        &url,
        RealtimeSettings {
            timeout: Duration::from_secs(2),
            keepalive: Duration::from_secs(15),
            send_queue: Duration::from_secs(1),
        },
    );
    let (tx, mut events) = mpsc::unbounded_channel();
    let mut socket = link.open(5, true, tx);
    // Nobody answers the handshake yet: everything queues. 1 s is 50 chunks of 20 ms.
    for i in 0..60 {
        socket.send(ClientEvent::append_pcm16(&[i as i16; 480]));
    }
    socket.send(ClientEvent::Commit);
    socket.send(ClientEvent::ResponseCreate);
    match next(&mut events).await {
        SessionEvent::Note(n) => {
            assert!(n.contains("voice.send_queue_s") && n.contains("1 s"), "{n}")
        }
        other => panic!("not the note: {other:?}"),
    }
    assert!(events.try_recv().is_err(), "said once per overflow");
    let (mut ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    let mut appends = 0;
    loop {
        let t = text(&mut ws).await;
        match t["type"].as_str() {
            Some("input_audio_buffer.append") => appends += 1,
            Some("input_audio_buffer.commit") => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(appends, 50, "the bound's worth of audio");
    assert_eq!(text(&mut ws).await["type"], "response.create");
    // Written, the queue has room again.
    socket.send(ClientEvent::append_pcm16(&[7; 480]));
    assert_eq!(text(&mut ws).await["type"], "input_audio_buffer.append");
}

/// Review KAI-2 K2-6: a ping every `voice.keepalive_s`; a peer that answers nothing for two
/// intervals ends the session as a drop that says so, while one that reads (and so answers the
/// pings) keeps it.
#[tokio::test]
async fn a_dead_socket_is_found_by_the_keep_alive() {
    let settings = RealtimeSettings {
        timeout: Duration::from_secs(2),
        keepalive: Duration::from_millis(200),
        send_queue: Duration::from_secs(30),
    };
    // A peer that reads: pongs go back, the session stays.
    let (l, url) = listener().await;
    let mut link = link_with(&url, settings);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(5, true, tx);
    let (mut ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    let reading = tokio::spawn(async move {
        let mut pings = 0;
        let until = tokio::time::Instant::now() + Duration::from_millis(1200);
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(until, ws.next()).await {
            if matches!(m, Message::Ping(_)) {
                pings += 1;
            }
        }
        (pings, ws)
    });
    let (pings, _ws) = reading.await.unwrap();
    assert!(pings >= 3, "{pings} pings in 1.2 s at 200 ms");
    assert!(
        events.try_recv().is_err(),
        "a peer that answers keeps the session"
    );

    // A peer that reads nothing: no pong, and the session ends.
    let (l, url) = listener().await;
    let mut link = link_with(&url, settings);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(5, true, tx);
    let (_ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    let t0 = std::time::Instant::now();
    match next(&mut events).await {
        SessionEvent::Closed { code, reason } => {
            assert_eq!(code, CLOSE_DROPPED);
            assert!(
                reason.contains("voice.keepalive_s") && reason.contains("dead"),
                "{reason}"
            );
        }
        other => panic!("not Closed: {other:?}"),
    }
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
}

/// Review KAI-2 K2-25: a revocation's close (4003) reaches the session with its reason.
#[tokio::test]
async fn a_4003_close_is_passed_on_with_its_reason() {
    let (l, url) = listener().await;
    let mut link = link(&url);
    let (tx, mut events) = mpsc::unbounded_channel();
    let _socket = link.open(5, true, tx);
    let (mut ws, _) = accept_ws(&l).await;
    assert_eq!(next(&mut events).await, SessionEvent::Opened);
    ws.close(Some(CloseFrame {
        code: CloseCode::Library(4003),
        reason: "the device was disabled".into(),
    }))
    .await
    .unwrap();
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Closed {
            code: 4003,
            reason: "the device was disabled".into()
        }
    );
}

/// WP19: a thread's waiting calls, from `GET /chat/api/threads/{id}` (the messages'
/// `pending_approvals`; a message with none, or `null`, lists none).
#[test]
fn a_threads_waiting_calls_read_from_its_messages() {
    let body = json!({
        "thread": {"id": 7},
        "messages": [
            {"id": 1, "role": "user", "content": "x"},
            {"id": 2, "role": "assistant", "pending_approvals": null},
            {"id": 3, "role": "assistant", "pending_approvals": [
                {"approval_request_id": "mcpr_1", "server_label": "kai", "name": "a",
                 "arguments": "{}"},
                {"approval_request_id": "mcpr_2", "server_label": "kai", "name": "b",
                 "arguments": "{\"x\":1}"}
            ]}
        ]
    })
    .to_string();
    let calls = read_pending_approvals(200, &body).unwrap();
    let ids: Vec<&str> = calls
        .iter()
        .map(|c| c.approval_request_id.as_str())
        .collect();
    assert_eq!(ids, ["mcpr_1", "mcpr_2"]);
    assert_eq!(calls[1].arguments, "{\"x\":1}");
    let e =
        read_pending_approvals(404, r#"{"code":"not_found","message":"no thread"}"#).unwrap_err();
    assert_eq!(e.code, "not_found");
    assert!(read_pending_approvals(200, "{}").unwrap().is_empty());
}
