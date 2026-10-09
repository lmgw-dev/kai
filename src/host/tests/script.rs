//! The fake host's script: the handshake, `initialize`, listing, a call with `_meta`, a list that
//! stays the same across reconnects, `off` unlisting with `list_changed`; and PD6's contract test
//! over every tool this build hosts.

use serde_json::json;

use super::*;
use crate::host::registry::providers;
use crate::host::server::render;
use crate::host::settings::ToolState;

#[tokio::test]
async fn initialize_and_listing_as_lmgw_does_them() {
    let mut r = rig().await;
    assert_eq!(r.status(|_| true).await, HostStatus::Connecting);
    let mut c = r.fake.accept().await;
    assert_eq!(c.path, "/mcp/host");
    assert_eq!(
        c.headers.get("authorization").unwrap().to_str().unwrap(),
        format!("Bearer {KEY}")
    );
    assert!(
        c.headers.get("origin").is_none(),
        "lmgw refuses any Origin (api §5.6)"
    );
    let init = c.initialize().await;
    let result = &init["result"];
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert_eq!(result["serverInfo"]["name"], "kai");
    assert_eq!(result["capabilities"]["tools"]["listChanged"], true);
    assert_eq!(
        r.status(|s| matches!(s, HostStatus::Linked { .. })).await,
        HostStatus::Linked { tools: 6 }
    );
    let tools = c.list().await;
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["echo", "poke", "hang", "own_only", "prep", "ctx"]);
    assert_eq!(tools[0]["description"], "Answer with the text.");
    assert_eq!(
        tools[0]["inputSchema"],
        json!({"type": "object", "properties": {"text": {"type": "string"}},
               "required": ["text"], "additionalProperties": false})
    );
    assert_eq!(
        tools[0]["annotations"],
        json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true,
               "openWorldHint": false})
    );
    assert_eq!(tools[1]["annotations"]["readOnlyHint"], false);
}

#[tokio::test]
async fn a_call_runs_as_the_caller_lmgw_stamped() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c.call("echo", json!({"text": "hi"}), owner(), 60_000).await;
    let a = c.answer(id, &mut Vec::new()).await;
    assert_eq!(text_of(&a), ("echo: hi".into(), false));
    assert_eq!(r.ran.all(), ["echo by the owner"]);

    // Kai's own device, once the feed named it: the own-only tool runs for it and no one else.
    r.host.feed_up("desktop", Some("kai"));
    let device = json!({"kind": "device", "name": "desktop"});
    let id = c.call("own_only", json!({}), device, 60_000).await;
    assert_eq!(text_of(&c.answer(id, &mut Vec::new()).await).0, "mine");
    let id = c.call("own_only", json!({}), owner(), 60_000).await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error && text.contains("Kai's own session only"), "{text}");
    assert_eq!(r.ran.all(), ["echo by the owner", "own_only by Kai itself"]);

    // A call without `_meta` is an unknown caller's: a read-only tool is asked about.
    c.send(json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call",
                  "params": {"name": "echo", "arguments": {"text": "x"}}}))
        .await;
    let q = r.ask().await;
    assert_eq!(q.caller, "a caller lmgw did not name");
    assert_eq!(
        q.tool, "kai__echo",
        "the dialog names the tool as the caller sees it"
    );
    q.reply.send(false).unwrap();
    let (text, error) = text_of(&c.answer(900, &mut Vec::new()).await);
    assert!(error && text.contains("denied"), "{text}");

    // Arguments that do not fit, and a tool Kai does not offer.
    let id = c.call("echo", json!({"text": 5}), owner(), 60_000).await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert_eq!((text.as_str(), error), ("echo: text must be a text", true));
    let id = c.call("nothing", json!({}), owner(), 60_000).await;
    let a = c.answer(id, &mut Vec::new()).await;
    assert!(
        a["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no tool 'nothing'"),
        "{a}"
    );

    // A frame that is no MCP client message: an error to its id, and the link goes on.
    c.send_text(r#"{"jsonrpc": "2.0", "id": 77, "method": 5}"#)
        .await;
    let a = c.answer(77, &mut Vec::new()).await;
    assert_eq!(a["error"]["code"], -32600, "{a}");
    let id = c
        .call("echo", json!({"text": "still"}), owner(), 60_000)
        .await;
    assert_eq!(
        text_of(&c.answer(id, &mut Vec::new()).await).0,
        "echo: still"
    );
}

#[tokio::test]
async fn the_owner_answers_a_confirmation() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    // A state-changing tool is asked about for every caller (K15), the owner included.
    let id = c
        .call("poke", json!({"text": "a b"}), owner(), 60_000)
        .await;
    let q = r.ask().await;
    assert_eq!(
        (q.tool.as_str(), q.caller.as_str(), q.lines.clone()),
        ("poke", "the owner", vec!["text: a b".to_owned()])
    );
    let left = q.deadline.unwrap() - std::time::Instant::now();
    assert!(
        left > Duration::from_secs(55) && left <= Duration::from_secs(60),
        "{left:?}"
    );
    q.reply.send(true).unwrap();
    assert_eq!(text_of(&c.answer(id, &mut Vec::new()).await).0, "poked");
    let id = c
        .call("poke", json!({"text": "b"}), key("script"), 60_000)
        .await;
    r.ask().await.reply.send(false).unwrap();
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(
        error && text == "the owner denied this call of poke",
        "{text}"
    );
    assert_eq!(r.ran.all(), ["poke by the owner"]);
}

