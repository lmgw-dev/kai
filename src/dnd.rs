//! Do not disturb, followed (WP18: a job's result is not spoken while it is on, but notified). The
//! notification server's `Inhibited` property (`org.freedesktop.Notifications`; Plasma's server has
//! it and announces its changes with `PropertiesChanged`, the freedesktop specification does not
//! have it) is read at start and followed; each change reaches the core as `Command::Dnd`. The
//! owner's do not disturb and Kai's own (`set_dnd`, WP16) both make it true. A server that leaves
//! the bus takes its fact with it (off until one is back); one with no such property is logged
//! once and counts as off. The desktop tools' own reading (`desktop_context`, a question under do
//! not disturb) stays theirs: they ask when they run.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::core::Command;

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Follows `Inhibited` on `bus` (module doc); `timeout` (`dbus.timeout_s`) bounds the start.
pub async fn watch(cmds: mpsc::UnboundedSender<Command>, bus: zbus::Connection, timeout: Duration) {
    let proxy = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(&bus)
        .destination(NOTIFICATIONS)
        .and_then(|b| b.path(PATH))
        .and_then(|b| b.interface(NOTIFICATIONS))
        .map(|b| b.cache_properties(zbus::proxy::CacheProperties::Yes));
    let proxy = match proxy {
        Ok(b) => match tokio::time::timeout(timeout, b.build()).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                tracing::warn!("do not disturb: the notification server cannot be followed: {e}");
                return;
            }
            Err(_) => {
                tracing::warn!(
                    "do not disturb: the notification server did not answer within {} s \
                     (dbus.timeout_s); it is not followed",
                    timeout.as_secs()
                );
                return;
            }
        },
        Err(e) => {
            tracing::warn!("do not disturb: {e}");
            return;
        }
    };
    let mut owners = match proxy.receive_owner_changed().await {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!("do not disturb: the server's owner is not followed: {e}");
            None
        }
    };
    let mut changes = proxy.receive_property_changed::<bool>("Inhibited").await;
    let mut last: Option<bool> = None;
    let mut publish = |on: bool| -> bool {
        if last == Some(on) {
            return true;
        }
        last = Some(on);
        tracing::info!(on, "do not disturb");
        cmds.send(Command::Dnd(on)).is_ok()
    };
    // The start's value: the stream yields the cached value first, but a server with no such
    // property never does.
    match tokio::time::timeout(timeout, proxy.get_property::<bool>("Inhibited")).await {
        Ok(Ok(on)) => {
            if !publish(on) {
                return;
            }
        }
        Ok(Err(e)) => tracing::info!(
            "do not disturb: the notification server has no Inhibited state ({e}); counted as off"
        ),
        Err(_) => tracing::info!("do not disturb: Inhibited did not come within dbus.timeout_s"),
    }
    loop {
        tokio::select! {
            c = changes.next() => {
                let Some(c) = c else { return };
                match c.get().await {
                    Ok(on) => if !publish(on) { return },
                    Err(e) => tracing::debug!("do not disturb: a change did not read: {e}"),
                }
            }
            o = async {
                match owners.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            } => {
                match o {
                    None => owners = None,
                    Some(None) => {
                        tracing::info!("do not disturb: the notification server left the bus");
                        if !publish(false) { return }
                    }
                    Some(Some(owner)) => {
                        tracing::info!(%owner, "do not disturb: a notification server is on the bus");
                        if let Ok(Ok(on)) = tokio::time::timeout(
                            timeout,
                            proxy.get_property::<bool>("Inhibited"),
                        ).await && !publish(on) {
                            return;
                        }
                    }
                }
            }
        }
    }
}
