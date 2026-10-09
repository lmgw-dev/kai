//! The adapter against a loopback lmgw that answers like `/v1/chat/completions` with scripted turns
//! (tool calls, then a text) and lists the folders, and a fake desktop: the key, the alias and the
//! body on the wire; the static part a byte prefix of every request; the screenshots in the
//! delegate's requests only; K15 by the delegate's caller and the plan's one dialog; listing;
//! cancellation; nothing private in a trace log.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::host::confirm::ConfirmRequest;
use crate::host::policy::{Policy, SharedPolicy};
use crate::host::registry::Args;
use crate::host::settings::{Named, PolicyConfig, ToolState};
use crate::pairing::DeviceKey;

mod review;
mod screenshots;

const KEY: &str = "lmgw-device-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const WORDS: &str = "Kai, was steht da im Fenster? «UTTERANCE-4711»";
const QUESTION: &str = "what does the window say? QUESTION-0815";
const SHOT: &str = "iVBORw0KGgoSHOT-WINDOW";
const SCREEN: &str = "iVBORw0KGgoSHOT-SCREEN";

/// One request lmgw saw.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub path: String,
    pub auth: String,
    pub body: String,
}

/// A loopback lmgw: the folder list, and the scripted completion answers in order; with them
/// spent, a completion is never answered.
pub(crate) struct Lmgw {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// What `/v1/models` lists (nothing known of any alias, unless a test sets it).
    models: Arc<Mutex<Value>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Lmgw {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Lmgw {
    pub async fn start(folders: Value, turns: Vec<Value>) -> Self {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let turns = Arc::new(Mutex::new(VecDeque::from(turns)));
        let s = seen.clone();
        let models = Arc::new(Mutex::new(json!({"object": "list", "data": []})));
        let listed = models.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                let (seen, turns, folders) = (s.clone(), turns.clone(), folders.clone());
                let listed = listed.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let head_end = loop {
                        let mut chunk = [0u8; 4096];
                        let n = c.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let header = |name: &str| {
                        head.lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                            })
                            .unwrap_or_default()
                    };
                    let len: usize = header("content-length").parse().unwrap_or(0);
                    while buf.len() < head_end + len {
                        let mut chunk = [0u8; 65536];
                        let n = c.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    seen.lock().unwrap().push(Seen {
                        path: path.clone(),
                        auth: header("authorization"),
                        body: String::from_utf8_lossy(&buf[head_end..head_end + len]).into(),
                    });
                    let answer = if path == "/chat/api/folders" {
                        Some(folders.to_string())
                    } else if path == "/v1/models" {
                        Some(listed.lock().unwrap().to_string())
                    } else {
                        turns.lock().unwrap().pop_front().map(|t| t.to_string())
                    };
                    let Some(answer) = answer else {
                        std::future::pending::<()>().await;
                        return;
                    };
                    // A scripted turn that carries `_fallback` is answered under that header.
                    let fallback = serde_json::from_str::<Value>(&answer)
                        .ok()
                        .and_then(|v| v["_fallback"].as_str().map(str::to_owned))
                        .map(|a| format!("x-lmgw-fallback: {a}\r\n"))
                        .unwrap_or_default();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{fallback}\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    let _ = c.write_all(reply.as_bytes()).await;
                });
            }
        });
        Self {
            url,
            seen,
            models,
            task,
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Lists `data` at `/v1/models` from now on.
    fn list_models(&self, data: Value) {
        *self.models.lock().unwrap() = json!({"object": "list", "data": data});
    }

    pub fn completions(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.path == "/v1/chat/completions")
            .collect()
    }
}

pub(crate) fn folders(model: Option<&str>) -> Value {
    json!({"folders": [{
        "id": 7, "name": "Kai", "sort": 0,
        "defaults": {"model_alias": model},
        "ongoing": {"idle_minutes": 60},
        "archive_days": null, "purge_days": null,
        "created_at": "2026-10-09T00:00:00Z", "updated_at": "2026-10-09T00:00:00Z",
        "threads_active": 1, "threads_archived": 0
    }]})
}

