//! The ring overlay (§9, WP5): lmgw's `ring.js` on a layer-shell surface over the desktop, shown
//! while Kai is active (actively listening, thinking or talking) and hidden in standby.
//!
//! - [`lifecycle`] decides, from the voice session's [`RingInput`], when the view exists, when it
//!   shows and what the page is told (K1, K11, §9.4). Pure; the tests pin it.
//! - [`view`] is the surface and the WebKit view (§9.1), [`page`] the `kai://` assets and the bridge
//!   (§9.2), [`placement`] and [`outputs`] the anchor, layer and output.
//! - The [`Overlay`] task on the GTK thread carries the decisions out and feeds the levels from
//!   [`LevelTaps`] 30 times a second while the surface is mapped (§9.3), skipping a frame equal to
//!   the last one sent ([`feed`]). Readers exist only then, so nothing is computed while the ring
//!   is hidden.
//! - After a second crash it retries on a timer whose wait doubles ([`RETRY_FIRST`] to
//!   [`RETRY_MAX`]), and tells the tray while the ring is down ([`RingNotes`]).
//! - It keeps the window when the compositor closes the surface (its output went away) and shows
//!   the ring again; an output added or removed places a ring that is up again (review KAI-5
//!   R5-14). While the compositor lists no output at all, nothing is mapped until one is added
//!   (review KAI-6 R6-2).
//!
//! **The seam for the voice session (WP4).** The core sends [`OverlayCmd::Ring`] (through
//! `UiCmd::Overlay`, see `Core::set_ring`) whenever the session's facts change: `bound` from the
//! bind until the session ends, `active` while Kai is not in standby (the ring shows exactly then;
//! out of a turn in the listening state), `turn` from `speech_started` or a talk press to the end
//! of the reply's playback, and the voice state. [`OverlayCmd::Timing`] carries
//! `lmgw.response.timing`.

mod feed;
mod lifecycle;
pub(crate) mod outputs;
mod page;
pub mod placement;
mod view;

#[cfg(test)]
mod live_tests;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use serde_json::Value;
use webkit2gtk::WebViewExt;

use crate::audio::{LevelTaps, Tap};
use crate::config::Config;
use feed::FeedDedup;
use lifecycle::{Action, Down, Lifecycle};
use page::PageMsg;
use placement::{Layer, Listed, Placing};
use view::RingView;

pub use lifecycle::{RingInput, RingState};
/// The ring's surface code, reused by the tool-controlled overlays (`crate::widgets`).
pub(crate) use view::{Place, layer_window, place_window};

/// The feed rate while shown (K11, §9.3).
pub const FEED_HZ: u32 = 30;

/// How long Kai waits for the page to report the idle state drawn before it unmaps anyway (a hung
/// or crashed page must not leave the ring up). Named in the note when it passes.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(2);

/// How long a new view's page may take to say `ready` (it takes about a quarter of a second, WP0
/// (i)). Past it the page counts as hung: logged with this value and torn down like a crashed
/// one (§9.4 says when a new view comes).
pub const READY_LIMIT: Duration = Duration::from_secs(10);

/// After a second crash with no turn's start and no leaving standby in between, the first retry
/// comes this long after (§9.4, review KAI-5 R5-3). Each retry is logged.
pub const RETRY_FIRST: Duration = Duration::from_secs(30);

/// The retries' wait doubles up to this. There is no cap on their number: a page that keeps
/// crashing costs one web process per this long, and the tray says the ring is down meanwhile.
pub const RETRY_MAX: Duration = Duration::from_secs(300);

/// Tells the tray while the ring is down after a second crash (`Some`, a line for its tooltip),
/// and once it is not (`None`). Called on the GTK thread, only on a change.
pub type RingNotes = Box<dyn Fn(Option<String>)>;

/// From the core, through `UiCmd::Overlay`.
#[derive(Debug)]
pub enum OverlayCmd {
    /// The voice session's facts (the WP4 seam; see [`RingInput`]).
    Ring(RingInput),
    /// `lmgw.response.timing` of the last turn, or `None` (§9.3 `setTiming`).
    #[allow(dead_code)] // the voice session (WP4)
    Timing(Option<Value>),
    /// `overlay.ring_over_fullscreen`: a ring that is up moves to that layer at once (review KAI-5
    /// R5-2), a hidden one from the next time it shows.
    RingOverFullscreen(bool),
    /// The live check's window into the overlay.
    #[cfg(test)]
    Probe(async_channel::Sender<Probe>),
    #[cfg(test)]
    Eval(String),
    /// The live check's A/B of the feed's saving: every frame sent, equal or not, as before it.
    #[cfg(test)]
    SendEqual(bool),
    /// The live check's shorter first retry wait (it doubles from there, up to [`RETRY_MAX`]).
    #[cfg(test)]
    RetryFirst(Duration),
    /// The live check's latency probe: the next input level at or above this is reported when the
    /// feed reads it ([`Probe::Heard`]) and when the page drew a frame with it (`heard`).
    #[cfg(test)]
    WatchInput(f32),
    /// The live check's stand-in for a compositor with no output (KWin keeps at least one in a
    /// nested session): while set, each output resolution finds the output list empty.
    #[cfg(test)]
    NoOutputs(bool),
}

