//! The bridge's rules (K18, WP12) against a fake `/mcp` ([`Fake`]) and a scripted owner
//! ([`Owner`]): what a view may call and read, K15's confirmation for a call not marked
//! `readOnlyHint`, the refusals of `_meta.ui`, the bounds, and the end of a view.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;

use super::*;

/// A `/mcp` that answers by method and keeps what it was asked.
#[derive(Default)]
pub(super) struct Fake {
    pub(super) asked: Mutex<Vec<(String, Value)>>,
    closed: Mutex<bool>,
}

impl Upstream for Arc<Fake> {
    fn request(&self, method: &'static str, params: Value) -> Pending<Result<Value, McpError>> {
        self.asked
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        Box::pin(async move {
            match method {
                "tools/call" if params["name"] == "fx__fails" => Err(McpError::Rpc {
                    code: -32601,
                    message: "fx__fails is out of this key's tool scope".into(),
                    data: None,
                }),
                "tools/call"
                    if matches!(params["name"].as_str(), Some("fx__slow" | "fx__wait")) =>
                {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(json!({}))
                }
                "tools/call" => Ok(json!({
                    "content": [{"type": "text", "text": "done"}],
                    "structuredContent": {"called": params["name"]}
                })),
                "resources/read" => Ok(json!({"contents": [{
                    "uri": params["uri"], "mimeType": "text/plain", "text": "data"
                }]})),
                "resources/list" => Ok(json!({"resources": [
                    {"uri": "ui://fx__card/main", "name": "card"},
                    {"uri": "file://fx__/notes.txt", "name": "notes"},
                    {"uri": "ui://fx__bar__card", "name": "fx__bar's, or fx's bar__card"},
                    {"uri": "ui://fx___c", "name": "fx_'s"},
                    {"uri": "ui://fxx__card", "name": "fxx's"},
                    {"uri": "ui://other__card", "name": "other's"},
                    {"uri": "urn:x:y", "name": "a bare server's"},
                    {"name": "no uri"},
                ]})),
                "resources/templates/list" => Ok(json!({"resourceTemplates": [
                    {"uriTemplate": "file://fx__/{path}", "name": "files"},
                    {"uriTemplate": "db://fx__bar__{x}", "name": "fx__bar's"},
                    {"uriTemplate": "ui://other__{x}", "name": "other's"},
                ]})),
                _ => Err(McpError::Link("no".into())),
            }
        })
    }

    fn close(&self) -> Pending<()> {
        *self.closed.lock().unwrap() = true;
        Box::pin(async {})
    }
}

/// What a dialog showed: the tool, the caller, the argument lines, destructive.
type Shown = (String, String, Vec<String>, bool);

/// The owner: answers each dialog with the next of `answers` (none left: the dialog stays open),
/// and keeps what each showed.
pub(super) struct Owner {
    answers: Mutex<Vec<bool>>,
    shown: Mutex<Vec<Shown>>,
    open: Mutex<Vec<oneshot::Sender<bool>>>,
}

impl Owner {
    pub(super) fn new(answers: &[bool]) -> Arc<Owner> {
        Arc::new(Owner {
            answers: Mutex::new(answers.iter().rev().copied().collect()),
            shown: Mutex::default(),
            open: Mutex::default(),
        })
    }
}

impl Confirmer for Owner {
    fn ask(&self, req: ConfirmRequest) {
        self.shown.lock().unwrap().push((
            req.tool.clone(),
            req.caller.clone(),
            req.lines.clone(),
            req.destructive,
        ));
        match self.answers.lock().unwrap().pop() {
            Some(a) => {
                let _ = req.reply.send(a);
            }
            None => self.open.lock().unwrap().push(req.reply),
        }
    }
}

fn tools() -> Value {
    json!({"tools": [
        {"name": "fx__read", "description": "Read it.\nMore.", "annotations": {"readOnlyHint": true}},
        {"name": "fx__bump", "description": "Bump it.", "annotations": {"readOnlyHint": false, "destructiveHint": false}},
        {"name": "fx__wipe", "description": "Wipe it."},
        {"name": "fx__hidden", "description": "App only.", "annotations": {"readOnlyHint": true},
         "_meta": {"ui": {"visibility": ["app"]}}},
        {"name": "fx__model", "description": "Model only.", "annotations": {"readOnlyHint": true},
         "_meta": {"ui": {"visibility": "model"}}},
        {"name": "fx__fails", "annotations": {"readOnlyHint": true}},
        {"name": "fx__slow", "description": "Slow.", "annotations": {"readOnlyHint": false}},
        {"name": "fx__wait", "description": "Slow, read-only.", "annotations": {"readOnlyHint": true}},
        {"name": "fx___under", "annotations": {"readOnlyHint": true}},
        {"name": "fx__bar__x", "description": "fx__bar's x, or fx's bar__x.", "annotations": {"readOnlyHint": true}},
        {"name": "other__read", "annotations": {"readOnlyHint": true}},
        {"name": "read", "annotations": {"readOnlyHint": true}},
    ]})
}

fn bridge(owner: Arc<Owner>) -> (Bridge, Arc<Fake>) {
    bridge_of(owner, 16)
}

/// A bridge whose view may have `calls` requests in flight.
fn bridge_of(owner: Arc<Owner>, calls: usize) -> (Bridge, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    (
        Bridge::new(
            Arc::new(fake.clone()),
            Rules::of_list("fx", &tools()),
            owner,
            Bound {
                bytes: 4096,
                says: "overlay.app_max_mb (test)".into(),
            },
            Calls {
                max: calls,
                says: "overlay.app_max_calls (test)".into(),
            },
            (480, 360),
        ),
        fake,
    )
}

