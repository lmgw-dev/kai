//! The screen lock (decided by the owner 2026-10-09: Kai is not active while the screen is
//! locked).
//!
//! **Which signal.** Two facts say the session is locked, and Kai follows both:
//! - logind's `LockedHint` on **Kai's own session** (the system bus: `XDG_SESSION_ID`'s session,
//!   else logind's `GetSessionByPID` of Kai's PID; not `user/self`'s `Display`, which can be
//!   another of the user's sessions), whose changes come as `PropertiesChanged`. Plasma's locker
//!   (KScreenLocker, in KWin) sets it when the lock is up and clears it at the unlock
//!   (`SetLockedHint`), and so do other desktops' lockers;
//! - the screen saver service's `ActiveChanged` (`org.freedesktop.ScreenSaver` on the session bus,
//!   `GetActive` at start), which KScreenLocker emits at the lock and the unlock. On Plasma it is
//!   the locker's own signal, so it holds where logind is out of reach.
//!
//! The session counts as locked while either says so (the safe side: Kai is not active when in
//! doubt). logind's `Lock` and `Unlock` signals are not used as the state: they are requests to the
//! locker (`loginctl lock-session` sends them whether or not anything locks), which KScreenLocker
//! answers by locking or unlocking, and the two facts above then follow.
//!
//! **Following the services, not a process.** A signal carries its sender's unique name, so each
//! fact's signal is followed from its service's current owner, and the owner itself is followed by
//! the bus's `NameOwnerChanged`: a service that leaves the bus (KWin or logind restarting) takes its
//! fact with it (`None`: it says nothing), a new owner is followed and asked again (`GetActive`,
//! `LockedHint`), and a screen saver service that appears after Kai's start is picked up. When the
//! two facts disagree after a change of one, the other is read again before the lock is published,
//! so a signal that never came cannot hold the lock (or the unlock) for good.
//!
//! **At start** the lock is not known until both sources were looked at ([`ScreenLock::unknown`]),
//! at most `dbus.timeout_s` after the system bus answered; until then it counts as locked (the host
//! refuses with [`REFUSAL`]). The watch publishes its first look in any case, a source that cannot
//! be followed then counting as unlocked (logged). The watch is the shared flag's only writer; the
//! core reads it at each `Command::ScreenLock`.
//!
//! **What locking does** (the core, `Command::ScreenLock`): the voice stops (the capture closes, a
//! press session ends with its audio discarded, conversation mode ends, a playing reply stops with
//! its truncate, the session closes), the talk key and listening on do nothing, the approval card
//! hides (its calls stay waiting, for other clients and for after the unlock), the widgets' surfaces
//! are unmapped, accessibility is put back (`crate::desktop::fdo::atspi::switch`), and every hosted
//! tool call is refused with [`REFUSAL`]: at its start, and a call still preparing, waiting for the
//! owner's confirmation or running is stopped ([`ScreenLock::locked`], `host::server`). After the
//! unlock Kai is idle: nothing resumes by itself.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use crate::core::Command;

/// What a hosted tool call answers while the screen is locked.
pub const REFUSAL: &str = "the screen is locked; Kai is not active";

const LOGIND: &str = "org.freedesktop.login1";
const SAVER: &str = "org.freedesktop.ScreenSaver";
const DBUS: &str = "org.freedesktop.DBus";

/// Whether the screen is locked now, shared with every place that refuses while it is. `None`:
/// not known yet (counts as locked).
#[derive(Clone)]
pub struct ScreenLock {
    tx: Arc<watch::Sender<Option<bool>>>,
}

impl Default for ScreenLock {
    /// Unlocked (the tests, `kai --tool-list`, a host with no watch).
    fn default() -> Self {
        Self {
            tx: Arc::new(watch::channel(Some(false)).0),
        }
    }
}

impl ScreenLock {
    /// Not known until the watch's first look ([`watch`]), and locked until then: no call runs
    /// before Kai knows the screen is unlocked.
    pub fn unknown() -> Self {
        Self {
            tx: Arc::new(watch::channel(None).0),
        }
    }

    pub fn is_locked(&self) -> bool {
        *self.tx.borrow() != Some(false)
    }

    /// `true` when this changed it (from not known, always). Only the watch writes it.
    pub fn set(&self, locked: bool) -> bool {
        self.tx.send_if_modified(|l| {
            let changed = *l != Some(locked);
            *l = Some(locked);
            changed
        })
    }

    /// Resolves once the screen is known to be unlocked (at once while it is).
    pub async fn unlocked(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|l| *l == Some(false)).await;
    }

    /// Resolves once the screen is locked (at once while it is).
    pub async fn locked(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|l| *l != Some(false)).await;
    }
}

