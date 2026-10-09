//! The approval card on the GTK thread (§11.1, WP19): one layer-shell surface on the OVERLAY layer
//! (WP10's surface code, `overlay::layer_window`), interactive, with the keyboard **on demand**
//! (WP13 (c): KWin moves the keyboard to it the moment it maps, and back when it goes or the owner
//! clicks elsewhere). Its page is Kai's own (`kai://approvals/`, `assets/approvals/`) in a web
//! context of its own, ephemeral and dropped with the card, so no web process is left (K1).
//!
//! **Arming.** The keyboard moves at the map, so a key meant for another window would answer the
//! card. The card answers nothing, neither a key nor a click, until `approvals.arm_ms` has passed
//! with no key and no click reaching it: each key or button press before then starts the wait
//! again, and so does the keyboard arriving (`focus-in`) while the pointer is not on the card; the
//! card says it is not armed yet. The keyboard that arrives while the pointer is on the card came
//! with a click on it (KWin gives an `OnDemand` layer surface the keyboard at its map, which
//! starts the wait anyway, and when it is clicked, before the click itself reaches it), so it does
//! not start the wait again: the first click on an armed card the keyboard had left counts (a
//! click before the arming still starts the wait again, and the card says it is not armed). A new call on the card arms again. Enter approves and Esc declines; a key held
//! down (its repeats) counts once, at its press, and only a press that came after the card armed.
//! **A key that reached the card before it armed for a call** takes Enter's approval of that call
//! away: then Ctrl+Enter, a click or (a call of Kai's own) the voice approves, Esc still declines,
//! and the card says so. Plain Enter also waits until the end of the arguments was in view (they
//! scroll when the output is too short; the page shows "more below"); Ctrl+Enter does not. Keys
//! are taken by the window itself, before the page: the page gets none. The card tells the
//! approvals when it armed and when it waits again for a call ([`CardEvent`]): the spoken answer
//! counts only while it is armed.
//!
//! **The keyboard flag** ([`super::keyboard`]): set on `focus-in` only, cleared on `focus-out` and
//! when the card goes, so Kai's own input tools never type into the card, and a card KWin never
//! gives the keyboard does not hold Kai's input tools back. (Keys that reach the card between its
//! map and its `focus-in` come before it armed: they answer nothing.)
//!
//! **A card that cannot show** (no layer-shell, its page ended a second time for the same call)
//! says so back ([`CardEvent::Lost`]): the approvals then leave that call to lmgw, say so, and do
//! not hold back the voice for it.
//!
//! The commands come from the voice's thread through [`send`] (registered by [`start`] on the GTK
//! thread; nothing is shown while it is not up); the answers go back on the channel each
//! [`CardCmd::Show`] carries.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{KeyboardMode, LayerShell};
use javascriptcore::ValueExt;
use serde_json::Value;
use tokio::sync::mpsc;
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt, PolicyDecisionType,
    SecurityManagerExt, URIRequestExt, URISchemeRequestExt, URISchemeResponse,
    URISchemeResponseExt, UserContentManager, UserContentManagerExt, WebContext, WebContextExt,
    WebView, WebViewExt, WebViewExtManual,
};

use super::card::{CardAnswer, CardCmd, CardEvent, CardModel, Look};
use super::keyboard::Holder;
use crate::overlay::{Resolved, layer_window, place_window, resolve_output};

const SCHEME: &str = "kai";
const HOST: &str = "approvals";
const PAGE: &str = "kai://approvals/card.html";
/// The page's own files only; no inline script or style.
const CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'self'; base-uri 'none'; form-action 'none'";

const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/card.html",
        "text/html",
        include_bytes!("../../assets/approvals/card.html"),
    ),
    (
        "/card.css",
        "text/css",
        include_bytes!("../../assets/approvals/card.css"),
    ),
    (
        "/card.js",
        "text/javascript",
        include_bytes!("../../assets/approvals/card.js"),
    ),
];

/// The content type and body for `kai://approvals<path>`; `None` is a 404.
pub fn asset(host: &str, path: &str) -> Option<(&'static str, &'static [u8])> {
    if host != HOST {
        return None;
    }
    ASSETS
        .iter()
        .find(|(p, _, _)| *p == path)
        .map(|(_, mime, body)| (*mime, *body))
}

static REMOTE: Mutex<Option<async_channel::Sender<CardCmd>>> = Mutex::new(None);

