//! The window of the link's Firefox that shows a tab in front (the parent's module doc, "Which
//! window and which tab"), for `firefox_ui`, `firefox_press`, `page_text` on Firefox's own pages
//! and the press through accessibility (`super::super::trusted`).
//!
//! A window is the tab's when its page in front is the tab's address and its tab strip's selected
//! tab is the tab (named with the tab's title). Its page in front is the page in its selected tab
//! panel (Firefox's class `deck-selected` on the panel), not every page that is `SHOWING`: a
//! Firefox 159 on a private bus kept a background tab's page `SHOWING` for good (the first tab's,
//! after an add-on opened a tab), and keeps the page before `SHOWING` for about 230 ms after a tab
//! switch. Where no panel says it is the
//! selected one, the window's one shown page is taken, and two are refused (a split view). A window
//! whose page in front is the tab's address but whose selected tab is another (a second window on
//! the same page) is not the tab's; none, and two windows, are refused.
//!
//! An error page's `DocURL` is the address that failed (kai-firefox WP0 (d2), checked on Firefox
//! 159 on a private bus), which is the tab's; an `about:neterror?u=…` or `about:certerror?u=…`
//! form, should a Firefox give one, is read as its `u`.

use kai_desktop::caps::accessibility::{Accessibility, Element, How, Tree};

use crate::browser::tabs::Tab;

/// What Firefox must have for Kai to read it this way, for the errors.
pub const ON_THE_BUS: &str = "Firefox joins the accessibility bus only when the desktop's \
                              accessibility (org.a11y.Status IsEnabled) is on before it starts, or \
                              with accessibility.force_disabled = -1 in about:config; restart \
                              Firefox after switching it on";

/// `url` without its fragment.
pub fn bare(url: &str) -> &str {
    url.split_once('#').map_or(url, |(u, _)| u)
}

/// The address a document shows as its tab's (module doc).
fn shown_as(doc: &str) -> String {
    if let Ok(u) = url::Url::parse(doc)
        && u.scheme() == "about"
        && matches!(u.path(), "neterror" | "certerror" | "blocked")
        && let Some((_, failed)) = u.query_pairs().find(|(k, _)| k == "u")
    {
        return failed.into_owned();
    }
    doc.to_owned()
}

/// The window and the tab's page in it.
#[derive(Debug, Clone)]
pub struct Window {
    /// The window (`frame`), read with details and without its pages.
    pub frame: Element,
    /// The tab's page in it: a document element, with its place.
    pub page: Element,
}

/// The app's windows: the root's children, or its apps' children where it has several apps.
fn frames(tree: &Tree) -> Vec<&Element> {
    let root = &tree.root;
    if root.children.iter().any(|c| c.role == "application") {
        root.children.iter().flat_map(|a| &a.children).collect()
    } else {
        root.children.iter().collect()
    }
}

/// The documents below `e`, not looking inside them.
fn documents<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
    if e.document.is_some() {
        out.push(e);
        return;
    }
    for c in &e.children {
        documents(c, out);
    }
}

/// The window's page in front (module doc): the tab page in its selected tab panel, else its one
/// shown tab page; `Err` with how many it shows where it cannot tell.
fn in_front(frame: &Element) -> Result<Option<&Element>, usize> {
    let mut selected = Vec::new();
    let mut shown = Vec::new();
    for panels in frame
        .children
        .iter()
        .filter(|c| c.id == "tabbrowser-tabpanels")
    {
        for panel in &panels.children {
            let mut pages = Vec::new();
            documents(panel, &mut pages);
            pages.retain(|d| d.role != "dialog");
            if panel.class.split_whitespace().any(|k| k == "deck-selected") {
                selected.extend(pages.iter().copied());
            }
            shown.extend(pages);
        }
    }
    let pages = if selected.is_empty() { shown } else { selected };
    match pages.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        many => Err(many.len()),
    }
}

