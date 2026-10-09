//! The feed task (§3.2, §3.3): one `GET /chat/api/feed` at a time over reqwest, its bytes through
//! `lmgw-client`'s [`FeedReader`], its events mapped onto [`LinkEvent`]s by [`Translate`].

use std::time::Duration;

use lmgw_client::feed::{FeedEvent, FeedItem, FeedReader, FolderNow, ThreadNow};
use lmgw_client::requests;
use lmgw_client::types::chat_feed::{LiveTurn, LiveVoice};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{Credentials, FolderEvent, KeyLoss, LinkEvent, LmgwLink, ProfileEvent, describe};
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
}

impl FeedSettings {
    pub fn from_config(c: &Config) -> Self {
        Self {
            retry_max: c.lmgw.retry_max(),
            timeout: c.lmgw.timeout(),
        }
    }
}

/// Kai's lmgw link: the feed task, started and stopped by the core.
pub struct FeedLink {
    settings: FeedSettings,
    client: reqwest::Client,
    task: Option<JoinHandle<()>>,
}

impl FeedLink {
    /// The HTTP client is built once (it reads the system's trust store, for an https gateway).
    /// No proxy: Kai talks to its gateway directly (loopback, the LAN, a tunnel), and a proxy
    /// would hold back the event stream.
    pub fn new(settings: FeedSettings) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {}", describe(&e)))?;
        Ok(Self {
            settings,
            client,
            task: None,
        })
    }

    /// The HTTP client, for the voice's requests to the same gateway (`RealtimeLink`).
    pub fn client(&self) -> reqwest::Client {
        self.client.clone()
    }
}

impl LmgwLink for FeedLink {
    fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<LinkEvent>) {
        self.stop();
        tracing::info!(url = %creds.url, "lmgw link: opening the feed");
        let run = Run {
            client: self.client.clone(),
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
    /// The core no longer listens.
    Gone,
}

struct Run {
    client: reqwest::Client,
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
        // The request carries the key: it is never logged.
        let req = requests::feed(cursor.as_deref()).bearer(self.creds.key.expose());
        let mut rb = self.client.get(req.url(&self.creds.url));
        for h in &req.headers {
            match super::header(h) {
                Ok((name, value)) => rb = rb.header(name, value),
                Err(e) => {
                    return End::Dropped {
                        why: e,
                        was_up: false,
                        dead: false,
                    };
                }
            }
        }
        tracing::debug!(resume = cursor.is_some(), "lmgw link: connecting the feed");
        let dropped = |why: String| End::Dropped {
            why,
            was_up: false,
            dead: false,
        };
        let mut resp = match tokio::time::timeout(timeout, rb.send()).await {
            Err(_) => {
                return dropped(format!(
                    "lmgw did not answer the feed's request within {secs} s (lmgw.timeout_s)"
                ));
            }
            Ok(Err(e)) => return dropped(format!("lmgw is not reachable: {}", describe(&e))),
            Ok(Ok(r)) => r,
        };
        let status = resp.status();
        if !status.is_success() {
            let body = match tokio::time::timeout(timeout, resp.text()).await {
                Ok(Ok(b)) => b,
                _ => String::new(),
            };
            let e = requests::read_refusal(status.as_u16(), &body);
            if status == reqwest::StatusCode::UNAUTHORIZED {
                // A device's 401s (`device_key_unknown`: rotated or deleted, `device_disabled`,
                // `key_expired`) and any other: the key opens nothing now. "Pair again" with
                // lmgw's words, except for a disabled device and an expired key, whose key works
                // again once the owner enables the device or moves the expiry (§3.3).
                let why = requests::key_refused(&e);
                tracing::warn!(code = %e.code, ?why, "lmgw link: the feed answered 401");
                return End::Revoked(e.message, Some(KeyLoss::of_code(&e.code)));
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
                        up |= matches!(ev, FeedEvent::Hello(_));
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
            // WP19: the approval card's calls (any thread the device sees).
            FeedEvent::ApprovalRequested(r) => {
                fact(FeedFact::ApprovalRequested(crate::approvals::Requested {
                    thread_id: r.thread_id,
                    request: lmgw_client::requests::approvals::ApprovalRequest {
                        approval_request_id: r.approval_request_id,
                        server_label: r.server_label,
                        name: r.name,
                        arguments: r.arguments,
                        call_id: r.call_id,
                    },
                }))
            }
            FeedEvent::ApprovalDecided(d) => fact(FeedFact::ApprovalDecided {
                id: d.approval_request_id,
                approve: d.approve && !d.not_run,
                by: d.by,
            }),
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
