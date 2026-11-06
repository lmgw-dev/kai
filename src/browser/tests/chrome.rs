//! Kai for Firefox's WP12 through the host's dispatch against the fake Kai for Firefox and a fake
//! of the desktop's accessibility: `firefox_ui` and `firefox_press` listed only with it, what
//! they read (an allow-list) and leave out, the page's dialog said as the page's text and judged
//! by the origin its title names, a prompt for a site with its words, the tab in front of its own
//! window only, the yes the owner gives, the never-press list, the last look before the press,
//! the gate around a press; `page_text` on one of Firefox's own pages; and the trusted press
//! through accessibility (`mark`).

mod mark;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use kai_desktop::caps::accessibility::{
    Accessibility, Element, Found, How, Marked, Pressed, Query, Target, Tree,
};
use kai_desktop::caps::{Failure, Fut};
use serde_json::json;

use super::fake::{Host, Model, Steps, Twist, all_ops, shop, tab};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::host::settings::{PolicyConfig, ToolState};

const HEAD: &str = "\"Checkout\" https://shop.example/cart (tab 12)";

/// The desktop's accessibility as the tests give it: Firefox's windows (the next trees first,
/// then the fixed one), the dialogs' trees, what a mark search finds; the steps it takes go to the
/// host's steps.
pub struct FakeA11y {
    steps: Steps,
    pub app: Mutex<Tree>,
    /// Trees the next reads of the app give, in order, each kept as `app` once read.
    pub next: Mutex<VecDeque<Tree>>,
    pub subtrees: Mutex<HashMap<String, Element>>,
    pub marked: Mutex<Marked>,
    /// The pages a mark was searched in, by their path.
    pub searched: Mutex<Vec<String>>,
}

fn find<'a>(e: &'a Element, path: &str) -> Option<&'a Element> {
    if e.at.as_ref().map(|a| a.1.as_str()) == Some(path) {
        return Some(e);
    }
    e.children.iter().find_map(|c| find(c, path))
}

impl FakeA11y {
    fn element(&self, at: &(String, String)) -> Option<Element> {
        if let Some(e) = find(&self.app.lock().unwrap().root, &at.1) {
            return Some(e.clone());
        }
        self.subtrees
            .lock()
            .unwrap()
            .values()
            .find_map(|r| find(r, &at.1).cloned())
    }
}

impl Accessibility for FakeA11y {
    fn tree<'a>(&'a self, _: &'a Target) -> Fut<'a, Tree> {
        Box::pin(async { Err(Failure::Failed("not in these tests".into())) })
    }
    fn selection<'a>(&'a self, _: &'a Target) -> Fut<'a, Tree> {
        Box::pin(async { Err(Failure::Failed("not in these tests".into())) })
    }
    fn find<'a>(&'a self, _: &'a Target, _: &'a Query) -> Fut<'a, Found> {
        Box::pin(async { Err(Failure::Failed("not in these tests".into())) })
    }
    fn press<'a>(&'a self, found: &'a Found) -> Fut<'a, Pressed> {
        Box::pin(async move {
            self.steps
                .lock()
                .unwrap()
                .push(format!("a11y press {}", found.name));
            Ok(Pressed {
                role: found.role.clone(),
                name: found.name.clone(),
                action: "press".into(),
            })
        })
    }
    fn app_tree<'a>(&'a self, _: u32, how: How) -> Fut<'a, Tree> {
        Box::pin(async move {
            assert!(how.details && !how.into_documents, "{how:?}");
            if let Some(t) = self.next.lock().unwrap().pop_front() {
                *self.app.lock().unwrap() = t;
            }
            Ok(self.app.lock().unwrap().clone())
        })
    }
    fn subtree<'a>(&'a self, at: &'a (String, String), _: How) -> Fut<'a, Tree> {
        Box::pin(async move {
            let root = self
                .subtrees
                .lock()
                .unwrap()
                .get(&at.1)
                .cloned()
                .ok_or_else(|| Failure::Refused("no such element".into()))?;
            Ok(Tree {
                root,
                unanswered: 0,
            })
        })
    }
    fn found_at<'a>(&'a self, at: &'a (String, String)) -> Fut<'a, Found> {
        Box::pin(async move {
            let e = self
                .element(at)
                .ok_or_else(|| Failure::Refused("the element is no longer there".into()))?;
            if !e.showing {
                return Err(Failure::Refused("not shown".into()));
            }
            Ok(Found {
                role: e.role,
                name: e.name,
                at: at.clone(),
                documents: Vec::new(),
            })
        })
    }
    fn find_marked<'a>(
        &'a self,
        page: &'a (String, String),
        name: &'a str,
        value: &'a str,
    ) -> Fut<'a, Marked> {
        Box::pin(async move {
            self.steps
                .lock()
                .unwrap()
                .push(format!("a11y find {name}={value}"));
            self.searched.lock().unwrap().push(page.1.clone());
            Ok(self.marked.lock().unwrap().clone())
        })
    }
}

