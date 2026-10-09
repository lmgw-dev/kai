//! The configuration window (WP21b, ruling 26, K29): a normal GTK toplevel, with KWin's frame, that
//! shows the page of `ui/` (Leptos, built by trunk, embedded: [`assets`]) in a WebKitGTK view of
//! its own ephemeral `WebContext`, created when the owner opens it from the tray's Settings and
//! destroyed when it is closed (K1's RAM rule: nothing of it lingers).
//!
//! **The page reaches Kai through the scheme handler only.** The context serves `kai://settings/`:
//! the page's files, `GET /api/tools`, and `POST /api/apply`, which is [`super::Request`] in and
//! [`super::Response`] out, run by the core (`Command::Setting`) like the owner tools' calls, so
//! the core's copy of the configuration and the host's policy follow every change. The Personality
//! page adds `GET /api/conversation` (`Command::WindowConversation`) and lmgw's profile editor's
//! requests under `/lmgw/` ([`profiles`]), which the core sends to the paired lmgw with the device
//! key (`Command::WindowProxy`), so the page holds no key and opens no connection. The Desktop page
//! shows and forgets the RemoteDesktop portal's stored permission ([`remote`],
//! `Command::WindowRemoteControl`). [`route`]
//! decides what a request gets and refuses every other origin; the view itself may not navigate
//! away, open a window, ask for a permission or show WebKit's context menu. Kai opens no network
//! listener.
//!
//! **Look.** The kit's tokens (`kit.css`) and `ui/assets/window.css`, in the desktop's own fonts;
//! the view is zoomed to the default interface scale ([`UI_SCALE`], Ctrl+plus, Ctrl+minus and
//! Ctrl+0 change it while the window is open). No header bar: GTK draws no title bar of its own, so
//! KWin's frame is the window's.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use tokio::sync::{mpsc, oneshot};
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt,
    PolicyDecisionType, SecurityManagerExt, SettingsExt, URIRequestExt, URISchemeRequest,
    URISchemeRequestExt, URISchemeResponse, URISchemeResponseExt, WebContext, WebContextExt,
    WebView, WebViewExt,
};

use self::route::{Answer, HOST, Incoming, Routed, SCHEME};
use crate::core::Command;

pub mod assets;
pub mod profiles;
pub mod remote;
pub mod route;
pub mod tools;

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;

/// The page's zoom when the window opens: 125 %, lmgw's default interface scale.
pub const UI_SCALE: f64 = 1.25;
const MIN_SCALE: f64 = 0.5;
const MAX_SCALE: f64 = 3.0;
const SCALE_STEP: f64 = 0.1;

/// The window's size in logical pixels, before the zoom, on a screen big enough.
const SIZE: (i32, i32) = (1100, 760);
/// How much of the screen's work area the window takes at most.
const SCREEN_SHARE: f64 = 0.94;

/// [`SIZE`], or what a small work area (`(width, height)`, if known) leaves.
pub fn fit_size(workarea: Option<(i32, i32)>) -> (i32, i32) {
    match workarea {
        Some((w, h)) => (
            SIZE.0.min((f64::from(w) * SCREEN_SHARE) as i32),
            SIZE.1.min((f64::from(h) * SCREEN_SHARE) as i32),
        ),
        None => SIZE,
    }
}

/// The primary monitor's work area, in logical pixels.
fn workarea() -> Option<(i32, i32)> {
    let display = gdk::Display::default()?;
    let monitor = display.primary_monitor().or_else(|| display.monitor(0))?;
    let area = monitor.workarea();
    Some((area.width(), area.height()))
}

/// The zoom after Ctrl+plus (`1`), Ctrl+minus (`-1`) or Ctrl+0 (`0`: back to [`UI_SCALE`]).
pub fn next_scale(current: f64, step: i32) -> f64 {
    match step {
        0 => UI_SCALE,
        s => {
            // Rounded to the hundredth, so repeated steps do not drift (1.25 + 0.1 - 0.1).
            let next = (current + SCALE_STEP * f64::from(s.signum())) * 100.0;
            (next.round() / 100.0).clamp(MIN_SCALE, MAX_SCALE)
        }
    }
}

/// The open window. Dropping it destroys the window, its view and the context.
pub struct SettingsWindow {
    window: gtk::Window,
    #[cfg_attr(not(test), allow(dead_code))]
    view: WebView,
}

