//! Accessibility through AT-SPI (K23's fdo provider): `read_window` and `read_selection` (WP15);
//! WP16c's `ui_press` builds on it ([`press`]).
//!
//! - **The bus:** the session bus's `org.a11y.Bus` gives the accessibility bus's address
//!   (`GetAddress`, which starts the bus where it is D-Bus activated); Kai connects to it per call.
//! - **The switch** ([`switch`], the owner's rule of 2026-10-09): Kai switches `org.a11y.Status`
//!   `IsEnabled` on while conversation mode is active and, in every other mode, around each read
//!   (`read_window`, `read_selection`, `ui_press`'s find and press), and puts back the value it had
//!   (`accessibility.switch`: `conversation`, `read` or `never`). `ScreenReaderEnabled` is never
//!   written: in testing on Plasma 6, Qt apps (Dolphin, plasmashell, LibreOffice) joined once
//!   `IsEnabled` was true; `ScreenReaderEnabled` exposed no further element and costs memory;
//!   Firefox registered without its windows even with both on, and
//!   Electron and Chromium apps stayed off the bus. A window whose app is not on the bus right
//!   after Kai switched the switch on is waited for: the registry's `ChildrenChanged` on its root
//!   announces each app that joins, and the read goes on at its app's, at most
//!   `accessibility.wait_ms` after the switch went on. An app still off the bus is an error that
//!   says the switch's state and, when it is off, names it precisely and why Kai did not switch it.
//! - **The window:** AT-SPI has no compositor window id on Wayland, so the window comes from the
//!   Windows capability's facts: the app is the one registered from the window's process
//!   (`GetConnectionUnixProcessID` of its bus name), and the window among its top-level children is
//!   the one named like the window's title (exactly, else the app's only one, else by a part of the
//!   title). Two windows of one title cannot be told apart: that is an error, never a guess.
//! - **The walk:** every visible element (state `VISIBLE`; hidden menus and pages are left out),
//!   each element's calls side by side, every call bounded by `dbus.timeout_s`. An element that does
//!   not answer is counted and said; nothing is cut. At most `accessibility.concurrency` calls are
//!   in flight at once in one walk (a throughput bound: a big tree takes longer, it is not
//!   shortened), so a window with thousands of elements cannot flood the bus or the target.
//! - **Not walked, and said:** an element that `MANAGES_DESCENDANTS` (a big table or list that
//!   creates its cells on demand; asking it for its children can freeze the app) is not descended
//!   into: its line carries its `ChildCount` and that it was not walked. A reference to an element
//!   of another application (an accessible may point anywhere on the bus) is not followed: one line
//!   says so. The address `org.a11y.Bus.GetAddress` gives is used only if it is a `unix:` address
//!   (path or abstract); any other transport (`unixexec:` starts a program) is an error.
//! - **Passwords:** nothing inside a `password text` element is read, itself or any element below
//!   it: no text, no selection, no value.
//! - **The selection:** the selected ranges of every visible text element of the window
//!   (`GetNSelections`, `GetSelection`, `GetText`), in the tree's order; the not-walked notes
//!   above are part of the answer.
//!
//! *Built against the documented interfaces (at-spi2-core's `xml/`) and fakes on private buses;
//! a real session's trees are checked by `spikes/p3/owner-checks.sh a` (WP13 (a)).*

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::join_all;
use kai_desktop::caps::accessibility::{
    Accessibility, Element, Found, Pressed, Query, Target, Tree,
};
use kai_desktop::caps::{Failure, Fut};
use tokio::sync::Semaphore;
use zbus::zvariant::OwnedObjectPath;

use crate::desktop::{call, property};
use crate::host::registry::SessionBus;

pub const A11Y_BUS: &str = "org.a11y.Bus";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT: &str = "org.a11y.atspi.Text";
const VALUE: &str = "org.a11y.atspi.Value";

/// AT-SPI's state bits (`AtspiStateType`).
mod state {
    pub const CHECKED: u32 = 4;
    pub const COLLAPSED: u32 = 5;
    pub const ENABLED: u32 = 8;
    pub const EXPANDABLE: u32 = 9;
    pub const EXPANDED: u32 = 10;
    pub const FOCUSED: u32 = 12;
    pub const PRESSED: u32 = 20;
    pub const SELECTED: u32 = 23;
    pub const VISIBLE: u32 = 30;
    pub const MANAGES_DESCENDANTS: u32 = 31;
}

