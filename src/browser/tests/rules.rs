//! The browser's policy (design §6.4) through the host's dispatch: K15's defaults and the
//! hands-and-eyes switch, the site rules before every call, and the focus rule as far as the
//! contract lets Kai see it (`crate::browser::moves`).

use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::fake::{Firefox, Host, Model, Twist, WP4A, shop, stranger, tab};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::host::policy::Caller;
use crate::host::settings::{PolicyConfig, ToolState};

fn switch_on() -> BrowserConfig {
    BrowserConfig {
        hands_and_eyes: true,
        ..Default::default()
    }
}

/// The note of the one question asked about `tool` since the last look, if one was.
fn asked_about(h: &Host, tool: &str) -> Option<String> {
    let a = h.asked();
    assert!(a.len() <= 1, "{a:?}");
    a.first().map(|a| {
        assert_eq!(a.tool, tool);
        assert!(
            !a.always,
            "no \"always allow\": the policy is not what asks"
        );
        a.notes.join(" ")
    })
}

const BEHIND: &str = "Asked because this is not the tab in front of you";
const UNSURE: &str = "Asked because Kai cannot tell which tab is in front of you";
const BROUGHT: &str = "Asked because Kai brought this tab";
const OPENED: &str = "Asked because Kai opened this tab";
const ELSEWHERE: &str = "Asked because Kai took this tab to another site";
const UNSEEN: &str = "Asked because Firefox's history has never seen";
const KAIS_ONLY: &str = "Asked because https://evil.example is in Firefox's history only through \
                         Kai's own visits";

/// The question about `tool` has `note` among its notes.
fn asked_with(h: &Host, tool: &str, note: &str) {
    let why = asked_about(h, tool).unwrap_or_else(|| panic!("{tool} was not asked about"));
    assert!(why.contains(note), "{why}");
}

/// The question about `tool` starts with `note`.
fn asked_because(h: &Host, tool: &str, note: &str) {
    let why = asked_about(h, tool).unwrap_or_else(|| panic!("{tool} was not asked about"));
    assert!(why.starts_with(note), "{why}");
}

fn tab_url(ff: &Firefox, id: i64, url: &str) {
    let mut m = ff.model();
    let t = m.tabs.iter_mut().find(|t| t["id"] == id).unwrap();
    t["url"] = json!(url);
}

#[tokio::test]
async fn acting_tools_are_asked_about_until_the_switch_lets_trusted_callers_through() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", WP4A, shop()).await;
    // K15's defaults: a read runs unasked for the owner; an acting tool is asked.
    h.owner("tab_list", json!({})).await;
    h.owner("browser_context", json!({})).await;
    assert!(h.asked().is_empty());
    h.owner("tab_reload", json!({})).await;
    assert_eq!(h.asked().len(), 1);
    // The switch: unasked for trusted callers, asked for the others.
    h.set(PolicyConfig::default(), switch_on());
    assert!(!h.owner("tab_reload", json!({})).await.0);
    h.owner("tab_mute", json!({"on": true})).await;
    h.owner("tab_switch", json!({"tab": "lamps"})).await;
    assert!(h.asked().is_empty());
    h.call("tab_reload", json!({"tab": "14"}), &stranger())
        .await;
    assert_eq!(h.asked().len(), 1, "an untrusted caller is asked");
    // The owner's own state for a tool wins over the switch.
    let mut policy = PolicyConfig::default();
    policy.tools.insert("tab_reload".into(), ToolState::Confirm);
    policy.tools.insert("tab_mute".into(), ToolState::Deny);
    h.set(policy, switch_on());
    h.owner("tab_reload", json!({})).await;
    assert_eq!(h.asked().len(), 1);
    let (err, text) = h.owner("tab_mute", json!({"on": false})).await;
    assert!(
        err && text.contains("policy.tools.tab_mute = \"deny\""),
        "{text}"
    );
}

