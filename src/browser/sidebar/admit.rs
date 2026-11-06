//! What Kai forwards for the sidebar (kai-firefox design §8.2): exactly the requests lmgw's Chat
//! (`lmgw-chat-ui` with the kit pieces it renders, lmgw's chat-plane design §1, §8.2, §9) makes,
//! by method and path. Every other request is refused 404 `not_admitted` with what it was, before
//! anything goes to lmgw.
//!
//! The table is the Chat's call sites as read in lmgw's `crates/lmgw-chat-ui` and
//! `crates/lmgw-ui-kit` (WP3b, checked against lmgw 0.6.0), not lmgw's whole
//! Chat plane: the profile editor's writes (`POST /chat/api/profiles` and its `/{id}`,
//! `/{id}/reset`, `/{id}/delete`, `/preview`, `/test`, `/speak`), which the Chat does not render
//! (it reads the list only), are not in it, nor is `GET /chat/api/profiles/{id}`, which nothing
//! calls, nor `GET /chat/api/folders` (the Chat reads the folders inside `GET /chat/api/threads`).
//! `/v1/realtime` is a WebSocket the sidebar does not proxy yet (§8.2), and the rest of `/v1` and
//! every `/api` route are not the Chat's.
//!
//! Segments: `{id}` is a whole number lmgw's `i64` reads (a temporary thread's is negative);
//! `{name}` (a tool label, a call's or a task's id) is one segment of URL characters. Every segment,
//! percent-decoded, is not empty, `.` or `..` and holds no `/` or `\`: a URL library turns `%2e%2e`
//! into `..` as it does `..`, so `…/tasks/%2e%2e/cancel` would reach another route. The query is
//! passed on as it came, checked only for its characters: lmgw reads it.
//!
//! **The routes that start a turn** name their thread, which the turn marking of design §8.3 takes
//! (`super::turns`): `send`, `continue`, `answer`, a message's `edit` and `regenerate` a new turn
//! ([`Start::New`]), `approvals` a resumed one ([`Start::Resume`]). An image card's action
//! (`/chat/api/attachments/{id}/media`) starts none: it runs with no model in the loop (lmgw's
//! chat-media design §8.2).

use super::turns::Start;
use crate::lmgw_link::proxy::Method;

