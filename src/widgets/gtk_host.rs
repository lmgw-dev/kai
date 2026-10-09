//! The widgets on the GTK thread: [`WidgetsCmd`]s from the tools (through `UiCmd::Widgets`), the
//! tray and the `hide-overlays` shortcut ([`super::hide`]), carried out on [`Table`]'s bookkeeping.
//!
//! A show goes: the content filter compiled (once per run) → the view created hidden, its output
//! resolved meanwhile (the ring's resolution: `overlay.output`, else KWin's active output) → the
//! host page ready → told the content at its final width → the text's height measured, or the
//! caller's frame loaded → the surface sized within the output (less the margins) and mapped →
//! the id to the caller, the expiry armed. A failure on the way answers the call with why, and so
//! does `overlay.show_timeout_s` passing first. A widget whose web process ends, or whose surface
//! the compositor closes (its output went away), is hidden and logged, and `overlay_list` says so
//! once; the ring's own process is another one (K1). A web process WebKit finds unresponsive (a
//! script that never yields) is ended, and with it every widget in it. The last widget hidden drops
//! the widgets' web context, and with it their web process.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gtk::gdk::prelude::*;
use gtk::{gdk, glib};
use tokio::sync::oneshot;

use super::filter::{self, Filter};
use super::kinds::{Body, Kind};
use super::sandbox::{self, PageMsg, Refused};
use super::table::{Clock, Count, Table, ids};
use super::view::{Frame, Shared, WidgetView};
use crate::overlay::placement::Anchor;
use crate::overlay::{Resolved, resolve_output};

/// How a show looks, from `[overlay.<kind>]` and the call's `position`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub position: Anchor,
    pub margin: u32,
    pub width: u32,
    /// 0: the content's height ([`Kind::measures`]).
    pub height: u32,
    pub timeout: Option<Duration>,
    pub interactive: bool,
    /// `overlay.output`.
    pub output: Option<String>,
    /// `overlay.show_timeout_s`: how long the show may take from the call to the map.
    pub show_timeout: Duration,
    /// `overlay.unresponsive_s`: how long the widgets' web process may leave Kai's ping unanswered.
    pub unresponsive: Duration,
}

/// The answer to a show: the id, and what the caller should know (a fitted size, scrolling text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub id: u64,
    pub notes: Vec<String>,
}

pub struct ShowRequest {
    pub kind: Kind,
    pub body: Body,
    pub look: Look,
    pub reply: oneshot::Sender<Result<Shown, String>>,
}

pub enum WidgetsCmd {
    Show(ShowRequest),
    /// `overlay_hide`; the tray's and the shortcut's hides have no reply.
    Hide {
        id: Option<u64>,
        reply: Option<oneshot::Sender<Result<Vec<u64>, String>>>,
    },
    List {
        reply: oneshot::Sender<Vec<String>>,
    },
    /// A Kai confirmation dialog opened (`true`) or the last one closed (`false`)
    /// ([`super::DialogOpen`]): every MCP Apps view click-through and only Kai's strip meanwhile
    /// (§10.1).
    Guard(bool),
    /// The screen locked (`true`) or unlocked (`crate::screen_lock`): every shown widget's surface
    /// is unmapped while it is locked (nothing of Kai's on the lock screen, nothing to click),
    /// and mapped again after; a show still on its way fails.
    Locked(bool),
    /// The live check's window into the widgets.
    #[cfg(test)]
    Probe(async_channel::Sender<Probe>),
    /// The live check's crash of the widgets' web process (through widget `id`'s view).
    #[cfg(test)]
    Terminate(u64),
    /// The live check's look into widget `id`'s page from the main world.
    #[cfg(test)]
    EvalMain {
        id: u64,
        js: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

impl std::fmt::Debug for WidgetsCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the content.
        match self {
            WidgetsCmd::Show(r) => write!(f, "Show({:?})", r.kind),
            WidgetsCmd::Hide { id, .. } => write!(f, "Hide({id:?})"),
            WidgetsCmd::List { .. } => write!(f, "List"),
            WidgetsCmd::Guard(on) => write!(f, "Guard({on})"),
            WidgetsCmd::Locked(on) => write!(f, "Locked({on})"),
            #[cfg(test)]
            _ => write!(f, "(a live-check command)"),
        }
    }
}