fn req(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

pub(super) fn call(id: u64, name: &str) -> String {
    req(
        id,
        "tools/call",
        json!({"name": name, "arguments": {"n": 1, "s": "a\nb"}}),
    )
}

pub(super) fn code(v: &Value) -> i64 {
    v["error"]["code"].as_i64().unwrap_or(0)
}

#[test]
fn the_target_is_a_prefixed_servers_ui_uri() {
    assert_eq!(
        target(" fx ", "ui://fx__card/main"),
        Ok(Target {
            server: "fx".into(),
            uri: "ui://fx__card/main".into()
        })
    );
    for (server, uri) in [
        ("", "ui://fx__card/main"),
        ("f x", "ui://f x__card"),
        ("fx", "https://fx__card/main"),
        ("fx", "ui://card/main"),
        ("fx", "ui://other__card/main"),
        ("fx", "ui://fxx__card"),
        // Another server's under an overlapping prefix (fx_, fx__bar): not fx's view.
        ("fx", "ui://fx___card"),
        ("fx", "ui://fx__bar__card/main"),
        // lmgw's own labels.
        ("lmgw", "ui://lmgw__card"),
        ("docs", "ui://docs__card"),
        ("kb", "ui://kb__card"),
        ("lmgw__x", "ui://lmgw__x__card"),
    ] {
        assert!(target(server, uri).is_err(), "{server} {uri}");
    }
    let e = target("fx", "ui://card/main").unwrap_err();
    assert!(e.contains("ui://fx__…"), "{e}");
    let e = target("kb", "ui://kb__card").unwrap_err();
    assert!(e.contains("lmgw's own label"), "{e}");
    assert!(
        target("kbx", "ui://kbx__card").is_ok() && target("fx", "ui://fx__card/a__b").is_ok(),
        "only the labels themselves, and only the authority"
    );
}

#[test]
fn the_rules_take_only_the_servers_tools_for_views() {
    let r = Rules::of_list("fx", &tools());
    let names: Vec<&str> = r.tools.iter().map(|t| t.exposed.as_str()).collect();
    assert_eq!(
        names,
        [
            "fx__read",
            "fx__bump",
            "fx__wipe",
            "fx__hidden",
            "fx__model",
            "fx__fails",
            "fx__slow",
            "fx__wait"
        ],
        "another server's, bare tools, and names that could be another server's under an \
         overlapping prefix (fx_, fx__bar) are not the view's"
    );
    let read = r.tool("read").unwrap();
    assert!(read.read_only && !read.destructive);
    assert_eq!(read.description, "Read it.");
    let bump = r.tool("bump").unwrap();
    assert!(!bump.read_only && !bump.destructive);
    assert!(
        r.tool("wipe").unwrap().destructive,
        "MCP's default: destructive"
    );
    assert!(r.tool("hidden").unwrap().app, "app-only is for views");
    assert!(r.tool("model").unwrap_err().contains("leaves out \"app\""));
    for name in ["other__read", "fx__read", "nothing", "", "_under", "bar__x"] {
        assert!(r.tool(name).is_err(), "{name}");
    }
    assert!(
        r.tool("bar__x")
            .unwrap_err()
            .contains("starts with _ or holds __")
    );
}

#[test]
fn a_view_reads_only_its_own_namespace() {
    let r = Rules::of_list("fx", &json!({}));
    assert_eq!(r.resource("ui://card/data").unwrap(), "ui://fx__card/data");
    assert_eq!(
        r.resource("ui://fx__card/data").unwrap(),
        "ui://fx__card/data"
    );
    // No authority of its own: lmgw's namespace is `<prefix>__` alone (api §7.2).
    assert_eq!(r.resource("file:///x").unwrap(), "file://fx__/x");
    assert_eq!(r.resource("ui:///x").unwrap(), "ui://fx__/x");
    assert_eq!(
        r.resource("ui://card/a__b").unwrap(),
        "ui://fx__card/a__b",
        "only the authority counts"
    );
    for uri in [
        "urn:x:y",
        "data:text/plain,x",
        "card/x",
        "://x",
        "",
        "ui://",
        // Another server's under an overlapping prefix (fx_ or fx__other).
        "ui://other__card/x",
        "ui://fx__other__card/x",
        "ui://_c",
        "ui://fx___c",
        "ui://a__b?q",
        // lmgw's scheme rule: a letter first.
        "1ab://x",
        "+a://x",
        ".a://x",
        "-a://x",
    ] {
        assert!(r.resource(uri).is_err(), "{uri}");
    }
    assert!(
        r.resource("ui://other__card/x")
            .unwrap_err()
            .contains("starts with _ or holds __")
    );
    // The lists' entries in the server's own terms, only its own.
    assert_eq!(r.own("ui://fx__card/x").as_deref(), Some("ui://card/x"));
    assert_eq!(r.own("file://fx__/x").as_deref(), Some("file:///x"));
    for uri in [
        "ui://fx__bar__c",
        "ui://fx___c",
        "ui://fxx__c",
        "ui://other__c",
        "ui://card",
        "urn:fx__x",
        "1ab://fx__c",
    ] {
        assert_eq!(r.own(uri), None, "{uri}");
    }
}

#[test]
fn the_document_must_be_html_and_its_asks_are_refused_by_name() {
    let read = json!({"contents": [{
        "uri": "ui://fx__card/main", "mimeType": "text/html;profile=mcp-app", "text": "<p>hi</p>",
        "_meta": {"ui": {
            "csp": {"connectDomains": ["https://api.example.com"], "resourceDomains": [],
                    "frameDomains": ["https://youtube.com", "https://vimeo.com"]},
            "permissions": {"camera": {}, "clipboardWrite": {}},
            "prefersBorder": true
        }}
    }]});
    let r = resource(&read, "ui://fx__card/main").unwrap();
    assert_eq!(r.html, "<p>hi</p>");
    assert_eq!(
        r.notes,
        [
            "refused: csp connectDomains https://api.example.com (a view reaches no network)",
            "refused: csp frameDomains https://youtube.com, https://vimeo.com (a view reaches no \
             network)",
            "refused: permissions camera, clipboardWrite (a view gets none)"
        ]
    );
    let blob = json!({"contents": [{"uri": "ui://fx__c", "mimeType": "text/html",
        "blob": lmgw_client::base64::encode(b"<b>x</b>")}]});
    assert_eq!(resource(&blob, "ui://fx__c").unwrap().html, "<b>x</b>");
    assert!(resource(&blob, "ui://fx__c").unwrap().notes.is_empty());
    let plain = json!({"contents": [{"uri": "ui://fx__c", "mimeType": "text/plain", "text": "x"}]});
    assert!(
        resource(&plain, "ui://fx__c")
            .unwrap_err()
            .contains("not an MCP Apps view")
    );
    assert!(resource(&json!({"contents": []}), "ui://fx__c").is_err());
    let empty = json!({"contents": [{"uri": "ui://fx__c", "mimeType": "text/html", "text": " "}]});
    assert!(resource(&empty, "ui://fx__c").is_err());
}

#[tokio::test]
async fn initialize_and_ping_are_answered_and_the_rest_is_not_offered() {
    let (b, fake) = bridge(Owner::new(&[]));
    let init = b
        .answer(&req(
            1,
            "ui/initialize",
            json!({"protocolVersion": "2026-01-26"}),
        ))
        .await
        .unwrap();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2026-01-26");
    let caps = &init["result"]["hostCapabilities"];
    assert!(caps["serverTools"].is_object() && caps["serverResources"].is_object());
    assert!(caps.get("openLinks").is_none() && caps.get("sandbox").is_none());
    assert_eq!(
        init["result"]["hostContext"]["containerDimensions"],
        json!({"width": 480, "height": 360})
    );
    assert_eq!(
        b.answer(&req(2, "ping", json!({}))).await.unwrap()["result"],
        json!({})
    );
    for m in [
        "ui/open-link",
        "ui/message",
        "ui/update-model-context",
        "tools/list",
        "sampling/createMessage",
    ] {
        let a = b.answer(&req(3, m, json!({}))).await.unwrap();
        assert_eq!(code(&a), -32601, "{m}");
    }
    // Notifications and responses get no answer.
    for n in [
        json!({"jsonrpc": "2.0", "method": "ui/notifications/initialized"}),
        json!({"jsonrpc": "2.0", "method": "ui/notifications/size-changed", "params": {"width": 9, "height": 9}}),
        json!({"jsonrpc": "2.0", "id": 9, "result": {}}),
        json!([1, 2]),
    ] {
        assert_eq!(b.answer(&n.to_string()).await, None, "{n}");
    }
    assert_eq!(b.answer("not json").await, None);
    assert!(
        fake.asked.lock().unwrap().is_empty(),
        "nothing reached lmgw"
    );
}

#[tokio::test]
async fn a_read_only_call_runs_and_a_state_changing_one_asks_the_owner() {
    let owner = Owner::new(&[true, false]);
    let (b, fake) = bridge(owner.clone());
    let a = b.answer(&call(1, "read")).await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "fx__read");
    assert!(
        owner.shown.lock().unwrap().is_empty(),
        "read-only: nobody asked"
    );
    let a = b.answer(&call(2, "bump")).await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "fx__bump");
    let a = b.answer(&call(3, "wipe")).await.unwrap();
    assert_eq!(code(&a), -32000);
    assert_eq!(a["error"]["message"], "the owner denied fx__wipe");
    let shown = owner.shown.lock().unwrap().clone();
    assert_eq!(
        shown,
        [
            (
                "fx__bump".to_owned(),
                "the view of app 'fx'".to_owned(),
                vec!["n: 1".to_owned(), "s: \"a\\nb\"".to_owned()],
                false
            ),
            (
                "fx__wipe".to_owned(),
                "the view of app 'fx'".to_owned(),
                vec!["n: 1".to_owned(), "s: \"a\\nb\"".to_owned()],
                true
            )
        ]
    );
    let asked: Vec<Value> = fake
        .asked
        .lock()
        .unwrap()
        .iter()
        .map(|(_, p)| p["name"].clone())
        .collect();
    assert_eq!(
        asked,
        [json!("fx__read"), json!("fx__bump")],
        "the denied call never ran"
    );
    let (_, p) = fake.asked.lock().unwrap()[0].clone();
    assert_eq!(p["arguments"], json!({"n": 1, "s": "a\nb"}));
}

