//! Kai's accessibility switch (the owner's rule, 2026-10-09): `org.a11y.Status` `IsEnabled` on
//! while conversation mode is active, and in every other mode only around a read (switched on, the
//! window's app waited for, read, the earlier value put back). `ScreenReaderEnabled` is never
//! written: in testing it exposed no further tree and only added memory use (WP13 (a)).
//!
//! - **The rule** ([`Rule`], pure): Kai wants the switch on while `accessibility.switch` allows it
//!   (`conversation`, the default: conversation mode and reads; `read`: reads only; `never`) and
//!   conversation mode is on or a read holds it ([`Hold`]), and never while the screen is locked
//!   or Kai quits. Each stretch of wanting is one period: at its start Kai reads the switch; off,
//!   Kai switches it on and owns that change ([`Held::On`]); on already, it is not Kai's and stays
//!   as it is after ([`Held::NotKais`]). At the period's end Kai switches off only what it owns.
//! - **The owner's change wins.** Kai follows `PropertiesChanged` of `org.a11y.Status`: the switch
//!   going off while Kai owns it ends Kai's ownership (Kai does not switch it on again in that
//!   period, nor off at its end). A switch going off and on again between two of Kai's looks is
//!   seen by the signal; without the signal (a launcher that emits none), Kai switches off only a
//!   switch that still reads on. The signal of Kai's own write can come late (the launcher
//!   announces its GSettings key's change, after the write): each write of Kai's absorbs the first
//!   signal of its value within `dbus.timeout_s` ([`Rule::observed`]), so the late "off" of one
//!   read's release does not end the next read's ownership.
//! - **Cancellation.** Every settle runs in a task of its own, which a dropped waiter (a cancelled
//!   call, the lock stopping it, a deadline) does not stop: a switch-on is always followed by its
//!   ownership and marker, a switch-off by the marker's removal. A read's [`Hold`] exists before
//!   the read counts, so a read dropped while the switch settles still ends.
//! - **Waiting for the apps** ([`A11ySwitch::registration_deadline`]): an app joins the
//!   accessibility bus when it sees the switch go on (Qt's bridge embeds itself in the registry,
//!   which announces it with `ChildrenChanged` on its root). A read whose app is not on the bus
//!   yet waits for that announcement, at most `accessibility.wait_ms` from the moment Kai switched
//!   the switch on (default 5000: long enough for the apps seen in testing to join).
//!   Not a sleep: the read goes on at the app's announcement (`crate::desktop::fdo::atspi`).
//! - **Restore** on conversation mode's end, on a read's end, when the screen locks, and when Kai
//!   quits (SIGTERM and SIGINT quit through the core). A crash or SIGKILL cannot restore: the
//!   switch is persistent (the launcher keeps it in GSettings), so while Kai owns it a marker file
//!   (`$XDG_STATE_HOME/kai/a11y-switched-on`) says so, and the next start switches a switch that
//!   still reads on off again ([`A11ySwitch::recover`]). What happened to the switch between the
//!   crash and that start cannot be told apart from Kai's change.
//!
//! Every D-Bus call is bounded by `dbus.timeout_s`; a failure is logged and said in a read's error
//! ([`A11ySwitch::words`]), never retried in a loop.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use zbus::zvariant::{OwnedValue, Value};

use super::A11Y_BUS;
use crate::desktop::{call, property};
use crate::host::registry::SessionBus;

pub const STATUS: &str = "org.a11y.Status";
const PATH: &str = "/org/a11y/bus";
const IS_ENABLED: &str = "IsEnabled";

/// The marker's file name in Kai's state directory.
pub const MARKER: &str = "a11y-switched-on";

/// `accessibility.switch`: when Kai may switch `IsEnabled` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchMode {
    /// While conversation mode is active, and around each read (the owner's rule).
    #[default]
    Conversation,
    /// Only around each read.
    Read,
    /// Never: the switch is the owner's alone.
    Never,
}

impl SwitchMode {
    pub fn key(self) -> &'static str {
        match self {
            SwitchMode::Conversation => "conversation",
            SwitchMode::Read => "read",
            SwitchMode::Never => "never",
        }
    }
}

/// What Kai holds of the switch in the current period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Kai switched nothing (no period, or the switch could not be read or written yet).
    Nothing,
    /// Kai switched `IsEnabled` on from off at `at`: Kai switches it off at the period's end.
    On { at: Instant },
    /// The switch is not Kai's in this period: it was on already, or it was switched off while
    /// Kai owned it. Kai leaves it as it is.
    NotKais,
}

