//! Kai for Firefox's events and Kai's announcements of them (kai-firefox design §7.1, §7.2; the
//! contract's Events and `events.config@1`).
//!
//! - **On and off.** Events are on while `browser.announce` is (`BrowserPolicy::announcing`;
//!   the hands-and-eyes switch does not turn them on). A link starts with them off: after each `welcome` Kai sends
//!   `events.config@1` (`on` with `browser.announce_quiet_ms`, or off), and again to every link
//!   when the policy changes it. The policy reaches this module where the host lists its tools
//!   again (`super::provider`'s `entries`, which every write of the policy runs). The calls to one
//!   link go in order. Switching them off drops every announcement still waiting.
//! - **One frame per event, in `seq` order.** The extension numbers a link's event frames; an
//!   event whose `seq` is not above the last one of its link is the extension's broken contract,
//!   logged and dropped. `read_ended` is reading aloud's (`super::tools::aloud`), not an
//!   announcement.
//! - **Site rules** (§6.4): an event whose tab's URL or whose document's origin is a `none` site
//!   is dropped (an opaque origin gets the strictest rule configured), so is a `tab_opened` whose
//!   `opened_url` is, and a `download_done` whose `url`, `final_url`, `referrer`, `source.url` or
//!   `source.origin` is; one with neither `source` nor `referrer` names no page and gets the
//!   strictest rule ([`words::Event::sites`]). Nothing of such an event is logged, its URLs least.
//! - **What is said** ([`words`]): one sentence per event, Kai's words first and the page's text
//!   last, in English and German. A `navigated` is not spoken (its `loaded` follows); it is kept
//!   for `browser_context`.
//! - **Kai's own actions** (`by_kai`): a page that loaded, a focus that moved and a tab opened by
//!   Kai's own acting tool are not spoken (the tool's answer says them); a dialog, an error, a
//!   region's change or a download after Kai's action are spoken after "After Kai's action:".
//!   `browser_context` marks them.
//! - **Newest only where the contract says so:** a `focus` replaces the one of its link that waits
//!   to be spoken, an atomic live region's change the one of that region (its text is the whole
//!   region's). A non-atomic region's changes (a log, a plain `aria-live`) each say what was added,
//!   so none replaces another: each waits its turn.
//! - **A field's error and its live region are spoken once** (the contract's `field_error`): a
//!   `live` event whose region is in the `regions` of a `field_error` of its link, whose text is
//!   part of that error's `error` or `description`, within `browser.announce_quiet_ms` of it by
//!   `at`, is not spoken, in either order: it is not handed over when the error came first, and is
//!   taken back while it waits when the error comes second. An older error suppresses nothing.
//! - **Spoken by Kai itself** (`crate::voice::speak`), with a gate the speech runs again when the
//!   announcement is made and when it plays: switched off, its site `none` now, or older than
//!   `browser.announce_max_age_s` drops it; Firefox's focus is the newest event's of its link.
//! - **For `browser_context`** ([`Announcer::since_last_look`]): every announcement since the
//!   previous call (from any caller: they are not per thread), with whether it was spoken and why
//!   not; one older than `browser.announce_keep_s` is dropped from that list and counted, and so
//!   are those that went unspoken for their age or the screen lock.

pub mod words;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::links::{Incoming, LinkId, Links};
use super::policy::BrowserPolicy;
use super::sites::{Rule, SiteRules};
use crate::host::confirm::shown_text;
use crate::host::policy::SharedPolicy;
use crate::voice::speak::{Dropped, Kind, Mouth, Replies, Said, Spoken, Text, Utterance, Words};
use words::{Event, Site};

/// The operation that turns the events on and off.
pub const CONFIG: &str = "events.config@1";

/// The reading aloud's event (`super::tools::aloud`).
pub const READ_ENDED: &str = "read_ended";

/// What `events.config@1` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub on: bool,
    pub quiet_ms: u64,
}

