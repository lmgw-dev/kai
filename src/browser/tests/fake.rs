//! The browser provider's rig: a fake Kai for Firefox that keeps a small model of Firefox's tabs
//! and answers the contract's operations (WP4a: `tabs.*@1`, `context.get@1`,
//! `element.describe@1` from its elements; WP4b's reading operations as a test's twists; WP6's
//! acting operations from its elements, with the network gate's questions it is given and the
//! answers Kai sends) on a real `browser.sock` connection, and Kai's host built on the real
//! registry and dispatch (policy, preparation, confirmation, handler) with a confirmer that records
//! what it is asked and answers as the test says (yes unless told), and a desk that records the
//! trusted path's steps.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{Client, Rig, WAIT, frame, hello};
use crate::browser::provider::BrowserTools;
use crate::browser::settings::BrowserConfig;
use crate::browser::tools::Route;
use crate::host::HostLink;
use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::host::local::Local;
use crate::host::policy::{Caller, Policy, SharedPolicy};
use crate::host::registry::{Block, Reply};
use crate::host::settings::{Named, PolicyConfig};

/// Every operation of the contract's first version (WP4a), and `history.seen@1`, which the
/// navigations' commit guard needs (WP6).
pub const WP4A: &[&str] = &[
    "tabs.list@1",
    "tabs.open@1",
    "tabs.go@1",
    "tabs.switch@1",
    "tabs.close@1",
    "tabs.back@1",
    "tabs.forward@1",
    "tabs.reload@1",
    "tabs.mute@1",
    "context.get@1",
    "element.describe@1",
    "history.seen@1",
];

/// The reading, hint and pointing operations (WP4b).
pub const WP4B: &[&str] = &[
    "page.outline@1",
    "page.controls@1",
    "page.links@1",
    "page.fields@1",
    "page.text@1",
    "page.text_all@1",
    "page.text_visible@1",
    "element.text@1",
    "table.read@1",
    "page.selection@1",
    "page.find@1",
    "pdf.bytes@1",
    "hints.show@1",
    "hints.hide@1",
    "element.point@1",
];

/// The acting operations, the focus check, the gate and the input spans (WP6).
pub const WP6: &[&str] = &[
    "input.begin@1",
    "input.end@1",
    "element.press@1",
    "element.fill@1",
    "element.choose@1",
    "element.check@1",
    "element.file@1",
    "element.focus@1",
    "element.hover@1",
    "element.drag@1",
    "element.show@1",
    "element.scroll@1",
    "page.scroll@1",
    "page.key@1",
    "focus.check@1",
    "gate.result@1",
];

/// Firefox's own features, the source and the DOM, the style sheets, the scripts and the
/// screenshots (WP7).
pub const WP7: &[&str] = &[
    "page.see@1",
    "element.see@1",
    "page.zoom@1",
    "reader.view@1",
    "bookmarks.find@1",
    "history.find@1",
    "downloads.list@1",
    "page.source@1",
    "page.dom@1",
    "element.dom@1",
    "page.css_check@1",
    "page.css@1",
    "page.css_remove@1",
    "page.script@1",
    "page.script_main@1",
];

/// The press through accessibility and Firefox's own UI (WP12).
pub const WP12: &[&str] = &["element.mark@1", "element.unmark@1", "gate.open@1"];

/// Every operation the contract has so far.
pub fn all_ops() -> Vec<&'static str> {
    WP4A.iter()
        .chain(WP4B)
        .chain(WP6)
        .chain(WP7)
        .chain(WP12)
        .copied()
        .collect()
}

/// The tools those operations serve, in the contract's order.
pub const BUILT: &[&str] = &[
    "browser_context",
    "tab_list",
    "tab_open",
    "tab_go",
    "tab_switch",
    "tab_close",
    "tab_back",
    "tab_forward",
    "tab_reload",
    "tab_mute",
];

/// A tab as the extension reports it.
pub fn tab(id: i64, window: i64, title: &str, url: &str, user: bool) -> Value {
    json!({"id": id, "window": window, "index": 0, "title": title, "url": url,
           "active": user, "user": user, "pinned": false, "loading": false,
           "discarded": false, "audible": false, "muted": false, "container": null})
}

/// What the fake Firefox does with an operation besides its model.
#[derive(Debug, Clone)]
pub enum Twist {
    /// Answers `{error: {code, message}}`.
    Fail(&'static str, &'static str),
    /// Answers `{error: <value>}`: an error with its tab's fields (`tab`, `title`, `url`, …).
    FailWith(Value),
    /// Does the operation in the model, then answers `{error: {code, message}}`: a navigation
    /// that goes on after `nav_timeout`.
    FailAfter(&'static str, &'static str),
    /// Does the operation in the model and never answers: a navigation still loading.
    Hang,
    /// Closes the connection (Firefox quits) when the operation arrives.
    Quit,
    /// Answers `{ok: <value>}`.
    Answer(Value),
}