/// What the overlay needs from the configuration (§9.1, §16 answer 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingSettings {
    pub place: Place,
    /// `ring.output`, a connector name.
    pub output: Option<String>,
    pub over_fullscreen: bool,
}

impl RingSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            place: Place {
                anchor: c.ring.anchor,
                margin: c.ring.margin,
                size: c.ring.size,
            },
            output: c.ring.output.clone(),
            over_fullscreen: c.overlay.ring_over_fullscreen,
        }
    }
}

/// What the live check sees.
#[cfg(test)]
#[derive(Debug, Clone)]
pub enum Probe {
    Created,
    Ready {
        ms: f64,
    },
    Mapped {
        ms: f64,
        layer: Layer,
        /// The connector the surface was put on; `None` for the compositor's choice or the output
        /// it is on.
        output: Option<String>,
        note: Option<String>,
    },
    Drawn {
        ms: f64,
    },
    Hidden,
    Destroyed,
    /// The compositor closed the surface.
    Closed,
    /// A show found the compositor listing no output: nothing was mapped.
    NoOutput,
    /// The feed read an input level at or above the watched one ([`OverlayCmd::WatchInput`]).
    Heard,
    /// An output was added (`true`) or removed.
    Outputs(bool),
    Page(PageMsg),
    /// What the tray was told ([`RingNotes`]).
    Note(Option<String>),
}

enum Event {
    Cmd(OverlayCmd),
    Page {
        view: u64,
        msg: PageMsg,
    },
    Crashed {
        view: u64,
        reason: String,
    },
    /// The compositor closed view `view`'s surface (its output went away); the window is kept.
    Closed {
        view: u64,
    },
    /// GDK added (`true`) or removed an output.
    Outputs(bool),
    /// The output for show request `show` is resolved.
    Output {
        show: u64,
        resolved: Resolved,
    },
    SettleTimeout {
        seq: u64,
    },
    /// The page of view `view` did not say `ready` within [`READY_LIMIT`].
    ReadyTimeout {
        view: u64,
    },
    /// The retry timer after a second crash.
    Retry,
    Reduced(bool),
    Quit,
}

pub struct OverlayHandle {
    tx: async_channel::Sender<Event>,
    task: glib::JoinHandle<()>,
}

impl OverlayHandle {
    pub fn send(&self, cmd: OverlayCmd) {
        let _ = self.tx.try_send(Event::Cmd(cmd));
    }

    /// §2's quit step 4: the overlays go. Returns once the view is destroyed.
    pub async fn quit(self) {
        let _ = self.tx.send(Event::Quit).await;
        let _ = self.task.await;
    }
}

/// Starts the overlay on the GTK thread (after `gtk::init`).
pub fn start(settings: RingSettings, taps: LevelTaps, notes: RingNotes) -> OverlayHandle {
    let (tx, rx) = async_channel::unbounded();
    let reduced = match gtk::Settings::default() {
        Some(s) => {
            let t = tx.clone();
            s.connect_gtk_enable_animations_notify(move |s| {
                let _ = t.try_send(Event::Reduced(!s.is_gtk_enable_animations()));
            });
            !s.is_gtk_enable_animations()
        }
        None => false,
    };
    let layer_shell = gtk_layer_shell::is_supported();
    // An output that goes or comes places a ring that is up again (review KAI-5 R5-14).
    let monitor_signals = gdk::Display::default().map(|d| {
        let t = tx.clone();
        let added = d.connect_monitor_added(move |_, _| {
            let _ = t.try_send(Event::Outputs(true));
        });
        let t = tx.clone();
        let removed = d.connect_monitor_removed(move |_, _| {
            let _ = t.try_send(Event::Outputs(false));
        });
        (d, [added, removed])
    });
    let overlay = Overlay {
        layer_shell,
        settings,
        taps,
        life: Lifecycle::new(),
        view: None,
        next_id: 0,
        show_gen: 0,
        feed: None,
        feed_stats: None,
        ready_timer: None,
        settle: None,
        retry: None,
        retry_first: RETRY_FIRST,
        mapped: false,
        resolving: false,
        resolve_again: false,
        monitor_signals,
        notes,
        note: None,
        reduced,
        timing: None,
        tx: tx.clone(),
        created_at: None,
        show_at: None,
        last_note: None,
        #[cfg(test)]
        probe: None,
        #[cfg(test)]
        send_equal: Rc::default(),
        #[cfg(test)]
        watch: Rc::default(),
        #[cfg(test)]
        no_outputs: false,
    };
    let task = glib::MainContext::default().spawn_local(overlay.run(rx));
    OverlayHandle { tx, task }
}