/// AT-SPI's role names (`AtspiRole`, in its order; `atspi_role_get_name`'s words).
const ROLES: &[&str] = &[
    "invalid",
    "accelerator label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check box",
    "check menu item",
    "color chooser",
    "column header",
    "combo box",
    "date editor",
    "desktop icon",
    "desktop frame",
    "dial",
    "dialog",
    "directory pane",
    "drawing area",
    "file chooser",
    "filler",
    "focus traversable",
    "font chooser",
    "frame",
    "glass pane",
    "html container",
    "icon",
    "image",
    "internal frame",
    "label",
    "layered pane",
    "list",
    "list item",
    "menu",
    "menu bar",
    "menu item",
    "option pane",
    "page tab",
    "page tab list",
    "panel",
    "password text",
    "popup menu",
    "progress bar",
    "button",
    "radio button",
    "radio menu item",
    "root pane",
    "row header",
    "scroll bar",
    "scroll pane",
    "separator",
    "slider",
    "spin button",
    "split pane",
    "status bar",
    "table",
    "table cell",
    "table column header",
    "table row header",
    "tearoff menu item",
    "terminal",
    "text",
    "toggle button",
    "tool bar",
    "tool tip",
    "tree",
    "tree table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document frame",
    "heading",
    "page",
    "section",
    "redundant object",
    "form",
    "link",
    "input method window",
    "table row",
    "tree item",
    "document spreadsheet",
    "document presentation",
    "document text",
    "document web",
    "document email",
    "comment",
    "list box",
    "grouping",
    "image map",
    "notification",
    "info bar",
    "level bar",
    "title bar",
    "block quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math fraction",
    "math root",
    "subscript",
    "superscript",
    "description list",
    "description term",
    "description value",
    "footnote",
    "content deletion",
    "content insertion",
    "mark",
    "suggestion",
    "push button menu",
    "switch",
];

const PASSWORD_TEXT: u32 = 40;

/// The roles whose "disabled" is worth saying (a greyed-out control).
const CONTROL_ROLES: &[u32] = &[7, 8, 11, 35, 37, 43, 44, 45, 51, 52, 62, 79, 88, 130];

pub fn role_name(role: u32) -> String {
    ROLES
        .get(role as usize)
        .map(|r| (*r).to_owned())
        .unwrap_or_else(|| format!("role {role}"))
}

pub struct Atspi {
    pub bus: SessionBus,
    pub timeout: Duration,
    /// `accessibility.concurrency`: calls in flight at once in one walk (0: no bound).
    pub concurrency: usize,
    /// Kai's accessibility switch (`None`: Kai switches nothing, the providers' tests).
    pub switch: Option<switch::A11ySwitch>,
}

/// One accessible object: its bus name and path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Node {
    dest: String,
    path: String,
}

