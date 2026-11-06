//! `ui_press` (WP16c): runs the default action of one element of a window through AT-SPI's Action
//! interface, preferred over pixel input.
//!
//! - **Finding it:** the window's visible elements are walked as `read_window` walks them (same
//!   bound on calls in flight, same rules: an element that manages its own descendants is not
//!   descended into, and a miss says so); an element is a candidate when it has the Action
//!   interface with at least one action and a name. [`Query::select`] picks: the name as given (any
//!   case), else a fragment of it, narrowed by the role where the query names one. More than one
//!   is a refusal that lists them, one `role "name"` line each; none is a refusal that says what the
//!   walk could not see. Only pages shown are walked (`super`'s module doc: a background tab's page
//!   is not).
//! - **Actions not ready yet** (kai-firefox WP12, from WP0 (d2)): Firefox can answer a link's or a
//!   button's first `NActions` read with 0 and the same read about 100 ms later with 1. An element
//!   the query names whose first read said none is read again after `accessibility.action_settle_ms`
//!   (once per find, for all such elements together) before it is taken for one without an action;
//!   so is the element at the press, right before its action runs.
//! - **Before the confirmation:** the element is found ([`Atspi::find_element`]), so the dialog
//!   names it and an ambiguity is refused without asking anyone; after it, exactly that element is
//!   pressed ([`Atspi::press_found`]), once it still answers with the same role and name.
//! - **The documents:** the element carries the addresses of the documents it is in below the
//!   window, outermost first (`Found::documents`, read as the walk passes them); right before the
//!   press they are read again from the element up (`Parent`), and an element whose documents are
//!   no longer the same (a frame that went elsewhere), or whose page is no longer shown (another
//!   tab came to the front), is not pressed. Kai's browser site rules judge the press by them.
//! - **Refusing:** a control that is not enabled is not pressed.
//! - **The action:** the first of `click`, `press`, `activate`, `toggle`, `check`, `select`, `open`,
//!   `jump` the element offers (any case; toolkits name them differently), else its first action.
//! - **A press that opens a dialog** may not return until the dialog closes (a toolkit runs its
//!   handler inside the call). If the app has not answered after `dbus.timeout_s`, the press is
//!   reported as made and the result says the app did not answer.
//! - **Nothing is logged** of the names it matches.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use futures_util::future::{BoxFuture, join_all};
use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Candidate, Found, Match, Pressed, Query, Target};

use zbus::zvariant::OwnedObjectPath;

use super::walk::{Place, Walk};
use super::{ACCESSIBLE, Atspi, CONTROL_ROLES, DOCUMENT, Node, ROOT, role_name, state};
use crate::desktop::resolve::{quoted, shown_text};
use crate::desktop::{call, property};

pub(super) const ACTION: &str = "org.a11y.atspi.Action";

fn disabled(role: &str, name: &str) -> Failure {
    Failure::Refused(format!("{} {} is disabled", shown_text(role), quoted(name)))
}

/// The action names a press prefers, in order (lower case).
const PREFERRED: &[&str] = &[
    "click", "press", "activate", "toggle", "check", "select", "open", "jump",
];

/// One element that can be pressed.
struct Hit {
    node: Node,
    role: String,
    name: String,
    disabled: bool,
    /// The documents it is in (module doc).
    documents: Vec<String>,
    /// Its first `NActions` read said it has an action; `false`: it has the Action interface and
    /// said none, which may not be ready yet (module doc).
    ready: bool,
}

