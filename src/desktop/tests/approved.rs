//! A call lmgw's thread approved was approved before it arrived: the active window at arrival is
//! not what the owner saw (it may be lmgw's dashboard). Such a call must name its window for the
//! input tools, `ui_press` and every state-changing window action, and the window actions are
//! prepared like `ui_press`: the dialog names the resolved window, and the action runs on that id.

use std::sync::Arc;

use kai_desktop::caps::accessibility::Found as Element;
use serde_json::{Value, json};

use super::input::FakeInput;
use super::read::FakeAccess;
use super::{FakeWindows, all_services, snapshot};
use crate::desktop::tools::{self, Ctx, Services};
use crate::host::registry::{Args, Block, Origin, Preparer};

const APPROVED: Origin = Origin {
    caller: None,
    approved_in_lmgw: true,
};

fn fixture(windows: FakeWindows, input: &FakeInput, access: &FakeAccess) -> Services {
    Services {
        input: Some(Arc::new(input.clone())),
        accessibility: Some(Arc::new(access.clone())),
        ..all_services(windows)
    }
}

fn args(name: &str, a: Value) -> Args {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    Args::read(spec, a.as_object().cloned()).unwrap()
}

fn preparer(services: &Services, name: &str) -> Preparer {
    crate::desktop::entries(services)
        .into_iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not served"))
        .prepare
        .unwrap_or_else(|| panic!("{name} has no Preparer"))
}