struct Overlay {
    /// The compositor speaks wlr-layer-shell. Without it a ring window would be an ordinary
    /// toplevel, so there is no ring (said once).
    layer_shell: bool,
    settings: RingSettings,
    taps: LevelTaps,
    life: Lifecycle,
    view: Option<RingView>,
    next_id: u64,
    /// Bumped by every show, hide and destroy: an output that resolves late is dropped.
    show_gen: u64,
    feed: Option<glib::SourceId>,
    /// The running feed's counts.
    feed_stats: Option<Rc<FeedCounts>>,
    /// The deadline for the new view's `ready`.
    ready_timer: Option<glib::SourceId>,
    settle: Option<(u64, glib::SourceId)>,
    /// The retry timer after a second crash: when it fires, that time on the wall clock (for the
    /// tray), and its source.
    retry: Option<(Instant, String, glib::SourceId)>,
    /// [`RETRY_FIRST`], shorter in the live check.
    retry_first: Duration,
    /// The surface is mapped: `on_output` showed it, and no hide or destroy came since.
    mapped: bool,
    /// An output resolution runs ([`Overlay::request_show`]).
    resolving: bool,
    /// A show was requested while it ran: another resolution follows it.
    resolve_again: bool,
    /// GDK's monitor-added and monitor-removed handlers, disconnected at quit.
    monitor_signals: Option<(gdk::Display, [glib::SignalHandlerId; 2])>,
    notes: RingNotes,
    /// What the tray was last told.
    note: Option<String>,
    reduced: bool,
    timing: Option<Value>,
    tx: async_channel::Sender<Event>,
    created_at: Option<Instant>,
    show_at: Option<Instant>,
    last_note: Option<String>,
    #[cfg(test)]
    probe: Option<async_channel::Sender<Probe>>,
    /// The live check's A/B: frames equal to the last one are sent too.
    #[cfg(test)]
    send_equal: Rc<Cell<bool>>,
    /// The live check's latency probe: the input level the feed watches for.
    #[cfg(test)]
    watch: Rc<Cell<Option<f32>>>,
    /// The live check's compositor with no output ([`OverlayCmd::NoOutputs`]).
    #[cfg(test)]
    no_outputs: bool,
}

/// The running feed's counts, shared with its GTK-thread timer.
#[derive(Default)]
struct FeedCounts {
    /// Calls sent.
    sent: Cell<u64>,
    /// Frames equal to the last one sent: the page holds them already, so nothing was sent.
    same: Cell<u64>,
    /// Skipped while the last call had not completed.
    behind: Cell<u64>,
}

impl FeedCounts {
    fn bump(c: &Cell<u64>) {
        c.set(c.get() + 1);
    }
}

