//! Desktop notifications (`org.freedesktop.Notifications`), for outcomes nobody watches a window
//! for: a pairing link waiting for the wallet, a voice session that stopped, an audio problem, an
//! update (with actions, [`notify`] and [`actions`]).
//!
//! Every call is bounded by `dbus.timeout_s`: a notification server that hangs (a plasmashell that
//! is still starting at login) never holds up Kai or its scheme handler. Past the bound the
//! notification is dropped with a log line naming the setting.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::Value;

const SERVER: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// The D-Bus specification's largest message, 2^27 bytes (128 MiB): what zbus and the bus daemons
/// accept at most, so the real bound of a notification.
pub const DBUS_MAX_MESSAGE: u64 = 1 << 27;

/// The bytes of text a notification's body can carry as Kai sends it: the largest message less
/// 64 KiB for the call's other fields (the header, the summary, the app name, the hints), divided by
/// five because escaping can make each byte five (`&` is `&amp;`). A routine run from the tray or
/// a key with more stdout is the run's error naming this size (`crate::routines`).
pub const BODY_ROOM: u64 = (DBUS_MAX_MESSAGE - 64 * 1024) / 5;

/// [`BODY_ROOM`] as a refusal names it.
pub fn body_room_says() -> String {
    format!(
        "a notification's room: {BODY_ROOM} bytes, the D-Bus message limit of {DBUS_MAX_MESSAGE} \
         bytes for text that escaping may make five times longer"
    )
}

/// A notification's action: the key `ActionInvoked` names, and the button's label.
pub type Action = (&'static str, &'static str);

/// A notification the server showed: its id and the server's unique bus name, which an action's
/// signal must come from ([`Invoked::of`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub id: u32,
    pub server: String,
}

/// An action the owner clicked, as the server's `ActionInvoked` said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoked {
    /// The signal's sender, a unique bus name.
    pub server: String,
    pub id: u32,
    pub key: String,
}

impl Invoked {
    /// The action is one of `shown`'s: its id, from the server that showed it. Every client's
    /// `ActionInvoked` reaches every listener, and any peer of the session can send a signal under
    /// that name, so both are checked.
    pub fn of(&self, shown: &Shown) -> bool {
        self.id == shown.id && self.server == shown.server
    }
}

pub async fn send(conn: &zbus::Connection, summary: &str, body: &str, timeout: Duration) {
    notify(conn, summary, body, &[], None, timeout).await;
}

/// A notification with `actions` (none: a plain one), taking the place of `replaces` when that is
/// one of Kai's still shown. What the server showed, or `None` (logged) when it failed.
pub async fn notify(
    conn: &zbus::Connection,
    summary: &str,
    body: &str,
    actions: &[Action],
    replaces: Option<&Shown>,
    timeout: Duration,
) -> Option<Shown> {
    #[cfg(feature = "live-test")]
    if crate::live_test::active() {
        tracing::info!(summary, body, "live-test: notification (not sent)");
        return None;
    }
    let hints: HashMap<&str, Value<'_>> =
        HashMap::from([("desktop-entry", Value::from(crate::APP_ID))]);
    let body = crate::tray::escape(body);
    let actions: Vec<&str> = actions.iter().flat_map(|(k, l)| [*k, *l]).collect();
    let args = (
        "Kai",
        replaces.map_or(0u32, |s| s.id),
        crate::APP_ID,
        summary,
        body.as_str(),
        actions,
        hints,
        -1i32,
    );
    let call = conn.call_method(Some(SERVER), PATH, Some(SERVER), "Notify", &args);
    match tokio::time::timeout(timeout, call).await {
        Ok(Ok(reply)) => {
            let server = reply.header().sender().map(|s| s.to_string());
            match (reply.body().deserialize::<u32>(), server) {
                (Ok(id), Some(server)) => Some(Shown { id, server }),
                _ => {
                    tracing::warn!(summary, "the notification server's answer named no id");
                    None
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!("desktop notification failed: {e}");
            None
        }
        Err(_) => {
            tracing::warn!(
                summary,
                "the notification server did not answer within {} s (dbus.timeout_s); the \
                 notification is dropped",
                timeout.as_secs()
            );
            None
        }
    }
}

/// Follows the notification server's `ActionInvoked` into `tx` until the connection or the
/// receiver goes. `timeout` (`dbus.timeout_s`) bounds the match rule; a failure is logged once and
/// the watch ends (the notifications still show; their buttons then do nothing).
pub async fn actions(
    conn: zbus::Connection,
    tx: mpsc::UnboundedSender<Invoked>,
    timeout: Duration,
) {
    let stream = tokio::time::timeout(timeout, async {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(SERVER)?
            .member("ActionInvoked")?
            .path(PATH)?
            .build()
            .to_owned();
        zbus::MessageStream::for_match_rule(rule, &conn, None).await
    })
    .await;
    let mut stream = match stream {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!("cannot follow the notifications' buttons ({e}); they do nothing");
            return;
        }
        Err(_) => {
            tracing::warn!(
                "the session bus did not answer within {} s (dbus.timeout_s); the notifications' \
                 buttons do nothing",
                timeout.as_secs()
            );
            return;
        }
    };
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };
        let Some(server) = msg.header().sender().map(|s| s.to_string()) else {
            continue;
        };
        let Ok((id, key)) = msg.body().deserialize::<(u32, String)>() else {
            continue;
        };
        if tx.send(Invoked { server, id, key }).is_err() {
            return;
        }
    }
}

