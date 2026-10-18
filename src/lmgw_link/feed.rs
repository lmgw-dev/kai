//! The feed task (§3.2, §3.3): one `GET /chat/api/feed` at a time over the device link's reqwest
//! client (`super::tls::Tls`), tried on the pairing's URLs by the device link's rule
//! (`super::tls::race`), its bytes through `lmgw-client`'s [`FeedReader`], its events mapped onto
//! [`LinkEvent`]s by [`Translate`].

use std::time::Duration;

use lmgw_client::feed::{
    ApprovalRequested, ApprovalsChange, FeedEvent, FeedItem, FeedReader, FolderNow, ThreadNow,
};
use lmgw_client::requests;
use lmgw_client::types::chat_feed::{LiveTurn, LiveVoice};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{
    Credentials, DeviceFacts, FolderEvent, KeyLoss, LinkEvent, LmgwLink, ProfileEvent, describe,
    tls,
};
use crate::config::Config;
use crate::conversation::FolderInfo;
use crate::voice::{By, FeedFact};

/// The first reconnect waits this long; each further one twice the last, up to
/// `lmgw.retry_max_s` (§3.2).
const FIRST_RETRY: Duration = Duration::from_secs(1);

/// What the feed needs from the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedSettings {
    /// `lmgw.retry_max_s`.
    pub retry_max: Duration,
    /// `lmgw.timeout_s`: the request's answer and the body's first bytes, and the keep-alive
    /// interval's stand-in until lmgw states one ([`Wait`]).
    pub timeout: Duration,
    /// `lmgw.url_stagger_ms`: with several URLs, how long one may go without a connection before
    /// the next is tried beside it (`super::tls::race`).
    pub stagger: Duration,
}

impl FeedSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            retry_max: c.lmgw.retry_max(),
            timeout: c.lmgw.timeout(),
            stagger: c.lmgw.url_stagger(),
        }
    }
}

/// Kai's lmgw link: the feed task, started and stopped by the core.
pub struct FeedLink {
    settings: FeedSettings,
    task: Option<JoinHandle<()>>,
}

impl FeedLink {
    pub fn new(settings: FeedSettings) -> Self {
        Self {
            settings,
            task: None,
        }
    }
}