pub(crate) fn tool_turn(calls: &[(&str, &str, Value)]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| {
            json!({"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}})
        })
        .collect();
    json!({"choices": [{"index": 0, "finish_reason": "tool_calls",
                        "message": {"role": "assistant", "content": null, "tool_calls": calls}}]})
}

pub(crate) fn text_turn(text: &str) -> Value {
    json!({"choices": [{"index": 0, "finish_reason": "stop",
                        "message": {"role": "assistant", "content": text}}]})
}

/// A desktop call: the tool, its caller, its argument lines.
type Ran = (String, Caller, Vec<String>);

/// The fake desktop: the read tools, recording who called which.
#[derive(Clone, Default)]
struct Desk {
    names: Vec<&'static str>,
    calls: Arc<Mutex<Vec<Ran>>>,
}

impl Desk {
    fn with(names: &[&'static str]) -> Self {
        Self {
            names: names.to_vec(),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<Ran> {
        self.calls.lock().unwrap().clone()
    }
}

impl Provider for Desk {
    fn entries(&self) -> Vec<Entry> {
        self.names
            .iter()
            .map(|n| {
                let (area, spec) = kai_desktop::contract::find(n).unwrap();
                let calls = self.calls.clone();
                Entry {
                    area: area.name,
                    spec,
                    handler: Arc::new(move |call: Call| {
                        let calls = calls.clone();
                        Box::pin(async move {
                            calls.lock().unwrap().push((
                                call.spec.name.to_owned(),
                                call.caller.clone(),
                                call.args.lines(call.spec),
                            ));
                            match call.spec.name {
                                "see_window" => Reply {
                                    blocks: vec![
                                        Block::Text("window w1 (editor), 640×480".into()),
                                        Block::Image {
                                            mime: "image/png".into(),
                                            base64: SHOT.into(),
                                        },
                                    ],
                                    error: false,
                                },
                                "see_screen" => Reply {
                                    blocks: vec![
                                        Block::Text("output DP-1, 1920×1080".into()),
                                        Block::Image {
                                            mime: "image/png".into(),
                                            base64: SCREEN.into(),
                                        },
                                    ],
                                    error: false,
                                },
                                "window_list" => Reply::text("w1 editor \"notes.txt\""),
                                other => Reply::text(format!("{other} read")),
                            }
                        })
                    }),
                    prepare: None,
                }
            })
            .collect()
    }
}

/// A dialog as shown: the tool, the caller, the lines, destructive.
type Shown = (String, String, Vec<String>, bool);

/// The owner's dialog: every request kept, answered `yes`.
struct Asked {
    yes: bool,
    /// The dialog cannot be shown: the reply is dropped.
    unavailable: bool,
    asked: Mutex<Vec<Shown>>,
    /// Each dialog's description and whether it showed the time left.
    described: Mutex<Vec<(String, bool)>>,
}

impl Asked {
    fn answering(yes: bool) -> Self {
        Self {
            yes,
            unavailable: false,
            asked: Mutex::default(),
            described: Mutex::default(),
        }
    }
}

impl Confirmer for Asked {
    fn ask(&self, req: ConfirmRequest) {
        self.asked.lock().unwrap().push((
            req.tool.clone(),
            req.caller.clone(),
            req.lines.clone(),
            req.destructive,
        ));
        self.described
            .lock()
            .unwrap()
            .push((req.description.clone(), req.deadline.is_some()));
        if !self.unavailable {
            let _ = req.reply.send(self.yes);
        }
    }
}

fn heard_words() -> Option<String> {
    Some(WORDS.to_owned())
}

struct Rig {
    tools: DelegateTools,
    /// The policy the delegates read live (the host's, in Kai).
    policy: SharedPolicy,
    desk: Desk,
    asked: Arc<Asked>,
    _creds: watch::Sender<Option<Credentials>>,
}

fn rig(url: &str, desk: Desk, settings: DelegateConfig, policy: PolicyConfig) -> Rig {
    rig_with(
        url,
        desk,
        settings,
        policy,
        Asked::answering(true),
        heard_words,
    )
}

fn rig_with(
    url: &str,
    desk: Desk,
    settings: DelegateConfig,
    policy: PolicyConfig,
    asked: Asked,
    utterance: fn() -> Option<String>,
) -> Rig {
    let (tx, creds) = watch::channel(Some(Credentials {
        url: url.to_owned(),
        key: DeviceKey::parse(KEY).unwrap(),
    }));
    let asked = Arc::new(asked);
    let policy: SharedPolicy = Arc::new(RwLock::new(Policy::new(&policy)));
    let tools = DelegateTools::new(Inner {
        desktop: Box::new(desk.clone()),
        policy: policy.clone(),
        confirmer: asked.clone(),
        creds,
        client: Some(reqwest::Client::builder().no_proxy().build().unwrap()),
        settings,
        folder_name: "Kai".into(),
        folder_id: None,
        lmgw_timeout: Duration::from_secs(5),
        utterance,
    });
    Rig {
        tools,
        policy,
        desk,
        asked,
        _creds: tx,
    }
}

fn entry(tools: &DelegateTools, name: &str) -> Entry {
    tools
        .entries()
        .into_iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} not listed"))
}

async fn call(tools: &DelegateTools, name: &str, args: Value, caller: Caller) -> Reply {
    call_with(tools, name, args, caller, CancellationToken::new()).await
}

async fn call_with(
    tools: &DelegateTools,
    name: &str,
    args: Value,
    caller: Caller,
    cancel: CancellationToken,
) -> Reply {
    let e = entry(tools, name);
    let call = Call::new(
        e.spec,
        Args::read(e.spec, args.as_object().cloned()).unwrap(),
        caller,
        cancel,
    );
    tokio::time::timeout(Duration::from_secs(10), (e.handler)(call))
        .await
        .expect("answered in time")
}

fn text(r: &Reply) -> String {
    match &r.blocks[..] {
        [Block::Text(t)] => t.clone(),
        other => panic!("one text block: {other:?}"),
    }
}

const LOOK_TOOLS: &[&str] = &[
    "desktop_context",
    "window_list",
    "see_window",
    "see_screen",
    "read_window",
    "read_selection",
];

/// The serialized body up to the end of its system message: the static part's bytes (the
/// dynamic messages follow it).
fn static_bytes(body: &str) -> &str {
    let end = r#""role":"system"}"#;
    let i = body.find(end).expect("a system message");
    &body[..i + end.len()]
}

#[tokio::test]
async fn look_goes_to_lmgw_under_the_key_on_the_conversations_alias_with_the_screenshot() {
    let lmgw = Lmgw::start(
        folders(Some("kai-chat")),
        vec![
            tool_turn(&[("c1", "see_screen", json!({}))]),
            text_turn("done: The editor shows a shopping list."),
            text_turn("partial: Only half is visible."),
        ],
    )
    .await;
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    let reply = call(
        &r.tools,
        "look",
        json!({"question": QUESTION, "window": "editor"}),
        Caller::Own,
    )
    .await;
    assert_eq!(
        (text(&reply), reply.error),
        (
            "done: The editor shows a shopping list.\n(used: see_screen)".to_owned(),
            false
        )
    );
    let seen = lmgw.seen();
    assert_eq!(
        seen[0].path, "/chat/api/folders",
        "the alias from the Kai folder"
    );
    for s in &seen {
        assert_eq!(s.auth, format!("Bearer {KEY}"), "Kai's device key");
    }
    let bodies: Vec<Value> = lmgw
        .completions()
        .iter()
        .map(|s| serde_json::from_str(&s.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    let b0 = &bodies[0];
    assert_eq!(b0["model"], "kai-chat");
    assert_eq!(b0["stream"], false);
    let names: Vec<&str> = b0["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "desktop_context",
            "window_list",
            "see_window",
            "see_screen",
            "read_window",
            "read_selection",
            "propose_plan"
        ]
    );
    // The first request: the system message, then the task: no conversation.
    let m0 = b0["messages"].as_array().unwrap();
    assert_eq!(m0.len(), 2);
    assert_eq!(m0[0]["role"], "system");
    let parts = m0[1]["content"].as_array().unwrap();
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(
        parts[1]["image_url"]["url"],
        format!("data:image/png;base64,{SHOT}"),
        "see_window's screenshot of the named window, in OpenAI's image format"
    );
    let last = parts.last().unwrap()["text"].as_str().unwrap();
    assert!(last.contains(&format!("question: {QUESTION}\nwindow: editor")));
    assert!(
        last.ends_with(&format!("The user's last words, verbatim:\n{WORDS}")),
        "{last}"
    );
    // The second: the call, its text, its image as a user message.
    let m1 = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(m1.len(), 5);
    assert_eq!(m1[2]["tool_calls"][0]["function"]["name"], "see_screen");
    assert_eq!(m1[3]["role"], "tool");
    assert_eq!(
        m1[3]["content"],
        "output DP-1, 1920×1080\n(its image follows in the next message)"
    );
    assert_eq!(m1[4]["role"], "user");
    assert_eq!(
        m1[4]["content"][1]["image_url"]["url"],
        format!("data:image/png;base64,{SCREEN}")
    );
    // The desktop's tools ran in process as Kai's own calls, the window handed on.
    let calls = r.desk.calls();
    assert_eq!(
        calls
            .iter()
            .map(|(t, c, a)| (t.as_str(), c.clone(), a.clone()))
            .collect::<Vec<_>>(),
        [
            ("window_list", Caller::Own, vec![]),
            ("see_window", Caller::Own, vec!["window: editor".to_owned()]),
            ("see_screen", Caller::Own, vec![]),
        ]
    );
    // The screenshots stay in the delegate's requests; the caller gets text.
    assert!(!text(&reply).contains("iVBOR"));

    // A second call with another question: the static part is byte for byte the same.
    let reply = call(
        &r.tools,
        "look",
        json!({"question": "anything else?"}),
        Caller::Own,
    )
    .await;
    assert_eq!(text(&reply), "partial: Only half is visible.");
    let all = lmgw.completions();
    assert_eq!(all.len(), 3);
    let (a, c) = (static_bytes(&all[0].body), static_bytes(&all[2].body));
    assert_eq!(a, c);
    assert_eq!(a, static_bytes(&all[1].body));
    assert!(a.starts_with(r#"{"model":"kai-chat","stream":false,"tools":["#));
    assert!(a.contains(r#""messages":[{"content":"You are Kai's look delegate. "#));
    for private in [QUESTION, "anything else", WORDS, SHOT, "notes.txt"] {
        assert!(!a.contains(private), "{private} in the static part");
    }
    assert!(r.asked.asked.lock().unwrap().is_empty(), "nothing asked");
}

#[tokio::test]
async fn a_delegates_calls_are_its_callers_under_k15_and_the_plan_is_confirmed_once() {
    let lmgw = Lmgw::start(
        folders(Some("kai-chat")),
        vec![
            tool_turn(&[(
                "p",
                "propose_plan",
                json!({"steps": [
                    {"tool": "see_window", "arguments": {"window": "editor"}},
                    {"tool": "read_window", "arguments": {"window": "editor"}}
                ]}),
            )]),
            tool_turn(&[
                ("a", "see_window", json!({"window": "editor"})),
                ("b", "read_window", json!({"window": "editor"})),
                ("c", "see_screen", json!({})),
            ]),
            text_turn("done: A shopping list."),
        ],
    )
    .await;
    let mut policy = PolicyConfig::default();
    policy.tools.insert("window_list".into(), ToolState::Deny);
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        policy,
    );
    let laptop = Caller::Named(Named {
        kind: "key".into(),
        name: "laptop".into(),
    });
    let reply = call(
        &r.tools,
        "look",
        json!({"question": QUESTION, "window": "editor"}),
        laptop.clone(),
    )
    .await;
    assert_eq!(
        text(&reply),
        "done: A shopping list.\n(used: see_window (editor), read_window (editor))"
    );
    // An untrusted caller's reads need a confirmation: none ran before the plan, the denied one
    // never, and only the plan's steps after it.
    let ran: Vec<(String, Caller)> = r.desk.calls().into_iter().map(|(t, c, _)| (t, c)).collect();
    assert_eq!(
        ran,
        [
            ("see_window".to_owned(), laptop.clone()),
            ("read_window".to_owned(), laptop.clone())
        ]
    );
    assert_eq!(
        *r.asked.asked.lock().unwrap(),
        [(
            "look (a plan)".to_owned(),
            "key 'laptop'".to_owned(),
            vec![
                "1. see_window (window: editor)".to_owned(),
                "2. read_window (window: editor)".to_owned()
            ],
            false
        )],
        "one dialog, naming the caller and the steps"
    );
    let bodies = lmgw.completions();
    let first: Value = serde_json::from_str(&bodies[0].body).unwrap();
    let task = first["messages"][1]["content"].as_str().unwrap();
    assert!(
        task.contains("## Open windows\n(not read: window_list is switched off"),
        "{task}"
    );
    assert!(task.contains("## The window\n(not read: see_window needs the owner's confirmation"));
    assert!(
        task.ends_with("The user's words are not known here (the call came from key 'laptop', whose conversation Kai does not hear)."),
        "{task}"
    );
    let third: Value = serde_json::from_str(&bodies[2].body).unwrap();
    let outside = third["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "c")
        .unwrap();
    assert!(
        outside["content"]
            .as_str()
            .unwrap()
            .contains("not a step of the confirmed plan")
    );
}

#[tokio::test]
async fn the_alias_setting_wins_and_a_missing_alias_names_the_setting() {
    let lmgw = Lmgw::start(folders(None), vec![text_turn("done: found it")]).await;
    let set = DelegateConfig {
        find: Some("small-model".into()),
        ..DelegateConfig::default()
    };
    let r = rig(
        &lmgw.url,
        Desk::with(&["window_list", "find_files"]),
        set,
        PolicyConfig::default(),
    );
    let reply = call(
        &r.tools,
        "find",
        json!({"what": "the invoice"}),
        Caller::Own,
    )
    .await;
    assert_eq!(text(&reply), "done: found it");
    let seen = lmgw.seen();
    assert_eq!(seen.len(), 1, "no folder lookup: {seen:?}");
    let body: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["model"], "small-model");
    assert!(
        !seen[0].body.contains("image_url"),
        "find reads no screenshot"
    );

    // No setting, and the folder has no model: failed, naming what to set.
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    assert!(reply.error);
    assert_eq!(
        text(&reply),
        "failed: the Kai conversation's folder 'Kai' has no model, so the look delegate has no \
         alias; set delegate.look or the folder's model"
    );
    assert!(r.desk.calls().is_empty(), "nothing read without a model");
}

#[tokio::test]
async fn a_model_error_is_a_failed_answer_naming_lmgws_words() {
    let lmgw = Lmgw::start(folders(Some("kai-chat")), vec![]).await;
    // A refusal: a 503 with OpenAI's error shape, from a server that answers it.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let server = tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let mut buf = vec![0u8; 65536];
            let _ = c.read(&mut buf).await;
            let body =
                r#"{"error":{"code":"gpu_hold","message":"local models are paused","type":"x"}}"#;
            let _ = c
                .write_all(
                    format!(
                        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
        }
    });
    drop(lmgw);
    let set = DelegateConfig {
        look: Some("kai-chat".into()),
        ..DelegateConfig::default()
    };
    let r = rig(&url, Desk::with(LOOK_TOOLS), set, PolicyConfig::default());
    let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    server.abort();
    assert!(reply.error);
    assert_eq!(
        text(&reply),
        "failed: the delegate's model did not answer: lmgw answered 503 gpu_hold: local models \
         are paused"
    );
}

#[tokio::test]
async fn a_delegate_is_listed_where_the_desktop_serves_what_it_needs() {
    let listed = |desk: &[&'static str], policy: PolicyConfig| {
        let r = rig(
            "http://127.0.0.1:1",
            Desk::with(desk),
            DelegateConfig::default(),
            policy,
        );
        r.tools
            .entries()
            .iter()
            .map(|e| (e.area, e.spec.name))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        listed(LOOK_TOOLS, PolicyConfig::default()),
        [("delegate", "look")]
    );
    assert_eq!(
        listed(&["recent_files"], PolicyConfig::default()),
        [("delegate", "find")]
    );
    assert_eq!(
        listed(&["see_window", "find_files"], PolicyConfig::default()),
        [("delegate", "look"), ("delegate", "find")]
    );
    assert!(listed(&["window_list", "read_window"], PolicyConfig::default()).is_empty());
    let mut off = PolicyConfig::default();
    off.tools.insert("see_window".into(), ToolState::Off);
    assert!(
        listed(LOOK_TOOLS, off).is_empty(),
        "a tool the owner switched off is no delegate's either"
    );
    // The contract keeps K22's rules, and both are synchronous (K26).
    for spec in kai_desktop::contract::delegate::AREA.tools {
        assert!(kai_desktop::contract::check(spec).is_empty());
        assert_eq!(spec.class, Class::Sync);
        assert!(spec.hints.read_only);
    }
}

#[tokio::test]
async fn a_cancel_ends_the_task_while_the_model_is_asked() {
    // No scripted turn: the completion is never answered.
    let lmgw = Lmgw::start(folders(Some("kai-chat")), vec![]).await;
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let seen = lmgw.seen.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            if seen
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.path == "/v1/chat/completions")
            {
                stop.cancel();
                let _ = tx.send(());
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let reply = call_with(
        &r.tools,
        "look",
        json!({"question": "q"}),
        Caller::Own,
        cancel,
    )
    .await;
    rx.recv().await.unwrap();
    assert_eq!(
        (text(&reply), reply.error),
        ("the look task was cancelled".to_owned(), true)
    );
}

/// No key, prompt, utterance, argument or screenshot in a trace log (K25, WP8's rule).
#[test]
fn a_trace_log_carries_nothing_private() {
    let _ = tracing_log::LogTracer::init();
    // While a single dispatcher exists tracing works a callsite's interest out from the default of
    // the thread that registers it (`has_just_one`), so a callsite another test's thread registered
    // first would stay disabled here; a second live dispatcher makes it ask every dispatcher.
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let out = crate::logging::test_support::Capture::default();
    tracing::subscriber::with_default(
        crate::logging::subscriber("trace", out.clone(), false),
        || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let lmgw = Lmgw::start(
                        folders(Some("kai-chat")),
                        vec![
                            tool_turn(&[("c1", "see_screen", json!({}))]),
                            text_turn("done: RESULT-31337"),
                        ],
                    )
                    .await;
                    let r = rig(
                        &lmgw.url,
                        Desk::with(LOOK_TOOLS),
                        DelegateConfig::default(),
                        PolicyConfig::default(),
                    );
                    let reply = call(
                        &r.tools,
                        "look",
                        json!({"question": QUESTION, "window": "editor"}),
                        Caller::Own,
                    )
                    .await;
                    assert!(text(&reply).contains("RESULT-31337"));
                });
        },
    );
    let text = out.text();
    for want in [
        "delegate: a task",
        "delegate: a call answered",
        "delegate: answered",
    ] {
        assert!(text.contains(want), "'{want}' missing: {text}");
    }
    let secret = &KEY["lmgw-device-".len()..];
    for private in [
        secret,
        QUESTION,
        "UTTERANCE-4711",
        "RESULT-31337",
        SHOT,
        SCREEN,
        "You are Kai's",
        "window: editor",
    ] {
        assert!(!text.contains(private), "{private} is in the log");
    }
}

/// The delegates over a desktop that has every tool (the host's contract test: the tests' own
/// desktop provider probes nothing, so it lists no delegate).
pub(crate) fn full_entries() -> Vec<Entry> {
    struct Full;
    impl Provider for Full {
        fn entries(&self) -> Vec<Entry> {
            crate::desktop::tests::full_entries()
        }
    }
    let (_tx, creds) = watch::channel(None);
    DelegateTools::new(Inner {
        desktop: Box::new(Full),
        policy: Arc::default(),
        confirmer: Arc::new(Asked::answering(false)),
        creds,
        client: None,
        settings: DelegateConfig::default(),
        folder_name: "Kai".into(),
        folder_id: None,
        lmgw_timeout: Duration::from_secs(5),
        utterance: heard_words,
    })
    .entries()
}