/// The fake Firefox's state.
#[derive(Debug, Default)]
pub struct Model {
    pub tabs: Vec<Value>,
    /// The operations that arrived, with their arguments.
    pub ops: Vec<(String, Value)>,
    pub twists: HashMap<&'static str, Twist>,
    /// `tabs.switch@1` gives the window the focus (KWin may keep it from taking it).
    pub focus_follows: bool,
    /// `context.get@1`'s answer, when a test sets it; else one built from the user's tab.
    pub context: Option<Value>,
    /// The elements `element.describe@1` knows, by ref: `{"tab": <id>, "role", "name"}` (and
    /// `frame`, `origin`, `level` where a test sets them; else the top document, its tab's
    /// origin), or `{"error": [<code>, <message>]}` for a stale one. Any other ref is unknown.
    /// The acting operations answer from them too: `guard` (the page side's verdicts at its first
    /// look, a list, until a call comes with each `confirmed`), `late` (the verdicts of its second
    /// look, before the click: the answer says `clicked: false` and the requests), `needs_trusted`
    /// and `key` for `element.press@1`, `focus` (where `focus.check@1` says the focus is), and
    /// `answer`, keys every acting answer of it carries.
    pub elements: HashMap<i64, Value>,
    /// The windows `tabs.list@1` reports (the contract's Window: `id`, `focused`, `focused_at`).
    pub windows: Vec<Value>,
    /// The origins `history.seen@1` has not seen; every other one it has.
    pub unvisited: HashSet<String>,
    /// The origins `history.seen@1` has seen only through Kai's own navigations.
    pub kai_only: HashSet<String>,
    /// `history.seen@1` answers without `seen_by_user` (an extension that does not say it).
    pub no_seen_by_user: bool,
    /// The input spans open now, and how many were opened.
    pub spans: HashSet<u64>,
    pub spans_opened: u64,
    /// `input.begin@1` fails with this code.
    pub begin_fails: Option<&'static str>,
    /// `gate.result@1` says the gate expired before it was read.
    pub gate_expired: bool,
    /// `gate.result@1`'s `key_seen`, where a test sets it; else the key the desk pressed, on the
    /// checked element.
    pub key_seen: Option<Value>,
    /// `page.key@1`'s page side: its verdicts at the first look and at the second.
    pub key_guard: Vec<Value>,
    pub key_late: Vec<Value>,
    /// The network gate's requests: the next gated operation (an acting one with a guard, or
    /// `gate.result@1`) asks about each (`{"method", "url", "type", "body"}`) and answers with
    /// what Kai said.
    pub asks: Vec<Value>,
    /// The gate decides `timeout` without waiting for Kai's answer.
    pub gate_times_out: bool,
    /// Kai's answers to the questions, in order.
    pub answers: Vec<bool>,
    /// The operation whose gate `focus.check@1` opened, and the element it checked.
    gate_op: Option<Value>,
    gate_ref: Option<i64>,
    /// The key the desk pressed last.
    pub last_key: Option<String>,
    /// The questions asked so far: each gets its own id.
    questions: u64,
    /// Reading aloud: the page's paragraphs (`read.paragraph@1`'s answers without `session`,
    /// `index` and `tab`), where `read.open@1` starts and how it found it (`None`: nothing to
    /// read), and the session's end (`session_ended`'s `why`) once a test ends it.
    pub paragraphs: Vec<Value>,
    pub read_start: Option<(u64, &'static str)>,
    pub read_ended: Option<&'static str>,
    sessions: i64,
}

impl Model {
    pub fn with(tabs: Vec<Value>) -> Self {
        Self {
            tabs,
            focus_follows: true,
            ..Default::default()
        }
    }

    fn user_tab(&self) -> Option<i64> {
        self.tabs
            .iter()
            .find(|t| t["user"] == true)
            .and_then(|t| t["id"].as_i64())
    }

    /// Trusted input reaches tab `id` (a key through the desktop, Kai's or the user's): inside an
    /// input span it is Kai's (`kai_input_at`), outside the user's act (`user_at`,
    /// `user_origin`).
    pub fn trusted_input(&mut self, id: i64) {
        let open = !self.spans.is_empty();
        let now = crate::browser::moves::now_ms();
        if let Ok(t) = self.find_mut(id) {
            if open {
                t["kai_input_at"] = json!(now);
            } else {
                let origin = t["url"]
                    .as_str()
                    .and_then(|u| url::Url::parse(u).ok())
                    .map(|u| u.origin().ascii_serialization());
                t["user_at"] = json!(now);
                t["user_origin"] = json!(origin);
            }
        }
    }

    /// The user goes to tab `id` themselves.
    pub fn user_goes_to(&mut self, id: i64) {
        let window = self.find(id).map(|t| t["window"].clone());
        for t in &mut self.tabs {
            t["user"] = json!(t["id"] == id);
            if t["window"] == window.clone().unwrap_or_default() {
                t["active"] = json!(t["id"] == id);
            }
        }
    }

    fn find(&self, id: i64) -> Option<&Value> {
        self.tabs.iter().find(|t| t["id"] == id)
    }

    fn find_mut(&mut self, id: i64) -> Result<&mut Value, (String, String)> {
        self.tabs
            .iter_mut()
            .find(|t| t["id"] == id)
            .ok_or_else(|| ("no_tab".to_owned(), format!("no tab {id}")))
    }

    /// The tab an operation names, else the user's.
    fn named(&self, args: &Value) -> Result<i64, (String, String)> {
        args["tab"]
            .as_i64()
            .or(self.user_tab())
            .ok_or_else(|| ("private_window".to_owned(), "private".to_owned()))
    }

    fn goes_to(args: &Value) -> String {
        match (args["url"].as_str(), args["search"].as_str()) {
            (Some(u), _) => u.to_owned(),
            (None, Some(s)) => format!("https://search.example/?q={}", s.replace(' ', "+")),
            _ => "about:blank".into(),
        }
    }

