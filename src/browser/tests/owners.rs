//! The focus rule hands free after kai-firefox WP10b's review (H1, L1, L2, M3): which moves of a
//! turn the owner started count as the owner's own act (a pressed link or submit control, an Enter
//! that submits a form, the tab tools), only where the tab landed where the move was meant to go;
//! every acting operation marked as Kai's navigation before it is sent; the owner's marks with the
//! switch off, after a close that failed and while Kai's continuation for jobs' results runs; the
//! tab tools for every other caller.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::fake::{Firefox, Host, Twist, all_ops, shop, stranger, tab};
use crate::browser::settings::BrowserConfig;
use crate::host::policy::Caller;
use crate::host::settings::{PolicyConfig, ToolState};

const CART: &str = "https://shop.example/cart";
const ELSEWHERE: &str = "https://elsewhere.example/";
const NEWS: &str = "https://news.example/";

fn switch(on: bool) -> BrowserConfig {
    BrowserConfig {
        hands_and_eyes: on,
        ..Default::default()
    }
}

/// Every tool the tests call allowed for any caller: the focus rule is what asks.
fn allowed() -> PolicyConfig {
    let mut tools = BTreeMap::new();
    for t in [
        "element_press",
        "element_fill",
        "element_hover",
        "element_scroll",
        "page_scroll",
        "page_key",
        "tab_go",
        "tab_open",
        "tab_switch",
        "tab_back",
        "tab_close",
    ] {
        tools.insert(t.to_owned(), ToolState::Allow);
    }
    PolicyConfig {
        tools,
        ..Default::default()
    }
}

fn element(ff: &Firefox, r: i64, tab: i64, role: &str, name: &str) {
    ff.model()
        .elements
        .insert(r, json!({"tab": tab, "role": role, "name": name}));
}

fn with(ff: &Firefox, r: i64, key: &str, v: Value) {
    ff.model().elements.get_mut(&r).unwrap()[key] = v;
}

/// Points tab `id` of the fake Firefox at `url`, as a navigation there leaves it.
fn goes(ff: &Firefox, id: i64, url: &str) {
    let mut m = ff.model();
    let t = m.tabs.iter_mut().find(|t| t["id"] == id).unwrap();
    t["url"] = json!(url);
}

/// An acting answer whose page was left for `url`, with the extension's `goes` given.
fn left_for(url: &str, goes: Value) -> Value {
    json!({"after": "page_left", "tab": {"id": 12, "title": "Away", "url": url}, "goes": goes})
}

fn link_to(origin: &str) -> Value {
    json!({"by": "link", "origin": origin})
}

/// Fills the search field (element 61) as `caller`: what the focus rule asked, if it did.
async fn fill_asked(h: &Host, caller: &Caller) -> Option<String> {
    let _ = h.asked();
    let (err, text) = h
        .call("element_fill", json!({"ref": 61, "text": "lamp"}), caller)
        .await;
    assert!(!err, "{text}");
    h.asked().first().map(|a| a.notes.join(" "))
}