/// What the live check sees.
#[cfg(test)]
#[derive(Debug, Clone)]
pub enum Probe {
    Created(u64),
    Page(u64, PageMsg),
    Mapped {
        id: u64,
        output: Option<String>,
        width: u32,
        height: u32,
    },
    Destroyed(u64),
    /// The widgets' web context was dropped (no widget left).
    ContextGone,
    Refused {
        id: u64,
        uri: String,
        refused: Refused,
    },
    Crashed(u64),
    /// WebKit found the widgets' web process unresponsive (through widget `id`'s view).
    Unresponsive(u64),
    /// MCP Apps view `id` went click-through and shrank to Kai's strip (`true`) or back (`false`).
    Guarded {
        id: u64,
        on: bool,
    },
    /// Widget `id`'s surface was unmapped for the screen lock (`true`) or mapped again.
    LockHidden {
        id: u64,
        hidden: bool,
    },
}

pub enum Event {
    Cmd(WidgetsCmd),
    Page {
        id: u64,
        msg: PageMsg,
    },
    /// A navigation of widget `id`'s view was refused. `uri` is the caller's content: never
    /// logged, only the live check's probe sees it.
    Refused {
        id: u64,
        uri: String,
        refused: Refused,
    },
    Crashed {
        id: u64,
        reason: String,
    },
    /// WebKit says widget `id`'s web process does not respond.
    Unresponsive {
        id: u64,
    },
    /// Time to ping every widget view (`overlay.unresponsive_s`).
    Ping,
    /// Widget `id` asked for a permission, denied.
    Permission {
        id: u64,
    },
    /// Widget `id`'s bridge answers its view (K18): a JSON-RPC message for `__kaiWidget.post`.
    AppPost {
        id: u64,
        message: String,
    },
    /// The compositor closed widget `id`'s surface (its output went away).
    Closed {
        id: u64,
    },
    Output {
        id: u64,
        resolved: Resolved,
    },
    ShowTimeout {
        id: u64,
    },
    Expire,
    Filter(Result<Filter, String>),
    Quit,
}

pub struct WidgetsHandle {
    tx: async_channel::Sender<Event>,
    task: glib::JoinHandle<()>,
}

impl WidgetsHandle {
    pub fn send(&self, cmd: WidgetsCmd) {
        let _ = self.tx.try_send(Event::Cmd(cmd));
    }

    /// §2's quit step 4: every widget goes. Returns once they are destroyed.
    pub async fn quit(self) {
        super::remote::clear();
        let _ = self.tx.send(Event::Quit).await;
        let _ = self.task.await;
    }
}

/// Starts the widgets on the GTK thread (after `gtk::init`).
pub fn start() -> WidgetsHandle {
    let (tx, rx) = async_channel::unbounded();
    // The tray's and the shortcut's way in, from other threads: commands only (an `Event` holds
    // GTK objects).
    let (remote, from_remote) = async_channel::unbounded::<WidgetsCmd>();
    super::remote::set(remote);
    let forward = tx.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(cmd) = from_remote.recv().await {
            if forward.send(Event::Cmd(cmd)).await.is_err() {
                break;
            }
        }
    });
    let host = Host {
        layer_shell: gtk_layer_shell::is_supported(),
        table: Table::default(),
        widgets: HashMap::new(),
        shared: None,
        filter: FilterState::None,
        expiry: None,
        ping: None,
        hung: HashMap::new(),
        guarded: false,
        locked: false,
        tx: tx.clone(),
        #[cfg(test)]
        probe: None,
    };
    let task = glib::MainContext::default().spawn_local(host.run(rx));
    WidgetsHandle { tx, task }
}

enum FilterState {
    None,
    Compiling,
    Ready(Filter),
}

/// One widget on its way or shown.
struct Widget {
    kind: Kind,
    look: Look,
    body: Body,
    reply: Option<oneshot::Sender<Result<Shown, String>>>,
    view: Option<WidgetView>,
    ready: bool,
    resolved: Option<Resolved>,
    /// The width the page was told, once it was.
    width: Option<u32>,
    /// The text's measured height (`Some`), or the caller's frame loaded (`None`), once known.
    content: Option<Option<u32>>,
    mapped: bool,
    timer: Option<glib::SourceId>,
}

