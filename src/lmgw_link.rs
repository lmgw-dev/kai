//! The lmgw link (§3.2, §3.3): the seam the core drives, and [`FeedLink`], Kai's implementation on
//! top of `lmgw-client`'s feed reader.
//!
//! [`FeedLink`] keeps one `GET /chat/api/feed` open with the device key (api §2):
//! - resumed with `Last-Event-ID` (the reader's cursor, opaque text) after every reconnect of a run;
//!   a `resync` is passed on as [`crate::voice::FeedFact::Resync`], and so is a known event whose
//!   data does not read (`FeedItem::Unreadable`: what it was about may be shown stale);
//! - reconnecting after 1 s, doubling to `lmgw.retry_max_s` (§3.2), reported as
//!   [`LinkEvent::Down`] with the next try in its note; the backoff starts over after a `hello`;
//! - every byte is progress, a keep-alive comment included: `lmgw.timeout_s` for the request's
//!   answer and the body's first bytes; a body that began without `hello` (lmgw's keep-alive comment
//!   while a level change goes live, api §2.1) waits two keep-alive intervals between bytes, the
//!   interval lmgw stated in its last `hello` on the run or `lmgw.timeout_s` until it stated one;
//!   once up, two `hello.keepalive_s` intervals without a byte are a dead link;
//! - validating the key by opening the feed (§3.1 step 3): `hello` is [`LinkEvent::Connected`];
//! - §3.3: the `revoked` event or a 401 on the feed (`device_key_unknown`, `device_disabled`,
//!   `key_expired`: `lmgw_client::requests::key_refused`) is [`LinkEvent::Revoked`] with the
//!   server's message verbatim and what it asks of the owner, by the event's `kind` or the 401's
//!   code ([`KeyLoss`]: a disabled device is enabled again, an expired key's expiry is moved, a
//!   rotated or deleted key is paired again), after which the link stops retrying by itself; a
//!   `kind` Kai does not know is asked about once while the link tries again after its backoff,
//!   never started over by that end (a 4003 or 4004 close of a WebSocket link comes from the
//!   voice);
//! - `device.revoked` (another device's key deleted) is [`LinkEvent::DeviceRevoked`]: the core
//!   cancels the jobs that device started;
//! - what the device's admin tools may do (`hello.self_admin`, then `state.self_admin` when it
//!   moves) as lmgw's [`AdminLevel`];
//! - the live facts the voice session acts on (§3.2's table: `hello`'s and `state`'s `voice` and
//!   `turns`, `voice.bound`/`voice.ended`, `turn.started`/`turn.done`, `resync`) as
//!   [`LinkEvent::Fact`], each `by` marked `own` when it is Kai's own principal
//!   (`FeedPrincipal::by` of `hello`'s principal); the threads' title, archive and deletion as
//!   facts too, and the folders' events as [`LinkEvent::Folder`] (§4: the core follows its own
//!   folder's and passes its `folder.current` on to the voice);
//! - the personality profiles' events (`profile.*`) and each thread's profile as
//!   [`LinkEvent::Profile`] (WP22: the tray's picker).
//!
//! [`mcp`] is Kai's own client of lmgw's `/mcp` (WP12: the MCP Apps bridge's reads and calls).
//! [`proxy`] sends the configuration window's profile-editor requests with the device key (WP21b).
//!
//! The core owns one implementation, calls [`LmgwLink::start`] after every successful pairing or
//! load, and [`LmgwLink::stop`] at quit, re-pairing and revocation. Each start gets a fresh event
//! channel and a fresh cursor, so events of an earlier run never reach the model.

mod feed;
pub mod mcp;
pub mod proxy;
mod realtime;

use lmgw_client::realtime::RevokeKind;
use lmgw_client::requests::KeyRefused;
use tokio::sync::mpsc;

use crate::conversation::FolderInfo;
use crate::pairing::DeviceKey;

pub use feed::{FeedLink, FeedSettings};
pub(crate) use realtime::refusal_body;
pub use realtime::{RealtimeLink, RealtimeSettings};

/// What the link needs to connect.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// The paired base URL (`lmgw.url`).
    pub url: String,
    pub key: DeviceKey,
}

/// What this device's admin tools may do now (api L3, the pre-merge review's P-3):
/// `hello.self_admin` and `state.self_admin`, lmgw's typed level (`off`, `read_only`, `full`, or
/// a newer gateway's as sent). Since lmgw 3ac1155 it is the level the owner set on the Devices
/// card already capped by lmgw's own self-admin level, and lmgw sends a `state` whenever either
/// moves it. Above `off` the device also sees the threads and folders that carry the self-admin
/// toolset.
pub use lmgw_client::types::AdminLevel;