/// One segment of a route's path.
#[derive(Clone, Copy)]
enum Seg {
    Lit(&'static str),
    /// A whole number (`{id}`).
    Id,
    /// One segment of URL characters (`{label}`, `{call_id}`, `{task}`).
    Name,
}

use Seg::{Id, Lit, Name};

/// A route: its method, its segments, and how it starts a turn of the thread its first `{id}`
/// names, when it starts one.
struct Route {
    method: Method,
    segs: &'static [Seg],
    turn: Option<Start>,
}

const fn get(segs: &'static [Seg]) -> Route {
    Route {
        method: Method::Get,
        segs,
        turn: None,
    }
}

const fn post(segs: &'static [Seg]) -> Route {
    Route {
        method: Method::Post,
        segs,
        turn: None,
    }
}

const fn turn(segs: &'static [Seg]) -> Route {
    Route {
        method: Method::Post,
        segs,
        turn: Some(Start::New),
    }
}

const fn resume(segs: &'static [Seg]) -> Route {
    Route {
        method: Method::Post,
        segs,
        turn: Some(Start::Resume),
    }
}

const C: Seg = Lit("chat");
const A: Seg = Lit("api");
const T: Seg = Lit("threads");

/// The Chat's requests (module doc).
const ROUTES: &[Route] = &[
    // The feed and the reads beside the threads.
    get(&[C, A, Lit("feed")]),
    get(&[C, A, Lit("settings")]),
    get(&[C, A, Lit("tools")]),
    get(&[C, A, Lit("tools"), Name]),
    get(&[C, A, Lit("knowledge"), Lit("bases")]),
    get(&[C, A, Lit("knowledge"), Lit("files"), Id, Lit("text")]),
    get(&[C, A, Lit("knowledge"), Lit("files"), Id, Lit("original")]),
    get(&[C, A, Lit("media"), Lit("defaults")]),
    get(&[C, A, Lit("profiles")]),
    get(&[C, A, Lit("search")]),
    get(&[C, A, Lit("export")]),
    // Threads.
    get(&[C, A, T]),
    post(&[C, A, T]),
    get(&[C, A, T, Lit("rows")]),
    get(&[C, A, T, Id]),
    get(&[C, A, T, Id, Lit("export")]),
    get(&[C, A, T, Id, Lit("tasks")]),
    post(&[C, A, T, Id, Lit("settings")]),
    post(&[C, A, T, Id, Lit("delete")]),
    post(&[C, A, T, Id, Lit("pin")]),
    post(&[C, A, T, Id, Lit("title")]),
    post(&[C, A, T, Id, Lit("archive")]),
    post(&[C, A, T, Id, Lit("persist")]),
    post(&[C, A, T, Id, Lit("move")]),
    post(&[C, A, T, Id, Lit("attachments")]),
    post(&[C, A, T, Id, Lit("transcribe")]),
    post(&[C, A, T, Id, Lit("voice"), Lit("warm")]),
    post(&[C, A, T, Id, Lit("speech"), Lit("stop")]),
    post(&[C, A, T, Id, Lit("tasks"), Name, Lit("cancel")]),
    post(&[C, A, T, Id, Lit("media"), Name, Lit("cancel")]),
    post(&[C, A, T, Id, Lit("messages"), Id, Lit("delete")]),
    post(&[C, A, T, Id, Lit("messages"), Id, Lit("speak")]),
    // Turns.
    turn(&[C, A, T, Id, Lit("send")]),
    turn(&[C, A, T, Id, Lit("continue")]),
    resume(&[C, A, T, Id, Lit("approvals")]),
    turn(&[C, A, T, Id, Lit("answer")]),
    turn(&[C, A, T, Id, Lit("messages"), Id, Lit("edit")]),
    turn(&[C, A, T, Id, Lit("messages"), Id, Lit("regenerate")]),
    // Folders (the Chat reads them inside `GET /chat/api/threads`).
    post(&[C, A, Lit("folders")]),
    post(&[C, A, Lit("folders"), Id]),
    post(&[C, A, Lit("folders"), Id, Lit("current")]),
    post(&[C, A, Lit("folders"), Id, Lit("delete")]),
    get(&[C, A, Lit("folders"), Id, Lit("export")]),
    // Attachments.
    get(&[C, A, Lit("attachments"), Id]),
    get(&[C, A, Lit("attachments"), Id, Lit("text")]),
    post(&[C, A, Lit("attachments"), Id, Lit("delete")]),
    post(&[C, A, Lit("attachments"), Id, Lit("mode")]),
    post(&[C, A, Lit("attachments"), Id, Lit("transcribe")]),
    post(&[C, A, Lit("attachments"), Id, Lit("media")]),
    // The inference plane's two reads: the model catalog and the voice picker.
    get(&[Lit("v1"), Lit("models")]),
    get(&[Lit("v1"), Lit("audio"), Lit("voices")]),
];

/// An admitted request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub method: Method,
    /// The path and query as they came.
    pub path: String,
    /// The thread whose turn it starts and how (module doc), when it starts one.
    pub turn: Option<(i64, Start)>,
}

