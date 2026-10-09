//! No key and no tool arguments in the log at any level (WP8): a whole link at `KAI_LOG=trace`,
//! rmcp's and tungstenite's own traces asked for too.

use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;

use super::*;
use crate::logging::test_support::Capture;

const ARGUMENT: &str = "ARG-7d1f-correct-horse";
const RESULT_ARGUMENT: &str = "ARG-9e2a-battery-staple";

/// A link with a call that runs, one that is asked about and allowed, and one that is refused.
async fn session() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.list().await;
    let id = c
        .call("echo", json!({"text": RESULT_ARGUMENT}), owner(), 60_000)
        .await;
    c.answer(id, &mut Vec::new()).await;
    let id = c
        .call("poke", json!({"text": ARGUMENT}), key("script"), 60_000)
        .await;
    r.ask().await.reply.send(true).unwrap();
    c.answer(id, &mut Vec::new()).await;
    let id = c
        .call(
            "echo",
            json!({"text": 7, "extra": ARGUMENT}),
            owner(),
            60_000,
        )
        .await;
    c.answer(id, &mut Vec::new()).await;
    r.host.stop();
    c.closed_by_kai().await;
}

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
fn a_trace_log_carries_neither_the_key_nor_an_argument() {
    let _ = tracing_log::LogTracer::init();
    let secret = &KEY["lmgw-device-".len()..];

    // Without the cap rmcp traces whole messages, a call's arguments among them: the case the cap
    // exists for (if this fails, rmcp stopped tracing them; the cap and the test below still
    // stand).
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
        bare.text().contains(ARGUMENT),
        "rmcp no longer traces a call's arguments"
    );

    for spec in [
        "trace",
        "trace,rmcp=trace,tungstenite=trace,tokio_tungstenite=trace",
    ] {
        let out = Capture::default();
        logged(crate::logging::subscriber(spec, out.clone(), false));
        let text = out.text();
        for want in ["host: a call", "host: answered", "host: asking the owner"] {
            assert!(text.contains(want), "{spec}: '{want}' missing: {text}");
        }
        assert!(!text.contains(secret), "{spec}: the key is in the log");
        for arg in [ARGUMENT, RESULT_ARGUMENT] {
            assert!(!text.contains(arg), "{spec}: an argument is in the log");
        }
    }
}
