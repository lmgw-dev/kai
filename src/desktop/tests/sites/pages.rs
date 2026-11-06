//! The pages a desktop tool read or presses in (`crate::desktop::sites::pages`): Firefox's window
//! taken exactly on the accessibility bus, the pages and frames of a `none` site left out of
//! `read_window` and `read_selection`, the checks after a read (a tab switched, the window
//! closed), and `ui_press` only inside pages Kai may act on. Every refusal asserts that nothing
//! was read or pressed.

use kai_desktop::caps::accessibility::{Element, Found, Tree};
use serde_json::json;

use super::*;
use crate::desktop::sites::pages::LEFT_OUT;

fn el(role: &str, name: &str, text: &str, children: Vec<Element>) -> Element {
    Element {
        role: role.into(),
        name: name.into(),
        text: text.into(),
        children,
        ..Default::default()
    }
}

fn doc(url: &str, children: Vec<Element>) -> Element {
    Element {
        role: "document web".into(),
        document: Some(url.into()),
        children,
        ..Default::default()
    }
}

/// Firefox's window as AT-SPI shows it: the tab strip (Firefox's own), the page with its frames.
fn page(top: &str) -> Tree {
    Tree {
        root: el(
            "frame",
            "News — Mozilla Firefox",
            "",
            vec![
                el("page tab", "News", "", vec![]),
                doc(
                    top,
                    vec![
                        el("paragraph", "", "the story", vec![]),
                        doc(
                            "https://mail.example/frame",
                            vec![el("paragraph", "", "a secret", vec![])],
                        ),
                        doc("about:blank", vec![el("paragraph", "", "an ad", vec![])]),
                        doc(
                            "https://shop.example/price",
                            vec![el("paragraph", "", "the price", vec![])],
                        ),
                    ],
                ),
            ],
        ),
        unanswered: 0,
    }
}

/// Under the rules Firefox's window is taken on the accessibility bus by its exact name only: the
/// app's only window there, the mail's, is not taken for the news window the list names.
#[tokio::test]
async fn firefoxs_window_is_taken_exactly_under_the_rules() {
    let mut d = Desk::new("ff3");
    d.access.windows = vec!["Inbox — Mozilla Firefox".into()];
    let s = d.services(&[MAIL], &[]);
    for (tool, v) in [
        ("read_window", json!({"window": "ff3"})),
        ("read_selection", json!({})),
    ] {
        let r = read(&s, tool, v).await;
        assert!(r.error, "{tool}");
        assert!(
            first(&r).starts_with("none of the app's windows"),
            "{}",
            first(&r)
        );
    }
    let e = act(&s, "ui_press", json!({"element": "Send"}))
        .await
        .unwrap_err();
    assert!(e.starts_with("none of the app's windows"), "{e}");
    assert!(d.access.picked.lock().unwrap().is_empty(), "nothing read");
    assert!(
        d.access.presses.lock().unwrap().is_empty(),
        "nothing pressed"
    );
    let asked = d.access.asked.lock().unwrap().clone();
    assert!(asked.iter().all(|t| t.exact), "{asked:?}");
    // Without a rule, or for another app's window, the provider's fallbacks hold.
    let s = d.services(&[], &[]);
    assert!(
        !read(&s, "read_window", json!({"window": "ff3"}))
            .await
            .error
    );
    let s = d.services(&[MAIL], &[]);
    let _ = read(&s, "read_window", json!({"window": "kon"})).await;
    let asked = d.access.asked.lock().unwrap().clone();
    assert!(!asked[asked.len() - 1].exact && !asked[asked.len() - 2].exact);
}

/// The pages and frames read are judged by their own addresses: a none site's, and a page without
/// a site (an about:blank frame) while a site is none, are left out with all they hold.
#[tokio::test]
async fn read_window_leaves_out_pages_and_frames_kai_may_not_read() {
    let mut d = Desk::new("kon");
    d.access.tree = page("https://news.example/");
    let s = d.services(&[MAIL], &[SHOP]);
    let r = read(&s, "read_window", json!({"window": "ff3"})).await;
    assert!(!r.error, "{}", first(&r));
    let text = first(&r);
    assert!(
        text.contains("the story") && text.contains("the price"),
        "{text}"
    );
    assert!(
        !text.contains("a secret") && !text.contains("an ad"),
        "{text}"
    );
    assert_eq!(text.matches(LEFT_OUT).count(), 2, "{text}");
    assert!(
        text.contains(
            "2 pages or frames of sites Kai does not reach, or ones Kai cannot tell, were left \
             out (Kai's site rules)"
        ),
        "{text}"
    );
    assert!(text.ends_with(GAP), "{text}");
    // The page itself on a none site (the bus took another window's, or the tab went there):
    // nothing of it is said.
    d.access.tree = page("https://mail.example/inbox");
    let s = d.services(&[MAIL], &[SHOP]);
    let text = first(&read(&s, "read_window", json!({"window": "ff3"})).await).to_owned();
    assert!(
        !text.contains("the story") && !text.contains("the price"),
        "{text}"
    );
    assert!(text.contains("1 page or frame of a site"), "{text}");
    // Only read sites: nothing is left out.
    let s = d.services(&[], &[SHOP]);
    let text = first(&read(&s, "read_window", json!({"window": "ff3"})).await).to_owned();
    assert!(
        text.contains("a secret") && !text.contains(LEFT_OUT),
        "{text}"
    );
}