impl Atspi {
    /// The one element `query` names in the window `target` (module doc), before the confirmation.
    pub(super) async fn find_element(
        &self,
        target: &Target,
        query: &Query,
    ) -> Result<Found, Failure> {
        let bus = self.a11y_bus().await?;
        let window = self.window(&bus, target).await?;
        let walk = Walk::new(window.dest.clone(), self.concurrency);
        let not_walked = std::sync::atomic::AtomicUsize::new(0);
        let mut found = self
            .pressable(&bus, window, Place::Root, Vec::new(), &walk, &not_walked)
            .await;
        // Elements the query names whose actions were not ready are read again once (module doc).
        let named: Vec<usize> = match query.select(&candidates(&found)) {
            Match::One(i) => vec![i],
            Match::Many(v) => v,
            Match::None => Vec::new(),
        };
        let late: Vec<usize> = named.into_iter().filter(|i| !found[*i].ready).collect();
        let settled = !late.is_empty();
        if settled {
            tokio::time::sleep(self.action_settle).await;
            let again = join_all(late.iter().map(|i| self.actions(&bus, &found[*i].node))).await;
            for (i, n) in late.iter().zip(again) {
                found[*i].ready = matches!(n, Ok(n) if n > 0);
            }
        }
        found.retain(|h| h.ready);
        let chosen = match query.select(&candidates(&found)) {
            Match::One(i) => &found[i],
            Match::None => {
                let mut why = String::from(
                    "no pressable element of the window matches (read_window shows its controls)",
                );
                if settled {
                    why.push_str(&format!(
                        "; elements of that name had no action, also {} ms later \
                         (accessibility.action_settle_ms)",
                        self.action_settle.as_millis()
                    ));
                }
                let unanswered = walk.unanswered.load(Ordering::Relaxed);
                if unanswered > 0 {
                    why.push_str(&format!("; elements that did not answer: {unanswered}"));
                }
                let skipped = not_walked.load(Ordering::Relaxed);
                if skipped > 0 {
                    why.push_str(&format!(
                        "; elements that manage their own descendants were not looked into: \
                         {skipped}"
                    ));
                }
                return Err(Failure::Refused(why));
            }
            Match::Many(hits) => {
                let mut text = format!(
                    "{} elements match; name one with its role, as in button \"Name\":",
                    hits.len()
                );
                for i in hits {
                    text.push_str(&format!(
                        "\n{} {}",
                        shown_text(&found[i].role),
                        quoted(&found[i].name)
                    ));
                }
                return Err(Failure::Refused(text));
            }
        };
        if chosen.disabled {
            return Err(disabled(&chosen.role, &chosen.name));
        }
        Ok(Found {
            role: chosen.role.clone(),
            name: chosen.name.clone(),
            at: (chosen.node.dest.clone(), chosen.node.path.clone()),
            documents: chosen.documents.clone(),
        })
    }

    /// The documents `node` is in below its window, outermost first, read from it up (module
    /// doc): every element up to the window (the element whose parent is its app's root) that has
    /// the Document interface, `node` included; and whether the outermost one is shown (`true`
    /// where there is none).
    pub(super) async fn documents_of(
        &self,
        bus: &zbus::Connection,
        node: &Node,
    ) -> Result<(Vec<String>, bool), Failure> {
        let t = self.timeout;
        let mut chain = Vec::new();
        let mut outermost: Option<Node> = None;
        let mut seen = HashSet::new();
        let mut at = node.clone();
        loop {
            if !seen.insert(at.clone()) {
                return Err(Failure::Refused(
                    "the element's ancestors over AT-SPI loop, so Kai cannot tell which page it \
                     is in"
                        .into(),
                ));
            }
            let (dest, path): (String, OwnedObjectPath) =
                property(bus, &at.dest, &at.path, ACCESSIBLE, "Parent", t).await?;
            let parent = Node {
                dest,
                path: path.to_string(),
            };
            if parent.path == ROOT && parent.dest == at.dest {
                // `at` is the window.
                break;
            }
            if parent.dest != at.dest || parent.path == "/org/a11y/atspi/null" {
                return Err(Failure::Refused(
                    "the element is no longer below its window over AT-SPI, so Kai cannot tell \
                     which page it is in"
                        .into(),
                ));
            }
            let ifaces: Vec<String> =
                call(bus, &at.dest, &at.path, ACCESSIBLE, "GetInterfaces", &(), t).await?;
            if ifaces.iter().any(|i| i == DOCUMENT) {
                chain.push(self.doc_url(bus, &at).await);
                outermost = Some(at.clone());
            }
            at = parent;
        }
        chain.reverse();
        let shown = match outermost {
            None => true,
            Some(o) => self.states(bus, &o).await? & (1u64 << state::SHOWING) != 0,
        };
        Ok((chain, shown))
    }

