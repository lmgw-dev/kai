//! The machine's sleep (review KAI-2 K2-8): logind's `PrepareForSleep(false)` on the system bus
//! says the machine resumed, and the core then looks at the conversation's idle rollover at once,
//! by the wall clock (tokio's timers do not count a suspend). Without logind, or with the system bus
//! out of reach, the voice's own look every `voice::thread::IDLE_CHECK` still finds it, later.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::core::Command;

const LOGIND: &str = "org.freedesktop.login1";

/// Watches logind until the stream ends; `timeout` (`dbus.timeout_s`) bounds the connect and the
/// match rule. A failure is logged once and the watch ends.
pub async fn watch(cmds: mpsc::UnboundedSender<Command>, timeout: Duration) {
    let stream = tokio::time::timeout(timeout, async {
        let conn = zbus::Connection::system().await?;
        // Signals carry the sender's unique name: the rule pins logind's (as `shortcuts` does).
        let owner = zbus::fdo::DBusProxy::new(&conn)
            .await?
            .get_name_owner(zbus::names::BusName::try_from(LOGIND)?)
            .await?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(owner.as_str())?
            .interface("org.freedesktop.login1.Manager")?
            .member("PrepareForSleep")?
            .path("/org/freedesktop/login1")?
            .build()
            .to_owned();
        zbus::MessageStream::for_match_rule(rule, &conn, None).await
    })
    .await;
    let mut stream = match stream {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(
                "cannot follow logind's sleep ({e}); the idle rollover is looked at every 30 s"
            );
            return;
        }
        Err(_) => {
            tracing::warn!(
                "the system bus did not answer within {} s (dbus.timeout_s); the idle rollover is \
                 looked at every 30 s",
                timeout.as_secs()
            );
            return;
        }
    };
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };
        match msg.body().deserialize::<bool>() {
            Ok(true) => tracing::info!("the machine goes to sleep"),
            Ok(false) => {
                if cmds.send(Command::Resumed).is_err() {
                    return;
                }
            }
            Err(_) => {}
        }
    }
}
