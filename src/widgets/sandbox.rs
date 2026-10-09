//! The widgets' sandbox (§10.1, K1, K10, K18), as rules the GTK side applies and the tests pin:
//!
//! - **Its own origin and context.** Widgets live on `kai-sandbox://`, in a web context of their
//!   own that knows no other scheme: the ring's `kai://` is not registered there, so nothing of
//!   Kai's own pages can be loaded, and `file:` stays refused (no local-file access is granted).
//! - **The host page** (`kai-sandbox://widget/host.html`) is Kai's: no script of its own (its CSP
//!   has no `script-src`); Kai's host script runs as a user script in an isolated world, the only
//!   world that has the message handler, so the caller's document cannot post to Kai.
//! - **The caller's document** is an `<iframe sandbox="allow-scripts">` (`assets/widgets/host.js`;
//!   no same origin, no top
//!   navigation, no popups, no forms) whose document Kai serves under §10.1's CSP:
//!   [`CONTENT_CSP`]. Inline scripts and styles run; `data:` and `blob:` images and fonts load;
//!   nothing else does (no fetch, no WebSocket, no external script, style, frame or image).
//! - **Navigation** ([`navigation`]): a widget's view goes only to the host page and its own
//!   document; every other navigation and every new window is refused (`decide-policy`) and logged
//!   with its scheme only (the URL is the caller's content).
//! - **Loads** ([`FILTER_RULES`]): a content filter in the web process blocks every load that is not
//!   `kai-sandbox:`, `data:` or `blob:`, whatever the CSP says; the scheme handler serves a
//!   document only to its own view ([`serve`]).
//! - **Permissions** (camera, microphone, geolocation, notifications, clipboard, pointer lock) are
//!   all denied.

/// The widgets' scheme, separate from the ring's `kai://` (§10.1).
pub const SCHEME: &str = "kai-sandbox";

/// The host page every widget view loads.
pub const HOST_URI: &str = "kai-sandbox://widget/host.html";

/// The isolated world Kai's host script and message handler live in.
pub const WORLD: &str = "kai-widget";

/// The message handler's name (in [`WORLD`] only).
pub const HANDLER: &str = "kai";

/// The host page's CSP: its stylesheet, the caller's frame on this scheme, images for the kinds
/// that show them (WP11); no script at all (Kai's runs in [`WORLD`]).
pub const HOST_CSP: &str = "default-src 'none'; style-src 'self'; frame-src kai-sandbox:; \
                            img-src data: blob:; base-uri 'none'; form-action 'none'";

/// §10.1's CSP for the caller's document, as written there.
pub const CONTENT_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src \
                               'unsafe-inline'; img-src data: blob:; font-src data:";

/// The caller's document for widget `id`.
pub fn frame_uri(id: u64) -> String {
    format!("{SCHEME}://widget/frame/{id}")
}

/// Kai's host script (a user script in [`WORLD`], top frame only).
pub const HOST_SCRIPT: &str = include_str!("../../assets/widgets/host.js");

/// The content filter (WebKit's content-blocker rules): block everything, then let this scheme,
/// `data:` and `blob:` through.
pub const FILTER_RULES: &str = r#"[
  {"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": "^kai-sandbox:"}, "action": {"type": "ignore-previous-rules"}},
  {"trigger": {"url-filter": "^data:"}, "action": {"type": "ignore-previous-rules"}},
  {"trigger": {"url-filter": "^blob:"}, "action": {"type": "ignore-previous-rules"}}
]"#;

/// The filter's identifier in its store; a change of the rules changes it, so a store from an
/// older build is never used.
pub fn filter_id() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in FILTER_RULES.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    format!("kai-widgets-{h:016x}")
}

/// The scheme of `uri`, for a log line (never the URL: it is the caller's content).
pub fn scheme_of(uri: &str) -> &str {
    match uri.split_once(':') {
        Some((s, _))
            if !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            s
        }
        _ => "(none)",
    }
}

/// Why a navigation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub scheme: String,
    pub why: &'static str,
}