pub fn el(role: &str, name: &str, id: &str, children: Vec<Element>) -> Element {
    Element {
        role: role.into(),
        name: name.into(),
        id: id.into(),
        showing: true,
        children,
        ..Default::default()
    }
}

/// `e` with a place, pressable where it is a button, a check box or a link.
pub fn at(e: Element, path: &str) -> Element {
    Element {
        at: Some((":1.9".into(), path.into())),
        pressable: matches!(e.role.as_str(), "button" | "check box" | "link"),
        ..e
    }
}

fn doc(role: &str, name: &str, url: &str, path: &str) -> Element {
    Element {
        document: Some(url.into()),
        ..at(el(role, name, "", Vec::new()), path)
    }
}

/// A prompt for a site saying `says`, as Firefox 159 shows one in `notification-popup`.
pub fn prompt(says: &str) -> Element {
    let mut d = el("label", says, "", vec![]);
    d.class = "popup-notification-description".into();
    el(
        "alert",
        "",
        "notification-popup",
        vec![
            d,
            at(
                el("check box", "Remember this decision", "", vec![]),
                "/remember",
            ),
            at(el("button", "Block", "main-button", vec![]), "/block"),
            at(el("button", "Allow", "main-button", vec![]), "/allow"),
        ],
    )
}

/// The page tabs of the tab strip: `selected` is in front.
fn strip(selected: &str) -> Element {
    let page_tab = |name: &str| {
        let mut t = el("page tab", name, "", vec![]);
        if name == selected {
            t.states = vec!["selected"];
        }
        t
    };
    let mut names = vec!["Mail – Inbox", "Checkout"];
    if !names.contains(&selected) {
        names.push(selected);
    }
    el(
        "page tab list",
        "",
        "tabbrowser-tabs",
        names.into_iter().map(page_tab).collect(),
    )
}

/// Firefox's window showing `page_url` in front with the tab `selected` selected, a dialog titled
/// `dialog_title` (none when empty), `popups` (prompts, panels), the tool bars and the tab strip.
pub fn window_with(
    page_url: &str,
    selected: &str,
    dialog_title: &str,
    popups: Vec<Element>,
) -> Element {
    let mut urlbar = el(
        "combo box",
        "Search or enter address",
        "urlbar-input",
        vec![],
    );
    urlbar.text = page_url.into();
    let mut panel = vec![el(
        "internal frame",
        "",
        "",
        vec![doc("document web", "Checkout", page_url, "/page")],
    )];
    if !dialog_title.is_empty() {
        panel.push(el(
            "internal frame",
            "",
            "",
            vec![doc(
                "dialog",
                dialog_title,
                "chrome://global/content/commonDialog.xhtml",
                "/dialog",
            )],
        ));
    }
    let mut kids = vec![
        el(
            "tool bar",
            "Browser tabs",
            "TabsToolbar",
            vec![strip(selected)],
        ),
        el(
            "tool bar",
            "Navigation",
            "nav-bar",
            vec![
                at(el("button", "Back", "back-button", vec![]), "/back"),
                urlbar,
            ],
        ),
    ];
    kids.extend(popups);
    kids.push(el(
        "panel",
        "",
        "tabbrowser-tabpanels",
        vec![el("scroll pane", "", "", panel)],
    ));
    el("frame", "Checkout — Mozilla Firefox", "main-window", kids)
}

/// Firefox's window in front showing `page_url` (the user's tab "Checkout"), with a dialog titled
/// `dialog_title` (none when empty), the permission prompt, the tool bars and the tab strip.
pub fn window(page_url: &str, dialog_title: &str) -> Tree {
    tree(vec![window_with(
        page_url,
        "Checkout",
        dialog_title,
        vec![prompt("Allow shop.example to know your location?")],
    )])
}