/// The tooltip's line for the device's admin tools ("admin tools: read only"); none at `off`.
/// The level is what the tools may do, lmgw's own level included, so it needs no "up to" (review
/// KAI-3 K3-7 added it while the feed sent the device's own level). A level a newer lmgw added
/// reads in words ("ask first"); an empty one, which no feed sends, as `off`.
pub fn admin_note(level: &AdminLevel) -> Option<String> {
    let level = match level {
        AdminLevel::Off => return None,
        AdminLevel::ReadOnly => "read only".to_owned(),
        AdminLevel::Full => "full".to_owned(),
        other if other.as_str().trim().is_empty() => return None,
        other => other.as_str().replace('_', " "),
    };
    Some(format!("admin tools: {level}"))
}

/// What a key that opens nothing now asks of the owner (§3.3, review KAI-3 K3-6), as lmgw says it
/// by kind (api §1.6, §1.8): a 401's code, the feed's `revoked.kind`, and a 4003 close's token
/// (`lmgw_client::realtime::close_kind`). Kai never reads it from lmgw's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLoss {
    /// The key matches no device any more (rotated or deleted: `device_key_unknown`,
    /// `key_unknown`), another kind of key was revoked (`revoked`), or a 401 Kai cannot tell
    /// apart: the device is paired again.
    PairAgain,
    /// The device is disabled on lmgw (`device_disabled`): the same key works once the owner
    /// enables it again, so Kai keeps it.
    Disabled,
    /// The key passed its expiry (`key_expired`): the same key works once the owner moves the
    /// expiry, so Kai keeps it.
    Expired,
}

impl KeyLoss {
    /// A 401's code, as `lmgw_client::requests::key_refused` reads it; any other 401 is
    /// [`KeyLoss::PairAgain`].
    pub fn of_code(code: &str) -> Self {
        let refusal = lmgw_client::requests::ApiError {
            code: code.to_owned(),
            message: String::new(),
        };
        match lmgw_client::requests::key_refused(&refusal) {
            Some(KeyRefused::Disabled) => Self::Disabled,
            Some(KeyRefused::Expired) => Self::Expired,
            _ => Self::PairAgain,
        }
    }

    /// A revocation's kind: the feed's `revoked.kind`, or the token a 4003 close's reason starts
    /// with. `None` for a kind this build does not know, or none (an older gateway's): lmgw's
    /// guidance is to say its sentence, and Kai then asks lmgw once whether the key still opens
    /// (§3.3).
    pub fn of_kind(kind: &RevokeKind) -> Option<Self> {
        match kind {
            RevokeKind::DeviceDisabled => Some(Self::Disabled),
            RevokeKind::KeyExpired => Some(Self::Expired),
            RevokeKind::KeyUnknown | RevokeKind::Revoked => Some(Self::PairAgain),
            _ => None,
        }
    }
}

/// What the link reports to the core, which maps it onto the tray model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// The feed's `hello` arrived: the key is valid. `device` from `hello.principal.name`, `held`
    /// from `hello.hold.active`, `hosts_label` (the label Kai's hosted tools go under, §10.1)
    /// from `hello.hosts_label`, `admin` from `hello.self_admin`.
    Connected {
        device: String,
        held: bool,
        hosts_label: Option<String>,
        admin: AdminLevel,
    },
    /// The feed is down or not yet up; `note` says why and when it retries. `dead`: the link's
    /// dead-link verdict (two keep-alive intervals without a byte), which also ends a bound voice
    /// session (review KAI-2 K2-6).
    Down { note: String, dead: bool },
    /// The feed's `hold` event, and `state`'s hold.
    Hold { active: bool },
    /// `state`'s `self_admin`: the device's level of lmgw's admin tools as it is now (lmgw sends a
    /// `state` after every move of the level).
    AdminTools(AdminLevel),
    /// §3.3: the key was revoked, disabled, rotated or expired: the feed's `revoked` event, or a 401
    /// on the feed. `reason` is the server's message, `loss` what it asks of the owner; the link
    /// then stops. `None` for a `revoked` of a kind this build does not know, or none: Kai asks
    /// lmgw once, and the link tries again after its backoff (a [`LinkEvent::Down`] follows).
    Revoked {
        reason: String,
        loss: Option<KeyLoss>,
    },
    /// The feed's `device.revoked`: the paired device `name` was deleted in lmgw (never a disable,
    /// expiry or rotation); the jobs it started are cancelled (WP18).
    DeviceRevoked { name: String },
    /// A fact for the voice session (K5, K16).
    Fact(crate::voice::FeedFact),
    /// A folder event (§4): the core follows its conversation's folder.
    Folder(FolderEvent),
    /// A personality profile's event, or a thread's profile (WP22): the tray's picker follows it.
    Profile(ProfileEvent),
}

