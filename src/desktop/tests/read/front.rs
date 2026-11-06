//! Kai for Firefox's WP12 fixes of the AT-SPI walk (`fdo::atspi`'s module doc) against the fake
//! accessibility bus of the parent module, with the facts of kai-firefox WP0 (d2): a background
//! tab's page is visible but not shown, so it is neither read nor pressed in, while a frame of the
//! page in front is read whether scrolled away or not; a press whose page went to the background
//! after it was found is refused; an empty `DocURL` (Firefox's answer for `data:` and `blob:`) is
//! passed on as empty, never dropped; an element whose first `NActions` read says none is read
//! again after `accessibility.action_settle_ms`.

use std::sync::atomic::AtomicUsize;

use super::*;
use kai_desktop::caps::accessibility::Element as A11yElement;

const ENABLED: u32 = 8;
const SHOWING: u32 = 25;
const DOCUMENT_WEB: u32 = 95;
const FRONT: &[u32] = &[VISIBLE, ENABLED, SHOWING];

fn url(u: &str) -> Arc<Mutex<String>> {
    Arc::new(Mutex::new(u.into()))
}

fn button(name: &'static str) -> Spec {
    Spec {
        role: 43,
        name,
        states: FRONT,
        actions: &["press"],
        ..Default::default()
    }
}

fn paragraph(text: &'static str) -> Spec {
    Spec {
        role: 73,
        states: FRONT,
        text: Some(text),
        selections: &[(0, 5)],
        ..Default::default()
    }
}

/// A page's document: shown or (`states` without SHOWING) only visible.
fn doc(url: &Arc<Mutex<String>>, states: &'static [u32], children: Vec<Spec>) -> Spec {
    Spec {
        role: DOCUMENT_WEB,
        states,
        doc: Some(url.clone()),
        children,
        ..Default::default()
    }
}

