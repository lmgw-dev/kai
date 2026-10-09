//! A widget's surface and view on the GTK thread (§10.1, K1): one layer-shell surface per widget
//! (the ring's window code, `overlay::view`), one WebKit view in it, and every widget view related
//! to the others so they share one web process, in one ephemeral web context of their own
//! ([`Shared`]) that is dropped with the last widget, so no web process is left (K1).
//!
//! The sandbox's rules are [`super::sandbox`]'s; this file applies them: the `kai-sandbox:` scheme
//! (the only one the context knows), the content filter, the isolated world, the navigation
//! policy, the denied permissions, the view's settings and features ([`super::features`]), and the
//! web process's responsiveness.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use javascriptcore::ValueExt;
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt,
    PolicyDecisionType, Settings, SettingsExt, URIRequestExt, URISchemeRequestExt,
    URISchemeResponse, URISchemeResponseExt, UserContentInjectedFrames, UserContentManager,
    UserContentManagerExt, UserScript, UserScriptInjectionTime, WebContext, WebContextExt, WebView,
    WebViewExt,
};

use super::filter::Filter;
use super::gtk_host::Event;
use super::sandbox::{
    self, CONTENT_CSP, HANDLER, HOST_CSP, HOST_SCRIPT, HOST_URI, SCHEME, Served, WORLD,
};
use crate::overlay::placement::Anchor;
use crate::overlay::{layer_window, place_window};

/// Which view is which widget's.
type ViewIds = Rc<RefCell<Vec<(u64, glib::WeakRef<WebView>)>>>;

/// The widgets' web context and what its scheme handler serves: the callers' documents by widget,
/// and which view is which widget's (a document goes only to its own view).
pub struct Shared {
    context: WebContext,
    docs: Rc<RefCell<HashMap<u64, Rc<str>>>>,
    views: ViewIds,
}

impl Shared {
    pub fn new() -> Shared {
        // Ephemeral and its own (K1): dropped with the last widget, it takes its processes along.
        let context = WebContext::new_ephemeral();
        let docs: Rc<RefCell<HashMap<u64, Rc<str>>>> = Rc::default();
        let views: ViewIds = Rc::default();
        let (d, v) = (docs.clone(), views.clone());
        context.register_uri_scheme(SCHEME, move |request| {
            let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
            let view = request.web_view().and_then(|asking| {
                v.borrow()
                    .iter()
                    .find(|(_, w)| w.upgrade().as_ref() == Some(&asking))
                    .map(|(id, _)| *id)
            });
            match sandbox::serve(&uri, view) {
                Served::Asset { mime, body } => {
                    respond(request, glib::Bytes::from_static(body), mime, HOST_CSP)
                }
                Served::Frame(id) => match d.borrow().get(&id) {
                    Some(doc) => respond(
                        request,
                        glib::Bytes::from(doc.as_bytes()),
                        "text/html; charset=utf-8",
                        CONTENT_CSP,
                    ),
                    None => not_found(request),
                },
                Served::NotFound => {
                    // The URL may be the caller's content: not logged.
                    tracing::warn!(
                        widget = view,
                        "a widget view asked for a kai-sandbox file Kai does not serve it; refused"
                    );
                    not_found(request);
                }
            }
        });
        Shared {
            context,
            docs,
            views,
        }
    }

    pub fn set_doc(&self, id: u64, doc: &str) {
        self.docs.borrow_mut().insert(id, Rc::from(doc));
    }

    fn forget(&self, id: u64) {
        self.docs.borrow_mut().remove(&id);
        self.views
            .borrow_mut()
            .retain(|(i, w)| *i != id && w.upgrade().is_some());
    }

    /// A live widget view to relate a new one to (one web process for all, K1).
    fn related(&self) -> Option<WebView> {
        self.views.borrow().iter().find_map(|(_, w)| w.upgrade())
    }
}

