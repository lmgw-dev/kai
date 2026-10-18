//! The review of WP17: the policy read live, the answering alias, the plan's dialog, the user's
//! words when none came, and no Preparer among the delegates' tools.

use super::*;

fn laptop() -> Caller {
    Caller::Named(Named {
        kind: "key".into(),
        name: "laptop".into(),
    })
}

fn nothing_heard() -> Option<String> {
    None
}

fn off(tool: &str, state: ToolState) -> Policy {
    let mut c = PolicyConfig::default();
    c.tools.insert(tool.into(), state);
    Policy::new(&c)
}

/// The two reads before a plan, then the plan's calls.
fn plan_script() -> Vec<Value> {
    vec![
        tool_turn(&[(
            "p",
            "propose_plan",
            json!({"steps": [{"tool": "see_window", "arguments": {"window": "editor"}}]}),
        )]),
        tool_turn(&[("a", "see_window", json!({"window": "editor"}))]),
        text_turn("done: seen"),
    ]
}

#[tokio::test]
async fn a_policy_change_applies_to_the_delegates_calls_and_listing() {
    let lmgw = Lmgw::start(
        folders(Some("kai-chat")),
        vec![text_turn("done: first"), text_turn("done: second")],
    )
    .await;
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    let reads = |r: &Rig, tool: &str| r.desk.calls().iter().filter(|c| c.0 == tool).count();
    assert_eq!(
        reads(&r, "window_list"),
        1,
        "allowed: the window list was read"
    );

    // The host's policy changes (WP21a's `set_policy` replaces the value in the same lock).
    *r.policy.write().unwrap() = off("window_list", ToolState::Deny);
    call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    assert_eq!(reads(&r, "window_list"), 1, "denied now: not read again");
    let second: Value = serde_json::from_str(&lmgw.completions()[1].body).unwrap();
    let task = second["messages"][1]["content"].as_array().unwrap()[0]["text"]
        .as_str()
        .unwrap();
    assert!(task.contains("## Open windows\n(not read: "), "{task}");

    // The list: `look` is offered while the desktop serves `see_window` and the policy lists it.
    assert_eq!(r.tools.entries().len(), 1);
    *r.policy.write().unwrap() = off("see_window", ToolState::Off);
    assert!(r.tools.entries().is_empty(), "the tool it needs is off now");
    *r.policy.write().unwrap() = Policy::default();
    assert_eq!(r.tools.entries().len(), 1);
}

#[tokio::test]
async fn a_policy_change_during_a_task_applies_to_its_next_step() {
    let r = rig(
        "https://127.0.0.1:1",
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    let tools = tools::KaiTools {
        delegate: "look",
        entries: r.tools.inner_entries(),
        policy: r.policy.clone(),
        caller: Caller::Own,
        deadline: None,
        timeout_ms: None,
    };
    use kai_delegate::Tools;
    let gate = |t: &tools::KaiTools| {
        t.check("read_window", &json!({"window": "w"}))
            .unwrap()
            .gate
    };
    assert_eq!(gate(&tools), kai_delegate::Gate::Allow);
    *r.policy.write().unwrap() = off("read_window", ToolState::Deny);
    assert!(matches!(gate(&tools), kai_delegate::Gate::Deny(_)));
}

#[tokio::test]
async fn the_alias_that_answered_is_named_when_a_fallback_did() {
    let mut turn = text_turn("done: it says hello");
    turn["_fallback"] = json!("cloud-b");
    let lmgw = Lmgw::start(folders(None), vec![turn]).await;
    lmgw.list_models(json!([{"id": "local-a", "capabilities": {"task": "chat", "endpoints": [], "source": "config", "vision": true}}]));
    let set = DelegateConfig {
        look: Some("local-a".into()),
        ..DelegateConfig::default()
    };
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        set,
        PolicyConfig::default(),
    );
    let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    assert_eq!(
        text(&reply),
        "done: it says hello\n(model: local-a, answered by cloud-b)"
    );
    // Without a fallback nothing is added.
    let lmgw = Lmgw::start(folders(None), vec![text_turn("done: x")]).await;
    let set = DelegateConfig {
        look: Some("local-a".into()),
        ..DelegateConfig::default()
    };
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        set,
        PolicyConfig::default(),
    );
    let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    assert_eq!(text(&reply), "done: x");
}