impl SettingsWindow {
    /// Creates, loads and shows the window; `on_closed` runs once when it is gone (the owner closed
    /// it, or it was dropped).
    pub fn open(cmds: mpsc::UnboundedSender<Command>, on_closed: impl Fn() + 'static) -> Self {
        // Its own context per window (K1): a dropped ephemeral context takes its processes along.
        let context = WebContext::new_ephemeral();
        serve(&context, cmds);
        let view = WebView::with_context(&context);
        if let Some(s) = WebViewExt::settings(&view) {
            s.set_enable_developer_extras(false);
            // A debugging aid: the page's console (a CSP violation, a panic) on stdout.
            s.set_enable_write_console_messages_to_stdout(
                std::env::var_os("KAI_WINDOW_CONSOLE").is_some(),
            );
            s.set_enable_page_cache(false);
            s.set_enable_back_forward_navigation_gestures(false);
            s.set_javascript_can_open_windows_automatically(false);
        }
        // The page's own palette behind it until it paints, so the window does not flash white.
        view.set_background_color(&gdk::RGBA::new(0.086, 0.098, 0.110, 1.0));
        view.set_zoom_level(UI_SCALE);
        guard(&view);

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Kai Settings");
        window.set_icon_name(Some(crate::APP_ID));
        let (w, h) = fit_size(workarea());
        window.set_default_size(w, h);
        window.set_position(gtk::WindowPosition::Center);
        window.add(&view);
        let scale = Rc::new(Cell::new(UI_SCALE));
        {
            let view = view.clone();
            window.connect_key_press_event(move |_, ev| {
                if !ev.state().contains(gdk::ModifierType::CONTROL_MASK) {
                    return glib::Propagation::Proceed;
                }
                let key = ev.keyval();
                let step = if key == gdk::keys::constants::plus
                    || key == gdk::keys::constants::equal
                    || key == gdk::keys::constants::KP_Add
                {
                    1
                } else if key == gdk::keys::constants::minus
                    || key == gdk::keys::constants::KP_Subtract
                {
                    -1
                } else if key == gdk::keys::constants::_0 || key == gdk::keys::constants::KP_0 {
                    0
                } else {
                    return glib::Propagation::Proceed;
                };
                scale.set(next_scale(scale.get(), step));
                view.set_zoom_level(scale.get());
                glib::Propagation::Stop
            });
        }
        let gone = Rc::new(Cell::new(false));
        window.connect_destroy(move |_| {
            if !gone.replace(true) {
                on_closed();
            }
        });
        // The view and the context live as long as the window: they are owned by its handlers.
        let keep = (view.clone(), context);
        window.connect_destroy(move |_| {
            let _ = &keep;
        });
        view.load_uri(&format!("{SCHEME}://{HOST}/"));
        window.show_all();
        window.present();
        Self { window, view }
    }

    /// Runs `js` in the page; the result is its string value (the live check).
    #[cfg(test)]
    pub async fn eval(&self, js: &str) -> Result<String, String> {
        use javascriptcore::ValueExt;
        self.view
            .evaluate_javascript_future(js, None, None)
            .await
            .map(|v| v.to_str().to_string())
            .map_err(|e| e.to_string())
    }

    /// Brings an open window to the front.
    pub fn present(&self) {
        self.window.present();
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        // SAFETY: Kai holds no other reference to the window; its handlers capture only the view,
        // the context and plain flags, none of which is used after this.
        unsafe { self.window.destroy() };
    }
}

/// The view may show `kai://settings/` and nothing else: no other navigation, no window, no
/// permission, no WebKit menu (Reload, Back, Inspect).
fn guard(view: &WebView) {
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
        if kind == PolicyDecisionType::NavigationAction
            && uri.starts_with(&format!("{SCHEME}://{HOST}/"))
        {
            return false;
        }
        tracing::warn!(
            scheme = uri.split(':').next().unwrap_or(""),
            "the settings page tried to leave kai://settings/; ignored"
        );
        decision.ignore();
        true
    });
    view.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    view.connect_context_menu(|_, _, _, _| true);
    view.connect_load_failed(|_, _, uri, error| {
        tracing::warn!(%uri, "the settings page did not load: {error}");
        false
    });
    view.connect_web_process_terminated(|_, reason| {
        tracing::warn!(
            ?reason,
            "the settings page's web process ended; close the window and open it again"
        );
    });
}

