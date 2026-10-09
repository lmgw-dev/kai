//! A view's tools from lmgw's `_meta["lmgw/server"]` stamps: a tool a collision moved to
//! `<server name>__x` is still its own server's, another server's never is, and `policy.tools`
//! reads both spellings of a moved tool (the strictest wins, an `allow` of the old one does not
//! carry over).

use serde_json::json;

use super::tests::{Fake, Owner, call, code, policy};
use super::*;

/// Two servers share the prefix `fx` (so the label): alpha's `x` kept `fx__x`, beta's was moved to
/// `beta__x`; alpha's `fx__y` is renamed by the owner to `fx__why`; gamma is another label.
fn list() -> Value {
    let stamp = |label: &str, name: &str, tool: &str| json!({"lmgw/server": {"label": label, "name": name, "tool": tool}});
    json!({"tools": [
        {"name": "fx__x", "annotations": {"readOnlyHint": true}, "_meta": stamp("fx", "alpha", "x")},
        {"name": "beta__x", "annotations": {"readOnlyHint": true}, "_meta": stamp("fx", "beta", "x")},
        {"name": "beta__z__w", "annotations": {"readOnlyHint": true}, "_meta": stamp("fx", "beta", "z__w")},
        {"name": "fx__why", "annotations": {"readOnlyHint": true}, "_meta": stamp("fx", "alpha", "y")},
        {"name": "gy__x", "annotations": {"readOnlyHint": true}, "_meta": stamp("gy", "gamma", "x")},
        {"name": "docs__search", "annotations": {"readOnlyHint": true}},
    ]})
}

fn exposed(r: &Rules) -> Vec<&str> {
    r.tools.iter().map(|t| t.exposed.as_str()).collect()
}

#[test]
fn a_stamped_list_gives_a_view_its_labels_tools_by_their_own_names() {
    let r = Rules::of_list("fx", &list());
    assert!(r.stamped);
    assert_eq!(exposed(&r), ["fx__x", "beta__x", "beta__z__w", "fx__why"]);
    let why = r.tools.iter().find(|t| t.exposed == "fx__why").unwrap();
    assert_eq!(
        (why.own.as_str(), why.alias.as_deref(), why.by.as_deref()),
        ("y", Some("fx__y"), Some("alpha"))
    );
    assert_eq!(
        r.tool("y").unwrap().exposed,
        "fx__why",
        "own name to exposed"
    );
    assert!(
        r.tool("why").is_err(),
        "the exposed spelling is not its own name"
    );
    // The stamp makes an own name with `__` the server's own: no overlapping prefix to fear.
    assert_eq!(r.tool("z__w").unwrap().exposed, "beta__z__w");
    // Two servers of the label both list `x`: Kai cannot tell which the view means.
    assert!(r.tool("x").unwrap_err().contains("no single tool"));
    // Never another label's.
    assert!(r.tool("search").is_err() && !exposed(&r).contains(&"gy__x"));
    // By the server's name when no stamp has that label.
    let by_name = Rules::of_list("gamma", &list());
    assert_eq!(exposed(&by_name), ["gy__x"]);
    assert_eq!(
        by_name.tool("x").unwrap().alias,
        None,
        "gy__x is its own name"
    );
}

#[test]
fn a_call_says_which_server_of_a_shared_label_its_view_is_for() {
    let mut r = Rules::of_list("fx", &list());
    r.narrow_to_call("beta__x");
    assert_eq!(exposed(&r), ["beta__x", "beta__z__w"]);
    let moved = r.tool("x").unwrap();
    assert_eq!(moved.exposed, "beta__x", "beta's x, never alpha's fx__x");
    assert_eq!(moved.alias.as_deref(), Some("fx__x"));
    assert!(r.tool("y").is_err(), "alpha's tools are not beta's view's");
    // A tool the list does not hold narrows nothing.
    let mut same = Rules::of_list("fx", &list());
    same.narrow_to_call("nothing__x");
    assert_eq!(same.tools.len(), 4);
}

#[test]
fn a_list_without_stamps_is_read_as_before() {
    let old = json!({"tools": [
        {"name": "fx__x", "annotations": {"readOnlyHint": true}},
        {"name": "beta__x", "annotations": {"readOnlyHint": true}},
        {"name": "fx__a__b", "annotations": {"readOnlyHint": true}},
    ]});
    let r = Rules::of_list("fx", &old);
    assert!(!r.stamped);
    assert_eq!(
        exposed(&r),
        ["fx__x"],
        "a moved tool is not found without the stamp"
    );
    assert_eq!(r.tool("x").unwrap().own, "x");
    assert!(r.tool("a__b").is_err());
    assert!(r.tool("x").unwrap().alias.is_none());
}

