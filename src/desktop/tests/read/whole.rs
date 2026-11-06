//! The reads and presses Kai's browser module builds on (`fdo::atspi::whole`) against the fake
//! accessibility bus of the parent module: an app's whole tree with each element's id, classes,
//! SHOWING, place and action, its documents not walked into where asked and a background tab's
//! page left out; one element's subtree; an element by its place; an element a page marked, found
//! through the documents' Collection only in a page that is shown and only when exactly one
//! carries the mark; a dialog's text field's text replaced, never a password field's.

use super::*;
use kai_desktop::caps::accessibility::{Element as A11yElement, How, Marked};

const ENABLED: u32 = 8;
const SHOWING: u32 = 25;
const DOCUMENT_WEB: u32 = 95;
const DIALOG: u32 = 16;
const ENTRY: u32 = 79;
const FRONT: &[u32] = &[VISIBLE, ENABLED, SHOWING];

fn url(u: &str) -> Arc<Mutex<String>> {
    Arc::new(Mutex::new(u.into()))
}

fn button(name: &'static str, attrs: &'static [(&'static str, &'static str)]) -> Spec {
    Spec {
        role: 43,
        name,
        states: FRONT,
        actions: &["press"],
        attrs,
        ..Default::default()
    }
}

struct Fixture {
    prompt: Arc<Mutex<String>>,
    secret: Arc<Mutex<String>>,
}