/// The feed's personality events (WP22; lmgw's personality-profiles design §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileEvent {
    /// `profile.created` / `profile.updated`, and the list repeated after a `resync`.
    Upsert { id: i64, name: String },
    /// `profile.deleted`.
    Removed { id: i64 },
    /// `thread.created` / `thread.updated`: the profile the thread talks with now (`None`: none).
    Thread {
        thread_id: i64,
        profile_id: Option<i64>,
    },
}

/// The feed's folder events (§3.2's table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderEvent {
    /// `folder.created` / `folder.updated`: the folder as it is now.
    Changed(FolderInfo),
    /// `folder.deleted`, or a folder gone by the time its event was delivered.
    Gone(i64),
    /// `folder.current`: the folder's current thread moved (`None`: it has none any more).
    Current {
        folder_id: i64,
        thread_id: Option<i64>,
        previous: Option<i64>,
        reason: String,
    },
}

pub trait LmgwLink: Send + 'static {
    /// Starts the link for `creds`, replacing any earlier run. Events go to `events` until
    /// [`LmgwLink::stop`] or the next start.
    fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<LinkEvent>);
    /// Closes every connection of the link.
    fn stop(&mut self);
}

/// A request header as the HTTP and WebSocket clients take it. `Authorization` (the device key) is
/// marked sensitive, so no `Debug` of a request shows it (review KAI-2 K2-1; the log cap in
/// `crate::logging` keeps the libraries' own traces of it out). An invalid header names itself,
/// never its value.
pub(crate) fn header(
    h: &lmgw_client::requests::Header,
) -> Result<(reqwest::header::HeaderName, reqwest::header::HeaderValue), String> {
    let name = reqwest::header::HeaderName::from_bytes(h.name.as_bytes())
        .map_err(|_| format!("the header {} cannot be sent", h.name))?;
    let mut value = reqwest::header::HeaderValue::from_str(&h.value)
        .map_err(|_| format!("the header {} cannot be sent", h.name))?;
    if name == reqwest::header::AUTHORIZATION {
        value.set_sensitive(true);
    }
    Ok((name, value))
}

/// An error and its causes, down to the one that says what happened ("Connection refused").
pub(crate) fn describe(e: &(dyn std::error::Error + 'static)) -> String {
    let mut last = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let text = s.to_string();
        if !text.is_empty() {
            last = text;
        }
        cur = s.source();
    }
    last
}

#[cfg(test)]
mod tests {
    use lmgw_client::realtime::{CloseKind, RevokeKind, close_kind};

    use super::KeyLoss;

    /// Review KAI-3 K3-6, and lmgw's kinds (api §1.6, 2026-10-07): a disabled device and an
    /// expired key keep the key; a rotated or deleted key, and another kind of key's revocation, is
    /// paired again. A 401's code, the feed's `revoked.kind` and a 4003 close's token each say it.
    #[test]
    fn each_kind_says_what_it_asks() {
        for (code, loss) in [
            ("device_disabled", KeyLoss::Disabled),
            ("key_expired", KeyLoss::Expired),
            ("device_key_unknown", KeyLoss::PairAgain),
            // Any other 401 opens nothing either.
            ("session_required", KeyLoss::PairAgain),
            ("", KeyLoss::PairAgain),
        ] {
            assert_eq!(KeyLoss::of_code(code), loss, "{code}");
        }
        for (kind, loss) in [
            ("device_disabled", Some(KeyLoss::Disabled)),
            ("key_expired", Some(KeyLoss::Expired)),
            ("key_unknown", Some(KeyLoss::PairAgain)),
            ("revoked", Some(KeyLoss::PairAgain)),
            // A newer gateway's kind: Kai asks lmgw once.
            ("key_paused", None),
        ] {
            let kind: RevokeKind = serde_json::from_value(serde_json::json!(kind)).unwrap();
            assert_eq!(KeyLoss::of_kind(&kind), loss, "{kind}");
        }
        // An event without a kind (an older gateway's) is of an unknown kind.
        assert_eq!(KeyLoss::of_kind(&RevokeKind::default()), None);
        // A 4003's token, read by lmgw-client; never Kai's reading of the words.
        for (reason, loss) in [
            (
                "device_disabled: device 'desktop' was disabled",
                Some(KeyLoss::Disabled),
            ),
            (
                "key_expired: device 'desktop' expired",
                Some(KeyLoss::Expired),
            ),
            (
                "key_unknown: device 'desktop' was rotated — pair it again",
                Some(KeyLoss::PairAgain),
            ),
            (
                "revoked: key 'laptop' was disabled",
                Some(KeyLoss::PairAgain),
            ),
            ("key_paused: device 'desktop' was paused", None),
            // An older gateway's reason, with no token: its words are not read.
            ("device 'desktop' was disabled", None),
            ("", None),
        ] {
            let CloseKind::Revoked { kind } = close_kind(4003, reason) else {
                panic!("{reason}: a 4003 is a revocation");
            };
            assert_eq!(KeyLoss::of_kind(&kind), loss, "{reason}");
        }
    }
}