/// What each source said last (`None`: nothing yet, its service is not on the bus, or no such
/// source).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// logind's `LockedHint`.
    pub logind: Option<bool>,
    /// The screen saver service's active state.
    pub saver: Option<bool>,
}

impl Facts {
    /// Locked while either source says so.
    pub fn locked(&self) -> bool {
        self.logind == Some(true) || self.saver == Some(true)
    }

    fn of(&mut self, kind: Kind) -> &mut Option<bool> {
        match kind {
            Kind::Logind => &mut self.logind,
            Kind::Saver => &mut self.saver,
        }
    }

    /// Both sources speak and say different things.
    fn disagree(&self) -> bool {
        matches!((self.logind, self.saver), (Some(a), Some(b)) if a != b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Logind,
    Saver,
}

impl Kind {
    fn other(self) -> Self {
        match self {
            Kind::Logind => Kind::Saver,
            Kind::Saver => Kind::Logind,
        }
    }
}

/// One source: its service followed by name (`NameOwnerChanged`), its fact's signal from the
/// service's current owner.
struct Source {
    kind: Kind,
    /// `None`: no such bus, nothing followed.
    bus: Option<zbus::Connection>,
    /// The bus's `NameOwnerChanged` for the service's name.
    owners: Option<zbus::MessageStream>,
    /// The fact's signal from the current owner.
    events: Option<zbus::MessageStream>,
    /// logind: Kai's session's object path, once found.
    path: Option<String>,
    /// logind: `XDG_SESSION_ID`.
    session_id: Option<String>,
    timeout: Duration,
}

impl Source {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Logind => LOGIND,
            Kind::Saver => SAVER,
        }
    }

    fn what(&self) -> &'static str {
        match self.kind {
            Kind::Logind => "logind's LockedHint",
            Kind::Saver => "the screen saver service's ActiveChanged",
        }
    }

    /// Something is followed: the service's name or its signal.
    fn followed(&self) -> bool {
        self.owners.is_some() || self.events.is_some()
    }

    /// The start: the service's name followed, then its current owner's signal and fact. Bounded
    /// as a whole by `dbus.timeout_s`; what was set up by then stays.
    async fn start(&mut self) -> Option<bool> {
        let bus = self.bus.clone()?;
        let timeout = self.timeout;
        let what = self.what();
        let run = async {
            match owner_changes(&bus, self.name()).await {
                Ok(s) => self.owners = Some(s),
                Err(e) => tracing::warn!(
                    "screen lock: cannot follow {}'s owner ({e}); a restart of it is not seen",
                    self.name()
                ),
            }
            match current_owner(&bus, self.name(), timeout).await {
                Ok(Some(owner)) => self.attach(&owner).await,
                Ok(None) => {
                    tracing::info!(
                        "screen lock: {} is not on the bus; followed once it appears",
                        self.name()
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!("screen lock: cannot follow {what} ({e})");
                    None
                }
            }
        };
        match tokio::time::timeout(timeout, run).await {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!(
                    "screen lock: {what} did not answer within {} s (dbus.timeout_s)",
                    timeout.as_secs()
                );
                None
            }
        }
    }

    /// Follows `owner`'s signal, then reads the fact (in that order: no change between is lost).
    async fn attach(&mut self, owner: &str) -> Option<bool> {
        self.events = None;
        let what = self.what();
        match bounded(what, self.timeout, self.subscribe(owner)).await {
            Ok((s, v)) => {
                self.events = Some(s);
                Some(v)
            }
            Err(e) => {
                tracing::warn!("screen lock: cannot follow {what} ({e})");
                None
            }
        }
    }

    async fn subscribe(&mut self, owner: &str) -> Result<(zbus::MessageStream, bool), String> {
        let bus = self.bus.clone().ok_or("no bus")?;
        match self.kind {
            Kind::Logind => {
                let path = match &self.path {
                    Some(p) => p.clone(),
                    None => {
                        let p = own_session(&bus, self.session_id.as_deref(), self.timeout).await?;
                        tracing::info!(session = p, "screen lock: Kai's logind session");
                        self.path = Some(p.clone());
                        p
                    }
                };
                let s = stream(
                    &bus,
                    owner,
                    "org.freedesktop.DBus.Properties",
                    "PropertiesChanged",
                    Some(&path),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok((s, self.read().await?))
            }
            Kind::Saver => {
                let s = stream(&bus, owner, SAVER, "ActiveChanged", None)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok((s, self.read().await?))
            }
        }
    }

    /// The fact now: `LockedHint` of Kai's session, or `GetActive`.
    async fn read(&self) -> Result<bool, String> {
        let bus = self.bus.as_ref().ok_or("no bus")?;
        match self.kind {
            Kind::Logind => {
                let path = self.path.as_deref().ok_or("Kai's session is not known")?;
                crate::desktop::property(
                    bus,
                    LOGIND,
                    path,
                    "org.freedesktop.login1.Session",
                    "LockedHint",
                    self.timeout,
                )
                .await
                .map_err(|e| e.to_string())
            }
            Kind::Saver => {
                // Plasma serves `/ScreenSaver`; the specification's path is
                // `/org/freedesktop/ScreenSaver`.
                let mut last = String::new();
                for path in ["/ScreenSaver", "/org/freedesktop/ScreenSaver"] {
                    match crate::desktop::call::<_, bool>(
                        bus,
                        SAVER,
                        path,
                        SAVER,
                        "GetActive",
                        &(),
                        self.timeout,
                    )
                    .await
                    {
                        Ok(active) => return Ok(active),
                        Err(e) => last = e.to_string(),
                    }
                }
                Err(last)
            }
        }
    }

    /// The fact a signal of this source carries.
    fn fact(&self, msg: &zbus::Message) -> Option<bool> {
        match self.kind {
            Kind::Logind => locked_hint(msg),
            Kind::Saver => msg.body().deserialize::<bool>().ok(),
        }
    }
}