    /// Presses the element [`Self::find_element`] found, after checking it is still there with the
    /// same role and name and still enabled: an application that rebuilt its window meanwhile may
    /// have given the path to another element, or none.
    pub(super) async fn press_found(&self, found: &Found) -> Result<Pressed, Failure> {
        let bus = self.a11y_bus().await?;
        let node = Node {
            dest: found.at.0.clone(),
            path: found.at.1.clone(),
        };
        let t = self.timeout;
        let (role, name, states) = tokio::join!(
            call::<_, u32>(&bus, &node.dest, &node.path, ACCESSIBLE, "GetRole", &(), t),
            self.name(&bus, &node),
            self.states(&bus, &node),
        );
        let gone = || {
            Failure::Refused(format!(
                "{} {} is no longer in the window (it changed after the confirmation); nothing \
                 was pressed",
                shown_text(&found.role),
                quoted(&found.name)
            ))
        };
        let (Ok(role), Ok(name), Ok(states)) = (role, name, states) else {
            return Err(gone());
        };
        if role_name(role) != found.role || name != found.name {
            return Err(gone());
        }
        if CONTROL_ROLES.contains(&role) && states & (1u64 << state::ENABLED) == 0 {
            return Err(disabled(&found.role, &found.name));
        }
        self.same_documents(&bus, &node, found).await?;
        let action = self.do_action(&bus, &node).await?;
        Ok(Pressed {
            role: found.role.clone(),
            name: found.name.clone(),
            action,
        })
    }

    /// `found`'s documents are still those it was found in, and its page is still shown (module
    /// doc); else a refusal, nothing pressed.
    pub(super) async fn same_documents(
        &self,
        bus: &zbus::Connection,
        node: &Node,
        found: &Found,
    ) -> Result<(), Failure> {
        let (documents, shown) = self.documents_of(bus, node).await?;
        if documents != found.documents {
            return Err(Failure::Refused(format!(
                "the page {} {} is in changed after the confirmation; nothing was pressed",
                shown_text(&found.role),
                quoted(&found.name)
            )));
        }
        if !shown {
            return Err(Failure::Refused(format!(
                "the page {} {} is in is no longer shown (another tab came to the front of its \
                 window); nothing was pressed",
                shown_text(&found.role),
                quoted(&found.name)
            )));
        }
        Ok(())
    }