pub fn tree(windows: Vec<Element>) -> Tree {
    Tree {
        root: el("application", "Firefox", "", windows),
        unanswered: 0,
    }
}

/// A `confirm()`'s dialog, with a text field where `prompt`.
fn dialog(title: &str, says: &str, prompt: bool) -> Element {
    let mut kids = vec![el("label", says, "", vec![])];
    if prompt {
        let mut entry = at(el("entry", says, "", vec![]), "/entry");
        entry.text = "Ada".into();
        kids.push(entry);
    }
    kids.push(at(el("button", "Cancel", "", vec![]), "/cancel"));
    kids.push(at(el("button", "OK", "", vec![]), "/ok"));
    Element {
        document: Some("chrome://global/content/commonDialog.xhtml".into()),
        ..at(el("dialog", title, "commonDialogWindow", kids), "/dialog")
    }
}

/// The host with the fake accessibility on its desk.
pub async fn with_access(h: &Host, tree: Tree, dialog: Option<Element>) -> Arc<FakeA11y> {
    let a = Arc::new(FakeA11y {
        steps: h.steps.clone(),
        app: Mutex::new(tree),
        next: Mutex::default(),
        subtrees: Mutex::new(HashMap::new()),
        marked: Mutex::new(Marked::Nothing),
        searched: Mutex::default(),
    });
    if let Some(d) = dialog {
        a.subtrees.lock().unwrap().insert("/dialog".into(), d);
    }
    *h.desk.access.lock().unwrap() = Some(a.clone());
    a
}

const TITLE: &str = "The page at https://shop.example says:";

/// What the answer says was left out.
const LEFT: &str = "element(s) left out: the tab strip (tab_list), the user's history, \
                    bookmarks, synced and recently closed tabs, the address bar's suggestions, \
                    the password manager, saved cards and addresses, add-on prompts and other \
                    prompts and panels Kai does not read, pages inside Firefox's own UI)";