/// Firefox's window: its navigation bar, the tab panel in front with the page and the page's
/// dialog (a document of Firefox's own, with a text field), a background tab whose page carries
/// a mark too, and a second marked element only where `twice`.
fn firefox(f: &Fixture, twice: bool) -> Spec {
    let marked: &'static [(&'static str, &'static str)] = &[("kai-mark", "m1"), ("id", "buy")];
    let mut page_kids = vec![
        button("Add to cart", &[("id", "a1")]),
        button("Add to cart", marked),
        // A frame of the page: its elements are searched too.
        Spec {
            role: DOCUMENT_WEB,
            states: &[VISIBLE, ENABLED],
            doc: Some(url("https://pay.example/")),
            children: vec![button("Pay", &[("kai-mark", "m2")])],
            ..Default::default()
        },
    ];
    if twice {
        page_kids.push(button("Copy", &[("kai-mark", "m1")]));
    }
    let page = Spec {
        role: DOCUMENT_WEB,
        states: FRONT,
        doc: Some(url("https://shop.example/")),
        children: page_kids,
        ..Default::default()
    };
    let dialog = Spec {
        role: DIALOG,
        name: "The page at https://shop.example says:",
        states: FRONT,
        doc: Some(url("chrome://global/content/commonDialog.xhtml")),
        attrs: &[("id", "commonDialogWindow")],
        children: vec![
            Spec {
                role: 29,
                name: "Your name?",
                states: FRONT,
                attrs: &[("class", "infoBody")],
                ..Default::default()
            },
            Spec {
                role: ENTRY,
                name: "Your name?",
                states: FRONT,
                text: Some("Ada"),
                editable: Some(f.prompt.clone()),
                ..Default::default()
            },
            Spec {
                role: ENTRY,
                name: "Sticky",
                states: FRONT,
                text: Some("kept"),
                editable: Some(Arc::new(Mutex::new("kept".into()))),
                sticky: true,
                ..Default::default()
            },
            Spec {
                role: 40,
                name: "Password",
                states: FRONT,
                editable: Some(f.secret.clone()),
                ..Default::default()
            },
            button("OK", &[]),
        ],
        ..Default::default()
    };
    let background = Spec {
        role: DOCUMENT_WEB,
        states: &[VISIBLE, ENABLED],
        doc: Some(url("https://bank.example/")),
        children: vec![button("Transfer", &[("kai-mark", "m3")])],
        ..Default::default()
    };
    let frame = Spec {
        role: 23,
        name: "Shop — Mozilla Firefox",
        states: FRONT,
        doc: Some(url("chrome://browser/content/browser.xhtml")),
        attrs: &[("id", "main-window")],
        children: vec![
            Spec {
                role: 63,
                name: "Navigation",
                states: FRONT,
                attrs: &[("id", "nav-bar"), ("class", "browser-toolbar")],
                children: vec![
                    button("Back", &[("id", "back-button")]),
                    // Visible but not shown (scrolled away, a closed popup's).
                    Spec {
                        role: 43,
                        name: "Forward",
                        states: &[VISIBLE, ENABLED],
                        actions: &["press"],
                        attrs: &[("id", "forward-button")],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            Spec {
                role: 39,
                states: FRONT,
                attrs: &[("id", "tabbrowser-tabpanels")],
                children: vec![
                    Spec {
                        role: 49,
                        states: FRONT,
                        attrs: &[("class", "browserSidebarContainer deck-selected")],
                        children: vec![
                            Spec {
                                role: 28,
                                states: FRONT,
                                children: vec![page],
                                ..Default::default()
                            },
                            Spec {
                                role: 28,
                                states: FRONT,
                                attrs: &[("class", "dialogFrame")],
                                children: vec![dialog],
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                    Spec {
                        role: 49,
                        states: &[VISIBLE, ENABLED],
                        children: vec![Spec {
                            role: 28,
                            states: &[VISIBLE, ENABLED],
                            children: vec![background],
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    Spec {
        role: 75,
        name: "Firefox",
        states: FRONT,
        children: vec![frame],
        ..Default::default()
    }
}

fn fixture() -> Fixture {
    Fixture {
        prompt: Arc::new(Mutex::new("Ada".into())),
        secret: Arc::new(Mutex::new(String::new())),
    }
}

/// Every element below `e`, depth first.
fn all(e: &A11yElement) -> Vec<&A11yElement> {
    let mut out = vec![e];
    for c in &e.children {
        out.extend(all(c));
    }
    out
}

#[tokio::test]
async fn an_apps_tree_has_its_windows_with_each_elements_details() {
    let f = fixture();
    let Some(b) = a11y_with(true, firefox(&f, false), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let pid = std::process::id();
    let how = How {
        details: true,
        into_documents: false,
    };
    let tree = a.app_tree(pid, how).await.unwrap();
    assert_eq!(tree.root.role, "application");
    let window = &tree.root.children[0];
    assert_eq!(window.role, "frame");
    assert_eq!(window.id, "main-window");
    assert_eq!(window.document, None, "a window's own document is no page");
    let nodes = all(window);
    let back = nodes.iter().find(|e| e.id == "back-button").unwrap();
    assert!(back.pressable && back.showing && back.at.is_some());
    let deck = nodes
        .iter()
        .find(|e| e.class.contains("deck-selected"))
        .unwrap();
    assert_eq!(deck.role, "scroll pane");
    // The pages: the shown one and the dialog, as one element each; the background tab's not at
    // all.
    let docs: Vec<(&str, usize)> = nodes
        .iter()
        .filter_map(|e| e.document.as_deref().map(|d| (d, e.children.len())))
        .collect();
    assert_eq!(
        docs,
        [
            ("https://shop.example/", 0),
            ("chrome://global/content/commonDialog.xhtml", 0)
        ]
    );
    // The dialog's subtree, walked from its place.
    let dialog = nodes.iter().find(|e| e.id == "commonDialogWindow").unwrap();
    let sub = a.subtree(dialog.at.as_ref().unwrap(), how).await.unwrap();
    assert_eq!(
        sub.root.document.as_deref(),
        Some("chrome://global/content/commonDialog.xhtml")
    );
    let kids: Vec<(&str, &str, &str)> = sub
        .root
        .children
        .iter()
        .map(|e| (e.role.as_str(), e.name.as_str(), e.text.as_str()))
        .collect();
    assert_eq!(
        kids,
        [
            ("label", "Your name?", ""),
            ("entry", "Your name?", "Ada"),
            ("entry", "Sticky", "kept"),
            ("password text", "Password", ""),
            ("button", "OK", "")
        ]
    );
    // Walked into the documents, the page's elements are there, the background's are not.
    let deep = a
        .app_tree(
            pid,
            How {
                details: false,
                into_documents: true,
            },
        )
        .await
        .unwrap();
    let names: Vec<&str> = all(&deep.root).iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"Pay") && !names.contains(&"Transfer"),
        "{names:?}"
    );
    // An element by its place: its documents and that its page is shown.
    let ok = all(&sub.root)
        .into_iter()
        .find(|e| e.name == "OK")
        .unwrap()
        .at
        .clone()
        .unwrap();
    let found = a.found_at(&ok).await.unwrap();
    assert_eq!(
        found.documents,
        ["chrome://global/content/commonDialog.xhtml"]
    );
    assert_eq!(a.press(&found).await.unwrap().name, "OK");
}

/// The shown page's place in the app's tree.
async fn shown_page(a: &Atspi) -> (String, String) {
    let how = How {
        details: true,
        into_documents: false,
    };
    let tree = a.app_tree(std::process::id(), how).await.unwrap();
    all(&tree.root)
        .into_iter()
        .find(|e| e.document.as_deref() == Some("https://shop.example/"))
        .and_then(|e| e.at.clone())
        .unwrap()
}

#[tokio::test]
async fn an_element_a_page_marked_is_found_in_the_page_asked_and_a_copy_is_said() {
    let f = fixture();
    let Some(b) = a11y_with(true, firefox(&f, false), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let page = shown_page(&a).await;
    let Marked::One(found) = a.find_marked(&page, "kai-mark", "m1").await.unwrap() else {
        panic!("one element carries m1");
    };
    assert_eq!(
        (found.role.as_str(), found.name.as_str()),
        ("button", "Add to cart")
    );
    assert_eq!(found.documents, ["https://shop.example/"]);
    // In a frame of the page.
    let Marked::One(pay) = a.find_marked(&page, "kai-mark", "m2").await.unwrap() else {
        panic!("one element carries m2");
    };
    assert_eq!(
        pay.documents,
        ["https://shop.example/", "https://pay.example/"]
    );
    // Only the page asked is searched: the background tab's mark is not found there.
    assert_eq!(
        a.find_marked(&page, "kai-mark", "m3").await.unwrap(),
        Marked::Nothing
    );
    // Two elements with the mark: said, none taken.
    let Some(b) = a11y_with(true, firefox(&f, true), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let page = shown_page(&a).await;
    assert_eq!(
        a.find_marked(&page, "kai-mark", "m1").await.unwrap(),
        Marked::Several(2)
    );
}

/// An element by its place is refused once it is not shown on the screen (Firefox's own UI has
/// no page whose SHOWING would say it).
#[tokio::test]
async fn an_element_by_its_place_must_be_shown() {
    let f = fixture();
    let Some(b) = a11y_with(true, firefox(&f, false), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let how = How {
        details: true,
        into_documents: false,
    };
    let tree = a.app_tree(std::process::id(), how).await.unwrap();
    let place = |id: &str| {
        all(&tree.root)
            .into_iter()
            .find(|e| e.id == id)
            .and_then(|e| e.at.clone())
            .unwrap()
    };
    assert!(a.found_at(&place("back-button")).await.is_ok());
    let e = a.found_at(&place("forward-button")).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "button \"Forward\" is not shown on the screen now"
    );
}

#[tokio::test]
async fn a_dialogs_text_field_takes_a_text_and_a_password_field_never_does() {
    let f = fixture();
    let Some(b) = a11y_with(true, firefox(&f, false), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a
        .app_tree(
            std::process::id(),
            How {
                details: true,
                into_documents: false,
            },
        )
        .await
        .unwrap();
    let dialog = all(&tree.root)
        .into_iter()
        .find(|e| e.id == "commonDialogWindow")
        .unwrap()
        .at
        .clone()
        .unwrap();
    let sub = a
        .subtree(
            &dialog,
            How {
                details: true,
                into_documents: false,
            },
        )
        .await
        .unwrap();
    let at = |role: &str| {
        all(&sub.root)
            .into_iter()
            .find(|e| e.role == role)
            .unwrap()
            .at
            .clone()
            .unwrap()
    };
    // A field that answers yes and keeps its text (Firefox's prompt, headless): an error.
    let sticky = all(&sub.root)
        .into_iter()
        .find(|e| e.name == "Sticky")
        .unwrap()
        .at
        .clone()
        .unwrap();
    let sticky = a.found_at(&sticky).await.unwrap();
    let e = a.set_text(&sticky, "Grace").await.unwrap_err().to_string();
    assert_eq!(
        e,
        "entry \"Sticky\" did not take the text through accessibility (it answered yes, and its \
         text stayed as it was)"
    );
    let entry = a.found_at(&at("entry")).await.unwrap();
    a.set_text(&entry, "Grace").await.unwrap();
    assert_eq!(*f.prompt.lock().unwrap(), "Grace");
    let secret = a.found_at(&at("password text")).await.unwrap();
    let e = a.set_text(&secret, "hunter2").await.unwrap_err();
    assert_eq!(e.to_string(), "Kai never types into a password field");
    assert_eq!(*f.secret.lock().unwrap(), "");
}