    /// The pressable elements below `node`, in the tree's order, with the named elements whose
    /// actions may not be ready yet (`Hit::ready`); `documents`: those `node` is in (module doc).
    fn pressable<'a>(
        &'a self,
        bus: &'a zbus::Connection,
        node: Node,
        place: Place,
        mut documents: Vec<String>,
        walk: &'a Walk,
        not_walked: &'a std::sync::atomic::AtomicUsize,
    ) -> BoxFuture<'a, Vec<Hit>> {
        Box::pin(async move {
            if node.dest != walk.app
                || !walk
                    .seen
                    .lock()
                    .expect("never poisoned")
                    .insert(node.clone())
            {
                return Vec::new();
            }
            let t = self.timeout;
            let role = walk.bounded(call::<_, u32>(
                bus,
                &node.dest,
                &node.path,
                ACCESSIBLE,
                "GetRole",
                &(),
                t,
            ));
            let ifaces = walk.bounded(call::<_, Vec<String>>(
                bus,
                &node.dest,
                &node.path,
                ACCESSIBLE,
                "GetInterfaces",
                &(),
                t,
            ));
            let (role, name, states, ifaces) = tokio::join!(
                role,
                walk.bounded(self.name(bus, &node)),
                walk.bounded(self.states(bus, &node)),
                ifaces,
            );
            let (Ok(role), Ok(name), Ok(states), Ok(ifaces)) = (role, name, states, ifaces) else {
                walk.unanswered.fetch_add(1, Ordering::Relaxed);
                return Vec::new();
            };
            let has = |bit: u32| states & (1u64 << bit) != 0;
            if place != Place::Root && !has(state::VISIBLE) {
                return Vec::new();
            }
            let page = walk.page(place, ifaces.iter().any(|i| i == DOCUMENT));
            // A background tab's page is visible but not shown (`super`'s module doc).
            if page && place == Place::Ui && !has(state::SHOWING) {
                return Vec::new();
            }
            if page {
                documents.push(walk.bounded(self.doc_url(bus, &node)).await);
            }
            let mut out = Vec::new();
            if !name.is_empty() && ifaces.iter().any(|i| i == ACTION) {
                let n = walk.bounded(self.actions(bus, &node)).await;
                match n {
                    Ok(n) => out.push(Hit {
                        node: node.clone(),
                        role: role_name(role),
                        name,
                        disabled: CONTROL_ROLES.contains(&role) && !has(state::ENABLED),
                        documents: documents.clone(),
                        ready: n > 0,
                    }),
                    Err(_) => {
                        walk.unanswered.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            if has(state::MANAGES_DESCENDANTS) {
                not_walked.fetch_add(1, Ordering::Relaxed);
                return out;
            }
            let Ok(kids) = walk.bounded(self.children(bus, &node)).await else {
                walk.unanswered.fetch_add(1, Ordering::Relaxed);
                return out;
            };
            let place = walk.below(place, page);
            for below in join_all(
                kids.into_iter()
                    .map(|k| self.pressable(bus, k, place, documents.clone(), walk, not_walked)),
            )
            .await
            {
                out.extend(below);
            }
            out
        })
    }

    /// How many actions `node` has (`NActions`), read once.
    pub(super) async fn actions(
        &self,
        bus: &zbus::Connection,
        node: &Node,
    ) -> Result<i32, Failure> {
        property(
            bus,
            &node.dest,
            &node.path,
            ACTION,
            "NActions",
            self.timeout,
        )
        .await
    }

    /// [`Self::actions`], read again after `accessibility.action_settle_ms` where the first read
    /// said none (module doc).
    pub(super) async fn actions_settled(
        &self,
        bus: &zbus::Connection,
        node: &Node,
    ) -> Result<i32, Failure> {
        let n = self.actions(bus, node).await?;
        if n > 0 {
            return Ok(n);
        }
        tokio::time::sleep(self.action_settle).await;
        self.actions(bus, node).await
    }

    /// Runs the element's preferred action; the action's name (empty when the app calls it so). An
    /// element whose actions read none is read again after `accessibility.action_settle_ms`
    /// (module doc).
    pub(super) async fn do_action(
        &self,
        bus: &zbus::Connection,
        node: &Node,
    ) -> Result<String, Failure> {
        let t = self.timeout;
        let n = self.actions_settled(bus, node).await?;
        if n <= 0 {
            return Err(Failure::Refused(format!(
                "the element has no action to run, also {} ms later \
                 (accessibility.action_settle_ms); nothing was pressed",
                self.action_settle.as_millis()
            )));
        }
        let mut names = Vec::new();
        for i in 0..n.max(0) {
            let name: String = call(bus, &node.dest, &node.path, ACTION, "GetName", &(i,), t)
                .await
                .unwrap_or_default();
            names.push(name);
        }
        let index = PREFERRED
            .iter()
            .find_map(|p| names.iter().position(|n| n.to_lowercase() == *p))
            .unwrap_or(0);
        let action = names.get(index).cloned().unwrap_or_default();
        let body = (index as i32,);
        let sent = bus.call_method(
            Some(node.dest.as_str()),
            node.path.as_str(),
            Some(ACTION),
            "DoAction",
            &body,
        );
        match tokio::time::timeout(t, sent).await {
            Ok(Ok(reply)) => match reply.body().deserialize::<bool>() {
                Ok(true) => Ok(action),
                Ok(false) => Err(Failure::Failed(
                    "the element did not take the action".into(),
                )),
                Err(e) => Err(Failure::Failed(format!("the element answered oddly: {e}"))),
            },
            Ok(Err(e)) => Err(Failure::Failed(format!(
                "the element refused the action: {e}"
            ))),
            // The press was sent; a handler that opens a dialog and waits in it never returns.
            Err(_) => Ok(action),
        }
    }
}

/// The candidates a query picks among.
fn candidates(found: &[Hit]) -> Vec<Candidate> {
    found
        .iter()
        .map(|f| Candidate {
            role: f.role.clone(),
            name: f.name.clone(),
        })
        .collect()
}