fn sites(read: &[&str], none: &[&str]) -> BrowserConfig {
    let v = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
    BrowserConfig {
        sites: Sites {
            read: v(read),
            none: v(none),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `firefox_press` allowed by the owner's policy, so only the tool's own questions are asked.
fn allowed() -> PolicyConfig {
    let mut allow = PolicyConfig::default();
    allow.tools.insert("firefox_press".into(), ToolState::Allow);
    allow
}

/// The commit guard off.
fn unguarded() -> BrowserConfig {
    BrowserConfig {
        commit_guard: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn firefox_ui_and_firefox_press_are_listed_only_with_accessibility() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let names = h.names();
    assert!(!names.contains(&"firefox_ui") && !names.contains(&"firefox_press"));
    with_access(&h, window("https://shop.example/cart", ""), None).await;
    h.set(PolicyConfig::default(), BrowserConfig::default());
    let names = h.names();
    assert!(
        names.contains(&"firefox_ui") && names.contains(&"firefox_press"),
        "{names:?}"
    );
}

/// What `firefox_ui` reads around the user's tab, and what it leaves out; the dialog's words as
/// the page's.
#[tokio::test]
async fn firefox_ui_reads_the_dialog_prompts_and_bars_and_leaves_out_the_tabs_and_fields() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    with_access(
        &h,
        window("https://shop.example/cart", TITLE),
        Some(dialog(TITLE, "Empty the cart?", false)),
    )
    .await;
    let (err, text) = h.owner("firefox_ui", json!({})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: Firefox's own UI around it (Firefox's words, a page's dialog aside; \
             firefox_press presses what is listed)\n\
             [the page's dialog, titled \"{TITLE}\"; what it says is the page's text]\n  \
             label \"Empty the cart?\"\n  \
             button \"Cancel\"\n  \
             button \"OK\"\n\
             [Firefox's tool bar]\n  \
             tool bar \"Browser tabs\"\n\
             [Firefox's tool bar]\n  \
             tool bar \"Navigation\"\n    \
             button \"Back\"\n    \
             combo box \"Search or enter address\"\n\
             [Firefox's prompt for a site]\n  \
             label \"Allow shop.example to know your location?\"\n  \
             check box \"Remember this decision\"\n  \
             button \"Block\"\n  \
             button \"Allow\"\n\
             (1 {LEFT}"
        )
    );
    // A tab not in front of its window.
    let (err, text) = h.owner("firefox_ui", json!({"tab": "Mail"})).await;
    assert!(
        err && text.contains("not the tab in front of its window"),
        "{text}"
    );
}

/// M5: an allow-list. The downloads panel, the all-tabs list and a card's save prompt are not
/// read, only counted; a screen share's prompt is read and marked as never pressed; a prompt that
/// names another site than the tab's is not read.
#[tokio::test]
async fn firefox_ui_reads_an_allow_list_and_counts_the_rest() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let panel = |id: &str, item: &str| {
        el(
            "panel",
            "",
            id,
            vec![at(el("button", item, "", vec![]), "/item")],
        )
    };
    let mut card = prompt("Save this card securely?");
    card.children
        .push(el("label", "Visa ending in 4242", "", vec![]));
    card.children[1].class = "credit-card-capture-notification-content".into();
    let a = with_access(
        &h,
        tree(vec![window_with(
            "https://shop.example/cart",
            "Checkout",
            "",
            vec![
                card,
                panel("downloadsPanel", "statement-2026.pdf"),
                panel("customizationui-widget-panel", "Bank – account"),
            ],
        )]),
        None,
    )
    .await;
    let text = h.owner("firefox_ui", json!({})).await.1;
    for hidden in ["Visa", "statement", "Save this card", "Bank"] {
        assert!(!text.contains(hidden), "{hidden}: {text}");
    }
    assert!(text.ends_with(&format!("(4 {LEFT}")), "{text}");
    // A screen share: read, never pressed.
    let mut screen = prompt("Allow shop.example to see your screen?");
    screen.children.push(el(
        "combo box",
        "Window or screen to share",
        "webRTC-selectWindow-menulist",
        vec![],
    ));
    *a.app.lock().unwrap() = tree(vec![window_with(
        "https://shop.example/cart",
        "Checkout",
        "",
        vec![screen],
    )]);
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(
        text.contains(
            "[Firefox's prompt for a site; Firefox's prompt for a site to share the screen or a \
             window, which Kai never presses]\n  label \"Allow shop.example to see your screen?\""
        ),
        "{text}"
    );
    // A prompt that names another site than the tab's.
    *a.app.lock().unwrap() = tree(vec![window_with(
        "https://shop.example/cart",
        "Checkout",
        "",
        vec![prompt("Allow evil.example to know your location?")],
    )]);
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(!text.contains("evil.example"), "{text}");
}

/// A dialog's title names its origin: a site Kai does not reach keeps it unread; one that names
/// none gets the strictest rule; a prompt by the sites its words name.
#[tokio::test]
async fn a_dialog_is_judged_by_the_origin_its_title_names() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let bank = "The page at https://bank.example says:";
    with_access(
        &h,
        window("https://shop.example/cart", bank),
        Some(dialog(bank, "Your balance is 1200", false)),
    )
    .await;
    h.set(PolicyConfig::default(), sites(&[], &["bank.example"]));
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(!text.contains("balance"), "{text}");
    assert!(
        text.contains("(1 dialog(s) or prompt(s) of a site Kai does not reach"),
        "{text}"
    );
    // Pressing in it is refused before anyone is asked.
    let (err, text) = h.owner("firefox_press", json!({"control": "OK"})).await;
    assert!(err && text.contains("Kai's site rules"), "{text}");
    // A title that names no origin, while some site is none: the strictest rule.
    let untold = "This page is asking you to confirm that you want to leave";
    *h.desk.access.lock().unwrap() = None;
    with_access(
        &h,
        window("https://shop.example/cart", untold),
        Some(dialog(untold, "Leave?", false)),
    )
    .await;
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(!text.contains("Leave?"), "{text}");
    // Without a none site, it is read.
    h.set(PolicyConfig::default(), BrowserConfig::default());
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(text.contains("label \"Leave?\""), "{text}");
}

