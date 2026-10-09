//! Review KAI-2 K2-1: a bound session at `KAI_LOG=trace`, tungstenite's own trace asked for too,
//! leaves neither the device key nor the microphone's audio in the log. The realtime adapter runs
//! against a WebSocket server on loopback, the appends go through [`Session::send`] as the voice
//! sends them, and the server answers with audio deltas and an event Kai cannot read.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::fmt::MakeWriter;

use super::link::{ClientEvent, SessionEvent, ThreadRef, VoiceLink};
use super::session::Session;
use crate::lmgw_link::{Credentials, RealtimeLink, RealtimeSettings};
use crate::logging::test_support::Capture;
use crate::pairing::DeviceKey;
use crate::pairing::tests::KEY;

const WAIT: Duration = Duration::from_secs(10);

/// A sample whose PCM16 bytes make a base64 run no log line has by chance.
const SAMPLE: i16 = 0x1d5b;

/// One session: the handshake with the bearer, appends both before and after `session.created`,
/// audio deltas and an unreadable event from the server, then the server's close.
async fn session() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let mut link = RealtimeLink::new(
        RealtimeSettings {
            timeout: Duration::from_secs(2),
            keepalive: Duration::from_secs(15),
            send_queue: Duration::from_secs(30),
        },
        reqwest::Client::builder().no_proxy().build().unwrap(),
    );
    link.set_credentials(Some(Credentials {
        url,
        key: DeviceKey::parse(KEY).unwrap(),
    }));
    let (tx, rx) = mpsc::unbounded_channel();
    let socket = link.open(812, true, tx);
    let thread = ThreadRef {
        id: 812,
        title: "Kai".into(),
    };
    let mut s = Session::new(socket, rx, thread, tokio::time::Instant::now());
    s.send(ClientEvent::append_pcm16(&[SAMPLE; 480]));

    let (stream, _) = tokio::time::timeout(WAIT, l.accept())
        .await
        .unwrap()
        .unwrap();
    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    let audio = lmgw_client::base64::encode(&lmgw_client::base64::pcm16_le_bytes(&[SAMPLE; 480]));
    for ev in [
        json!({"type": "session.created", "session": {"audio": {"input": {"turn_detection": null}}}}),
        json!({"type": "response.output_audio.delta", "response_id": "r1", "item_id": "i1",
               "delta": audio}),
        json!({"type": "lmgw.newer.audio", "audio": audio}),
    ] {
        ws.send(Message::Text(ev.to_string().into())).await.unwrap();
    }
    for _ in 0..3 {
        s.send(ClientEvent::append_pcm16(&[SAMPLE; 480]));
    }
    s.send(ClientEvent::Commit);
    let mut appends = 0;
    while appends < 4 {
        match tokio::time::timeout(WAIT, ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(t) if t.contains("input_audio_buffer.append") => appends += 1,
            _ => {}
        }
    }
    let mut seen = 0;
    while seen < 4 {
        match tokio::time::timeout(WAIT, s.events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            SessionEvent::Opened | SessionEvent::Server(_) => seen += 1,
            other => panic!("{other:?}"),
        }
    }
    ws.close(None).await.unwrap();
    while let Some(ev) = tokio::time::timeout(WAIT, s.events.recv()).await.unwrap() {
        if matches!(ev, SessionEvent::Closed { .. }) {
            break;
        }
    }
}

/// Runs [`session`] on a runtime of this thread, logging into `subscriber`.
fn logged(subscriber: impl tracing::Subscriber + Send + Sync + 'static) {
    tracing::subscriber::with_default(subscriber, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(session());
    });
}

#[test]
fn a_trace_log_carries_neither_the_key_nor_audio() {
    // tungstenite logs through the `log` crate: its bridge into tracing, as `logging::init`
    // installs it (records go to this thread's subscriber).
    let _ = tracing_log::LogTracer::init();
    let audio = lmgw_client::base64::encode(&lmgw_client::base64::pcm16_le_bytes(&[SAMPLE; 16]));
    let audio = &audio[..20];
    let secret = &KEY["lmgw-device-".len()..];

    // Without the cap, tungstenite writes the handshake's Authorization header: the case the cap
    // exists for (if this ever fails, tungstenite stopped tracing the request head; the cap and
    // the test below still stand).
    let bare = Capture::default();
    let w = bare.clone();
    logged(
        tracing_subscriber::fmt()
            .with_env_filter("trace")
            .with_writer(move || w.make_writer())
            .with_ansi(false)
            .finish(),
    );
    assert!(
        bare.text().contains(secret),
        "tungstenite no longer traces the request head"
    );

    for spec in [
        "trace",
        "trace,tungstenite=trace,tokio_tungstenite=trace,hyper=trace",
    ] {
        let out = Capture::default();
        logged(crate::logging::subscriber(spec, out.clone(), false));
        let text = out.text();
        assert!(
            text.contains("voice: send") && text.contains("samples=480"),
            "{spec}: Kai's own trace was captured: {text}"
        );
        assert!(!text.contains(secret), "{spec}: the key is in the log");
        assert!(!text.contains(audio), "{spec}: audio is in the log");
        assert!(
            !text.contains("\"audio\":\""),
            "{spec}: an append is in the log"
        );
    }
}