#[tokio::test]
async fn site_rules_keep_kai_off_none_sites_and_its_hands_off_read_ones() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                none: vec!["mail.example".into()],
                read: vec!["shop.example".into()],
                act: Vec::new(),
            },
            ..switch_on()
        },
    );
    assert_eq!(
        h.owner("tab_list", json!({})).await.1,
        "12 \"Checkout\" https://shop.example/cart (window 1, the user's tab)\n\
         14 \"Lamps\" https://shop.example/lamps (window 2, active)\n\
         1 more tab on sites Kai does not reach (Kai's site rules)"
    );
    for (tool, args) in [
        ("tab_close", json!({"tab": "3"})),
        ("tab_switch", json!({"tab": "3"})),
        ("tab_mute", json!({"tab": "3", "on": true})),
    ] {
        assert_eq!(
            h.owner(tool, args).await,
            (
                true,
                format!(
                    "tab 3 is on a site Kai does not reach (Kai's site rules); {tool} did not run"
                )
            )
        );
    }
    assert_eq!(
        h.owner("tab_close", json!({"tab": "inbox"})).await.1,
        "no tab has that id, title or URL (tab_list lists them)",
        "never matched by its title"
    );
    assert_eq!(
        h.owner(
            "tab_go",
            json!({"url": "https://mail.example/x", "tab": "14"})
        )
        .await,
        (
            true,
            "Kai does not go to https://mail.example/x: Kai's site rules keep it away from that \
             site"
                .to_owned()
        )
    );
    // A read site: what acts on the page is refused, the tab itself may be muted or shown.
    assert_eq!(
        h.owner("tab_reload", json!({})).await,
        (
            true,
            "\"Checkout\" https://shop.example/cart (tab 12) is on a site where Kai may only \
             read (Kai's site rules); tab_reload did not run"
                .to_owned()
        )
    );
    assert_eq!(
        h.owner("tab_mute", json!({"on": true})).await,
        (
            false,
            "\"Checkout\" https://shop.example/cart (tab 12): muted".to_owned()
        )
    );
    assert_eq!(
        ff.acted(),
        [("tabs.mute@1".to_owned(), json!({"tab": 12, "on": true}))]
    );
    // The user on a none site: browser_context names the tab only (the switch does not turn the
    // announcements on, so no line about them).
    ff.model().user_goes_to(3);
    assert_eq!(
        h.owner("browser_context", json!({})).await.1,
        "the user's tab (tab 3) shows a site Kai does not reach (Kai's site rules)\ntabs open: \
         3"
    );
    assert!(h.asked().is_empty());
}

#[tokio::test]
async fn the_focus_rule_asks_away_from_the_tab_the_user_has_in_front() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(PolicyConfig::default(), switch_on());

    // The user's own tab: as the policy says (unasked with the switch).
    h.owner("tab_reload", json!({})).await;
    assert_eq!(asked_about(&h, "tab_reload"), None);
    // Another tab: asked, and the dialog says why.
    h.owner("tab_reload", json!({"tab": "14"})).await;
    asked_because(&h, "tab_reload", BEHIND);
    // Unless Kai may act in background tabs.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            act_in_background: true,
            ..switch_on()
        },
    );
    h.owner("tab_reload", json!({"tab": "14"})).await;
    assert_eq!(asked_about(&h, "tab_reload"), None);
    h.set(PolicyConfig::default(), switch_on());

    // A tab Kai brought to the front is not the user's until they come back to it.
    h.owner("tab_switch", json!({"tab": "14"})).await;
    assert_eq!(
        asked_about(&h, "tab_switch"),
        None,
        "switching acts on no page"
    );
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", BROUGHT);
    ff.model().user_goes_to(12);
    h.owner("tab_list", json!({})).await;
    ff.model().user_goes_to(14);
    h.owner("tab_reload", json!({})).await;
    assert_eq!(
        asked_about(&h, "tab_reload"),
        None,
        "the user came back to it"
    );

    // A tab Kai took to another site is asked about there; the navigation itself, to a site the
    // history has seen, on the user's tab, is not.
    h.owner("tab_go", json!({"url": "https://news.example/"}))
        .await;
    assert_eq!(asked_about(&h, "tab_go"), None);
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", ELSEWHERE);
    // The user leaves and comes back: the site stays the one they had there.
    ff.model().user_goes_to(12);
    h.owner("tab_list", json!({})).await;
    ff.model().user_goes_to(14);
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", ELSEWHERE);
    h.owner("tab_go", json!({"url": "https://shop.example/x"}))
        .await;
    let _ = h.asked();
    h.owner("tab_reload", json!({})).await;
    assert_eq!(
        asked_about(&h, "tab_reload"),
        None,
        "back on the user's site"
    );

    // A tab Kai opened.
    h.owner("tab_open", json!({"url": "https://news.example/"}))
        .await;
    assert_eq!(asked_about(&h, "tab_open"), None);
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", OPENED);

    // A trusted approver approved it in lmgw: no second question, but it must name its tab.
    assert!(!h.approved("tab_reload", json!({"tab": "15"})).await.0);
    assert_eq!(asked_about(&h, "tab_reload"), None);
    let (err, text) = h.approved("tab_reload", json!({})).await;
    assert!(
        err && text.contains("approved in lmgw before it ran"),
        "{text}"
    );
}

