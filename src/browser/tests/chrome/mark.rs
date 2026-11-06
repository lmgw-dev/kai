//! §6.3 through accessibility (kai-firefox WP12 and its review round): the tab's own window
//! looked up, the mark, the search in that window's page only, the mark taken away, the element
//! checked against the extension's facts, the gate on the element's document, the window looked
//! at again, the press and the gate read, inside the spans, without a window or a key; an element
//! not on the bus takes the key's path; a copied mark, an element that is not the extension's
//! and a tab switch right before the press end it.

use kai_desktop::caps::accessibility::{Element, Found, Marked};
use serde_json::json;

use super::{FakeA11y, tree, window, window_with, with_access};
use crate::browser::tests::fake::{Firefox, Host, all_ops, shop};

const HEAD: &str = "\"Checkout\" https://shop.example/cart (tab 12)";
const CART: &str = "https://shop.example/cart";

fn found(role: &str, name: &str, documents: &[&str]) -> Found {
    Found {
        role: role.into(),
        name: name.into(),
        at: (":1.9".into(), "/price".into()),
        documents: documents.iter().map(|d| (*d).to_owned()).collect(),
    }
}

/// The host, a Firefox whose link 60 takes a real press, and the fake accessibility with the
/// user's window; the steps so far dropped.
async fn rig() -> (Host, Firefox, std::sync::Arc<FakeA11y>) {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    ff.model().elements.insert(
        60,
        json!({"tab": 12, "role": "link", "name": "Price list", "needs_trusted": "new_window"}),
    );
    let a = with_access(&h, window(CART, ""), None).await;
    let _ = h.steps();
    (h, ff, a)
}

/// No window to the front, no focus check, no key; the asked question says how.
#[tokio::test]
async fn the_trusted_press_goes_through_accessibility_and_falls_back_to_the_key() {
    let (h, ff, a) = rig().await;
    *a.marked.lock().unwrap() = Marked::One(found("link", "Price list", &[CART]));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: pressed 60 link \"Price list\" with a real click through accessibility (a \
             link that opens a new window, which Firefox opens only on a real key or click)"
        )
    );
    assert_eq!(
        h.steps(),
        [
            "element.describe@1",
            "element.press@1",
            "turn",
            "input.begin@1",
            "element.mark@1",
            "a11y find kai-mark=m60",
            "element.unmark@1",
            "gate.open@1",
            "a11y press Price list",
            "gate.result@1",
            "input.end@1"
        ],
        "no window to the front, no focus check, no key"
    );
    assert_eq!(
        *a.searched.lock().unwrap(),
        ["/page"],
        "the tab's page only"
    );
    let asked = h.asked();
    assert_eq!(
        asked[1].tool,
        "key_press (a real press in Firefox, for element_press)"
    );
    assert_eq!(
        asked[1].lines[2],
        "how: a click through accessibility, else Enter"
    );
    let open = ff
        .acted()
        .into_iter()
        .find(|(o, _)| o == "gate.open@1")
        .unwrap()
        .1;
    assert_eq!(open["ref"], 60);
    assert!(open["guard"].is_object());
    // A mark in another page than the element's: not pressed.
    *a.marked.lock().unwrap() =
        Marked::One(found("link", "Price list", &["https://bank.example/"]));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("in another page than the element"),
        "{text}"
    );
    // Not on the bus: the key's path.
    *a.marked.lock().unwrap() = Marked::Nothing;
    let _ = h.steps();
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    assert!(text.contains("with Enter through the desktop"), "{text}");
    assert_eq!(
        h.steps(),
        [
            "element.describe@1",
            "element.press@1",
            "turn",
            "input.begin@1",
            "element.mark@1",
            "a11y find kai-mark=m60",
            "element.unmark@1",
            "input.end@1",
            "turn",
            "input.begin@1",
            "activate Checkout",
            "focus.check@1",
            "key Enter",
            "gate.result@1",
            "input.end@1"
        ]
    );
}

/// The steps after the search when the press ended there: nothing pressed, no key.
fn ended(steps: &[String]) {
    assert!(
        !steps.iter().any(|s| s.starts_with("a11y press")
            || s.starts_with("key ")
            || s.starts_with("activate")),
        "{steps:?}"
    );
}

/// M4: the mark on two elements is the page copying it: refused as tampering, no key.
#[tokio::test]
async fn a_duplicated_mark_is_tampering_and_presses_nothing() {
    let (h, _ff, a) = rig().await;
    *a.marked.lock().unwrap() = Marked::Several(2);
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("2 elements of the tab's page carry the mark")
            && text.contains("tampering")
            && text.contains("neither through accessibility nor with a key"),
        "{text}"
    );
    ended(&h.steps());
}

/// M4: an element found in a page Firefox gives no address (a `blob:` frame's copy), and one of
/// another role than the extension's.
#[tokio::test]
async fn a_mark_in_an_empty_address_or_of_another_role_is_refused() {
    let (h, ff, a) = rig().await;
    ff.model().elements.insert(
        60,
        json!({"tab": 12, "role": "link", "name": "Price list", "needs_trusted": "new_window",
               "frames": [CART, "blob:https://shop.example/0b1c"]}),
    );
    *a.marked.lock().unwrap() = Marked::One(found("link", "Price list", &[CART, ""]));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(err && text.contains("gives no address"), "{text}");
    ended(&h.steps());
    ff.model().elements.insert(
        60,
        json!({"tab": 12, "role": "link", "name": "Price list", "needs_trusted": "new_window"}),
    );
    *a.marked.lock().unwrap() = Marked::One(found("button", "Price list", &[CART]));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("is a button on the accessibility bus, the element a link"),
        "{text}"
    );
    ended(&h.steps());
}

/// `e` with every place `from` renamed `to`.
fn repath(mut e: Element, from: &str, to: &str) -> Element {
    if let Some(at) = &mut e.at
        && at.1 == from
    {
        at.1 = to.into();
    }
    e.children = e
        .children
        .into_iter()
        .map(|c| repath(c, from, to))
        .collect();
    e
}

/// M4: only the tab's own window is searched; a copy of the mark in another window (the same
/// Firefox's second window, showing another tab) is never looked at.
#[tokio::test]
async fn only_the_tabs_window_is_searched() {
    let (h, _ff, a) = rig().await;
    let other = repath(
        window_with("https://shop.example/lamps", "Lamps", "", vec![]),
        "/page",
        "/lamps",
    );
    *a.app.lock().unwrap() = tree(vec![other, window_with(CART, "Checkout", "", vec![])]);
    *a.marked.lock().unwrap() = Marked::One(found("link", "Price list", &[CART]));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    assert_eq!(*a.searched.lock().unwrap(), ["/page"]);
}

/// The moment after a tab switch: the window was the tab's at the search, and right before the
/// press another tab is selected in it (Firefox still shows the page before): nothing pressed.
#[tokio::test]
async fn a_tab_switch_before_the_press_presses_nothing() {
    let (h, _ff, a) = rig().await;
    *a.marked.lock().unwrap() = Marked::One(found("link", "Price list", &[CART]));
    let switched = tree(vec![window_with(CART, "Mail – Inbox", "", vec![])]);
    a.next.lock().unwrap().extend([window(CART, ""), switched]);
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("its selected tab is not tab 12"),
        "{text}"
    );
    ended(&h.steps());
}