/// H1: with the switch on, a scroll, a fill, a hover, a key or a page scroll of the owner's own
/// turn whose page goes elsewhere by itself (a page that navigates on a scroll, an input, a
/// hover, a key) is Kai's navigation: acting there is asked, though the answer names a link. An
/// Enter that submits a form to where the tab landed is the owner's.
#[tokio::test]
async fn an_owners_act_whose_page_leaves_by_itself_stays_asked() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 41, 12, "textbox", "Full name");
    element(&ff, 42, 12, "region", "Reviews");
    element(&ff, 43, 12, "link", "Lamps");
    element(&ff, 61, 12, "searchbox", "Search");
    let away = left_for(ELSEWHERE, link_to("https://elsewhere.example"));
    for (tool, args, r) in [
        ("element_scroll", json!({"ref": 42, "to": "down"}), 42),
        ("element_fill", json!({"ref": 41, "text": "Ada"}), 41),
        ("element_hover", json!({"ref": 43}), 43),
    ] {
        goes(&ff, 12, CART);
        with(&ff, r, "answer", away.clone());
        let (err, text) = h.call(tool, args, &Caller::Own).await;
        assert!(!err, "{tool}: {text}");
        assert!(h.asked().is_empty(), "{tool} itself is unasked");
        goes(&ff, 12, ELSEWHERE);
        let asked = fill_asked(&h, &Caller::Own).await;
        assert!(
            asked
                .as_deref()
                .is_some_and(|n| n.contains("another site than the one you had open")),
            "{tool}: {asked:?}"
        );
        with(&ff, r, "answer", json!({}));
    }
    // A page scroll and keys, answered as the extension answers after a navigation the page made.
    let page = json!({"id": 12, "title": "Away", "url": ELSEWHERE});
    for (op, tool, args, goes_to, owners) in [
        (
            "page.scroll@1",
            "page_scroll",
            json!({"to": "down"}),
            Value::Null,
            false,
        ),
        (
            "page.key@1",
            "page_key",
            json!({"keys": "ArrowDown"}),
            Value::Null,
            false,
        ),
        // An Enter on a link is not the owner's move: only a form's submit is.
        (
            "page.key@1",
            "page_key",
            json!({"keys": "Enter"}),
            link_to("https://elsewhere.example"),
            false,
        ),
        // An Enter that submits a form, landed on its action's origin: the owner's.
        (
            "page.key@1",
            "page_key",
            json!({"keys": "Enter"}),
            json!({"by": "submit", "origin": "https://elsewhere.example"}),
            true,
        ),
    ] {
        goes(&ff, 12, CART);
        let mut answer = json!({"tab": page.clone(), "keys": args["keys"], "target": null,
                                "prevented": false, "emulated": "submit", "scrolled": true,
                                "container": null, "percent": 40, "at_top": false,
                                "at_bottom": false});
        if !goes_to.is_null() {
            answer["goes"] = goes_to.clone();
        }
        ff.model().twists.insert(op, Twist::Answer(answer));
        let (err, text) = h.call(tool, args.clone(), &Caller::Own).await;
        assert!(!err, "{tool}: {text}");
        ff.model().twists.remove(op);
        assert!(h.asked().is_empty());
        goes(&ff, 12, ELSEWHERE);
        let asked = fill_asked(&h, &Caller::Own).await;
        assert_eq!(
            asked.is_none(),
            owners,
            "{tool} {args} {goes_to}: {asked:?}"
        );
    }
}

/// H1: the owner's press is theirs only where it pressed a link or a submit control and the tab
/// landed on the origin the extension says it goes to: a link redirected elsewhere is asked.
#[tokio::test]
async fn an_owners_press_is_theirs_only_where_it_landed_where_it_goes() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 61, 12, "searchbox", "Search");
    // A press of a link that landed on another origin than its href's (a redirect): Kai's.
    element(&ff, 60, 12, "link", "Partner shop");
    with(
        &ff,
        60,
        "answer",
        left_for(ELSEWHERE, link_to("https://partner.example")),
    );
    let (err, text) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{text}");
    goes(&ff, 12, ELSEWHERE);
    assert!(fill_asked(&h, &Caller::Own).await.is_some());
    // A press of a submit control that landed on its form's action's origin: the owner's.
    goes(&ff, 12, CART);
    with(
        &ff,
        60,
        "answer",
        left_for(
            "https://form.example/done",
            json!({"by": "submit", "origin": "https://form.example"}),
        ),
    );
    h.call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    goes(&ff, 12, "https://form.example/done");
    assert_eq!(fill_asked(&h, &Caller::Own).await, None);
    // The same press for lmgw's own run: Kai's navigation.
    goes(&ff, 12, CART);
    h.set(allowed(), switch(true));
    with(
        &ff,
        60,
        "answer",
        left_for(
            "https://form2.example/done",
            json!({"by": "submit", "origin": "https://form2.example"}),
        ),
    );
    h.call("element_press", json!({"ref": 60}), &Caller::Gateway)
        .await;
    goes(&ff, 12, "https://form2.example/done");
    assert!(fill_asked(&h, &Caller::Gateway).await.is_some());
}