/// The press: found before the confirmation, which names the dialog's words; the gate on the tab,
/// the spans, the press; the commit guard's question where the call ran unasked.
#[tokio::test]
async fn firefox_press_answers_a_dialog_through_the_gate_and_the_spans() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    with_access(
        &h,
        window("https://shop.example/cart", TITLE),
        Some(dialog(TITLE, "Your name?", true)),
    )
    .await;
    let _ = h.steps();
    let (err, text) = h
        .owner("firefox_press", json!({"control": "button \"OK\""}))
        .await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!("{HEAD}: pressed button \"OK\" in the page's dialog")
    );
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "the tool's own question only: {asked:?}");
    for line in [
        "control: button \"OK\" in the page's dialog",
        "the dialog's title: \"The page at https://shop.example says:\"",
        "the page's dialog says: \"Your name?\"",
    ] {
        assert!(
            asked[0].lines.iter().any(|l| l == line),
            "{line}: {:?}",
            asked[0].lines
        );
    }
    assert_eq!(
        h.steps(),
        [
            "turn",
            "input.begin@1",
            "gate.open@1",
            "a11y press OK",
            "gate.result@1",
            "input.end@1"
        ]
    );
    // Allowed by the owner's policy: the commit guard asks about the page's choice.
    h.set(allowed(), BrowserConfig::default());
    let (err, text) = h.owner("firefox_press", json!({"control": "Cancel"})).await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, "firefox_press (a choice the page asks for)");
    // A tool bar's button is no choice.
    assert!(!h.owner("firefox_press", json!({"control": "Back"})).await.0);
    assert_eq!(h.asked().len(), 0, "a tool bar's button is no choice");
    // With the commit guard off, the page's choice is not asked about.
    h.set(allowed(), unguarded());
    assert!(
        !h.owner("firefox_press", json!({"control": "Cancel"}))
            .await
            .0
    );
    assert_eq!(h.asked().len(), 0);
    // A read site: refused.
    h.set(allowed(), sites(&["shop.example"], &[]));
    let (err, text) = h.owner("firefox_press", json!({"control": "Back"})).await;
    assert!(err && text.contains("may only read"), "{text}");
}

/// H2 and H3: a prompt for a site is asked with its words whatever the commit guard's and the
/// tool's state; declined, nothing is pressed; one naming a read site is read, not pressed.
#[tokio::test]
async fn a_prompt_for_a_site_is_always_asked_with_its_words() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    with_access(&h, window("https://shop.example/cart", ""), None).await;
    let words = [
        "control: button \"Allow\" in Firefox's prompt for a site",
        "Firefox's prompt for a site says: \"Allow shop.example to know your location?\"",
        "Firefox's prompt for a site says: check box \"Remember this decision\" (not checked)",
        "the sites it names: shop.example",
    ];
    // Confirmed by default: the call's own confirmation shows the words, and nothing else asks.
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    for w in words {
        assert!(
            asked[0].lines.iter().any(|l| l == w),
            "{w}: {:?}",
            asked[0].lines
        );
    }
    // Allowed by the policy and the commit guard off: asked all the same, with its words.
    h.set(allowed(), unguarded());
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].tool,
        "firefox_press (a site's permission prompt or a notification bar)"
    );
    assert_eq!(asked[0].lines[0], format!("tab: {HEAD}"));
    for w in words {
        assert!(
            asked[0].lines.iter().any(|l| l == w),
            "{w}: {:?}",
            asked[0].lines
        );
    }
    // Declined: nothing pressed.
    h.answers(&[Some(false)]);
    let _ = h.steps();
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(
        err && text
            .contains("the owner declined it (the question every prompt for a site and every"),
        "{text}"
    );
    assert!(!h.steps().iter().any(|s| s.starts_with("a11y press")));
    // A prompt naming a site where Kai may only read: read, not pressed.
    h.set(allowed(), sites(&["shop.example"], &[]));
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(err && text.contains("may only read"), "{text}");
}

/// A notification bar (Firefox's bars carry a site's pop-ups, DRM and other permissions) is asked
/// about with its words too, whatever the state.
#[tokio::test]
async fn a_notification_bar_is_always_asked_with_its_words() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let mut w = window_with("https://shop.example/cart", "Checkout", "", vec![]);
    let bar = el(
        "alert",
        "Firefox prevented this site from opening a pop-up window.",
        "",
        vec![at(el("button", "Allow pop-ups", "", vec![]), "/popups")],
    );
    let panels = w.children.last_mut().unwrap();
    panels.children[0].children.push(bar);
    with_access(&h, tree(vec![w]), None).await;
    h.set(allowed(), unguarded());
    let (err, text) = h
        .owner("firefox_press", json!({"control": "Allow pop-ups"}))
        .await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].lines.iter().any(|l| l
            == "Firefox's notification bar says: \"Firefox prevented this site from opening a \
                pop-up window.\""),
        "{:?}",
        asked[0].lines
    );
}

