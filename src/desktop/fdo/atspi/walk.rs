//! The walk of an accessible tree (the parent's module doc: visible elements, documents only
//! where shown, the calls bounded, what was not walked said), shared by the reads
//! (`read_window`, `read_selection`, an app's whole tree, an element's subtree) and
//! `ui_press`'s find (`super::press`).

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::future::join_all;
use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Element, How};
use tokio::sync::Semaphore;

use super::press::ACTION;
use super::{
    ACCESSIBLE, Atspi, CONTROL_ROLES, DOCUMENT, Node, PASSWORD_TEXT, TEXT, VALUE, role_name, state,
};
use crate::desktop::{call, property};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Names, texts, values and states.
    Read,
    /// The selected texts only.
    Selection,
}

/// One walk's shared state: the elements seen (a tree that loops is walked once) and those that
/// did not answer.
pub(super) struct Walk {
    pub(super) seen: Mutex<HashSet<Node>>,
    pub(super) unanswered: AtomicUsize,
    /// The window's app: only its nodes are walked.
    pub(super) app: String,
    /// The calls in flight (module doc).
    pub(super) permits: Semaphore,
    /// What is read of each element, and whether documents are walked into.
    pub(super) how: How,
    /// What the walk starts at.
    pub(super) start: Start,
}

/// What a walk starts at: a window (its own Document, Firefox's chrome, is no page), an app (its
/// children are its windows), or an element a caller picked (a document there is read whether
/// shown or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Start {
    Window,
    App,
    Element,
}

/// Where an element is, for the documents rule (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    /// The walk's start.
    Root,
    /// One of an app's windows.
    Window,
    /// Below a window, outside any document.
    Ui,
    /// Inside a document.
    Page,
}

impl Walk {
    /// A window's walk: into its documents, without details.
    pub(super) fn new(app: String, concurrency: usize) -> Self {
        let how = How {
            details: false,
            into_documents: true,
        };
        Self::with(app, concurrency, how, Start::Window)
    }

    /// Where the children of an element at `place` are; `page`: the element is a document of a
    /// page ([`Walk::page`]).
    pub(super) fn below(&self, place: Place, page: bool) -> Place {
        match place {
            Place::Root if self.start == Start::App => Place::Window,
            Place::Page => Place::Page,
            _ if page => Place::Page,
            _ => Place::Ui,
        }
    }

    /// An element at `place` with the Document interface is a page's document (module doc): not
    /// a window's own (Firefox's chrome is a Document too), unless the walk started at it.
    pub(super) fn page(&self, place: Place, document: bool) -> bool {
        document
            && match place {
                Place::Root => self.start == Start::Element,
                Place::Window => false,
                Place::Ui | Place::Page => true,
            }
    }

    pub(super) fn with(app: String, concurrency: usize, how: How, start: Start) -> Self {
        Self {
            how,
            start,
            seen: Mutex::default(),
            unanswered: AtomicUsize::new(0),
            app,
            permits: Semaphore::new(if concurrency == 0 {
                Semaphore::MAX_PERMITS
            } else {
                concurrency.min(Semaphore::MAX_PERMITS)
            }),
        }
    }

    /// `f` once a permit is free; the permit is held for that one call only (never across the
    /// recursion into children).
    pub(super) async fn bounded<F: std::future::Future>(&self, f: F) -> F::Output {
        let _permit = self.permits.acquire().await.expect("never closed");
        f.await
    }
}

/// The note of an element the walk did not enter.
const FOREIGN: &str = "(element of another application, not read)";