#[tokio::test]
async fn look_fails_up_front_on_an_alias_lmgw_lists_without_vision() {
    let lmgw = Lmgw::start(folders(None), vec![text_turn("done: x")]).await;
    lmgw.list_models(json!([
        {"id": "blind", "capabilities": {"task": "chat", "endpoints": [], "source": "config", "vision": false}},
        {"id": "sighted", "capabilities": {"task": "chat", "endpoints": [], "source": "config", "vision": true}},
    ]));
    let set = |alias: &str| DelegateConfig {
        look: Some(alias.into()),
        find: Some("blind".into()),
        ..DelegateConfig::default()
    };
    let r = rig(
        &lmgw.url,
        Desk::with(&["see_window", "find_files"]),
        set("blind"),
        PolicyConfig::default(),
    );
    let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    assert!(reply.error);
    let said = text(&reply);
    assert!(
        said.contains("'blind' takes no images")
            && said.contains("set delegate.look to a vision alias"),
        "{said}"
    );
    assert!(lmgw.completions().is_empty(), "no model request was made");
    // `find` works from text: the same alias runs it.
    let reply = call(&r.tools, "find", json!({"what": "w"}), Caller::Own).await;
    assert_eq!(text(&reply), "done: x");
    // A vision alias, and one lmgw says nothing of, both go on.
    for (alias, listed) in [("sighted", true), ("unlisted", false)] {
        let lmgw = Lmgw::start(folders(None), vec![text_turn("done: ok")]).await;
        if listed {
            lmgw.list_models(json!([{"id": alias, "capabilities": {"task": "chat", "endpoints": [], "source": "config", "vision": true}}]));
        }
        let r = rig(
            &lmgw.url,
            Desk::with(LOOK_TOOLS),
            set(alias),
            PolicyConfig::default(),
        );
        let reply = call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
        assert_eq!(text(&reply), "done: ok", "{alias}");
    }
}

#[tokio::test]
async fn the_plan_dialog_names_the_task_the_model_and_the_time_left() {
    let lmgw = Lmgw::start(folders(Some("kai-chat")), plan_script()).await;
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    let e = entry(&r.tools, "look");
    let mut c = Call::new(
        e.spec,
        Args::read(e.spec, json!({"question": QUESTION}).as_object().cloned()).unwrap(),
        laptop(),
        CancellationToken::new(),
    );
    c.deadline = Some(tokio::time::Instant::now() + Duration::from_secs(60));
    c.timeout_ms = Some(60_000);
    let reply = (e.handler)(c).await;
    assert_eq!(text(&reply), "done: seen\n(used: see_window (editor))");
    let described = r.asked.described.lock().unwrap().clone();
    assert_eq!(described.len(), 1);
    let (description, has_deadline) = &described[0];
    assert!(description.contains(QUESTION), "{description}");
    assert!(description.contains("model kai-chat"), "{description}");
    assert!(has_deadline, "the dialog counts the time down");
    // A call with no deadline asks without one.
    let lmgw = Lmgw::start(folders(Some("kai-chat")), plan_script()).await;
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
    );
    call(&r.tools, "look", json!({"question": "q"}), laptop()).await;
    assert!(!r.asked.described.lock().unwrap()[0].1);
}

#[tokio::test]
async fn a_dialog_that_cannot_be_shown_is_not_a_refusal() {
    let lmgw = Lmgw::start(folders(Some("kai-chat")), plan_script()).await;
    let asked = Asked {
        unavailable: true,
        ..Asked::answering(true)
    };
    let r = rig_with(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
        asked,
        heard_words,
    );
    let reply = call(&r.tools, "look", json!({"question": "q"}), laptop()).await;
    assert!(reply.error);
    let said = text(&reply);
    assert!(said.contains("could not ask the owner"), "{said}");
    assert!(!said.contains("declined"), "{said}");
    assert!(
        r.desk.calls().iter().all(|c| c.0 != "see_window"),
        "nothing ran"
    );
}

#[tokio::test]
async fn words_that_have_not_come_are_said_to_be_untranscribed() {
    let lmgw = Lmgw::start(folders(Some("kai-chat")), vec![text_turn("done: x")]).await;
    let r = rig_with(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        DelegateConfig::default(),
        PolicyConfig::default(),
        Asked::answering(true),
        nothing_heard,
    );
    call(&r.tools, "look", json!({"question": "q"}), Caller::Own).await;
    let first: Value = serde_json::from_str(&lmgw.completions()[0].body).unwrap();
    let task = first["messages"][1]["content"].as_array().unwrap();
    let last = task.last().unwrap()["text"].as_str().unwrap();
    assert!(
        last.contains("not known here (not transcribed yet)"),
        "{last}"
    );
}

/// A Preparer resolves after the plan's confirmation would have to, and a delegate's plan shows its
/// arguments only: until WP18 brings the delegates that use such a tool, none of them may.
#[test]
fn no_delegate_tool_has_a_preparer_until_wp18() {
    let entries = crate::desktop::tests::full_entries();
    for d in &defs::DELEGATES {
        for tool in d.def.tools {
            let e = entries
                .iter()
                .find(|e| e.spec.name == *tool)
                .unwrap_or_else(|| panic!("{tool} is not a desktop tool"));
            assert!(
                e.prepare.is_none(),
                "{} uses {tool}, which has a Preparer: its plan would not show what it resolves",
                d.spec.name
            );
        }
    }
}
