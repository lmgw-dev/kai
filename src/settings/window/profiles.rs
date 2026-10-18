//! The Personality page's two needs from the scheme handler (WP21b, ruling 29): lmgw's profile
//! editor (`lmgw-ui-kit`'s `ProfilesPanel`) reaches lmgw through Kai, and the page's header says
//! which profile Kai's conversation talks with.
//!
//! **The proxy.** The kit calls lmgw's routes relative to the base the page configures
//! (`kit::http::configure`), which is [`PREFIX`]: `kai://settings/lmgw/<lmgw's path>`. [`forward`]
//! admits exactly the requests the kit makes (lmgw's personality-profiles design §5) and nothing
//! else under the prefix:
//!
//! | Method | lmgw's path |
//! |---|---|
//! | GET | `/chat/api/profiles` |
//! | POST | `/chat/api/profiles` (create, or the built-in again) |
//! | POST | `/chat/api/profiles/{id}`, `…/{id}/reset`, `…/{id}/delete` |
//! | POST | `/chat/api/profiles/preview`, `…/test`, `…/speak` |
//! | GET | `/v1/models` |
//! | GET | `/v1/audio/voices?model=<alias>` (that one parameter) |
//!
//! Any other method, path or query (lmgw's `GET /chat/api/profiles/{id}`, which the kit does not
//! call, included) is a 404, a POST whose body is not JSON a 400; the origin checks of
//! `/api/apply` run first (`route`). The core sends an admitted request over the device link
//! (`crate::lmgw_link::proxy`); the page holds no credential and opens no connection.
//!
//! **The conversation's facts** (`GET /api/conversation`, [`Conversation`]): what the core knows of
//! Kai's conversation (WP22's state): the pairing, the folder, the thread, the folder's model and
//! the profile the tray's Personality radio checks.

use serde::Serialize;

use super::route::Answer;
use crate::lmgw_link::proxy::{Forward, Method, Proxied};

/// Where lmgw's routes are under the window's origin: the kit's configured base.
pub const PREFIX: &str = "/lmgw";

/// Whether `path` is under [`PREFIX`].
pub fn under_prefix(path: &str) -> bool {
    path == PREFIX
        || path
            .strip_prefix(PREFIX)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// lmgw's answer (or Kai's refusal of a request for it) as the page gets it: no CSP, as any
/// answer of an API route.
impl From<Proxied> for Answer {
    fn from(p: Proxied) -> Self {
        Self {
            status: p.status,
            mime: p.mime,
            body: p.body,
            csp: None,
        }
    }
}

/// A Kai refusal in lmgw's error shape (`{code, message}`), which the kit reads.
fn refused(status: u16, code: &str, message: &str) -> Answer {
    Proxied::refused(status, code, message).into()
}

fn not_forwarded(method: &str, path: &str) -> Answer {
    refused(
        404,
        "not_found",
        &format!(
            "Kai forwards only the profile editor's requests to lmgw; {method} {path} is not one"
        ),
    )
}

/// The request for lmgw, or the answer that refuses it (module doc's table). `url` is the whole
/// `kai://settings/lmgw/…` URL, already checked for this window's origin.
pub fn forward(method: &str, url: &url::Url, body: &[u8]) -> Result<Forward, Answer> {
    let full = url.path();
    let path = full.strip_prefix(PREFIX).unwrap_or_default();
    let Some(rest) = path.strip_prefix('/') else {
        return Err(not_forwarded(method, full));
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let id = |s: &str| !s.is_empty() && s.len() <= 18 && s.bytes().all(|b| b.is_ascii_digit());
    let admitted = match (method, segments.as_slice()) {
        ("GET", ["chat", "api", "profiles"]) => Some(Method::Get),
        ("POST", ["chat", "api", "profiles"]) => Some(Method::Post),
        ("POST", ["chat", "api", "profiles", "preview" | "test" | "speak"]) => Some(Method::Post),
        ("POST", ["chat", "api", "profiles", n]) if id(n) => Some(Method::Post),
        ("POST", ["chat", "api", "profiles", n, "reset" | "delete"]) if id(n) => Some(Method::Post),
        ("GET", ["v1", "models"]) => Some(Method::Get),
        ("GET", ["v1", "audio", "voices"]) => Some(Method::Get),
        _ => None,
    };
    let Some(m) = admitted else {
        return Err(not_forwarded(method, full));
    };
    // The query: only the voices' one `model`, written again as lmgw reads it; none elsewhere.
    let voices = segments == ["v1", "audio", "voices"];
    let query = match (voices, url.query()) {
        (false, None) => String::new(),
        (true, Some(_)) => {
            let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            match pairs.as_slice() {
                [(k, v)] if k == "model" && !v.is_empty() => {
                    let encoded = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("model", v)
                        .finish();
                    format!("?{encoded}")
                }
                _ => return Err(not_forwarded(method, full)),
            }
        }
        _ => return Err(not_forwarded(method, full)),
    };
    let body = match m {
        Method::Get if !body.is_empty() => return Err(not_forwarded(method, full)),
        Method::Get => Vec::new(),
        Method::Post => {
            if let Err(e) = serde_json::from_slice::<serde::de::IgnoredAny>(body) {
                return Err(refused(
                    400,
                    "invalid_json",
                    &format!("the request's body is not JSON: {e}"),
                ));
            }
            body.to_vec()
        }
    };
    Ok(Forward {
        method: m,
        path: format!("{path}{query}"),
        body,
    })
}

/// `GET /api/conversation`: Kai's conversation as the core knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conversation {
    /// Kai holds a working pairing; otherwise `why` says what the owner can do.
    pub paired: bool,
    pub why: Option<String>,
    /// The paired gateway (the pairing's first URL).
    pub gateway: Option<String>,
    /// The conversation's Chat folder, once Kai found it.
    pub folder: Option<String>,
    /// The conversation's current thread, once a session or a lookup named it.
    pub thread_id: Option<i64>,
    /// The folder's model (`defaults.model_alias`), the one a thread of it talks with.
    pub model: Option<String>,
    /// The tray's checked Personality item ("Concise", "Default (global: Concise)"); `None` while
    /// Kai knows no profile.
    pub profile: Option<String>,
}

#[cfg(test)]
mod tests;