/// `kai://settings/…` for this window's context.
fn serve(context: &WebContext, cmds: mpsc::UnboundedSender<Command>) {
    if let Some(sm) = context.security_manager() {
        sm.register_uri_scheme_as_secure(SCHEME);
        sm.register_uri_scheme_as_cors_enabled(SCHEME);
    }
    context.register_uri_scheme(SCHEME, move |request| {
        let cmds = cmds.clone();
        let request = request.clone();
        glib::MainContext::default().spawn_local(async move {
            let answer = handle(&request, &cmds).await;
            finish(&request, &answer);
        });
    });
}

async fn handle(request: &URISchemeRequest, cmds: &mpsc::UnboundedSender<Command>) -> Answer {
    let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
    let method = request
        .http_method()
        .map(|m| m.to_string())
        .unwrap_or_default();
    let header = |name: &str| {
        request
            .http_headers()
            .and_then(|h| h.one(name))
            .map(|v| v.to_string())
    };
    let (origin, fetch_site) = (header("Origin"), header("Sec-Fetch-Site"));
    let body = match request.http_body() {
        Some(stream) => read_all(&stream).await,
        None => Vec::new(),
    };
    let routed = route::route(&Incoming {
        method: &method,
        uri: &uri,
        origin: origin.as_deref(),
        fetch_site: fetch_site.as_deref(),
        body: &body,
    });
    match routed {
        Routed::Answer(a) => {
            if a.status >= 400 {
                tracing::debug!(%uri, status = a.status, "the settings page's request was not served");
            }
            a
        }
        Routed::Apply(op) => {
            let (tx, rx) = oneshot::channel();
            if cmds
                .send(Command::Setting {
                    request: op,
                    reply: tx,
                })
                .is_err()
            {
                return Answer::refused(503, "Kai is quitting");
            }
            match rx.await {
                Ok(Ok(response)) => Answer::json(&response),
                Ok(Err(refusal)) => Answer::refused(422, &refusal.0),
                Err(_) => Answer::refused(503, "Kai is quitting"),
            }
        }
        Routed::Conversation => {
            let (reply, rx) = oneshot::channel();
            if cmds.send(Command::WindowConversation { reply }).is_err() {
                return Answer::refused(503, "Kai is quitting");
            }
            match rx.await {
                Ok(facts) => Answer::json(&facts),
                Err(_) => Answer::refused(503, "Kai is quitting"),
            }
        }
        Routed::RemoteControl { forget } => {
            let (reply, rx) = oneshot::channel();
            if cmds
                .send(Command::WindowRemoteControl { forget, reply })
                .is_err()
            {
                return Answer::refused(503, "Kai is quitting");
            }
            match rx.await {
                Ok(Ok(state)) => Answer::json(&state),
                Ok(Err(why)) => Answer::refused(500, &why),
                Err(_) => Answer::refused(503, "Kai is quitting"),
            }
        }
        Routed::Proxy(forward) => {
            use crate::lmgw_link::proxy::Proxied;
            let quitting =
                || Answer::from(Proxied::refused(503, "kai_quitting", "Kai is quitting"));
            let (reply, rx) = oneshot::channel();
            if cmds.send(Command::WindowProxy { forward, reply }).is_err() {
                return quitting();
            }
            match rx.await {
                Ok(p) => Answer::from(p),
                Err(_) => quitting(),
            }
        }
    }
}

async fn read_all(stream: &gio::InputStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(chunk) = stream
        .read_bytes_future(16 * 1024, glib::Priority::DEFAULT)
        .await
    {
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
    }
    out
}

fn finish(request: &URISchemeRequest, answer: &Answer) {
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(&answer.body[..]));
    let response = URISchemeResponse::new(&stream, answer.body.len() as i64);
    response.set_status(u32::from(answer.status), None);
    response.set_content_type(answer.mime);
    // The headers replace the response's content type: it goes in them too (a stylesheet under
    // nosniff is applied empty).
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    headers.append("Content-Type", answer.mime);
    headers.append("X-Content-Type-Options", "nosniff");
    headers.append("Cache-Control", "no-store");
    if let Some(csp) = &answer.csp {
        headers.append("Content-Security-Policy", csp);
    }
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}