/// Sends `cmd` to the card on the GTK thread, from any thread; dropped while the card's side is not
/// up (no GTK: nothing is shown). The tests' cards stay on their own thread ([`test_cards`]).
pub fn send(cmd: CardCmd) {
    // A test thread that asked for its cards ([`test_cards`]) keeps them; the nested live check
    // sends to its card.
    #[cfg(test)]
    let cmd = {
        let mut cmd = Some(cmd);
        TEST_CARDS.with(|c| {
            if let Some(v) = c.borrow_mut().as_mut() {
                v.extend(cmd.take());
            }
        });
        match cmd {
            Some(c) => c,
            None => return,
        }
    };
    let remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    match remote.as_ref() {
        Some(tx) => {
            let _ = tx.try_send(cmd);
        }
        None => tracing::debug!(?cmd, "approvals: no GTK side; the card is not shown"),
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CARDS: RefCell<Option<Vec<CardCmd>>> = const { RefCell::new(None) };
}

/// What the card was sent on this thread since the last call; the first call starts keeping them
/// (a test's core and voice run on its own thread).
#[cfg(test)]
pub fn test_cards() -> Vec<CardCmd> {
    TEST_CARDS.with(|c| std::mem::take(c.borrow_mut().get_or_insert_with(Vec::new)))
}

/// What the page posts.
#[derive(Debug, Clone, PartialEq)]
pub enum PageMsg {
    Ready,
    /// The card's content height, in CSS pixels.
    Size(u32),
    /// A click on Approve or Decline for `call`.
    Answer {
        call: String,
        approve: bool,
    },
    /// Whether the end of `call`'s arguments is in view now.
    Seen {
        call: String,
        all: bool,
    },
    Error(String),
    Other,
}

pub fn parse(text: &str) -> PageMsg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return PageMsg::Other;
    };
    match v.get("ev").and_then(Value::as_str) {
        Some("ready") => PageMsg::Ready,
        Some("size") => v
            .get("h")
            .and_then(Value::as_u64)
            .map_or(PageMsg::Other, |h| PageMsg::Size(h as u32)),
        Some("answer") => match (
            v.get("call").and_then(Value::as_str),
            v.get("approve").and_then(Value::as_bool),
        ) {
            (Some(call), Some(approve)) => PageMsg::Answer {
                call: call.to_owned(),
                approve,
            },
            _ => PageMsg::Other,
        },
        Some("seen") => match (
            v.get("call").and_then(Value::as_str),
            v.get("all").and_then(Value::as_bool),
        ) {
            (Some(call), Some(all)) => PageMsg::Seen {
                call: call.to_owned(),
                all,
            },
            _ => PageMsg::Other,
        },
        Some("error") => PageMsg::Error(
            v.get("msg")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => PageMsg::Other,
    }
}

/// The arming (module doc), as a clock the tests drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arming {
    arm: Duration,
    /// When the card arms; `None` until it is mapped.
    at: Option<Instant>,
    /// Keys down now (their repeats count once).
    down: HashSet<u32>,
    /// A key reached the card before it armed for this call: Enter alone does not approve it.
    typed: bool,
    /// The end of this call's arguments was in view.
    seen: bool,
}

/// One key press as the card reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: u32,
    pub enter: bool,
    pub escape: bool,
    /// Ctrl was held.
    pub ctrl: bool,
}

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAnswer {
    Approve,
    Decline,
    /// Not armed yet: the wait started again.
    Rearmed,
    /// Enter, after a key reached the card before it armed for this call: Ctrl+Enter approves.
    Typed,
    /// Enter before the end of the arguments was in view: Ctrl+Enter approves.
    Unseen,
    /// Any other key, or a repeat.
    Nothing,
}

impl Arming {
    pub fn new(arm: Duration) -> Self {
        Self {
            arm,
            at: None,
            down: HashSet::new(),
            typed: false,
            seen: false,
        }
    }

    /// The card mapped, or shows a new call: no key reached it for this call yet, its arguments
    /// were not seen, and the wait starts now.
    pub fn new_call(&mut self, now: Instant) {
        self.typed = false;
        self.seen = false;
        self.start(now);
    }

    /// The wait starts again now.
    pub fn start(&mut self, now: Instant) {
        self.at = Some(now + self.arm);
    }

    /// The keyboard came to the mapped card (`focus-in`); `pointer_on`: the pointer is on the card,
    /// so the keyboard came with a click on it (module doc). Otherwise the wait starts again
    /// (`true`).
    pub fn focus_in(&mut self, now: Instant, pointer_on: bool) -> bool {
        if pointer_on || self.at.is_none() {
            return false;
        }
        self.start(now);
        true
    }

    pub fn armed(&self, now: Instant) -> bool {
        self.at.is_some_and(|at| now >= at)
    }

    /// When the card arms, while it waits.
    pub fn until(&self, now: Instant) -> Option<Duration> {
        self.at.filter(|at| *at > now).map(|at| at - now)
    }

    pub fn typed(&self) -> bool {
        self.typed
    }

    /// The page says whether the end of the arguments is in view; once it was, it counts.
    pub fn seen(&mut self, all: bool) {
        self.seen |= all;
    }

    /// A button press reached the card: before it armed, the wait starts again (`true`).
    pub fn button(&mut self, now: Instant) -> bool {
        if self.at.is_some() && !self.armed(now) {
            self.start(now);
            return true;
        }
        false
    }

