//! Kai's own windows are never acted on (`resolve::not_kais`): its app id, its own process and a
//! process it started (its WebKit processes) are refused by the window tools, the input tools and
//! `ui_press`, before the confirmation and at the call; reading them is fine.

use std::sync::Arc;

use kai_desktop::caps::accessibility::Found as Element;
use kai_desktop::caps::windows::Window;
use serde_json::{Value, json};

use super::input::FakeInput;
use super::read::FakeAccess;
use super::{FakeWindows, all_services, snapshot, window};
use crate::desktop::resolve;
use crate::desktop::tools::{self, Ctx, Services};
use crate::host::registry::{Args, Block, Origin};

async fn call(services: &Services, name: &str, args: Value) -> (String, bool) {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    let r = tools::run_with(services, name, &args, &Ctx::default()).await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

async fn prepare(services: &Services, name: &'static str, args: Value) -> Result<(), String> {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    tools::input::prepare(services, name)(&args, Origin::default())
        .await
        .map(|_| ())
}

/// The fixture's windows, and Kai's: its configuration window by app id, one by Kai's own pid
/// (this test process stands for Kai), one by a child process's (a WebKit process), the last one
/// active.
fn services(kais: Window, input: &FakeInput, access: &FakeAccess) -> (Services, FakeWindows) {
    let mut snap = snapshot();
    for w in &mut snap.windows {
        w.active = false;
    }
    let mut kais = kais;
    kais.active = true;
    snap.windows.push(kais);
    let windows = FakeWindows {
        snap,
        ..Default::default()
    };
    let s = Services {
        input: Some(Arc::new(input.clone())),
        accessibility: Some(Arc::new(access.clone())),
        ..all_services(windows.clone())
    };
    (s, windows)
}

#[tokio::test]
async fn kais_own_windows_are_never_acted_on() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep");
    let by_app = window("kkkk", crate::APP_ID, "Kai Settings");
    let mut by_pid = window("kpid", "org.example.other", "Confirm — Kai");
    by_pid.pid = std::process::id();
    let mut by_child = window("kweb", "WebKitWebProcess", "ring");
    by_child.pid = child.id();
    assert!(resolve::own_process(std::process::id()));
    assert!(resolve::own_process(child.id()), "a process Kai started");
    assert!(!resolve::own_process(1) && !resolve::own_process(0));
    for kais in [by_app, by_pid, by_child] {
        let id = kais.id.clone();
        let input = FakeInput::default();
        let access = FakeAccess {
            found: Some(Element {
                role: "button".into(),
                name: "Allow".into(),
                at: (":1.9".into(), "/org/a11y/atspi/accessible/3".into()),
            }),
            ..Default::default()
        };
        let (s, windows) = services(kais, &input, &access);
        // The window tools: by id, and as the active window.
        for (tool, extra) in [
            ("window_close", json!({})),
            ("window_activate", json!({})),
            ("window_minimize", json!({})),
            ("window_tile", json!({"side": "left"})),
        ] {
            for args in [json!({"window": id}), extra.clone()] {
                let mut args = args;
                if let (Some(a), Some(e)) = (args.as_object_mut(), extra.as_object()) {
                    for (k, v) in e {
                        a.entry(k.clone()).or_insert(v.clone());
                    }
                }
                let (text, err) = call(&s, tool, args).await;
                assert!(
                    err && text.contains("one of Kai's own windows"),
                    "{tool} {id}: {text}"
                );
            }
        }
        assert!(
            windows.acts.lock().unwrap().is_empty(),
            "{id}: nothing done"
        );
        // The input tools: the focused window is Kai's: refused before the dialog and at the call.
        for (tool, args) in [
            ("type_text", json!({"text": "yes"})),
            ("key_press", json!({"keys": "enter"})),
        ] {
            let why = prepare(&s, tool, args.clone()).await.unwrap_err();
            assert!(
                why.contains("one of Kai's own windows"),
                "{tool} {id}: {why}"
            );
            let (text, err) = call(&s, tool, args).await;
            assert!(
                err && text.contains("one of Kai's own windows"),
                "{tool}: {text}"
            );
        }
        assert!(input.sent_nothing(), "{id}: nothing typed or pressed");
        // ui_press: refused before the element is even looked up.
        let args = json!({"window": id, "element": "button \"Allow\""});
        let why = prepare(&s, "ui_press", args.clone()).await.unwrap_err();
        assert!(why.contains("one of Kai's own windows"), "{id}: {why}");
        let (text, err) = call(&s, "ui_press", args).await;
        assert!(err && text.contains("one of Kai's own windows"), "{text}");
        assert!(access.finds.lock().unwrap().is_empty());
        assert!(access.presses.lock().unwrap().is_empty());
        // Reading is fine.
        let (list, err) = call(&s, "window_list", json!({})).await;
        assert!(!err && list.contains(&id), "{list}");
        // Another window still is acted on.
        let (text, err) = call(&s, "window_close", json!({"window": "dolphin"})).await;
        assert!(!err, "{text}");
    }
    let _ = child.kill();
    let _ = child.wait();
}
