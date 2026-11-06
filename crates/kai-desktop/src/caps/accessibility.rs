//! The Accessibility capability (K23): `read_window` and `read_selection` (WP15) read a window's
//! accessible tree; WP16c's `ui_press` adds its method here. The freedesktop provider is AT-SPI
//! (`src/desktop/fdo/atspi.rs`). Kai for Firefox's WP12 adds the reads and presses of a whole
//! app's windows with each element's details ([`How`], [`Accessibility::app_tree`],
//! [`Accessibility::subtree`], [`Accessibility::found_at`], [`Accessibility::set_text`]) and the
//! search for an element a page marked ([`Accessibility::find_marked`]): Kai's browser module
//! reads Firefox's own UI and presses a page's element through them.
//!
//! A window is found by what the Windows capability knows of it (its process and title), since
//! AT-SPI has no compositor window id on Wayland.
//!
//! **Documents** (a web page, a frame's page: AT-SPI's Document interface) are read only where the
//! window shows them: a document directly below the window's own UI (a tab's page, a page's
//! dialog) is walked only while it is shown on the screen (AT-SPI's SHOWING), so a background tab's
//! page, which Firefox keeps in the tree as visible, is never read or pressed in. A document below
//! another document (a frame of the page) is walked as the page is.

use super::Fut;

/// The window to read: from a Windows snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pid: u32,
    pub title: String,
    /// Only a top-level element named exactly `title` is the window: none of the fallbacks a
    /// provider otherwise takes (the app's only visible window, a part of the title). Kai's browser
    /// site rules ask it for Firefox's windows, so the window checked is the window read.
    pub exact: bool,
}

/// One element of a window's tree, as its toolkit describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// The role's name (`push button`, `label`, `entry`).
    pub role: String,
    pub name: String,
    /// What it holds as text (an entry's content, a paragraph), where it has text of its own.
    pub text: String,
    /// A slider's or a progress bar's value.
    pub value: Option<f64>,
    /// The states worth saying (`focused`, `checked`, `selected`, `expanded`, `disabled`).
    pub states: Vec<&'static str>,
    pub children: Vec<Element>,
    /// What the walk did not do here, in words, parenthesised: an element that manages its own
    /// descendants (said with its child count, not walked), or a reference into another
    /// application (an element with no role and only this note). Shown as the element's line.
    pub note: String,
    /// Where the element is a document below the window (a web page or a frame's page: AT-SPI's
    /// Document interface), the address it shows (`DocURL`), empty when it did not say (Firefox
    /// gives none for a `data:` or a `blob:` document, and none is given where the call failed);
    /// `None` for every other element. Kai's browser site rules judge the pages read by it, an
    /// empty address by the strictest rule.
    pub document: Option<String>,
    /// With [`How::details`]: the toolkit's own id for the element (AT-SPI's `id` object attribute:
    /// Firefox's element id), empty where it gives none.
    pub id: String,
    /// With [`How::details`]: the toolkit's class names (AT-SPI's `class` object attribute), empty
    /// where it gives none.
    pub class: String,
    /// With [`How::details`]: shown on the screen now (AT-SPI's SHOWING); a visible element may be
    /// scrolled away or in a background tab.
    pub showing: bool,
    /// With [`How::details`]: where the provider finds the element again ([`Found::at`]'s form),
    /// for [`Accessibility::found_at`] and [`Accessibility::subtree`].
    pub at: Option<(String, String)>,
    /// With [`How::details`]: the element has an action to run (a button, a link, a check box).
    pub pressable: bool,
}

/// How a provider walks ([`Accessibility::app_tree`], [`Accessibility::subtree`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct How {
    /// Each element's id, classes, SHOWING, handle and whether it has an action
    /// ([`Element::id`] and the fields after it).
    pub details: bool,
    /// Walk into the documents below the start: `false` gives each document as one element with
    /// its address and nothing below it (Firefox's own UI without its pages). Either way a
    /// document directly below a window is taken only while it is shown (the module doc).
    pub into_documents: bool,
}

/// A window's tree: its root element, and how many elements did not answer (said in the result,
/// never dropped silently).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tree {
    pub root: Element,
    pub unanswered: usize,
}