/// Firefox's window as WP0 (d2) found it: its own button, then the tab panels, each a scroll pane
/// holding the tab's browser (an internal frame) and its document. The news tab is in front, with
/// a frame of the mail site scrolled away (visible, not shown); the bank tab is in the background
/// (visible, not shown), and so is its whole panel.
fn tabs(news: Spec, bank_states: &'static [u32]) -> Spec {
    let panel = |states: &'static [u32], page: Spec| Spec {
        role: 49,
        states,
        children: vec![Spec {
            role: 28,
            states,
            children: vec![page],
            ..Default::default()
        }],
        ..Default::default()
    };
    let bank = doc(
        &url("https://bank.example/"),
        bank_states,
        vec![paragraph("balance 1200"), button("Transfer")],
    );
    let frame = Spec {
        role: 23,
        name: "News — Mozilla Firefox",
        states: SHOWN,
        children: vec![
            button("Reload"),
            Spec {
                role: 39,
                states: FRONT,
                children: vec![panel(FRONT, news), panel(&[VISIBLE, ENABLED], bank)],
                ..Default::default()
            },
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

fn news(news_states: Option<Arc<Mutex<Vec<u32>>>>) -> Spec {
    Spec {
        live_states: news_states,
        ..doc(
            &url("https://news.example/"),
            FRONT,
            vec![
                paragraph("front story"),
                button("Send"),
                doc(
                    &url("https://mail.example/"),
                    &[VISIBLE, ENABLED],
                    vec![button("Reply")],
                ),
            ],
        )
    }
}

fn target() -> Target {
    Target {
        pid: std::process::id(),
        title: "News — Mozilla Firefox".into(),
        exact: true,
    }
}

fn texts(e: &A11yElement, out: &mut Vec<String>) {
    if !e.text.is_empty() {
        out.push(e.text.clone());
    }
    if !e.name.is_empty() {
        out.push(e.name.clone());
    }
    for c in &e.children {
        texts(c, out);
    }
}

/// The page in front is read, its frame scrolled away too; the background tab's page is not, in
/// a read, in the selection or in a find, though Firefox marks it visible.
#[tokio::test]
async fn only_the_page_in_front_is_read_and_pressed_in() {
    let Some(b) = a11y_with(true, tabs(news(None), &[VISIBLE, ENABLED]), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a.tree(&target()).await.unwrap();
    let mut seen = Vec::new();
    texts(&tree.root, &mut seen);
    assert!(seen.contains(&"front story".to_owned()), "{seen:?}");
    assert!(
        seen.contains(&"Reply".to_owned()),
        "a frame scrolled away: {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|t| t.contains("balance") || t == "Transfer"),
        "the background tab's page: {seen:?}"
    );
    let sel = a.selection(&target()).await.unwrap().texts();
    assert_eq!(sel, ["front"], "only the page in front's selection");
    let e = a
        .find(&target(), &Query::parse("Transfer").unwrap())
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("no pressable element"), "{e}");
    let reply = a
        .find(&target(), &Query::parse("Reply").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reply.documents,
        ["https://news.example/", "https://mail.example/"]
    );
    // With the bank tab in front instead (shown), its page is read.
    let Some(b) = a11y_with(true, tabs(news(None), FRONT), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let mut seen = Vec::new();
    texts(&a.tree(&target()).await.unwrap().root, &mut seen);
    assert!(seen.contains(&"balance 1200".to_owned()), "{seen:?}");
}

/// The tab of a found element went to the background before the press: nothing is pressed.
#[tokio::test]
async fn a_press_in_a_page_no_longer_shown_is_refused() {
    let states = Arc::new(Mutex::new(FRONT.to_vec()));
    let Some(b) = a11y_with(
        true,
        tabs(news(Some(states.clone())), &[VISIBLE, ENABLED]),
        None,
    )
    .await
    else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let send = a
        .find(&target(), &Query::parse("Send").unwrap())
        .await
        .unwrap();
    *states.lock().unwrap() = vec![VISIBLE, ENABLED];
    let e = a.press(&send).await.unwrap_err().to_string();
    assert_eq!(
        e,
        "the page button \"Send\" is in is no longer shown (another tab came to the front of its \
         window); nothing was pressed"
    );
    *states.lock().unwrap() = FRONT.to_vec();
    assert_eq!(a.press(&send).await.unwrap().action, "press");
    // Firefox's own button is in no page, so no page decides it.
    let reload = a
        .find(&target(), &Query::parse("Reload").unwrap())
        .await
        .unwrap();
    assert!(reload.documents.is_empty());
    assert_eq!(a.press(&reload).await.unwrap().name, "Reload");
}

/// Firefox answers `DocURL` with the empty string for a `data:` or a `blob:` document: the walk
/// and the find pass it on as empty (so Kai's rules give it the strictest rule), never as no
/// document.
#[tokio::test]
async fn an_empty_doc_url_is_passed_on_as_empty() {
    let blank = doc(&url(""), FRONT, vec![button("Inside")]);
    let page = doc(
        &url("https://news.example/"),
        FRONT,
        vec![paragraph("story"), blank],
    );
    let Some(b) = a11y_with(true, tabs(page, &[VISIBLE, ENABLED]), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a.tree(&target()).await.unwrap();
    fn docs(e: &A11yElement, out: &mut Vec<Option<String>>) {
        if e.document.is_some() {
            out.push(e.document.clone());
        }
        for c in &e.children {
            docs(c, out);
        }
    }
    let mut got = Vec::new();
    docs(&tree.root, &mut got);
    assert_eq!(
        got,
        [Some("https://news.example/".into()), Some(String::new())]
    );
    let inside = a
        .find(&target(), &Query::parse("Inside").unwrap())
        .await
        .unwrap();
    assert_eq!(inside.documents, ["https://news.example/", ""]);
    // What Kai's rules make of it: the strictest rule configured.
    let sites = crate::browser::sites::SiteRules::read(&crate::browser::settings::Sites {
        act: Vec::new(),
        read: vec!["shop.example".into()],
        none: vec!["bank.example".into()],
    })
    .unwrap();
    assert_eq!(sites.page_rule(""), crate::browser::sites::Rule::None);
}

fn late_button(name: &'static str, late: usize) -> (Spec, Arc<AtomicUsize>, Done) {
    let reads = Arc::new(AtomicUsize::new(late));
    let done: Done = Done::default();
    let spec = Spec {
        late_actions: Some(reads.clone()),
        done: Some(done.clone()),
        ..button(name)
    };
    (spec, reads, done)
}

/// Firefox's first `NActions` read of a link or a button can be 0 and the next, 100 ms later, 1
/// (kai-firefox WP0 (d2)): such an element is read again after the settle, at the find and at the
/// press; one that still says none is not taken, and the refusal names the setting.
#[tokio::test]
async fn actions_not_ready_at_the_first_read_are_read_again() {
    let (go, _, go_done) = late_button("Go", 1);
    let (never, _, _) = late_button("Never", usize::MAX);
    let (later, later_reads, later_done) = late_button("Later", 0);
    let page = doc(&url("https://news.example/"), FRONT, vec![go, never, later]);
    let Some(b) = a11y_with(true, tabs(page, &[VISIBLE, ENABLED]), None).await else {
        return;
    };
    let mut a = atspi(&b, b.session.connect().await);
    a.action_settle = Duration::from_millis(150);
    let began = std::time::Instant::now();
    let found = a
        .find(&target(), &Query::parse("Go").unwrap())
        .await
        .unwrap();
    assert!(began.elapsed() >= Duration::from_millis(150), "it waited");
    assert_eq!(a.press(&found).await.unwrap().action, "press");
    assert_eq!(go_done.lock().unwrap().as_slice(), [("Go", 0)]);
    let e = a
        .find(&target(), &Query::parse("Never").unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("elements of that name had no action, also 150 ms later (accessibility.action_settle_ms)"),
        "{e}"
    );
    // Found ready, then not at the press's first read: read again after the settle.
    let found = a
        .find(&target(), &Query::parse("Later").unwrap())
        .await
        .unwrap();
    later_reads.store(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(a.press(&found).await.unwrap().name, "Later");
    assert_eq!(later_done.lock().unwrap().as_slice(), [("Later", 0)]);
    // A find that names no element whose actions were late does not wait.
    let began = std::time::Instant::now();
    a.find(&target(), &Query::parse("Reload").unwrap())
        .await
        .unwrap();
    assert!(began.elapsed() < Duration::from_millis(150));
}