    fn answer(&mut self, op: &str, args: &Value) -> Result<Value, (String, String)> {
        let nav = |t: &Value| json!({"tab": t, "loaded": true});
        match op {
            "tabs.list@1" => {
                Ok(json!({"tabs": self.tabs, "user_tab": self.user_tab(), "windows": self.windows}))
            }
            "tabs.open@1" => {
                let id = self
                    .tabs
                    .iter()
                    .filter_map(|t| t["id"].as_i64())
                    .max()
                    .unwrap_or(0)
                    + 1;
                for t in &mut self.tabs {
                    if t["window"] == 1 {
                        t["active"] = json!(false);
                    }
                    t["user"] = json!(false);
                }
                let url = Self::goes_to(args);
                self.tabs.push(tab(id, 1, "Opened", &url, true));
                Ok(nav(self.tabs.last().unwrap()))
            }
            "tabs.go@1" => {
                let id = self.named(args)?;
                let url = Self::goes_to(args);
                let t = self.find_mut(id)?;
                t["url"] = json!(url);
                t["title"] = json!("Went");
                Ok(nav(t))
            }
            "tabs.switch@1" => {
                let id = self.named(args)?;
                self.find_mut(id)?;
                if self.focus_follows {
                    self.user_goes_to(id);
                }
                Ok(json!({"tab": self.find(id)}))
            }
            "tabs.close@1" => {
                let id = self.named(args)?;
                let mut t = self.find_mut(id)?.clone();
                self.tabs.retain(|t| t["id"] != id);
                // Firefox shows a neighbour in its place: the user's tab now, if this one was.
                let next = self
                    .tabs
                    .iter()
                    .find(|n| n["window"] == t["window"])
                    .and_then(|n| n["id"].as_i64());
                if t["user"] == true
                    && let Some(next) = next
                {
                    self.user_goes_to(next);
                }
                t["user"] = json!(false);
                Ok(json!({"closed": t}))
            }
            "tabs.back@1" | "tabs.forward@1" | "tabs.reload@1" => {
                let id = self.named(args)?;
                Ok(nav(self.find_mut(id)?))
            }
            "tabs.mute@1" => {
                let id = self.named(args)?;
                let on = args["on"].as_bool().unwrap_or_default();
                let t = self.find_mut(id)?;
                t["muted"] = json!(on);
                Ok(json!({"tab": t}))
            }
            "element.describe@1" => {
                let r = args["ref"].as_i64().unwrap_or_default();
                let Some(e) = self.elements.get(&r) else {
                    return Err((
                        "unknown_ref".into(),
                        format!("ref {r} was never given out in this Firefox run"),
                    ));
                };
                if let Some(err) = e["error"].as_array() {
                    let s = |i: usize| err[i].as_str().unwrap_or_default().to_owned();
                    return Err((s(0), s(1)));
                }
                let id = e["tab"].as_i64().unwrap_or_default();
                let t = self.find(id).cloned().unwrap_or_default();
                let origin = t["url"]
                    .as_str()
                    .and_then(|u| url::Url::parse(u).ok())
                    .map(|u| u.origin().ascii_serialization());
                let mut d = json!({"ref": r,
                                   "tab": {"id": id, "title": t["title"], "url": t["url"]},
                                   "frame": 0, "origin": origin,
                                   "role": e["role"], "name": e["name"]});
                for key in ["frame", "origin", "level"] {
                    if let Some(v) = e.get(key) {
                        d[key] = v.clone();
                    }
                }
                Ok(d)
            }
            "context.get@1" => {
                if let Some(c) = &self.context {
                    return Ok(c.clone());
                }
                let mut t = self
                    .user_tab()
                    .and_then(|id| self.find(id))
                    .cloned()
                    .ok_or_else(|| ("private_window".to_owned(), "private".to_owned()))?;
                t["reader"] = json!({"available": false, "on": false});
                t["zoom_percent"] = json!(100);
                Ok(
                    json!({"tab": t, "tabs_open": self.tabs.len(), "focus": null,
                          "selection": null, "pointer": null, "view": null, "dialog": null,
                          "frames": {"read": 1, "unread": 0}, "page_error": null}),
                )
            }
            "history.seen@1" => {
                let url = args["url"].as_str().unwrap_or_default();
                let origin = url::Url::parse(url)
                    .map(|u| u.origin().ascii_serialization())
                    .unwrap_or_default();
                if origin == "null" || origin.is_empty() {
                    return Err(("bad_url".into(), "no origin".into()));
                }
                let seen = !self.unvisited.contains(&origin);
                let at = if seen {
                    json!(1_790_006_400_000i64)
                } else {
                    Value::Null
                };
                let mut v = json!({"url": url, "origin": origin, "seen": seen,
                                   "first_visit": at, "last_visit": at});
                if !self.no_seen_by_user {
                    v["seen_by_user"] = json!(seen && !self.kai_only.contains(&origin));
                }
                Ok(v)
            }
            "events.config@1" => {
                let on = args["on"] == true;
                let tab = if on {
                    json!(self.user_tab())
                } else {
                    Value::Null
                };
                Ok(json!({"on": on, "quiet_ms": args["quiet_ms"], "tab": tab}))
            }
            "read.open@1" => {
                let id = match args["ref"].as_i64() {
                    Some(r) => self
                        .elements
                        .get(&r)
                        .and_then(|e| e["tab"].as_i64())
                        .unwrap_or_default(),
                    None => self.named(args)?,
                };
                let t = self.find(id).cloned().unwrap_or_default();
                self.sessions += 1;
                self.read_ended = None;
                let (start, from) = match self.read_start {
                    Some((s, f)) => (json!(s), json!(f)),
                    None => (Value::Null, Value::Null),
                };
                Ok(json!({"session": 1_767_225_660_000i64 + self.sessions,
                          "tab": {"id": id, "title": t["title"], "url": t["url"]},
                          "paragraphs": self.paragraphs.len(), "start": start, "from": from,
                          "frames": {"read": 1, "unread": 0}}))
            }
            "read.paragraph@1" | "read.mark@1" => {
                if let Some(why) = self.read_ended {
                    return Err(("session_ended".into(), format!("ended: {why}")));
                }
                let index = args["index"].as_u64().unwrap_or_default();
                let id = self.user_tab().unwrap_or_default();
                let t = self.find(id).cloned().unwrap_or_default();
                if op == "read.mark@1" {
                    return Ok(json!({"session": args["session"], "index": index,
                                     "marked": true, "scrolled": true}));
                }
                let mut p = self.paragraphs[index as usize].clone();
                p["session"] = args["session"].clone();
                p["index"] = json!(index);
                p["tab"] = json!({"id": id, "title": t["title"], "url": t["url"]});
                if p.get("frame").is_none() {
                    p["frame"] = json!(0);
                }
                Ok(p)
            }
            "read.close@1" => Ok(json!({"session": args["session"], "closed": true})),
            // A sheet that fetches nothing, unless a test says what it fetches.
            "page.css_check@1" => Ok(json!({"rules": 1, "dropped": [], "fetches": []})),
            "input.begin@1" => {
                if let Some(code) = self.begin_fails {
                    return Err((code.into(), "no span".into()));
                }
                self.spans_opened += 1;
                let span = self.spans_opened;
                self.spans.insert(span);
                Ok(json!({"span": span}))
            }
            "input.end@1" => {
                let span = args["span"].as_u64().unwrap_or_default();
                if !self.spans.remove(&span) {
                    return Err(("unknown_span".into(), format!("no span {span} is open")));
                }
                Ok(json!({"span": span, "tabs": []}))
            }
            "page.scroll@1" | "page.key@1" => {
                let id = self.named(args)?;
                let t = self.find(id).cloned().unwrap_or_default();
                let page = json!({"id": id, "title": t["title"], "url": t["url"]});
                if op == "page.scroll@1" {
                    return Ok(
                        json!({"tab": page, "container": null, "scrolled": true, "percent": 40,
                               "at_top": false, "at_bottom": false}),
                    );
                }
                let confirmed = args["confirmed"].as_array().cloned().unwrap_or_default();
                let open = |list: &[Value]| -> Vec<Value> {
                    list.iter()
                        .filter(|g| !confirmed.contains(g))
                        .cloned()
                        .collect()
                };
                let guarded = args["guard"].is_object();
                let (first, second) = (open(&self.key_guard), open(&self.key_late));
                Ok(if guarded && !first.is_empty() {
                    json!({"tab": page, "keys": args["keys"], "target": null, "guard": first})
                } else if guarded && !second.is_empty() {
                    json!({"tab": page, "keys": args["keys"], "target": null,
                           "prevented": false, "emulated": null, "guard": second})
                } else {
                    json!({"tab": page, "keys": args["keys"], "target": null,
                           "prevented": false, "emulated": null})
                })
            }
            "element.unmark@1" => Ok(json!({"value": args["value"], "removed": 1})),
            "gate.open@1" if args["tab"].is_number() => {
                let id = args["tab"].as_i64().unwrap_or_default();
                let t = self.find(id).cloned().unwrap_or_default();
                self.gate_ref = None;
                Ok(json!({"tab": {"id": id, "title": t["title"], "url": t["url"]}, "gate": 6}))
            }
            "gate.result@1" => {
                let mut v = json!({"gate": args["gate"], "requests": []});
                v["click_seen"] = match self.gate_ref {
                    Some(r) => {
                        let e = self.elements.get(&r).cloned().unwrap_or_default();
                        json!({"element": {"ref": r, "role": e["role"], "name": e["name"]}})
                    }
                    None => Value::Null,
                };
                v["key_seen"] = match &self.key_seen {
                    Some(k) => k.clone(),
                    None => {
                        let r = self.gate_ref.unwrap_or_default();
                        let e = self.elements.get(&r).cloned().unwrap_or_default();
                        json!({"key": self.last_key.clone().unwrap_or_else(|| "Enter".into()),
                               "element": {"ref": r, "role": e["role"], "name": e["name"]}})
                    }
                };
                if self.gate_expired {
                    v["expired"] = json!(true);
                    v["expired_at"] = json!(1_767_225_660_020i64);
                }
                Ok(v)
            }
            op if op.starts_with("element.") || op == "focus.check@1" || op == "gate.open@1" => {
                self.act(op, args)
            }
            other => Err(("unknown_op".into(), format!("no {other}"))),
        }
    }