/// [`notify`] and [`actions`] against a notification server of the test's own on a private bus (a
/// `dbus-daemon` in a temp directory that activates nothing), never the desktop's.
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use zbus::zvariant::OwnedValue;

    use super::*;

    /// What the fake server was sent: replaces_id, app name, summary, body, actions, hints' keys.
    type Call = (u32, String, String, String, Vec<String>, Vec<String>);

    struct FakeServer {
        calls: Arc<Mutex<Vec<Call>>>,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl FakeServer {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app_name: String,
            replaces_id: u32,
            _app_icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            let mut calls = self.calls.lock().unwrap();
            let mut keys: Vec<String> = hints.into_keys().collect();
            keys.sort();
            calls.push((replaces_id, app_name, summary, body, actions, keys));
            if replaces_id != 0 {
                replaces_id
            } else {
                calls.len() as u32
            }
        }
    }

    /// A private bus: its address, and the daemon (killed on drop).
    async fn private_bus(dir: &std::path::Path) -> Option<(String, tokio::process::Child)> {
        let conf = dir.join("bus.conf");
        std::fs::write(
            &conf,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\"
 \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">
<busconfig><type>session</type><listen>unix:dir={}</listen><auth>EXTERNAL</auth>
<policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>
<allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
                dir.display()
            ),
        )
        .unwrap();
        let mut child = match tokio::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", conf.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipped: no dbus-daemon ({e})");
                return None;
            }
        };
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let address = lines.next_line().await.unwrap().expect("the bus's address");
        Some((address, child))
    }

    #[tokio::test]
    async fn notify_shows_actions_and_an_action_counts_only_from_its_server() {
        let dir = tempfile::tempdir().unwrap();
        let Some((address, _daemon)) = private_bus(dir.path()).await else {
            return;
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let server = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .name(SERVER)
            .unwrap()
            .serve_at(
                PATH,
                FakeServer {
                    calls: calls.clone(),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let kai = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let timeout = Duration::from_secs(5);

        let shown = notify(
            &kai,
            "Kai update available",
            "a <b>",
            &[("install", "Install"), ("later", "Later")],
            None,
            timeout,
        )
        .await
        .expect("shown");
        let server_name = server.unique_name().unwrap().to_string();
        assert_eq!(
            shown,
            Shown {
                id: 1,
                server: server_name.clone()
            }
        );
        let again = notify(&kai, "Installing", "", &[], Some(&shown), timeout)
            .await
            .unwrap();
        assert_eq!(again.id, 1, "replaced in place");
        {
            let calls = calls.lock().unwrap();
            assert_eq!(
                calls[0],
                (
                    0,
                    "Kai".to_owned(),
                    "Kai update available".to_owned(),
                    "a &lt;b&gt;".to_owned(),
                    vec![
                        "install".to_owned(),
                        "Install".to_owned(),
                        "later".to_owned(),
                        "Later".to_owned()
                    ],
                    vec!["desktop-entry".to_owned()]
                )
            );
            assert_eq!((calls[1].0, calls[1].4.len()), (1, 0));
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let watch = tokio::spawn(actions(kai.clone(), tx, timeout));
        // The match rule is in place once a signal arrives: the server sends until one does.
        let got = tokio::time::timeout(timeout, async {
            loop {
                server
                    .emit_signal(
                        None::<&str>,
                        PATH,
                        SERVER,
                        "ActionInvoked",
                        &(1u32, "install"),
                    )
                    .await
                    .unwrap();
                tokio::select! {
                    got = rx.recv() => break got.unwrap(),
                    () = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        })
        .await
        .expect("the action within 5 s");
        assert_eq!(
            got,
            Invoked {
                server: server_name.clone(),
                id: 1,
                key: "install".into()
            }
        );
        assert!(got.of(&shown));

        // Any peer can send the signal; it is not the server's. Its own key tells it apart from a
        // late one of the server's loop above.
        let forger = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        forger
            .emit_signal(
                None::<&str>,
                PATH,
                SERVER,
                "ActionInvoked",
                &(1u32, "forged"),
            )
            .await
            .unwrap();
        let forged = tokio::time::timeout(timeout, async {
            loop {
                let a = rx.recv().await.unwrap();
                if a.key == "forged" {
                    break a;
                }
            }
        })
        .await
        .expect("the forged signal within 5 s");
        assert_eq!(forged.id, 1);
        assert!(!forged.of(&shown), "{forged:?}");
        watch.abort();
    }
}
