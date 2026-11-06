//! kai-firefox WP8b's real run of the sidebar (kai-firefox design §8, §13): its page in a headless
//! Firefox, through the real `kai-browser-host`, Kai's browser server (the peer check at its
//! default scopes, the sidebar's admit list, its site question) and Kai's sidebar relay over lmgw's
//! device link with a device Kai paired itself, to a dev lmgw whose chat model is a mock that
//! echoes. `scripts/live-check-sidebar.sh` (`just live-check-sidebar`) prepares it and runs it;
//! ignored by default.
//!
//! What is Kai's own here and what is not: the server, the admit list, the relay
//! (`relay::send`, `Reply::run` with `browser.sidebar_ahead_mb`'s window) and the pairing
//! (`voice::lmgw_live_tests::device`: Kai's enrolment and the owner's Confirm) are the code Kai
//! runs; the core's part around the relay (the paired state, the turn marking, leaving the gateway)
//! is not, and has its own tests (WP8a). kai-firefox's `tests/real-run.mjs` drives the page.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::browser::links::Links;
use crate::browser::server::{Server, serve};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::browser::sidebar::{Proxy, Relay};
use crate::browser::sites::SiteRules;
use crate::host::registry::Changes;
use crate::lmgw_link::proxy::{Limits, relay};
use crate::voice::lmgw_live_tests::{Lmgw, device, env};

const THREAD: &str = "Lamps for the ridge (live check)";
const SAY: &str = "Which lamp clips onto a backpack?";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/live-check-sidebar.sh (a dev lmgw, kai-firefox with its sidebar page built, Firefox 159)"]
async fn sidebar_live() {
    let out = std::path::PathBuf::from(env("KAI_LIVE_OUT"));
    let firefox = env("KAI_FIREFOX");
    let mut lmgw = Lmgw::new();
    lmgw.start().await;

    // The mock chat model, and a thread of the owner's on it, which the device sees.
    let chat = env("KAI_LIVE_MOCK_CHAT");
    lmgw.op(
        "upstream_set",
        json!({"action": "create", "name": "mock-chat", "protocol": "llama_cpp",
            "base_url": format!("http://127.0.0.1:{chat}/v1")}),
    )
    .await;
    lmgw.op(
        "model_set",
        json!({"action": "create", "alias": "kai-chat", "upstream": "mock-chat",
            "upstream_model": "triage"}),
    )
    .await;
    let made = lmgw
        .owner(
            "POST",
            "/chat/api/threads",
            Some(json!({"model_alias": "kai-chat"})),
        )
        .await;
    let thread = made["id"]
        .as_i64()
        .or_else(|| made["thread"]["id"].as_i64())
        .unwrap_or_else(|| panic!("the new thread's id: {made}"));
    // A new thread is "New chat" until named (`POST …/title`).
    lmgw.owner(
        "POST",
        &format!("/chat/api/threads/{thread}/title"),
        Some(json!({"title": THREAD})),
    )
    .await;

    // Kai's device, paired as Kai pairs: its own key, the owner's Confirm.
    let dev = device(&lmgw, "Kai sidebar live check", json!({})).await;
    let creds = dev.creds.clone();
    let limits = Limits::from_config(&crate::config::WindowConfig::default());
    let ahead = BrowserConfig::default().sidebar_ahead_mb;
    let proxy: Proxy = Arc::new(move |Relay { request, reply }| {
        let creds = creds.clone();
        let answer = Box::pin(async move { relay::send(&creds, request.out, limits).await });
        tokio::spawn(reply.run(answer, ahead, None, Default::default()));
    });
    // The fixture site's hosts get the rules a user might set: 127.0.0.1 read, so the strip says so.
    let sites: crate::browser::sidebar::site::Rules = Arc::new(|| {
        SiteRules::read(&Sites {
            act: Vec::new(),
            read: vec!["127.0.0.1".into()],
            none: Vec::new(),
        })
        .unwrap()
    });
    let socket = out.join("browser.sock");
    let (changes, _changed) = Changes::new();
    let server = tokio::spawn(serve(
        socket.clone(),
        Server {
            links: Links::new(changes),
            config_file: None,
            scopes: BrowserConfig::default().peer_scopes,
            peer_of: crate::browser::peer::proc_peer,
            sidebar: Some(proxy),
            sites: Some(sites),
        },
    ));

    // kai-firefox's driver.
    let run = tokio::time::timeout(
        Duration::from_secs(240),
        tokio::process::Command::new("node")
            .arg(format!("{firefox}/tests/real-run.mjs"))
            .args(["--socket", &socket.to_string_lossy()])
            .args(["--thread", THREAD, "--say", SAY])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("kai-firefox's real run ends within 4 minutes")
    .expect("node runs");
    let steps: Vec<Value> = String::from_utf8_lossy(&run.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let report = steps
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(out.join("report.txt"), format!("{report}\n")).unwrap();
    println!("{report}");
    eprintln!("{}", String::from_utf8_lossy(&run.stderr));
    let step = |name: &str| {
        steps
            .iter()
            .find(|s| s["step"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name} step in the run: {report}"))
    };
    assert!(
        steps.iter().any(|s| s["done"] == true),
        "the run did not finish: {report}"
    );
    assert_eq!(step("strip")["rule"], "read", "the strip's rule is Kai's");
    assert!(
        step("threads")["titles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == THREAD)
    );
    assert!(
        step("reply")["text"].as_str().unwrap().contains(SAY),
        "the mock echoes what was said: {report}"
    );
    assert_eq!(step("violations")["count"], 0);

    // lmgw's side: the message and the reply are in the thread, as Kai's device sent them.
    let messages = lmgw.messages(thread).await;
    let mut text = messages
        .iter()
        .map(|(_, role, content, _)| (role.as_str(), content.as_str()));
    assert!(
        text.any(|(role, c)| role == "user" && c == SAY),
        "the sent message in lmgw's thread: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|(_, role, c, _)| role == "assistant" && c.contains(SAY)),
        "the reply in lmgw's thread: {messages:?}"
    );
    server.abort();
    lmgw.stop().await;
}
