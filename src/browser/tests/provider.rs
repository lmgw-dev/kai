//! The browser provider through the host's registry and dispatch against a fake Kai for Firefox:
//! D3's listing (by the operations one linked extension serves, the union over profiles, the
//! hands-and-eyes switch), each tab tool's lines and the operation it sends, the tab's resolution,
//! and the extension's errors in words.

use std::sync::Arc;

use kai_desktop::contract::browser::AREA;
use serde_json::json;

use super::fake::{BUILT, Host, Model, Twist, WP4A, shop, tab};
use crate::browser::links::LinkInfo;
use crate::browser::provider::BrowserTools;
use crate::browser::settings::BrowserConfig;
use crate::browser::tools::{Inner, Reach, Route, route};
use crate::host::registry::{Call, Reply};
use crate::host::settings::PolicyConfig;
use crate::voice::link::Pending;

#[tokio::test]
async fn a_tool_is_listed_while_one_linked_firefox_serves_all_it_needs() {
    let h = Host::start().await;
    assert!(h.names().is_empty(), "no Firefox, no browser tools");
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    assert_eq!(h.names(), BUILT);
    ff.quit();
    h.rig.links_are(0).await;
    assert!(h.names().is_empty());
    // A Firefox with part of the operations: what they serve in full.
    let _a = h
        .firefox("a", &["tabs.list@1", "tabs.go@1", "history.seen@1"], shop())
        .await;
    assert_eq!(h.names(), ["tab_list", "tab_go"]);
    // A second profile adds its own; one tool's operations never come from two.
    let _b = h
        .firefox("b", &["context.get@1", "tabs.close@1"], shop())
        .await;
    assert_eq!(h.names(), ["browser_context", "tab_list", "tab_go"]);
}

fn links_run(_: Arc<Inner>, _: Call) -> Pending<Reply> {
    Box::pin(async { Reply::text("links") })
}

/// `page_links` built, as WP9's table will have it.
fn with_links(name: &str) -> Route {
    match name {
        "page_links" => Route::Built {
            ops: &["page.links@1"],
            reach: Reach::Read,
            run: links_run,
        },
        other => crate::browser::tools::route(other),
    }
}

#[tokio::test]
async fn the_hands_and_eyes_tools_are_listed_while_the_switch_is_on() {
    let h = Host::with_routes(with_links).await;
    let _ff = h
        .firefox("c0ffee", &["tabs.list@1", "page.links@1"], shop())
        .await;
    assert_eq!(h.names(), ["tab_list"]);
    let on = BrowserConfig {
        hands_and_eyes: true,
        ..Default::default()
    };
    assert!(h.set(PolicyConfig::default(), on.clone()), "listed again");
    assert_eq!(h.names(), ["tab_list", "page_links"]);
    assert!(!h.set(PolicyConfig::default(), on), "no change, no relist");
    assert!(h.set(PolicyConfig::default(), BrowserConfig::default()));
    assert_eq!(h.names(), ["tab_list"]);
}

#[tokio::test]
async fn tab_list_says_one_line_per_tab_and_each_profile_under_its_heading() {
    let h = Host::start().await;
    let _a = h.firefox("c0ffee", WP4A, shop()).await;
    let shop_lines = "3 \"Mail – Inbox\" https://mail.example/inbox (window 1)\n\
                      12 \"Checkout\" https://shop.example/cart (window 1, the user's tab)\n\
                      14 \"Lamps\" https://shop.example/lamps (window 2, active)";
    assert_eq!(
        h.owner("tab_list", json!({})).await,
        (false, shop_lines.to_owned())
    );
    assert!(h.asked().is_empty(), "a read runs unasked for the owner");
    let mut news = tab(5, 1, "News", "https://news.example/", true);
    news["pinned"] = json!(true);
    news["muted"] = json!(true);
    news["container"] = json!("firefox-container-1");
    let mut private = Model::with(vec![news, tab(6, 1, "x\ny", "https://x.example/", false)]);
    private.tabs[0]["user"] = json!(false);
    let _b = h.firefox("beef", WP4A, private).await;
    assert_eq!(
        h.owner("tab_list", json!({})).await.1,
        format!(
            "profile c0ffee (Firefox 159.0):\n{shop_lines}\nprofile beef (Firefox 159.0):\n\
             5 \"News\" https://news.example/ (active, pinned, muted, container \
             firefox-container-1)\n\
             6 \"x\\ny\" https://x.example/\n\
             the user is in a private window, which Kai has no access to"
        )
    );
}

#[tokio::test]
async fn tab_list_tells_no_window_from_a_private_one() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", WP4A, Model::with(Vec::new())).await;
    assert_eq!(
        h.owner("tab_list", json!({})).await,
        (
            false,
            "no tabs: Firefox has no window open, or only private windows, which Kai has no \
             access to"
                .to_owned()
        )
    );
}

