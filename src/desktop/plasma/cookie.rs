//! The cookie of an inhibition Kai holds on a Plasma service (do not disturb, the Night Light
//! pause). The service drops the inhibition with Kai's connection, so the cookie is the only way
//! to end it earlier, and it must not get lost.
//!
//! Both calls run in a task of their own that owns the slot while it works. The caller waits at
//! most `dbus.timeout_s` for it, and if the caller is dropped (lmgw's `notifications/cancelled`)
//! or gives up after the server acted, the task still stores the cookie the reply carries (or
//! keeps the one a failed release did not free). The next call waits for the slot, so it sees the
//! outcome of the one before it.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::Failure;
use tokio::sync::Mutex;

/// The bound of a call inside the task: none, the caller's wait is the bound.
pub const UNBOUNDED: Duration = Duration::MAX;

pub struct Cookie {
    slot: Arc<Mutex<Option<u32>>>,
    timeout: Duration,
}

impl Cookie {
    pub fn new(timeout: Duration) -> Self {
        Self {
            slot: Arc::default(),
            timeout,
        }
    }

    fn late(&self, what: &str) -> Failure {
        Failure::Failed(format!(
            "{what} did not finish within {} s (dbus.timeout_s); if the service answers late, \
             Kai still keeps the cookie it gets",
            self.timeout.as_secs()
        ))
    }

    /// Takes the inhibition unless Kai holds one: `inhibit` asks the service and returns the
    /// cookie.
    pub async fn hold<F>(&self, inhibit: F) -> Result<(), Failure>
    where
        F: Future<Output = Result<u32, Failure>> + Send + 'static,
    {
        let mut slot = tokio::time::timeout(self.timeout, self.slot.clone().lock_owned())
            .await
            .map_err(|_| self.late("the previous inhibition call"))?;
        let task = tokio::spawn(async move {
            if slot.is_none() {
                *slot = Some(inhibit.await?);
            }
            Ok(())
        });
        self.finish(task, "the inhibition").await
    }

    /// Ends the inhibition Kai holds: `release` is given the cookie and asks the service. The
    /// cookie is dropped only after a release that succeeded. `false`: Kai held none.
    pub async fn release<R, F>(&self, release: R) -> Result<bool, Failure>
    where
        R: FnOnce(u32) -> F + Send + 'static,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let mut slot = tokio::time::timeout(self.timeout, self.slot.clone().lock_owned())
            .await
            .map_err(|_| self.late("the previous inhibition call"))?;
        let task = tokio::spawn(async move {
            let Some(c) = *slot else { return Ok(false) };
            release(c).await?;
            *slot = None;
            Ok(true)
        });
        self.finish(task, "the release").await
    }

    async fn finish<T>(
        &self,
        task: tokio::task::JoinHandle<Result<T, Failure>>,
        what: &str,
    ) -> Result<T, Failure> {
        match tokio::time::timeout(self.timeout, task).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => Err(Failure::Failed(format!("{what} stopped: {e}"))),
            Err(_) => Err(self.late(what)),
        }
    }
}