struct Host {
    layer_shell: bool,
    table: Table,
    widgets: HashMap<u64, Widget>,
    shared: Option<Shared>,
    filter: FilterState,
    expiry: Option<glib::SourceId>,
    /// The ping's timer while a widget view exists.
    ping: Option<glib::SourceId>,
    /// The widgets whose web process Kai ended for not responding, and why, until each one's view
    /// says so.
    hung: HashMap<u64, String>,
    /// A Kai confirmation dialog is open ([`WidgetsCmd::Guard`]).
    guarded: bool,
    /// The screen is locked ([`WidgetsCmd::Locked`]).
    locked: bool,
    tx: async_channel::Sender<Event>,
    #[cfg(test)]
    probe: Option<async_channel::Sender<Probe>>,
}

/// Why the widgets of a web process that stopped responding went: `how` it was found.
fn hung(how: &str) -> String {
    format!(
        "its web process stopped responding ({how}; a script that never yields), and Kai ended it \
         with every widget in it"
    )
}

impl Host {
    async fn run(mut self, rx: async_channel::Receiver<Event>) {
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Cmd(cmd) => self.on_cmd(cmd),
                Event::Page { id, msg } => self.on_page(id, msg),
                Event::Refused { id, uri, refused } => {
                    // The first per widget is logged; the rest are counted (overlay_list).
                    if self.table.count(id, Count::Navigation) == Some(1) {
                        tracing::warn!(
                            widget = id,
                            scheme = refused.scheme,
                            "a widget's navigation was refused: {}; further ones are counted in \
                             overlay_list",
                            refused.why
                        );
                    }
                    #[cfg(test)]
                    self.probe(Probe::Refused { id, uri, refused });
                    #[cfg(not(test))]
                    let _ = uri;
                }
                Event::Permission { id } => {
                    if self.table.count(id, Count::Permission) == Some(1) {
                        tracing::warn!(
                            widget = id,
                            "a widget asked for a permission; denied; further ones are counted \
                             in overlay_list"
                        );
                    }
                }
                Event::AppPost { id, message } => {
                    if let Some(v) = self.widgets.get(&id).and_then(|w| w.view.as_ref()) {
                        v.eval(&sandbox::post_js(&message));
                    }
                }
                Event::Unresponsive { id } => self.on_unresponsive(id, hung("WebKit's own check")),
                Event::Ping => self.on_ping(),
                Event::Crashed { id, reason } => {
                    if let Some(why) = self.hung.remove(&id) {
                        #[cfg(test)]
                        self.probe(Probe::Crashed(id));
                        self.fail(id, why);
                    } else if self.widgets.contains_key(&id) {
                        tracing::warn!(
                            widget = id,
                            %reason,
                            "the widgets' web process ended; the widget is hidden (the ring's web \
                             process is its own)"
                        );
                        #[cfg(test)]
                        self.probe(Probe::Crashed(id));
                        self.fail(id, format!("the widgets' web process ended ({reason})"));
                    }
                }
                Event::Closed { id } => {
                    if self.widgets.contains_key(&id) {
                        tracing::info!(
                            widget = id,
                            "the compositor closed a widget's surface (its output went away); it \
                             is hidden"
                        );
                        self.fail(
                            id,
                            "the compositor closed its surface (its output went away)".into(),
                        );
                    }
                }
                Event::Output { id, resolved } => {
                    if let Some(w) = self.widgets.get_mut(&id) {
                        w.resolved = Some(resolved);
                        self.advance(id);
                    }
                }
                Event::ShowTimeout { id } => {
                    if let Some(w) = self.widgets.get_mut(&id)
                        && !w.mapped
                    {
                        w.timer = None;
                        let secs = w.look.show_timeout.as_secs();
                        tracing::warn!(
                            widget = id,
                            "a widget did not show within {secs} s (overlay.show_timeout_s); it is \
                             dropped"
                        );
                        self.fail(
                            id,
                            format!("it did not show within {secs} s (overlay.show_timeout_s)"),
                        );
                    }
                }
                Event::Expire => {
                    self.expiry = None;
                    for id in self.table.expire(Instant::now()) {
                        tracing::info!(widget = id, "a widget's time is up; hidden");
                        self.destroy(id, "its time is up");
                    }
                    self.after_change();
                }
                Event::Filter(result) => self.on_filter(result),
                Event::Quit => break,
            }
        }
        let all = self.table.hide(None).unwrap_or_default();
        for id in all {
            self.destroy(id, "Kai quits");
        }
        self.after_change();
    }

    #[cfg(test)]
    fn probe(&self, p: Probe) {
        if let Some(tx) = &self.probe {
            let _ = tx.try_send(p);
        }
    }

    fn on_cmd(&mut self, cmd: WidgetsCmd) {
        match cmd {
            WidgetsCmd::Show(req) => self.show(req),
            WidgetsCmd::Hide { id, reply } => {
                let result = self.table.hide(id);
                if let Ok(hidden) = &result {
                    if reply.is_none() {
                        tracing::info!(
                            "overlays hidden by the owner (the tray or the shortcut): {}",
                            if hidden.is_empty() {
                                "none was shown".to_owned()
                            } else {
                                ids(hidden)
                            }
                        );
                    }
                    for &id in hidden {
                        self.destroy(id, "hidden");
                    }
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
                self.after_change();
            }
            WidgetsCmd::List { reply } => {
                let _ = reply.send(self.table.list());
            }
            WidgetsCmd::Guard(on) => {
                self.guarded = on;
                let mut ids: Vec<u64> = Vec::new();
                for (id, w) in &self.widgets {
                    if w.view.as_ref().is_some_and(|v| v.guard(on)) {
                        ids.push(*id);
                    }
                }
                ids.sort_unstable();
                if !ids.is_empty() {
                    tracing::info!(
                        widgets = super::table::ids(&ids),
                        "app views {} while a Kai confirmation is open",
                        if on {
                            "click-through and shrunk to Kai's strip"
                        } else {
                            "as configured again"
                        }
                    );
                }
                #[cfg(test)]
                for id in ids {
                    self.probe(Probe::Guarded { id, on });
                }
            }
            WidgetsCmd::Locked(on) => {
                self.locked = on;
                let mut ids: Vec<u64> = Vec::new();
                for (id, w) in &self.widgets {
                    if let Some(v) = w.view.as_ref().filter(|_| w.mapped) {
                        v.set_mapped(!on);
                        ids.push(*id);
                    }
                }
                ids.sort_unstable();
                if !ids.is_empty() {
                    tracing::info!(
                        widgets = super::table::ids(&ids),
                        "widgets {} for the screen lock",
                        if on { "unmapped" } else { "mapped again" }
                    );
                }
                #[cfg(test)]
                for id in ids {
                    self.probe(Probe::LockHidden { id, hidden: on });
                }
            }
            #[cfg(test)]
            WidgetsCmd::Probe(tx) => self.probe = Some(tx),
            #[cfg(test)]
            WidgetsCmd::Terminate(id) => {
                if let Some(v) = self.widgets.get(&id).and_then(|w| w.view.as_ref()) {
                    v.terminate();
                }
            }
            #[cfg(test)]
            WidgetsCmd::EvalMain { id, js, reply } => {
                match self.widgets.get(&id).and_then(|w| w.view.as_ref()) {
                    Some(v) => v.eval_main(&js, move |r| {
                        let _ = reply.send(r);
                    }),
                    None => {
                        let _ = reply.send(Err("no such widget".into()));
                    }
                }
            }
        }
    }

    fn show(&mut self, req: ShowRequest) {
        let ShowRequest {
            kind,
            body,
            look,
            reply,
        } = req;
        if !self.layer_shell {
            let _ = reply.send(Err(
                "the compositor has no layer-shell (wlr-layer-shell): Kai shows no overlays".into(),
            ));
            return;
        }
        let id = self
            .table
            .add(kind, look.position, look.interactive, look.timeout);
        let (tx, configured) = (self.tx.clone(), look.output.clone());
        glib::MainContext::default().spawn_local(async move {
            let resolved = resolve_output("overlay.output", configured).await;
            let _ = tx.send(Event::Output { id, resolved }).await;
        });
        let tx = self.tx.clone();
        let timer = glib::timeout_add_local_once(look.show_timeout, move || {
            let _ = tx.try_send(Event::ShowTimeout { id });
        });
        self.widgets.insert(
            id,
            Widget {
                kind,
                look,
                body,
                reply: Some(reply),
                view: None,
                ready: false,
                resolved: None,
                width: None,
                content: None,
                mapped: false,
                timer: Some(timer),
            },
        );
        tracing::info!(widget = id, kind = kind.key(), "showing a widget");
        match self.filter {
            FilterState::Ready(_) => self.create(id),
            FilterState::Compiling => {}
            FilterState::None => {
                self.filter = FilterState::Compiling;
                let tx = self.tx.clone();
                filter::compile(&filter::store_dir(), move |r| {
                    let _ = tx.try_send(Event::Filter(r));
                });
            }
        }
        self.after_change();
    }

    fn on_filter(&mut self, result: Result<Filter, String>) {
        match result {
            Ok(f) => {
                tracing::debug!("the widgets' content filter is compiled");
                self.filter = FilterState::Ready(f);
                let waiting: Vec<u64> = self
                    .widgets
                    .iter()
                    .filter(|(_, w)| w.view.is_none())
                    .map(|(id, _)| *id)
                    .collect();
                for id in waiting {
                    self.create(id);
                }
            }
            Err(e) => {
                // Fail closed: no widget without the filter. The next show compiles it again.
                tracing::error!(
                    "the widgets' content filter did not compile ({e}); no widget is shown \
                     without it"
                );
                self.filter = FilterState::None;
                let waiting: Vec<u64> = self.widgets.keys().copied().collect();
                for id in waiting {
                    self.fail(
                        id,
                        format!(
                            "the overlays' content filter did not compile ({e}); Kai shows no \
                             overlay without it"
                        ),
                    );
                }
            }
        }
    }

    fn create(&mut self, id: u64) {
        let FilterState::Ready(filter) = &self.filter else {
            return;
        };
        let Some(w) = self.widgets.get(&id) else {
            return;
        };
        let shared = self.shared.get_or_insert_with(Shared::new);
        let bridge = match &w.body {
            Body::Document(doc) => {
                shared.set_doc(id, doc);
                None
            }
            Body::App(app) => {
                shared.set_doc(id, &app.html);
                Some((
                    app.ends.from_bridge.clone(),
                    super::view::AppFrame {
                        max: app.bound.bytes,
                        server: app.server.clone(),
                    },
                ))
            }
            Body::Text(_) => None,
        };
        let frame = Frame {
            anchor: w.look.position,
            margin: w.look.margin,
            interactive: w.look.interactive,
        };
        let every = w.look.unresponsive;
        let app = bridge.as_ref().map(|(_, a)| a);
        match WidgetView::create(id, shared, filter, frame, app, self.tx.clone()) {
            Ok(view) => {
                // A confirmation already open: the new view waits like the others.
                if self.guarded && view.guard(true) {
                    #[cfg(test)]
                    self.probe(Probe::Guarded { id, on: true });
                }
                if let Some(w) = self.widgets.get_mut(&id) {
                    w.view = Some(view);
                }
                // The bridge's answers to the view, until the bridge ends (the widget went).
                if let Some((answers, _)) = bridge {
                    let tx = self.tx.clone();
                    glib::MainContext::default().spawn_local(async move {
                        while let Ok(message) = answers.recv().await {
                            if tx.send(Event::AppPost { id, message }).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                if self.ping.is_none() {
                    let tx = self.tx.clone();
                    self.ping = Some(glib::timeout_add_local(every, move || {
                        let _ = tx.try_send(Event::Ping);
                        glib::ControlFlow::Continue
                    }));
                }
                #[cfg(test)]
                self.probe(Probe::Created(id));
            }
            Err(e) => {
                // Fail closed, visibly: the error names the feature.
                tracing::error!(widget = id, "a widget view was not created: {e}");
                self.fail(id, e);
            }
        }
    }

    /// Every widget view is pinged in Kai's world; a view whose last ping is still unanswered (for
    /// at least `overlay.unresponsive_s`) has a web process that stopped responding. WebKit's own
    /// check runs only when the page is used or closed (2.54: a stalled widget nobody touches went
    /// 120 s without its verdict, the live check), so Kai asks itself.
    fn on_ping(&mut self) {
        let mut stalled = None;
        let mut every = Duration::ZERO;
        for (id, w) in &self.widgets {
            if let Some(v) = &w.view
                && !v.ping()
                && !self.hung.contains_key(id)
            {
                stalled = Some(*id);
                every = w.look.unresponsive;
            }
        }
        if let Some(id) = stalled {
            self.on_unresponsive(
                id,
                hung(&format!(
                    "no answer within {} s, overlay.unresponsive_s",
                    every.as_secs()
                )),
            );
        }
    }

    /// Widget `id`'s web process stopped responding: Kai ends it. Every widget view in it then says
    /// so (`web-process-terminated`), and each goes with `why`.
    fn on_unresponsive(&mut self, id: u64, why: String) {
        #[cfg(test)]
        self.probe(Probe::Unresponsive(id));
        if self.hung.contains_key(&id) {
            return;
        }
        let Some(view) = self.widgets.get(&id).and_then(|w| w.view.as_ref()) else {
            return;
        };
        let with_views: Vec<u64> = self
            .widgets
            .iter()
            .filter(|(_, w)| w.view.is_some())
            .map(|(i, _)| *i)
            .collect();
        let mut sorted = with_views.clone();
        sorted.sort_unstable();
        tracing::warn!(
            widgets = ids(&sorted),
            "the widgets' web process stopped responding; Kai ends it, and the widgets in it go: \
             {why}"
        );
        self.hung
            .extend(with_views.into_iter().map(|i| (i, why.clone())));
        view.terminate();
    }

    fn on_page(&mut self, id: u64, msg: PageMsg) {
        #[cfg(test)]
        self.probe(Probe::Page(id, msg.clone()));
        let Some(w) = self.widgets.get_mut(&id) else {
            return;
        };
        match msg {
            // Only an overlay_app widget has a bridge (overlay_html none, K18), and only its own.
            PageMsg::Rpc(message) => {
                match &w.body {
                    Body::App(app) => {
                        let _ = app.ends.to_bridge.send(message);
                    }
                    _ => tracing::warn!(
                        widget = id,
                        "a widget without a bridge relayed a view's message; ignored"
                    ),
                }
                return;
            }
            PageMsg::Ready => w.ready = true,
            PageMsg::Size(h) if w.kind.measures() => w.content = Some(Some(h)),
            PageMsg::Loaded if !w.kind.measures() => w.content = Some(None),
            other => {
                tracing::debug!(widget = id, "a widget's host page said {other:?}");
                return;
            }
        }
        self.advance(id);
    }

    /// The next step of a show, once what it waits for is there.
    fn advance(&mut self, id: u64) {
        let Some(w) = self.widgets.get_mut(&id) else {
            return;
        };
        if w.mapped || !w.ready {
            return;
        }
        if self.locked {
            return self.fail(
                id,
                format!(
                    "{}; the screen locked before it showed",
                    crate::screen_lock::REFUSAL
                ),
            );
        }
        let Some(resolved) = &w.resolved else {
            return;
        };
        if resolved.none_listed {
            return self.fail(
                id,
                "no output is connected, so the overlay cannot show".into(),
            );
        }
        let bounds = match &resolved.monitor {
            Some((m, _)) => Bounds::of(m),
            // The compositor chooses: what fits on every output.
            None => Bounds::smallest().unwrap_or(Bounds {
                width: w.look.width,
                height: w.look.height.max(1),
            }),
        };
        let fit = fit(&w.look, w.kind, bounds);
        let Some(view) = &w.view else { return };
        if w.width.is_none() {
            w.width = Some(fit.width);
            view.eval(&sandbox::show_js(id, &w.body, fit.width));
            return;
        }
        let Some(content) = w.content else { return };
        if w.reply.as_ref().is_some_and(|r| r.is_closed()) {
            return self.fail(id, "the call was cancelled before it showed".into());
        }
        let (height, notes, scrolls) = fit.height(content);
        if scrolls {
            view.eval("__kaiWidget.fit(true)");
        }
        let (monitor, output) = match &resolved.monitor {
            Some((m, name)) => (Some(m.clone()), Some(name.clone())),
            None => (None, None),
        };
        if let Some(note) = &resolved.note {
            tracing::info!(widget = id, "widget output: {note}");
        }
        view.show(monitor.as_ref(), fit.width, height);
        w.mapped = true;
        if let Some(t) = w.timer.take() {
            remove_source(t);
        }
        let refused = match &w.body {
            Body::App(app) => app.notes.clone(),
            _ => Vec::new(),
        };
        let notes: Vec<String> = fit.notes.into_iter().chain(notes).chain(refused).collect();
        let reply = w.reply.take();
        self.table
            .shown(id, Instant::now(), &Clock::Local, notes.clone());
        tracing::info!(
            widget = id,
            output = output.as_deref().unwrap_or("(the compositor's choice)"),
            width = fit.width,
            height,
            "a widget is shown"
        );
        #[cfg(test)]
        self.probe(Probe::Mapped {
            id,
            output,
            width: fit.width,
            height,
        });
        if let Some(r) = reply {
            let _ = r.send(Ok(Shown { id, notes }));
        }
        self.after_change();
    }

    /// Widget `id` does not show (or goes): its call answered with `why`, or, once shown,
    /// `overlay_list` says so once; its view destroyed.
    fn fail(&mut self, id: u64, why: String) {
        let reason = why.clone();
        match self.widgets.get_mut(&id).and_then(|w| w.reply.take()) {
            Some(r) => {
                self.table.remove(id);
                let _ = r.send(Err(why));
            }
            None => self.table.ended(id, &Clock::Local, &why),
        }
        self.destroy(id, &reason);
        self.after_change();
    }

    /// Widget `id` goes; `reason` is what an `overlay_app` view's `ui/resource-teardown` says.
    fn destroy(&mut self, id: u64, reason: &str) {
        self.hung.remove(&id);
        let Some(mut w) = self.widgets.remove(&id) else {
            return;
        };
        if let Some(t) = w.timer.take() {
            remove_source(t);
        }
        if let Some(r) = w.reply.take() {
            let _ = r.send(Err("hidden before it showed".into()));
        }
        if let Some(v) = w.view.take() {
            match &w.body {
                // MCP Apps: the view is told before it goes (module doc of app_bridge).
                Body::App(_) => v.teardown(
                    self.shared.as_ref(),
                    &super::app_bridge::teardown(reason),
                    w.look.unresponsive,
                ),
                _ => v.destroy(self.shared.as_ref()),
            }
            #[cfg(test)]
            self.probe(Probe::Destroyed(id));
        }
    }

    /// After any change: the context dropped once no view is left, the expiry timer re-armed, the
    /// tray told.
    fn after_change(&mut self) {
        if self.widgets.values().all(|w| w.view.is_none())
            && let Some(t) = self.ping.take()
        {
            remove_source(t);
        }
        if self.shared.is_some() && self.widgets.values().all(|w| w.view.is_none()) {
            self.shared = None;
            tracing::debug!("no widget left: the widgets' web context is dropped");
            #[cfg(test)]
            self.probe(Probe::ContextGone);
        }
        if let Some(t) = self.expiry.take() {
            remove_source(t);
        }
        if let Some(at) = self.table.next_expiry() {
            let tx = self.tx.clone();
            let wait = at.saturating_duration_since(Instant::now());
            self.expiry = Some(glib::timeout_add_local_once(wait, move || {
                let _ = tx.try_send(Event::Expire);
            }));
        }
        super::remote::tell_tray(self.table.tray());
    }
}

/// Removes a timer unless it is gone already (a one-shot that fired): removing a gone source
/// panics.
fn remove_source(id: glib::SourceId) {
    if glib::MainContext::default()
        .find_source_by_id(&id)
        .is_some()
    {
        id.remove();
    }
}

/// The room an output gives, in logical px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    fn of(m: &gdk::Monitor) -> Bounds {
        let g = m.geometry();
        Bounds {
            width: g.width().max(1) as u32,
            height: g.height().max(1) as u32,
        }
    }

    /// The smallest width and height among the outputs (`None` with none).
    fn smallest() -> Option<Bounds> {
        let d = gdk::Display::default()?;
        (0..d.n_monitors())
            .filter_map(|i| d.monitor(i))
            .map(|m| Bounds::of(&m))
            .reduce(|a, b| Bounds {
                width: a.width.min(b.width),
                height: a.height.min(b.height),
            })
    }
}

/// A widget's size within its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fit {
    pub width: u32,
    /// The most height the output leaves (less the margins).
    pub room: u32,
    /// The configured height (0: the content's).
    pub want: u32,
    pub notes: Vec<String>,
}

/// The output less the margins, at least one pixel.
fn room(total: u32, margin: u32) -> u32 {
    total.saturating_sub(margin.saturating_mul(2)).max(1)
}

/// The width a widget gets on `bounds`, and the height it may take.
pub fn fit(look: &Look, kind: Kind, bounds: Bounds) -> Fit {
    let room_w = room(bounds.width, look.margin);
    let room_h = room(bounds.height, look.margin);
    let mut notes = Vec::new();
    let width = look.width.min(room_w);
    if width < look.width {
        notes.push(format!(
            "its width of {} px is fitted to the output's {room_w} px (less the margins; \
             overlay.{}.width)",
            look.width,
            kind.key()
        ));
    }
    Fit {
        width,
        room: room_h,
        want: look.height,
        notes,
    }
}

impl Fit {
    /// The surface's height for text `measured` px tall (`None`: the caller's frame, whose
    /// document scrolls itself), the notes, and whether the text scrolls.
    pub fn height(&self, measured: Option<u32>) -> (u32, Vec<String>, bool) {
        let mut notes = Vec::new();
        if self.want == 0 {
            let content = measured.unwrap_or(self.room);
            let h = content.clamp(1, self.room);
            let scrolls = content > self.room;
            if scrolls {
                notes.push(format!(
                    "the text is {content} px tall and {} px fit on the output (less the \
                     margins): it scrolls",
                    self.room
                ));
            }
            return (h, notes, scrolls);
        }
        let h = self.want.min(self.room);
        if h < self.want {
            notes.push(format!(
                "its height of {} px is fitted to the output's {} px (less the margins)",
                self.want, self.room
            ));
        }
        // A measured text taller than a fixed height scrolls; a frame's own document scrolls.
        let scrolls = measured.is_some_and(|content| content > h);
        if let Some(content) = measured.filter(|_| scrolls) {
            notes.push(format!(
                "the text is {content} px tall and the overlay {h} px: it scrolls"
            ));
        }
        (h, notes, scrolls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(width: u32, height: u32) -> Look {
        Look {
            position: Anchor::TopRight,
            margin: 24,
            width,
            height,
            timeout: None,
            interactive: false,
            output: None,
            show_timeout: Duration::from_secs(10),
            unresponsive: Duration::from_secs(3),
        }
    }

    const FHD: Bounds = Bounds {
        width: 1920,
        height: 1080,
    };

    #[test]
    fn a_text_follows_its_lines_up_to_the_output() {
        let f = fit(&look(360, 0), Kind::Text, FHD);
        assert_eq!((f.width, f.room), (360, 1032));
        assert!(f.notes.is_empty());
        assert_eq!(f.height(Some(118)), (118, vec![], false));
        let (h, notes, scrolls) = f.height(Some(5000));
        assert_eq!((h, scrolls), (1032, true));
        assert_eq!(
            notes,
            [
                "the text is 5000 px tall and 1032 px fit on the output (less the margins): it \
              scrolls"
            ]
        );
    }

    #[test]
    fn a_size_beyond_the_output_is_fitted_and_said() {
        let small = Bounds {
            width: 400,
            height: 300,
        };
        let f = fit(&look(480, 320), Kind::Html, small);
        assert_eq!(f.width, 352);
        assert_eq!(
            f.notes,
            [
                "its width of 480 px is fitted to the output's 352 px (less the margins; \
              overlay.html.width)"
            ]
        );
        let (h, notes, scrolls) = f.height(None);
        assert_eq!(
            (h, scrolls),
            (252, false),
            "the frame's document scrolls itself"
        );
        assert_eq!(
            notes,
            ["its height of 320 px is fitted to the output's 252 px (less the margins)"]
        );
        // A fixed height for text: taller text scrolls, and says so.
        let f = fit(&look(360, 200), Kind::Text, FHD);
        let (h, notes, scrolls) = f.height(Some(450));
        assert_eq!((h, scrolls), (200, true));
        assert_eq!(
            notes,
            ["the text is 450 px tall and the overlay 200 px: it scrolls"]
        );
        assert_eq!(f.height(Some(120)), (200, vec![], false));
        // Margins wider than the output leave one pixel, never zero or a panic.
        let tiny = Bounds {
            width: 10,
            height: 10,
        };
        let f = fit(&look(360, 0), Kind::Text, tiny);
        assert_eq!((f.width, f.room), (1, 1));
    }
}
