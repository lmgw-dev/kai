//! The Accessibility capability (K23): `read_window` and `read_selection` (WP15) read a window's
//! accessible tree; WP16c's `ui_press` adds its method here. The freedesktop provider is AT-SPI
//! (`src/desktop/fdo/atspi.rs`).
//!
//! A window is found by what the Windows capability knows of it (its process and title), since
//! AT-SPI has no compositor window id on Wayland.

use super::Fut;

/// The window to read: from a Windows snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pid: u32,
    pub title: String,
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
}

/// A window's tree: its root element, and how many elements did not answer (said in the result,
/// never dropped silently).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tree {
    pub root: Element,
    pub unanswered: usize,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub role: String,
    pub name: String,
    /// Where the provider finds it again (AT-SPI: the application's bus name and the object's
    /// path); opaque to the tools.
    pub at: (String, String),
}

/// What a press did: the element's role and name, and the action that was run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pressed {
    pub role: String,
    pub name: String,
    pub action: String,
}

pub trait Accessibility: Send + Sync {
    /// The visible elements of the window `target`.
    fn tree<'a>(&'a self, target: &'a Target) -> Fut<'a, Tree>;

    /// The texts selected in the window `target`, in the tree's order.
    fn selection<'a>(&'a self, target: &'a Target) -> Fut<'a, Vec<String>>;

    /// The one visible, enabled, pressable element of the window `target` that `query` names
    /// (WP16c, `ui_press`). More than one, none or a disabled one is a refusal that says so.
    fn find<'a>(&'a self, target: &'a Target, query: &'a Query) -> Fut<'a, Found>;

    /// Runs the default action (click, press, toggle) of `found`, and of no other element: one that
    /// is gone, or is no longer that role and name, is a refusal and nothing is pressed.
    fn press<'a>(&'a self, found: &'a Found) -> Fut<'a, Pressed>;
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
