//! The ring's surface and view on the GTK thread (§9.1, §9.2), as WP0 verified them: a
//! gtk-layer-shell window with an RGBA visual and transparent background, click-through by a 1×1 px
//! input region, hosting a WebKitGTK view in its own ephemeral `WebContext` with the `kai://`
//! scheme. Dropping the view with its context leaves no WebKit process (WP0 (o)).

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, LayerShell};
use javascriptcore::ValueExt;
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt, PolicyDecisionType,
    SecurityManagerExt, SettingsExt, URIRequestExt, URISchemeRequestExt, URISchemeResponse,
    URISchemeResponseExt, UserContentManager, UserContentManagerExt, WebContext, WebContextExt,
    WebView, WebViewExt, WebViewExtManual,
};

use super::Event;
use super::page::{self, SCHEME};
use super::placement::{Anchor, Layer};

/// Where the surface sits on its output (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub anchor: Anchor,
    pub margin: u32,
    pub size: u32,
}

/// Where [`RingView::show`] puts the surface.
pub enum Output<'a> {
    Monitor(&'a gdk::Monitor),
    /// The compositor chooses (nil).
    Compositor,
    /// A mapped surface stays on its output (one that could not be resolved now).
    Keep,
}

pub struct RingView {
    pub id: u64,
    window: gtk::Window,
    view: WebView,
    _ucm: UserContentManager,
    _context: WebContext,
}

impl RingView {
    /// Creates the window unmapped and loads the page: the web process starts now, nothing shows.
    pub fn create(
        id: u64,
        place: Place,
        reduced: bool,
        events: async_channel::Sender<Event>,
    ) -> RingView {
        // Its own context per view (K1): a dropped ephemeral context takes its network process along.
        let context = WebContext::new_ephemeral();
        serve(&context);
        let ucm = UserContentManager::new();
        ucm.register_script_message_handler("kai");
        let tx = events.clone();
        ucm.connect_script_message_received(Some("kai"), move |_, result| {
            let text = result
                .js_value()
                .map(|v| v.to_str().to_string())
                .unwrap_or_default();
            let _ = tx.try_send(Event::Page {
                view: id,
                msg: page::parse(&text),
            });
        });
        let view = WebView::new_with_context_and_user_content_manager(&context, &ucm);
        view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        if let Some(s) = WebViewExt::settings(&view) {
            s.set_enable_developer_extras(false);
            s.set_enable_write_console_messages_to_stdout(false);
            s.set_enable_page_cache(false);
            s.set_enable_back_forward_navigation_gestures(false);
        }
        let tx = events.clone();
        view.connect_web_process_terminated(move |_, reason| {
            let _ = tx.try_send(Event::Crashed {
                view: id,
                reason: format!("{reason:?}"),
            });
        });
        // A page that does not load never says `ready`, and the ring never shows: logged.
        view.connect_load_failed(|_, _, uri, error| {
            tracing::warn!(%uri, "the ring page did not load: {error}");
            false
        });
        // The ring page never navigates: anything but kai:// is ignored.
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
            if uri.starts_with(&format!("{SCHEME}://app/")) {
                return false;
            }
            tracing::warn!(
                scheme = uri.split(':').next().unwrap_or(""),
                "the ring page tried to leave kai://; ignored"
            );
            decision.ignore();
            true
        });

        let tx = events;
        let window = layer_window("Kai ring", "kai-ring", move || {
            let _ = tx.try_send(Event::Closed { view: id });
        });
        place_window(&window, place.anchor, place.margin);
        let size = place.size as i32;
        window.set_default_size(size, size);
        window.set_size_request(size, size);
        window.add(&view);
        // Click-through after the map and after every size change: GDK sends the input region at
        // map only (WP0 (d)).
        window.connect_map_event(|w, _| {
            click_through(w);
            glib::Propagation::Proceed
        });
        window.connect_size_allocate(|w, _| click_through(w));
        view.load_uri(&page::page_uri(reduced));
        RingView {
            id,
            window,
            view,
            _ucm: ucm,
            _context: context,
        }
    }

    /// Maps the surface (or places a mapped one again) on `output` and `layer`.
    pub fn show(&self, output: Output<'_>, layer: Layer) {
        match output {
            Output::Monitor(m) => self.window.set_monitor(m),
            // SAFETY: a valid window; NULL is the documented "let the compositor choose".
            Output::Compositor => unsafe {
                gtk_layer_shell_sys::gtk_layer_set_monitor(
                    self.window.to_glib_none().0,
                    std::ptr::null_mut(),
                );
            },
            Output::Keep => {}
        }
        self.window.set_layer(match layer {
            Layer::Overlay => gtk_layer_shell::Layer::Overlay,
            Layer::Top => gtk_layer_shell::Layer::Top,
        });
        self.window.show_all();
    }

    pub fn hide(&self) {
        self.window.hide();
    }

    pub fn eval(&self, js: &str) {
        self.view
            .evaluate_javascript(js, None, None, gio::Cancellable::NONE, |result| {
                if let Err(e) = result {
                    tracing::debug!("the ring page: {e}");
                }
            });
    }

    pub fn weak_view(&self) -> glib::WeakRef<WebView> {
        self.view.downgrade()
    }

    /// Destroys the window (and with it the view); the context goes when the last reference does.
    pub fn destroy(self) {
        // SAFETY: Kai holds no other reference to the window, and the view's handlers capture only
        // channel senders, so nothing uses either after this.
        unsafe { self.window.destroy() };
    }
}

