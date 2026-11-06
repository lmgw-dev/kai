//! Kai for Firefox's WP12 on AT-SPI: an app's whole tree with each element's details, an
//! element's subtree, an element found by its place or by a mark a page set, and a text field's
//! text replaced. Kai's browser module reads Firefox's own UI (its dialogs, prompts, bars and tool
//! bars) and Firefox's own pages through these, and presses a page's element the extension marked
//! (`crate::browser::tools::chrome`, `crate::browser::tools::trusted`).
//!
//! - **The app's tree** ([`Atspi::app_tree_of`]): every window of the app of a process, walked as
//!   a window is (`super`'s module doc: visible elements, documents only where shown), with the
//!   details [`How`] asks for (the `id` and `class` object attributes Firefox gives its elements,
//!   SHOWING, the element's place, whether it has an action).
//! - **A mark** ([`Atspi::find_marked_in`]): Firefox gives a page's unknown `aria-*` attribute as
//!   an object attribute without its `aria-` (kai-firefox WP12: `aria-kai-mark` is `kai-mark`, set
//!   and taken away within one 10 ms poll). The one document the caller names (the page its tab
//!   shows, found by the caller in that tab's window) is searched with the Collection interface,
//!   which reaches every frame, of any origin, below it; the answer says whether one, none or
//!   several elements carry the mark, and the one must have an action to run (read again after
//!   `accessibility.action_settle_ms`).
//! - **An element by its place** ([`Atspi::found_at_of`]): as it is now, refused where it is a
//!   disabled control, in a page that is not shown, or not shown itself (SHOWING: an element of
//!   Firefox's own UI has no page whose SHOWING would say it, a closed popup's).
//! - **A text** ([`Atspi::set_text_of`]): EditableText's `SetTextContents`, never into a password
//!   field, on the element as it was found (its role and name the same, its page still shown), and
//!   the field read again: a text it did not take is an error. Firefox 159 (headless, on a private
//!   bus) answered yes for its `prompt()` dialog's field and kept the field as it was.

use std::collections::HashMap;

use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Element, Found, How, Marked, Tree};
use zbus::zvariant::OwnedObjectPath;

use super::walk::{Mode, Place, Start, Walk};
use super::{ACCESSIBLE, Atspi, CONTROL_ROLES, Node, PASSWORD_TEXT, role_name, state};
use crate::desktop::call;
use crate::desktop::resolve::{quoted, shown_text};

const COLLECTION: &str = "org.a11y.atspi.Collection";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";

/// AT-SPI's `MATCH_ALL` (`AtspiCollectionMatchType`).
const MATCH_ALL: i32 = 1;
/// AT-SPI's `SORT_ORDER_CANONICAL` (`AtspiCollectionSortOrder`).
const CANONICAL: u32 = 1;

/// A Collection match rule (`(aiia{ss}iaiiasib)`): states, attributes, roles, interfaces, each
/// with its match type, and whether the rule is inverted.
type MatchRule = (
    Vec<i32>,
    i32,
    HashMap<String, String>,
    i32,
    Vec<i32>,
    i32,
    Vec<String>,
    i32,
    bool,
);

impl Atspi {
    /// [`kai_desktop::caps::accessibility::Accessibility::app_tree`].
    pub(super) async fn app_tree_of(&self, pid: u32, how: How) -> Result<Tree, Failure> {
        let bus = self.a11y_bus().await?;
        let apps = self.apps(&bus, pid).await?;
        let mut roots = Vec::new();
        let mut unanswered = 0;
        for app in apps {
            let walk = Walk::with(app.dest.clone(), self.concurrency, how, Start::App);
            if let Some(e) = self
                .element(&bus, app, Place::Root, false, Mode::Read, &walk)
                .await
            {
                roots.push(e);
            }
            unanswered += walk.unanswered.load(std::sync::atomic::Ordering::Relaxed);
        }
        let root = match roots.len() {
            1 => roots.remove(0),
            _ => Element {
                role: "application".into(),
                children: roots,
                ..Default::default()
            },
        };
        Ok(Tree { root, unanswered })
    }