/// H2: the request swapped while the owner was asked (the same button, now for the camera and
/// the microphone): refused with what changed, nothing pressed.
#[tokio::test]
async fn a_request_swapped_during_the_question_is_not_pressed() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let a = with_access(&h, window("https://shop.example/cart", ""), None).await;
    h.set(allowed(), BrowserConfig::default());
    let swapped = a.clone();
    h.while_asked(move || {
        *swapped.app.lock().unwrap() = tree(vec![window_with(
            "https://shop.example/cart",
            "Checkout",
            "",
            vec![prompt(
                "Allow shop.example to use your camera and microphone?",
            )],
        )]);
    });
    let _ = h.steps();
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(err, "{text}");
    assert!(
        text.contains(
            "button \"Allow\" is no longer what the confirmation named: Firefox's prompt for a \
             site now says: \"Allow shop.example to use your camera and microphone?\""
        ) && text.ends_with("nothing was pressed"),
        "{text}"
    );
    assert!(!h.steps().iter().any(|s| s.starts_with("a11y press")));
}

/// H2: the tab switched while the owner was asked (by the extension's listing and by the window's
/// selected tab): refused, nothing pressed.
#[tokio::test]
async fn a_tab_switch_during_the_question_presses_nothing() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let a = with_access(&h, window("https://shop.example/cart", ""), None).await;
    h.set(allowed(), BrowserConfig::default());
    let model = ff.model.clone();
    let switched = a.clone();
    h.while_asked(move || {
        let mut m = model.lock().unwrap();
        for t in &mut m.tabs {
            let id = t["id"].as_i64();
            t["active"] = json!(id == Some(3) || id == Some(14));
        }
        *switched.app.lock().unwrap() = tree(vec![window_with(
            "https://mail.example/inbox",
            "Mail – Inbox",
            "",
            vec![prompt("Allow shop.example to know your location?")],
        )]);
    });
    let _ = h.steps();
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(
        err && text.contains("tab 12 is not the tab in front of its window")
            && text.ends_with("nothing was pressed"),
        "{text}"
    );
    assert!(!h.steps().iter().any(|s| s.starts_with("a11y press")));
}

/// The last look right before the press: the window changed after the gate opened (another tab is
/// selected now, the moment after a switch in which the page before still shows).
#[tokio::test]
async fn the_last_look_before_the_press_sees_a_switch() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let a = with_access(&h, window("https://shop.example/cart", ""), None).await;
    h.set(allowed(), BrowserConfig::default());
    let now = window("https://shop.example/cart", "");
    let after = tree(vec![window_with(
        "https://shop.example/cart",
        "Mail – Inbox",
        "",
        vec![],
    )]);
    // The preparation, the look after the confirmation, then the switch.
    a.next
        .lock()
        .unwrap()
        .extend([now.clone(), now, after.clone()]);
    let _ = h.steps();
    let (err, text) = h.owner("firefox_press", json!({"control": "Back"})).await;
    assert!(
        err && text.contains("its selected tab is not tab 12")
            && text.ends_with("nothing was pressed"),
        "{text}"
    );
    let steps = h.steps();
    assert!(steps.contains(&"gate.open@1".to_owned()), "{steps:?}");
    assert!(
        !steps.iter().any(|s| s.starts_with("a11y press")),
        "{steps:?}"
    );
    // firefox_ui at that moment: the tab is not the one in front of that window.
    let (err, text) = h.owner("firefox_ui", json!({})).await;
    assert!(
        err && text.contains("its selected tab is not tab 12"),
        "{text}"
    );
}