    /// An acting operation on a known element (the model's `elements`).
    fn act(&mut self, op: &str, args: &Value) -> Result<Value, (String, String)> {
        let r = args["ref"].as_i64().unwrap_or_default();
        let Some(e) = self.elements.get(&r).cloned() else {
            return Err((
                "unknown_ref".into(),
                format!("ref {r} was never given out in this Firefox run"),
            ));
        };
        if let Some(err) = e["error"].as_array() {
            let s = |i: usize| err[i].as_str().unwrap_or_default().to_owned();
            return Err((s(0), s(1)));
        }
        let id = e["tab"].as_i64().unwrap_or_default();
        let t = self.find(id).cloned().unwrap_or_default();
        let mut v = json!({"ref": r, "tab": {"id": id, "title": t["title"], "url": t["url"]},
                           "role": e["role"], "name": e["name"]});
        let guarded = args["guard"].is_object();
        if args["confirmed"].is_boolean() {
            return Err((
                "bad_args".into(),
                "confirmed must be a list of verdicts".into(),
            ));
        }
        let confirmed = args["confirmed"].as_array().cloned().unwrap_or_default();
        let open = |verdicts: &Value| -> Vec<Value> {
            verdicts
                .as_array()
                .into_iter()
                .flatten()
                .filter(|g| !confirmed.contains(g))
                .cloned()
                .collect()
        };
        let first_look = open(&e["guard"]);
        let second_look = open(&e["late"]);
        match op {
            "element.press@1" if guarded && !first_look.is_empty() => {
                v["guard"] = json!(first_look);
            }
            "element.press@1" if guarded && !second_look.is_empty() => {
                v["guard"] = json!(second_look);
                v["clicked"] = json!(false);
                v["focused"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.press@1" if args["activate"] == false => {
                v["focused"] = json!(true);
                v["scrolled"] = json!(false);
                v["key"] = e.get("key").cloned().unwrap_or_else(|| {
                    json!(match e["role"].as_str() {
                        Some("checkbox" | "radio" | "switch") => "Space",
                        _ => "Enter",
                    })
                });
                if !e["needs_trusted"].is_null() {
                    v["needs_trusted"] = e["needs_trusted"].clone();
                }
            }
            "element.press@1" if !e["needs_trusted"].is_null() => {
                v["needs_trusted"] = e["needs_trusted"].clone();
                v["key"] = e.get("key").cloned().unwrap_or(json!("Enter"));
                v["focused"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.press@1" => {
                v["clicked"] = json!(true);
                v["focused"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.mark@1" => {
                v["attribute"] = json!("aria-kai-mark");
                v["object_attribute"] = json!("kai-mark");
                v["value"] = json!(format!("m{r}"));
                v["url"] = t["url"].clone();
                v["frames"] = e.get("frames").cloned().unwrap_or(json!([t["url"]]));
            }
            "gate.open@1" => {
                v["gate"] = json!(5);
                self.gate_ref = Some(r);
                for k in ["role", "name"] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            "focus.check@1" => {
                match e["focus"].as_str() {
                    Some(f) => {
                        v["focused"] = json!(false);
                        v["focus"] = json!(f);
                    }
                    None => {
                        v["focused"] = json!(true);
                        if guarded {
                            v["gate"] = json!(4);
                            self.gate_ref = Some(r);
                        }
                    }
                }
                for k in ["role", "name"] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            "element.fill@1" => {
                v["value"] = args["text"].clone();
                v["how"] = json!("value");
                v["took"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.choose@1" => {
                v["chosen"] = args["option"].clone();
                v["value"] = args["option"].clone();
                v["took"] = json!(true);
            }
            "element.check@1" => {
                v["checked"] = args["on"].clone();
                v["changed"] = json!(true);
                v["took"] = json!(true);
            }
            "element.file@1" => {
                let files: Vec<Value> = args["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        let n = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            f["base64"].as_str().unwrap_or_default(),
                        )
                        .map(|b| b.len())
                        .unwrap_or_default();
                        json!({"name": f["name"], "type": f["type"], "bytes": n})
                    })
                    .collect();
                v["files"] = json!(files);
            }
            "element.focus@1" => {
                v["focused"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.hover@1" => {
                v["hovered"] = json!(true);
                v["scrolled"] = json!(false);
            }
            "element.drag@1" => {
                let to = args["to"].as_i64().unwrap_or_default();
                let te = self.elements.get(&to).cloned().unwrap_or_default();
                v["how"] = json!("html5");
                v["dropped"] = json!(true);
                v["to"] = json!({"ref": to, "role": te["role"], "name": te["name"]});
            }
            "element.show@1" => {
                v["scrolled"] = json!(true);
            }
            "element.scroll@1" => {
                v["scrolled"] = json!(true);
                v["percent"] = json!(18);
                v["at_top"] = json!(false);
                v["at_bottom"] = json!(false);
            }
            _ => {}
        }
        if let Some(extra) = e["answer"].as_object()
            && !matches!(op, "focus.check@1" | "element.mark@1" | "gate.open@1")
        {
            for (k, x) in extra {
                v[k] = x.clone();
            }
        }
        Ok(v)
    }
}

/// A linked fake Firefox: its model, shared with the task that answers.
pub struct Firefox {
    pub model: Arc<Mutex<Model>>,
    task: JoinHandle<()>,
    push: tokio::sync::mpsc::UnboundedSender<Value>,
    /// The last event frame's `seq`.
    seq: Mutex<u64>,
}

impl Firefox {
    pub fn model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.model.lock().unwrap()
    }

    /// Sends Kai an `event` frame with `fields` (the contract's Events), numbered as link.js
    /// numbers a link's event frames (`seq` from 1) unless `fields` names its own.
    pub fn event(&self, mut fields: Value) {
        fields["t"] = json!("event");
        let mut seq = self.seq.lock().unwrap();
        match fields.get("seq").and_then(Value::as_u64) {
            Some(s) => *seq = s,
            None => {
                *seq += 1;
                fields["seq"] = json!(*seq);
            }
        }
        self.push.send(fields).unwrap();
    }

    /// Waits until `op` arrived `n` times; its arguments each time.
    pub async fn arrived(&self, op: &str, n: usize) -> Vec<Value> {
        let end = std::time::Instant::now() + super::WAIT;
        loop {
            let got: Vec<Value> = self
                .model()
                .ops
                .iter()
                .filter(|(o, _)| o == op)
                .map(|(_, a)| a.clone())
                .collect();
            if got.len() >= n {
                return got;
            }
            assert!(
                std::time::Instant::now() < end,
                "{op} came {} times, not {n}",
                got.len()
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// The operations that arrived, without the looks: `tabs.list@1`, and `history.seen@1`, the
    /// commit guard's before a navigation.
    pub fn acted(&self) -> Vec<(String, Value)> {
        self.model()
            .ops
            .iter()
            .filter(|(op, _)| op != "tabs.list@1" && op != "history.seen@1")
            .cloned()
            .collect()
    }

    /// How many times `op` arrived.
    pub fn count(&self, op: &str) -> usize {
        self.model().ops.iter().filter(|(o, _)| o == op).count()
    }

    /// Firefox quits.
    pub fn quit(self) {
        self.task.abort();
    }
}

/// The steps of the trusted path and the operations, in the order they happened.
pub type Steps = Arc<Mutex<Vec<String>>>;

/// Asks Kai about each of the model's requests for the operation `op_id` (the network gate's
/// questions), and the decisions, in order: Kai's answers, or `timeout` where the model's gate
/// does not wait.
async fn questions(
    c: &mut Conn,
    model: &Arc<Mutex<Model>>,
    asks: Vec<Value>,
    op_id: Value,
) -> Option<Vec<Value>> {
    let mut decided = Vec::new();
    for a in asks {
        let q = {
            let mut m = model.lock().unwrap();
            m.questions += 1;
            100 + m.questions
        };
        let mut frame = a.clone();
        frame["t"] = json!("ask");
        frame["id"] = json!(q);
        frame["op"] = op_id.clone();
        frame["gate"] = json!(4);
        c.send(frame).await;
        let decision = if model.lock().unwrap().gate_times_out {
            "timeout"
        } else {
            loop {
                let body = c.read().await?;
                let f: Value = serde_json::from_slice(&body).unwrap();
                if f["t"] == "answer" && f["id"] == q {
                    let allow = f["allow"] == true;
                    model.lock().unwrap().answers.push(allow);
                    break if allow { "allowed" } else { "declined" };
                }
                if f["t"] == "cancel" {
                    break "cancelled";
                }
            }
        };
        decided.push(
            json!({"method": a["method"], "url": a["url"], "type": a["type"],
                   "reason": a.get("reason").cloned().unwrap_or(json!("write")),
                   "decision": decision}),
        );
    }
    Some(decided)
}

/// The fake Firefox's end of the link: what it writes, and Kai's frames as a reader task reads
/// them (so the events a test sends can go between them).
struct Conn {
    wr: tokio::net::unix::OwnedWriteHalf,
    bodies: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    reader: JoinHandle<()>,
}

impl Conn {
    async fn send(&mut self, v: Value) {
        frame::write(&mut self.wr, v.to_string().as_bytes())
            .await
            .unwrap();
    }

    async fn read(&mut self) -> Option<Vec<u8>> {
        self.bodies.recv().await
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Answers `c`'s operations from `model` until the connection or the test ends, and sends the
/// event frames `push` brings.
fn serve(
    c: Client,
    model: Arc<Mutex<Model>>,
    steps: Steps,
    mut push: tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> JoinHandle<()> {
    let Client { mut rd, wr } = c;
    let (tx, bodies) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Ok(Some(b)) = frame::read(&mut rd).await {
            if tx.send(b).is_err() {
                return;
            }
        }
    });
    let mut c = Conn { wr, bodies, reader };
    tokio::spawn(async move {
        loop {
            let body = tokio::select! {
                b = c.read() => match b {
                    Some(b) => b,
                    None => return,
                },
                Some(v) = push.recv() => {
                    c.send(v).await;
                    continue;
                }
            };
            let f: Value = serde_json::from_slice(&body).unwrap();
            if f["t"] != "op" {
                continue;
            }
            let op = f["op"].as_str().unwrap_or_default().to_owned();
            if op != "tabs.list@1" {
                steps.lock().unwrap().push(op.clone());
            }
            let answer = {
                let mut m = model.lock().unwrap();
                m.ops.push((op.clone(), f["args"].clone()));
                if op == "gate.result@1" {
                    // The key the desk pressed last is the one the gate saw.
                    m.last_key = steps
                        .lock()
                        .unwrap()
                        .iter()
                        .rev()
                        .find_map(|s| s.strip_prefix("key ").map(str::to_owned));
                }
                match m.twists.get(op.as_str()).cloned() {
                    Some(Twist::Quit) => return,
                    Some(Twist::Fail(code, message)) => {
                        Err(json!({"code": code, "message": message}))
                    }
                    Some(Twist::FailWith(error)) => Err(error),
                    Some(Twist::FailAfter(code, message)) => {
                        let _ = m.answer(&op, &f["args"]);
                        Err(json!({"code": code, "message": message}))
                    }
                    Some(Twist::Hang) => {
                        let _ = m.answer(&op, &f["args"]);
                        continue;
                    }
                    Some(Twist::Answer(v)) => Ok(v),
                    None => {
                        let why = m.read_ended;
                        m.answer(&op, &f["args"]).map_err(|(code, message)| {
                            let mut e = json!({"code": code, "message": message});
                            if code == "session_ended" {
                                e["why"] = json!(why);
                                e["tab"] = json!(m.user_tab());
                            }
                            e
                        })
                    }
                }
            };
            let answer = match answer {
                Ok(mut ok) => {
                    // The gate watches an operation that acted (the contract's `requests`).
                    let gated = (f["args"]["guard"].is_object()
                        && op != "focus.check@1"
                        && op != "gate.open@1"
                        && ok["needs_trusted"].is_null()
                        && f["args"]["activate"] != false)
                        || op == "gate.result@1";
                    let (asks, op_id) = {
                        let mut m = model.lock().unwrap();
                        if (op == "focus.check@1" || op == "gate.open@1") && ok["gate"].is_number()
                        {
                            m.gate_op = Some(f["id"].clone());
                        }
                        let id = match op.as_str() {
                            "gate.result@1" => m.gate_op.clone().unwrap_or_default(),
                            _ => f["id"].clone(),
                        };
                        // A first look's guard: nothing reached the page; a second look's
                        // (`clicked: false`, `emulated`) after the events went.
                        let acted = ok["guard"].is_null()
                            || ok["clicked"] == false
                            || ok.get("emulated").is_some();
                        let asks = if gated && acted {
                            std::mem::take(&mut m.asks)
                        } else {
                            Vec::new()
                        };
                        (asks, id)
                    };
                    let acted = ok["guard"].is_null()
                        || ok["clicked"] == false
                        || ok.get("emulated").is_some();
                    if gated && acted {
                        let Some(decided) = questions(&mut c, &model, asks, op_id).await else {
                            return;
                        };
                        ok["requests"] = json!(decided);
                    }
                    Ok(ok)
                }
                Err(e) => Err(e),
            };
            let frame = match answer {
                Ok(ok) => json!({"t": "op_result", "id": f["id"], "ok": ok}),
                Err(error) => json!({"t": "op_result", "id": f["id"], "error": error}),
            };
            c.send(frame).await;
        }
    })
}

/// What the confirmer was asked.
#[derive(Debug, Clone)]
pub struct Asked {
    pub tool: String,
    pub description: String,
    pub lines: Vec<String>,
    pub notes: Vec<String>,
    /// Whether "always allow" was offered.
    pub always: bool,
    /// The question in Kai's English words, where the asker gave words to speak for a call of
    /// Kai's own (`Confirmer::ask_spoken`).
    pub spoken: Option<String>,
}

/// What happens while the next dialog waits (a test's change to Firefox's tabs).
type Meanwhile = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// The owner's next answers: `Some(yes)`, or `None` for nobody at the desk (the dialog stays open
/// until Kai closes it). Yes once they run out.
type Answers = Arc<Mutex<VecDeque<Option<bool>>>>;

struct Yes {
    asked: Arc<Mutex<Vec<Asked>>>,
    meanwhile: Meanwhile,
    answers: Answers,
    /// The dialogs nobody answers, open until Kai drops its end.
    open: Mutex<Vec<tokio::sync::oneshot::Sender<bool>>>,
}

impl Confirmer for Yes {
    fn ask(&self, req: ConfirmRequest) {
        self.asked.lock().unwrap().push(Asked {
            tool: req.tool.clone(),
            description: req.description.clone(),
            lines: req.lines.clone(),
            notes: req.notes.clone(),
            always: !req.options.is_empty(),
            spoken: None,
        });
        if let Some(then) = self.meanwhile.lock().unwrap().take() {
            then();
        }
        match self.answers.lock().unwrap().pop_front() {
            Some(None) => self.open.lock().unwrap().push(req.reply),
            Some(Some(yes)) => {
                let _ = req.reply.send(yes);
            }
            None => {
                let _ = req.reply.send(true);
            }
        }
    }

    fn ask_spoken(&self, req: ConfirmRequest, spoken: crate::voice::speak::Words) {
        let at = self.asked.lock().unwrap().len();
        self.ask(req);
        if let Some(a) = self.asked.lock().unwrap().get_mut(at) {
            a.spoken = Some(spoken.en);
        }
    }
}

/// The trusted path's desk: records its steps, and what the test says of the overlay, the screen
/// lock and the key.
pub struct FakeDesk {
    pub steps: Steps,
    /// Kai's approval card holds the keyboard.
    pub overlay: Arc<std::sync::atomic::AtomicBool>,
    /// The screen is locked.
    pub locked: std::sync::atomic::AtomicBool,
    /// The key fails with this.
    pub press_fails: Mutex<Option<String>>,
    /// Cancelled as Firefox's window comes to the front (lmgw cancelling the call meanwhile).
    pub on_activate: Mutex<Option<CancellationToken>>,
    /// What the key does in Firefox (a test's model: `Model::trusted_input`).
    pub on_key: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// The desktop's accessibility, where a test gives one (kai-firefox WP12).
    pub access: Mutex<Option<Arc<dyn kai_desktop::caps::accessibility::Accessibility>>>,
}

impl crate::browser::tools::trusted::Desk for FakeDesk {
    fn overlay_holds(&self) -> bool {
        self.overlay.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn locked(&self) -> bool {
        self.locked.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn turn<'a>(
        &'a self,
        _call: &'a crate::host::registry::Call,
    ) -> crate::browser::tools::trusted::Fut<'a, Result<crate::browser::tools::trusted::Turn, String>>
    {
        Box::pin(async move {
            self.steps.lock().unwrap().push("turn".into());
            Ok(Box::new(()) as crate::browser::tools::trusted::Turn)
        })
    }

    fn activate<'a>(
        &'a self,
        firefox: Option<u32>,
        title: &'a str,
    ) -> crate::browser::tools::trusted::Fut<'a, Result<kai_desktop::caps::windows::Window, String>>
    {
        Box::pin(async move {
            self.steps.lock().unwrap().push(format!("activate {title}"));
            if let Some(c) = self.on_activate.lock().unwrap().take() {
                c.cancel();
            }
            Ok(kai_desktop::caps::windows::Window {
                id: "7".into(),
                app: "org.mozilla.firefox".into(),
                title: format!("{title} — Mozilla Firefox"),
                pid: firefox.unwrap_or_default(),
                active: true,
                ..Default::default()
            })
        })
    }

    fn access(&self) -> Option<Arc<dyn kai_desktop::caps::accessibility::Accessibility>> {
        self.access.lock().unwrap().clone()
    }

    fn press<'a>(
        &'a self,
        _window: &'a kai_desktop::caps::windows::Window,
        key: crate::browser::tools::trusted::Key,
    ) -> crate::browser::tools::trusted::Fut<'a, Result<(), String>> {
        Box::pin(async move {
            self.steps
                .lock()
                .unwrap()
                .push(format!("key {}", key.name()));
            if let Some(hook) = &*self.on_key.lock().unwrap() {
                hook();
            }
            match self.press_fails.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })
    }
}

/// Kai's speech as the tests see it: every utterance the browser module hands over, which the
/// test answers as the speech would (`crate::voice::speak`).
#[derive(Clone, Default)]
pub struct FakeMouth(Arc<Mutex<VecDeque<crate::voice::speak::Utterance>>>);

impl crate::voice::speak::Mouth for FakeMouth {
    fn say(&self, u: crate::voice::speak::Utterance) {
        self.0.lock().unwrap().push_back(u);
    }
}

impl FakeMouth {
    /// The next utterance handed over.
    pub async fn next(&self) -> crate::voice::speak::Utterance {
        let end = std::time::Instant::now() + super::WAIT;
        loop {
            if let Some(u) = self.0.lock().unwrap().pop_front() {
                return u;
            }
            assert!(std::time::Instant::now() < end, "nothing was said");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Nothing more was handed over within `ms`.
    pub async fn silent(&self, ms: u64) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        let left: Vec<String> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|u| u.text.en().to_owned())
            .collect();
        assert!(left.is_empty(), "said: {left:?}");
    }
}

/// Answers `u` as the speech would.
pub fn answer(u: &crate::voice::speak::Utterance, said: crate::voice::speak::Said) {
    if let Some(r) = &u.reply {
        let _ = r.send((u.tag, said));
    }
}

/// `browser.sock` with Kai's host on the browser provider.
pub struct Host {
    /// What the browser module says (announcements, reading aloud).
    pub mouth: FakeMouth,
    pub rig: Rig,
    pub host: HostLink,
    pub local: Local,
    asked: Arc<Mutex<Vec<Asked>>>,
    meanwhile: Meanwhile,
    answers: Answers,
    /// The trusted path's steps and the operations (but the listings), in order.
    pub steps: Steps,
    pub desk: Arc<FakeDesk>,
    /// Kai for Firefox's input spans, as the desktop's input tools share them.
    pub spans: crate::browser::spans::Spans,
    /// K26's job table, the host's and the browser provider's (`Jobs::continuing`).
    pub jobs: crate::jobs::Jobs,
}

/// A key of another device: an untrusted caller.
pub fn stranger() -> Caller {
    Caller::Named(Named {
        kind: "key".into(),
        name: "laptop".into(),
    })
}

impl Host {
    pub async fn start() -> Self {
        Self::with_routes(crate::browser::tools::route).await
    }

    pub async fn with_routes(routes: fn(&str) -> Route) -> Self {
        let rig = Rig::start(super::in_firefox).await;
        let policy: SharedPolicy = Arc::new(std::sync::RwLock::new(Policy::new(
            &PolicyConfig::default(),
        )));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let meanwhile: Meanwhile = Arc::default();
        let answers: Answers = Arc::default();
        let confirmer = Arc::new(Yes {
            asked: asked.clone(),
            meanwhile: meanwhile.clone(),
            answers: answers.clone(),
            open: Mutex::default(),
        });
        let steps: Steps = Arc::default();
        let desk = Arc::new(FakeDesk {
            steps: steps.clone(),
            overlay: Arc::default(),
            locked: Default::default(),
            press_fails: Mutex::default(),
            on_activate: Mutex::default(),
            on_key: Mutex::default(),
            access: Mutex::default(),
        });
        let spans = crate::browser::spans::Spans::new(
            rig.links.clone(),
            policy.clone(),
            crate::browser::moves::Moves::default(),
        );
        let mouth = FakeMouth::default();
        let jobs = crate::jobs::Jobs::default();
        let host = HostLink::new(
            crate::host::tests::settings(5),
            policy.clone(),
            vec![Box::new(
                BrowserTools::with_parts(
                    rig.links.clone(),
                    policy.clone(),
                    routes,
                    confirmer.clone(),
                    desk.clone(),
                    spans.clone(),
                    Arc::new(mouth.clone()),
                )
                .with_jobs(jobs.clone()),
            )],
            rig.changed.clone(),
            confirmer,
            tokio::sync::watch::channel(None).0,
            crate::screen_lock::ScreenLock::default(),
            jobs.clone(),
        );
        let local = host.local();
        Self {
            mouth,
            rig,
            host,
            local,
            asked,
            meanwhile,
            answers,
            steps,
            desk,
            spans,
            jobs,
        }
    }

    /// The owner's next answers (`Some(yes)`, `None`: nobody at the desk), then yes; the ones
    /// set before and not used are dropped.
    pub fn answers(&self, next: &[Option<bool>]) {
        *self.answers.lock().unwrap() = next.iter().copied().collect();
    }

    /// The steps since the last look.
    pub fn steps(&self) -> Vec<String> {
        std::mem::take(&mut *self.steps.lock().unwrap())
    }

    /// `then` runs while the next dialog waits, before the owner answers yes.
    pub fn while_asked(&self, then: impl FnOnce() + Send + 'static) {
        *self.meanwhile.lock().unwrap() = Some(Box::new(then));
    }

    /// A Firefox of `profile` serving `ops`, its tabs `model`'s.
    pub async fn firefox(&self, profile: &str, ops: &[&str], model: Model) -> Firefox {
        let n = self.rig.links.list().len();
        let mut c = Client::connect(&self.rig.socket).await;
        let mut h = hello(1, ops);
        h["profile"] = json!(profile);
        c.send(h).await;
        assert_eq!(c.recv().await.unwrap()["t"], "welcome");
        self.rig.links_are(n + 1).await;
        let model = Arc::new(Mutex::new(model));
        let (push, pushed) = tokio::sync::mpsc::unbounded_channel();
        Firefox {
            task: serve(c, model.clone(), self.steps.clone(), pushed),
            model,
            push,
            seq: Mutex::new(0),
        }
    }

    /// The browser tools listed now.
    pub fn names(&self) -> Vec<&'static str> {
        self.local
            .entries()
            .into_iter()
            .filter(|e| e.area == "browser")
            .map(|e| e.spec.name)
            .collect()
    }

    /// The owner's `[policy]` and `[browser]`; whether the list changed.
    pub fn set(&self, policy: PolicyConfig, browser: BrowserConfig) -> bool {
        self.host
            .set_policy(Policy::new(&policy).with_browser(&browser))
    }

    pub async fn call(&self, tool: &str, args: Value, caller: &Caller) -> (bool, String) {
        self.call_until(tool, args, caller, CancellationToken::new())
            .await
    }

    /// The owner's call, its reply whole (an image block's data too).
    pub async fn reply(&self, tool: &str, args: Value) -> Reply {
        let entry = self
            .local
            .entries()
            .into_iter()
            .find(|e| e.spec.name == tool)
            .unwrap_or_else(|| panic!("{tool} is not listed"));
        let raw: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        tokio::time::timeout(
            WAIT,
            self.local
                .call(&entry, Some(raw), &Caller::Owner, CancellationToken::new()),
        )
        .await
        .expect("the call ended")
    }

    /// A call that ends when `cancel` fires, as lmgw cancels one at its deadline.
    pub async fn call_until(
        &self,
        tool: &str,
        args: Value,
        caller: &Caller,
        cancel: CancellationToken,
    ) -> (bool, String) {
        let entry = self
            .local
            .entries()
            .into_iter()
            .find(|e| e.spec.name == tool)
            .unwrap_or_else(|| panic!("{tool} is not listed"));
        let raw: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        let r = tokio::time::timeout(WAIT, self.local.call(&entry, Some(raw), caller, cancel))
            .await
            .expect("the call ended");
        said(r)
    }

    pub async fn owner(&self, tool: &str, args: Value) -> (bool, String) {
        self.call(tool, args, &Caller::Owner).await
    }

    /// A call of an untrusted caller that the owner approved in lmgw.
    pub async fn approved(&self, tool: &str, args: Value) -> (bool, String) {
        self.approved_by(tool, args, &stranger(), &Caller::Owner)
            .await
    }

    /// `caller`'s call, which `approver` approved in lmgw.
    pub async fn approved_by(
        &self,
        tool: &str,
        args: Value,
        caller: &Caller,
        approver: &Caller,
    ) -> (bool, String) {
        let entry = self
            .local
            .entries()
            .into_iter()
            .find(|e| e.spec.name == tool)
            .unwrap();
        let raw = args.as_object().cloned();
        let r = self
            .local
            .call_approved_by(&entry, raw, caller, approver)
            .await;
        said(r)
    }

    /// What the confirmer was asked since the last look.
    pub fn asked(&self) -> Vec<Asked> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
}

/// A reply's error flag and text.
pub fn said(r: Reply) -> (bool, String) {
    let text = r
        .blocks
        .iter()
        .map(|b| match b {
            Block::Text(t) => t.clone(),
            Block::Image { .. } => "(image)".into(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    (r.error, text)
}

/// Firefox's tabs for most tests: two windows, the user in window 1 on the checkout.
pub fn shop() -> Model {
    let mut lamps = tab(14, 2, "Lamps", "https://shop.example/lamps", false);
    lamps["active"] = json!(true);
    Model::with(vec![
        tab(3, 1, "Mail – Inbox", "https://mail.example/inbox", false),
        tab(12, 1, "Checkout", "https://shop.example/cart", true),
        lamps,
    ])
}