impl Wanted {
    pub fn of(p: &BrowserPolicy) -> Self {
        Self {
            on: p.announcing(),
            quiet_ms: p.announce_quiet_ms,
        }
    }

    fn args(self) -> Value {
        if self.on {
            json!({"on": true, "quiet_ms": self.quiet_ms})
        } else {
            json!({"on": false})
        }
    }
}

/// What became of an announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// Waiting for its turn, or being made; why it waits where the speech said.
    Waiting(Option<String>),
    Playing,
    Spoken,
    /// Not spoken, and why.
    Not(String),
    /// Kept for `browser_context` only (module doc: Kai's own navigation, a `navigated`).
    Kept,
}

struct Entry {
    id: u64,
    at: Instant,
    link: LinkId,
    tab: Option<i64>,
    /// The URLs and origins its site rules read, checked again when it is listed.
    sites: Vec<Site>,
    line: String,
    status: Status,
    /// Its speech's cancel, while it may still be taken back.
    cancel: Option<CancellationToken>,
    /// A `live` event's region, its text and its `at` (the error rule, module doc).
    live: Option<(i64, String, i64)>,
}

/// A `field_error` the error rule remembers (module doc).
struct FieldError {
    link: LinkId,
    regions: Vec<i64>,
    texts: Vec<String>,
    at: i64,
}

impl FieldError {
    /// `text` (white space collapsed) is part of its error or description.
    fn covers(&self, text: &str) -> bool {
        let t = collapse(text);
        !t.is_empty() && self.texts.iter().any(|e| e.contains(&t))
    }
}

fn collapse(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a waiting announcement replaces (module doc).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Focus(LinkId),
    Region(LinkId, i64),
}

#[derive(Default)]
struct St {
    entries: VecDeque<Entry>,
    next: u64,
    /// Since the last look: dropped from the list for their age, not spoken for their age when
    /// their turn came, dropped while the screen was locked.
    aged: u64,
    stale: u64,
    locked: u64,
    /// The announcement of each key that waits to be spoken.
    waiting: HashMap<Key, u64>,
    errors: Vec<FieldError>,
    /// Firefox's focus as each link's newest event said it.
    focused: HashMap<LinkId, bool>,
    /// Each link's last `seq`.
    seq: HashMap<LinkId, u64>,
}

impl St {
    fn entry(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Takes back announcement `id` while it waits: `why` is what the list says.
    fn take_back(&mut self, id: u64, why: &str) {
        self.waiting.retain(|_, w| *w != id);
        if let Some(e) = self.entry(id)
            && matches!(e.status, Status::Waiting(_))
        {
            e.status = Status::Not(why.to_owned());
            if let Some(c) = e.cancel.take() {
                c.cancel();
            }
        }
    }
}

/// The announcements (module doc). Shared by the provider's handlers; its task runs from the first
/// listing in a runtime.
pub struct Announcer {
    links: Links,
    policy: SharedPolicy,
    mouth: Arc<dyn Mouth>,
    wanted: watch::Sender<Wanted>,
    st: Mutex<St>,
    /// Taken by the task when it starts.
    start: Mutex<Option<(mpsc::UnboundedReceiver<Incoming>, watch::Receiver<Wanted>)>>,
    running: AtomicBool,
}

impl Announcer {
    /// Subscribes to the links at once, so a link that comes before the task runs is not missed.
    pub fn new(links: Links, policy: SharedPolicy, mouth: Arc<dyn Mouth>) -> Arc<Self> {
        let wanted = Wanted::of(policy.read().expect("never poisoned").browser());
        let (tx, rx) = watch::channel(wanted);
        let incoming = links.subscribe();
        Arc::new(Self {
            links,
            policy,
            mouth,
            wanted: tx,
            st: Mutex::default(),
            start: Mutex::new(Some((incoming, rx))),
            running: AtomicBool::new(false),
        })
    }

