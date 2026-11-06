//! `delegate.screenshots` (the owner, 2026-10-09): with `delegate`, the host lists the desktop's
//! and the browser's tools without the image tools, `look` stays and still sees; a change relists
//! once with `list_changed`; `main` lists everything.

use super::*;
use crate::delegate::screenshots::HostView;
use crate::delegate::settings::Screenshots;
use crate::host::tests::{owner, rig_with_providers, settings};

/// The tools whose reply is an image, by the contract's flag, leaving out only the browser tools
/// that are not built yet (`crate::browser::tools::Route::NotYet`): a built one must be listed
/// through a [`HostView`] (`the_browsers_image_tools_follow_the_same_setting`).
fn image_tools() -> Vec<&'static str> {
    kai_desktop::contract::areas()
        .into_iter()
        .flat_map(|a| a.tools.iter().map(move |t| (a.name, t)))
        .filter(|(area, t)| {
            *area != "browser"
                || !matches!(
                    crate::browser::tools::route(t.name),
                    crate::browser::tools::Route::NotYet
                )
        })
        .map(|(_, t)| t)
        .filter(|t| t.returns_images)
        .map(|t| t.name)
        .collect()
}

fn names(entries: &[Entry]) -> Vec<&'static str> {
    entries.iter().map(|e| e.spec.name).collect()
}

/// The image tools are the contract's: the desktop's, which `look` keeps, and the browser's
/// screenshots (kai-firefox §5.3, WP7), which `look` does not have yet (it sees a page through
/// `see_window` on Firefox's window). Every one reads.
#[test]
fn the_image_tools_are_the_contracts_seeing_tools() {
    assert_eq!(
        image_tools(),
        ["see_window", "see_screen", "page_see", "element_see"]
    );
    let mut desktop = Vec::new();
    for name in image_tools() {
        let (area, spec) = kai_desktop::contract::find(name).unwrap();
        assert!(["read", "browser"].contains(&area.name), "{name}");
        assert!(spec.hints.read_only);
        if area.name == "read" {
            desktop.push(name);
        }
    }
    assert!(
        desktop
            .iter()
            .all(|t| crate::delegate::defs::LOOK.tools.contains(t)),
        "look keeps the desktop's"
    );
}

/// Each provider of `registry::providers` that serves an image tool is listed through a view of
/// who looks at screenshots: the desktop's, and the browser's, which is never listed bare.
#[test]
fn every_provider_with_image_tools_is_listed_through_the_view() {
    let all = crate::host::registry::providers(&crate::host::tests::deps());
    let browser = |a: &dyn std::any::Any| {
        a.downcast_ref::<crate::browser::provider::BrowserTools>()
            .is_some()
    };
    assert!(
        !all.iter().any(|p| p.as_any().is_some_and(browser)),
        "never bare"
    );
    let views: Vec<&HostView> = all
        .iter()
        .filter_map(|p| p.as_any()?.downcast_ref::<HostView>())
        .collect();
    assert_eq!(views.len(), 2, "the desktop's and the browser's");
    assert!(views[1].wrapped().is_some_and(browser));
}

/// A provider of browser entries: the screenshots and a text tool.
struct Browser;

impl Provider for Browser {
    fn entries(&self) -> Vec<Entry> {
        ["page_see", "element_see", "page_dom"]
            .into_iter()
            .map(|name| {
                let (area, spec) = kai_desktop::contract::find(name).unwrap();
                Entry {
                    area: area.name,
                    spec,
                    handler: Arc::new(|_| Box::pin(async { Reply::text("") })),
                    prepare: None,
                }
            })
            .collect()
    }
}

/// The browser's view follows the setting the host sets on the desktop's: one change drops (or
/// brings back) both providers' image tools.
#[test]
fn the_browsers_image_tools_follow_the_same_setting() {
    let desk = Desk::with(LOOK_TOOLS);
    let view = HostView::new(desk.clone(), Screenshots::Main);
    let browser = view.also(Browser);
    assert_eq!(
        names(&browser.entries()),
        ["page_see", "element_see", "page_dom"]
    );
    view.set(Screenshots::Delegate);
    assert_eq!(names(&browser.entries()), ["page_dom"]);
    assert_eq!(browser.seen_by(), Screenshots::Delegate);
    view.set(Screenshots::Main);
    assert_eq!(browser.entries().len(), 3);
}

#[test]
fn the_view_drops_only_the_image_tools_and_follows_the_setting() {
    let desk = Desk::with(LOOK_TOOLS);
    let view = HostView::new(desk.clone(), Screenshots::Main);
    assert_eq!(names(&view.entries()), LOOK_TOOLS);
    view.set(Screenshots::Delegate);
    assert_eq!(
        names(&view.entries()),
        [
            "desktop_context",
            "window_list",
            "read_window",
            "read_selection"
        ]
    );
    assert_eq!(view.seen_by(), Screenshots::Delegate);
    // The host reaches it among its providers.
    let registry = crate::host::registry::Registry::new(vec![Box::new(view)]);
    assert!(registry.find::<HostView>().is_some());
}