/// H1: the tab tools of the owner's turn are the owner's only where the tab landed on the origin
/// they were meant for: `tab_go`'s URL (a redirect to another origin is asked), a search's engine
/// and the history entry `tab_back` went to, as the extension says them (`goes`).
#[tokio::test]
async fn a_redirect_after_the_owners_tab_go_is_asked() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 61, 12, "searchbox", "Search");
    let landed = |url: &str, goes: Value| json!({"tab": tab(12, 1, "Went", url, true), "loaded": true, "goes": goes});
    // Redirected from the shop's deals to another site.
    ff.model().twists.insert(
        "tabs.go@1",
        Twist::Answer(landed(
            ELSEWHERE,
            json!({"by": "url", "origin": "https://shop.example"}),
        )),
    );
    let (err, text) = h
        .call(
            "tab_go",
            json!({"url": "https://shop.example/deals"}),
            &Caller::Own,
        )
        .await;
    assert!(!err, "{text}");
    goes(&ff, 12, ELSEWHERE);
    let asked = fill_asked(&h, &Caller::Own).await;
    assert!(asked.is_some_and(|n| n.contains("another site")));
    // Where it was meant to go: the owner's.
    ff.model().twists.remove("tabs.go@1");
    h.call("tab_go", json!({"url": NEWS}), &Caller::Own).await;
    let _ = h.asked();
    assert_eq!(fill_asked(&h, &Caller::Own).await, None);
    // A search lands on the engine's origin the extension says: the owner's; one the engine sent
    // on to another site is asked.
    for (url, engine, owners) in [
        (
            "https://search.example/?q=lamp",
            "https://search.example",
            true,
        ),
        (ELSEWHERE, "https://search.example", false),
    ] {
        ff.model().twists.insert(
            "tabs.go@1",
            Twist::Answer(landed(url, json!({"by": "search", "origin": engine}))),
        );
        h.call("tab_go", json!({"url": "lamp shades"}), &Caller::Own)
            .await;
        let _ = h.asked();
        goes(&ff, 12, url);
        assert_eq!(
            fill_asked(&h, &Caller::Own).await.is_none(),
            owners,
            "{url}"
        );
    }
    ff.model().twists.remove("tabs.go@1");
    // Back: the history entry's origin, as the extension says it.
    for (entry, url, owners) in [
        ("https://elsewhere.example", ELSEWHERE, true),
        ("https://news.example", "https://other.example/", false),
    ] {
        goes(&ff, 12, NEWS);
        ff.model().twists.insert(
            "tabs.back@1",
            Twist::Answer(landed(url, json!({"by": "history", "origin": entry}))),
        );
        h.call("tab_back", json!({}), &Caller::Own).await;
        let _ = h.asked();
        goes(&ff, 12, url);
        assert_eq!(
            fill_asked(&h, &Caller::Own).await.is_none(),
            owners,
            "{entry}"
        );
    }
}

/// H1: a window the page opened under the owner's trusted press is Kai's opened tab, asked about;
/// the window the pressed link opened, on its `href`'s origin, is the owner's.
#[tokio::test]
async fn a_window_the_page_opened_under_the_owners_press_is_asked() {
    for (opened, owners) in [
        ("https://popup.example/", false),
        ("https://shop.example/prices", true),
    ] {
        let h = Host::start().await;
        let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
        h.set(PolicyConfig::default(), switch(true));
        element(&ff, 60, 12, "link", "Price list");
        with(&ff, 60, "needs_trusted", json!("new_window"));
        with(
            &ff,
            60,
            "answer",
            json!({"goes": {"by": "link", "origin": "https://shop.example"}}),
        );
        element(&ff, 61, 20, "searchbox", "Search");
        let model = ff.model.clone();
        let url = opened.to_owned();
        *h.desk.on_key.lock().unwrap() = Some(Box::new(move || {
            let mut m = model.lock().unwrap();
            for t in &mut m.tabs {
                t["user"] = json!(false);
                if t["window"] == 1 {
                    t["active"] = json!(false);
                }
            }
            m.tabs.push(tab(20, 1, "Opened", &url, true));
        }));
        let (err, text) = h
            .call("element_press", json!({"ref": 60}), &Caller::Own)
            .await;
        assert!(!err, "{text}");
        assert!(h.asked().is_empty(), "the press itself: {text}");
        let asked = fill_asked(&h, &Caller::Own).await;
        assert_eq!(asked.is_none(), owners, "{opened}: {asked:?}");
        if !owners {
            assert!(asked.unwrap().contains("Kai opened this tab"));
        }
    }
}