/// The pure rule (module doc).
#[derive(Debug, Clone)]
pub struct Rule {
    pub mode: SwitchMode,
    conversation: bool,
    reads: usize,
    locked: bool,
    closed: bool,
    pub held: Held,
    /// Why the switch is not on in Kai's hands, for a read's error.
    pub why_not: Option<String>,
    /// Kai's own writes whose `PropertiesChanged` has not come yet: the value and when it was
    /// written. Each absorbs one signal of its value ([`Rule::observed`]).
    pending: Vec<(bool, Instant)>,
}

/// What a settle does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Read the switch and switch it on if it is off.
    Acquire,
    /// Switch it off if it still reads on (Kai owns it).
    Release,
    /// The period ended with nothing of Kai's to put back.
    Forget,
}

impl Rule {
    pub fn new(mode: SwitchMode) -> Self {
        Self {
            mode,
            conversation: false,
            reads: 0,
            locked: false,
            closed: false,
            held: Held::Nothing,
            why_not: None,
            pending: Vec::new(),
        }
    }

    /// Kai wants the switch on now.
    pub fn wants(&self) -> bool {
        if self.locked || self.closed {
            return false;
        }
        match self.mode {
            SwitchMode::Never => false,
            SwitchMode::Read => self.reads > 0,
            SwitchMode::Conversation => self.conversation || self.reads > 0,
        }
    }

    pub fn step(&self) -> Step {
        match (self.wants(), self.held) {
            (true, Held::Nothing) => Step::Acquire,
            (false, Held::On { .. }) => Step::Release,
            (false, Held::NotKais) => Step::Forget,
            _ => Step::Nothing,
        }
    }

    /// The switch was read as `on` at a period's start: Kai switches it on when it is off.
    /// `true`: switch it on.
    pub fn acquired(&mut self, on: bool) -> bool {
        if on {
            self.held = Held::NotKais;
            false
        } else {
            true
        }
    }

    /// Kai is about to write `on` at `at`: its signal is Kai's own, not the owner's. Writes older
    /// than `window` are forgotten (a launcher that emits no signal leaves nothing to absorb).
    pub fn wrote(&mut self, on: bool, at: Instant, window: Duration) {
        self.pending
            .retain(|(_, w)| at.saturating_duration_since(*w) <= window);
        self.pending.push((on, at));
    }

    /// `PropertiesChanged` said the switch is `on` now (at `now`). `true`: Kai's ownership ended
    /// (someone else switched it off while Kai owned it).
    ///
    /// The launcher's signal can come late (it follows its GSettings key, whose change is
    /// announced after the write): an "off" of Kai's own release can arrive after the next
    /// period's switch-on. So each of Kai's writes absorbs the first signal of its value that
    /// comes within `window` (`dbus.timeout_s`) of the write; only a signal no write of Kai's
    /// accounts for is someone else's.
    pub fn observed(&mut self, on: bool, now: Instant, window: Duration) -> bool {
        self.pending
            .retain(|(_, at)| now.saturating_duration_since(*at) <= window);
        if let Some(i) = self.pending.iter().position(|(v, _)| *v == on) {
            self.pending.remove(i);
            return false;
        }
        if !on && matches!(self.held, Held::On { .. }) {
            self.held = Held::NotKais;
            self.why_not = Some(
                "it was switched off while Kai held it on, and Kai leaves it off until conversation \
                 mode and the read end"
                    .into(),
            );
            return true;
        }
        false
    }

    pub fn set_conversation(&mut self, on: bool) {
        self.conversation = on;
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn read_began(&mut self) {
        self.reads += 1;
    }

    pub fn read_ended(&mut self) {
        self.reads = self.reads.saturating_sub(1);
    }

    /// How many reads hold the switch.
    #[cfg(test)]
    pub fn reads(&self) -> usize {
        self.reads
    }
}

struct Inner {
    /// `None`: an inert switch (the tests' providers, `kai --tool-list`), which does nothing.
    bus: Option<SessionBus>,
    timeout: Duration,
    /// `accessibility.wait_ms`.
    wait: Duration,
    /// The marker file; `None`: none is kept.
    marker: Option<PathBuf>,
    rule: Mutex<Rule>,
    /// One settle at a time.
    turn: tokio::sync::Mutex<()>,
    /// The watch of `PropertiesChanged` runs.
    watching: std::sync::atomic::AtomicBool,
    /// Every value Kai wrote, in order (the tests' look: `ScreenReaderEnabled` never written).
    #[cfg(test)]
    written: Mutex<Vec<(&'static str, bool)>>,
}

/// The switch (module doc); cloned handles share it.
#[derive(Clone)]
pub struct A11ySwitch {
    inner: Arc<Inner>,
}

/// What [`A11ySwitch`] needs from `[accessibility]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchSettings {
    pub mode: SwitchMode,
    /// `accessibility.wait_ms`.
    pub wait: Duration,
}

impl A11ySwitch {
    pub fn new(
        bus: SessionBus,
        timeout: Duration,
        settings: SwitchSettings,
        marker: Option<PathBuf>,
    ) -> Self {
        Self::build(Some(bus), timeout, settings, marker)
    }