    /// [`kai_desktop::caps::accessibility::Accessibility::subtree`].
    pub(super) async fn subtree_of(
        &self,
        at: &(String, String),
        how: How,
    ) -> Result<Tree, Failure> {
        let bus = self.a11y_bus().await?;
        let node = Node {
            dest: at.0.clone(),
            path: at.1.clone(),
        };
        let walk = Walk::with(node.dest.clone(), self.concurrency, how, Start::Element);
        let root = self
            .element(&bus, node, Place::Root, false, Mode::Read, &walk)
            .await
            .ok_or_else(|| Failure::Refused("the element is no longer there".into()))?;
        Ok(Tree {
            root,
            unanswered: walk.unanswered.load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// The element `node` as a [`Found`] now: its role, name and documents; a disabled control,
    /// or one in a page not shown, is a refusal.
    async fn found_now(&self, bus: &zbus::Connection, node: &Node) -> Result<Found, Failure> {
        let t = self.timeout;
        let (role, name, states) = tokio::join!(
            call::<_, u32>(bus, &node.dest, &node.path, ACCESSIBLE, "GetRole", &(), t),
            self.name(bus, node),
            self.states(bus, node),
        );
        let (Ok(role), Ok(name), Ok(states)) = (role, name, states) else {
            return Err(Failure::Refused("the element is no longer there".into()));
        };
        if CONTROL_ROLES.contains(&role) && states & (1u64 << state::ENABLED) == 0 {
            return Err(Failure::Refused(format!(
                "{} {} is disabled",
                shown_text(&role_name(role)),
                quoted(&name)
            )));
        }
        let (documents, shown) = self.documents_of(bus, node).await?;
        if !shown {
            return Err(Failure::Refused(format!(
                "{} {} is in a page that is not shown (a tab in the background)",
                shown_text(&role_name(role)),
                quoted(&name)
            )));
        }
        Ok(Found {
            role: role_name(role),
            name,
            at: (node.dest.clone(), node.path.clone()),
            documents,
        })
    }

    /// [`kai_desktop::caps::accessibility::Accessibility::found_at`] (module doc).
    pub(super) async fn found_at_of(&self, at: &(String, String)) -> Result<Found, Failure> {
        let bus = self.a11y_bus().await?;
        let node = Node {
            dest: at.0.clone(),
            path: at.1.clone(),
        };
        let found = self.found_now(&bus, &node).await?;
        let states = self
            .states(&bus, &node)
            .await
            .map_err(|_| Failure::Refused("the element is no longer there".into()))?;
        if states & (1u64 << state::SHOWING) == 0 {
            return Err(Failure::Refused(format!(
                "{} {} is not shown on the screen now",
                shown_text(&found.role),
                quoted(&found.name)
            )));
        }
        Ok(found)
    }

    /// [`kai_desktop::caps::accessibility::Accessibility::find_marked`] (module doc).
    pub(super) async fn find_marked_in(
        &self,
        page: &(String, String),
        name: &str,
        value: &str,
    ) -> Result<Marked, Failure> {
        let bus = self.a11y_bus().await?;
        let rule: MatchRule = (
            Vec::new(),
            MATCH_ALL,
            HashMap::from([(name.to_owned(), value.to_owned())]),
            MATCH_ALL,
            Vec::new(),
            MATCH_ALL,
            Vec::new(),
            MATCH_ALL,
            false,
        );
        let body = (rule, CANONICAL, 0i32, true);
        let got: Vec<(String, OwnedObjectPath)> = call(
            &bus,
            &page.0,
            &page.1,
            COLLECTION,
            "GetMatches",
            &body,
            self.timeout,
        )
        .await?;
        let mut hits: Vec<Node> = Vec::new();
        for n in got {
            let n = Node::from(n);
            if !hits.contains(&n) {
                hits.push(n);
            }
        }
        match hits.as_slice() {
            [] => Ok(Marked::Nothing),
            [one] => {
                let found = self.found_now(&bus, one).await?;
                // An element with no action to run (a media element, Firefox's) is not pressed
                // this way; its first read may say none too early (`super::press`).
                if self.actions_settled(&bus, one).await.unwrap_or(0) <= 0 {
                    return Err(Failure::Refused(format!(
                        "the marked element has no action to run, also {} ms later \
                         (accessibility.action_settle_ms)",
                        self.action_settle.as_millis()
                    )));
                }
                Ok(Marked::One(found))
            }
            many => Ok(Marked::Several(many.len())),
        }
    }

    /// [`kai_desktop::caps::accessibility::Accessibility::set_text`] (module doc).
    pub(super) async fn set_text_of(&self, found: &Found, text: &str) -> Result<(), Failure> {
        let bus = self.a11y_bus().await?;
        let node = Node {
            dest: found.at.0.clone(),
            path: found.at.1.clone(),
        };
        let t = self.timeout;
        let (role, name) = tokio::join!(
            call::<_, u32>(&bus, &node.dest, &node.path, ACCESSIBLE, "GetRole", &(), t),
            self.name(&bus, &node),
        );
        let (Ok(role), Ok(name)) = (role, name) else {
            return Err(Failure::Refused(
                "the text field is no longer there; nothing was typed".into(),
            ));
        };
        if role == PASSWORD_TEXT {
            return Err(Failure::Refused(
                "Kai never types into a password field".into(),
            ));
        }
        if role_name(role) != found.role || name != found.name {
            return Err(Failure::Refused(format!(
                "{} {} is no longer there; nothing was typed",
                shown_text(&found.role),
                quoted(&found.name)
            )));
        }
        self.same_documents(&bus, &node, found).await?;
        let done: bool = call(
            &bus,
            &node.dest,
            &node.path,
            EDITABLE_TEXT,
            "SetTextContents",
            &(text,),
            t,
        )
        .await?;
        // Firefox 159 answers yes and keeps its dialog's field as it was (headless, kai-firefox
        // WP12): the field is read again, so a text not taken is never taken for one that was.
        let now: Option<String> = if done {
            self.text(&bus, &node, 0, -1).await.ok()
        } else {
            None
        };
        if now.as_deref() == Some(text) {
            Ok(())
        } else {
            Err(Failure::Failed(format!(
                "{} {} did not take the text through accessibility{}",
                shown_text(&found.role),
                quoted(&found.name),
                if done {
                    " (it answered yes, and its text stayed as it was)"
                } else {
                    ""
                }
            )))
        }
    }
}