/// Kai's own logind session's object path: `XDG_SESSION_ID`'s session, else the session of Kai's
/// PID (`GetSessionByPID`).
async fn own_session(
    system: &zbus::Connection,
    id: Option<&str>,
    timeout: Duration,
) -> Result<String, String> {
    async fn manager<B: serde::Serialize + zbus::zvariant::DynamicType>(
        system: &zbus::Connection,
        method: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<String, String> {
        crate::desktop::call::<_, zbus::zvariant::OwnedObjectPath>(
            system,
            LOGIND,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            method,
            body,
            timeout,
        )
        .await
        .map(|p| p.to_string())
        .map_err(|e| e.to_string())
    }
    let mut why = String::new();
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        match manager(system, "GetSession", &(id,), timeout).await {
            Ok(p) => return Ok(p),
            Err(e) => why = format!("XDG_SESSION_ID {id}: {e}; "),
        }
    }
    let pid = std::process::id();
    manager(system, "GetSessionByPID", &(pid,), timeout)
        .await
        .map_err(|e| format!("{why}no logind session holds Kai's PID {pid} ({e})"))
}

/// The bus's `NameOwnerChanged` for `name`.
async fn owner_changes(bus: &zbus::Connection, name: &str) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(DBUS)?
        .interface(DBUS)?
        .member("NameOwnerChanged")?
        .path("/org/freedesktop/DBus")?
        .arg(0, name)?
        .build()
        .to_owned();
    zbus::MessageStream::for_match_rule(rule, bus, None).await
}

/// `name`'s unique owner, `None` while nobody owns it.
async fn current_owner(
    bus: &zbus::Connection,
    name: &str,
    timeout: Duration,
) -> Result<Option<String>, String> {
    if !crate::desktop::has_owner(bus, name, timeout).await? {
        return Ok(None);
    }
    // Gone between the two calls: the owner's change says what comes next.
    Ok(crate::desktop::call::<_, String>(
        bus,
        DBUS,
        "/org/freedesktop/DBus",
        DBUS,
        "GetNameOwner",
        &(name,),
        timeout,
    )
    .await
    .ok())
}

/// A match rule for `member` of `iface` from `owner` (a unique name: signals carry the sender's
/// unique name, as `crate::sleep` pins logind's), on `path` when given.
async fn stream(
    bus: &zbus::Connection,
    owner: &str,
    iface: &str,
    member: &str,
    path: Option<&str>,
) -> zbus::Result<zbus::MessageStream> {
    let mut rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner)?
        .interface(iface)?
        .member(member)?;
    if let Some(p) = path {
        rule = rule.path(p)?;
    }
    zbus::MessageStream::for_match_rule(rule.build().to_owned(), bus, None).await
}