/// An approval that does not count (an untrusted approver, or a caller approving its own call)
/// spares no question: the focus rule asks, and the call still names its tab.
#[tokio::test]
async fn an_approval_that_does_not_count_spares_no_question() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    let owner = Caller::Owner;
    let (err, text) = h
        .approved_by("tab_reload", json!({"tab": "14"}), &owner, &stranger())
        .await;
    assert!(!err, "{text}");
    asked_because(&h, "tab_reload", BEHIND);
    let (err, text) = h
        .approved_by("tab_reload", json!({}), &owner, &stranger())
        .await;
    assert!(
        err && text.contains("approved in lmgw before it ran"),
        "{text}"
    );
    // A trusted approver of the owner's call: the approver saw the tab, nothing more is asked.
    h.approved_by("tab_reload", json!({"tab": "14"}), &owner, &owner)
        .await;
    assert_eq!(asked_about(&h, "tab_reload"), None);
}

/// Calls `tool` and cancels it once `op` reached Firefox, as lmgw does at its deadline.
async fn cancelled_at(h: &Host, ff: &Firefox, tool: &str, args: Value, op: &'static str) {
    ff.model().ops.clear();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let model = ff.model.clone();
    let ((err, text), ()) = tokio::join!(
        h.call_until(tool, args, &Caller::Owner, cancel),
        async move {
            while !model.lock().unwrap().ops.iter().any(|(o, _)| o == op) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            stop.cancel();
        }
    );
    assert!(err, "{text}");
}

/// A navigation goes on in Firefox after Kai stopped waiting for it (`nav_timeout`, or lmgw
/// cancelled the call): Kai marked its move before it sent it, so the next act is asked about.
#[tokio::test]
async fn a_navigation_kai_stopped_waiting_for_is_marked_all_the_same() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    ff.model().twists.insert(
        "tabs.go@1",
        Twist::FailAfter("nav_timeout", "the page has not finished loading"),
    );
    let (err, text) = h
        .owner("tab_go", json!({"url": "https://news.example/"}))
        .await;
    assert!(
        err && text.starts_with("the page did not load in time"),
        "{text}"
    );
    let _ = h.asked();
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", ELSEWHERE);

    // Cancelled while the page loads, on the other window's tab.
    ff.model().user_goes_to(14);
    ff.model().twists.insert("tabs.go@1", Twist::Hang);
    cancelled_at(
        &h,
        &ff,
        "tab_go",
        json!({"url": "https://news.example/"}),
        "tabs.go@1",
    )
    .await;
    let _ = h.asked();
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", ELSEWHERE);

    // tab_open, cancelled while its page loads: the new tab is Kai's.
    ff.model().twists.insert("tabs.open@1", Twist::Hang);
    cancelled_at(
        &h,
        &ff,
        "tab_open",
        json!({"url": "https://news.example/"}),
        "tabs.open@1",
    )
    .await;
    let _ = h.asked();
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", OPENED);
    // And timed out.
    ff.model().twists.insert(
        "tabs.open@1",
        Twist::FailAfter("nav_timeout", "the site has not answered"),
    );
    let (err, _) = h
        .owner("tab_open", json!({"url": "https://news.example/"}))
        .await;
    assert!(err);
    let _ = h.asked();
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", OPENED);
}

/// Kai closes the user's tab: the tab Firefox shows in its place is Kai's doing, not the user's
/// choice, until they leave it and come back.
#[tokio::test]
async fn the_tab_in_place_of_the_users_tab_kai_closed_is_not_the_users() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    h.owner("tab_close", json!({})).await;
    assert_eq!(asked_about(&h, "tab_close"), None);
    assert_eq!(
        ff.model().tabs.iter().find(|t| t["user"] == true).unwrap()["id"],
        3
    );
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", BROUGHT);
    ff.model().user_goes_to(14);
    h.owner("tab_list", json!({})).await;
    ff.model().user_goes_to(3);
    h.owner("tab_reload", json!({})).await;
    assert_eq!(asked_about(&h, "tab_reload"), None);
}