fn ms_since(t: Option<Instant>) -> f64 {
    t.map(|t| t.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(f64::NAN)
}

impl Overlay {
    async fn run(mut self, rx: async_channel::Receiver<Event>) {
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Cmd(cmd) => self.on_cmd(cmd),
                Event::Page { view, msg } if self.is_current(view) => self.on_page(msg),
                Event::Crashed { view, reason } if self.is_current(view) => {
                    let actions = self.life.crashed();
                    tracing::warn!(%reason, "the ring's web process ended; {}", self.after_crash(&actions));
                    self.apply(actions);
                }
                Event::Retry => self.on_retry(),
                Event::Closed { view } if self.is_current(view) => self.on_closed(),
                Event::Outputs(added) => self.on_outputs(added),
                Event::Page { .. } | Event::Crashed { .. } | Event::Closed { .. } => {}
                Event::Output { show, resolved } => {
                    self.resolving = false;
                    if std::mem::take(&mut self.resolve_again) {
                        self.start_resolve();
                    } else {
                        self.on_output(show, resolved);
                    }
                }
                Event::ReadyTimeout { view } => {
                    self.ready_timer = None;
                    if self.is_current(view) && self.life.loading() {
                        let actions = self.life.crashed();
                        tracing::warn!(
                            "the ring page did not say ready within {} s; its view is torn down \
                             like a crashed one; {}",
                            READY_LIMIT.as_secs(),
                            self.after_crash(&actions)
                        );
                        self.apply(actions);
                    }
                }
                Event::SettleTimeout { seq } => {
                    if self.life.settling_seq() == Some(seq) {
                        tracing::info!(
                            "the ring page did not report the idle state drawn within {} ms; hiding the ring",
                            SETTLE_LIMIT.as_millis()
                        );
                        self.settle = None;
                        let actions = self.life.settled(seq);
                        self.apply(actions);
                    }
                }
                Event::Reduced(on) => {
                    self.reduced = on;
                    tracing::info!(reduced = on, "reduced motion (gtk-enable-animations)");
                    if let Some(v) = &self.view {
                        v.eval(&page::reduced_js(on));
                    }
                }
                Event::Quit => break,
            }
        }
        if let Some((_, _, id)) = self.retry.take() {
            remove_source(id);
        }
        if let Some((d, ids)) = self.monitor_signals.take() {
            for id in ids {
                d.disconnect(id);
            }
        }
        self.destroy();
    }

    fn is_current(&self, id: u64) -> bool {
        self.view.as_ref().is_some_and(|v| v.id == id)
    }

    /// What follows a crash, for its log line (§9.4).
    fn after_crash(&self, actions: &[Action]) -> String {
        if actions.contains(&Action::Create) {
            "a new view is created now".into()
        } else if self.life.waiting_after_crash() {
            format!(
                "it ended twice since the last turn's start or the last time Kai left standby, so a \
                 new view comes with the next turn, when Kai leaves standby again, or with retry {} \
                 in {} s (each wait doubles, up to {} s)",
                self.life.retries() + 1,
                self.next_retry_in().as_secs(),
                RETRY_MAX.as_secs()
            )
        } else {
            "nothing needs the ring now".into()
        }
    }

    /// How long until the next retry: the armed timer's remaining time, or the wait it gets.
    fn next_retry_in(&self) -> Duration {
        match &self.retry {
            Some((at, _, _)) => at.saturating_duration_since(Instant::now()),
            None => lifecycle::retry_wait(self.retry_first, RETRY_MAX, self.life.retries()),
        }
    }

    /// The compositor closed the ring's surface: its output went away, a monitor powered off or
    /// unplugged (§9.1, review KAI-5 R5-14).
    fn on_closed(&mut self) {
        let before = self.life.down();
        let actions = self.life.closed();
        if actions.contains(&Action::Show) {
            tracing::info!(
                "the compositor closed the ring's surface (its output went away); the ring shows \
                 again on an output resolved now"
            );
        } else if before.is_none() && self.life.down() == Some(Down::Closed) {
            // What Kai knows, not a claim about the outputs (review KAI-6 R6-2).
            tracing::warn!(
                "the compositor closed the ring's surface again, and Kai saw no output added or \
                 removed and no turn start since it closed it last; the ring stays down until one \
                 of those"
            );
        } else {
            tracing::debug!("the compositor closed the ring's surface; nothing to show");
        }
        #[cfg(test)]
        self.probe(Probe::Closed);
        self.apply(actions);
    }

    /// GDK added or removed an output (§9.1, review KAI-5 R5-14).
    fn on_outputs(&mut self, added: bool) {
        let actions = self.life.outputs_changed();
        tracing::info!(
            "an output was {}{}",
            if added { "added" } else { "removed" },
            if actions.contains(&Action::Show) {
                "; the ring that is up is placed again"
            } else {
                ""
            }
        );
        #[cfg(test)]
        self.probe(Probe::Outputs(added));
        self.apply(actions);
    }

    /// The retry timer fired (§9.4, review KAI-5 R5-3).
    fn on_retry(&mut self) {
        self.retry = None;
        let before = self.life.retries();
        let actions = self.life.retry();
        if actions.contains(&Action::Create) {
            tracing::warn!(
                retry = self.life.retries(),
                "the ring's web process ended twice: retry {}, a new view is created now; should it \
                 end again, the next retry comes {} s later (each wait doubles, up to {} s)",
                self.life.retries(),
                lifecycle::retry_wait(self.retry_first, RETRY_MAX, self.life.retries()).as_secs(),
                RETRY_MAX.as_secs()
            );
        } else if before > 0 && !self.life.waiting_after_crash() {
            tracing::info!(
                "the ring's view from retry {before} stayed up until the next retry was due: the \
                 ring counts as recovered, and a crash is recreated at once again"
            );
        }
        self.apply(actions);
    }

    #[cfg(test)]
    fn probe(&self, p: Probe) {
        if let Some(tx) = &self.probe {
            let _ = tx.try_send(p);
        }
    }

    fn on_cmd(&mut self, cmd: OverlayCmd) {
        match cmd {
            OverlayCmd::Ring(input) => {
                let actions = self.life.input(input);
                self.apply(actions);
            }
            OverlayCmd::Timing(t) => {
                self.timing = t;
                if let Some(v) = &self.view {
                    v.eval(&page::timing_js(self.timing.as_ref()));
                }
            }
            OverlayCmd::RingOverFullscreen(on) => {
                self.settings.over_fullscreen = on;
                // The same re-placement a turn's start does: gtk-layer-shell remaps only for a
                // layer or output that changed. A ring that is down stays down (review KAI-6
                // R6-3).
                let actions = self.life.replace();
                tracing::info!(
                    on,
                    "ring over fullscreen windows: layer {:?} {}",
                    Layer::for_setting(on),
                    if actions.contains(&Action::Show) {
                        "now (the ring is up)"
                    } else {
                        "from the next time the ring shows"
                    }
                );
                self.apply(actions);
            }
            #[cfg(test)]
            OverlayCmd::Probe(tx) => self.probe = Some(tx),
            #[cfg(test)]
            OverlayCmd::Eval(js) => {
                if let Some(v) = &self.view {
                    v.eval(&js);
                }
            }
            #[cfg(test)]
            OverlayCmd::SendEqual(on) => self.send_equal.set(on),
            #[cfg(test)]
            OverlayCmd::RetryFirst(d) => self.retry_first = d,
            #[cfg(test)]
            OverlayCmd::NoOutputs(on) => self.no_outputs = on,
            #[cfg(test)]
            OverlayCmd::WatchInput(level) => {
                self.watch.set(Some(level));
                if let Some(v) = &self.view {
                    v.eval(&format!("__kai.watch({level})"));
                }
            }
        }
    }

    fn on_page(&mut self, msg: PageMsg) {
        #[cfg(test)]
        self.probe(Probe::Page(msg.clone()));
        match msg {
            PageMsg::Ready => {
                self.cancel_ready_timer();
                let ms = ms_since(self.created_at);
                tracing::debug!("the ring page is ready {ms:.0} ms after the view was created");
                #[cfg(test)]
                self.probe(Probe::Ready { ms });
                if let (Some(v), Some(_)) = (&self.view, &self.timing) {
                    v.eval(&page::timing_js(self.timing.as_ref()));
                }
                let actions = self.life.ready();
                self.apply(actions);
            }
            PageMsg::Drawn => {
                let ms = ms_since(self.show_at);
                tracing::debug!("the ring's first frame came {ms:.0} ms after it was shown");
                #[cfg(test)]
                self.probe(Probe::Drawn { ms });
            }
            PageMsg::Settled { seq } => {
                let actions = self.life.settled(seq);
                self.apply(actions);
            }
            PageMsg::Error(e) => tracing::warn!("the ring page: {e}"),
            PageMsg::Csp { directive, blocked } => {
                tracing::warn!(%directive, %blocked, "the ring page broke its CSP")
            }
            PageMsg::Stats(v) => tracing::debug!(%v, "the ring page's stats"),
            PageMsg::Other(t) => tracing::debug!("the ring page said: {t}"),
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Create => self.create(),
                Action::Destroy => self.destroy(),
                Action::Show => self.request_show(),
                Action::Hide => self.hide(),
                Action::Send(p) => {
                    if let Some(v) = &self.view {
                        v.eval(&page::set_state_js(&p));
                    }
                }
            }
        }
        self.arm_settle();
        self.arm_retry();
        self.tell_tray();
    }

    fn create(&mut self) {
        if !self.layer_shell {
            if self.next_id == 0 {
                tracing::warn!(
                    "the compositor has no layer-shell (wlr-layer-shell): Kai shows no ring"
                );
            }
            self.next_id += 1;
            return;
        }
        self.next_id += 1;
        let v = RingView::create(
            self.next_id,
            self.settings.place,
            self.reduced,
            self.tx.clone(),
        );
        tracing::info!("the ring's view is created (hidden)");
        self.view = Some(v);
        self.cancel_ready_timer();
        let (tx, view) = (self.tx.clone(), self.next_id);
        self.ready_timer = Some(glib::timeout_add_local_once(READY_LIMIT, move || {
            let _ = tx.try_send(Event::ReadyTimeout { view });
        }));
        #[cfg(test)]
        self.probe(Probe::Created);
        self.created_at = Some(Instant::now());
    }

    fn destroy(&mut self) {
        self.show_gen += 1;
        self.mapped = false;
        self.stop_feed();
        self.cancel_settle();
        self.cancel_ready_timer();
        if let Some(v) = self.view.take() {
            v.destroy();
            tracing::info!("the ring's view is destroyed");
            #[cfg(test)]
            self.probe(Probe::Destroyed);
        }
    }

    /// The output is resolved first (a Wayland roundtrip and KWin over D-Bus); the surface maps
    /// when it is, unless a hide or destroy came in between. One resolution runs at a time: a
    /// request while one runs gets its own once that ends, started after the request, so it sees
    /// an output that just went or came (an output's removal and the compositor's close come
    /// together, review KAI-5 R5-14).
    fn request_show(&mut self) {
        self.show_gen += 1;
        self.show_at = Some(Instant::now());
        if self.resolving {
            self.resolve_again = true;
            return;
        }
        self.start_resolve();
    }

    /// Resolves the output for the latest show request.
    fn start_resolve(&mut self) {
        self.resolving = true;
        let (show, configured, tx) = (self.show_gen, self.settings.output.clone(), self.tx.clone());
        #[cfg(test)]
        let no_outputs = self.no_outputs;
        glib::MainContext::default().spawn_local(async move {
            #[cfg(test)]
            if no_outputs {
                let resolved = Resolved::no_output();
                let _ = tx.send(Event::Output { show, resolved }).await;
                return;
            }
            let resolved = resolve_output("ring.output", configured).await;
            let _ = tx.send(Event::Output { show, resolved }).await;
        });
    }

    /// Maps (or places again) the surface, only while the ring is to be up: a ring that began to
    /// ease out before its output was known is not mapped (review KAI-5 R5-10). A ring that is up
    /// keeps its output when the output is not resolved, so a stalled query never moves it (R5-5).
    /// With no output listed at all, nothing is mapped and the lifecycle waits for one (review
    /// KAI-6 R6-2): mapping with no output named would leave the choice to a compositor that has
    /// none to give (KWin's internal placeholder, which nobody sees).
    fn on_output(&mut self, show: u64, resolved: Resolved) {
        if show != self.show_gen || !self.life.shown() || self.view.is_none() {
            return;
        }
        if resolved.none_listed {
            let actions = self.life.no_output();
            if !actions.is_empty() {
                tracing::info!(
                    "the compositor lists no output (every monitor is off or unplugged): the \
                     ring is not mapped, and it shows once an output is added"
                );
                self.last_note = None;
                #[cfg(test)]
                self.probe(Probe::NoOutput);
            }
            return self.apply(actions);
        }
        let Some(v) = &self.view else { return };
        let Resolved { monitor, note, .. } = resolved;
        let placing = Placing::of(monitor.is_some(), self.mapped);
        let output = match (&monitor, placing) {
            (Some((m, _)), _) => view::Output::Monitor(m),
            (None, Placing::Keep) => view::Output::Keep,
            (None, _) => view::Output::Compositor,
        };
        let note = placing.line(
            note.as_deref(),
            monitor.as_ref().map(|(_, name)| name.as_str()),
        );
        if note != self.last_note {
            if let Some(n) = &note {
                tracing::info!("ring output: {n}");
            }
            self.last_note = note.clone();
        }
        v.show(output, Layer::for_setting(self.settings.over_fullscreen));
        self.mapped = true;
        #[cfg(test)]
        self.probe(Probe::Mapped {
            ms: ms_since(self.show_at),
            layer: Layer::for_setting(self.settings.over_fullscreen),
            output: monitor.map(|(_, name)| name),
            note,
        });
        self.start_feed();
    }

    fn hide(&mut self) {
        self.show_gen += 1;
        self.mapped = false;
        self.stop_feed();
        self.cancel_settle();
        if let Some(v) = &self.view {
            v.hide();
        }
        #[cfg(test)]
        self.probe(Probe::Hidden);
    }

    /// Reads both taps and feeds the page 30 times a second. The readers live in the timer: they
    /// exist only while the surface is mapped.
    /// - A frame equal to the last one sent is not sent ([`FeedDedup`]): the page's analysers hold
    ///   it already, so the engine draws the same (continuous listening in silence sends nothing).
    ///   A call that failed does not count as sent.
    /// - Back-pressure: a feed is skipped while the last one has not completed (a hung web process
    ///   gets no queue of calls), and the skips are counted in the log when the feed stops.
    fn start_feed(&mut self) {
        self.stop_feed();
        let Some(v) = &self.view else { return };
        let view = v.weak_view();
        let mut output = self.taps.reader(Tap::Output);
        let mut input = self.taps.reader(Tap::Input);
        let period = Duration::from_secs_f64(1.0 / FEED_HZ as f64);
        let in_flight = Rc::new(Cell::new(false));
        let counts = Rc::new(FeedCounts::default());
        self.feed_stats = Some(counts.clone());
        // A new feed starts with a send: a new page, or one shown again, is told the levels once.
        let dedup = Rc::new(RefCell::new(FeedDedup::default()));
        #[cfg(test)]
        let (send_equal, watch, probe) = (
            self.send_equal.clone(),
            self.watch.clone(),
            self.probe.clone(),
        );
        self.feed = Some(glib::timeout_add_local(period, move || {
            let Some(view) = view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if in_flight.get() {
                FeedCounts::bump(&counts.behind);
                return glib::ControlFlow::Continue;
            }
            let (out_levels, in_levels) = (output.read(), input.read());
            #[cfg(test)]
            if let Some(level) = watch.get()
                && in_levels.level >= level
            {
                watch.set(None);
                if let Some(p) = &probe {
                    let _ = p.try_send(Probe::Heard);
                }
            }
            let js = page::feed_js(&out_levels, &in_levels);
            #[cfg(test)]
            let js = if send_equal.get() {
                Some(js)
            } else {
                dedup.borrow_mut().take(js)
            };
            #[cfg(not(test))]
            let js = dedup.borrow_mut().take(js);
            let Some(js) = js else {
                FeedCounts::bump(&counts.same);
                return glib::ControlFlow::Continue;
            };
            in_flight.set(true);
            FeedCounts::bump(&counts.sent);
            let (done, sent) = (in_flight.clone(), dedup.clone());
            view.evaluate_javascript(&js, None, None, gio::Cancellable::NONE, move |r| {
                done.set(false);
                if r.is_err() {
                    sent.borrow_mut().failed();
                }
            });
            glib::ControlFlow::Continue
        }));
    }

    fn stop_feed(&mut self) {
        if let Some(id) = self.feed.take() {
            remove_source(id);
        }
        let Some(c) = self.feed_stats.take() else {
            return;
        };
        let (sent, same, behind) = (c.sent.get(), c.same.get(), c.behind.get());
        tracing::debug!(
            sent,
            same,
            behind,
            "the ring's feed stops: {sent} feeds sent, {same} equal to the last one and not sent"
        );
        if behind > 0 {
            tracing::warn!(
                sent,
                skipped = behind,
                "the ring page fell behind its {FEED_HZ} Hz feed: feeds were skipped while the \
                 last one had not completed"
            );
        }
    }

    fn cancel_ready_timer(&mut self) {
        if let Some(id) = self.ready_timer.take() {
            remove_source(id);
        }
    }

    /// While settling, a fallback for a page that never reports the idle state drawn. A surface
    /// that was never mapped has nothing to ease out (its page is hidden, so it draws nothing and
    /// would never report): it settles at once (review KAI-5 R5-10).
    fn arm_settle(&mut self) {
        let want = self.life.settling_seq();
        if let Some(seq) = want
            && !self.mapped
        {
            tracing::debug!("the ring was not mapped yet: nothing to ease out; it hides now");
            self.cancel_settle();
            let actions = self.life.settled(seq);
            return self.apply(actions);
        }
        if self.settle.as_ref().map(|s| s.0) == want {
            return;
        }
        self.cancel_settle();
        if let Some(seq) = want {
            let tx = self.tx.clone();
            let id = glib::timeout_add_local_once(SETTLE_LIMIT, move || {
                let _ = tx.try_send(Event::SettleTimeout { seq });
            });
            self.settle = Some((seq, id));
        }
    }

    fn cancel_settle(&mut self) {
        if let Some((_, id)) = self.settle.take() {
            remove_source(id);
        }
    }

    /// While waiting after a second crash, the retry timer runs; its wait doubles with each retry
    /// (§9.4, review KAI-5 R5-3).
    fn arm_retry(&mut self) {
        if !self.life.waiting_after_crash() {
            if let Some((_, _, id)) = self.retry.take() {
                remove_source(id);
            }
            return;
        }
        if self.retry.is_some() {
            return;
        }
        let wait = lifecycle::retry_wait(self.retry_first, RETRY_MAX, self.life.retries());
        let tx = self.tx.clone();
        let id = glib::timeout_add_local_once(wait, move || {
            let _ = tx.try_send(Event::Retry);
        });
        let clock = glib::DateTime::now_local()
            .and_then(|t| t.add_seconds(wait.as_secs_f64()))
            .and_then(|t| t.format("%H:%M:%S"))
            .map(|t| t.to_string())
            .unwrap_or_else(|_| format!("{} s", wait.as_secs()));
        self.retry = Some((Instant::now() + wait, clock, id));
    }

    /// The tray's line while the ring is down; told only on a change.
    fn tell_tray(&mut self) {
        let note = self.life.down().map(|d| match d {
            Down::Crashed => {
                let at = self.retry.as_ref().map_or("", |r| r.1.as_str());
                format!(
                    "its web process ended twice, so the ring is down; Kai starts it again at {at} \
                     (retry {}; each wait doubles, up to {} s)",
                    self.life.retries() + 1,
                    RETRY_MAX.as_secs()
                )
            }
            // What Kai saw, not a claim about the outputs (review KAI-6 R6-2, R6-11): a close
            // can come before GDK's event for the output that explains it.
            Down::Closed => "the compositor closed its surface again, and Kai saw no output added \
                             or removed and no turn start since it closed it last, so the ring is \
                             down until one of those"
                .to_owned(),
            Down::NoOutput => "no output is connected, so the ring is down; it shows again when \
                               one is"
                .to_owned(),
        });
        if note == self.note {
            return;
        }
        self.note = note.clone();
        #[cfg(test)]
        self.probe(Probe::Note(note.clone()));
        (self.notes)(note);
    }
}

