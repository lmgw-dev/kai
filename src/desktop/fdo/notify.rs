//! The freedesktop notification server (`org.freedesktop.Notifications` on the session bus): the
//! owner-facing `notify` and `ask` of WP16b. The freedesktop specification has no do-not-disturb
//! state, so this provider says so ([`FdoNotify`]'s `dnd`); the `plasma` provider has one and uses
//! [`show`] and [`ask`] for the rest.
//!
//! `ask` shows one button per choice (the action keys are the choices' positions, so a label
//! never has to be unique or well-formed) and waits for the server's `ActionInvoked` or
//! `NotificationClosed` for that notification, from the server that showed it: any peer of the
//! session can send those signals. The wait has no bound of its own (the owner may answer at any
//! time; lmgw's cancel or timeout drops the call), and dropping it closes the notification, so a
//! question nobody waits for does not stay on screen. The wait ends with an error when the server
//! that showed the question leaves the bus (Plasma's shell restarting): its notifications are gone
//! with it, and nobody could answer.
//!
//! Under do not disturb a question goes out with urgency 2 (critical), which Plasma shows through
//! it; the answer says so. Plain `notify` stays as quiet as the owner set it.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use kai_desktop::caps::notify::{Asked, Dnd, Notify};
use kai_desktop::caps::{Failure, Fut};
use zbus::zvariant::Value;

use crate::host::registry::SessionBus;

const SERVER: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct FdoNotify {
    pub bus: SessionBus,
    pub timeout: Duration,
}

impl Notify for FdoNotify {
    fn can_dnd(&self) -> bool {
        false
    }

    fn dnd(&self) -> Fut<'_, bool> {
        Box::pin(async {
            Err(Failure::Refused(
                "the freedesktop notification server has no do-not-disturb state".into(),
            ))
        })
    }

    fn set_dnd(&self, _on: bool) -> Fut<'_, Dnd> {
        Box::pin(async {
            Err(Failure::Refused(
                "the freedesktop notification server has no do-not-disturb state".into(),
            ))
        })
    }

    fn show<'a>(&'a self, title: &'a str, body: &'a str) -> Fut<'a, ()> {
        Box::pin(show(&self.bus, title, body, self.timeout))
    }

    fn ask<'a>(&'a self, question: &'a str, choices: &'a [String]) -> Fut<'a, Asked> {
        Box::pin(ask(&self.bus, question, choices, false, self.timeout))
    }
}

/// The notification the server showed.
struct Shown {
    id: u32,
    /// The server's unique bus name: the sender its signals must come from.
    server: String,
}

async fn send(
    conn: &zbus::Connection,
    title: &str,
    body: &str,
    actions: &[String],
    expire_ms: i32,
    critical: bool,
    timeout: Duration,
) -> Result<Shown, Failure> {
    let mut hints: HashMap<&str, Value<'_>> =
        HashMap::from([("desktop-entry", Value::from(crate::APP_ID))]);
    if critical {
        hints.insert("urgency", Value::U8(2));
    }
    let body = crate::tray::escape(body);
    let args = (
        "Kai",
        0u32,
        crate::APP_ID,
        title,
        body.as_str(),
        actions,
        hints,
        expire_ms,
    );
    let call = conn.call_method(Some(SERVER), PATH, Some(SERVER), "Notify", &args);
    let reply = match tokio::time::timeout(timeout, call).await {
        Ok(Ok(r)) => r,
        Ok(Err(zbus::Error::MethodError(name, _, _)))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            return Err(Failure::Failed("no notification server is running".into()));
        }
        Ok(Err(e)) => {
            return Err(Failure::Failed(format!(
                "the notification server refused it: {e}"
            )));
        }
        Err(_) => {
            return Err(Failure::Failed(format!(
                "the notification server did not answer within {} s (dbus.timeout_s)",
                timeout.as_secs()
            )));
        }
    };
    let server = reply.header().sender().map(|s| s.to_string());
    match (reply.body().deserialize::<u32>(), server) {
        (Ok(id), Some(server)) => Ok(Shown { id, server }),
        _ => Err(Failure::Failed(
            "the notification server's answer named no id".into(),
        )),
    }
}