/// Two profiles each with a tab in front, or one whose tabs could not be listed: Kai cannot tell
/// which tab the user has in front of them, and asks.
#[tokio::test]
async fn with_two_profiles_in_front_or_one_unlisted_kai_asks() {
    let h = Host::start().await;
    let _a = h.firefox("c0ffee", WP4A, shop()).await;
    let b = h
        .firefox(
            "beef",
            WP4A,
            Model::with(vec![tab(5, 1, "News", "https://news.example/", true)]),
        )
        .await;
    h.set(PolicyConfig::default(), switch_on());
    h.owner("tab_reload", json!({"tab": "12"})).await;
    asked_because(&h, "tab_reload", UNSURE);
    // beef's listing fails: its user's tab may be the one in front.
    b.model()
        .twists
        .insert("tabs.list@1", Twist::Fail("internal", "the listing failed"));
    let (err, text) = h.owner("tab_reload", json!({})).await;
    assert!(!err, "{text}");
    assert!(text.contains("(tab 12)"), "{text}");
    asked_because(&h, "tab_reload", UNSURE);
}

/// The commit guard on navigations (§6.4): `tab_go` and `tab_open` to a site the user has never
/// visited (`history.seen@1`'s `seen_by_user`, asked for the page's real site) are asked about,
/// with the switch on too; a site the user visited, a search, `about:blank` and a trusted
/// approver's approval in lmgw (the approver saw the URL) are not. A site in the history only
/// through Kai's own navigations is asked about until the user acts on it; an extension that does
/// not say `seen_by_user`, and a URL without a site a history could have seen (`bad_url`), are
/// asked about too.
#[tokio::test]
async fn navigations_to_a_site_the_user_never_visited_are_asked_about() {
    let h = Host::start().await;
    let mut model = shop();
    model.unvisited.insert("https://evil.example".into());
    let ff = h.firefox("c0ffee", WP4A, model).await;
    h.set(PolicyConfig::default(), switch_on());
    h.owner("tab_back", json!({})).await;
    assert_eq!(asked_about(&h, "tab_back"), None, "no URL of the model's");
    for (tool, args) in [
        ("tab_go", json!({"url": "https://news.example/?q=x"})),
        ("tab_go", json!({"url": "the owner's address"})),
        ("tab_open", json!({"url": "lamps"})),
        ("tab_open", json!({"url": "about:blank"})),
    ] {
        let (err, text) = h.owner(tool, args).await;
        assert!(!err, "{text}");
        let why = asked_about(&h, tool).unwrap_or_default();
        assert!(
            !why.contains("history"),
            "{tool}: seen, a search, a blank page: {why}"
        );
    }
    ff.model().user_goes_to(12);
    for (tool, args) in [
        ("tab_go", json!({"url": "https://evil.example/?q=secret"})),
        ("tab_open", json!({"url": "evil.example"})),
    ] {
        let (err, text) = h.owner(tool, args).await;
        assert!(!err, "{text}");
        asked_with(&h, tool, UNSEEN);
    }
    // The history has it through Kai's navigations only: still asked.
    {
        let mut m = ff.model();
        m.unvisited.clear();
        m.kai_only.insert("https://evil.example".into());
        m.user_goes_to(3);
    }
    h.owner(
        "tab_go",
        json!({"url": "https://evil.example/2", "tab": "3"}),
    )
    .await;
    asked_with(&h, "tab_go", KAIS_ONLY);
    // Until the user acts on it themselves.
    {
        let mut m = ff.model();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 3).unwrap();
        t["user_origin"] = json!("https://evil.example");
        t["url"] = json!("https://evil.example/2");
    }
    h.owner(
        "tab_go",
        json!({"url": "https://evil.example/3", "tab": "3"}),
    )
    .await;
    let why = asked_about(&h, "tab_go").unwrap_or_default();
    assert!(!why.contains("history"), "{why}");
    // History is asked for the page's real site: Reader view's, view-source's.
    let n = ff.count("history.seen@1");
    h.owner(
        "tab_go",
        json!({"url": "view-source:https://evil.example/x", "tab": "3"}),
    )
    .await;
    let _ = h.asked();
    assert_eq!(ff.count("history.seen@1"), n, "the user acted on that site");
    ff.model().kai_only.insert("https://far.example".into());
    h.owner(
        "tab_go",
        json!({"url": "view-source:https://far.example/x", "tab": "3"}),
    )
    .await;
    asked_with(
        &h,
        "tab_go",
        "https://far.example is in Firefox's history only through Kai's own visits",
    );
    let (_, args) = ff
        .model()
        .ops
        .iter()
        .rev()
        .find(|(o, _)| o == "history.seen@1")
        .cloned()
        .unwrap();
    assert_eq!(args["url"], "https://far.example/x");
    // An extension that does not say whether the user was there: asked.
    ff.model().no_seen_by_user = true;
    h.owner(
        "tab_go",
        json!({"url": "https://news.example/", "tab": "3"}),
    )
    .await;
    asked_with(
        &h,
        "tab_go",
        "did not say whether you have been to https://news.example",
    );
    ff.model().no_seen_by_user = false;
    // A URL without a site a history could have seen: asked.
    h.owner("tab_go", json!({"url": "data:text/html,hi", "tab": "3"}))
        .await;
    asked_with(
        &h,
        "tab_go",
        "no page of a site Firefox's history could have seen",
    );
    // A trusted approver saw the URL.
    ff.model().kai_only.insert("https://news.example".into());
    let n = ff.count("history.seen@1");
    h.approved(
        "tab_go",
        json!({"url": "https://news.example/", "tab": "3"}),
    )
    .await;
    assert_eq!(asked_about(&h, "tab_go"), None);
    assert_eq!(ff.count("history.seen@1"), n, "no history asked either");
    // The guard off: nothing asked.
    ff.model().unvisited.insert("https://far.example".into());
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            commit_guard: false,
            ..switch_on()
        },
    );
    let n = ff.count("history.seen@1");
    h.owner("tab_go", json!({"url": "https://far.example/", "tab": "3"}))
        .await;
    let why = asked_about(&h, "tab_go").unwrap_or_default();
    assert!(!why.contains("history"), "{why}");
    assert_eq!(ff.count("history.seen@1"), n);
}