/// The next message of a stream that may be gone (`None` then never comes).
async fn next(s: &mut Option<zbus::MessageStream>) -> Option<zbus::Result<zbus::Message>> {
    match s.as_mut() {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

enum Event {
    /// The service's owner changed (`None`: that stream ended).
    Owner(Kind, Option<zbus::Result<zbus::Message>>),
    /// The fact's signal (`None`: that stream ended).
    Signal(Kind, Option<zbus::Result<zbus::Message>>),
}

/// Follows both sources (module doc) while anything of them can be followed; each change of
/// [`Facts::locked`] sets `screen` (the flag's only writer) and reaches the core as
/// `Command::ScreenLock`. `session` and `system` are the session and system buses (`None`: that
/// source is not followed), `session_id` is `XDG_SESSION_ID`, and `timeout` (`dbus.timeout_s`)
/// bounds each source's start and each look. The first look is published whatever it found.
pub async fn watch(
    cmds: mpsc::UnboundedSender<Command>,
    screen: ScreenLock,
    session: Option<zbus::Connection>,
    system: Option<zbus::Connection>,
    session_id: Option<String>,
    timeout: Duration,
) {
    let source = |kind, bus| Source {
        kind,
        bus,
        owners: None,
        events: None,
        path: None,
        session_id: session_id.clone(),
        timeout,
    };
    let mut logind = source(Kind::Logind, system);
    let mut saver = source(Kind::Saver, session);
    let mut facts = Facts::default();
    (facts.logind, facts.saver) = tokio::join!(logind.start(), saver.start());
    let publish = |facts: &Facts| -> bool {
        let now = facts.locked();
        if screen.set(now) {
            tracing::info!(
                ?facts,
                locked = now,
                "screen lock: the session's lock changed"
            );
            return cmds.send(Command::ScreenLock).is_ok();
        }
        true
    };
    if !logind.followed() && !saver.followed() {
        tracing::warn!(
            "screen lock: neither logind nor the screen saver service can be followed; Kai does \
             not see the screen lock"
        );
        publish(&facts);
        return;
    }
    tracing::info!(?facts, "screen lock: following the session's lock");
    if !publish(&facts) {
        return;
    }
    loop {
        let event = tokio::select! {
            m = next(&mut logind.owners) => Event::Owner(Kind::Logind, m),
            m = next(&mut logind.events) => Event::Signal(Kind::Logind, m),
            m = next(&mut saver.owners) => Event::Owner(Kind::Saver, m),
            m = next(&mut saver.events) => Event::Signal(Kind::Saver, m),
        };
        let kind = match &event {
            Event::Owner(k, _) | Event::Signal(k, _) => *k,
        };
        let src = match kind {
            Kind::Logind => &mut logind,
            Kind::Saver => &mut saver,
        };
        let before = facts;
        match event {
            Event::Owner(_, Some(Ok(msg))) => {
                let Ok((_, _, new)) = msg.body().deserialize::<(String, String, String)>() else {
                    continue;
                };
                if new.is_empty() {
                    tracing::warn!(
                        "screen lock: {} left the bus; it says nothing until it is back",
                        src.name()
                    );
                    src.events = None;
                    *facts.of(kind) = None;
                } else {
                    tracing::info!(
                        owner = new,
                        "screen lock: {} has a new owner; followed and asked again",
                        src.name()
                    );
                    *facts.of(kind) = src.attach(&new).await;
                }
            }
            Event::Owner(_, Some(Err(_))) | Event::Signal(_, Some(Err(_))) => continue,
            Event::Owner(_, None) => {
                tracing::warn!("screen lock: following {}'s owner ended", src.name());
                src.owners = None;
            }
            Event::Signal(_, Some(Ok(msg))) => match src.fact(&msg) {
                Some(v) => *facts.of(kind) = Some(v),
                None => continue,
            },
            Event::Signal(_, None) => {
                tracing::warn!("screen lock: {}'s signals ended", src.what());
                src.events = None;
                *facts.of(kind) = None;
            }
        }
        // The other source may have missed its signal: when the two disagree after this one's
        // change, it is asked again before the lock is published.
        if facts != before && facts.disagree() {
            let other = kind.other();
            let src = match other {
                Kind::Logind => &logind,
                Kind::Saver => &saver,
            };
            match bounded(src.what(), timeout, src.read()).await {
                Ok(v) => {
                    if Some(v) != *facts.of(other) {
                        tracing::info!(
                            "screen lock: {} had changed without its signal",
                            src.what()
                        );
                    }
                    *facts.of(other) = Some(v);
                }
                Err(e) => tracing::warn!("screen lock: asking {} again failed ({e})", src.what()),
            }
        }
        if !publish(&facts) {
            return;
        }
        if !logind.followed() && !saver.followed() {
            tracing::warn!("screen lock: nothing of either source is followed any more");
            return;
        }
    }
}

/// `f`, or why it did not finish within `timeout`.
async fn bounded<T>(
    what: &str,
    timeout: Duration,
    f: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match tokio::time::timeout(timeout, f).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "{what} did not answer within {} s (dbus.timeout_s)",
            timeout.as_secs()
        )),
    }
}

/// `LockedHint` in a `PropertiesChanged` of the session, when it is there.
fn locked_hint(msg: &zbus::Message) -> Option<bool> {
    let (iface, changed, _) = msg
        .body()
        .deserialize::<(
            String,
            std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            Vec<String>,
        )>()
        .ok()?;
    if iface != "org.freedesktop.login1.Session" {
        return None;
    }
    changed
        .get("LockedHint")
        .and_then(|v| bool::try_from(v).ok())
}

#[cfg(test)]
mod tests;