/// Shows a plain notification.
pub async fn show(
    bus: &SessionBus,
    title: &str,
    body: &str,
    timeout: Duration,
) -> Result<(), Failure> {
    let conn = bus.get().await.map_err(Failure::Failed)?;
    send(&conn, title, body, &[], -1, false, timeout)
        .await
        .map(drop)
}

/// Closes the notification when the wait for its answer ends any way but an answer.
struct CloseOnDrop {
    conn: zbus::Connection,
    id: u32,
    armed: bool,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (conn, id) = (self.conn.clone(), self.id);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = conn
                    .call_method(
                        Some(SERVER),
                        PATH,
                        Some(SERVER),
                        "CloseNotification",
                        &(id,),
                    )
                    .await;
            });
        }
    }
}

/// Shows `question` with a button per choice and waits: the choice's position, or `None` when the
/// notification closed without one. `under_dnd`: do not disturb is on, so the question is critical.
pub async fn ask(
    bus: &SessionBus,
    question: &str,
    choices: &[String],
    under_dnd: bool,
    timeout: Duration,
) -> Result<Asked, Failure> {
    let conn = bus.get().await.map_err(Failure::Failed)?;
    // The signals are followed before the notification is sent, so none is missed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SERVER)
        .and_then(|b| b.path(PATH))
        .map_err(|e| Failure::Failed(format!("cannot follow the notification server: {e}")))?
        .build()
        .to_owned();
    let mut signals = match tokio::time::timeout(
        timeout,
        zbus::MessageStream::for_match_rule(rule, &conn, None),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err(Failure::Failed(format!(
                "cannot follow the notification server: {e}"
            )));
        }
        Err(_) => {
            return Err(Failure::Failed(format!(
                "the session bus did not answer within {} s (dbus.timeout_s)",
                timeout.as_secs()
            )));
        }
    };
    let actions: Vec<String> = choices
        .iter()
        .enumerate()
        .flat_map(|(i, c)| [i.to_string(), c.clone()])
        .collect();
    // Expiry 0: the notification stays until the owner answers or dismisses it.
    let shown = send(&conn, question, "", &actions, 0, under_dnd, timeout).await?;
    let mut guard = CloseOnDrop {
        conn: conn.clone(),
        id: shown.id,
        armed: true,
    };
    // The server's unique name losing its owner is the server leaving. Followed after the
    // notification is shown (its name is not known before), then checked once more for a server
    // that left in between.
    let gone_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.interface("org.freedesktop.DBus"))
        .and_then(|b| b.member("NameOwnerChanged"))
        .and_then(|b| b.arg(0, shown.server.as_str()))
        .map_err(|e| Failure::Failed(format!("cannot follow the notification server: {e}")))?
        .build()
        .to_owned();
    let mut gone = zbus::MessageStream::for_match_rule(gone_rule, &conn, None)
        .await
        .map_err(|e| Failure::Failed(format!("cannot follow the notification server: {e}")))?;
    let server_left = || {
        Failure::Failed(
            "the notification server went away while the question was open, so it is gone \
             unanswered"
                .into(),
        )
    };
    if !crate::desktop::has_owner(&conn, &shown.server, timeout)
        .await
        .map_err(Failure::Failed)?
    {
        return Err(server_left());
    }
    let asked = |choice| Asked { choice, under_dnd };
    loop {
        let msg = tokio::select! {
            m = signals.next() => m,
            Some(m) = gone.next() => {
                if let Ok(m) = m
                    && let Ok((_name, _old, new)) = m.body().deserialize::<(String, String, String)>()
                    && new.is_empty()
                {
                    return Err(server_left());
                }
                continue;
            }
        };
        let Some(msg) = msg else { break };
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        if header.sender().map(|s| s.as_str()) != Some(shown.server.as_str()) {
            continue;
        }
        match header.member().map(|m| m.as_str()) {
            Some("ActionInvoked") => {
                if let Ok((id, key)) = msg.body().deserialize::<(u32, String)>()
                    && id == shown.id
                    && let Ok(i) = key.parse::<usize>()
                    && i < choices.len()
                {
                    guard.armed = false;
                    return Ok(asked(Some(i)));
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>()
                    && id == shown.id
                {
                    guard.armed = false;
                    return Ok(asked(None));
                }
            }
            _ => {}
        }
    }
    Err(Failure::Failed(
        "the session bus closed while the question was open".into(),
    ))
}