    /// One that switches nothing.
    pub fn inert() -> Self {
        Self::build(
            None,
            Duration::from_secs(1),
            SwitchSettings {
                mode: SwitchMode::Never,
                wait: Duration::ZERO,
            },
            None,
        )
    }

    fn build(
        bus: Option<SessionBus>,
        timeout: Duration,
        settings: SwitchSettings,
        marker: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                bus,
                timeout,
                wait: settings.wait,
                marker,
                rule: Mutex::new(Rule::new(settings.mode)),
                turn: tokio::sync::Mutex::new(()),
                watching: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                written: Mutex::default(),
            }),
        }
    }

    fn rule(&self) -> std::sync::MutexGuard<'_, Rule> {
        self.inner.rule.lock().expect("never poisoned")
    }

    /// What Kai holds now.
    #[cfg(test)]
    pub fn held(&self) -> Held {
        self.rule().held
    }

    /// How many reads hold the switch.
    #[cfg(test)]
    pub fn reads(&self) -> usize {
        self.rule().reads()
    }

    /// Every value Kai wrote, in order.
    #[cfg(test)]
    pub fn written(&self) -> Vec<(&'static str, bool)> {
        self.inner.written.lock().unwrap().clone()
    }

    /// Until when a read waits for its app to join the bus: `accessibility.wait_ms` from the moment
    /// Kai switched the switch on; `None` when Kai did not switch it on (nothing to wait for).
    pub fn registration_deadline(&self) -> Option<Instant> {
        match self.rule().held {
            Held::On { at } => at.checked_add(self.inner.wait),
            _ => None,
        }
    }

    /// Conversation mode went on or off (the core). The returned task settles the switch.
    pub fn set_conversation(&self, on: bool) -> Option<tokio::task::JoinHandle<()>> {
        self.rule().set_conversation(on);
        self.spawn_settle()
    }

    /// The screen locked or unlocked (the core): while locked Kai wants nothing on.
    pub fn set_locked(&self, locked: bool) -> Option<tokio::task::JoinHandle<()>> {
        self.rule().set_locked(locked);
        self.spawn_settle()
    }

    /// Kai quits: the switch is put back, and nothing switches it on again.
    pub async fn shutdown(&self) {
        self.rule().close();
        self.settled().await;
    }

    /// A read begins: the switch goes on (by the rule) before this returns. The read ends when
    /// the hold is released or dropped, also when this future is dropped before it returns (a
    /// cancelled call, the lock stopping it, a deadline): the hold exists before the read counts.
    pub async fn hold(&self) -> Hold {
        let hold = Hold {
            switch: Some(self.clone()),
        };
        self.rule().read_began();
        self.settled().await;
        hold
    }

    fn spawn_settle(&self) -> Option<tokio::task::JoinHandle<()>> {
        let me = self.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => Some(rt.spawn(async move { me.settle().await })),
            Err(_) => None,
        }
    }

    /// A settle in a task of its own, waited for: dropping the waiter leaves the settle running to
    /// its end, so a switch-on is always followed by its ownership (and its marker), and a
    /// switch-off by the marker's removal; nothing is left half done (the module doc).
    async fn settled(&self) {
        if self.inner.bus.is_none() {
            return;
        }
        if let Some(task) = self.spawn_settle() {
            let _ = task.await;
        }
    }

    /// The rule's step carried out (one settle at a time). Only in a task of its own
    /// ([`Self::settled`], [`Self::spawn_settle`]): dropped half way it could leave a switch on
    /// that nothing owns.
    async fn settle(&self) {
        let Some(bus) = self.inner.bus.clone() else {
            return;
        };
        let _turn = self.inner.turn.lock().await;
        let step = self.rule().step();
        match step {
            Step::Nothing => {}
            Step::Forget => {
                self.rule().held = Held::Nothing;
            }
            Step::Acquire => self.acquire(&bus).await,
            Step::Release => self.release(&bus).await,
        }
    }

    async fn acquire(&self, bus: &SessionBus) {
        let conn = match bus.get().await {
            Ok(c) => c,
            Err(e) => return self.failed(format!("no session bus ({e})")),
        };
        let on = match self.read(&conn).await {
            Ok(on) => on,
            Err(e) => return self.failed(format!("reading {STATUS} {IS_ENABLED} failed: {e}")),
        };
        if !self.rule().acquired(on) {
            tracing::info!(
                "accessibility: org.a11y.Status IsEnabled is on already; Kai leaves it as it is"
            );
            return;
        }
        self.watch(&conn).await;
        self.mark(true);
        match self.write(&conn, true).await {
            Ok(()) => {
                let mut rule = self.rule();
                rule.held = Held::On { at: Instant::now() };
                rule.why_not = None;
                tracing::info!(
                    mode = rule.mode.key(),
                    "accessibility: Kai switched org.a11y.Status IsEnabled on (accessibility.switch)"
                );
            }
            Err(e) => {
                self.mark(false);
                self.failed(format!("switching {STATUS} {IS_ENABLED} on failed: {e}"));
            }
        }
    }

    async fn release(&self, bus: &SessionBus) {
        // Ownership ends before the write, so the write's own signal is not taken for the owner's.
        self.rule().held = Held::Nothing;
        let conn = match bus.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "accessibility: cannot put org.a11y.Status IsEnabled back: no session bus \
                     ({e}); the next start does (marker kept)"
                );
                return;
            }
        };
        match self.read(&conn).await {
            Ok(false) => {
                tracing::info!(
                    "accessibility: org.a11y.Status IsEnabled is off already; nothing to put back"
                );
                self.mark(false);
            }
            Ok(true) => match self.write(&conn, false).await {
                Ok(()) => {
                    tracing::info!(
                        "accessibility: Kai switched org.a11y.Status IsEnabled off again"
                    );
                    self.mark(false);
                }
                Err(e) => tracing::warn!(
                    "accessibility: switching org.a11y.Status IsEnabled off again failed: {e}; the \
                     next start tries again (marker kept)"
                ),
            },
            Err(e) => tracing::warn!(
                "accessibility: reading org.a11y.Status IsEnabled failed: {e}; it was not put back; \
                 the next start tries again (marker kept)"
            ),
        }
    }

    fn failed(&self, why: String) {
        tracing::warn!("accessibility: {why}");
        self.rule().why_not = Some(why);
    }

    async fn read(&self, conn: &zbus::Connection) -> Result<bool, String> {
        property::<bool>(conn, A11Y_BUS, PATH, STATUS, IS_ENABLED, self.inner.timeout)
            .await
            .map_err(|e| e.to_string())
    }

    /// The one write Kai makes: `IsEnabled`, never `ScreenReaderEnabled`.
    async fn write(&self, conn: &zbus::Connection, on: bool) -> Result<(), String> {
        #[cfg(test)]
        self.inner.written.lock().unwrap().push((IS_ENABLED, on));
        // Before the call: its signal can come before the reply.
        self.rule().wrote(on, Instant::now(), self.inner.timeout);
        call::<_, ()>(
            conn,
            A11Y_BUS,
            PATH,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(STATUS, IS_ENABLED, Value::from(on)),
            self.inner.timeout,
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// The marker says Kai owns a switch that is on (`true`), or nothing (`false`).
    fn mark(&self, on: bool) {
        let Some(path) = &self.inner.marker else {
            return;
        };
        let r = if on {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::write(
                        path,
                        "Kai switched org.a11y.Status IsEnabled on; it switches it off again\n",
                    )
                })
        } else {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                r => r,
            }
        };
        if let Err(e) = r {
            tracing::warn!("accessibility: the marker {}: {e}", path.display());
        }
    }

    /// At start: a marker left by a Kai that could not put the switch back (a crash, SIGKILL) means
    /// the switch is Kai's; one that still reads on is switched off.
    pub async fn recover(&self) {
        let (Some(bus), Some(path)) = (self.inner.bus.clone(), self.inner.marker.clone()) else {
            return;
        };
        if !path.exists() {
            return;
        }
        let _turn = self.inner.turn.lock().await;
        let conn = match bus.get().await {
            Ok(c) => c,
            Err(e) => {
                return tracing::warn!(
                    "accessibility: a former Kai left org.a11y.Status IsEnabled switched on, and \
                     there is no session bus to put it back ({e})"
                );
            }
        };
        match self.read(&conn).await {
            Ok(true) => match self.write(&conn, false).await {
                Ok(()) => {
                    tracing::info!(
                        "accessibility: a former Kai ended without putting org.a11y.Status \
                         IsEnabled back (its marker was left); Kai switched it off"
                    );
                    self.mark(false);
                }
                Err(e) => tracing::warn!(
                    "accessibility: a former Kai left org.a11y.Status IsEnabled on; switching it \
                     off failed: {e}"
                ),
            },
            Ok(false) => self.mark(false),
            Err(e) => tracing::warn!(
                "accessibility: a former Kai left a marker; reading org.a11y.Status IsEnabled \
                 failed: {e}"
            ),
        }
    }

    /// Follows `PropertiesChanged` of `org.a11y.Status` (started at Kai's first switch-on, once):
    /// the owner's change wins (module doc).
    async fn watch(&self, conn: &zbus::Connection) {
        use std::sync::atomic::Ordering;
        if self.inner.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let stream = tokio::time::timeout(self.inner.timeout, async {
            // Signals carry the sender's unique name: the rule pins the launcher's.
            let owner = zbus::fdo::DBusProxy::new(conn)
                .await?
                .get_name_owner(zbus::names::BusName::try_from(A11Y_BUS)?)
                .await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(owner.as_str())?
                .interface("org.freedesktop.DBus.Properties")?
                .member("PropertiesChanged")?
                .path(PATH)?
                .build()
                .to_owned();
            zbus::MessageStream::for_match_rule(rule, conn, None).await
        })
        .await;
        let mut stream = match stream {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                self.inner.watching.store(false, Ordering::SeqCst);
                return tracing::warn!(
                    "accessibility: cannot follow org.a11y.Status's changes ({e}); Kai switches \
                     off only a switch that still reads on"
                );
            }
            Err(_) => {
                self.inner.watching.store(false, Ordering::SeqCst);
                return tracing::warn!(
                    "accessibility: following org.a11y.Status's changes did not start within {} s \
                     (dbus.timeout_s)",
                    self.inner.timeout.as_secs()
                );
            }
        };
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Some(inner) = weak.upgrade() else { return };
                let Ok(msg) = msg else { continue };
                let Ok((iface, changed, _)) = msg.body().deserialize::<(
                    String,
                    std::collections::HashMap<String, OwnedValue>,
                    Vec<String>,
                )>() else {
                    continue;
                };
                if iface != STATUS {
                    continue;
                }
                let Some(on) = changed.get(IS_ENABLED).and_then(|v| bool::try_from(v).ok()) else {
                    continue;
                };
                let me = A11ySwitch { inner };
                let window = me.inner.timeout;
                if me.rule().observed(on, Instant::now(), window) {
                    tracing::info!(
                        "accessibility: org.a11y.Status IsEnabled was switched off by someone \
                         else while Kai held it on; Kai leaves it off"
                    );
                    me.mark(false);
                }
            }
        });
    }

    /// The words a read's error says about the switch when the window's app is not on the bus.
    pub fn words(&self) -> SwitchWords {
        let rule = self.rule();
        SwitchWords {
            mode: rule.mode,
            held: rule.held,
            why_not: rule.why_not.clone(),
            wait: self.inner.wait,
            inert: self.inner.bus.is_none(),
        }
    }
}

