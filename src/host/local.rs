//! Kai's own calls of its hosted tools, not through lmgw (K28): the routines' CLI (`kai tool`), a
//! routine's steps and goal, the tray's and a key's start of a routine. Each goes the one path
//! lmgw's calls go ([`Shared::dispatch`]): K15's policy for its caller, the tool's preparation, the
//! owner's confirmation (PD4) and the handler. There is no approver (F is lmgw's) and no deadline
//! or message limit of lmgw's: the caller's cancellation ends the call. A caller with a bound of
//! its own gives it ([`LocalBounds`]: the tray's notification room, the CLI's stdout stream).

use std::sync::{Arc, Weak};

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::limits::LinkLimits;
use super::policy::Caller;
use super::registry::{Args, Entry, Out, Reply};
use super::server::{Bounds, Listed, Shared, Stop};

/// What a call of Kai's own is bounded by besides its cancellation.
pub struct LocalBounds {
    pub cancel: CancellationToken,
    /// The bytes its result may take, and how a refusal names that bound (`Call::room`).
    pub room: Option<(u64, String)>,
    /// Where a script routine streams its stdout (`Call::out`).
    pub out: Option<Out>,
}

/// The host, for Kai's own calls. Weak: a provider holds it, and the host holds the providers.
#[derive(Clone)]
pub struct Local(Weak<Shared>);

impl Local {
    pub fn new(shared: &Arc<Shared>) -> Self {
        Self(Arc::downgrade(shared))
    }

    fn shared(&self) -> Option<Arc<Shared>> {
        self.0.upgrade()
    }

    /// The tools offered now, as lmgw would list them (the policy's `off` and K30's gate applied),
    /// read afresh from the providers.
    pub fn entries(&self) -> Vec<Entry> {
        let Some(s) = self.shared() else {
            return Vec::new();
        };
        let policy = s.policy.read().expect("never poisoned").clone();
        Listed::build(&s.registry, &policy).entries
    }

    /// `name`'s entry whether it is listed or not (the tray's switch reaches `routine_enable` with
    /// the configuration tools off); the policy still decides its calls.
    pub fn entry_any(&self, name: &str) -> Option<Entry> {
        self.shared()?
            .registry
            .entries()
            .into_iter()
            .find(|e| e.spec.name == name)
    }

    /// `f` on the host's provider of type `T` (the routines' runtime finds its provider so).
    pub fn with_provider<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.shared()?.registry.find::<T>().map(f)
    }

    /// Sets the limits as a link would (the tests').
    #[cfg(test)]
    pub fn set_limits(&self, limits: LinkLimits) {
        if let Some(s) = self.shared() {
            *s.limits.write().expect("never poisoned") = limits;
        }
    }

    /// The host link's limits as last known (`Shared::limits`).
    pub fn limits(&self) -> Option<LinkLimits> {
        Some(*self.shared()?.limits.read().expect("never poisoned"))
    }

    /// Calls `entry` with the arguments `raw` (checked against its parameters) as `caller`.
    /// Every way it ends is a reply: a refusal or a cancellation is an error the caller reads.
    pub async fn call(
        &self,
        entry: &Entry,
        raw: Option<Map<String, Value>>,
        caller: &Caller,
        cancel: CancellationToken,
    ) -> Reply {
        let bounds = LocalBounds {
            cancel,
            room: None,
            out: None,
        };
        self.call_with(entry, raw, caller, bounds).await
    }

    /// [`Local::call`] with `bounds`.
    pub async fn call_with(
        &self,
        entry: &Entry,
        raw: Option<Map<String, Value>>,
        caller: &Caller,
        bounds: LocalBounds,
    ) -> Reply {
        self.call_approved(entry, raw, caller, None, bounds).await
    }

    /// The tests' way to a call that a thread approved (lmgw's F), as `server.rs` makes one.
    #[cfg(test)]
    pub async fn call_approved_by(
        &self,
        entry: &Entry,
        raw: Option<Map<String, Value>>,
        caller: &Caller,
        approver: &Caller,
    ) -> Reply {
        let bounds = LocalBounds {
            cancel: CancellationToken::new(),
            room: None,
            out: None,
        };
        self.call_approved(entry, raw, caller, Some(approver), bounds)
            .await
    }

    /// A call that `approver` approved (lmgw's F in a thread; the owner's own tray or key for a
    /// routine's run), decided as `server.rs` decides one.
    pub async fn call_approved(
        &self,
        entry: &Entry,
        raw: Option<Map<String, Value>>,
        caller: &Caller,
        approver: Option<&Caller>,
        bounds: LocalBounds,
    ) -> Reply {
        let LocalBounds { cancel, room, out } = bounds;
        let Some(s) = self.shared() else {
            return Reply::error("Kai's tool host is gone (Kai is quitting)");
        };
        let tool = entry.spec.name;
        tracing::info!(tool, caller = %caller.describe(), "host: a call of Kai's own (K28)");
        let args = match Args::read(entry.spec, raw) {
            Ok(a) => a,
            Err(why) => {
                tracing::info!(
                    tool,
                    "host: refused: the arguments do not fit the tool's parameters"
                );
                return Reply::error(why);
            }
        };
        let (room, room_says) = match room {
            Some((r, says)) => (Some(r), says),
            None => (None, String::new()),
        };
        let bounds = Bounds {
            cancel,
            deadline: None,
            timeout_ms: None,
            room,
            room_says,
            out,
        };
        match s.dispatch(entry, args, caller, approver, bounds).await {
            Ok(reply) => reply,
            Err(Stop::Refused(why)) => Reply::error(why),
            Err(Stop::Cancelled(when)) => {
                tracing::info!(tool, caller = %caller.describe(), "host: the call was cancelled {when}");
                Reply::error(format!("the call of {tool} was cancelled"))
            }
        }
    }
}