/// L1: every acting operation is Kai's navigation from before it is sent: a navigation that
/// commits after its answer (the commit guard off: no window), a call that failed or was cancelled,
/// and the trusted press's real key all leave the tab's site where it was.
#[tokio::test]
async fn a_late_commit_a_failure_a_cancel_and_the_real_key_are_kais_navigation() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            commit_guard: false,
            ..switch(true)
        },
    );
    element(&ff, 60, 12, "button", "Show more");
    element(&ff, 61, 12, "searchbox", "Search");
    // The press answers on its page; the page commits its navigation afterwards.
    let (err, text) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{text}");
    goes(&ff, 12, ELSEWHERE);
    assert!(
        fill_asked(&h, &Caller::Own).await.is_some(),
        "a late commit"
    );
    // A press that failed; the page went on.
    goes(&ff, 12, CART);
    ff.model().twists.insert(
        "element.press@1",
        Twist::Fail("content_timeout", "no answer"),
    );
    let (err, _) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(err);
    ff.model().twists.remove("element.press@1");
    goes(&ff, 12, ELSEWHERE);
    assert!(
        fill_asked(&h, &Caller::Own).await.is_some(),
        "a failed call"
    );
    // A press cancelled while it waits (lmgw's deadline); the page went on.
    goes(&ff, 12, CART);
    ff.model().twists.insert("element.press@1", Twist::Hang);
    let cancel = CancellationToken::new();
    let later = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        later.cancel();
    });
    let (err, _) = h
        .call_until("element_press", json!({"ref": 60}), &Caller::Own, cancel)
        .await;
    assert!(err);
    ff.model().twists.remove("element.press@1");
    goes(&ff, 12, ELSEWHERE);
    assert!(
        fill_asked(&h, &Caller::Own).await.is_some(),
        "a cancelled call"
    );
    // The real key of the trusted press took the same tab elsewhere; the press goes nowhere the
    // extension knows of.
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 60, 12, "button", "Play");
    element(&ff, 61, 12, "searchbox", "Search");
    with(&ff, 60, "needs_trusted", json!("media"));
    let model = ff.model.clone();
    *h.desk.on_key.lock().unwrap() = Some(Box::new(move || {
        let mut m = model.lock().unwrap();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 12).unwrap();
        t["url"] = json!(ELSEWHERE);
    }));
    let (err, text) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{text}");
    let _ = h.asked();
    assert!(fill_asked(&h, &Caller::Own).await.is_some(), "the real key");
}

/// L2: the owner's close that failed hands the tab Firefox shows next to nobody.
#[tokio::test]
async fn the_owners_close_that_failed_hands_the_next_tab_to_nobody() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 61, 3, "searchbox", "Search");
    ff.model()
        .twists
        .insert("tabs.close@1", Twist::Fail("internal", "no"));
    let (err, _) = h
        .call("tab_close", json!({"tab": "12"}), &Caller::Own)
        .await;
    assert!(err);
    ff.model().twists.remove("tabs.close@1");
    let _ = h.asked();
    // Another caller's switch to the mail is Kai's, and nothing of the close makes it the owner's.
    h.call("tab_switch", json!({"tab": "3"}), &Caller::Gateway)
        .await;
    let _ = h.asked();
    let asked = fill_asked(&h, &Caller::Own).await;
    assert!(
        asked
            .as_deref()
            .is_some_and(|n| n.contains("Kai brought this tab")),
        "{asked:?}"
    );
}

/// M3: while Kai's voice runs its continuation for jobs' results, the calls of Kai's own device
/// answer tool output: none of their moves is the owner's.
#[tokio::test]
async fn no_move_is_the_owners_while_kais_continuation_runs() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch(true));
    element(&ff, 61, 12, "searchbox", "Search");
    h.jobs.set_continuing(true);
    h.call("tab_go", json!({"url": NEWS}), &Caller::Own).await;
    let _ = h.asked();
    goes(&ff, 12, NEWS);
    assert!(fill_asked(&h, &Caller::Own).await.is_some());
    h.jobs.set_continuing(false);
    h.call("tab_go", json!({"url": ELSEWHERE}), &Caller::Own)
        .await;
    let _ = h.asked();
    assert_eq!(fill_asked(&h, &Caller::Own).await, None);
}