#[tokio::test]
async fn browser_context_starts_with_the_users_tab() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    assert_eq!(
        h.owner("browser_context", json!({})).await,
        (
            false,
            "\"Checkout\" https://shop.example/cart (tab 12)\ntabs open: 3".to_owned()
        )
    );
    ff.model().twists.insert(
        "context.get@1",
        Twist::Fail("private_window", "the user is in a private window"),
    );
    assert_eq!(
        h.owner("browser_context", json!({})).await,
        (
            true,
            "the tab is in a private window, which Kai has no access to".to_owned()
        )
    );
}

#[tokio::test]
async fn each_tab_tool_acts_on_the_tab_it_names_and_says_what_it_did() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;

    // tab_go: text that is no URL is a search, on the user's tab by default.
    assert_eq!(
        h.owner("tab_go", json!({"url": "lamps shop"})).await,
        (
            false,
            "\"Went\" https://search.example/?q=lamps+shop (tab 12): went there, a search for \
             \"lamps shop\""
                .to_owned()
        )
    );
    // The confirmation (no switch: the owner is asked) names the tab.
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].lines,
        [
            "url: lamps shop",
            "acts on: 12 \"Checkout\" https://shop.example/cart"
        ]
    );
    assert!(
        asked[0].notes.is_empty(),
        "the user's tab: the focus rule is quiet; a search goes to the user's own engine, so the \
         commit guard is too: {:?}",
        asked[0].notes
    );
    // A host is a URL; the tab by a fragment of its title.
    assert_eq!(
        h.owner("tab_go", json!({"url": "news.example", "tab": "lamps"}))
            .await
            .1,
        "\"Went\" https://news.example/ (tab 14): went there"
    );
    assert_eq!(
        ff.acted(),
        [
            (
                "tabs.go@1".to_owned(),
                json!({"tab": 12, "search": "lamps shop"})
            ),
            (
                "tabs.go@1".to_owned(),
                json!({"tab": 14, "url": "https://news.example/"})
            ),
        ]
    );
    assert_eq!(
        h.owner("tab_open", json!({"url": "shop.example/lamps"}))
            .await
            .1,
        "\"Opened\" https://shop.example/lamps (tab 15): opened in a new tab"
    );
    // In Kai's own container (§6.4).
    assert_eq!(
        h.owner("tab_open", json!({"url": "x", "sandbox": true}))
            .await,
        (
            false,
            "\"Opened\" https://search.example/?q=x (tab 16): opened in a new tab in Kai's own \
             container, a search for \"x\""
                .to_owned()
        )
    );
    assert_eq!(
        ff.acted().last().unwrap().1,
        json!({"search": "x", "sandbox": true})
    );
    let asked = h.asked();
    assert!(
        asked
            .last()
            .unwrap()
            .lines
            .contains(&"in Kai's own container, without the user's cookies".to_owned()),
        "{asked:?}"
    );
    assert_eq!(
        h.owner("tab_switch", json!({"tab": "3"})).await.1,
        "\"Mail – Inbox\" https://mail.example/inbox (tab 3): in front now"
    );
    ff.model().focus_follows = false;
    let (_, text) = h.owner("tab_switch", json!({"tab": "15"})).await;
    assert!(
        text.starts_with("\"Opened\" https://shop.example/lamps (tab 15): the active tab"),
        "{text}"
    );
    assert!(text.ends_with("so it is not the user's tab"), "{text}");
    assert_eq!(
        h.owner("tab_mute", json!({"on": true})).await.1,
        "\"Mail – Inbox\" https://mail.example/inbox (tab 3): muted"
    );
    assert_eq!(
        h.owner("tab_back", json!({"tab": "news"})).await.1,
        "\"Went\" https://news.example/ (tab 14): went back"
    );
    assert_eq!(
        h.owner("tab_forward", json!({"tab": "14"})).await.1,
        "\"Went\" https://news.example/ (tab 14): went forward"
    );
    assert_eq!(
        h.owner("tab_reload", json!({})).await.1,
        "\"Mail – Inbox\" https://mail.example/inbox (tab 3): reloaded"
    );
    assert_eq!(
        h.owner("tab_close", json!({"tab": "news"})).await.1,
        "\"Went\" https://news.example/ (tab 14): closed"
    );
    assert!(ff.model().tabs.iter().all(|t| t["id"] != 14));
}