    /// A key press reached the card.
    pub fn press(&mut self, p: Press, now: Instant) -> KeyAnswer {
        let repeat = !self.down.insert(p.key);
        if !self.armed(now) {
            if self.at.is_some() {
                self.typed = true;
                self.start(now);
            }
            return KeyAnswer::Rearmed;
        }
        if repeat {
            return KeyAnswer::Nothing;
        }
        match (p.enter, p.escape) {
            (_, true) => KeyAnswer::Decline,
            (true, _) if p.ctrl => KeyAnswer::Approve,
            (true, _) if self.typed => KeyAnswer::Typed,
            (true, _) if !self.seen => KeyAnswer::Unseen,
            (true, _) => KeyAnswer::Approve,
            _ => KeyAnswer::Nothing,
        }
    }

    pub fn release(&mut self, key: u32) {
        self.down.remove(&key);
    }
}

enum Event {
    Cmd(CardCmd),
    Page {
        no: u64,
        msg: PageMsg,
    },
    Output {
        no: u64,
        resolved: Resolved,
    },
    Key {
        no: u64,
        press: Press,
    },
    KeyUp {
        no: u64,
        key: u32,
    },
    /// A button press reached the window (before the page sees it).
    Button {
        no: u64,
    },
    /// The keyboard came to the card (`true`) or left it.
    Focus {
        no: u64,
        on: bool,
    },
    /// The pointer came onto the card (`true`) or left it (moves onto its page's own window are
    /// no leave).
    Pointer {
        no: u64,
        on: bool,
    },
    /// The arming's one timer fired.
    Tick {
        no: u64,
    },
    Closed {
        no: u64,
    },
    Crashed {
        no: u64,
        reason: String,
    },
    Quit,
}

/// The card while it exists.
struct Card {
    no: u64,
    window: gtk::Window,
    view: WebView,
    _ucm: UserContentManager,
    _context: WebContext,
    ready: bool,
    /// The button presses, seen in the capture phase before the page's own handlers.
    _gesture: gtk::GestureMultiPress,
    model: CardModel,
    look: Look,
    events: mpsc::UnboundedSender<CardEvent>,
    resolved: Option<Option<gdk::Monitor>>,
    height: Option<u32>,
    mapped: bool,
    /// The pointer is on the card (`Event::Pointer`).
    pointer_on: bool,
    arming: Arming,
    /// The arming's timer is set (one at a time; it sets itself again while the wait moved).
    timer: bool,
    /// The approvals were told this call armed.
    said_armed: bool,
    /// The page's web process ended once for this call: one new card, then no more.
    crashed_for: Option<String>,
}

/// The card's side, running on the GTK thread until [`CardHandle::quit`].
pub struct CardHandle {
    tx: async_channel::Sender<Event>,
    task: glib::JoinHandle<()>,
}

impl CardHandle {
    /// §2's quit: [`send`] reaches nothing any more, and the card goes.
    pub async fn quit(self) {
        REMOTE.lock().unwrap_or_else(|e| e.into_inner()).take();
        let _ = self.tx.send(Event::Quit).await;
        let _ = self.task.await;
    }
}

/// Starts the card's side on the GTK thread: [`send`] reaches it from now on.
pub fn start() -> CardHandle {
    let (tx, rx) = async_channel::unbounded::<Event>();
    let (remote, from_remote) = async_channel::unbounded::<CardCmd>();
    *REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(remote);
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
        card: None,
        no: 0,
        tx: tx.clone(),
        keyboard: Holder::global(),
    };
    let task = glib::MainContext::default().spawn_local(host.run(rx));
    CardHandle { tx, task }
}

struct Host {
    layer_shell: bool,
    card: Option<Card>,
    no: u64,
    tx: async_channel::Sender<Event>,
    /// "The card holds the keyboard" ([`super::keyboard`]).
    keyboard: Holder,
}

impl Host {
    async fn run(mut self, rx: async_channel::Receiver<Event>) {
        while let Ok(ev) = rx.recv().await {
            if matches!(ev, Event::Quit) {
                break;
            }
            self.on(ev);
        }
        self.hide();
    }