impl From<(String, OwnedObjectPath)> for Node {
    fn from((dest, path): (String, OwnedObjectPath)) -> Self {
        Self {
            dest,
            path: path.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Names, texts, values and states.
    Read,
    /// The selected texts only.
    Selection,
}

/// One walk's shared state: the elements seen (a tree that loops is walked once) and those that
/// did not answer.
struct Walk {
    seen: Mutex<HashSet<Node>>,
    unanswered: AtomicUsize,
    /// The window's app: only its nodes are walked.
    app: String,
    /// The calls in flight (module doc).
    permits: Semaphore,
}

impl Walk {
    fn new(app: String, concurrency: usize) -> Self {
        Self {
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
    async fn bounded<F: std::future::Future>(&self, f: F) -> F::Output {
        let _permit = self.permits.acquire().await.expect("never closed");
        f.await
    }
}

/// The note of an element the walk did not enter.
const FOREIGN: &str = "(element of another application, not read)";

/// What an app off the bus may need besides the switch (WP13 (a): Electron and Chromium were absent
/// from the bus with accessibility on).
const ELECTRON: &str =
    "Electron and Chromium apps join only when started with --force-renderer-accessibility";

mod press;
pub mod switch;

impl Accessibility for Atspi {
    fn tree<'a>(&'a self, target: &'a Target) -> Fut<'a, Tree> {
        Box::pin(self.held(async move {
            let bus = self.a11y_bus().await?;
            let window = self.window(&bus, target).await?;
            let walk = Walk::new(window.dest.clone(), self.concurrency);
            let root = self
                .element(&bus, window, true, false, Mode::Read, &walk)
                .await
                .ok_or_else(|| Failure::Failed("the window did not answer over AT-SPI".into()))?;
            Ok(Tree {
                root,
                unanswered: walk.unanswered.load(Ordering::Relaxed),
            })
        }))
    }

    fn find<'a>(&'a self, target: &'a Target, query: &'a Query) -> Fut<'a, Found> {
        Box::pin(self.held(self.find_element(target, query)))
    }

    fn press<'a>(&'a self, found: &'a Found) -> Fut<'a, Pressed> {
        Box::pin(self.held(self.press_found(found)))
    }

    fn selection<'a>(&'a self, target: &'a Target) -> Fut<'a, Vec<String>> {
        Box::pin(self.held(async move {
            let bus = self.a11y_bus().await?;
            let window = self.window(&bus, target).await?;
            let walk = Walk::new(window.dest.clone(), self.concurrency);
            let root = self
                .element(&bus, window, true, false, Mode::Selection, &walk)
                .await
                .ok_or_else(|| Failure::Failed("the window did not answer over AT-SPI".into()))?;
            // What was not walked is part of the answer, never silently missing.
            let mut out = Vec::new();
            collect_texts(&root, &mut out);
            Ok(out)
        }))
    }
}

fn collect_texts(e: &Element, out: &mut Vec<String>) {
    if !e.text.is_empty() {
        out.push(e.text.clone());
    }
    if !e.note.is_empty() {
        out.push(e.note.clone());
    }
    for c in &e.children {
        collect_texts(c, out);
    }
}

impl Atspi {
    /// `f` under a hold of the switch: on (by the rule) before `f` runs, put back after it.
    async fn held<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        let hold = match &self.switch {
            Some(s) => Some(s.hold().await),
            None => None,
        };
        let out = f.await;
        if let Some(h) = hold {
            h.release().await;
        }
        out
    }

    /// A connection to the accessibility bus.
    async fn a11y_bus(&self) -> Result<zbus::Connection, Failure> {
        let session = self.bus.get().await.map_err(Failure::Failed)?;
        let address: String = call(
            &session,
            A11Y_BUS,
            "/org/a11y/bus",
            A11Y_BUS,
            "GetAddress",
            &(),
            self.timeout,
        )
        .await?;
        check_address(&address).map_err(Failure::Failed)?;
        let connect = async {
            zbus::connection::Builder::address(address.as_str())?
                .build()
                .await
        };
        match tokio::time::timeout(self.timeout, connect).await {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(Failure::Failed(format!(
                "cannot reach the accessibility bus: {e}"
            ))),
            Err(_) => Err(Failure::Failed(format!(
                "the accessibility bus did not answer within {} s (dbus.timeout_s)",
                self.timeout.as_secs()
            ))),
        }
    }

    /// `org.a11y.Status`'s two switches and Kai's own part, for an error's words.
    async fn switches(&self) -> String {
        let Ok(session) = self.bus.get().await else {
            return String::new();
        };
        let get = |name: &'static str| {
            let session = session.clone();
            async move {
                property::<bool>(
                    &session,
                    A11Y_BUS,
                    "/org/a11y/bus",
                    switch::STATUS,
                    name,
                    self.timeout,
                )
                .await
                .ok()
            }
        };
        let words = self
            .switch
            .clone()
            .unwrap_or_else(switch::A11ySwitch::inert)
            .words();
        match (get("IsEnabled").await, get("ScreenReaderEnabled").await) {
            (Some(false), Some(false)) | (Some(false), None) => format!(
                "; accessibility is off: Qt apps join it once org.a11y.Status IsEnabled is true \
                 ({A11Y_BUS} at /org/a11y/bus on the session bus, kept as the GSettings key \
                 org.gnome.desktop.interface toolkit-accessibility; ScreenReaderEnabled is not \
                 needed), {}; {ELECTRON}",
                words.off()
            ),
            (Some(true), _) => match words.on() {
                Some(kais) => {
                    format!("; accessibility is on (org.a11y.Status IsEnabled; {kais}): {ELECTRON}")
                }
                None => format!("; accessibility is on (org.a11y.Status IsEnabled): {ELECTRON}"),
            },
            (_, Some(true)) => {
                format!("; accessibility is on (org.a11y.Status ScreenReaderEnabled): {ELECTRON}")
            }
            _ => format!("; {ELECTRON}"),
        }
    }