#[tokio::test]
async fn a_tab_that_is_not_one_is_said_and_nothing_runs() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    assert_eq!(
        h.owner("tab_close", json!({"tab": "shop.example"})).await,
        (
            true,
            "2 tabs match; name one by its id (tab_list lists them all):\n\
             12 \"Checkout\" https://shop.example/cart\n\
             14 \"Lamps\" https://shop.example/lamps"
                .to_owned()
        )
    );
    assert_eq!(
        h.owner("tab_close", json!({"tab": "bank"})).await,
        (
            true,
            "no tab has that id, title or URL (tab_list lists them)".to_owned()
        )
    );
    assert!(ff.acted().is_empty());
    assert!(h.asked().is_empty(), "refused before anyone is asked");
    // Two profiles, each with a tab in front: the call names its tab.
    let _b = h
        .firefox(
            "beef",
            &["tabs.list@1"],
            Model::with(vec![tab(5, 1, "News", "https://news.example/", true)]),
        )
        .await;
    let (err, text) = h.owner("tab_reload", json!({})).await;
    assert!(
        err && text.starts_with("Firefox runs in 2 profiles"),
        "{text}"
    );
    assert!(text.contains("\n12 \"Checkout\" https://shop.example/cart in profile c0ffee"));
    assert!(text.contains("\n5 \"News\" https://news.example/ in profile beef"));
    // Listed through c0ffee, the tab in beef, which cannot reload: said by profile.
    assert_eq!(
        h.owner("tab_reload", json!({"tab": "news"})).await,
        (
            true,
            "Kai for Firefox in profile beef does not offer tabs.reload@1; an update of the \
             extension there brings it"
                .to_owned()
        )
    );
}

#[tokio::test]
async fn the_extensions_errors_read_in_words() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    {
        let mut m = ff.model();
        m.twists.insert(
            "tabs.back@1",
            Twist::Fail("no_history", "tab 12 has one page"),
        );
        m.twists.insert(
            "tabs.go@1",
            Twist::Fail("tel_refused", "tel: links open no page"),
        );
        m.twists.insert(
            "tabs.forward@1",
            Twist::Answer(json!({
                "tab": tab(12, 1, "Problem loading page", "https://nowhere.example/", true),
                "loaded": false, "load_error": "NS_ERROR_UNKNOWN_HOST"})),
        );
        m.twists.insert("tabs.reload@1", Twist::Quit);
    }
    assert_eq!(
        h.owner("tab_back", json!({})).await,
        (
            true,
            "the tab has only this one page in its history".to_owned()
        )
    );
    assert_eq!(
        h.owner("tab_go", json!({"url": "tel:+12025550123"})).await,
        (
            true,
            "Kai for Firefox answered with an error Kai does not know yet (tel_refused): tel: \
             links open no page"
                .to_owned()
        )
    );
    assert_eq!(
        h.owner("tab_forward", json!({})).await,
        (
            false,
            "\"Problem loading page\" https://nowhere.example/ (tab 12): went forward; the page \
             did not load: no such host (NS_ERROR_UNKNOWN_HOST)"
                .to_owned()
        )
    );
    assert_eq!(
        h.owner("tab_reload", json!({})).await,
        (true, "Firefox closed while the call ran".to_owned())
    );
}

/// Every built tool that does more than read, or takes a ref, is prepared: its site rules and
/// the focus rule cannot be left out by the work package that builds it (WP7, WP9).
#[tokio::test]
async fn every_built_tool_that_acts_or_takes_a_ref_is_prepared() {
    let check = |routes: fn(&str) -> Route| {
        let ops: Vec<String> = AREA
            .tools
            .iter()
            .filter_map(|t| match routes(t.name) {
                Route::Built { ops, .. } => Some(ops.iter().map(|o| (*o).to_owned())),
                Route::NotYet => None,
            })
            .flatten()
            .collect();
        let rig_links = crate::browser::Links::new(crate::host::registry::Changes::new().0);
        let policy = Arc::new(std::sync::RwLock::new(crate::host::policy::Policy::new(
            &PolicyConfig::default(),
        )));
        let tools = BrowserTools::with_routes(rig_links, policy, routes);
        let linked = [LinkInfo {
            id: 1,
            profile: "c0ffee".into(),
            ext_version: "0.1.0".into(),
            firefox: "159.0".into(),
            protocol: 1,
            ops,
            firefox_pid: None,
        }];
        let entries = tools.entries_for(&linked, true);
        // Without the desktop's accessibility (`NoDesk`), the tools that need it are not listed
        // (`tests::chrome` lists them with it).
        let built = AREA
            .tools
            .iter()
            .filter(|t| matches!(routes(t.name), Route::Built { .. }))
            .filter(|t| !crate::browser::provider::needs_access(t.name))
            .count();
        assert_eq!(entries.len(), built, "every built tool is listed");
        for e in &entries {
            let Route::Built { reach, .. } = routes(e.spec.name) else {
                unreachable!("listed")
            };
            if reach != Reach::Read || e.spec.param("ref").is_some() {
                assert!(e.prepare.is_some(), "{} has no preparation", e.spec.name);
            }
        }
        entries
    };
    check(route);
}