impl Atspi {
    /// One element and, unless it is hidden, its children (module doc). `in_password`: an
    /// ancestor is a password field, so nothing here is read.
    pub(super) fn element<'a>(
        &'a self,
        bus: &'a zbus::Connection,
        node: Node,
        place: Place,
        in_password: bool,
        mode: Mode,
        walk: &'a Walk,
    ) -> futures_util::future::BoxFuture<'a, Option<Element>> {
        Box::pin(async move {
            if node.dest != walk.app {
                return Some(Element {
                    note: FOREIGN.into(),
                    ..Default::default()
                });
            }
            if !walk
                .seen
                .lock()
                .expect("never poisoned")
                .insert(node.clone())
            {
                return None;
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
                return None;
            };
            let has = |bit: u32| states & (1u64 << bit) != 0;
            if place != Place::Root && !has(state::VISIBLE) {
                return None;
            }
            let page = walk.page(place, ifaces.iter().any(|i| i == DOCUMENT));
            // A tab's page in the background is visible but not shown (module doc).
            if page && place == Place::Ui && !has(state::SHOWING) {
                return None;
            }
            // Nothing in a password field's subtree is read.
            let password = in_password || role == PASSWORD_TEXT;
            let texts = ifaces.iter().any(|i| i == TEXT) && !password;
            let mut e = Element {
                role: role_name(role),
                ..Default::default()
            };
            if page {
                e.document = Some(walk.bounded(self.doc_url(bus, &node)).await);
            }
            if walk.how.details {
                self.details(bus, &node, &ifaces, has(state::SHOWING), walk, &mut e)
                    .await;
            }
            match mode {
                Mode::Read => {
                    e.name = name;
                    if texts {
                        match walk.bounded(self.text(bus, &node, 0, -1)).await {
                            Ok(t) => e.text = t,
                            Err(_) => {
                                walk.unanswered.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    if !password && ifaces.iter().any(|i| i == VALUE) {
                        e.value = walk
                            .bounded(property(
                                bus,
                                &node.dest,
                                &node.path,
                                VALUE,
                                "CurrentValue",
                                t,
                            ))
                            .await
                            .ok();
                    }
                    for (bit, word) in [
                        (state::FOCUSED, "focused"),
                        (state::CHECKED, "checked"),
                        (state::SELECTED, "selected"),
                        (state::PRESSED, "pressed"),
                    ] {
                        if has(bit) {
                            e.states.push(word);
                        }
                    }
                    if has(state::EXPANDABLE) {
                        if has(state::EXPANDED) {
                            e.states.push("expanded");
                        } else if has(state::COLLAPSED) {
                            e.states.push("collapsed");
                        }
                    }
                    if CONTROL_ROLES.contains(&role) && !has(state::ENABLED) {
                        e.states.push("disabled");
                    }
                }
                Mode::Selection => {
                    if texts {
                        match walk.bounded(self.selected(bus, &node)).await {
                            Ok(s) => e.text = s.join("\n"),
                            Err(_) => {
                                walk.unanswered.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
            if page && place != Place::Root && !walk.how.into_documents {
                return Some(e);
            }
            if has(state::MANAGES_DESCENDANTS) {
                // Asking such an element for its children can make the app build them all.
                let count = walk
                    .bounded(property::<i32>(
                        bus,
                        &node.dest,
                        &node.path,
                        ACCESSIBLE,
                        "ChildCount",
                        t,
                    ))
                    .await;
                e.note = match count {
                    Ok(n) => format!(
                        "({n} children, not walked: the element manages its descendants itself)"
                    ),
                    Err(_) => "(children not walked: the element manages its descendants \
                               itself, and its child count did not answer)"
                        .into(),
                };
                return Some(e);
            }
            let Ok(kids) = walk.bounded(self.children(bus, &node)).await else {
                walk.unanswered.fetch_add(1, Ordering::Relaxed);
                return Some(e);
            };
            let below = walk.below(place, page);
            e.children = join_all(
                kids.into_iter()
                    .map(|k| self.element(bus, k, below, password, mode, walk)),
            )
            .await
            .into_iter()
            .flatten()
            .collect();
            Some(e)
        })
    }

    /// [`How::details`]' fields of `e` (the element `node`, whose interfaces are `ifaces`).
    pub(super) async fn details(
        &self,
        bus: &zbus::Connection,
        node: &Node,
        ifaces: &[String],
        showing: bool,
        walk: &Walk,
        e: &mut Element,
    ) {
        let t = self.timeout;
        e.showing = showing;
        e.at = Some((node.dest.clone(), node.path.clone()));
        let attrs = walk.bounded(call::<_, std::collections::HashMap<String, String>>(
            bus,
            &node.dest,
            &node.path,
            ACCESSIBLE,
            "GetAttributes",
            &(),
            t,
        ));
        let actions = async {
            if ifaces.iter().any(|i| i == ACTION) {
                walk.bounded(self.actions(bus, node)).await.unwrap_or(0)
            } else {
                0
            }
        };
        let (attrs, actions) = tokio::join!(attrs, actions);
        if let Ok(mut a) = attrs {
            e.id = a.remove("id").unwrap_or_default();
            e.class = a.remove("class").unwrap_or_default();
        }
        e.pressable = actions > 0;
    }

    pub(super) async fn text(
        &self,
        bus: &zbus::Connection,
        n: &Node,
        start: i32,
        end: i32,
    ) -> Result<String, Failure> {
        call(
            bus,
            &n.dest,
            &n.path,
            TEXT,
            "GetText",
            &(start, end),
            self.timeout,
        )
        .await
    }

    pub(super) async fn selected(
        &self,
        bus: &zbus::Connection,
        n: &Node,
    ) -> Result<Vec<String>, Failure> {
        let count: i32 = call(
            bus,
            &n.dest,
            &n.path,
            TEXT,
            "GetNSelections",
            &(),
            self.timeout,
        )
        .await?;
        let mut out = Vec::new();
        for i in 0..count.max(0) {
            let (start, end): (i32, i32) = call(
                bus,
                &n.dest,
                &n.path,
                TEXT,
                "GetSelection",
                &(i,),
                self.timeout,
            )
            .await?;
            if end > start {
                out.push(self.text(bus, n, start, end).await?);
            }
        }
        Ok(out)
    }
}