    /// The apps on the bus registered from process `pid`.
    async fn apps_of(&self, bus: &zbus::Connection, pid: u32) -> Result<Vec<Node>, Failure> {
        let root = Node {
            dest: REGISTRY.into(),
            path: ROOT.into(),
        };
        let apps = self.children(bus, &root).await?;
        let pids = join_all(apps.iter().map(|a| async move {
            call::<_, u32>(
                bus,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                &(a.dest.as_str(),),
                self.timeout,
            )
            .await
            .ok()
        }))
        .await;
        Ok(apps
            .into_iter()
            .zip(pids)
            .filter(|(_, p)| *p == Some(pid))
            .map(|(a, _)| a)
            .collect())
    }

    /// The registry's announcements of apps that join (`ChildrenChanged` on its root), from the
    /// registry's own connection; `None` where they cannot be followed (said in the log).
    async fn registry_events(&self, bus: &zbus::Connection) -> Option<zbus::MessageStream> {
        let made = tokio::time::timeout(self.timeout, async {
            let owner = zbus::fdo::DBusProxy::new(bus)
                .await?
                .get_name_owner(zbus::names::BusName::try_from(REGISTRY)?)
                .await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(owner.as_str())?
                .interface("org.a11y.atspi.Event.Object")?
                .member("ChildrenChanged")?
                .path(ROOT)?
                .build()
                .to_owned();
            zbus::MessageStream::for_match_rule(rule, bus, None).await
        })
        .await;
        match made {
            Ok(Ok(s)) => Some(s),
            Ok(Err(e)) => {
                tracing::warn!(
                    "read: cannot follow the registry's announcements ({e}); the read looks again \
                     at accessibility.wait_ms's end"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    "read: following the registry's announcements did not start within {} s \
                     (dbus.timeout_s); the read looks again at accessibility.wait_ms's end",
                    self.timeout.as_secs()
                );
                None
            }
        }
    }