#[tokio::test]
async fn read_selection_leaves_out_the_selection_in_frames_kai_may_not_read() {
    let mut d = Desk::new("ff3");
    d.access.selection_tree = Some(Tree {
        root: el(
            "frame",
            "",
            "",
            vec![doc(
                "https://news.example/",
                vec![
                    el("", "", "selected story", vec![]),
                    doc(
                        "https://mail.example/",
                        vec![el("", "", "selected secret", vec![])],
                    ),
                ],
            )],
        ),
        unanswered: 0,
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "read_selection", json!({})).await;
    assert!(!r.error, "{}", first(&r));
    assert_eq!(first(&r), "selected story");
    let Block::Text(notes) = &r.blocks[1] else {
        panic!("{:?}", r.blocks)
    };
    assert!(notes.starts_with("1 page or frame of a site"), "{notes}");
    assert!(notes.ends_with(GAP), "{notes}");
}

/// The window is checked again after the read: a tab gone to a none site meanwhile, or the
/// window closed, withholds what was read.
#[tokio::test]
async fn reads_are_checked_after_the_read_and_withheld_when_the_window_closed() {
    for (tool, v) in [
        ("read_window", json!({"window": "ff3"})),
        ("read_selection", json!({})),
    ] {
        let d = Desk::new("ff3");
        let s = d.services(&[MAIL], &[]);
        let fx = d.fx.clone();
        d.access.then(move || {
            fx.set(listing(
                vec![front(7, 3, "News", "https://mail.example/", false)],
                &[3],
                None,
            ))
        });
        let r = read(&s, tool, v.clone()).await;
        assert!(r.error, "{tool}");
        assert_eq!(
            first(&r),
            format!(
                "Firefox's window ff3 shows a site Kai does not reach (Kai's site rules) by the \
                 time {tool} had it, so Kai does not say what it got"
            )
        );
        assert_eq!(r.blocks.len(), 1, "nothing of what was read");
        let d = Desk::new("ff3");
        let s = d.services(&[MAIL], &[]);
        let windows = d.windows.clone();
        d.access
            .then(move || windows.change(|s| s.windows.retain(|w| w.id != "ff3")));
        let r = read(&s, tool, v).await;
        assert!(r.error);
        assert_eq!(
            first(&r),
            format!(
                "Firefox's window ff3 closed while {tool} ran, so Kai cannot tell what it showed \
                 (Kai's site rules) and does not say what it got"
            )
        );
        assert_eq!(r.blocks.len(), 1, "nothing of what was read");
    }
}

/// `ui_press` presses in Firefox's window only inside a page, and only where neither the page nor
/// the element's own frame is on a read or none site.
#[tokio::test]
async fn ui_press_presses_only_inside_pages_kai_may_act_on() {
    let news = "https://news.example/";
    for (documents, refusal) in [
        (vec![], Some("one of Firefox's own controls")),
        (
            vec![news, "https://mail.example/"],
            Some(
                "the frame of the element in Firefox's window ff3 shows a site Kai does not reach",
            ),
        ),
        (
            vec![news, "https://shop.example/"],
            Some(
                "the frame of the element in Firefox's window ff3 shows a site where Kai may only read",
            ),
        ),
        (
            vec!["https://shop.example/"],
            Some(
                "the page of the element in Firefox's window ff3 shows a site where Kai may only read",
            ),
        ),
        (vec![news, "about:blank"], Some("the frame of the element")),
        (vec![news, "https://cdn.news.example/"], None),
    ] {
        let mut d = Desk::new("ff3");
        d.access.found = Some(Found {
            role: "push button".into(),
            name: "Send".into(),
            at: (":1.5".into(), "/a/1".into()),
            documents: documents.iter().map(|u| (*u).to_owned()).collect(),
        });
        let s = d.services(&[MAIL], &[SHOP]);
        let r = act(&s, "ui_press", json!({"element": "Send"})).await;
        match refusal {
            Some(want) => {
                let e = r.unwrap_err();
                assert!(e.contains(want), "{documents:?}: {e}");
                assert!(e.ends_with("ui_press did not run"), "{e}");
                assert!(!e.contains("News"), "never the window's title: {e}");
                assert!(
                    d.access.presses.lock().unwrap().is_empty(),
                    "nothing pressed"
                );
            }
            None => {
                assert!(r.is_ok(), "{documents:?}: {r:?}");
                assert_eq!(d.access.presses.lock().unwrap().len(), 1);
            }
        }
    }
    // Firefox's own controls in another app's window are its own business.
    let mut d = Desk::new("kon");
    d.access.found.as_mut().unwrap().documents.clear();
    let s = d.services(&[MAIL], &[SHOP]);
    assert!(
        act(&s, "ui_press", json!({"element": "Send"}))
            .await
            .is_ok()
    );
}