/// After the owner was asked, Kai lists the tabs again: a tab that went to another site or closed
/// while the dialog waited is not acted on.
#[tokio::test]
async fn a_tab_that_moved_while_the_owner_was_asked_is_not_acted_on() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    let model = ff.model.clone();
    h.while_asked(move || {
        let mut m = model.lock().unwrap();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 14).unwrap();
        t["url"] = json!("https://evil.example/");
    });
    assert_eq!(
        h.owner("tab_reload", json!({"tab": "14"})).await,
        (
            true,
            "\"Lamps\" https://evil.example/ (tab 14) went to another site than the dialog \
             showed while the owner was asked; tab_reload did not run"
                .to_owned()
        )
    );
    assert!(ff.acted().is_empty());
    // The same site: as asked.
    let model = ff.model.clone();
    h.while_asked(move || {
        let mut m = model.lock().unwrap();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 14).unwrap();
        t["url"] = json!("https://evil.example/other");
    });
    assert!(!h.owner("tab_reload", json!({"tab": "14"})).await.0);
    tab_url(&ff, 14, "https://shop.example/lamps");
    let model = ff.model.clone();
    h.while_asked(move || model.lock().unwrap().tabs.retain(|t| t["id"] != 3));
    assert_eq!(
        h.owner("tab_close", json!({"tab": "3"})).await,
        (
            true,
            "tab 3 closed while the owner was asked; tab_close did not run".to_owned()
        )
    );
    assert_eq!(
        ff.acted(),
        [("tabs.reload@1".to_owned(), json!({"tab": 14}))]
    );
}

/// Kai's marks belong to the profile: a link that drops and comes back keeps them; a Firefox that
/// restarted (not one tab as it was) starts clean.
#[tokio::test]
async fn kais_marks_outlive_a_new_link_but_not_a_restart_of_firefox() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", WP4A, shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    h.owner("tab_switch", json!({"tab": "14"})).await;
    let tabs = ff.model().tabs.clone();
    ff.quit();
    h.rig.links_are(0).await;
    let ff = h.firefox("c0ffee", WP4A, Model::with(tabs)).await;
    h.owner("tab_reload", json!({})).await;
    asked_because(&h, "tab_reload", BROUGHT);
    ff.quit();
    h.rig.links_are(0).await;
    let _ff = h
        .firefox(
            "c0ffee",
            WP4A,
            Model::with(vec![
                tab(13, 1, "News", "https://news.example/", false),
                tab(14, 1, "News 2", "https://news.example/2", true),
            ]),
        )
        .await;
    h.owner("tab_reload", json!({})).await;
    assert_eq!(asked_about(&h, "tab_reload"), None);
}