    /// Kai switched accessibility on and the app of `pid` is not on the bus yet: its announcement
    /// is waited for, at most until `deadline` (`accessibility.wait_ms` after the switch went on).
    async fn wait_for_app(
        &self,
        bus: &zbus::Connection,
        pid: u32,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<Node>, Failure> {
        let began = tokio::time::Instant::now();
        let mut events = self.registry_events(bus).await;
        loop {
            let mine = self.apps_of(bus, pid).await?;
            if !mine.is_empty() {
                tracing::info!(
                    waited_ms = began.elapsed().as_millis() as u64,
                    "read: the window's app joined the accessibility bus after Kai switched it on"
                );
                return Ok(mine);
            }
            let next = async {
                match events.as_mut() {
                    Some(s) => s.next().await.map(|_| ()),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                got = next => if got.is_none() {
                    events = None;
                },
                () = tokio::time::sleep_until(deadline) => {
                    let mine = self.apps_of(bus, pid).await?;
                    if mine.is_empty() {
                        tracing::info!(
                            waited_ms = began.elapsed().as_millis() as u64,
                            "read: the window's app did not join the accessibility bus within \
                             accessibility.wait_ms"
                        );
                    }
                    return Ok(mine);
                }
            }
        }
    }

    async fn children(&self, bus: &zbus::Connection, n: &Node) -> Result<Vec<Node>, Failure> {
        let kids: Vec<(String, OwnedObjectPath)> = call(
            bus,
            &n.dest,
            &n.path,
            ACCESSIBLE,
            "GetChildren",
            &(),
            self.timeout,
        )
        .await?;
        Ok(kids.into_iter().map(Node::from).collect())
    }

    async fn name(&self, bus: &zbus::Connection, n: &Node) -> Result<String, Failure> {
        property(bus, &n.dest, &n.path, ACCESSIBLE, "Name", self.timeout).await
    }

    async fn states(&self, bus: &zbus::Connection, n: &Node) -> Result<u64, Failure> {
        let words: Vec<u32> = call(
            bus,
            &n.dest,
            &n.path,
            ACCESSIBLE,
            "GetState",
            &(),
            self.timeout,
        )
        .await?;
        Ok(words
            .iter()
            .take(2)
            .enumerate()
            .fold(0u64, |acc, (i, w)| acc | (u64::from(*w) << (32 * i))))
    }

    /// The window `target` among its app's top-level elements (module doc).
    async fn window(&self, bus: &zbus::Connection, target: &Target) -> Result<Node, Failure> {
        let mut found = self.apps_of(bus, target.pid).await?;
        if found.is_empty()
            && let Some(deadline) = self
                .switch
                .as_ref()
                .and_then(switch::A11ySwitch::registration_deadline)
        {
            found = self.wait_for_app(bus, target.pid, deadline).await?;
        }
        let mine: Vec<&Node> = found.iter().collect();
        if mine.is_empty() {
            if flatpak(Path::new("/proc"), target.pid) {
                return Err(Failure::Refused(
                    "the window's app runs in a Flatpak sandbox: its accessibility connection \
                     goes through xdg-dbus-proxy, which shows the proxy's process instead of \
                     the app's, so Kai cannot match the window to its app"
                        .into(),
                ));
            }
            return Err(Failure::Refused(format!(
                "the window's app is not on the accessibility bus{}",
                self.switches().await
            )));
        }
        let mut windows: Vec<(Node, String, u64)> = Vec::new();
        for app in mine {
            for w in self.children(bus, app).await? {
                let (name, states) = tokio::join!(self.name(bus, &w), self.states(bus, &w));
                if let (Ok(name), Ok(states)) = (name, states) {
                    windows.push((w, name, states));
                }
            }
        }
        pick(windows, &target.title)
    }

    /// One element and, unless it is hidden, its children (module doc). `in_password`: an
    /// ancestor is a password field, so nothing here is read.
    fn element<'a>(
        &'a self,
        bus: &'a zbus::Connection,
        node: Node,
        root: bool,
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
            if !root && !has(state::VISIBLE) {
                return None;
            }
            // Nothing in a password field's subtree is read.
            let password = in_password || role == PASSWORD_TEXT;
            let texts = ifaces.iter().any(|i| i == TEXT) && !password;
            let mut e = Element {
                role: role_name(role),
                ..Default::default()
            };
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
            e.children = join_all(
                kids.into_iter()
                    .map(|k| self.element(bus, k, false, password, mode, walk)),
            )
            .await
            .into_iter()
            .flatten()
            .collect();
            Some(e)
        })
    }

    async fn text(
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

    async fn selected(&self, bus: &zbus::Connection, n: &Node) -> Result<Vec<String>, Failure> {
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

/// The accessibility bus address `org.a11y.Bus.GetAddress` gave, if it is one Kai connects to: a
/// `unix:` address with a `path=` or `abstract=`. Other transports are refused (`unixexec:` runs
/// a program, `tcp:` leaves the machine), in any entry of a `;` list.
fn check_address(address: &str) -> Result<(), String> {
    let ok = !address.is_empty()
        && address.split(';').all(|entry| {
            entry.strip_prefix("unix:").is_some_and(|keys| {
                keys.split(',').any(|kv| {
                    kv.split_once('=')
                        .is_some_and(|(k, v)| matches!(k, "path" | "abstract") && !v.is_empty())
                })
            })
        });
    if ok {
        Ok(())
    } else {
        Err(format!(
            "the accessibility bus gave an address Kai does not connect to ({address:?}): only \
             unix: addresses with a path or abstract socket are used"
        ))
    }
}

/// The process `pid` runs in a Flatpak sandbox (`/.flatpak-info` in its root), where `proc` is
/// `/proc`. Unreadable: not known, so no.
pub(crate) fn flatpak(proc: &Path, pid: u32) -> bool {
    proc.join(pid.to_string())
        .join("root/.flatpak-info")
        .exists()
}

/// The window named `title` among an app's top-level elements (module doc).
fn pick(windows: Vec<(Node, String, u64)>, title: &str) -> Result<Node, Failure> {
    let exact: Vec<&(Node, String, u64)> = windows.iter().filter(|w| w.1 == title).collect();
    let chosen = match exact.len() {
        1 => Some(exact[0]),
        0 => {
            let visible: Vec<&(Node, String, u64)> = windows
                .iter()
                .filter(|w| w.2 & (1u64 << state::VISIBLE) != 0)
                .collect();
            if visible.len() == 1 {
                Some(visible[0])
            } else {
                let part: Vec<&(Node, String, u64)> = windows
                    .iter()
                    .filter(|w| !w.1.is_empty() && (title.contains(&w.1) || w.1.contains(title)))
                    .collect();
                (part.len() == 1).then(|| part[0])
            }
        }
        _ => {
            return Err(Failure::Refused(format!(
                "the app has {} windows of that title, which Kai cannot tell apart through \
                 accessibility",
                exact.len()
            )));
        }
    };
    match chosen {
        Some(w) => Ok(w.0.clone()),
        None => {
            let names: Vec<String> = windows
                .iter()
                .map(|w| crate::desktop::resolve::quoted(&w.1))
                .collect();
            Err(Failure::Refused(if names.is_empty() {
                "the window's app is on the accessibility bus without its windows (Firefox \
                 registers so even with accessibility on)"
                    .into()
            } else {
                format!(
                    "none of the app's windows on the accessibility bus is that window; they are: \
                     {}",
                    names.join(", ")
                )
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_named_as_at_spi_names_them() {
        assert_eq!(role_name(23), "frame");
        assert_eq!(role_name(43), "button");
        assert_eq!(role_name(79), "entry");
        assert_eq!(role_name(130), "switch");
        assert_eq!(role_name(131), "role 131");
        assert_eq!(ROLES[PASSWORD_TEXT as usize], "password text");
    }

    #[test]
    fn only_unix_socket_addresses_pass() {
        for ok in [
            "unix:path=/run/user/1000/at-spi/bus_0",
            "unix:abstract=/tmp/dbus-AbCd,guid=0123",
            "unix:guid=1,path=/run/a;unix:abstract=/x",
        ] {
            assert_eq!(check_address(ok), Ok(()), "{ok}");
        }
        for bad in [
            "",
            "unixexec:path=/bin/true",
            "tcp:host=localhost,port=1234",
            "autolaunch:",
            "unix:",
            "unix:path=",
            "unix:tmpdir=/tmp",
            "unix:path=/a;tcp:host=h,port=1",
        ] {
            assert!(check_address(bad).is_err(), "{bad}");
        }
    }

    fn w(path: &str, name: &str, visible: bool) -> (Node, String, u64) {
        (
            Node {
                dest: ":1.5".into(),
                path: path.into(),
            },
            name.into(),
            if visible { 1 << state::VISIBLE } else { 0 },
        )
    }

    #[test]
    fn the_window_is_picked_by_its_title_never_guessed() {
        let got = |ws, t| pick(ws, t).map(|n| n.path);
        assert_eq!(
            got(vec![w("/a", "Notes", true), w("/b", "Kai", true)], "Kai"),
            Ok("/b".into())
        );
        assert_eq!(
            got(vec![w("/a", "Notes", true), w("/b", "x", false)], "Other"),
            Ok("/a".into()),
            "the app's only visible window"
        );
        assert_eq!(
            got(
                vec![w("/a", "Notes", true), w("/b", "Draft", true)],
                "Draft — Editor"
            ),
            Ok("/b".into()),
            "a part of the title"
        );
        let e = got(vec![w("/a", "Kai", true), w("/b", "Kai", true)], "Kai").unwrap_err();
        assert!(e.to_string().contains("2 windows of that title"), "{e}");
        let e = got(vec![w("/a", "A", true), w("/b", "B", true)], "C").unwrap_err();
        assert_eq!(
            e.to_string(),
            "none of the app's windows on the accessibility bus is that window; they are: \"A\", \
             \"B\""
        );
        // An app registered without a tree (Firefox does this, WP13 (a)).
        let e = got(Vec::new(), "Mozilla Firefox").unwrap_err();
        assert_eq!(
            e.to_string(),
            "the window's app is on the accessibility bus without its windows (Firefox registers \
             so even with accessibility on)"
        );
    }
}