    fn on(&mut self, ev: Event) {
        let no = self.card.as_ref().map(|c| c.no);
        match ev {
            Event::Cmd(CardCmd::Show {
                model,
                look,
                events,
            }) => self.show(*model, look, events),
            Event::Cmd(CardCmd::Hide) => self.hide(),
            Event::Page { no: g, msg } if Some(g) == no => self.on_page(msg),
            Event::Output { no: g, resolved } if Some(g) == no => {
                match &resolved.note {
                    Some(n) => tracing::info!("approvals: the card's output: {n}"),
                    None => tracing::debug!(
                        output = ?resolved.monitor.as_ref().map(|(_, name)| name),
                        "approvals: the card's output"
                    ),
                }
                let card = self.card.as_mut().expect("checked");
                card.resolved = Some(resolved.monitor.map(|(m, _)| m));
                self.place();
            }
            Event::Key { no: g, press } if Some(g) == no => self.on_key(press),
            Event::KeyUp { no: g, key } if Some(g) == no => {
                if let Some(c) = self.card.as_mut() {
                    c.arming.release(key);
                }
            }
            Event::Button { no: g } if Some(g) == no => {
                let card = self.card.as_mut().expect("checked");
                if card.arming.button(Instant::now()) {
                    tracing::debug!(
                        "approvals: a click before the card armed; the wait starts again"
                    );
                    Self::unarmed(&self.tx, card);
                }
            }
            Event::Focus { no: g, on } if Some(g) == no => {
                self.keyboard.set(on);
                let card = self.card.as_mut().expect("checked");
                if !on {
                    tracing::debug!("approvals: the keyboard left the card");
                } else if card.mapped && card.arming.focus_in(Instant::now(), card.pointer_on) {
                    tracing::info!(call = %card.model.call, "approvals: the keyboard came to the card; it arms again");
                    Self::unarmed(&self.tx, card);
                } else if card.mapped {
                    tracing::info!(call = %card.model.call, "approvals: the keyboard came with a click on the card; the arming stays");
                }
            }
            Event::Pointer { no: g, on } if Some(g) == no => {
                if let Some(c) = self.card.as_mut() {
                    c.pointer_on = on;
                }
            }
            Event::Tick { no: g } if Some(g) == no => self.on_tick(),
            Event::Closed { no: g } if Some(g) == no => {
                // The compositor closed the surface (its output went away): a new card, placed
                // again.
                tracing::info!("approvals: the compositor closed the card; it is shown again");
                self.recreate();
            }
            Event::Crashed { no: g, reason } if Some(g) == no => {
                let card = self.card.as_ref().expect("checked");
                if card.crashed_for.as_deref() == Some(card.model.call.as_str()) {
                    tracing::warn!(%reason, "approvals: the card's page ended again; the card goes (the call is left to lmgw)");
                    let (call, events) = (card.model.call.clone(), card.events.clone());
                    self.hide();
                    let _ = events.send(CardEvent::Lost {
                        call,
                        why: format!("its page ended twice ({reason})"),
                    });
                } else {
                    tracing::warn!(%reason, "approvals: the card's page ended; a new card");
                    let call = card.model.call.clone();
                    self.recreate();
                    if let Some(c) = self.card.as_mut() {
                        c.crashed_for = Some(call);
                    }
                }
            }
            // An event of a card that is gone.
            _ => {}
        }
    }

    fn show(&mut self, model: CardModel, look: Look, events: mpsc::UnboundedSender<CardEvent>) {
        if !self.layer_shell {
            tracing::warn!(
                "approvals: the compositor has no layer-shell; the card cannot show (the call is \
                 left to lmgw)"
            );
            let _ = events.send(CardEvent::Lost {
                call: model.call,
                why: "the compositor offers no layer-shell".into(),
            });
            return;
        }
        match self.card.as_mut() {
            Some(card) => {
                let new_call = card.model.call != model.call;
                card.model = model;
                card.look = look;
                card.events = events;
                if new_call {
                    tracing::info!(call = %card.model.call, "approvals: the card shows the next call; it arms again");
                    card.crashed_for = None;
                    if card.mapped {
                        card.arming.new_call(Instant::now());
                        card.said_armed = false;
                        Self::arm_timer(&self.tx, card);
                    }
                }
                Self::push(card);
            }
            None => self.create(model, look, events),
        }
    }