fn respond(request: &webkit2gtk::URISchemeRequest, body: glib::Bytes, mime: &str, csp: &str) {
    let len = body.len() as i64;
    let stream = gio::MemoryInputStream::from_bytes(&body);
    let response = URISchemeResponse::new(&stream, len);
    response.set_content_type(mime);
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    for (name, value) in sandbox::headers(mime, csp) {
        headers.append(name, &value);
    }
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}

fn not_found(request: &webkit2gtk::URISchemeRequest) {
    request.finish_error(&mut glib::Error::new(
        gio::IOErrorEnum::NotFound,
        "not served",
    ));
}

/// Where and how a widget's surface sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub anchor: Anchor,
    pub margin: u32,
    pub interactive: bool,
}

/// An MCP Apps view's own: its bridge's message bound and its server's label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppFrame {
    /// `overlay.app_max_mb` in bytes: Kai's app script refuses a bigger message, and so does the
    /// GTK side.
    pub max: u64,
    /// The server's label, which Kai's strip on an interactive view names.
    pub server: String,
}

/// The height of Kai's strip above an interactive MCP Apps view (§10.1), in application pixels:
/// part of the configured height, so the view itself gets that much less.
pub const STRIP_PX: u32 = 24;

/// Kai's palette for what it draws around a view (Breeze-like graphite and blue, never pastel).
const APP_CSS: &str = "
.kai-app-strip { background-color: #31363b; border-bottom: 2px solid #3daee9; }
.kai-app-strip label { color: #eff0f1; font-size: 9pt; font-weight: bold; padding: 0 8px; }
";

/// What Kai draws itself around an MCP Apps view: outside the WebView, so the view's content
/// cannot paint over it.
struct AppChrome {
    /// Kai's strip: above an interactive view always, and the whole surface while a Kai
    /// confirmation is open ([`WidgetView::guard`]).
    strip: gtk::Box,
    /// The strip's words while the view is as configured: its server's name.
    name: gtk::Label,
    /// The strip's words while a Kai confirmation is open: the view waits.
    note: gtk::Label,
    /// The configuration lets the view take clicks (`[overlay.app] interactive`).
    interactive: bool,
    input: super::input::Input,
    /// A Kai confirmation is open (the map's `show_all` must not show the content again).
    guarded: std::cell::Cell<bool>,
}

/// Where the surface of an MCP Apps view held for a Kai confirmation sits (§10.1, the owner's
/// decision of 2026-10-09): only Kai's strip is left, on the output's top edge (the bottom edge for
/// a view anchored at the bottom), at the view's own horizontal place, so it is nowhere near a
/// dialog the compositor centres. The edges anchored, then the vertical and the horizontal margin.
pub fn held_place(anchor: Anchor, margin: u32) -> (crate::overlay::placement::Edges, u32, u32) {
    let e = anchor.edges();
    let edges = crate::overlay::placement::Edges {
        top: !e.bottom,
        bottom: e.bottom,
        left: e.left,
        right: e.right,
    };
    let vertical = if e.top || e.bottom { margin } else { 0 };
    (edges, vertical, margin)
}

pub struct WidgetView {
    pub id: u64,
    window: gtk::Window,
    view: WebView,
    /// Where it sits as configured, and its size once shown (a held app view goes back to them).
    frame: Frame,
    size: std::cell::Cell<Option<(u32, u32)>>,
    _ucm: UserContentManager,
    /// A ping in Kai's world is unanswered.
    pinged: Rc<std::cell::Cell<bool>>,
    /// An MCP Apps view's strip and dimming.
    app: Option<AppChrome>,
}

fn styled(w: &impl IsA<gtk::Widget>, class: &str) {
    let provider = gtk::CssProvider::new();
    if let Err(e) = provider.load_from_data(APP_CSS.as_bytes()) {
        tracing::warn!("an app view's chrome style: {e}");
    }
    let ctx = w.style_context();
    ctx.add_class(class);
    ctx.add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// A label styled by its parent's class (a provider is per widget, so the label gets its own).
fn chrome_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    let provider = gtk::CssProvider::new();
    if provider.load_from_data(APP_CSS.as_bytes()).is_ok() {
        label
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    label
}

impl WidgetView {
    /// Creates the surface unmapped and loads the host page: nothing shows yet. An error (a WebKit
    /// feature the sandbox turns off that this WebKit does not have) creates nothing. `app`: an
    /// MCP Apps view's, whose page also gets Kai's app script (K18; no other kind's does), whose
    /// messages larger than its bound are refused unread, and which gets Kai's chrome: a strip
    /// naming its server above an interactive one, and the dimming of [`WidgetView::guard`].
    pub fn create(
        id: u64,
        shared: &Shared,
        filter: &Filter,
        frame: Frame,
        app: Option<&AppFrame>,
        events: async_channel::Sender<Event>,
    ) -> Result<WidgetView, String> {
        let bridge = app.map(|a| a.max);
        let settings = settings()?;
        let ucm = UserContentManager::new();
        // The handler exists in Kai's isolated world only: the caller's frame cannot post to it.
        ucm.register_script_message_handler_in_world(HANDLER, WORLD);
        let tx = events.clone();
        ucm.connect_script_message_received(Some(HANDLER), move |_, result| {
            let text = result
                .js_value()
                .map(|v| v.to_str().to_string())
                .unwrap_or_default();
            if let Some(max) = bridge
                && sandbox::rpc_too_big(&text, max)
            {
                // Kai's app script refuses such a message before it posts it, so this is a fault
                // of Kai's own; the content is the view's, never logged.
                tracing::warn!(
                    widget = id,
                    bytes = text.len(),
                    "an app view's message over overlay.app_max_mb reached Kai; ignored unread"
                );
                return;
            }
            let _ = tx.try_send(Event::Page {
                id,
                msg: sandbox::parse(&text),
            });
        });
        ucm.add_script(&UserScript::for_world(
            HOST_SCRIPT,
            UserContentInjectedFrames::TopFrame,
            UserScriptInjectionTime::End,
            WORLD,
            &[],
            &[],
        ));
        if bridge.is_some() {
            ucm.add_script(&UserScript::for_world(
                super::app_bridge::HOST_SCRIPT,
                UserContentInjectedFrames::TopFrame,
                UserScriptInjectionTime::End,
                WORLD,
                &[],
                &[],
            ));
        }
        filter.add_to(&ucm);
        let builder = WebView::builder()
            .user_content_manager(&ucm)
            .settings(&settings);
        let view = match shared.related() {
            Some(r) => builder.related_view(&r).build(),
            None => builder.web_context(&shared.context).build(),
        };
        shared.views.borrow_mut().push((id, view.downgrade()));
        view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        // A script that never yields stalls the shared web process (every widget's): WebKit's own
        // responsiveness check says so, and the host ends the process.
        let tx = events.clone();
        view.connect_is_web_process_responsive_notify(move |v| {
            if !v.is_web_process_responsive() {
                let _ = tx.try_send(Event::Unresponsive { id });
            }
        });
        let tx = events.clone();
        view.connect_web_process_terminated(move |_, reason| {
            let _ = tx.try_send(Event::Crashed {
                id,
                reason: format!("{reason:?}"),
            });
        });
        view.connect_load_failed(move |_, _, uri, error| {
            tracing::warn!(
                widget = id,
                scheme = sandbox::scheme_of(uri),
                "a widget's page did not load: {error}"
            );
            false
        });
        // Only the host page and its own document (sandbox::navigation); no window.
        let tx = events.clone();
        view.connect_decide_policy(move |_, decision, kind| {
            let new_window = match kind {
                PolicyDecisionType::NavigationAction => false,
                PolicyDecisionType::NewWindowAction => true,
                _ => return false,
            };
            let uri = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(|d| d.navigation_action())
                .and_then(|a| a.request())
                .and_then(|r| r.uri())
                .map(|u| u.to_string())
                .unwrap_or_default();
            match sandbox::navigation(id, &uri, new_window) {
                Ok(()) => false,
                Err(refused) => {
                    decision.ignore();
                    let _ = tx.try_send(Event::Refused { id, uri, refused });
                    true
                }
            }
        });
        // Camera, microphone, geolocation, notifications, clipboard, pointer lock: all denied (the
        // host logs the first per widget and counts the rest).
        let tx = events.clone();
        view.connect_permission_request(move |_, request| {
            request.deny();
            let _ = tx.try_send(Event::Permission { id });
            true
        });
        // No context menu (it would offer reload and navigation), no alert, confirm or prompt
        // dialog, no print dialog: each swallowed.
        view.connect_context_menu(|_, _, _, _| true);
        view.connect_script_dialog(|_, _| true);
        view.connect_print(|_, _| true);

        let tx = events;
        let window = layer_window("Kai overlay", "kai-widget", move || {
            let _ = tx.try_send(Event::Closed { id });
        });
        place_window(&window, frame.anchor, frame.margin);
        // The input region: the widget's rectangle when it takes clicks, else none.
        let input = super::input::keep(&window, frame.interactive);
        let app = match app {
            None => {
                window.add(&view);
                None
            }
            Some(a) => {
                // Kai's word on whose view it is, which the view cannot paint over: above an
                // interactive view always, and the whole surface while a Kai confirmation is open.
                let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                styled(&strip, "kai-app-strip");
                strip.set_size_request(-1, STRIP_PX as i32);
                let name = chrome_label(&format!("{} \u{2014} app view", a.server));
                name.set_xalign(0.0);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                strip.pack_start(&name, true, true, 0);
                let note = chrome_label(&format!(
                    "{}'s view waits while Kai asks you to confirm a call",
                    a.server
                ));
                note.set_xalign(0.0);
                note.set_ellipsize(gtk::pango::EllipsizeMode::End);
                strip.pack_start(&note, true, true, 0);
                column.pack_start(&strip, false, false, 0);
                column.pack_start(&view, true, true, 0);
                window.add(&column);
                Some(AppChrome {
                    strip,
                    name,
                    note,
                    interactive: frame.interactive,
                    input,
                    guarded: Default::default(),
                })
            }
        };
        view.load_uri(HOST_URI);
        Ok(WidgetView {
            id,
            window,
            view,
            frame,
            size: Default::default(),
            _ucm: ucm,
            pinged: Rc::default(),
            app,
        })
    }

    /// A Kai confirmation is open (`on`), or none is any more (§10.1, the owner's decision of
    /// 2026-10-09): an MCP Apps view then shrinks to Kai's strip alone, saying the view waits, at
    /// [`held_place`], so no part of it can cover the dialog, catch a click meant for it or pass
    /// for it; it takes no click meanwhile. After, it is as configured: its place, its size, its
    /// content. Other kinds are untouched (`false`).
    pub fn guard(&self, on: bool) -> bool {
        let Some(app) = &self.app else {
            return false;
        };
        app.input.set(&self.window, app.interactive && !on);
        app.guarded.set(on);
        self.layout();
        true
    }

    /// An app view's chrome and surface for its state (as configured, or held by [`Self::guard`]).
    fn layout(&self) {
        use gtk_layer_shell::{Edge, LayerShell};
        let Some(app) = &self.app else { return };
        let held = app.guarded.get();
        self.view.set_visible(!held);
        app.name.set_visible(app.interactive && !held);
        app.note.set_visible(held);
        app.strip.set_visible(app.interactive || held);
        let Some((width, height)) = self.size.get() else {
            return;
        };
        let (edges, vertical, horizontal) = if held {
            held_place(self.frame.anchor, self.frame.margin)
        } else {
            let m = self.frame.margin;
            (self.frame.anchor.edges(), m, m)
        };
        for (edge, on, margin) in [
            (Edge::Top, edges.top, vertical),
            (Edge::Bottom, edges.bottom, vertical),
            (Edge::Left, edges.left, horizontal),
            (Edge::Right, edges.right, horizontal),
        ] {
            self.window.set_anchor(edge, on);
            self.window
                .set_layer_shell_margin(edge, if on { margin as i32 } else { 0 });
        }
        let height = if held { STRIP_PX } else { height };
        self.window.set_size_request(width as i32, height as i32);
        self.window.resize(width as i32, height as i32);
    }

    /// Calls the host script (Kai's world).
    pub fn eval(&self, js: &str) {
        let id = self.id;
        self.view
            .evaluate_javascript(js, Some(WORLD), None, gio::Cancellable::NONE, move |r| {
                if let Err(e) = r {
                    tracing::debug!(widget = id, "a widget's host script: {e}");
                }
            });
    }

    /// Pings the web process (a no-op in Kai's world, answered by the web process's main thread);
    /// `false` when the last ping is still unanswered.
    pub fn ping(&self) -> bool {
        if self.pinged.get() {
            return false;
        }
        self.pinged.set(true);
        let pinged = self.pinged.clone();
        self.view
            .evaluate_javascript("0", Some(WORLD), None, gio::Cancellable::NONE, move |_| {
                pinged.set(false)
            });
        true
    }

    /// Sizes the surface and maps it on `monitor` (`None`: the compositor's choice), on the
    /// OVERLAY layer (over everything, like the ring, ruling 6).
    pub fn show(&self, monitor: Option<&gdk::Monitor>, width: u32, height: u32) {
        use gtk::glib::translate::ToGlibPtr;
        use gtk_layer_shell::LayerShell;
        match monitor {
            Some(m) => self.window.set_monitor(m),
            // SAFETY: a valid window; NULL is the documented "let the compositor choose".
            None => unsafe {
                gtk_layer_shell_sys::gtk_layer_set_monitor(
                    self.window.to_glib_none().0,
                    std::ptr::null_mut(),
                );
            },
        }
        self.window.set_layer(gtk_layer_shell::Layer::Overlay);
        self.size.set(Some((width, height)));
        self.window.set_default_size(width as i32, height as i32);
        self.window.set_size_request(width as i32, height as i32);
        self.window.show_all();
        // An app view's chrome as its state says (`show_all` showed all of it), held or not.
        self.layout();
    }

    /// The screen lock: the surface unmapped (`false`) or mapped again as [`Self::show`] left it.
    pub fn set_mapped(&self, mapped: bool) {
        if !mapped {
            self.window.hide();
            return;
        }
        self.window.show_all();
        self.layout();
    }

    /// Destroys the surface and the view; the context goes when [`Shared`] and the last view do.
    pub fn destroy(self, shared: Option<&Shared>) {
        if let Some(s) = shared {
            s.forget(self.id);
        }
        // SAFETY: Kai holds no other reference to the window, and the view's handlers capture only
        // channel senders, so nothing uses either after this.
        unsafe { self.window.destroy() };
    }

    /// An `overlay_app` view's end: Kai's `ui/resource-teardown` (`message`) is sent to the view,
    /// then the surface goes two turns of the page's event loop later ([`sandbox::teardown_js`]),
    /// or after `at_most` (`overlay.unresponsive_s`: a page that does not run its scripts gets no
    /// longer). The view's answer is not waited for.
    pub fn teardown(self, shared: Option<&Shared>, message: &str, at_most: std::time::Duration) {
        if let Some(s) = shared {
            s.forget(self.id);
        }
        let (id, view) = (self.id, self.view.clone());
        let this = Rc::new(RefCell::new(Some(self)));
        // Whichever comes first destroys it: the page's turn, or the time up.
        let gone = move || {
            if let Some(v) = this.borrow_mut().take() {
                v.destroy(None);
                true
            } else {
                false
            }
        };
        let timer = {
            let gone = gone.clone();
            glib::timeout_add_local_once(at_most, move || {
                if gone() {
                    tracing::debug!(widget = id, "an app view's teardown took its whole time");
                }
            })
        };
        view.call_async_javascript_function(
            &sandbox::teardown_js(message),
            None,
            Some(WORLD),
            None,
            gio::Cancellable::NONE,
            move |r| {
                match r {
                    Ok(v) if v.to_boolean() => {}
                    Ok(_) => tracing::debug!(widget = id, "an app view's teardown: no view placed"),
                    Err(e) => tracing::debug!(widget = id, "an app view's teardown: {e}"),
                }
                // Destroyed here: the timer has not fired, so it is still there to remove.
                if gone() {
                    timer.remove();
                }
            },
        );
    }

    /// Ends the view's web process (every related widget view's: they share it). Each view then
    /// says `web-process-terminated`.
    pub fn terminate(&self) {
        self.view.terminate_web_process();
    }

    /// The live check's look into the caller's frame from the main world.
    #[cfg(test)]
    pub fn eval_main(&self, js: &str, done: impl FnOnce(Result<String, String>) + 'static) {
        self.view
            .evaluate_javascript(js, None, None, gio::Cancellable::NONE, move |r| {
                done(r.map(|v| v.to_str().to_string()).map_err(|e| e.to_string()))
            });
    }
}

/// A widget view's settings: everything a widget does not need is off. Sound and media (a widget
/// could otherwise speak into Kai's own microphone), WebGL, fullscreen, prefetching and the
/// features of [`super::features::OFF`].
fn settings() -> Result<Settings, String> {
    let settings = Settings::new();
    harden(&settings)?;
    Ok(settings)
}

pub(crate) fn harden(settings: &Settings) -> Result<(), String> {
    settings.set_enable_developer_extras(false);
    settings.set_enable_write_console_messages_to_stdout(false);
    settings.set_enable_page_cache(false);
    settings.set_enable_back_forward_navigation_gestures(false);
    settings.set_javascript_can_open_windows_automatically(false);
    settings.set_javascript_can_access_clipboard(false);
    settings.set_allow_file_access_from_file_urls(false);
    settings.set_allow_universal_access_from_file_urls(false);
    settings.set_allow_top_navigation_to_data_urls(false);
    settings.set_enable_hyperlink_auditing(false);
    settings.set_enable_dns_prefetching(false);
    settings.set_enable_html5_local_storage(false);
    settings.set_enable_html5_database(false);
    settings.set_enable_media_stream(false);
    settings.set_enable_webrtc(false);
    settings.set_enable_encrypted_media(false);
    settings.set_enable_webaudio(false);
    settings.set_enable_media(false);
    settings.set_media_playback_requires_user_gesture(true);
    settings.set_enable_webgl(false);
    settings.set_enable_fullscreen(false);
    super::features::turn_off(settings)
}

#[cfg(test)]
mod tests {
    use super::held_place;
    use crate::overlay::placement::{Anchor, Edges};

    /// A held app view's strip goes to the output's top edge (bottom for a view anchored at the
    /// bottom), keeping its horizontal place; a centred view's never meets a centred dialog.
    #[test]
    fn a_held_view_sits_on_an_edge_away_from_a_centred_dialog() {
        let e = |top, bottom, left, right| Edges {
            top,
            bottom,
            left,
            right,
        };
        for (anchor, edges, vertical) in [
            (Anchor::Center, e(true, false, false, false), 0),
            (Anchor::Left, e(true, false, true, false), 0),
            (Anchor::Right, e(true, false, false, true), 0),
            (Anchor::Top, e(true, false, false, false), 24),
            (Anchor::TopLeft, e(true, false, true, false), 24),
            (Anchor::TopRight, e(true, false, false, true), 24),
            (Anchor::Bottom, e(false, true, false, false), 24),
            (Anchor::BottomLeft, e(false, true, true, false), 24),
            (Anchor::BottomRight, e(false, true, false, true), 24),
        ] {
            assert_eq!(held_place(anchor, 24), (edges, vertical, 24), "{anchor:?}");
        }
    }
}
