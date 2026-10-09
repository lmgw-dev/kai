//! `ui_press` (WP16c): runs the default action of one element of a window through AT-SPI's Action
//! interface, preferred over pixel input.
//!
//! - **Finding it:** the window's visible elements are walked as `read_window` walks them (same
//!   bound on calls in flight, same rules: an element that manages its own descendants is not
//!   descended into, and a miss says so); an element is a candidate when it has the Action
//!   interface with at least one action and a name. [`Query::select`] picks: the name as given (any
//!   case), else a fragment of it, narrowed by the role where the query names one. More than one
//!   is a refusal that lists them, one `role "name"` line each; none is a refusal that says what the
//!   walk could not see.
//! - **Before the confirmation:** the element is found ([`Atspi::find_element`]), so the dialog
//!   names it and an ambiguity is refused without asking anyone; after it, exactly that element is
//!   pressed ([`Atspi::press_found`]), once it still answers with the same role and name.
//! - **Refusing:** a control that is not enabled is not pressed.
//! - **The action:** the first of `click`, `press`, `activate`, `toggle`, `check`, `select`, `open`,
//!   `jump` the element offers (any case; toolkits name them differently), else its first action.
//! - **A press that opens a dialog** may not return until the dialog closes (a toolkit runs its
//!   handler inside the call). If the app has not answered after `dbus.timeout_s`, the press is
//!   reported as made and the result says the app did not answer.
//! - **Nothing is logged** of the names it matches.

use std::sync::atomic::Ordering;

use futures_util::future::{BoxFuture, join_all};
use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Candidate, Found, Match, Pressed, Query, Target};

use super::{ACCESSIBLE, Atspi, CONTROL_ROLES, Node, Walk, role_name, state};
use crate::desktop::resolve::{quoted, shown_text};
use crate::desktop::{call, property};

const ACTION: &str = "org.a11y.atspi.Action";

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
        let found = self.pressable(&bus, window, true, &walk, &not_walked).await;
        let candidates: Vec<Candidate> = found
            .iter()
            .map(|f| Candidate {
                role: f.role.clone(),
                name: f.name.clone(),
            })
            .collect();
        let chosen = match query.select(&candidates) {
            Match::One(i) => &found[i],
            Match::None => {
                let mut why = String::from(
                    "no pressable element of the window matches (read_window shows its controls)",
                );
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
        })
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
        let action = self.do_action(&bus, &node).await?;
        Ok(Pressed {
            role: found.role.clone(),
            name: found.name.clone(),
            action,
        })
    }

    /// The pressable elements below `node`, in the tree's order.
    fn pressable<'a>(
        &'a self,
        bus: &'a zbus::Connection,
        node: Node,
        root: bool,
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
            if !root && !has(state::VISIBLE) {
                return Vec::new();
            }
            let mut out = Vec::new();
            if !name.is_empty() && ifaces.iter().any(|i| i == ACTION) {
                let n = walk
                    .bounded(property::<i32>(
                        bus, &node.dest, &node.path, ACTION, "NActions", t,
                    ))
                    .await;
                match n {
                    Ok(n) if n > 0 => out.push(Hit {
                        node: node.clone(),
                        role: role_name(role),
                        name,
                        disabled: CONTROL_ROLES.contains(&role) && !has(state::ENABLED),
                    }),
                    Ok(_) => {}
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
            for below in join_all(
                kids.into_iter()
                    .map(|k| self.pressable(bus, k, false, walk, not_walked)),
            )
            .await
            {
                out.extend(below);
            }
            out
        })
    }

    /// Runs the element's preferred action; the action's name (empty when the app calls it so).
    async fn do_action(&self, bus: &zbus::Connection, node: &Node) -> Result<String, Failure> {
        let t = self.timeout;
        let n: i32 = property(bus, &node.dest, &node.path, ACTION, "NActions", t).await?;
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
