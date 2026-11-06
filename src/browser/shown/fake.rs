//! A linked Firefox for the tests of Kai's site rules over the desktop's tools: a real link in
//! [`Links`] whose extension answers `tabs.list@1` with the listing a test sets (or with the error
//! it sets) and `context.get@1` with the context it sets (an error until it sets one), and counts
//! how often each was asked.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::super::links::{LinkInfo, Links, Outgoing};
use super::super::tabs::LIST;
use super::Shown;
use crate::browser::settings::{BrowserConfig, Sites};
use crate::host::policy::{Policy, SharedPolicy};
use crate::host::registry::Changes;
use crate::host::settings::PolicyConfig;

/// One fake Firefox (module doc).
pub struct FakeFirefox {
    pub links: Links,
    listing: Arc<Mutex<Value>>,
    lists: Arc<AtomicUsize>,
    context: Arc<Mutex<Option<Value>>>,
    contexts: Arc<AtomicUsize>,
}

/// `context.get@1`'s answer: the user's tab at `url`, its window with the focus (`focused`), and
/// the focused element `focus` (an Element; with `origin` in a child frame), `unread` frames.
pub fn context(url: &str, focused: bool, focus: Value, unread: u64) -> Value {
    json!({"tab": {"id": 1, "window": 1, "title": "t", "url": url, "active": true, "user": true},
           "window": {"id": 1, "focused": focused},
           "focus": focus, "frames": {"read": 1, "unread": unread, "unread_frames": []},
           "page_error": null})
}

/// A tab in front of window `window` as `tabs.list@1` reports it (`user` for the user's tab).
pub fn front(id: i64, window: i64, title: &str, url: &str, user: bool) -> Value {
    json!({"id": id, "window": window, "index": 0, "title": title, "url": url,
           "active": true, "user": user})
}

/// A tab behind another in window `window`.
pub fn behind(id: i64, window: i64, title: &str, url: &str) -> Value {
    json!({"id": id, "window": window, "index": 1, "title": title, "url": url,
           "active": false, "user": false})
}

/// `tabs.list@1`'s answer: `tabs`, the windows by id with the one that has the focus.
pub fn listing(tabs: Vec<Value>, windows: &[i64], focused: Option<i64>) -> Value {
    let user = tabs
        .iter()
        .find(|t| t["user"] == true)
        .map(|t| t["id"].clone());
    let windows: Vec<Value> = windows
        .iter()
        .map(|w| json!({"id": w, "focused": Some(*w) == focused}))
        .collect();
    json!({"tabs": tabs, "user_tab": user, "windows": windows})
}

impl FakeFirefox {
    /// A Firefox whose main process is `pid`, linked in a set of links of its own.
    pub fn start(pid: Option<u32>, listing: Value) -> Self {
        Self::join(&Links::new(Changes::new().0), pid, listing)
    }

    /// A Firefox whose main process is `pid`, linked in `links`; in a tokio runtime.
    pub fn join(links: &Links, pid: Option<u32>, listing: Value) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
        let link = links.add(
            LinkInfo {
                id: 0,
                profile: format!("profile-{}", pid.unwrap_or_default()),
                ext_version: "0.1.0".into(),
                firefox: "159.0".into(),
                protocol: 1,
                ops: vec![LIST.into(), super::focus::GET.into()],
                firefox_pid: pid,
            },
            tx,
        );
        let listing = Arc::new(Mutex::new(listing));
        let lists = Arc::new(AtomicUsize::new(0));
        let context = Arc::new(Mutex::new(None::<Value>));
        let contexts = Arc::new(AtomicUsize::new(0));
        let (answer, counted, served) = (listing.clone(), lists.clone(), links.clone());
        let (said, asked) = (context.clone(), contexts.clone());
        tokio::spawn(async move {
            while let Some(out) = rx.recv().await {
                let Ok(frame) = serde_json::from_str::<Value>(&out.text) else {
                    continue;
                };
                if frame["t"] != "op" {
                    continue;
                }
                let now = if frame["op"] == LIST {
                    counted.fetch_add(1, Ordering::SeqCst);
                    answer.lock().unwrap().clone()
                } else if frame["op"] == super::focus::GET {
                    asked.fetch_add(1, Ordering::SeqCst);
                    said.lock().unwrap().clone().unwrap_or_else(|| {
                        json!({"error": {"code": "unset", "message": "the test set no context"}})
                    })
                } else {
                    continue;
                };
                let reply = match now.get("error") {
                    Some(e) => json!({"id": frame["id"], "error": e}),
                    None => json!({"id": frame["id"], "ok": now}),
                };
                if let Value::Object(map) = reply {
                    served.op_result(link, map);
                }
            }
        });
        Self {
            links: links.clone(),
            listing,
            lists,
            context,
            contexts,
        }
    }

    /// What `context.get@1` answers from now on: a context ([`context`]) or `{"error": …}`.
    pub fn set_context(&self, context: Value) {
        *self.context.lock().unwrap() = Some(context);
    }

    /// How often `context.get@1` was asked.
    pub fn contexts(&self) -> usize {
        self.contexts.load(Ordering::SeqCst)
    }

    /// What `tabs.list@1` answers from now on: a listing, or `{"error": {code, message}}`.
    pub fn set(&self, listing: Value) {
        *self.listing.lock().unwrap() = listing;
    }

    /// How often the tabs were listed.
    pub fn lists(&self) -> usize {
        self.lists.load(Ordering::SeqCst)
    }
}

/// A policy with these site rules.
pub fn policy(none: &[&str], read: &[&str]) -> SharedPolicy {
    let v = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
    let browser = BrowserConfig {
        sites: Sites {
            none: v(none),
            read: v(read),
            act: Vec::new(),
        },
        ..BrowserConfig::default()
    };
    Arc::new(std::sync::RwLock::new(
        Policy::new(&PolicyConfig::default()).with_browser(&browser),
    ))
}

/// Kai's site rules over `links` with these rules.
pub fn shown(links: &Links, none: &[&str], read: &[&str]) -> Shown {
    Shown::new(links.clone(), policy(none, read))
}