/// The tab tools' moves (a navigation, a new tab, a switch, back) are the owner's only for the
/// owner's own turn with the switch on: for lmgw's own runs, a named device, the owner's command
/// line and with the switch off, acting where they went is asked.
#[tokio::test]
async fn the_tab_tools_are_kais_moves_for_every_other_caller_and_with_the_switch_off() {
    let back = json!({"by": "history", "origin": "https://news.example"});
    for (caller, on) in [
        (Caller::Own, true),
        (Caller::Gateway, true),
        (stranger(), true),
        (Caller::Owner, true),
        (Caller::Own, false),
    ] {
        let owners = caller == Caller::Own && on;
        let h = Host::start().await;
        let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
        h.set(allowed(), switch(on));
        element(&ff, 61, 12, "searchbox", "Search");
        // tab_go.
        let (err, text) = h.call("tab_go", json!({"url": NEWS}), &caller).await;
        assert!(!err, "{caller:?}: {text}");
        let _ = h.asked();
        assert_eq!(
            fill_asked(&h, &caller).await.is_none(),
            owners,
            "{caller:?} {on}: tab_go"
        );
        // tab_back to an entry of the news, after the tab went elsewhere.
        goes(&ff, 12, ELSEWHERE);
        let _ = fill_asked(&h, &caller).await;
        ff.model().twists.insert(
            "tabs.back@1",
            Twist::Answer(
                json!({"tab": tab(12, 1, "News", NEWS, true), "loaded": true,
                                 "goes": back}),
            ),
        );
        h.call("tab_back", json!({}), &caller).await;
        let _ = h.asked();
        goes(&ff, 12, NEWS);
        assert_eq!(
            fill_asked(&h, &caller).await.is_none(),
            owners,
            "{caller:?} {on}: tab_back"
        );
        // tab_switch to the mail, which the user never acted in.
        element(&ff, 62, 3, "searchbox", "Search mail");
        h.call("tab_switch", json!({"tab": "3"}), &caller).await;
        let _ = h.asked();
        let (err, text) = h
            .call("element_fill", json!({"ref": 62, "text": "x"}), &caller)
            .await;
        assert!(!err, "{text}");
        assert_eq!(h.asked().is_empty(), owners, "{caller:?} {on}: tab_switch");
        // tab_open.
        let (err, text) = h.call("tab_open", json!({"url": NEWS}), &caller).await;
        assert!(!err, "{text}");
        let _ = h.asked();
        let id = ff
            .model()
            .tabs
            .iter()
            .filter_map(|t| t["id"].as_i64())
            .max()
            .unwrap();
        element(&ff, 63, id, "searchbox", "Search news");
        h.call("element_fill", json!({"ref": 63, "text": "x"}), &caller)
            .await;
        let asked = h.asked();
        assert_eq!(asked.is_empty(), owners, "{caller:?} {on}: tab_open");
        if !owners {
            assert!(
                asked[0].notes[0].contains("Kai opened this tab"),
                "{asked:?}"
            );
        }
    }
}

/// L2: the owner's marks left from while the switch was on count for nothing once it is off, and
/// again once it is on.
#[tokio::test]
async fn the_owners_marks_left_after_the_switch_went_off_count_for_nothing() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(allowed(), switch(true));
    element(&ff, 61, 12, "searchbox", "Search");
    h.call("tab_go", json!({"url": NEWS}), &Caller::Own).await;
    let _ = h.asked();
    assert_eq!(fill_asked(&h, &Caller::Own).await, None);
    h.set(allowed(), switch(false));
    assert!(
        fill_asked(&h, &Caller::Own)
            .await
            .is_some_and(|n| n.contains("another site")),
        "as without the switch: Kai took the tab there"
    );
    h.set(allowed(), switch(true));
    assert_eq!(fill_asked(&h, &Caller::Own).await, None);
}

/// L4: the trusted press's own question (its real key) is spoken for a call of Kai's own, Kai's
/// words first and the page's name for the element last; another caller's shows the dialog alone.
#[tokio::test]
async fn the_real_keys_question_is_spoken_for_kais_own_call() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    // The switch off: key_press's state (confirmed by default) decides the real key.
    h.set(allowed(), switch(false));
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    let (err, text) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{text}");
    let asked = h.asked();
    let key = asked
        .iter()
        .find(|a| a.tool.starts_with("key_press"))
        .unwrap_or_else(|| panic!("{asked:?}"));
    assert_eq!(
        key.spoken.as_deref(),
        Some(
            "May Kai press the link? It takes a real key, so Kai presses Enter through the \
             desktop. Yes or no? The page calls the link: Price list"
        )
    );
    h.call("element_press", json!({"ref": 60}), &Caller::Gateway)
        .await;
    let asked = h.asked();
    let key = asked
        .iter()
        .find(|a| a.tool.starts_with("key_press"))
        .unwrap_or_else(|| panic!("{asked:?}"));
    assert_eq!(key.spoken, None, "lmgw's own run: the dialog alone");
}