/// Removes a timer unless it is gone already (a one-shot that fired, a feed that stopped itself):
/// removing a gone source panics.
fn remove_source(id: glib::SourceId) {
    if glib::MainContext::default()
        .find_source_by_id(&id)
        .is_some()
    {
        id.remove();
    }
}

/// What [`resolve_output`] found: the GDK monitor for the ring (or a widget, `crate::widgets`) with
/// its connector, or `None`, and a note on any fallback. The note gives only the cause; `on_output`
/// says what follows ([`Placing::line`]).
pub(crate) struct Resolved {
    pub(crate) monitor: Option<(gdk::Monitor, String)>,
    pub(crate) note: Option<String>,
    /// The compositor listed no output at all (review KAI-6 R6-2).
    pub(crate) none_listed: bool,
}

impl Resolved {
    fn none(note: String) -> Resolved {
        Resolved {
            monitor: None,
            note: Some(note),
            none_listed: false,
        }
    }

    fn no_output() -> Resolved {
        Resolved {
            monitor: None,
            note: None,
            none_listed: true,
        }
    }
}

/// The GDK monitor for the ring (§9.1), or for a widget: `key` names the setting `configured` comes
/// from (`ring.output`, `overlay.output`) in the note.
pub(crate) async fn resolve_output(key: &str, configured: Option<String>) -> Resolved {
    // One query at a time: a compositor that stalls parks one worker, not one per show.
    let Some(query) = outputs::Query::start() else {
        return Resolved::none(
            "an earlier query for the outputs' names still waits for the compositor".into(),
        );
    };
    let answer = glib::future_with_timeout(
        outputs::OUTPUTS_TIMEOUT,
        gio::spawn_blocking(move || query.run()),
    )
    .await;
    let outputs = match answer {
        Ok(Ok(Ok(o))) => o,
        Ok(Ok(Err(e))) => return Resolved::none(format!("the outputs' names are unknown ({e})")),
        Ok(Err(_)) => return Resolved::none("the output query failed".into()),
        Err(_) => {
            return Resolved::none(format!(
                "the compositor did not list its outputs within {} ms",
                outputs::OUTPUTS_TIMEOUT.as_millis()
            ));
        }
    };
    let (names, need_active) = match Listed::of(configured.as_deref(), &outputs) {
        Listed::NoOutput => return Resolved::no_output(),
        Listed::Outputs { names, need_active } => (names, need_active),
    };
    let active = if need_active {
        outputs::kwin_active_output().await
    } else {
        Err("not asked".into())
    };
    let choice = placement::choose_output(
        key,
        configured.as_deref(),
        active.as_deref().map_err(String::as_str),
        &names,
    );
    let monitors = outputs::gdk_monitors();
    let infos: Vec<placement::Monitor> = monitors.iter().map(|(_, i)| i.clone()).collect();
    let Some(name) = choice.name else {
        return Resolved {
            monitor: None,
            note: choice.note,
            none_listed: false,
        };
    };
    match placement::monitor_for(&name, &outputs, &infos) {
        Some(i) => Resolved {
            monitor: Some((monitors[i].0.clone(), name)),
            note: choice.note,
            none_listed: false,
        },
        None => {
            let note = format!("no GDK monitor matches output '{name}'");
            Resolved::none(match choice.note {
                Some(n) => format!("{n}; {note}"),
                None => note,
            })
        }
    }
}