/// Through the host: `delegate` lists `look` and not the image tools (a call of one is refused
/// as unlisted); switching to `main` relists once with `list_changed`; the same again says
/// nothing.
#[tokio::test]
async fn the_hosts_list_follows_who_looks_at_screenshots() {
    let desk = Desk::with(LOOK_TOOLS);
    let delegates = rig(
        "https://127.0.0.1:1",
        desk.clone(),
        DelegateConfig::default(),
        PolicyConfig::default(),
    )
    .tools;
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(HostView::new(desk.clone(), Screenshots::Delegate)),
        Box::new(delegates),
    ];
    let mut r = rig_with_providers(PolicyConfig::default(), settings(1), providers).await;
    let mut c = r.linked().await;
    let listed = |list: &[Value]| -> Vec<String> {
        list.iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = listed(&c.list().await);
    assert!(before.contains(&"look".to_owned()), "{before:?}");
    assert!(before.contains(&"read_window".to_owned()));
    for image in image_tools() {
        assert!(!before.contains(&image.to_owned()), "{before:?}");
    }
    let id = c.call("see_window", json!({}), owner(), 60_000).await;
    assert!(
        c.answer(id, &mut Vec::new()).await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no tool 'see_window'")
    );
    assert!(desk.calls().is_empty(), "nothing captured");
    assert!(r.host.set_screenshots(Screenshots::Main));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    let after = listed(&c.list().await);
    assert_eq!(after.len(), before.len() + 2);
    assert!(after.contains(&"see_window".to_owned()) && after.contains(&"see_screen".to_owned()));
    assert!(!r.host.set_screenshots(Screenshots::Main), "no change");
    assert!(r.host.set_screenshots(Screenshots::Delegate));
    assert_eq!(c.recv().await["method"], "notifications/tools/list_changed");
    assert_eq!(listed(&c.list().await), before);
}

/// The view, held by the test too.
struct Held(Arc<HostView>);

impl Provider for Held {
    fn entries(&self) -> Vec<Entry> {
        self.0.entries()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(&*self.0)
    }
}

/// Two settings changed at once leave the list that matches the last state, never a stale
/// relister's (`Shared::relist` builds inside the write lock).
#[tokio::test(flavor = "multi_thread")]
async fn racing_setting_changes_leave_a_list_that_matches_the_setting() {
    let desk = Desk::with(LOOK_TOOLS);
    let view = Arc::new(HostView::new(desk.clone(), Screenshots::Main));
    let providers: Vec<Box<dyn Provider>> = vec![Box::new(Held(view.clone()))];
    let r = rig_with_providers(PolicyConfig::default(), settings(1), providers).await;
    for round in 0..300 {
        std::thread::scope(|s| {
            s.spawn(|| r.host.set_screenshots(Screenshots::Delegate));
            s.spawn(|| r.host.set_screenshots(Screenshots::Main));
        });
        let want = view.seen_by();
        let listed = r.host.tool_list_json("kai");
        let has_image = listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "kai__see_window");
        assert_eq!(has_image, want == Screenshots::Main, "round {round}");
    }
}

/// With `delegate`, `look` still sees: its context read and its own `see_screen` reach the
/// desktop, the screenshots go in the delegate's request, and its caller gets text only. Its
/// `window` takes `active` too.
#[tokio::test]
async fn look_still_sees_when_the_callers_do_not() {
    let lmgw = Lmgw::start(
        folders(Some("kai-chat")),
        vec![
            tool_turn(&[("c1", "see_screen", json!({}))]),
            text_turn("done: The clock shows 14:02."),
        ],
    )
    .await;
    let settings = DelegateConfig {
        screenshots: Screenshots::Delegate,
        ..DelegateConfig::default()
    };
    let r = rig(
        &lmgw.url,
        Desk::with(LOOK_TOOLS),
        settings,
        PolicyConfig::default(),
    );
    let reply = call(
        &r.tools,
        "look",
        json!({"question": QUESTION, "window": "active"}),
        Caller::Own,
    )
    .await;
    assert_eq!(
        (text(&reply), reply.error),
        (
            "done: The clock shows 14:02.\n(used: see_screen)".to_owned(),
            false
        )
    );
    let ran: Vec<(String, Vec<String>)> = r
        .desk
        .calls()
        .into_iter()
        .map(|(tool, _, args)| (tool, args))
        .collect();
    assert!(
        ran.contains(&("see_window".to_owned(), vec!["window: active".to_owned()])),
        "{ran:?}"
    );
    assert!(ran.iter().any(|(t, _)| t == "see_screen"), "{ran:?}");
    let bodies: Vec<String> = lmgw.completions().into_iter().map(|s| s.body).collect();
    assert!(bodies[0].contains(SHOT), "the window's screenshot");
    assert!(bodies[1].contains(SCREEN), "the screen's");
}