    fn create(&mut self, model: CardModel, look: Look, events: mpsc::UnboundedSender<CardEvent>) {
        self.no += 1;
        let no = self.no;
        tracing::info!(call = %model.call, "approvals: showing the card");
        let context = WebContext::new_ephemeral();
        if let Some(sm) = context.security_manager() {
            sm.register_uri_scheme_as_secure(SCHEME);
        }
        context.register_uri_scheme(SCHEME, |request| {
            let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
            let found = url::Url::parse(&uri)
                .ok()
                .and_then(|u| asset(u.host_str().unwrap_or_default(), u.path()));
            match found {
                Some((mime, body)) => {
                    let bytes = glib::Bytes::from_static(body);
                    let stream = gio::MemoryInputStream::from_bytes(&bytes);
                    let response = URISchemeResponse::new(&stream, bytes.len() as i64);
                    response.set_content_type(mime);
                    // The headers replace the content type: it goes in them too, or a script or
                    // stylesheet under nosniff is refused (the ring's rule).
                    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
                    headers.append("Content-Type", mime);
                    headers.append("Content-Security-Policy", CSP);
                    headers.append("X-Content-Type-Options", "nosniff");
                    response.set_http_headers(headers);
                    request.finish_with_response(&response);
                }
                None => request.finish_error(&mut glib::Error::new(
                    gio::IOErrorEnum::NotFound,
                    "not served",
                )),
            }
        });
        let ucm = UserContentManager::new();
        ucm.register_script_message_handler("kai");
        let tx = self.tx.clone();
        ucm.connect_script_message_received(Some("kai"), move |_, result| {
            let text = result
                .js_value()
                .map(|v| v.to_str().to_string())
                .unwrap_or_default();
            let _ = tx.try_send(Event::Page {
                no,
                msg: parse(&text),
            });
        });
        let view = WebView::new_with_context_and_user_content_manager(&context, &ucm);
        view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        if let Some(s) = WebViewExt::settings(&view)
            && let Err(e) = crate::widgets::view::harden(&s)
        {
            tracing::warn!("approvals: the card's view: {e}");
        }
        let tx = self.tx.clone();
        view.connect_web_process_terminated(move |_, reason| {
            let _ = tx.try_send(Event::Crashed {
                no,
                reason: format!("{reason:?}"),
            });
        });
        view.connect_load_changed(|_, ev| {
            tracing::debug!(?ev, "approvals: the card's page loads");
        });
        view.connect_load_failed(|_, _, _, error| {
            tracing::warn!("approvals: the card's page did not load: {error}");
            false
        });
        // The page never navigates and opens no window.
        view.connect_decide_policy(|_, decision, kind| {
            if !matches!(
                kind,
                PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
            ) {
                return false;
            }
            let uri = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(|d| d.navigation_action())
                .and_then(|a| a.request())
                .and_then(|r| r.uri())
                .map(|u| u.to_string())
                .unwrap_or_default();
            if kind == PolicyDecisionType::NavigationAction && uri == PAGE {
                return false;
            }
            tracing::warn!("approvals: the card's page tried to leave its page; ignored");
            decision.ignore();
            true
        });
        view.connect_context_menu(|_, _, _, _| true);
        view.connect_script_dialog(|_, _| true);
        view.connect_print(|_, _| true);
        view.connect_permission_request(|_, r| {
            use webkit2gtk::PermissionRequestExt;
            r.deny();
            true
        });

        let tx = self.tx.clone();
        let window = layer_window("Kai approval", "kai-approval", move || {
            let _ = tx.try_send(Event::Closed { no });
        });
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_layer(gtk_layer_shell::Layer::Overlay);
        place_window(&window, look.anchor, look.margin);
        window.set_default_size(look.width as i32, 1);
        window.set_size_request(look.width as i32, 1);
        let tx = self.tx.clone();
        window.connect_key_press_event(move |_, ev| {
            let key = ev.keyval();
            let _ = tx.try_send(Event::Key {
                no,
                press: Press {
                    key: *key,
                    enter: matches!(
                        key,
                        gdk::keys::constants::Return | gdk::keys::constants::KP_Enter
                    ),
                    escape: key == gdk::keys::constants::Escape,
                    ctrl: ev.state().contains(gdk::ModifierType::CONTROL_MASK),
                },
            });
            // Keys are the window's: the page gets none.
            glib::Propagation::Stop
        });
        let tx = self.tx.clone();
        window.connect_focus_in_event(move |_, _| {
            let _ = tx.try_send(Event::Focus { no, on: true });
            glib::Propagation::Proceed
        });
        let tx = self.tx.clone();
        window.connect_focus_out_event(move |_, _| {
            let _ = tx.try_send(Event::Focus { no, on: false });
            glib::Propagation::Proceed
        });
        // Where the pointer is: the keyboard arriving with it on the card came with a click
        // (module doc). A move onto the page's own window is the toplevel's `Inferior` leave.
        window.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        let tx = self.tx.clone();
        window.connect_enter_notify_event(move |_, _| {
            let _ = tx.try_send(Event::Pointer { no, on: true });
            glib::Propagation::Proceed
        });
        let tx = self.tx.clone();
        window.connect_leave_notify_event(move |_, ev| {
            if ev.detail() != gdk::NotifyType::Inferior {
                let _ = tx.try_send(Event::Pointer { no, on: false });
            }
            glib::Propagation::Proceed
        });
        // Every button press, before the page's own handlers see it (a click before the card
        // armed starts the wait again; the page's click then answers nothing).
        let gesture = gtk::GestureMultiPress::new(&window);
        gesture.set_button(0);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = self.tx.clone();
        gesture.connect_pressed(move |_, _, _, _| {
            let _ = tx.try_send(Event::Button { no });
        });
        let tx = self.tx.clone();
        window.connect_key_release_event(move |_, ev| {
            let _ = tx.try_send(Event::KeyUp {
                no,
                key: *ev.keyval(),
            });
            glib::Propagation::Stop
        });
        window.add(&view);
        crate::widgets::input::keep(&window, true);
        view.load_uri(PAGE);

        let (tx, configured) = (self.tx.clone(), look.output.clone());
        glib::MainContext::default().spawn_local(async move {
            let resolved = resolve_output("overlay.output", configured).await;
            let _ = tx.send(Event::Output { no, resolved }).await;
        });
        self.card = Some(Card {
            no,
            window,
            view,
            _ucm: ucm,
            _context: context,
            _gesture: gesture,
            ready: false,
            arming: Arming::new(look.arm),
            model,
            look,
            events,
            resolved: None,
            height: None,
            mapped: false,
            pointer_on: false,
            timer: false,
            said_armed: false,
            crashed_for: None,
        });
    }