/// Why a request is not forwarded: (status, code, message).
pub type Refused = (u16, &'static str, String);

/// `method` and `path` (with its query) checked against the Chat's requests (module doc).
pub fn admit(method: &str, path: &str) -> Result<Admitted, Refused> {
    let not = || {
        (
            404,
            "not_admitted",
            format!(
                "Kai forwards only the Chat's requests to lmgw for the sidebar; {method} {path} is \
                 not one"
            ),
        )
    };
    let m = match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => return Err(not()),
    };
    let (route_path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    if query.is_some_and(|q| !q.bytes().all(query_char)) {
        return Err(not());
    }
    let Some(rest) = route_path.strip_prefix('/') else {
        return Err(not());
    };
    let segs: Vec<&str> = rest.split('/').collect();
    if !segs.iter().all(|s| segment(s)) {
        return Err(not());
    }
    let route = ROUTES
        .iter()
        .find(|r| r.method == m && fits(r.segs, &segs))
        .ok_or_else(not)?;
    let turn = route.turn.and_then(|start| {
        let thread = route
            .segs
            .iter()
            .zip(&segs)
            .find(|(s, _)| matches!(s, Id))
            .and_then(|(_, v)| v.parse::<i64>().ok())?;
        // A temporary thread is never in the feed (lmgw's L7): no turn of it to mark.
        (thread >= 0).then_some((thread, start))
    });
    Ok(Admitted {
        method: m,
        path: path.to_owned(),
        turn,
    })
}

fn fits(pattern: &[Seg], segs: &[&str]) -> bool {
    pattern.len() == segs.len()
        && pattern.iter().zip(segs).all(|(p, s)| match p {
            Lit(l) => l == s,
            Id => id(s),
            Name => true,
        })
}

/// A whole number lmgw's `i64` path parameters read: an optional `-` and digits, within `i64`.
fn id(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && s.parse::<i64>().is_ok()
}

/// One path segment (module doc): RFC 3986's `pchar`s and a `%` only before two hex digits; decoded,
/// not empty, `.` or `..`, and no `/` or `\`.
fn segment(s: &str) -> bool {
    let b = s.as_bytes();
    let mut decoded = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let Some(hex) = s
                    .get(i + 1..i + 3)
                    .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                else {
                    return false;
                };
                let Ok(byte) = u8::from_str_radix(hex, 16) else {
                    return false;
                };
                decoded.push(byte);
                i += 3;
            }
            c if pchar(c) => {
                decoded.push(c);
                i += 1;
            }
            _ => return false,
        }
    }
    !(decoded.is_empty()
        || decoded == b"."
        || decoded == b".."
        || decoded.contains(&b'/')
        || decoded.contains(&b'\\'))
}

/// RFC 3986's unreserved and sub-delims characters, `:` and `@`.
fn pchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&c)
}