impl Tree {
    /// The texts and notes of its elements, in the tree's order (a selection's pieces, with what
    /// the walk did not do said where it happened).
    pub fn texts(&self) -> Vec<String> {
        fn collect(e: &Element, out: &mut Vec<String>) {
            if !e.text.is_empty() {
                out.push(e.text.clone());
            }
            if !e.note.is_empty() {
                out.push(e.note.clone());
            }
            for c in &e.children {
                collect(c, out);
            }
        }
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }
}

/// What `ui_press` was asked to press: an element's name, and optionally its role, as
/// `read_window` shows them. Written `Save` (any element of that name) or `button "Save"` (the
/// role, then the name in quotes; the quotes are JSON's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub role: Option<String>,
    pub name: String,
}

impl Query {
    pub fn parse(text: &str) -> Result<Query, String> {
        let t = text.trim();
        let unquote = |q: &str| -> Option<String> {
            let q = q.trim();
            if !(q.len() >= 2 && q.starts_with('"') && q.ends_with('"')) {
                return None;
            }
            serde_json::from_str::<String>(q)
                .ok()
                .or_else(|| Some(q[1..q.len() - 1].to_owned()))
        };
        let query = match t.find('"') {
            None => Query {
                role: None,
                name: t.to_owned(),
            },
            Some(at) => {
                let role = t[..at].trim();
                let name = unquote(&t[at..])
                    .ok_or_else(|| "element is a name, or a role and a quoted name".to_owned())?;
                Query {
                    role: (!role.is_empty()).then(|| role.to_lowercase()),
                    name,
                }
            }
        };
        if query.name.trim().is_empty() {
            return Err("element needs the name of what to press, as read_window shows it".into());
        }
        Ok(query)
    }
}

/// An element that can be pressed, as the walk found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub role: String,
    pub name: String,
}

/// The outcome of matching a [`Query`] against the pressable elements of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    One(usize),
    None,
    /// More than one, by index: the caller lists them.
    Many(Vec<usize>),
}

impl Query {
    /// The candidates the query names. In order, the first rule that matches anything decides: the
    /// name as it is (any case), then a fragment of the name; the role, where given, must be the
    /// candidate's whole role.
    pub fn select(&self, found: &[Candidate]) -> Match {
        let want = self.name.trim().to_lowercase();
        let in_role = |c: &Candidate| {
            self.role
                .as_ref()
                .is_none_or(|r| c.role.to_lowercase() == *r)
        };
        let rules: [&dyn Fn(&str) -> bool; 2] = [&|n| n == want, &|n| n.contains(&want)];
        for rule in rules {
            let hits: Vec<usize> = found
                .iter()
                .enumerate()
                .filter(|(_, c)| in_role(c) && rule(&c.name.to_lowercase()))
                .map(|(i, _)| i)
                .collect();
            match hits.len() {
                0 => {}
                1 => return Match::One(hits[0]),
                _ => return Match::Many(hits),
            }
        }
        Match::None
    }
}

/// The one element a [`Query`] named, found before the owner confirms the press (WP16c): the
/// confirmation shows its role and name, and [`Accessibility::press`] presses this element and no
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    pub role: String,
    pub name: String,
    /// Where the provider finds it again (AT-SPI: the application's bus name and the object's
    /// path); opaque to the tools.
    pub at: (String, String),
    /// The documents it is in below the window, outermost first ([`Element::document`]'s
    /// addresses; the element itself where it is one): empty for an element of the app's own
    /// window outside any page. [`Accessibility::press`] presses only while they are still these.
    pub documents: Vec<String>,
}

/// What [`Accessibility::find_marked`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marked {
    /// Exactly one element carries the mark.
    One(Found),
    /// None does.
    Nothing,
    /// This many do: the page copied the mark (Kai takes that for tampering).
    Several(usize),
}

/// What a press did: the element's role and name, and the action that was run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pressed {
    pub role: String,
    pub name: String,
    pub action: String,
}

/// The failure of a method a provider does not offer.
fn not_offered<T>(what: &str) -> Result<T, super::Failure> {
    Err(super::Failure::Failed(format!(
        "this desktop's accessibility does not offer {what}"
    )))
}

pub trait Accessibility: Send + Sync {
    /// The visible elements of the window `target`.
    fn tree<'a>(&'a self, target: &'a Target) -> Fut<'a, Tree>;

    /// The texts selected in the window `target`: a tree whose elements carry only their
    /// selected text (and their notes, and the documents' addresses); [`Tree::texts`] reads it in
    /// the tree's order.
    fn selection<'a>(&'a self, target: &'a Target) -> Fut<'a, Tree>;