/// K22: the list is the same on every link, byte for byte.
#[tokio::test]
async fn the_list_is_identical_across_reconnects() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let first = c.list().await;
    c.close(1001, "lmgw is stopping or restarting").await;
    let mut c = r.linked().await;
    let second = c.list().await;
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}

/// K15's `off`: the tool leaves the list and lmgw is told to list again; an unchanged policy says
/// nothing.
#[tokio::test]
async fn off_unlists_and_sends_list_changed() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    assert_eq!(c.list().await.len(), 6);
    let mut off = PolicyConfig::default();
    off.tools.insert("poke".into(), ToolState::Off);
    assert!(r.host.set_policy(Policy::new(&off)));
    let note = c.recv().await;
    assert_eq!(note["method"], "notifications/tools/list_changed");
    let names: Vec<String> = c
        .list()
        .await
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["echo", "hang", "own_only", "prep", "ctx"]);
    let id = c.call("poke", json!({"text": "x"}), owner(), 60_000).await;
    assert!(
        c.answer(id, &mut Vec::new()).await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no tool 'poke'")
    );
    // The same policy again: no change, no notification (the next message is the list's answer).
    assert!(!r.host.set_policy(Policy::new(&off)));
    let mut seen = Vec::new();
    let id = 4242;
    c.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}}))
        .await;
    c.answer(id, &mut seen).await;
    assert!(seen.is_empty(), "{seen:?}");
    // Back on.
    assert!(r.host.set_policy(Policy::new(&PolicyConfig::default())));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
}

/// K30: the owner's configuration tools are listed only while `policy.config_tools` is on, and
/// switching it changes the list once (one `list_changed`); a setting that changes nothing says
/// nothing.
#[tokio::test]
async fn config_tools_toggle_changes_the_list_once() {
    let (cmds, _core) = mpsc::unbounded_channel();
    let core_tools: Box<dyn Provider> = Box::new(crate::core::calls::provider(
        cmds,
        crate::jobs::Jobs::default(),
    ));
    let mut r = rig_with_providers(PolicyConfig::default(), settings(1), vec![core_tools]).await;
    let mut c = r.linked().await;
    let names = |list: &[Value]| -> Vec<String> {
        list.iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = names(&c.list().await);
    assert!(
        !before.iter().any(|n| n.starts_with("config_")),
        "{before:?}"
    );
    let mut on = PolicyConfig {
        config_tools: true,
        ..PolicyConfig::default()
    };
    assert!(r.host.set_policy(Policy::new(&on)));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    let after = names(&c.list().await);
    for tool in kai_desktop::contract::config::AREA.tools {
        assert!(
            after.iter().any(|n| n == tool.name),
            "{} missing",
            tool.name
        );
    }
    assert_eq!(after.len(), before.len() + 7);
    // One tool of them off: it leaves on its own; the same policy again says nothing.
    on.tools.insert("config_unset".into(), ToolState::Off);
    assert!(r.host.set_policy(Policy::new(&on)));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    assert!(!names(&c.list().await).contains(&"config_unset".to_owned()));
    assert!(!r.host.set_policy(Policy::new(&on)));
    // Off again: the list is the first one.
    assert!(r.host.set_policy(Policy::new(&PolicyConfig::default())));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    assert_eq!(names(&c.list().await), before);
}

/// PD6: every tool this build hosts keeps the contract's rules as lmgw sees it: at most two plain
/// parameters, no nested object, a one-line description, every MCP annotation, no desktop in its
/// name.
#[test]
fn every_hosted_tool_keeps_the_contract() {
    let mut entries = crate::host::registry::Registry::new(providers(&deps())).entries();
    entries.extend(TestTools(Ran::default()).entries());
    // The desktop's tools are listed only where the probe finds their capabilities (K23); the
    // tests' desktop provider probes nothing, so they come from a desktop that has them all.
    entries.extend(crate::desktop::tests::full_entries());
    entries.extend(crate::delegate::tests::full_entries());
    assert!(entries.len() >= 5, "the core's and the test tools at least");
    for e in &entries {
        let spec = e.spec;
        assert_eq!(
            kai_desktop::contract::check(spec),
            Vec::<String>::new(),
            "{}",
            spec.name
        );
        let tool = serde_json::to_value(render(spec)).unwrap();
        let props = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.len() <= 2, "{}", spec.name);
        for (p, schema) in props {
            let t = schema["type"].as_str().unwrap();
            assert!(
                ["string", "integer", "number", "boolean"].contains(&t)
                    || (t == "array" && schema["items"]["type"] == "string"),
                "{}: {p} is {schema}",
                spec.name
            );
        }
        let d = tool["description"].as_str().unwrap();
        assert!(!d.is_empty() && !d.contains('\n'), "{}", spec.name);
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                tool["annotations"][hint].is_boolean(),
                "{}: {hint}",
                spec.name
            );
        }
        let name = tool["name"].as_str().unwrap();
        for word in name.split('_') {
            assert!(
                !kai_desktop::contract::DESKTOP_NAMES.contains(&word),
                "{name}"
            );
        }
    }
    // Every contract area's tools have a provider in this build.
    for spec in kai_desktop::contract::tools() {
        assert!(
            entries.iter().any(|e| e.spec.name == spec.name),
            "{} has no provider",
            spec.name
        );
    }
}