    fn st(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn browser(&self) -> BrowserPolicy {
        self.policy
            .read()
            .expect("never poisoned")
            .browser()
            .clone()
    }

    /// The policy as the host lists with it (module doc): a change of what the events should be
    /// goes to every link, and what waits is checked against it. Starts the task where a runtime
    /// runs.
    pub fn follow(self: &Arc<Self>, p: &BrowserPolicy) {
        self.wanted.send_if_modified(|w| {
            let now = Wanted::of(p);
            std::mem::replace(w, now) != now
        });
        let mut st = self.st();
        let gone: Vec<(u64, &str)> = st
            .entries
            .iter()
            .filter(|e| matches!(e.status, Status::Waiting(_)))
            .filter_map(|e| {
                if !p.announcing() {
                    Some((e.id, "announcements were switched off"))
                } else if e.sites.iter().any(|s| reached(s, &p.sites) == Rule::None) {
                    Some((e.id, "its site is one Kai does not reach now"))
                } else {
                    None
                }
            })
            .collect();
        for (id, why) in gone {
            st.take_back(id, why);
        }
        drop(st);
        self.ensure_running();
    }

    /// Starts the task, once, where a runtime runs (`kai --tool-list` has none).
    pub fn ensure_running(self: &Arc<Self>) {
        if self.running.load(Ordering::SeqCst) || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some((incoming, wanted)) = self.start.lock().expect("never poisoned").take() else {
            return;
        };
        let (said_tx, said_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(
            Arc::downgrade(self),
            incoming,
            wanted,
            said_tx,
            said_rx,
        ));
    }

    /// `browser_context`'s lines (module doc), which empty the list: none while announcements are
    /// off. `several`: more than one profile is linked, so each line names its profile.
    pub fn since_last_look(&self, several: bool) -> Vec<String> {
        let p = self.browser();
        if !p.announcing() {
            return Vec::new();
        }
        let keep = Duration::from_secs(p.announce_keep_s);
        let profiles: HashMap<LinkId, String> = self
            .links
            .list()
            .into_iter()
            .map(|l| (l.id, l.profile))
            .collect();
        let mut st = self.st();
        let now = Instant::now();
        let mut lines = Vec::new();
        let mut kept = VecDeque::new();
        for e in std::mem::take(&mut st.entries) {
            let waits = matches!(e.status, Status::Waiting(_) | Status::Playing);
            // A site that became `none` since: nothing of it.
            if e.sites.iter().any(|s| reached(s, &p.sites) == Rule::None) {
                continue;
            }
            if !waits && now.saturating_duration_since(e.at) > keep {
                st.aged += 1;
                continue;
            }
            let ago = now.saturating_duration_since(e.at).as_secs();
            let mut place = match e.tab {
                Some(t) => format!("{ago} s ago, tab {t}"),
                None => format!("{ago} s ago"),
            };
            if several && let Some(profile) = profiles.get(&e.link) {
                place.push_str(&format!(" (profile {})", shown_text(profile)));
            }
            let status = match &e.status {
                Status::Waiting(None) => " (waiting to be spoken)".to_owned(),
                Status::Waiting(Some(why)) => format!(" (waiting to be spoken: {why})"),
                Status::Playing => " (being spoken)".to_owned(),
                Status::Spoken | Status::Kept => String::new(),
                Status::Not(why) => format!(" (not spoken: {why})"),
            };
            lines.push(format!("{place}: {}{status}", e.line));
            // One not spoken yet is said again at the next look, with how it ended.
            if waits {
                kept.push_back(e);
            }
        }
        st.entries = kept;
        let (aged, stale, locked) = (
            std::mem::take(&mut st.aged),
            std::mem::take(&mut st.stale),
            std::mem::take(&mut st.locked),
        );
        drop(st);
        let mut out = vec![match lines.len() {
            0 => "announcements since the last look: none".to_owned(),
            n => format!("announcements since the last look ({n}):"),
        }];
        out.extend(lines.into_iter().map(|l| format!("  {l}")));
        let were = |n: u64| {
            if n == 1 {
                "announcement was"
            } else {
                "announcements were"
            }
        };
        if stale > 0 {
            out.push(format!(
                "{stale} {} not spoken: older than browser.announce_max_age_s = {} s when their \
                 turn came",
                were(stale),
                p.announce_max_age_s
            ));
        }
        if locked > 0 {
            out.push(format!(
                "{locked} {} dropped while the screen was locked",
                were(locked)
            ));
        }
        if aged > 0 {
            out.push(format!(
                "{aged} older {} dropped from this list (kept for browser.announce_keep_s = {} s)",
                were(aged),
                p.announce_keep_s
            ));
        }
        out
    }

    /// Entries older than `browser.announce_keep_s` go, counted, unless they still wait.
    fn prune(st: &mut St, keep_s: u64) {
        let keep = Duration::from_secs(keep_s);
        let now = Instant::now();
        let before = st.entries.len();
        st.entries.retain(|e| {
            matches!(e.status, Status::Waiting(_) | Status::Playing)
                || now.saturating_duration_since(e.at) <= keep
        });
        st.aged += (before - st.entries.len()) as u64;
    }

    /// One event of `link` (module doc).
    fn event(
        self: &Arc<Self>,
        link: LinkId,
        kind: &str,
        f: &Map<String, Value>,
        replies: &Replies,
    ) {
        let ev = Event::read(kind, f);
        {
            let mut st = self.st();
            if let Some(seq) = ev.seq {
                let last = st.seq.entry(link).or_default();
                if seq <= *last {
                    tracing::warn!(
                        link,
                        seq,
                        last = *last,
                        "browser: an event out of its link's seq order (the extension's broken \
                         contract); dropped"
                    );
                    return;
                }
                *last = seq;
            }
            if kind != READ_ENDED {
                st.focused.insert(link, ev.focused);
            }
        }
        if kind == READ_ENDED {
            return;
        }
        let p = self.browser();
        if !p.announcing() {
            tracing::debug!(link, kind, "browser: an event while announcements are off");
            return;
        }
        let sites = ev.sites(kind, f);
        if sites.iter().any(|s| reached(s, &p.sites) == Rule::None) {
            // Nothing of it in the log, its URLs least (module doc).
            tracing::debug!(link, kind, "browser: an event of a none site; dropped");
            return;
        }
        let Some(said) = ev.said(kind, f, p.announce_max_chars) else {
            tracing::debug!(link, kind, "browser: an event Kai does not know; dropped");
            return;
        };
        let quiet = i64::try_from(p.announce_quiet_ms).unwrap_or(i64::MAX);
        let mut st = self.st();
        Self::prune(&mut st, p.announce_keep_s);
        // The error rule's memory: only what can still meet a later event (module doc).
        if let Some(at) = ev.at {
            st.errors.retain(|e| at.saturating_sub(e.at) <= quiet);
        }
        st.next += 1;
        let id = st.next;
        let live = match (kind, ev.region, &ev.text, ev.at) {
            ("live", Some(r), Some(t), Some(at)) => Some((r, t.clone(), at)),
            _ => None,
        };
        // A region's change the field's error already says (the error came first).
        let repeats = live.as_ref().is_some_and(|(r, t, at)| {
            st.errors.iter().any(|e| {
                e.link == link && e.regions.contains(r) && (e.at - at).abs() <= quiet && e.covers(t)
            })
        });
        let speak = said
            .spoken
            .filter(|_| !(ev.by_kai && said.quiet_by_kai) && !repeats);
        let cancel = CancellationToken::new();
        st.entries.push_back(Entry {
            id,
            at: Instant::now(),
            link,
            tab: ev.tab,
            sites: sites.clone(),
            line: if ev.by_kai {
                format!("{} (after Kai's action)", said.line)
            } else {
                said.line
            },
            status: if repeats {
                Status::Not("the field's error says it".into())
            } else if speak.is_some() {
                Status::Waiting(None)
            } else {
                Status::Kept
            },
            cancel: speak.as_ref().map(|_| cancel.clone()),
            live,
        });
        if kind == "field_error"
            && let Some(at) = ev.at
            && !ev.regions.is_empty()
        {
            let e = FieldError {
                link,
                regions: ev.regions.clone(),
                texts: [&ev.error, &ev.description]
                    .into_iter()
                    .flatten()
                    .map(|t| collapse(t))
                    .collect(),
                at,
            };
            // The region's change came first and still waits: taken back.
            let back: Vec<u64> = st
                .entries
                .iter()
                .filter(|x| x.link == link && matches!(x.status, Status::Waiting(_)))
                .filter(|x| {
                    x.live.as_ref().is_some_and(|(r, t, lat)| {
                        e.regions.contains(r) && (at - lat).abs() <= quiet && e.covers(t)
                    })
                })
                .map(|x| x.id)
                .collect();
            for b in back {
                st.take_back(b, "the field's error says it");
            }
            st.errors.push(e);
        }
        let Some(words) = speak else {
            return;
        };
        let words = if ev.by_kai {
            Words::new(
                format!("After Kai's action: {}", lower_first(&words.en)),
                format!("Nach Kais Aktion: {}", words.de),
            )
        } else {
            words
        };
        if let Some(k) = Self::key_of(kind, link, &ev) {
            if let Some(old) = st.waiting.get(&k).copied() {
                tracing::debug!(
                    link,
                    kind,
                    old,
                    "browser: a newer announcement replaces one"
                );
                st.take_back(old, "a newer one replaced it");
            }
            st.waiting.insert(k, id);
        }
        drop(st);
        self.mouth.say(Utterance {
            kind: Kind::Announcement {
                urgent: ev.assertive,
            },
            text: Text::Kai(words),
            voice: None,
            gate: Some(self.gate(link, sites)),
            cancel,
            tag: id,
            reply: Some(replies.clone()),
        });
    }

    /// What a waiting announcement of this event replaces (module doc).
    fn key_of(kind: &str, link: LinkId, ev: &Event) -> Option<Key> {
        match kind {
            "focus" => Some(Key::Focus(link)),
            "live" if ev.atomic => ev.region.map(|r| Key::Region(link, r)),
            _ => None,
        }
    }

    /// The check the speech runs when the announcement is made and when it plays (module doc).
    fn gate(self: &Arc<Self>, link: LinkId, sites: Vec<Site>) -> crate::voice::speak::Gate {
        let me: Weak<Self> = Arc::downgrade(self);
        let born = Instant::now();
        Arc::new(move || {
            let a = me.upgrade().ok_or(Dropped::Off)?;
            let p = a.browser();
            if !p.announcing() {
                return Err(Dropped::Off);
            }
            if sites.iter().any(|s| reached(s, &p.sites) == Rule::None) {
                return Err(Dropped::Site);
            }
            if born.elapsed() > Duration::from_secs(p.announce_max_age_s) {
                return Err(Dropped::Stale(p.announce_max_age_s));
            }
            Ok(a.st().focused.get(&link).copied().unwrap_or(false))
        })
    }

    /// What the speech said of announcement `id`.
    fn said(&self, id: u64, s: Said) {
        let mut st = self.st();
        if !matches!(s, Said::Waiting(_)) {
            // Playing or done: it is no longer replaced by a newer one.
            st.waiting.retain(|_, w| *w != id);
        }
        let status = match s {
            // Only one that still waits (one taken back keeps why).
            Said::Waiting(why) => match st.entry(id).map(|e| &e.status) {
                Some(Status::Waiting(_)) => Status::Waiting(Some(why)),
                _ => return,
            },
            Said::Started => Status::Playing,
            Said::Done(Spoken::Heard) => Status::Spoken,
            Said::Done(Spoken::Cut(why)) => Status::Not(format!("cut, {why}")),
            Said::Done(Spoken::Dropped(Dropped::Cancelled)) => {
                // Whoever took it back said why already.
                match st.entry(id).map(|e| e.status.clone()) {
                    Some(Status::Not(_)) => return,
                    _ => Status::Not("a newer one replaced it".into()),
                }
            }
            Said::Done(Spoken::Dropped(d)) => {
                match d {
                    Dropped::Stale(_) => st.stale += 1,
                    Dropped::Locked => st.locked += 1,
                    _ => {}
                }
                Status::Not(d.to_string())
            }
            Said::Done(Spoken::Failed(f)) => Status::Not(format!("its speech failed: {f}")),
            Said::Done(Spoken::Notified) => Status::Not("shown as a notification".into()),
        };
        if let Some(e) = st.entry(id) {
            if !matches!(status, Status::Waiting(_)) {
                e.cancel = None;
            }
            e.status = status;
        }
    }
}

/// `s` with its first letter lowercase (Kai's prefix goes before it).
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_lowercase().chain(c).collect())
        .unwrap_or_default()
}