    /// The one visible, enabled, pressable element of the window `target` that `query` names
    /// (WP16c, `ui_press`). More than one, none or a disabled one is a refusal that says so.
    fn find<'a>(&'a self, target: &'a Target, query: &'a Query) -> Fut<'a, Found>;

    /// Runs the default action (click, press, toggle) of `found`, and of no other element: one that
    /// is gone, is no longer that role and name, or is no longer in the same documents (a page no
    /// longer shown, since another tab came to the front, included), is a refusal and nothing is
    /// pressed.
    fn press<'a>(&'a self, found: &'a Found) -> Fut<'a, Pressed>;

    /// Every window of the app registered from process `pid`, walked as [`Accessibility::tree`]
    /// walks one, under `how`: the root is the application, its children are its windows.
    fn app_tree<'a>(&'a self, pid: u32, how: How) -> Fut<'a, Tree> {
        let _ = (pid, how);
        Box::pin(async { not_offered("a whole app's tree") })
    }

    /// The element `at` (an [`Element::at`]) and what is below it, under `how`; a document there is
    /// read whether it is shown or not (the caller picked it).
    fn subtree<'a>(&'a self, at: &'a (String, String), how: How) -> Fut<'a, Tree> {
        let _ = (at, how);
        Box::pin(async { not_offered("an element's tree") })
    }

    /// The element `at` as it is now, as [`Accessibility::find`] gives one, for a press of an
    /// element a tree named; a disabled control, and one not shown on the screen now (SHOWING:
    /// a closed popup's, a background tab's), is a refusal.
    fn found_at<'a>(&'a self, at: &'a (String, String)) -> Fut<'a, Found> {
        let _ = at;
        Box::pin(async { not_offered("pressing an element by its place") })
    }

    /// The elements inside the document `page` (an [`Element::at`] of a shown document: a tab's
    /// page, searched with every frame below it) whose object attribute `name` is `value` (a mark
    /// Kai for Firefox's `element.mark@1` set on one element): [`Marked`] says whether one, none
    /// or several carry it. The one element must have an action to run, else it is a refusal.
    fn find_marked<'a>(
        &'a self,
        page: &'a (String, String),
        name: &'a str,
        value: &'a str,
    ) -> Fut<'a, Marked> {
        let _ = (page, name, value);
        Box::pin(async { not_offered("a search by an element's attribute") })
    }

    /// Replaces the text of the editable element `found` (a dialog's text field) with `text`. A
    /// password field is refused.
    fn set_text<'a>(&'a self, found: &'a Found, text: &'a str) -> Fut<'a, ()> {
        let _ = (found, text);
        Box::pin(async { not_offered("typing into an element") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(role: &str, name: &str) -> Candidate {
        Candidate {
            role: role.into(),
            name: name.into(),
        }
    }

    #[test]
    fn a_query_is_a_name_or_a_role_and_a_quoted_name() {
        let q = |s| Query::parse(s).unwrap();
        assert_eq!(
            q("  Save as "),
            Query {
                role: None,
                name: "Save as".into()
            }
        );
        assert_eq!(
            q("Button \"Save\""),
            Query {
                role: Some("button".into()),
                name: "Save".into()
            }
        );
        assert_eq!(q("\"a \\\" b\"").name, "a \" b");
        for bad in ["", "  ", "button \"\"", "button \"x", "\"x\" y"] {
            assert!(Query::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_first_rule_that_matches_decides() {
        let found = [
            c("button", "Save"),
            c("button", "Save as…"),
            c("menu item", "Save"),
            c("check box", "Autosave"),
        ];
        let sel = |s: &str| Query::parse(s).unwrap().select(&found);
        assert_eq!(sel("button \"save\""), Match::One(0));
        assert_eq!(sel("Save"), Match::Many(vec![0, 2]), "same name, two roles");
        assert_eq!(sel("ave"), Match::Many(vec![0, 1, 2, 3]), "fragments");
        assert_eq!(sel("auto"), Match::One(3));
        assert_eq!(sel("save as"), Match::One(1));
        assert_eq!(sel("menu item \"Save\""), Match::One(2));
        assert_eq!(sel("nothing"), Match::None);
        assert_eq!(
            sel("button \"Autosave\""),
            Match::None,
            "the role is the whole role"
        );
    }
}