#[tokio::test]
async fn calls_outside_the_views_server_never_reach_lmgw() {
    let owner = Owner::new(&[]);
    let (b, fake) = bridge(owner.clone());
    for name in ["other__read", "fx__read", "model", "nothing"] {
        let a = b.answer(&call(1, name)).await.unwrap();
        assert_eq!(code(&a), -32602, "{name}");
    }
    let a = b
        .answer(&req(
            2,
            "tools/call",
            json!({"name": "read", "arguments": [1]}),
        ))
        .await
        .unwrap();
    assert_eq!(code(&a), -32602);
    let a = b.answer(&req(3, "tools/call", json!({}))).await.unwrap();
    assert_eq!(code(&a), -32602);
    let a = b
        .answer(&req(4, "resources/read", json!({"uri": "urn:x"})))
        .await
        .unwrap();
    assert_eq!(code(&a), -32602);
    assert!(fake.asked.lock().unwrap().is_empty());
    assert!(owner.shown.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lmgws_errors_pass_on_and_reads_answer_in_the_views_terms() {
    let (b, fake) = bridge(Owner::new(&[]));
    let a = b.answer(&call(1, "fails")).await.unwrap();
    assert_eq!(code(&a), -32601);
    assert_eq!(
        a["error"]["message"],
        "fx__fails is out of this key's tool scope"
    );
    let a = b
        .answer(&req(2, "resources/read", json!({"uri": "ui://card/data"})))
        .await
        .unwrap();
    assert_eq!(a["result"]["contents"][0]["uri"], "ui://card/data");
    assert_eq!(a["result"]["contents"][0]["text"], "data");
    let asked = fake.asked.lock().unwrap().clone();
    assert_eq!(asked[1].1, json!({"uri": "ui://fx__card/data"}));
}

/// The last of the three bounds (Kai's app script answers such a message `-32600`, the GTK side
/// drops it unread): the bridge reads nothing of it.
#[tokio::test]
async fn a_message_over_the_bound_is_not_read() {
    let (b, fake) = bridge(Owner::new(&[]));
    let big = req(
        7,
        "tools/call",
        json!({"name": "read", "arguments": {"x": "y".repeat(5000)}}),
    );
    assert_eq!(b.answer(&big).await, None);
    assert!(fake.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_lists_are_the_views_own_servers_in_its_own_terms() {
    let (b, fake) = bridge(Owner::new(&[]));
    let a = b
        .answer(&req(1, "resources/list", json!({})))
        .await
        .unwrap();
    assert_eq!(
        a["result"]["resources"],
        json!([
            {"uri": "ui://card/main", "name": "card"},
            {"uri": "file:///notes.txt", "name": "notes"},
        ]),
        "neither another server's (other, fxx, fx_, fx__bar) nor a bare one's"
    );
    let a = b
        .answer(&req(2, "resources/templates/list", json!({"cursor": "c1"})))
        .await
        .unwrap();
    assert_eq!(
        a["result"]["resourceTemplates"],
        json!([{"uriTemplate": "file:///{path}", "name": "files"}])
    );
    let asked = fake.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            ("resources/list".to_owned(), json!({})),
            (
                "resources/templates/list".to_owned(),
                json!({"cursor": "c1"})
            )
        ],
        "a cursor passes on, nothing else"
    );
    // A URI from the list reads back as listed.
    let a = b
        .answer(&req(
            3,
            "resources/read",
            json!({"uri": "file:///notes.txt"}),
        ))
        .await
        .unwrap();
    assert_eq!(a["result"]["contents"][0]["uri"], "file:///notes.txt");
    assert_eq!(
        fake.asked.lock().unwrap()[2].1,
        json!({"uri": "file://fx__/notes.txt"})
    );
}

/// Waits (up to 1 s) until `n` dialogs are open.
async fn dialogs(owner: &Owner, n: usize) {
    for _ in 0..100 {
        if owner.open.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_view_has_one_confirmation_open_at_a_time() {
    let owner = Owner::new(&[]);
    let (b, fake) = bridge(owner.clone());
    let b = Arc::new(b);
    let first = {
        let b = b.clone();
        tokio::spawn(async move { b.answer(&call(1, "bump")).await })
    };
    dialogs(&owner, 1).await;
    // Every further call that needs the owner, while that one is open: refused at once.
    for (id, name) in [(2, "wipe"), (3, "bump"), (4, "slow")] {
        let a = b.answer(&call(id, name)).await.unwrap();
        assert_eq!(code(&a), -32000, "{name}");
        assert!(
            a["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("a confirmation is already open"),
            "{a}"
        );
    }
    // A read-only one still runs.
    let a = b.answer(&call(5, "read")).await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "fx__read");
    assert_eq!(owner.shown.lock().unwrap().len(), 1, "one dialog only");
    // Answered: the next may ask.
    let _ = owner.open.lock().unwrap().pop().unwrap().send(true);
    let a = first.await.unwrap().unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "fx__bump");
    let next = {
        let b = b.clone();
        tokio::spawn(async move { b.answer(&call(6, "wipe")).await })
    };
    dialogs(&owner, 1).await;
    assert_eq!(owner.shown.lock().unwrap().len(), 2);
    let _ = owner.open.lock().unwrap().pop().unwrap().send(false);
    assert_eq!(code(&next.await.unwrap().unwrap()), -32000);
    let ran: Vec<Value> = fake
        .asked
        .lock()
        .unwrap()
        .iter()
        .map(|(_, p)| p["name"].clone())
        .collect();
    assert_eq!(ran, [json!("fx__read"), json!("fx__bump")]);
}

#[tokio::test]
async fn a_flood_opens_one_dialog_and_at_most_the_bound_of_requests() {
    let owner = Owner::new(&[]);
    let (b, fake) = bridge_of(owner.clone(), 3);
    let ends = b.spawn();
    // Five calls that need the owner: one dialog, the other four refused at once.
    for id in 1..=5 {
        ends.to_bridge.send(call(id, "bump")).unwrap();
    }
    let mut refused = Vec::new();
    for _ in 0..4 {
        let a: Value = serde_json::from_str(&ends.from_bridge.recv().await.unwrap()).unwrap();
        assert_eq!(code(&a), -32000, "{a}");
        refused.push(a["id"].as_u64().unwrap());
    }
    refused.sort_unstable();
    assert_eq!(owner.shown.lock().unwrap().len(), 1, "one dialog");
    assert_eq!(refused.len(), 4);
    // The refused ones' permits go right after their answers were taken.
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Two read-only calls that wait for lmgw: with the dialog's, three in flight, the bound.
    ends.to_bridge.send(call(6, "wait")).unwrap();
    ends.to_bridge.send(call(7, "wait")).unwrap();
    for _ in 0..100 {
        if fake.asked.lock().unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // One more request is refused naming the key, a notification over it is dropped.
    ends.to_bridge
        .send(json!({"jsonrpc": "2.0", "method": "ui/notifications/size-changed"}).to_string())
        .unwrap();
    ends.to_bridge.send(req(8, "ping", json!({}))).unwrap();
    let a: Value = serde_json::from_str(&ends.from_bridge.recv().await.unwrap()).unwrap();
    assert_eq!(a["id"], 8);
    assert_eq!(code(&a), -32000);
    assert!(
        a["error"]["message"]
            .as_str()
            .unwrap()
            .contains("3 requests in flight already (overlay.app_max_calls (test))"),
        "{a}"
    );
    // The dialog answered: room again.
    let _ = owner.open.lock().unwrap().pop().unwrap().send(false);
    let a: Value = serde_json::from_str(&ends.from_bridge.recv().await.unwrap()).unwrap();
    assert_eq!(
        (a["id"].as_u64(), code(&a)),
        (Some(1), -32000),
        "the owner's no"
    );
    for _ in 0..100 {
        ends.to_bridge.send(req(9, "ping", json!({}))).unwrap();
        let a: Value = serde_json::from_str(&ends.from_bridge.recv().await.unwrap()).unwrap();
        if a.get("result").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        fake.asked.lock().unwrap().len(),
        2,
        "only the two waits reached lmgw"
    );
}

#[test]
fn the_teardown_is_kais_one_request_to_the_view() {
    let t: Value = serde_json::from_str(&teardown("hidden")).unwrap();
    assert_eq!(
        t,
        json!({"jsonrpc": "2.0", "id": "kai-teardown", "method": "ui/resource-teardown",
               "params": {"reason": "hidden"}})
    );
}

/// An open cancelled after lmgw gave it a session (the show's deadline, the caller gone) ends
/// that session: lmgw keeps none Kai forgot.
#[tokio::test]
async fn a_cancelled_open_ends_its_session() {
    use crate::lmgw_link::mcp::fake::{Reply, serve};
    let f = serve(Arc::new(|m, _| match m {
        "resources/read" => Reply::Hang,
        _ => Reply::Ok(json!({})),
    }))
    .await;
    let apps = Apps {
        creds: watch::channel(Some(Credentials {
            url: f.url.clone(),
            key: crate::pairing::DeviceKey::from_store("lmgw-device-test".into()),
        }))
        .1,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        connect: Duration::from_secs(5),
        bound: Bound {
            bytes: 1 << 20,
            says: "overlay.app_max_mb (test)".into(),
        },
        calls: Calls {
            max: 16,
            says: "overlay.app_max_calls (test)".into(),
        },
        confirm: Owner::new(&[]),
        policy: Default::default(),
        lists: Default::default(),
        screen: Default::default(),
    };
    let open = apps.open("fx", "ui://fx__card", (480, 360));
    assert!(
        tokio::time::timeout(Duration::from_millis(500), open)
            .await
            .is_err(),
        "the read hangs: the open is cancelled"
    );
    for _ in 0..200 {
        if f.seen.lock().unwrap().iter().any(|s| s.http == "DELETE") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let seen = f.seen.lock().unwrap().clone();
    let delete = seen.iter().find(|s| s.http == "DELETE");
    assert_eq!(
        delete.and_then(|s| s.session.as_deref()),
        Some("s1"),
        "the session is ended: {:?}",
        seen.iter()
            .map(|s| (&s.http, &s.method))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_views_end_stops_its_calls_closes_a_dialog_and_ends_the_session() {
    let owner = Owner::new(&[]);
    let (b, fake) = bridge(owner.clone());
    let ends = b.spawn();
    ends.to_bridge.send(call(1, "read")).unwrap();
    let a: Value = serde_json::from_str(&ends.from_bridge.recv().await.unwrap()).unwrap();
    assert_eq!(a["id"], 1);
    // A call that waits for the owner, then the view goes.
    ends.to_bridge.send(call(2, "bump")).unwrap();
    for _ in 0..100 {
        if !owner.open.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let reply = owner
        .open
        .lock()
        .unwrap()
        .pop()
        .expect("the dialog is open");
    let Ends {
        to_bridge,
        from_bridge,
    } = ends;
    drop(to_bridge);
    assert!(
        from_bridge.recv().await.is_err(),
        "no answer after the view went"
    );
    assert!(
        reply.is_closed(),
        "the dialog's answer is no longer awaited"
    );
    assert!(*fake.closed.lock().unwrap(), "the /mcp session ends");
}

/// A policy as `[policy]` would read: `trusted` callers, `states` per tool.
pub(super) fn policy(trusted: &[&str], states: &[(&str, ToolState)]) -> SharedPolicy {
    let mut c = crate::host::settings::PolicyConfig {
        trusted_callers: trusted.iter().map(|t| (*t).to_owned()).collect(),
        ..Default::default()
    };
    for (tool, state) in states {
        c.tools.insert((*tool).to_owned(), *state);
    }
    Arc::new(std::sync::RwLock::new(Policy::new(&c)))
}

/// Decided by the owner, 2026-10-09: a view's calls run as `app:<server>`, which
/// `policy.trusted_callers` can name; `policy.tools.<server>__<name>` decides first.
#[test]
fn a_views_calls_are_decided_for_app_server() {
    let rules = Rules::of_list("fx", &tools());
    let (read, bump) = (rules.tool("read").unwrap(), rules.tool("bump").unwrap());
    let fx = caller("fx");
    assert_eq!(fx.describe(), "app 'fx'");
    let get = |p: &SharedPolicy, t: &ViewTool| decide(&p.read().unwrap(), t, &fx);
    let untrusted = policy(&[], &[]);
    assert_eq!(get(&untrusted, read), Decision::Allow, "read-only runs");
    assert!(matches!(get(&untrusted, bump), Decision::Confirm { .. }));
    // Another kind of the same name, or another app, is not this app.
    for other in [policy(&["key:fx"], &[]), policy(&["app:other"], &[])] {
        assert!(matches!(get(&other, bump), Decision::Confirm { .. }));
    }
    let trusted = policy(&["app:fx"], &[]);
    assert_eq!(
        get(&trusted, bump),
        Decision::Allow,
        "a trusted app is not asked"
    );
    // Destructive (MCP's default for a tool not read-only): a trusted app is still asked, unless
    // policy.tools allows it (a draft).
    let wipe = rules.tool("wipe").unwrap();
    assert!(
        matches!(get(&trusted, wipe), Decision::Confirm { why } if why.contains("destructive")),
        "a destructive tool asks even a trusted app"
    );
    assert_eq!(
        get(
            &policy(&["app:fx"], &[("fx__wipe", ToolState::Allow)]),
            wipe
        ),
        Decision::Allow
    );
    // Per tool, for any caller.
    let p = policy(
        &["app:fx"],
        &[
            ("fx__read", ToolState::Deny),
            ("fx__bump", ToolState::Confirm),
            ("fx__wipe", ToolState::Off),
        ],
    );
    assert_eq!(
        get(&p, read),
        Decision::Deny(
            "fx__read is switched off for Kai's views (policy.tools.fx__read = \"deny\")".into()
        )
    );
    assert!(
        matches!(get(&p, bump), Decision::Confirm { .. }),
        "confirm asks even a trusted app"
    );
    assert!(matches!(get(&p, wipe), Decision::Deny(w) if w.ends_with("= \"off\")")));
    let allow = policy(&[], &[("fx__bump", ToolState::Allow)]);
    assert_eq!(get(&allow, bump), Decision::Allow);
    // Such names are not "unknown tools" at the host's start (they are an app's).
    let offered = std::collections::HashSet::new();
    assert!(p.read().unwrap().unknown_names(&offered).is_empty());
}

#[tokio::test]
async fn a_trusted_apps_view_calls_without_a_dialog_and_a_denied_tool_never_reaches_lmgw() {
    let owner = Owner::new(&[]);
    let (b, fake) = bridge(owner.clone());
    let b = b.with_policy(policy(&["app:fx"], &[("fx__wipe", ToolState::Deny)]));
    let a = b.answer(&call(1, "bump")).await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "fx__bump");
    let a = b.answer(&call(2, "wipe")).await.unwrap();
    assert_eq!(code(&a), -32000);
    assert!(
        a["error"]["message"]
            .as_str()
            .unwrap()
            .contains("policy.tools.fx__wipe"),
        "{a}"
    );
    assert!(owner.shown.lock().unwrap().is_empty(), "nobody was asked");
    let names: Vec<Value> = fake
        .asked
        .lock()
        .unwrap()
        .iter()
        .map(|(_, p)| p["name"].clone())
        .collect();
    assert_eq!(names, [json!("fx__bump")]);
}

async fn next(ends: &Ends) -> Value {
    let text = tokio::time::timeout(Duration::from_secs(5), ends.from_bridge.recv())
        .await
        .expect("a message within 5 s")
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn initialized() -> String {
    json!({"jsonrpc": "2.0", "method": "ui/notifications/initialized"}).to_string()
}

/// A view shown for a call in Kai's conversation: `tool-input` once it said `initialized` and the
/// call runs, then `tool-result`; `ui/initialize` names the tool.
#[tokio::test]
async fn a_calls_view_gets_its_input_once_it_runs_then_its_result() {
    let (b, _) = bridge(Owner::new(&[]));
    // A Gemini call id (a thought signature rides in it): hundreds of characters, kept whole.
    let long_id = format!("call_{}", "Q2lTaWduYXR1cmU".repeat(40));
    let (tx, progress) = watch::channel(Progress::default());
    let mut args = Map::new();
    args.insert("city".into(), json!("Berlin"));
    let ends = b
        .with_run(ToolRun {
            arguments: args,
            tool: Some(json!({"name": "show", "inputSchema": {"type": "object"}})),
            call_id: Some(long_id.clone()),
            progress,
        })
        .spawn();
    ends.to_bridge
        .send(req(
            1,
            "ui/initialize",
            json!({"protocolVersion": "2026-01-26"}),
        ))
        .unwrap();
    let init = next(&ends).await;
    assert_eq!(
        init["result"]["hostContext"]["toolInfo"],
        json!({"id": long_id, "tool": {"name": "show", "inputSchema": {"type": "object"}}})
    );
    // Nothing unasked before the view says it is ready, nor before the call is known to run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(ends.from_bridge.is_empty());
    ends.to_bridge.send(initialized()).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        ends.from_bridge.is_empty(),
        "a call that may wait for an approval: no input yet"
    );
    tx.send_modify(|p| p.runs = true);
    let input = next(&ends).await;
    assert_eq!(
        input,
        json!({"jsonrpc": "2.0", "method": "ui/notifications/tool-input",
               "params": {"arguments": {"city": "Berlin"}}})
    );
    tx.send_replace(Progress::ended(
        true,
        Outcome::Result {
            result: json!({"content": [{"type": "text", "text": "sunny"},
                                       {"type": "image", "data": "AAAA", "mimeType": "image/png"}],
                           "structuredContent": {"temp": 21}, "isError": false}),
            ui: None,
        },
    ));
    let result = next(&ends).await;
    assert_eq!(result["method"], "ui/notifications/tool-result");
    assert_eq!(
        result["params"],
        json!({"content": [{"type": "text", "text": "sunny"},
                           {"type": "image", "data": "AAAA", "mimeType": "image/png"}],
               "structuredContent": {"temp": 21}, "isError": false}),
        "the result as the frame carried it"
    );
    assert!(result.get("id").is_none(), "a notification");
}

/// The result is what tells Kai a call ran (lmgw's frames today): input and result together.
#[tokio::test]
async fn a_result_brings_the_input_with_it() {
    let (b, _) = bridge(Owner::new(&[]));
    let (tx, progress) = watch::channel(Progress::default());
    let ends = b
        .with_run(ToolRun {
            arguments: Map::new(),
            tool: None,
            call_id: None,
            progress,
        })
        .spawn();
    ends.to_bridge.send(initialized()).unwrap();
    tx.send_replace(Progress::ended(
        false,
        Outcome::Result {
            result: json!({"content": []}),
            ui: None,
        },
    ));
    assert_eq!(next(&ends).await["method"], "ui/notifications/tool-input");
    assert_eq!(next(&ends).await["method"], "ui/notifications/tool-result");
}

#[tokio::test]
async fn a_call_that_ends_without_its_result_is_cancelled_to_the_view() {
    // It ran (its input went), then its turn was stopped.
    let (b, _) = bridge(Owner::new(&[]));
    let (tx, progress) = watch::channel(Progress::default());
    let ends = b
        .with_run(ToolRun {
            arguments: Map::new(),
            tool: None,
            call_id: None,
            progress,
        })
        .spawn();
    // The end before the view is ready waits for it.
    tx.send_replace(Progress::ended(
        true,
        Outcome::Cancelled("the turn was stopped".into()),
    ));
    ends.to_bridge.send(initialized()).unwrap();
    assert_eq!(next(&ends).await["method"], "ui/notifications/tool-input");
    assert_eq!(
        next(&ends).await,
        json!({"jsonrpc": "2.0", "method": "ui/notifications/tool-cancelled",
               "params": {"reason": "the turn was stopped"}})
    );
    // It never ran (declined while it waited): the cancel alone, no input.
    let (b, _) = bridge(Owner::new(&[]));
    let mut args = Map::new();
    args.insert("secret".into(), json!("s3cr3t"));
    let (tx, progress) = watch::channel(Progress::default());
    let ends = b
        .with_run(ToolRun {
            arguments: args,
            tool: None,
            call_id: None,
            progress,
        })
        .spawn();
    ends.to_bridge.send(initialized()).unwrap();
    tx.send_replace(Progress::ended(
        false,
        Outcome::Cancelled("the call was declined by the dashboard".into()),
    ));
    assert_eq!(
        next(&ends).await,
        json!({"jsonrpc": "2.0", "method": "ui/notifications/tool-cancelled",
               "params": {"reason": "the call was declined by the dashboard"}})
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(ends.from_bridge.is_empty(), "no input after the cancel");
    // Nobody following the call any more is a cancel too, without an input it never had.
    let (b, _) = bridge(Owner::new(&[]));
    let (tx, progress) = watch::channel(Progress::default());
    let ends = b
        .with_run(ToolRun {
            arguments: Map::new(),
            tool: None,
            call_id: None,
            progress,
        })
        .spawn();
    ends.to_bridge.send(initialized()).unwrap();
    drop(tx);
    assert_eq!(
        next(&ends).await["method"],
        "ui/notifications/tool-cancelled"
    );
    // overlay_app's own view (no run) gets nothing unasked.
    let (b, _) = bridge(Owner::new(&[]));
    let ends = b.spawn();
    ends.to_bridge.send(initialized()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(ends.from_bridge.is_empty());
}

fn creds(url: &str) -> watch::Receiver<Option<Credentials>> {
    watch::channel(Some(Credentials {
        url: url.to_owned(),
        key: crate::pairing::DeviceKey::from_store("lmgw-device-test".into()),
    }))
    .1
}

fn apps_at(url: &str) -> Apps {
    Apps {
        creds: creds(url),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        connect: Duration::from_secs(5),
        bound: Bound {
            bytes: 1 << 20,
            says: "overlay.app_max_mb (test)".into(),
        },
        calls: Calls {
            max: 16,
            says: "overlay.app_max_calls (test)".into(),
        },
        confirm: Owner::new(&[]),
        policy: Default::default(),
        lists: Default::default(),
        screen: Default::default(),
    }
}

/// `/mcp` with one tool that has a view and one that has none.
fn apps_mcp() -> crate::lmgw_link::mcp::fake::Answer {
    use crate::lmgw_link::mcp::fake::Reply;
    Arc::new(|m, p| match m {
        "tools/list" => Reply::Ok(json!({"tools": [
            {"name": "fx__show", "annotations": {"readOnlyHint": true},
             "_meta": {"ui": {"resourceUri": "ui://fx__card/main"}}},
            {"name": "fx__plain", "annotations": {"readOnlyHint": true}},
            {"name": "fx__old", "_meta": {"ui/resourceUri": "ui://fx__card/old"}},
        ]})),
        "resources/read" => Reply::Ok(json!({"contents": [{
            "uri": p["uri"], "mimeType": "text/html;profile=mcp-app", "text": "<p>card</p>"
        }]})),
        _ => Reply::Ok(json!({})),
    })
}

/// The view of a call in Kai's conversation: the tool list says whether it has one (before the
/// read), the result frame's names open it straight away; a tool with none ends the session.
#[tokio::test]
async fn a_calls_view_is_found_through_the_tool_list() {
    let f = crate::lmgw_link::mcp::fake::serve(apps_mcp()).await;
    let apps = apps_at(&f.url);
    let view = |name: &str, known: Option<(&str, &str)>| CallView {
        name: name.into(),
        arguments: Map::new(),
        call_id: None,
        progress: watch::channel(Progress::default()).1,
        known: known.map(|(s, u)| (s.into(), u.into())),
    };
    let body = apps
        .open_for_call(view("fx__show", None), (480, 360))
        .await
        .unwrap()
        .expect("a view");
    assert_eq!(body.html, "<p>card</p>");
    let methods = f.methods();
    let list = methods.iter().position(|m| m == "tools/list").unwrap();
    let read = methods.iter().position(|m| m == "resources/read").unwrap();
    assert!(list < read, "{methods:?}");
    drop(body);
    // The deprecated flat key is still read.
    assert!(
        apps.open_for_call(view("fx__old", None), (480, 360))
            .await
            .unwrap()
            .is_some()
    );
    let before = f.methods().len();
    assert!(
        apps.open_for_call(view("fx__plain", None), (480, 360))
            .await
            .unwrap()
            .is_none()
    );
    let after: Vec<String> = f.methods()[before..].to_vec();
    assert!(!after.iter().any(|m| m == "resources/read"), "{after:?}");
    // Known from the result frame: read first, as overlay_app does.
    let before = f.methods().len();
    assert!(
        apps.open_for_call(
            view("fx__show", Some(("fx", "ui://fx__card/main"))),
            (480, 360)
        )
        .await
        .unwrap()
        .is_some()
    );
    let after: Vec<String> = f.methods()[before..].to_vec();
    let read = after.iter().position(|m| m == "resources/read").unwrap();
    let list = after.iter().position(|m| m == "tools/list").unwrap();
    assert!(read < list, "{after:?}");
    // A URI outside the label's namespace is refused, as for overlay_app.
    let e = apps
        .open_for_call(
            view("fx__show", Some(("fx", "ui://other__card"))),
            (480, 360),
        )
        .await
        .unwrap_err();
    assert!(e.contains("namespace"), "{e}");
}

#[test]
fn the_tool_info_is_in_the_servers_own_terms() {
    let list = json!({"tools": [{"name": "fx__show", "description": "Show.",
        "_meta": {"ui": {"resourceUri": "ui://fx__card/main"}, "ui/resourceUri": "ui://fx__card/main"}}]});
    let rules = Rules::of_list("fx", &list);
    let t = own_tool(&rules, &list, "fx__show").unwrap();
    assert_eq!(t["name"], "show");
    assert_eq!(t["_meta"]["ui"]["resourceUri"], "ui://card/main");
    assert_eq!(t["_meta"]["ui/resourceUri"], "ui://card/main");
    assert_eq!(own_tool(&rules, &list, "fx__nothing"), None);
}

/// Kai's own tools' label (`hello.hosts_label`) is no MCP server's, like lmgw's own.
#[test]
fn kais_own_label_is_no_apps() {
    let e = target_beside("kai", "ui://kai__card", Some("kai")).unwrap_err();
    assert!(e.contains("Kai's own tools"), "{e}");
    assert!(
        target_beside("kai", "ui://kai__card", None).is_ok(),
        "the link down: not known"
    );
    assert!(target_beside("fx", "ui://fx__card", Some("kai")).is_ok());
}

/// The tool list as last fetched: a call whose tool it lists without a view opens no session;
/// one it does not hold is looked up; a `policy.tools.<server>__…` name the list lacks is named
/// in the view's notes.
#[tokio::test]
async fn a_calls_tool_without_a_view_opens_no_session_once_the_list_is_known() {
    let f = crate::lmgw_link::mcp::fake::serve(apps_mcp()).await;
    let mut apps = apps_at(&f.url);
    apps.policy = policy(
        &[],
        &[
            ("fx__show", ToolState::Allow),
            ("fx__shwo", ToolState::Allow),
            ("other__x", ToolState::Allow),
        ],
    );
    let view = |name: &str| CallView {
        name: name.into(),
        arguments: Map::new(),
        call_id: None,
        progress: watch::channel(Progress::default()).1,
        known: None,
    };
    let opens = || {
        f.methods()
            .iter()
            .filter(|m| m.as_str() == "initialize")
            .count()
    };
    assert!(
        apps.open_for_call(view("fx__plain"), (480, 360))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(opens(), 1, "the first time asks /mcp");
    assert!(
        apps.open_for_call(view("fx__plain"), (480, 360))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(opens(), 1, "the list as last fetched answers");
    // A tool the list does not hold is looked up afresh.
    assert!(
        apps.open_for_call(view("fx__new"), (480, 360))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(opens(), 2);
    let body = apps
        .open_for_call(view("fx__show"), (480, 360))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.server, "fx");
    assert_eq!(
        body.notes,
        [
            "policy.tools.fx__shwo: fx offers Kai no such tool (its /mcp list), so the setting does \
          nothing"
        ],
        "only this server's names, only the unlisted"
    );
    drop(body);
    // Kai's voice session ended: forgotten, asked again.
    apps.lists.forget();
    let before = opens();
    apps.open_for_call(view("fx__plain"), (480, 360))
        .await
        .unwrap();
    assert_eq!(opens(), before + 1);
}

fn list_req(id: u64, method: &str) -> String {
    req(id, method, json!({}))
}

/// K32: while the screen is locked, or not yet known to be unlocked, a view's request that would
/// reach lmgw is refused with the lock's words and nothing is forwarded; after the unlock the
/// same requests go through. The bridge's own answers (`ping`, `ui/initialize`) stay.
#[tokio::test]
async fn a_locked_screen_refuses_every_request_that_reaches_lmgw() {
    for screen in [ScreenLock::unknown(), {
        let s = ScreenLock::default();
        s.set(true);
        s
    }] {
        let (b, fake) = bridge(Owner::new(&[]));
        let b = b.with_screen(screen.clone());
        let read = req(5, "resources/read", json!({"uri": "ui://card/main"}));
        for text in [
            call(1, "read"),
            read.clone(),
            list_req(2, "resources/list"),
            list_req(3, "resources/templates/list"),
        ] {
            let a = b.answer(&text).await.unwrap();
            assert_eq!(code(&a), DENIED, "{a}");
            assert_eq!(a["error"]["message"], crate::screen_lock::REFUSAL);
        }
        assert!(fake.asked.lock().unwrap().is_empty(), "nothing forwarded");
        let a = b.answer(&list_req(4, "ping")).await.unwrap();
        assert_eq!(a["result"], json!({}), "the bridge's own answers stay");

        screen.set(false);
        for text in [
            call(1, "read"),
            read,
            list_req(2, "resources/list"),
            list_req(3, "resources/templates/list"),
        ] {
            let a = b.answer(&text).await.unwrap();
            assert!(a.get("error").is_none(), "{a}");
        }
        assert_eq!(
            fake.asked.lock().unwrap().len(),
            4,
            "forwarded after the unlock"
        );
    }
}

/// K32: a call that ends while the screen is locked delivers its input and result after the
/// unlock, not before.
#[tokio::test]
async fn a_calls_view_gets_nothing_while_locked_and_its_result_after() {
    let screen = ScreenLock::default();
    screen.set(true);
    let (b, _) = bridge(Owner::new(&[]));
    let (tx, progress) = watch::channel(Progress::default());
    let ends = b
        .with_screen(screen.clone())
        .with_run(ToolRun {
            arguments: Map::new(),
            tool: None,
            call_id: None,
            progress,
        })
        .spawn();
    ends.to_bridge.send(initialized()).unwrap();
    tx.send_replace(Progress::ended(
        true,
        Outcome::Result {
            result: json!({"structuredContent": {"temp": 21}}),
            ui: None,
        },
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(ends.from_bridge.is_empty(), "nothing while locked");
    screen.set(false);
    assert_eq!(next(&ends).await["method"], "ui/notifications/tool-input");
    assert_eq!(next(&ends).await["method"], "ui/notifications/tool-result");
}