/// A query's characters: `pchar`s, `/`, `?` and `%` (lmgw reads the encoding).
fn query_char(c: u8) -> bool {
    pchar(c) || b"/?%".contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chats_requests_are_admitted() {
        for (m, p) in [
            ("GET", "/chat/api/feed"),
            ("GET", "/chat/api/settings"),
            ("GET", "/chat/api/threads?archived=1"),
            ("GET", "/chat/api/threads/rows?ids=1,2,3"),
            ("GET", "/chat/api/threads/7"),
            ("GET", "/chat/api/threads/-3/export?format=json"),
            ("GET", "/chat/api/folders/2/export?format=json&archived=all"),
            ("GET", "/chat/api/export?format=md&archived=1"),
            ("GET", "/chat/api/search?q=caf%C3%A9&archived=0&offset=0"),
            ("GET", "/chat/api/tools/github"),
            (
                "GET",
                "/chat/api/knowledge/files/3/text?chunk=abc&span_start=10",
            ),
            ("GET", "/chat/api/knowledge/files/3/original"),
            ("GET", "/chat/api/media/defaults?model=img"),
            ("GET", "/chat/api/attachments/9"),
            ("GET", "/chat/api/attachments/9/text"),
            ("GET", "/chat/api/profiles"),
            ("GET", "/v1/models"),
            ("GET", "/v1/audio/voices?model=tts%2Fa+b"),
            ("POST", "/chat/api/threads"),
            ("POST", "/chat/api/threads/7/attachments?name=a%20b.png"),
            ("POST", "/chat/api/threads/7/media/call_ab-1/cancel"),
            ("POST", "/chat/api/threads/7/tasks/4/cancel"),
            ("POST", "/chat/api/threads/7/messages/30/speak"),
            ("POST", "/chat/api/folders/3/current"),
            ("POST", "/chat/api/attachments/9/media"),
            // lmgw's i64 ids, the whole range.
            ("GET", "/chat/api/threads/9223372036854775807"),
            ("GET", "/chat/api/threads/-9223372036854775808/export"),
            ("POST", "/chat/api/threads/7/tasks/a.b/cancel"),
            ("POST", "/chat/api/threads/7/tasks/...%2e/cancel"),
        ] {
            let a = admit(m, p).unwrap_or_else(|e| panic!("{m} {p}: {e:?}"));
            assert_eq!((a.path.as_str(), a.turn), (p, None), "{m} {p}");
        }
        // The count the docs give (kai-firefox's operations.md, Kai's design WP24).
        assert_eq!(ROUTES.len(), 51);
    }

    #[test]
    fn the_routes_that_start_a_turn_name_their_thread() {
        for p in [
            "/chat/api/threads/12/send",
            "/chat/api/threads/12/continue",
            "/chat/api/threads/12/answer",
            "/chat/api/threads/12/messages/40/edit",
            "/chat/api/threads/12/messages/40/regenerate",
        ] {
            assert_eq!(
                admit("POST", p).unwrap().turn,
                Some((12, Start::New)),
                "{p}"
            );
        }
        assert_eq!(
            admit("POST", "/chat/api/threads/12/approvals")
                .unwrap()
                .turn,
            Some((12, Start::Resume))
        );
        // A temporary thread's turn is never in the feed.
        assert_eq!(
            admit("POST", "/chat/api/threads/-4/approvals")
                .unwrap()
                .turn,
            None
        );
        assert_eq!(
            admit("POST", "/chat/api/threads/-4/send").unwrap().turn,
            None
        );
    }

    #[test]
    fn everything_else_is_refused_before_it_goes() {
        for (m, p) in [
            // The admin plane, the rest of /v1, the realtime socket.
            ("GET", "/api/settings-full"),
            ("GET", "/api/events"),
            ("POST", "/v1/chat/completions"),
            ("GET", "/v1/realtime?chat_thread=7"),
            // The profile editor's writes and a read nothing calls.
            ("POST", "/chat/api/profiles"),
            ("POST", "/chat/api/profiles/3"),
            ("POST", "/chat/api/profiles/speak"),
            ("GET", "/chat/api/profiles/3"),
            // The folders' list: the Chat reads it inside the threads' list.
            ("GET", "/chat/api/folders"),
            // A route with the wrong method, or another method.
            ("POST", "/chat/api/feed"),
            ("GET", "/chat/api/threads/7/send"),
            ("PUT", "/chat/api/threads/7"),
            ("DELETE", "/chat/api/threads/7"),
            // Paths that are not the route's.
            ("GET", "/"),
            ("GET", "chat/api/feed"),
            ("GET", "/chat/api/feed/"),
            ("GET", "/chat//api/feed"),
            ("GET", "/chat/api/threads/x"),
            ("GET", "/chat/api/threads/9223372036854775808"),
            ("GET", "/chat/api/threads/-9223372036854775809"),
            ("GET", "/chat/api/threads/+7"),
            ("GET", "/chat/api/threads/-"),
            ("GET", "/chat/api/tools/.."),
            ("GET", "/chat/api/tools/a%2Fb"),
            ("GET", "/chat/api/tools/a%5cb"),
            ("GET", "/chat/api/tools/a%zz"),
            ("GET", "/chat/api/tools/a%2"),
            // Dot segments however they are encoded: a URL library reads them all as `.` or `..`.
            ("GET", "/chat/api/tools/."),
            ("GET", "/chat/api/tools/%2e"),
            ("GET", "/chat/api/tools/%2E"),
            ("GET", "/chat/api/tools/%2e%2e"),
            ("GET", "/chat/api/tools/%2E%2E"),
            ("GET", "/chat/api/tools/.%2e"),
            ("GET", "/chat/api/tools/%2e."),
            ("POST", "/chat/api/threads/7/tasks/%2e%2e/cancel"),
            ("POST", "/chat/api/threads/7/media/.%2E/cancel"),
            ("GET", "/chat/api/tools/a b"),
            ("GET", "/chat/api/threads/../../api/events"),
            ("GET", "/chat/api/feed#x"),
            ("GET", "/chat/api/search?q=a b"),
            ("GET", "/chat/api/search?q=\u{e4}"),
        ] {
            let (status, code, message) = admit(m, p).unwrap_err();
            assert_eq!((status, code), (404, "not_admitted"), "{m} {p}");
            assert!(message.contains(&format!("{m} {p}")), "{message}");
        }
    }
}