/// The site rule of a URL or an origin an event names.
fn reached(s: &Site, sites: &SiteRules) -> Rule {
    match s {
        Site::Url(u) => sites.rule(u),
        Site::Origin(o) => sites.frame_rule(o.as_deref()),
    }
}

/// The task (module doc).
async fn run(
    me: Weak<Announcer>,
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    mut wanted: watch::Receiver<Wanted>,
    said_tx: Replies,
    mut said_rx: mpsc::UnboundedReceiver<(u64, Said)>,
) {
    // The configuration calls of each link, so they go in order.
    let mut sending: HashMap<LinkId, JoinHandle<()>> = HashMap::new();
    loop {
        tokio::select! {
            item = incoming.recv() => {
                let Some(item) = item else { return };
                let Some(a) = me.upgrade() else { return };
                match item {
                    Incoming::Linked { link } => {
                        // A new link numbers its events from 1 again.
                        let mut st = a.st();
                        st.seq.remove(&link);
                        st.focused.remove(&link);
                        drop(st);
                        let w = *wanted.borrow();
                        configure(&a.links, link, w, &mut sending);
                    }
                    Incoming::Event { link, kind, fields } => a.event(link, &kind, &fields, &said_tx),
                    Incoming::Ask { .. } => {}
                }
            }
            changed = wanted.changed() => {
                if changed.is_err() {
                    return;
                }
                let Some(a) = me.upgrade() else { return };
                let w = *wanted.borrow_and_update();
                tracing::info!(on = w.on, quiet_ms = w.quiet_ms, "browser: announcements changed");
                for l in a.links.list() {
                    configure(&a.links, l.id, w, &mut sending);
                }
            }
            Some((id, s)) = said_rx.recv() => {
                let Some(a) = me.upgrade() else { return };
                a.said(id, s);
            }
        }
        sending.retain(|_, h| !h.is_finished());
    }
}

/// Sends `events.config@1` to `link`, after its earlier ones.
fn configure(
    links: &Links,
    link: LinkId,
    w: Wanted,
    sending: &mut HashMap<LinkId, JoinHandle<()>>,
) {
    if !links
        .list()
        .iter()
        .any(|l| l.id == link && l.ops.iter().any(|o| o == CONFIG))
    {
        tracing::debug!(
            link,
            "browser: the extension does not offer {CONFIG}; no events"
        );
        return;
    }
    let before = sending.remove(&link);
    let links = links.clone();
    let task = tokio::spawn(async move {
        if let Some(b) = before {
            let _ = b.await;
        }
        match links
            .op(link, CONFIG, w.args(), &CancellationToken::new())
            .await
        {
            Ok(v) => {
                let tab = v.get("tab").cloned().unwrap_or(Value::Null);
                tracing::info!(link, on = w.on, %tab, "browser: events configured");
            }
            Err(e) => tracing::warn!(link, on = w.on, "browser: {CONFIG} failed: {e}"),
        }
    });
    sending.insert(link, task);
}