/// The selected tabs of a window's tab strip.
fn selected(e: &Element, out: &mut Vec<String>) {
    if e.document.is_some() {
        return;
    }
    if e.role == "page tab" && e.states.contains(&"selected") {
        out.push(e.name.clone());
    }
    for c in &e.children {
        selected(c, out);
    }
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The window of `tree` (an app read with details and without its pages) showing `tab` in front
/// (module doc).
pub fn of(tree: &Tree, tab: &Tab) -> Result<Window, String> {
    let want = bare(&tab.url);
    let is_tabs = |p: &Element| {
        p.document
            .as_deref()
            .is_some_and(|d| bare(&shown_as(d)) == want)
    };
    let mut found = Vec::new();
    let mut other_tab = 0;
    let mut unsure = None;
    for frame in frames(tree) {
        let page = match in_front(frame) {
            Ok(Some(p)) if is_tabs(p) => p,
            Ok(_) => continue,
            Err(n) => {
                let mut pages = Vec::new();
                documents(frame, &mut pages);
                if pages.iter().any(|p| is_tabs(p)) {
                    unsure = Some(n);
                }
                continue;
            }
        };
        let mut sel = Vec::new();
        selected(frame, &mut sel);
        let ours = match sel.as_slice() {
            [one] => tab.title.trim().is_empty() || words(one) == words(&tab.title),
            _ => false,
        };
        if !ours {
            other_tab += 1;
            continue;
        }
        found.push((frame, page));
    }
    match found.as_slice() {
        [(frame, page)] => Ok(Window {
            frame: (*frame).clone(),
            page: (*page).clone(),
        }),
        [] if unsure.is_some() => Err(format!(
            "Firefox's window shows {} tabs' pages at once and none as its selected one (a split \
             view), so Kai cannot tell that tab {}'s page is the one in front; try again",
            unsure.unwrap_or_default(),
            tab.id
        )),
        [] if other_tab > 0 => Err(format!(
            "a window of this Firefox shows tab {}'s address in front, but its selected tab is \
             not tab {} (another tab with that page, or a tab switch under way); Kai reads \
             Firefox's own UI only around the tab in front of its own window",
            tab.id, tab.id
        )),
        [] => Err(format!(
            "no window of this Firefox on the accessibility bus shows tab {}'s page in front{}; \
             {ON_THE_BUS}",
            tab.id,
            if tab.loading {
                " (the tab is still loading)"
            } else {
                ""
            }
        )),
        many => Err(format!(
            "{} windows of this Firefox show tab {}'s page in front, so Kai cannot tell which is \
             the tab's",
            many.len(),
            tab.id
        )),
    }
}

/// [`of`] on the windows of the app of `pid` as they are now.
pub async fn find(access: &dyn Accessibility, pid: u32, tab: &Tab) -> Result<Window, String> {
    let how = How {
        details: true,
        into_documents: false,
    };
    let tree = access.app_tree(pid, how).await.map_err(|e| {
        let e = e.to_string();
        if e.contains("not on the accessibility bus") {
            format!("{e}. {ON_THE_BUS}")
        } else {
            e
        }
    })?;
    of(&tree, tab)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str, children: Vec<Element>) -> Element {
        Element {
            role: role.into(),
            name: name.into(),
            showing: true,
            children,
            ..Default::default()
        }
    }

    fn page(url: &str, path: &str) -> Element {
        Element {
            document: Some(url.into()),
            at: Some((":1.1".into(), path.into())),
            ..el("document web", "", Vec::new())
        }
    }

    fn tab_el(name: &str, selected: bool) -> Element {
        let mut t = el("page tab", name, Vec::new());
        if selected {
            t.states = vec!["selected"];
        }
        t
    }

    /// A window with its tab strip and one tab panel per page; the first `deck` panel is the
    /// selected one (none where `deck` is false).
    fn window_deck(tabs: Vec<Element>, pages: Vec<Element>, deck: bool) -> Element {
        let panels = pages
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let mut panel = el("scroll pane", "", vec![el("internal frame", "", vec![p])]);
                panel.class = if deck && i == 0 {
                    "browserSidebarContainer deck-selected".into()
                } else {
                    "browserSidebarContainer".into()
                };
                panel
            })
            .collect();
        let mut panels = el("panel", "", panels);
        panels.id = "tabbrowser-tabpanels".into();
        el(
            "frame",
            "",
            vec![
                el(
                    "tool bar",
                    "Browser tabs",
                    vec![el("page tab list", "", tabs)],
                ),
                panels,
            ],
        )
    }

    fn window(tabs: Vec<Element>, pages: Vec<Element>) -> Element {
        window_deck(tabs, pages, false)
    }

    fn tab(url: &str, title: &str) -> Tab {
        Tab {
            id: 12,
            url: url.into(),
            title: title.into(),
            active: true,
            ..Default::default()
        }
    }

    fn tree(windows: Vec<Element>) -> Tree {
        Tree {
            root: el("application", "Firefox", windows),
            unanswered: 0,
        }
    }

    /// The tab's own window by its page in front and its selected tab; another window on the same
    /// page is not it; the moment after a switch, a page kept shown, a split view, an error page.
    #[test]
    fn the_tabs_window_is_the_one_whose_selected_tab_it_is() {
        let shop = "https://shop.example/cart";
        let t = tab(shop, "Checkout");
        let mine = window(
            vec![tab_el("Mail", false), tab_el("Checkout", true)],
            vec![page(&format!("{shop}#top"), "/mine")],
        );
        // A second window shows the same address with another tab in front.
        let twin = window(vec![tab_el("Cart copy", true)], vec![page(shop, "/twin")]);
        let w = of(&tree(vec![twin.clone(), mine.clone()]), &t).unwrap();
        assert_eq!(w.page.at.unwrap().1, "/mine");
        let e = of(&tree(vec![twin]), &t).unwrap_err();
        assert!(e.contains("its selected tab is not tab 12"), "{e}");
        // The moment after a switch, and a background error page Firefox keeps SHOWING: the
        // selected panel's page is the one in front.
        let switched = window_deck(
            vec![tab_el("Mail", true), tab_el("Checkout", false)],
            vec![
                page("https://mail.example/", "/mail"),
                page(shop, "/before"),
            ],
            true,
        );
        let e = of(&tree(vec![switched]), &t).unwrap_err();
        assert!(e.contains("no window of this Firefox"), "{e}");
        let stuck = window_deck(
            vec![
                tab_el("Checkout", true),
                tab_el("Problem loading page", false),
            ],
            vec![
                page(shop, "/cart"),
                page("http://127.0.0.1:1/refused", "/err"),
            ],
            true,
        );
        assert_eq!(
            of(&tree(vec![stuck]), &t).unwrap().page.at.unwrap().1,
            "/cart"
        );
        // Two pages shown and no panel selected (a split view): refused.
        let two = window(
            vec![tab_el("Checkout", true)],
            vec![page(shop, "/a"), page("https://mail.example/", "/b")],
        );
        let e = of(&tree(vec![two]), &t).unwrap_err();
        assert!(e.contains("shows 2 tabs' pages at once"), "{e}");
        // Two windows that both fit.
        let e = of(&tree(vec![mine.clone(), mine]), &t).unwrap_err();
        assert!(e.starts_with("2 windows"), "{e}");
        // An error page: Firefox gives the address that failed, or an about:neterror form.
        let failed = "http://127.0.0.1:1/refused";
        let t = tab(failed, "Problem loading page");
        for doc in [
            failed.to_owned(),
            format!(
                "about:neterror?e=connectionFailure&u={}",
                "http%3A//127.0.0.1%3A1/refused"
            ),
        ] {
            let w = window(
                vec![tab_el("Problem loading page", true)],
                vec![page(&doc, "/err")],
            );
            assert!(of(&tree(vec![w]), &t).is_ok(), "{doc}");
        }
        let e = of(&tree(vec![]), &t).unwrap_err();
        assert!(e.contains("no window of this Firefox"), "{e}");
    }
}