async fn run(services: &Services, name: &str, a: &Args, ctx: &Ctx) -> (String, bool) {
    let r = tools::run_with(services, name, a, ctx).await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

/// Every tool that acts on a window, with arguments that fit it (no `window` yet).
fn acting() -> Vec<(&'static str, Value)> {
    let mut v = vec![
        ("type_text", json!({"text": "yes"})),
        ("key_press", json!({"keys": "enter"})),
        ("ui_press", json!({"element": "button \"Approve\""})),
    ];
    for t in kai_desktop::contract::windows::AREA.tools {
        if !tools::is_window_action(t.name) {
            continue;
        }
        let extra = match t.name {
            "window_to_desktop" => json!({"desktop": "1"}),
            "window_to_output" => json!({"output": "DP-1"}),
            "window_tile" => json!({"side": "left"}),
            _ => json!({}),
        };
        v.push((t.name, extra));
    }
    v
}

fn with_window(mut a: Value, window: Option<&str>) -> Value {
    if let Some(w) = window {
        a["window"] = json!(w);
    }
    a
}

#[tokio::test]
async fn an_approved_call_must_name_its_window() {
    let input = FakeInput::default();
    let access = FakeAccess {
        found: Some(Element {
            role: "button".into(),
            name: "Approve".into(),
            at: (":1.9".into(), "/org/a11y/atspi/accessible/3".into()),
        }),
        ..Default::default()
    };
    let windows = FakeWindows {
        snap: snapshot(),
        ..Default::default()
    };
    let s = fixture(windows.clone(), &input, &access);
    let tools_checked = acting();
    assert!(tools_checked.len() >= 10, "{}", tools_checked.len());
    for (tool, base) in &tools_checked {
        let prepare = preparer(&s, tool);
        for implicit in [None, Some("active"), Some("ACTIVE"), Some("  ")] {
            let a = args(tool, with_window(base.clone(), implicit));
            let why = prepare(&a, APPROVED).await.err().unwrap_or_else(|| {
                panic!("{tool} {implicit:?}: an approved call with an implicit window ran")
            });
            assert!(
                why.contains("approved in lmgw") && why.contains("by its id or app"),
                "{tool}: {why}"
            );
            // The same call, not approved, resolves (the owner's own dialog names the window).
            if *tool != "ui_press" || implicit.is_none() {
                assert!(
                    prepare(&a, Origin::default()).await.is_ok(),
                    "{tool} {implicit:?}"
                );
            }
        }
    }
    assert!(windows.acts.lock().unwrap().is_empty());
    assert!(input.sent_nothing());
    assert!(access.presses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_approved_call_that_names_its_window_is_prepared() {
    let input = FakeInput::default();
    let access = FakeAccess::default();
    let s = fixture(
        FakeWindows {
            snap: snapshot(),
            ..Default::default()
        },
        &input,
        &access,
    );
    // The window actions: any way of naming one.
    for by in ["aaaa", "dolphin", "Home"] {
        let a = args("window_close", json!({"window": by}));
        let p = preparer(&s, "window_close")(&a, APPROVED).await.unwrap();
        assert_eq!(
            p.lines,
            ["acts on: aaaa org.kde.dolphin \"Home — Dolphin\""]
        );
    }
    // Keys go to the focused window: naming it is fine, naming another is a refusal.
    let a = args("key_press", json!({"keys": "enter", "window": "konsole"}));
    assert!(preparer(&s, "key_press")(&a, APPROVED).await.is_ok());
    let a = args("key_press", json!({"keys": "enter", "window": "dolphin"}));
    let why = preparer(&s, "key_press")(&a, APPROVED).await.err().unwrap();
    assert!(
        why.contains("does not have the keyboard focus") && why.contains("window_activate"),
        "{why}"
    );
    let a = args("type_text", json!({"text": "x", "window": "dolphin"}));
    assert!(
        preparer(&s, "type_text")(&a, Origin::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn read_only_window_tools_have_no_preparer_and_keep_active() {
    let s = fixture(
        FakeWindows {
            snap: snapshot(),
            ..Default::default()
        },
        &FakeInput::default(),
        &FakeAccess::default(),
    );
    for e in crate::desktop::entries(&s) {
        if e.spec.hints.read_only {
            assert!(e.prepare.is_none(), "{} is read-only", e.spec.name);
        }
    }
    let a = args("read_window", json!({}));
    let r = crate::desktop::read::run(&s, "read_window", &a).await;
    assert!(
        !r.error || !format!("{:?}", r.blocks).contains("approved in lmgw"),
        "{:?}",
        r.blocks
    );
}

/// The dialog named a window; the action runs on that id, not on whatever `active` means by then.
#[tokio::test]
async fn a_window_action_runs_on_the_window_the_dialog_named() {
    let input = FakeInput::default();
    let access = FakeAccess::default();
    let before = FakeWindows {
        snap: snapshot(),
        ..Default::default()
    };
    let s = fixture(before.clone(), &input, &access);
    let a = args("window_close", json!({}));
    let p = preparer(&s, "window_close")(&a, Origin::default())
        .await
        .unwrap();
    assert_eq!(
        p.lines,
        ["acts on: bbbb org.kde.konsole \"build — Konsole\""]
    );
    // Meanwhile the focus moved to dolphin.
    let mut moved = snapshot();
    for w in &mut moved.windows {
        w.active = w.id == "aaaa";
    }
    let after = FakeWindows {
        snap: moved.clone(),
        acts: before.acts.clone(),
        ..Default::default()
    };
    let s2 = fixture(after, &input, &access);
    let ctx = Ctx {
        prepared: Some(p.clone()),
        ..Ctx::default()
    };
    let (text, err) = run(&s2, "window_close", &a, &ctx).await;
    assert!(!err && text.contains("bbbb"), "{text}");
    assert_eq!(before.acts.lock().unwrap()[0].0, "bbbb");
    // The window is gone: refused, nothing done.
    moved.windows.retain(|w| w.id != "bbbb");
    let gone = FakeWindows {
        snap: moved,
        acts: before.acts.clone(),
        ..Default::default()
    };
    let s3 = fixture(gone, &input, &access);
    let (text, err) = run(&s3, "window_close", &a, &ctx).await;
    assert!(err && text.contains("is gone"), "{text}");
    assert_eq!(
        before.acts.lock().unwrap().len(),
        1,
        "nothing more was done"
    );
}