    fn recreate(&mut self) {
        if let Some(card) = self.card.take() {
            let (model, look, events) =
                (card.model.clone(), card.look.clone(), card.events.clone());
            let crashed_for = card.crashed_for.clone();
            if card.said_armed {
                let _ = events.send(CardEvent::Unarmed {
                    call: card.model.call.clone(),
                });
            }
            self.destroy(card);
            self.create(model, look, events);
            if let Some(c) = self.card.as_mut() {
                c.crashed_for = crashed_for;
            }
        }
    }

    fn hide(&mut self) {
        if let Some(card) = self.card.take() {
            tracing::info!(call = %card.model.call, "approvals: the card goes");
            self.destroy(card);
        }
    }

    fn destroy(&self, card: Card) {
        self.keyboard.set(false);
        // SAFETY: Kai holds no other reference to the window, and the view's and window's handlers
        // capture only channel senders, so nothing uses either after this.
        unsafe { card.window.destroy() };
    }

    fn on_page(&mut self, msg: PageMsg) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        tracing::debug!(?msg, "approvals: the card's page");
        match msg {
            PageMsg::Ready => {
                card.ready = true;
                Self::push(card);
            }
            PageMsg::Size(h) => {
                card.height = Some(h.max(1));
                self.place();
            }
            PageMsg::Answer { call, approve } => {
                let now = Instant::now();
                if call != card.model.call {
                    tracing::debug!("approvals: a click for a call the card no longer shows");
                } else if card.model.busy.is_some() {
                    tracing::debug!("approvals: a click while the spoken answer is heard; ignored");
                } else if !card.arming.armed(now) {
                    tracing::info!("approvals: a click before the card armed; ignored");
                } else {
                    tracing::info!(call = %call, approve, "approvals: answered on the card (click)");
                    let _ = card
                        .events
                        .send(CardEvent::Answer(CardAnswer { call, approve }));
                }
            }
            PageMsg::Seen { call, all } => {
                if call == card.model.call {
                    card.arming.seen(all);
                }
            }
            PageMsg::Error(e) => tracing::warn!("approvals: the card's page: {e}"),
            PageMsg::Other => {
                tracing::debug!("approvals: a message the card's page should not send")
            }
        }
    }

    fn on_key(&mut self, press: Press) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        let now = Instant::now();
        match card.arming.press(press, now) {
            KeyAnswer::Rearmed => {
                tracing::debug!(
                    "approvals: a key before the card armed; the wait starts again, and Enter alone no longer approves this call"
                );
                Self::unarmed(&self.tx, card);
            }
            KeyAnswer::Typed => tracing::info!(
                "approvals: Enter after a key reached the card before it armed; not an approval \
                 (Ctrl+Enter, a click or the voice)"
            ),
            KeyAnswer::Unseen => tracing::info!(
                "approvals: Enter before the end of the arguments was in view; not an approval \
                 (scroll, or Ctrl+Enter)"
            ),
            KeyAnswer::Nothing => {}
            a @ (KeyAnswer::Approve | KeyAnswer::Decline) => {
                if card.model.busy.is_some() {
                    tracing::debug!("approvals: a key while the spoken answer is heard; ignored");
                    return;
                }
                let approve = a == KeyAnswer::Approve;
                tracing::info!(call = %card.model.call, approve, ctrl = press.ctrl, "approvals: answered on the card (key)");
                let _ = card.events.send(CardEvent::Answer(CardAnswer {
                    call: card.model.call.clone(),
                    approve,
                }));
            }
        }
    }

    /// The arming's timer fired: armed now, or set again for the wait that moved.
    fn on_tick(&mut self) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        card.timer = false;
        if card.arming.armed(Instant::now()) {
            if !card.said_armed {
                card.said_armed = true;
                Self::eval(card, &format!("__kai.arm(true,{})", card.arming.typed()));
                let _ = card.events.send(CardEvent::Armed {
                    call: card.model.call.clone(),
                });
            }
        } else {
            Self::arm_timer(&self.tx, card);
        }
    }

    /// Sizes the card to its content within its output (less the margins), and maps it once the
    /// output is resolved and the page measured itself.
    fn place(&mut self) {
        let tx = self.tx.clone();
        let Some(card) = self.card.as_mut() else {
            return;
        };
        let (Some(monitor), Some(h)) = (card.resolved.clone(), card.height) else {
            return;
        };
        let room = monitor
            .as_ref()
            .map(|m| m.workarea().height() - 2 * card.look.margin as i32)
            .filter(|r| *r > 0);
        let height = match room {
            Some(r) => (h as i32).min(r),
            None => h as i32,
        };
        card.window.set_size_request(card.look.width as i32, height);
        card.window.resize(card.look.width as i32, height);
        if !card.mapped {
            match &monitor {
                Some(m) => card.window.set_monitor(m),
                None => {
                    use gtk::glib::translate::ToGlibPtr;
                    // SAFETY: a valid window; NULL is the documented "let the compositor choose".
                    unsafe {
                        gtk_layer_shell_sys::gtk_layer_set_monitor(
                            card.window.to_glib_none().0,
                            std::ptr::null_mut(),
                        );
                    }
                }
            }
            card.window.show_all();
            card.mapped = true;
            // KWin moves the keyboard to the card as it maps (`OnDemand`); the keyboard flag waits
            // for `focus-in` to say so (module doc).
            tracing::info!(call = %card.model.call, height, "approvals: the card is up; it arms");
            card.arming.new_call(Instant::now());
            card.said_armed = false;
            Self::arm_timer(&tx, card);
            Self::push(card);
        }
    }

    /// The wait started again (the arming moved it): the page and the approvals are told, and
    /// the timer runs.
    fn unarmed(tx: &async_channel::Sender<Event>, card: &mut Card) {
        Self::eval(card, &format!("__kai.arm(false,{})", card.arming.typed()));
        if std::mem::take(&mut card.said_armed) {
            let _ = card.events.send(CardEvent::Unarmed {
                call: card.model.call.clone(),
            });
        }
        Self::arm_timer(tx, card);
    }

    /// The arming's one timer, at the time the card arms now (a later key moves that time; the
    /// timer then sets itself again, [`Self::on_tick`]).
    fn arm_timer(tx: &async_channel::Sender<Event>, card: &mut Card) {
        if card.timer {
            return;
        }
        card.timer = true;
        let wait = card.arming.until(Instant::now()).unwrap_or_default();
        let (tx, no) = (tx.clone(), card.no);
        glib::timeout_add_local_once(wait, move || {
            let _ = tx.try_send(Event::Tick { no });
        });
    }

    /// The model into the page.
    fn push(card: &Card) {
        if !card.ready {
            return;
        }
        let model = serde_json::to_string(&card.model).unwrap_or_else(|_| "null".into());
        let armed = card.mapped && card.arming.armed(Instant::now());
        Self::eval(
            card,
            &format!(
                "__kai.show({model},{armed},{},{})",
                card.look.width,
                card.arming.typed()
            ),
        );
    }

    fn eval(card: &Card, js: &str) {
        card.view
            .evaluate_javascript(js, None, None, gio::Cancellable::NONE, |r| {
                if let Err(e) = r {
                    tracing::debug!("approvals: the card's page: {e}");
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTER: u32 = 0xff0d;
    const ESC: u32 = 0xff1b;
    const A: u32 = 0x61;

    fn key(key: u32) -> Press {
        Press {
            key,
            enter: key == ENTER,
            escape: key == ESC,
            ctrl: false,
        }
    }

    fn ctrl_enter() -> Press {
        Press {
            ctrl: true,
            ..key(ENTER)
        }
    }

    #[test]
    fn keys_count_only_after_the_card_armed_with_no_key_meanwhile() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Arming::new(Duration::from_millis(600));
        // Not mapped yet: nothing counts, nothing starts.
        assert_eq!(a.press(key(ENTER), ms(0)), KeyAnswer::Rearmed);
        a.release(ENTER);
        assert!(!a.armed(ms(10_000)));
        a.new_call(ms(0));
        a.seen(true);
        // No key before it armed: Enter approves, its repeats count once, Esc declines.
        assert!(!a.armed(ms(599)));
        assert!(a.armed(ms(600)));
        assert_eq!(a.until(ms(400)), Some(Duration::from_millis(200)));
        assert_eq!(a.press(key(ENTER), ms(600)), KeyAnswer::Approve);
        assert_eq!(a.press(key(ENTER), ms(630)), KeyAnswer::Nothing);
        a.release(ENTER);
        assert_eq!(a.press(key(ESC), ms(700)), KeyAnswer::Decline);
        a.release(ESC);
        assert_eq!(a.press(key(A), ms(800)), KeyAnswer::Nothing);
        a.release(A);

        // A burst of typing meant for another window keeps it unarmed, and takes Enter's
        // approval of this call away.
        let mut b = Arming::new(Duration::from_millis(600));
        b.new_call(ms(0));
        b.seen(true);
        for t in [100, 300, 500, 800, 1100] {
            assert_eq!(b.press(key(A), ms(t)), KeyAnswer::Rearmed, "{t}");
            b.release(A);
        }
        assert!(b.typed());
        assert!(!b.armed(ms(1699)));
        assert!(b.armed(ms(1700)), "600 ms with no key");
        assert_eq!(b.press(key(ENTER), ms(1700)), KeyAnswer::Typed);
        b.release(ENTER);
        assert_eq!(
            b.press(key(ESC), ms(1750)),
            KeyAnswer::Decline,
            "Esc still declines"
        );
        b.release(ESC);
        assert_eq!(
            b.press(ctrl_enter(), ms(1800)),
            KeyAnswer::Approve,
            "the chord approves"
        );
        b.release(ENTER);
        // The next call starts clean.
        b.new_call(ms(2000));
        b.seen(true);
        assert!(!b.typed());
        assert_eq!(b.press(key(ENTER), ms(2600)), KeyAnswer::Approve);
        b.release(ENTER);

        // A key held down from before the card armed counts not even at its repeats; its next
        // press is Enter after a key before arming.
        let mut c = Arming::new(Duration::from_millis(600));
        c.new_call(ms(0));
        c.seen(true);
        assert_eq!(c.press(key(ENTER), ms(50)), KeyAnswer::Rearmed);
        assert_eq!(c.press(key(ENTER), ms(5000)), KeyAnswer::Nothing);
        c.release(ENTER);
        assert_eq!(c.press(key(ENTER), ms(5100)), KeyAnswer::Typed);

        // 0: armed at once.
        let mut d = Arming::new(Duration::ZERO);
        d.new_call(ms(0));
        assert_eq!(d.press(key(ESC), ms(0)), KeyAnswer::Decline);
    }

    /// The keyboard arriving and a click before the card armed start the wait again; neither
    /// takes Enter away.
    #[test]
    fn focus_and_clicks_rearm() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Arming::new(Duration::from_millis(600));
        a.new_call(ms(0));
        a.seen(true);
        // The keyboard came at 500 (focus-in): the wait counts from there.
        a.start(ms(500));
        assert!(!a.armed(ms(1000)));
        assert!(a.button(ms(900)), "a click before it armed");
        assert!(!a.armed(ms(1400)));
        assert!(a.armed(ms(1500)));
        assert!(!a.button(ms(1600)), "a click once armed is a click");
        assert!(!a.typed());
        assert_eq!(a.press(key(ENTER), ms(1600)), KeyAnswer::Approve);
    }

    /// L1: the keyboard arriving with the pointer on the card came with a click on it: the arming
    /// stays, so the first click on an armed card counts. With the pointer elsewhere it starts the
    /// wait again; before the map nothing arms.
    #[test]
    fn the_keyboard_that_comes_with_a_click_keeps_the_arming() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Arming::new(Duration::from_millis(600));
        assert!(!a.focus_in(ms(0), false), "not mapped yet");
        assert!(!a.armed(ms(0)));
        a.new_call(ms(0));
        // Armed; the keyboard left (a click elsewhere) and comes back with a click on the card.
        assert!(a.armed(ms(700)));
        assert!(!a.button(ms(5000)), "the click's press: armed, it counts");
        assert!(!a.focus_in(ms(5001), true));
        assert!(a.armed(ms(5002)), "the click's answer counts");
        // The keyboard arriving with the pointer elsewhere: the wait starts again.
        assert!(a.focus_in(ms(6000), false));
        assert!(!a.armed(ms(6599)));
        assert!(a.armed(ms(6600)));
    }

    /// Plain Enter waits until the end of the arguments was in view; Ctrl+Enter does not.
    #[test]
    fn enter_waits_for_the_end_of_the_arguments() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut a = Arming::new(Duration::ZERO);
        a.new_call(ms(0));
        a.seen(false);
        assert_eq!(a.press(key(ENTER), ms(0)), KeyAnswer::Unseen);
        a.release(ENTER);
        a.seen(true);
        a.seen(false);
        assert_eq!(
            a.press(key(ENTER), ms(1)),
            KeyAnswer::Approve,
            "seen once counts"
        );
        a.release(ENTER);
        a.new_call(ms(2));
        assert_eq!(a.press(ctrl_enter(), ms(2)), KeyAnswer::Approve);
    }

    #[test]
    fn the_page_and_its_messages() {
        assert_eq!(
            asset("approvals", "/card.html").map(|a| a.0),
            Some("text/html")
        );
        assert!(asset("approvals", "/card.js").is_some());
        assert!(
            asset("app", "/card.js").is_none(),
            "the ring's host is not the card's"
        );
        assert!(asset("approvals", "/../ring.html").is_none());
        assert_eq!(parse(r#"{"ev":"ready"}"#), PageMsg::Ready);
        assert_eq!(parse(r#"{"ev":"size","h":312}"#), PageMsg::Size(312));
        assert_eq!(
            parse(r#"{"ev":"answer","call":"mcpr_1","approve":false}"#),
            PageMsg::Answer {
                call: "mcpr_1".into(),
                approve: false
            }
        );
        assert_eq!(parse(r#"{"ev":"answer","call":"mcpr_1"}"#), PageMsg::Other);
        assert_eq!(
            parse(r#"{"ev":"seen","call":"mcpr_1","all":true}"#),
            PageMsg::Seen {
                call: "mcpr_1".into(),
                all: true
            }
        );
        assert_eq!(parse("x"), PageMsg::Other);
        // The page puts texts in as text only.
        let js = std::str::from_utf8(asset("approvals", "/card.js").unwrap().1).unwrap();
        assert!(!js.contains("innerHTML") && !js.contains("insertAdjacentHTML"));
    }
}
