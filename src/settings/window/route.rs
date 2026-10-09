//! What a request to `kai://…` in the window's context gets, as a function of the request alone
//! (no GTK, no core): the page's files, `GET /api/tools`, `POST /api/apply`, which hands the
//! operation to the core, `GET /api/conversation` (the core's facts about Kai's conversation), and
//! the profile editor's requests to lmgw under `/lmgw/` (`profiles`), which the core forwards with
//! the device key, and the remote-control permission (`remote`: `GET /api/remote-control`, `POST
//! /api/remote-control/forget`), which the core looks at or forgets. Anything else is refused. The window's WebContext serves this scheme and nothing
//! else, so the page reaches Kai, and through it lmgw, through these things only.
//!
//! **Origins.** The authority must be `settings`: the ring's pages (`kai://app/`) and every other
//! origin get a 403 and no operation is run. A request that names an origin (`Origin`) must name
//! this one, and a fetch metadata header (`Sec-Fetch-Site`), when WebKit sends it, must say
//! same-origin or none (a navigation the owner started). These checks come first, for lmgw's
//! requests as for Kai's own. The operations are `settings::Request`'s
//! and no others: a body that is none of them is a 400. Switching a routine on is refused (403):
//! the page does not show routines, and a routine goes on only after the owner saw it in full.

use std::borrow::Cow;

use super::assets;
use super::profiles;
use super::tools;
use crate::lmgw_link::proxy::Forward;
use crate::settings::Request;

/// Kai's scheme (K10; the ring's is `kai://app/`).
pub const SCHEME: &str = "kai";
/// The authority of the window's page: `kai://settings/`.
pub const HOST: &str = "settings";

/// What the scheme handler was asked.
pub struct Incoming<'a> {
    pub method: &'a str,
    pub uri: &'a str,
    pub origin: Option<&'a str>,
    pub fetch_site: Option<&'a str>,
    pub body: &'a [u8],
}

/// A finished answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub mime: &'static str,
    pub body: Vec<u8>,
    /// The CSP header of a page (K10); the API's answers carry none.
    pub csp: Option<String>,
}

impl Answer {
    fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            mime: "text/plain",
            body: text.as_bytes().to_vec(),
            csp: None,
        }
    }

    /// A refusal as the page reads it (`{"refused": …}`).
    pub fn refused(status: u16, why: &str) -> Self {
        Self {
            status,
            mime: "application/json",
            body: serde_json::json!({ "refused": why })
                .to_string()
                .into_bytes(),
            csp: None,
        }
    }

    pub fn json<T: serde::Serialize>(value: &T) -> Self {
        Self {
            status: 200,
            mime: "application/json",
            body: serde_json::to_vec(value).expect("the answers serialize"),
            csp: None,
        }
    }
}

/// What to do with a request.
#[derive(Debug, PartialEq)]
pub enum Routed {
    Answer(Answer),
    /// Run it in the core, then answer with the result ([`Answer::json`] or a 422).
    Apply(Request),
    /// The core's facts about Kai's conversation ([`profiles::Conversation`]).
    Conversation,
    /// A profile-editor request the core sends to lmgw with the device key; lmgw's answer is the
    /// page's.
    Proxy(Forward),
    /// The remote-control permission (`remote`): looked at, or forgotten (`forget`).
    RemoteControl {
        forget: bool,
    },
}

pub fn route(req: &Incoming<'_>) -> Routed {
    let Ok(url) = url::Url::parse(req.uri) else {
        return Routed::Answer(Answer::text(400, "not a URL"));
    };
    if url.scheme() != SCHEME || url.host_str() != Some(HOST) {
        return Routed::Answer(Answer::text(403, "not this window's origin"));
    }
    if let Some(origin) = req.origin
        && origin != format!("{SCHEME}://{HOST}")
    {
        return Routed::Answer(Answer::refused(403, "a request from another origin"));
    }
    if let Some(site) = req.fetch_site
        && !matches!(site, "same-origin" | "none")
    {
        return Routed::Answer(Answer::refused(403, "a request from another site"));
    }
    if profiles::under_prefix(url.path()) {
        return match profiles::forward(req.method, &url, req.body) {
            Ok(f) => Routed::Proxy(f),
            Err(a) => Routed::Answer(a),
        };
    }
    match (req.method, url.path()) {
        ("GET", "/api/tools") => Routed::Answer(Answer::json(&tools::all())),
        ("GET", "/api/conversation") => Routed::Conversation,
        ("GET", "/api/remote-control") => Routed::RemoteControl { forget: false },
        ("POST", "/api/remote-control/forget") => Routed::RemoteControl { forget: true },
        ("POST", "/api/apply") => match serde_json::from_slice::<Request>(req.body) {
            // The page does not show routines yet: switching one on is the tray's and
            // routine_enable's, which show it in full first (switching one off is fine).
            Ok(Request::Routine { enabled: true, .. }) => Routed::Answer(Answer::refused(
                403,
                "this window does not show routines yet; switch one on from the tray's Routines \
                 menu, which shows it in full first",
            )),
            Ok(request) => Routed::Apply(request),
            Err(e) => Routed::Answer(Answer::refused(400, &format!("not an operation: {e}"))),
        },
        (
            _,
            "/api/tools"
            | "/api/apply"
            | "/api/conversation"
            | "/api/remote-control"
            | "/api/remote-control/forget",
        ) => Routed::Answer(Answer::text(405, "wrong method")),
        ("GET", path) if !path.starts_with("/api/") => Routed::Answer(page_or_asset(path)),
        _ => Routed::Answer(Answer::text(404, "not found")),
    }
}

fn page_or_asset(path: &str) -> Answer {
    let Some(asset) = assets::get(path) else {
        if path == "/" && !assets::built() {
            return Answer::text(
                404,
                "Kai's settings page is not built into this binary: run `just ui` (trunk), \
                 then start Kai again",
            );
        }
        return Answer::text(404, "not found");
    };
    // Every response carries the page's CSP, the stylesheets and scripts as well.
    let index = assets::get("/")
        .map(|i| i.body)
        .unwrap_or(Cow::Borrowed(&[]));
    Answer {
        status: 200,
        mime: asset.mime,
        body: asset.body.into_owned(),
        csp: Some(assets::csp(&index)),
    }
}