/// H3: the never-press list, whatever the rules, the commit guard and the tool's state: Firefox's
/// own pages that change how it works, an add-on's prompt, a screen share, a sign-in dialog.
#[tokio::test]
async fn the_never_press_list_holds_whatever_the_state() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &all_ops(),
            Model::with(vec![tab(
                12,
                1,
                "Settings",
                "about:preferences#privacy",
                true,
            )]),
        )
        .await;
    let a = with_access(
        &h,
        tree(vec![window_with(
            "about:preferences#privacy",
            "Settings",
            "",
            vec![],
        )]),
        None,
    )
    .await;
    a.subtrees.lock().unwrap().insert(
        "/page".into(),
        Element {
            document: Some("about:preferences#privacy".into()),
            ..at(
                el(
                    "document web",
                    "Settings",
                    "",
                    vec![at(el("button", "Saved passwords", "", vec![]), "/saved")],
                ),
                "/page",
            )
        },
    );
    h.set(allowed(), unguarded());
    let _ = h.steps();
    let (err, text) = h
        .owner("firefox_press", json!({"control": "Saved passwords"}))
        .await;
    assert!(
        err && text.contains(
            "about:preferences, Firefox's settings, a page of Firefox's that Kai never presses in"
        ),
        "{text}"
    );
    // A tool bar's button around it is not on the list.
    assert!(!h.owner("firefox_press", json!({"control": "Back"})).await.0);
    assert!(h.steps().contains(&"a11y press Back".to_owned()));
    // about:config and the others, by their names.
    for (url, name) in [
        ("about:config", "about:config"),
        ("about:addons", "about:addons"),
        ("about:debugging#/runtime/this-firefox", "about:debugging"),
        ("about:profiles", "about:profiles"),
        ("about:support", "about:support"),
        ("about:policies", "about:policies"),
    ] {
        ff.model().tabs = vec![tab(12, 1, "Settings", url, true)];
        *a.app.lock().unwrap() = tree(vec![window_with(url, "Settings", "", vec![])]);
        let (err, text) = h
            .owner("firefox_press", json!({"control": "Saved passwords"}))
            .await;
        assert!(
            err && text.contains(name) && text.contains("never presses in"),
            "{url}: {text}"
        );
    }
    // An add-on's prompt on a site's tab: not read, and refused by name.
    ff.model().tabs = vec![tab(12, 1, "Checkout", "https://shop.example/cart", true)];
    let mut addon = prompt("Add Password Helper? It requires your permission to:");
    addon.children.push(el(
        "label",
        "Access your data for all websites",
        "addon-webext-perm-list-required",
        vec![],
    ));
    addon.children.push(at(
        el(
            "check box",
            "Allow this extension to run in private windows",
            "",
            vec![],
        ),
        "/private",
    ));
    *a.app.lock().unwrap() = tree(vec![window_with(
        "https://shop.example/cart",
        "Checkout",
        "",
        vec![addon],
    )]);
    let text = h.owner("firefox_ui", json!({})).await.1;
    assert!(!text.contains("Password Helper"), "{text}");
    for control in ["Allow", "private windows"] {
        let (err, text) = h.owner("firefox_press", json!({"control": control})).await;
        assert!(
            err && text.contains("Firefox's prompt about an add-on")
                && text.contains("never presses"),
            "{control}: {text}"
        );
    }
    // A screen share's prompt.
    let mut screen = prompt("Allow shop.example to see your screen?");
    screen.children.push(el(
        "combo box",
        "Window or screen to share",
        "webRTC-selectWindow-menulist",
        vec![],
    ));
    *a.app.lock().unwrap() = tree(vec![window_with(
        "https://shop.example/cart",
        "Checkout",
        "",
        vec![screen],
    )]);
    let (err, text) = h.owner("firefox_press", json!({"control": "Allow"})).await;
    assert!(
        err && text.contains("share the screen or a window"),
        "{text}"
    );
    // A page dialog with a password field (a sign-in).
    let mut d = dialog(TITLE, "Sign in", false);
    d.children
        .insert(1, at(el("password text", "Password", "", vec![]), "/pw"));
    *a.app.lock().unwrap() = window("https://shop.example/cart", TITLE);
    a.subtrees.lock().unwrap().insert("/dialog".into(), d);
    let (err, text) = h.owner("firefox_press", json!({"control": "OK"})).await;
    assert!(
        err && text.contains("a dialog with a password field"),
        "{text}"
    );
    assert!(!h.steps().iter().any(|s| s.starts_with("a11y press")));
}