#[tokio::test]
async fn a_view_calls_its_servers_moved_tool_under_the_name_lmgw_lists() {
    let fake = Arc::new(Fake::default());
    let mut rules = Rules::of_list("fx", &list());
    rules.narrow_to_call("beta__x");
    let b = Bridge::new(
        Arc::new(fake.clone()),
        rules,
        Owner::new(&[]),
        Bound {
            bytes: 4096,
            says: "overlay.app_max_mb (test)".into(),
        },
        Calls {
            max: 16,
            says: "overlay.app_max_calls (test)".into(),
        },
        (480, 360),
    )
    .with_policy(policy(&[], &[]));
    let a = b.answer(&call(1, "x")).await.unwrap();
    assert_eq!(a["result"]["structuredContent"]["called"], "beta__x");
    // Alpha's tool, by its own name or its exposed name, is not this view's.
    for name in ["y", "fx__x", "beta__x", "gy__x"] {
        let a = b.answer(&call(2, name)).await.unwrap();
        assert_eq!(code(&a), -32602, "{name}");
    }
    let sent: Vec<_> = fake
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == "tools/call")
        .map(|(_, p)| p["name"].clone())
        .collect();
    assert_eq!(sent, [json!("beta__x")]);
}

#[test]
fn a_moved_tools_policy_reads_both_spellings_and_the_strictest_wins() {
    let mut r = Rules::of_list("fx", &list());
    r.narrow_to_call("beta__x");
    let moved = r.tool("x").unwrap().clone(); // exposed beta__x, alias fx__x
    let fx = caller("fx");
    let get =
        |states: &[(&str, ToolState)]| decide(&policy(&[], states).read().unwrap(), &moved, &fx);
    assert_eq!(get(&[]), Decision::Allow, "read-only, no entry");
    // Written for the old name: it still tightens.
    assert!(
        matches!(get(&[("fx__x", ToolState::Deny)]), Decision::Deny(m) if m.contains("policy.tools.fx__x"))
    );
    assert!(matches!(
        get(&[("fx__x", ToolState::Confirm)]),
        Decision::Confirm { .. }
    ));
    // The exposed name decides when it is the stricter, or the only one.
    assert!(
        matches!(get(&[("beta__x", ToolState::Off)]), Decision::Deny(m) if m.contains("policy.tools.beta__x"))
    );
    assert!(matches!(
        get(&[("beta__x", ToolState::Allow), ("fx__x", ToolState::Deny)]),
        Decision::Deny(m) if m.contains("policy.tools.fx__x")
    ));
    assert!(matches!(
        get(&[("beta__x", ToolState::Deny), ("fx__x", ToolState::Confirm)]),
        Decision::Deny(m) if m.contains("policy.tools.beta__x")
    ));
}

/// A state-changing moved tool, for the allow that must not carry over.
fn moved_bump() -> ViewTool {
    ViewTool {
        exposed: "beta__bump".into(),
        own: "bump".into(),
        alias: Some("fx__bump".into()),
        by: Some("beta".into()),
        app: true,
        read_only: false,
        destructive: false,
        description: String::new(),
    }
}

#[test]
fn an_allow_written_for_the_old_name_does_not_loosen_the_moved_tool() {
    let bump = moved_bump();
    let fx = caller("fx");
    let get = |trusted: &[&str], states: &[(&str, ToolState)]| {
        decide(&policy(trusted, states).read().unwrap(), &bump, &fx)
    };
    assert!(matches!(
        get(&[], &[("fx__bump", ToolState::Allow)]),
        Decision::Confirm { .. }
    ));
    assert_eq!(
        get(&[], &[("beta__bump", ToolState::Allow)]),
        Decision::Allow
    );
    assert_eq!(
        get(
            &[],
            &[
                ("beta__bump", ToolState::Allow),
                ("fx__bump", ToolState::Allow)
            ]
        ),
        Decision::Allow
    );
    assert!(
        matches!(
            get(&["app:fx"], &[("fx__bump", ToolState::AllowTrusted)]),
            Decision::Allow
        ),
        "the default for a trusted app's non-destructive tool, not the old entry"
    );
}