/// What a read's error says about Kai's switch.
#[derive(Debug, Clone)]
pub struct SwitchWords {
    pub mode: SwitchMode,
    pub held: Held,
    pub why_not: Option<String>,
    pub wait: Duration,
    inert: bool,
}

impl SwitchWords {
    /// With the switch off: why Kai did not switch it on.
    pub fn off(&self) -> String {
        if self.inert || self.mode == SwitchMode::Never {
            return "which Kai does not switch (accessibility.switch = \"never\")".into();
        }
        match &self.why_not {
            Some(why) => format!(
                "which Kai switches on around a read (accessibility.switch = \"{}\"), but {why}",
                self.mode.key()
            ),
            None => format!(
                "which Kai switches on around a read (accessibility.switch = \"{}\")",
                self.mode.key()
            ),
        }
    }

    /// With the switch on: whether Kai switched it and waited.
    pub fn on(&self) -> Option<String> {
        matches!(self.held, Held::On { .. }).then(|| {
            format!(
                "Kai switched it on and the app did not join within {} ms (accessibility.wait_ms)",
                self.wait.as_millis()
            )
        })
    }
}

/// A read's hold on the switch (module doc): released by [`Hold::release`], or on drop.
pub struct Hold {
    switch: Option<A11ySwitch>,
}

impl Hold {
    /// The read ended: the switch settles before this returns.
    pub async fn release(mut self) {
        if let Some(s) = self.switch.take() {
            s.rule().read_ended();
            s.settled().await;
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(s) = self.switch.take() {
            s.rule().read_ended();
            s.spawn_settle();
        }
    }
}

impl std::fmt::Debug for Hold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hold")
    }
}