/// `page_text` on one of Firefox's own pages: read through accessibility in the tab in front;
/// the strictest rule while some site is `none`.
#[tokio::test]
async fn page_text_reads_firefoxs_own_page_through_accessibility() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &all_ops(),
            Model::with(vec![tab(12, 1, "Settings", "about:preferences", true)]),
        )
        .await;
    ff.model().twists.insert(
        "page.text@1",
        Twist::Fail(
            "firefox_page",
            "Kai cannot read Firefox's own pages; tab 12 shows one",
        ),
    );
    let a = with_access(
        &h,
        tree(vec![window_with(
            "about:preferences",
            "Settings",
            "",
            vec![],
        )]),
        None,
    )
    .await;
    let mut checkbox = el(
        "check box",
        "Always check if Firefox is your default browser",
        "",
        vec![],
    );
    checkbox.states = vec!["checked"];
    a.subtrees.lock().unwrap().insert(
        "/page".into(),
        Element {
            document: Some("about:preferences".into()),
            ..el(
                "document web",
                "Settings",
                "",
                vec![el("heading", "General", "", vec![]), checkbox],
            )
        },
    );
    let (err, text) = h.owner("page_text", json!({})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "\"Settings\" about:preferences (tab 12): Firefox's own page, read through accessibility \
         (its text is the page's)\n\
         heading \"General\"\n\
         check box \"Always check if Firefox is your default browser\" (checked)"
    );
    h.set(PolicyConfig::default(), sites(&[], &["bank.example"]));
    let (err, text) = h.owner("page_text", json!({})).await;
    assert!(err && text.contains("strictest rule"), "{text}");
}

/// Never read or pressed in: the password manager and another extension's page (H1), whatever
/// the rules.
#[tokio::test]
async fn the_password_manager_and_other_extensions_pages_are_never_read() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &all_ops(),
            Model::with(vec![tab(12, 1, "Vault", "about:logins", true)]),
        )
        .await;
    ff.model().twists.insert(
        "page.text@1",
        Twist::Fail(
            "firefox_page",
            "Kai cannot read Firefox's own pages; tab 12 shows one",
        ),
    );
    let a = with_access(&h, window("about:logins", ""), None).await;
    for (url, what) in [
        ("about:logins", "Firefox's password manager (about:logins)"),
        (
            "moz-extension://4f0c2a/vault.html#unlock",
            "another extension's page (moz-extension:)",
        ),
    ] {
        ff.model().tabs = vec![tab(12, 1, "Vault", url, true)];
        *a.app.lock().unwrap() = tree(vec![window_with(url, "Vault", "", vec![])]);
        for (tool, args) in [
            ("page_text", json!({})),
            ("firefox_ui", json!({})),
            ("firefox_press", json!({"control": "Back"})),
        ] {
            let (err, text) = h.owner(tool, args).await;
            assert!(
                err && text.contains(&format!(
                    "shows {what}, which Kai never reads or presses in"
                )),
                "{tool} on {url}: {text}"
            );
        }
    }
}

/// The refusals the review named: `firefox_press` on one of Firefox's own pages while the
/// strictest rule is read, and Reader view of a `none` site.
#[tokio::test]
async fn own_pages_and_reader_view_follow_the_strictest_and_the_sites_rule() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &all_ops(),
            Model::with(vec![tab(12, 1, "Gort!", "about:robots", true)]),
        )
        .await;
    ff.model().twists.insert(
        "page.text@1",
        Twist::Fail(
            "reader_view",
            "Kai cannot read Reader view; tab 12 shows it",
        ),
    );
    let a = with_access(
        &h,
        tree(vec![window_with("about:robots", "Gort!", "", vec![])]),
        None,
    )
    .await;
    h.set(allowed(), sites(&["bank.example"], &[]));
    let (err, text) = h.owner("firefox_press", json!({"control": "Back"})).await;
    assert!(
        err && text.contains("one of Firefox's own pages, which get the strictest rule configured"),
        "{text}"
    );
    // Reader view of a none site: refused, its page never read.
    let reader = "about:reader?url=https%3A%2F%2Fmail.example%2Fletter";
    ff.model().tabs = vec![tab(12, 1, "Letter", reader, true)];
    *a.app.lock().unwrap() = tree(vec![window_with(reader, "Letter", "", vec![])]);
    a.subtrees.lock().unwrap().insert(
        "/page".into(),
        el(
            "document web",
            "Letter",
            "",
            vec![el("paragraph", "Dear Ada", "", vec![])],
        ),
    );
    h.set(PolicyConfig::default(), sites(&[], &["mail.example"]));
    for tool in ["page_text", "firefox_ui"] {
        let (err, text) = h.owner(tool, json!({})).await;
        assert!(err && !text.contains("Dear Ada"), "{tool}: {text}");
    }
}