/// A navigation of widget `id`'s view to `uri`: only the host page and its own document.
pub fn navigation(id: u64, uri: &str, new_window: bool) -> Result<(), Refused> {
    let refused = |why| {
        Err(Refused {
            scheme: scheme_of(uri).to_owned(),
            why,
        })
    };
    if new_window {
        return refused("a widget opens no window");
    }
    if uri == HOST_URI || uri == frame_uri(id) {
        return Ok(());
    }
    if uri.starts_with(&format!("{SCHEME}:")) {
        return refused("not this widget's page");
    }
    refused("a widget goes nowhere but its own page")
}

/// What the scheme handler answers for a `kai-sandbox:` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// One of the host page's files, under [`HOST_CSP`].
    Asset {
        mime: &'static str,
        body: &'static [u8],
    },
    /// Widget `id`'s document, under [`CONTENT_CSP`].
    Frame(u64),
    NotFound,
}

const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/host.html",
        "text/html",
        include_bytes!("../../assets/widgets/host.html"),
    ),
    (
        "/host.css",
        "text/css",
        include_bytes!("../../assets/widgets/host.css"),
    ),
];

/// `uri` as requested by widget `view`'s view (`None`: a request no widget view made).
pub fn serve(uri: &str, view: Option<u64>) -> Served {
    let Ok(u) = url::Url::parse(uri) else {
        return Served::NotFound;
    };
    if u.scheme() != SCHEME || u.host_str() != Some("widget") || u.query().is_some() {
        return Served::NotFound;
    }
    let path = u.path();
    if let Some(&(_, mime, body)) = ASSETS.iter().find(|(p, _, _)| *p == path) {
        return Served::Asset { mime, body };
    }
    match path.strip_prefix("/frame/").map(str::parse::<u64>) {
        Some(Ok(id)) if view == Some(id) && frame_uri(id) == uri => Served::Frame(id),
        _ => Served::NotFound,
    }
}

/// The response headers Kai sends with a body: the content type in them too (K10: the headers
/// replace it), the CSP, no sniffing, no referrer.
pub fn headers(mime: &str, csp: &str) -> [(&'static str, String); 4] {
    [
        ("Content-Type", mime.to_owned()),
        ("Content-Security-Policy", csp.to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Referrer-Policy", "no-referrer".to_owned()),
    ]
}

/// The JS call that shows `body` in the host page (in [`WORLD`]).
pub fn show_js(id: u64, body: &super::kinds::Body, width: u32) -> String {
    use super::kinds::Body;
    match body {
        Body::Text(t) => format!(
            "__kaiWidget.text({},{width})",
            serde_json::Value::String(t.clone())
        ),
        Body::Document(_) => format!(
            "__kaiWidget.frame({})",
            serde_json::Value::String(frame_uri(id))
        ),
        // The view's document goes through the scheme like overlay_html's; Kai's app script places
        // it and answers only it (K18), and refuses a message over `overlay.app_max_mb`.
        Body::App(app) => format!(
            "__kaiWidget.app({},{})",
            serde_json::Value::String(frame_uri(id)),
            serde_json::json!({"max": app.bound.bytes, "says": app.bound.says})
        ),
    }
}

/// The JS call that hands the bridge's message (JSON Kai serialized) to an `overlay_app` view.
pub fn post_js(message: &str) -> String {
    format!("__kaiWidget.post({message})")
}

/// The body of the async JS function that sends Kai's `ui/resource-teardown` (JSON Kai
/// serialized) to an `overlay_app` view and returns two turns of the page's event loop later: the
/// view takes it in the first, and an answer it gives at once is relayed in the second. The
/// answer itself is not waited for. `true`: it was posted (a view was placed).
pub fn teardown_js(message: &str) -> String {
    format!(
        "const sent = __kaiWidget.post({message}); \
         for (let i = 0; i < 2; i++) await new Promise((r) => setTimeout(r, 0)); \
         return sent;"
    )
}

/// How Kai's app script wraps a view's message: `{"ev":"rpc","m":<the message's JSON>}`, the
/// message as is (no second escaping), so its size is the text's less the envelope.
pub const RPC_OPEN: &str = r#"{"ev":"rpc","m":"#;

/// A message of an `overlay_app` page larger than a view's message may be (`max` bytes, Kai's
/// app script refuses those before posting) in its envelope: the GTK side refuses it before it
/// parses anything.
pub fn rpc_too_big(text: &str, max: u64) -> bool {
    text.len() as u64 > max.saturating_add(RPC_OPEN.len() as u64 + 1)
}

/// What the host script posts.
#[derive(Clone, PartialEq)]
pub enum PageMsg {
    Ready,
    /// The text's natural height in CSS px.
    Size(u32),
    /// The caller's frame loaded.
    Loaded,
    /// An `overlay_app` view's message (its JSON text), relayed by Kai's app script only when its
    /// source is the view's frame (K18). The view's content: its `Debug` says only its size.
    Rpc(String),
    Other(String),
}

impl std::fmt::Debug for PageMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PageMsg::Ready => write!(f, "Ready"),
            PageMsg::Size(h) => write!(f, "Size({h})"),
            PageMsg::Loaded => write!(f, "Loaded"),
            PageMsg::Rpc(m) => write!(f, "Rpc({} bytes)", m.len()),
            PageMsg::Other(t) => write!(f, "Other({t:?})"),
        }
    }
}