impl LmgwLink for FeedLink {
    fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<LinkEvent>) {
        self.stop();
        tracing::info!(urls = ?creds.reach.urls(), "lmgw link: opening the feed");
        let run = Run {
            creds,
            settings: self.settings,
            events,
        };
        self.task = Some(tokio::spawn(run.run()));
    }

    fn stop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Drop for FeedLink {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How one connection ended.
enum End {
    /// §3.3: the server's words, and what they ask of the owner: stop retrying. `None`: a
    /// `revoked` of a kind this build does not know, which the core asks lmgw about once while the
    /// feed tries again after its backoff.
    Revoked(String, Option<KeyLoss>),
    /// Retry after the backoff. `was_up`: a `hello` came first, so the backoff starts over.
    /// `dead`: the dead-link verdict (silence), not an end lmgw or the network said.
    Dropped {
        why: String,
        was_up: bool,
        dead: bool,
    },
    /// lmgw's 403 `device_unconfirmed`: the owner has not confirmed the code yet; asked again after
    /// the backoff.
    Unconfirmed,
    /// The core no longer listens.
    Gone,
}

struct Run {
    creds: Credentials,
    settings: FeedSettings,
    events: mpsc::UnboundedSender<LinkEvent>,
}

impl Run {
    async fn run(self) {
        // Where the next connection resumes from; a new run (a new pairing) starts from now.
        let mut cursor: Option<String> = None;
        // The keep-alive interval lmgw stated in its last `hello` on this run ([`Wait`]).
        let mut stated: Option<u32> = None;
        let mut delay = FIRST_RETRY;
        loop {
            match self.connect(&mut cursor, &mut stated).await {
                End::Gone => return,
                End::Revoked(reason, Some(loss)) => {
                    tracing::warn!(%reason, ?loss, "lmgw link: the key was revoked; the feed stops (§3.3)");
                    let _ = self.events.send(LinkEvent::Revoked {
                        reason,
                        loss: Some(loss),
                    });
                    return;
                }
                // A `revoked` of a kind this build does not know (§3.3): the core says it and asks
                // lmgw once (a 401 decides, and stops this run); the feed tries again after its
                // backoff, which this end never starts over, though a `hello` came before it, so
                // a gateway that ends every feed so is met ever more slowly, up to
                // `lmgw.retry_max_s` (review KAI-4 K4-7).
                End::Revoked(reason, None) => {
                    tracing::warn!(%reason, "lmgw link: revoked of a kind this build does not know; the feed tries again after its backoff (§3.3)");
                    let note = format!(
                        "lmgw ended the feed: {reason}; next try in {} s (the wait doubles up to {} \
                         s, lmgw.retry_max_s)",
                        delay.as_secs(),
                        self.settings.retry_max.as_secs()
                    );
                    if !self.send(LinkEvent::Revoked { reason, loss: None })
                        || !self.send(LinkEvent::Down { note, dead: false })
                    {
                        return;
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(self.settings.retry_max);
                }
                End::Unconfirmed => {
                    let note = format!(
                        "Kai asks again in {} s (the wait doubles up to {} s, lmgw.retry_max_s)",
                        delay.as_secs(),
                        self.settings.retry_max.as_secs()
                    );
                    tracing::info!(
                        "lmgw link: the device waits for the owner's confirmation; {note}"
                    );
                    if !self.send(LinkEvent::Unconfirmed { note }) {
                        return;
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(self.settings.retry_max);
                }
                End::Dropped { why, was_up, dead } => {
                    if was_up {
                        delay = FIRST_RETRY;
                    }
                    let note = format!(
                        "{why}; next try in {} s (the wait doubles up to {} s, lmgw.retry_max_s)",
                        delay.as_secs(),
                        self.settings.retry_max.as_secs()
                    );
                    tracing::info!("lmgw link: {note}");
                    if self.events.send(LinkEvent::Down { note, dead }).is_err() {
                        return;
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(self.settings.retry_max);
                }
            }
        }
    }

    fn send(&self, e: LinkEvent) -> bool {
        self.events.send(e).is_ok()
    }

    /// One connection, from the request to its end.
    async fn connect(&self, cursor: &mut Option<String>, stated: &mut Option<u32>) -> End {
        let timeout = self.settings.timeout;
        let secs = timeout.as_secs();
        let dropped = |why: String| End::Dropped {
            why,
            was_up: false,
            dead: false,
        };
        let req = requests::feed(cursor.as_deref(), None);
        let mut headers = Vec::with_capacity(req.headers.len());
        for h in &req.headers {
            match super::header(h) {
                Ok(h) => headers.push(h),
                Err(e) => return dropped(e),
            }
        }
        // Kai's own date decides a lapse (lmgw's device link §5.3), checked before each try: lmgw
        // ends the links at `notAfter`, and the next try finds it here. A handshake's alert never
        // does (the security review's B1, `tls`'s module doc).
        if self.creds.tls.lapsed(crate::pairing::now_ms()) {
            let date = self.creds.tls.not_after_ms().map(crate::pairing::date);
            tracing::warn!(
                ?date,
                "lmgw link: the device's certificate lapsed by Kai's date"
            );
            return End::Revoked(
                format!(
                    "this device's certificate lapsed on {} by Kai's date",
                    date.unwrap_or_default()
                ),
                Some(KeyLoss::Lapsed),
            );
        }
        tracing::debug!(resume = cursor.is_some(), "lmgw link: connecting the feed");
        // Each try's client is its own, so the race sees when its connection is up (§6.3).
        let attempt = |base: &str, answered: tls::Answered| {
            let http = self.creds.tls.racing(answered);
            let url = req.url(base);
            let headers = &headers;
            async move {
                let mut rb = http?.get(url);
                for (name, value) in headers {
                    rb = rb.header(name, value);
                }
                rb.send().await
            }
        };
        let raced = tls::race(self.creds.reach.order(), self.settings.stagger, attempt);
        let mut resp = match tokio::time::timeout(timeout, raced).await {
            Err(_) => {
                return dropped(format!(
                    "lmgw did not answer the feed's request within {secs} s (lmgw.timeout_s)"
                ));
            }
            Ok(Err(failed)) => {
                // An alert about the device's certificate is said and tried again, never acted
                // on: it may come from whoever answers at the address before the server proved
                // anything (B1). Kai's date above decides a lapse; one the alert claims while that
                // date holds is said as the clocks disagreeing.
                let mut why = unreachable(&failed);
                for (url, e) in &failed {
                    if let Some(alert) = tls::certificate_alert(e) {
                        tracing::warn!(%url, %alert, "lmgw link: the server refused the device's certificate in the handshake; Kai tries again");
                    }
                }
                let lapse_claimed = failed.iter().any(|(_, e)| {
                    tls::refusal(e) == Some(lmgw_client::tls::TlsRefusal::CertificateLapsed)
                });
                if lapse_claimed && let Some(t) = self.creds.tls.not_after_ms() {
                    why.push_str(&format!(
                        " (by Kai's date this device's certificate holds until {}: check this \
                         computer's clock and lmgw's)",
                        crate::pairing::date(t)
                    ));
                }
                return dropped(why);
            }
            Ok(Ok((url, r))) => {
                self.creds.reach.reached(&url);
                r
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let body = match tokio::time::timeout(timeout, resp.text()).await {
                Ok(Ok(b)) => b,
                _ => String::new(),
            };
            let e = requests::read_refusal(status.as_u16(), &body);
            let why = requests::key_refused(&e);
            if status == reqwest::StatusCode::UNAUTHORIZED
                || why == Some(requests::KeyRefused::CredentialOnDeviceListener)
            {
                // A device's 401s (`device_key_unknown`: re-paired or deleted, `device_disabled`,
                // `key_expired`, `device_certificate_expired`, `client_certificate_required`), the
                // 400 for a credential beside the certificate, and any other 401: the pairing
                // opens nothing now. "Pair again" with lmgw's words, except for a disabled device
                // and an expired key, which work again once the owner enables the device or moves
                // the expiry (§3.3).
                tracing::warn!(code = %e.code, ?why, "lmgw link: lmgw refused the feed");
                return End::Revoked(e.message, Some(KeyLoss::of_code(&e.code)));
            }
            if why == Some(requests::KeyRefused::Unconfirmed) {
                // Device link §3.5: lmgw's words are for people; Kai says its own code.
                tracing::info!("lmgw link: the device is not confirmed yet");
                return End::Unconfirmed;
            }
            return dropped(format!(
                "lmgw refused the feed (HTTP {}, {}: {})",
                status.as_u16(),
                e.code,
                e.message
            ));
        }
        let mut reader = FeedReader::new(cursor.as_deref());
        let mut translate = Translate::default();
        let mut up = false;
        // Any byte is progress, a keep-alive comment included; what may pass without one is
        // [`Wait`]'s, by how far the stream got.
        let mut began = false;
        loop {
            let wait = Wait::now(up, began, *stated, timeout);
            let chunk = match tokio::time::timeout(wait.bound(), resp.chunk()).await {
                Err(_) => {
                    return End::Dropped {
                        why: wait.said(),
                        was_up: up,
                        dead: up,
                    };
                }
                Ok(Err(e)) => {
                    return End::Dropped {
                        why: format!("the feed broke: {}", describe(&e)),
                        was_up: up,
                        dead: false,
                    };
                }
                Ok(Ok(None)) => {
                    return End::Dropped {
                        why: "lmgw closed the feed".to_owned(),
                        was_up: up,
                        dead: false,
                    };
                }
                Ok(Ok(Some(bytes))) => bytes,
            };
            began |= !chunk.is_empty();
            for item in reader.push(&chunk) {
                match item {
                    FeedItem::Event(ev) => {
                        // Up from `hello` on, not from the chunk's end: the catch-up lmgw sends
                        // right behind it may share its chunk (review KAI-3 K3-3).
                        let hello = matches!(ev, FeedEvent::Hello(_));
                        up |= hello;
                        // What `hello` says about this device, as the reader kept it, before the
                        // connection is said (lmgw's device link §7.2).
                        if hello && !self.send(LinkEvent::Device(DeviceFacts::of(&reader))) {
                            return End::Gone;
                        }
                        match translate.event(ev) {
                            Step::Revoked(reason, loss) => return End::Revoked(reason, loss),
                            Step::Events(evs) => {
                                for e in evs {
                                    if !self.send(e) {
                                        return End::Gone;
                                    }
                                }
                            }
                        }
                    }
                    FeedItem::KeepAlive => {}
                    // The open approvals (device link §7.3): the whole set at `caught_up`, then
                    // what is added and withdrawn.
                    FeedItem::Approvals(change) => {
                        if let Some(e) = Translate::approvals(change)
                            && !self.send(e)
                        {
                            return End::Gone;
                        }
                    }
                    // An event of a type this build does not know (a newer gateway's): skipped,
                    // never a link failure. Its name only, never its data, which may be the
                    // owner's messages (review KAI-2 K2-10).
                    FeedItem::Unknown { event, data } => tracing::debug!(
                        %event,
                        bytes = data.len(),
                        "lmgw link: an event this build does not know, skipped"
                    ),
                    // A `hello` that does not read: version skew, said at once rather than as a
                    // missing hello after lmgw.timeout_s (review KAI-3 K3-4); the backoff stays.
                    FeedItem::Unreadable { event, data } if !up && event == "hello" => {
                        tracing::warn!(
                            bytes = data.len(),
                            "lmgw link: the feed's hello does not read"
                        );
                        return dropped(
                            "lmgw's hello does not read (this Kai may be older than lmgw)"
                                .to_owned(),
                        );
                    }
                    // A known event whose data does not read: the cursor moved past it, so what it
                    // was about may be shown stale. Kai reloads as for `resync` once the feed is
                    // up (before `hello`, the missing hello says it). Its name only (K2-10).
                    FeedItem::Unreadable { event, data } => {
                        tracing::warn!(
                            %event,
                            bytes = data.len(),
                            "lmgw link: an event whose data does not read; Kai reloads as for resync"
                        );
                        if up && !self.send(LinkEvent::Fact(FeedFact::Resync)) {
                            return End::Gone;
                        }
                    }
                    _ => {
                        tracing::debug!("lmgw link: a feed item this build does not know, skipped")
                    }
                }
            }
            if let Some(c) = reader.cursor() {
                *cursor = Some(c.to_owned());
            }
            if let Some(k) = reader.keepalive_s() {
                up = true;
                *stated = Some(k);
            }
        }
    }
}

/// Every URL failed: why, in lmgw-client's words for a refused handshake.
fn unreachable(failed: &[(String, reqwest::Error)]) -> String {
    match failed {
        [(_, e)] => tls::unreachable(e),
        many => format!(
            "lmgw is not reachable at any of its addresses: {}",
            many.iter()
                .map(|(u, e)| format!("{u}: {}", tls::why(e)))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// How long the feed may go without a byte (§3.2), by how far the stream got. Every byte is
/// progress, a keep-alive comment included, so a stream that keeps sending is never cut; one that
/// sends nothing is, and says which wait ran out in the tooltip and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// lmgw answered the request; the body's first bytes are due within `lmgw.timeout_s`: `hello`,
    /// or the keep-alive comment lmgw sends first while a level change goes live (api §2.1).
    FirstBytes(Duration),
    /// The body began without `hello`, which lmgw sends within one keep-alive interval (api §2.1):
    /// two intervals without a byte, as once it is up. The interval is the one lmgw stated in its
    /// last `hello` on this run (`stated`), or `lmgw.timeout_s` until it stated one.
    Hello { interval: Duration, stated: bool },
    /// Up: two `hello.keepalive_s` intervals without a byte are a dead link.
    Up { interval: Duration },
}

impl Wait {
    fn now(up: bool, began: bool, stated: Option<u32>, timeout: Duration) -> Self {
        let interval = stated.map(|k| Duration::from_secs(u64::from(k.max(1))));
        match (up, began, interval) {
            (true, _, Some(interval)) => Self::Up { interval },
            (_, false, _) => Self::FirstBytes(timeout),
            (_, true, Some(interval)) => Self::Hello {
                interval,
                stated: true,
            },
            (_, true, None) => Self::Hello {
                interval: timeout,
                stated: false,
            },
        }
    }

    fn bound(self) -> Duration {
        match self {
            Self::FirstBytes(t) => t,
            Self::Hello { interval, .. } | Self::Up { interval } => interval * 2,
        }
    }

    fn said(self) -> String {
        let n = self.bound().as_secs();
        match self {
            Self::FirstBytes(_) => {
                format!("lmgw sent no hello, nor any byte, within {n} s (lmgw.timeout_s)")
            }
            Self::Hello { interval, stated } => format!(
                "lmgw began the feed but sent no hello and nothing else for {n} s (two keep-alive \
                 intervals of {} s: {})",
                interval.as_secs(),
                if stated {
                    "hello.keepalive_s, as lmgw last stated it"
                } else {
                    "lmgw.timeout_s, until lmgw states its keep-alive in a hello"
                }
            ),
            Self::Up { .. } => format!(
                "lmgw sent nothing for {n} s (two hello.keepalive_s intervals): the link is dead"
            ),
        }
    }
}

/// What one feed event means for the core.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Events(Vec<LinkEvent>),
    /// §3.3: the stream ends after it. What its `kind` asks (`None`: a kind this build does not
    /// know, or none).
    Revoked(String, Option<KeyLoss>),
}

/// The feed's events as [`LinkEvent`]s. Sans-IO; one per connection, since each connection's
/// `hello` names the principal anew.
#[derive(Debug, Default)]
struct Translate {
    /// The `by` of Kai's own changes (`FeedPrincipal::by` of `hello`'s principal).
    own: Option<String>,
}

impl Translate {
    fn by(&self, name: String) -> By {
        By {
            own: self.own.as_deref() == Some(name.as_str()),
            name,
        }
    }

    fn live(&self, voice: Vec<LiveVoice>, turns: Vec<LiveTurn>) -> LinkEvent {
        LinkEvent::Fact(FeedFact::Live {
            voice: voice
                .into_iter()
                .map(|v| (v.thread_id, self.by(v.by)))
                .collect(),
            turns: turns
                .into_iter()
                .map(|t| (t.thread_id, self.by(t.by)))
                .collect(),
        })
    }

    /// A change of the open approvals as the approval card's fact; `None` for a change a newer
    /// `lmgw-client` added (logged).
    fn approvals(change: ApprovalsChange) -> Option<LinkEvent> {
        fn requested(r: ApprovalRequested) -> crate::approvals::Requested {
            crate::approvals::Requested {
                thread_id: r.thread_id,
                request: lmgw_client::requests::approvals::ApprovalRequest {
                    approval_request_id: r.approval_request_id,
                    server_label: r.server_label,
                    name: r.name,
                    arguments: r.arguments,
                    call_id: r.call_id,
                },
            }
        }
        let fact = match change {
            ApprovalsChange::Open(set) => {
                FeedFact::ApprovalsOpen(set.into_iter().map(requested).collect())
            }
            ApprovalsChange::Added(r) => FeedFact::ApprovalRequested(requested(r)),
            ApprovalsChange::Withdrawn(ids) => FeedFact::ApprovalsWithdrawn(ids),
            // Not its Debug: a call's arguments may be in it.
            _ => {
                tracing::debug!("lmgw link: an approvals change this build does not know, skipped");
                return None;
            }
        };
        Some(LinkEvent::Fact(fact))
    }

    fn event(&mut self, ev: FeedEvent) -> Step {
        let fact = |f| Step::Events(vec![LinkEvent::Fact(f)]);
        let folder = |f| Step::Events(vec![LinkEvent::Folder(f)]);
        match ev {
            FeedEvent::Hello(h) => {
                self.own = Some(h.principal.by());
                tracing::info!(
                    device = %h.principal.name,
                    kind = %h.principal.kind,
                    hosts_label = ?h.hosts_label,
                    admin_tools = %h.self_admin,
                    keepalive_s = h.keepalive_s,
                    retention_days = h.retention_days,
                    held = h.hold.active,
                    "lmgw link: the feed is up"
                );
                let live = self.live(h.voice, h.turns);
                Step::Events(vec![
                    LinkEvent::Connected {
                        device: h.principal.name,
                        held: h.hold.active,
                        hosts_label: h.hosts_label,
                        admin: h.self_admin,
                    },
                    live,
                ])
            }
            FeedEvent::State(s) => {
                tracing::info!(reason = ?s.reason, admin_tools = %s.self_admin, "lmgw link: the live state again");
                let live = self.live(s.voice, s.turns);
                Step::Events(vec![
                    LinkEvent::Hold {
                        active: s.hold.active,
                    },
                    LinkEvent::AdminTools(s.self_admin),
                    live,
                ])
            }
            FeedEvent::Hold(h) => {
                tracing::info!(active = h.active, fallback = ?h.fallback_alias, "lmgw link: hold");
                Step::Events(vec![LinkEvent::Hold { active: h.active }])
            }
            FeedEvent::TurnStarted(t) => fact(FeedFact::TurnStarted {
                thread_id: t.thread_id,
                by: self.by(t.by),
            }),
            FeedEvent::TurnDone(t) => fact(FeedFact::TurnDone {
                thread_id: t.thread_id,
            }),
            FeedEvent::VoiceBound(v) => fact(FeedFact::VoiceBound {
                thread_id: v.thread_id,
                by: self.by(v.by),
            }),
            FeedEvent::VoiceEnded(v) => fact(FeedFact::VoiceEnded {
                thread_id: v.thread_id,
                by: self.by(v.by),
            }),
            FeedEvent::Resync(r) => {
                tracing::warn!(reason = %r.reason, "lmgw link: resync; Kai asks for its thread again");
                fact(FeedFact::Resync)
            }
            // §3.3: what it asks is its `kind` (api §1.6), never its words; an event without one
            // (an older gateway's) is of an unknown kind.
            FeedEvent::Revoked(r) => {
                let loss = KeyLoss::of_kind(&r.kind);
                tracing::warn!(kind = %r.kind, reason = %r.reason, ?loss, "lmgw link: revoked");
                Step::Revoked(
                    if r.message.is_empty() {
                        r.reason
                    } else {
                        r.message
                    },
                    loss,
                )
            }
            // §4: the core follows its own folder's events, the voice its thread's.
            FeedEvent::FolderCurrent(f) => {
                tracing::debug!(folder = f.folder_id, thread = ?f.thread_id, reason = %f.reason, "lmgw link: folder.current");
                folder(FolderEvent::Current {
                    folder_id: f.folder_id,
                    thread_id: f.thread_id,
                    previous: f.previous_thread_id,
                    reason: f.reason,
                })
            }
            FeedEvent::FolderCreated(n) | FeedEvent::FolderUpdated(n) => match n {
                FolderNow::Row(r) => folder(FolderEvent::Changed(FolderInfo::from(&r.folder))),
                FolderNow::Gone(g) => folder(FolderEvent::Gone(g.folder_id)),
            },
            FeedEvent::FolderDeleted(g) => folder(FolderEvent::Gone(g.folder_id)),
            // WP22: the profiles, for the tray's picker.
            FeedEvent::ProfileCreated(p) | FeedEvent::ProfileUpdated(p) => {
                Step::Events(vec![LinkEvent::Profile(ProfileEvent::Upsert {
                    id: p.id,
                    name: p.name,
                })])
            }
            FeedEvent::ProfileDeleted(p) => {
                Step::Events(vec![LinkEvent::Profile(ProfileEvent::Removed { id: p.id })])
            }
            FeedEvent::ThreadCreated(n) | FeedEvent::ThreadUpdated(n) => match n {
                ThreadNow::Row(r) => Step::Events(vec![
                    LinkEvent::Profile(ProfileEvent::Thread {
                        thread_id: r.thread.id,
                        profile_id: r.thread.profile_id,
                    }),
                    LinkEvent::Fact(FeedFact::ThreadChanged {
                        thread_id: r.thread.id,
                        title: r.thread.title,
                        archived: r.thread.archived_at.is_some(),
                        last_message_at: r.thread.last_message_at,
                    }),
                ]),
                ThreadNow::Gone(g) => fact(FeedFact::ThreadGone {
                    thread_id: g.thread_id,
                }),
            },
            FeedEvent::ThreadDeleted(g) => fact(FeedFact::ThreadGone {
                thread_id: g.thread_id,
            }),
            // WP19: the approval card's calls come from the reader's open set
            // (`FeedItem::Approvals`, device link §7.3), never from these events themselves: one
            // in the catch-up is already in `hello`'s set, and after `caught_up` the reader says
            // what each changed right after it.
            FeedEvent::ApprovalRequested(r) => {
                tracing::debug!(call = %r.approval_request_id, thread = r.thread_id, "lmgw link: approval.requested");
                Step::Events(Vec::new())
            }
            FeedEvent::ApprovalDecided(d) => {
                tracing::debug!(call = %d.approval_request_id, approve = d.approve && !d.not_run, by = ?d.by, "lmgw link: approval.decided");
                Step::Events(Vec::new())
            }
            FeedEvent::CaughtUp(c) => {
                tracing::debug!(cursor = %c.cursor, "lmgw link: caught up");
                Step::Events(Vec::new())
            }
            // WP18: a job's result entered its thread (the voice speaks or notifies it).
            FeedEvent::TaskDone(d) => fact(FeedFact::TaskDone(d)),
            // WP18: a deleted device's jobs here are cancelled (the kind is `device`; an owner
            // is never deleted).
            FeedEvent::DeviceRevoked(d)
                if d.device.kind == "device" && !d.device.name.is_empty() =>
            {
                Step::Events(vec![LinkEvent::DeviceRevoked {
                    name: d.device.name,
                }])
            }
            // Any other event, a newer gateway's included: skipped, never a link failure. Its name
            // only, never its data (review KAI-2 K2-10).
            other => {
                tracing::debug!(event = %other.name(), "lmgw link: an event this build does not follow");
                Step::Events(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests;
