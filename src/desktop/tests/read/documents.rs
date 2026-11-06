//! The documents AT-SPI names below a window (`fdo::atspi`'s module doc) against the fake
//! accessibility bus of the parent module: each page's and frame's address in a read and in the
//! selection, the documents an element found for `ui_press` is in, a press refused once a frame
//! went to another address, and a window taken by its exact name only where the target asks.

use super::*;

const ENABLED: u32 = 8;
const DOCUMENT_WEB: u32 = 95;

/// A page in front: shown (`SHOWING`, 25), as Firefox marks only the page in front (kai-firefox
/// WP0 (d2); `front.rs` has a background tab's).
fn doc(url: &Arc<Mutex<String>>, children: Vec<Spec>) -> Spec {
    Spec {
        role: DOCUMENT_WEB,
        states: &[VISIBLE, 8, 25],
        doc: Some(url.clone()),
        children,
        ..Default::default()
    }
}

/// Firefox's window: a toolbar button of its own, the news page with a paragraph and the mail's
/// frame holding a button.
fn firefox(news: &Arc<Mutex<String>>, mail: &Arc<Mutex<String>>) -> Spec {
    let button = |name: &'static str| Spec {
        role: 43,
        name,
        states: &[VISIBLE, ENABLED],
        actions: &["press"],
        ..Default::default()
    };
    let frame = Spec {
        role: 23,
        name: "News — Mozilla Firefox",
        states: SHOWN,
        children: vec![
            button("Reload"),
            doc(
                news,
                vec![
                    Spec {
                        role: 73,
                        states: SHOWN,
                        text: Some("the story"),
                        selections: &[(4, 9)],
                        ..Default::default()
                    },
                    doc(mail, vec![button("Send")]),
                ],
            ),
        ],
        ..Default::default()
    };
    Spec {
        role: 75,
        name: "firefox",
        states: SHOWN,
        children: vec![frame],
        ..Default::default()
    }
}

fn target(title: &str, exact: bool) -> Target {
    Target {
        pid: std::process::id(),
        title: title.into(),
        exact,
    }
}

fn url(u: &str) -> Arc<Mutex<String>> {
    Arc::new(Mutex::new(u.into()))
}

#[tokio::test]
async fn reads_and_presses_carry_the_documents_they_are_in() {
    let (news, mail) = (url("https://news.example/"), url("https://mail.example/"));
    let Some(b) = a11y_with(true, firefox(&news, &mail), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let t = target("News — Mozilla Firefox", true);
    // The read: the window itself is no document; each page and frame carries its address.
    let tree = a.tree(&t).await.unwrap();
    assert_eq!(tree.root.document, None);
    let page = &tree.root.children[1];
    assert_eq!(page.document.as_deref(), Some("https://news.example/"));
    assert_eq!(
        page.children[1].document.as_deref(),
        Some("https://mail.example/")
    );
    assert_eq!(page.children[0].document, None);
    // The selection too.
    let sel = a.selection(&t).await.unwrap();
    assert_eq!(sel.texts(), ["story"]);
    assert_eq!(
        sel.root.children[1].document.as_deref(),
        Some("https://news.example/")
    );
    // An element found for ui_press: the documents it is in, outermost first; Firefox's own
    // button is in none.
    let send = a.find(&t, &Query::parse("Send").unwrap()).await.unwrap();
    assert_eq!(
        send.documents,
        ["https://news.example/", "https://mail.example/"]
    );
    let reload = a.find(&t, &Query::parse("Reload").unwrap()).await.unwrap();
    assert!(reload.documents.is_empty());
    assert_eq!(a.press(&reload).await.unwrap().action, "press");
    // The frame went elsewhere after the confirmation: nothing is pressed.
    *mail.lock().unwrap() = "https://bank.example/".into();
    assert_eq!(
        a.press(&send).await.unwrap_err().to_string(),
        "the page button \"Send\" is in changed after the confirmation; nothing was pressed"
    );
    *mail.lock().unwrap() = "https://mail.example/".into();
    assert_eq!(a.press(&send).await.unwrap().name, "Send");
}

/// Exactly only where the target asks: the app's only window is not taken for another title.
#[tokio::test]
async fn a_window_is_taken_by_its_exact_name_where_the_target_asks() {
    let (news, mail) = (url("https://news.example/"), url("https://mail.example/"));
    let Some(b) = a11y_with(true, firefox(&news, &mail), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    assert!(
        a.tree(&target("Inbox — Mozilla Firefox", false))
            .await
            .is_ok()
    );
    let e = a
        .tree(&target("Inbox — Mozilla Firefox", true))
        .await
        .unwrap_err();
    assert!(
        e.to_string().starts_with("none of the app's windows"),
        "{e}"
    );
}