pub fn parse(text: &str) -> PageMsg {
    // A view's message, taken out whole (the bridge reads it).
    if let Some(m) = text
        .strip_prefix(RPC_OPEN)
        .and_then(|r| r.strip_suffix('}'))
    {
        return PageMsg::Rpc(m.to_owned());
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return PageMsg::Other(text.to_owned());
    };
    match v.get("ev").and_then(|e| e.as_str()) {
        Some("ready") => PageMsg::Ready,
        Some("loaded") => PageMsg::Loaded,
        Some("size") => match v.get("h").and_then(|h| h.as_u64()) {
            Some(h) => PageMsg::Size(h.min(u32::MAX as u64) as u32),
            None => PageMsg::Other(text.to_owned()),
        },
        _ => PageMsg::Other(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::kinds::Body;

    #[test]
    fn the_csps_are_section_10_1s() {
        assert_eq!(
            CONTENT_CSP,
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src \
             data: blob:; font-src data:"
        );
        // The host page runs no script of its own, frames only this scheme, and nothing reaches
        // the network.
        for d in HOST_CSP.split("; ") {
            let mut words = d.split(' ');
            let name = words.next().unwrap();
            let sources: Vec<&str> = words.collect();
            assert_ne!(name, "script-src", "{HOST_CSP}");
            assert!(
                sources
                    .iter()
                    .all(|s| ["'none'", "'self'", "kai-sandbox:", "data:", "blob:"].contains(s)),
                "{d}"
            );
        }
        assert!(HOST_CSP.starts_with("default-src 'none';"));
        assert!(HOST_CSP.contains("frame-src kai-sandbox:"));
    }

    #[test]
    fn the_host_script_frames_the_document_sandboxed_and_renders_text_as_text() {
        assert!(HOST_SCRIPT.contains(r#"setAttribute("sandbox", "allow-scripts")"#));
        assert!(!HOST_SCRIPT.contains("allow-same-origin"));
        assert!(!HOST_SCRIPT.contains("innerHTML"), "text is never markup");
        assert!(HOST_SCRIPT.contains("textContent"));
        assert!(
            !HOST_SCRIPT.contains("srcdoc"),
            "srcdoc would inherit the host's CSP"
        );
        let host = std::str::from_utf8(ASSETS[0].2).unwrap();
        assert!(!host.contains("<script"), "the host page has no script");
    }

    /// K18: Kai's app script relays only its own frame's messages, places the view as
    /// overlay_html's (scripts only, through the scheme), and has the handler's world only.
    #[test]
    fn the_app_script_answers_only_the_frame_it_placed() {
        let app = crate::widgets::app_bridge::HOST_SCRIPT;
        assert!(app.contains("e.source !== view.contentWindow"));
        assert!(app.contains("w.frame(src)"), "the same sandboxed frame");
        assert!(!app.contains("allow-same-origin") && !app.contains("srcdoc"));
        assert!(!app.contains("innerHTML"));
        assert!(app.contains(r#"post('{"ev":"rpc","m":' + text + "}")"#));
        assert!(
            app.contains("if (over(text))") && app.contains("code: -32600"),
            "a message over overlay.app_max_mb is refused before it is posted"
        );
    }

    #[test]
    fn a_view_goes_only_to_its_own_pages() {
        assert_eq!(navigation(3, HOST_URI, false), Ok(()));
        assert_eq!(navigation(3, "kai-sandbox://widget/frame/3", false), Ok(()));
        for (uri, scheme, why) in [
            (
                "kai-sandbox://widget/frame/4",
                "kai-sandbox",
                "not this widget's page",
            ),
            (
                "kai-sandbox://widget/frame/3?x",
                "kai-sandbox",
                "not this widget's page",
            ),
            (
                "kai://app/ring.html",
                "kai",
                "a widget goes nowhere but its own page",
            ),
            (
                "http://example.com/?secret",
                "http",
                "a widget goes nowhere but its own page",
            ),
            (
                "https://example.com/",
                "https",
                "a widget goes nowhere but its own page",
            ),
            (
                "file:///etc/passwd",
                "file",
                "a widget goes nowhere but its own page",
            ),
            (
                "data:text/html,<p>",
                "data",
                "a widget goes nowhere but its own page",
            ),
            (
                "blob:kai-sandbox://widget/x",
                "blob",
                "a widget goes nowhere but its own page",
            ),
            (
                "about:blank",
                "about",
                "a widget goes nowhere but its own page",
            ),
            (
                "javascript:alert(1)",
                "javascript",
                "a widget goes nowhere but its own page",
            ),
            ("", "(none)", "a widget goes nowhere but its own page"),
        ] {
            assert_eq!(
                navigation(3, uri, false),
                Err(Refused {
                    scheme: scheme.into(),
                    why
                }),
                "{uri}"
            );
        }
        assert_eq!(
            navigation(3, HOST_URI, true),
            Err(Refused {
                scheme: "kai-sandbox".into(),
                why: "a widget opens no window"
            })
        );
    }

    #[test]
    fn the_scheme_serves_the_host_page_and_a_document_only_to_its_view() {
        assert!(matches!(
            serve(HOST_URI, Some(1)),
            Served::Asset {
                mime: "text/html",
                ..
            }
        ));
        assert!(matches!(
            serve("kai-sandbox://widget/host.css", Some(1)),
            Served::Asset {
                mime: "text/css",
                ..
            }
        ));
        assert_eq!(serve(&frame_uri(5), Some(5)), Served::Frame(5));
        for (uri, view) in [
            (frame_uri(5), Some(6)),
            (frame_uri(5), None),
            ("kai-sandbox://widget/frame/05".to_owned(), Some(5)),
            ("kai-sandbox://widget/frame/5?x=1".to_owned(), Some(5)),
            ("kai-sandbox://widget/host.js".to_owned(), Some(5)),
            ("kai-sandbox://widget/../Cargo.toml".to_owned(), Some(5)),
            ("kai-sandbox://app/host.html".to_owned(), Some(5)),
            ("kai://app/ring.html".to_owned(), Some(5)),
            ("not a url".to_owned(), Some(5)),
        ] {
            assert_eq!(serve(&uri, view), Served::NotFound, "{uri} for {view:?}");
        }
    }

    #[test]
    fn the_filter_lets_only_three_schemes_through() {
        let rules: serde_json::Value = serde_json::from_str(FILTER_RULES).unwrap();
        let rules = rules.as_array().unwrap();
        assert_eq!(rules[0]["action"]["type"], "block");
        assert_eq!(rules[0]["trigger"]["url-filter"], ".*");
        let allowed: Vec<&str> = rules[1..]
            .iter()
            .map(|r| {
                assert_eq!(r["action"]["type"], "ignore-previous-rules");
                r["trigger"]["url-filter"].as_str().unwrap()
            })
            .collect();
        assert_eq!(allowed, ["^kai-sandbox:", "^data:", "^blob:"]);
        assert!(filter_id().starts_with("kai-widgets-"));
    }

    #[test]
    fn the_page_is_told_the_body_as_json() {
        let js = show_js(
            4,
            &Body::Text("a \"quoted\" </script> line\nnext".into()),
            360,
        );
        assert_eq!(
            js,
            r#"__kaiWidget.text("a \"quoted\" </script> line\nnext",360)"#
        );
        assert_eq!(
            show_js(4, &Body::Document("<p>".into()), 480),
            r#"__kaiWidget.frame("kai-sandbox://widget/frame/4")"#,
            "the document goes through the scheme, never into the host page"
        );
        assert_eq!(parse(r#"{"ev":"ready"}"#), PageMsg::Ready);
        assert_eq!(parse(r#"{"ev":"size","h":118}"#), PageMsg::Size(118));
        assert_eq!(parse(r#"{"ev":"loaded"}"#), PageMsg::Loaded);
        assert!(matches!(parse(r#"{"ev":"size"}"#), PageMsg::Other(_)));
        let rpc = parse(r#"{"ev":"rpc","m":{"jsonrpc":"2.0","id":1,"method":"ping"}}"#);
        assert_eq!(
            rpc,
            PageMsg::Rpc(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.into())
        );
        assert_eq!(
            format!("{rpc:?}"),
            "Rpc(40 bytes)",
            "a view's message is never shown"
        );
        assert_eq!(
            post_js(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            r#"__kaiWidget.post({"jsonrpc":"2.0","id":1,"result":{}})"#
        );
        assert!(matches!(parse("x"), PageMsg::Other(_)));
    }

    /// A view's message over `overlay.app_max_mb`: Kai's app script gets the bound with the view,
    /// and the GTK side refuses a bigger envelope before it parses it.
    #[test]
    fn an_app_views_message_is_bounded_before_it_is_read() {
        use crate::lmgw_link::mcp::Bound;
        use crate::widgets::app_bridge::{AppBody, Ends};
        let body = Body::App(AppBody {
            server: "fx".into(),
            html: "<p>".into(),
            ends: Ends {
                to_bridge: tokio::sync::mpsc::unbounded_channel().0,
                from_bridge: async_channel::unbounded().1,
            },
            notes: vec![],
            bound: Bound {
                bytes: 100,
                says: "overlay.app_max_mb = 1".into(),
            },
        });
        assert_eq!(
            show_js(5, &body, 480),
            r#"__kaiWidget.app("kai-sandbox://widget/frame/5",{"max":100,"says":"overlay.app_max_mb = 1"})"#
        );
        let fits = format!("{RPC_OPEN}{}}}", "x".repeat(100));
        assert!(!rpc_too_big(&fits, 100));
        assert_eq!(parse(&fits), PageMsg::Rpc("x".repeat(100)));
        let over = format!("{RPC_OPEN}{}}}", "x".repeat(101));
        assert!(rpc_too_big(&over, 100), "one byte more is refused unread");
        assert_eq!(
            teardown_js(r#"{"id":"kai-teardown"}"#),
            "const sent = __kaiWidget.post({\"id\":\"kai-teardown\"}); for (let i = 0; i < 2; \
             i++) await new Promise((r) => setTimeout(r, 0)); return sent;"
        );
    }

    #[test]
    fn headers_carry_the_type_and_the_csp() {
        let h = headers("text/html", CONTENT_CSP);
        assert_eq!(h[0], ("Content-Type", "text/html".to_owned()));
        assert_eq!(h[1], ("Content-Security-Policy", CONTENT_CSP.to_owned()));
        assert_eq!(h[2].1, "nosniff");
    }
}