/// A transparent, undecorated layer-shell window (§9.1), unmapped: the spike's full transparency
/// set (RGBA visual, app-paintable, the window's CSS, set on this window only), no keyboard,
/// exclusive zone −1, and the compositor's close asked for and handed to `on_closed` (the window is
/// kept). The ring's and the widgets' (`crate::widgets`) surfaces.
pub(crate) fn layer_window(
    title: &str,
    namespace: &str,
    on_closed: impl Fn() + 'static,
) -> gtk::Window {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(title);
    window.set_decorated(false);
    window.set_app_paintable(true);
    if let Some(visual) = gdk::Screen::default().and_then(|s| s.rgba_visual()) {
        window.set_visual(Some(&visual));
    }
    let css = gtk::CssProvider::new();
    if css
        .load_from_data(b"window { background: transparent; }")
        .is_ok()
    {
        window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }
    window.init_layer_shell();
    // The compositor closes the layer surface when its output goes away (a monitor powered
    // off or unplugged). gtk-layer-shell 0.10 ignores that by default, which leaves a window
    // GTK thinks mapped and nobody sees; older versions close the window. Kai asks for the
    // close and keeps the window: the owner decides what follows (review KAI-5 R5-14).
    respect_close(&window);
    window.connect_delete_event(move |_, _| {
        on_closed();
        glib::Propagation::Stop
    });
    window.set_namespace(namespace);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(-1);
    window
}

/// The anchor's edges pinned, `margin` px from each.
pub(crate) fn place_window(window: &gtk::Window, anchor: Anchor, margin: u32) {
    let edges = anchor.edges();
    let margin = margin as i32;
    for (edge, on) in [
        (Edge::Top, edges.top),
        (Edge::Bottom, edges.bottom),
        (Edge::Left, edges.left),
        (Edge::Right, edges.right),
    ] {
        window.set_anchor(edge, on);
        window.set_layer_shell_margin(edge, if on { margin } else { 0 });
    }
}

/// `gtk_layer_set_respect_close(window, TRUE)`: the compositor's close reaches GTK as `delete-event`.
/// The function exists from gtk-layer-shell 0.10, and the Rust bindings lack it, so it is looked up
/// at run time: an older library forwards the close anyway and still loads. A 0.10 or later whose
/// function cannot be found ignores the close, so that is a warning, said once (review KAI-6 R6-5).
fn respect_close(window: &gtk::Window) {
    type SetRespectClose = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, glib::ffi::gboolean);
    // SAFETY: a lookup in the process's global namespace, where libgtk-layer-shell is linked.
    let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"gtk_layer_set_respect_close".as_ptr()) };
    if f.is_null() {
        let version = (
            gtk_layer_shell::major_version(),
            gtk_layer_shell::minor_version(),
        );
        let (major, minor) = version;
        if version >= (0, 10) {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| {
                tracing::warn!(
                    "gtk-layer-shell {major}.{minor} has gtk_layer_set_respect_close, but Kai \
                     cannot find it, so the compositor's close of the ring's surface (its output \
                     went away) goes unseen: a ring whose output goes away may stay invisible \
                     until the next turn starts"
                );
            });
        } else {
            tracing::debug!(
                "gtk-layer-shell {major}.{minor}, before 0.10: it forwards the compositor's close \
                 itself"
            );
        }
        return;
    }
    // SAFETY: the symbol has this signature (gtk-layer-shell.h, since 0.10), and the window is a
    // layer surface (`init_layer_shell` ran).
    unsafe {
        let f = std::mem::transmute::<*mut libc::c_void, SetRespectClose>(f);
        f(window.to_glib_none().0, glib::ffi::GTRUE);
    }
}

/// GDK3 sends an empty input region as `set_input_region(nil)`, "the whole surface takes input":
/// one pixel in the corner stands in for none (WP0, Q1.2).
pub(crate) fn click_through(w: &gtk::Window) {
    if let Some(gw) = w.window() {
        let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, 1, 1));
        gw.input_shape_combine_region(&region, 0, 0);
    }
}

/// `kai://app/…` from the embedded assets, each response with the page's CSP (K10).
fn serve(context: &WebContext) {
    if let Some(sm) = context.security_manager() {
        sm.register_uri_scheme_as_secure(SCHEME);
        sm.register_uri_scheme_as_cors_enabled(SCHEME);
    }
    context.register_uri_scheme(SCHEME, |request| {
        let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
        let found = url::Url::parse(&uri).ok().and_then(|u| {
            let host = u.host_str().unwrap_or("").to_owned();
            page::asset(&host, u.path())
        });
        let Some((mime, body)) = found else {
            tracing::warn!(%uri, "the ring page asked for a file Kai does not have");
            request.finish_error(&mut glib::Error::new(
                gio::IOErrorEnum::NotFound,
                &format!("{uri}: not found"),
            ));
            return;
        };
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(body));
        let response = URISchemeResponse::new(&stream, body.len() as i64);
        response.set_content_type(mime);
        // The headers replace the response's content type: it goes in them too, or a stylesheet
        // under nosniff is applied empty.
        let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
        headers.append("Content-Type", mime);
        headers.append("Content-Security-Policy", page::CSP);
        headers.append("X-Content-Type-Options", "nosniff");
        response.set_http_headers(headers);
        request.finish_with_response(&response);
    });
}
