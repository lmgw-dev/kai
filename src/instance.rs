//! Single instance through the D-Bus name `dev.rootshell.kai` (§2, K13).
//!
//! The running Kai owns the name and serves `dev.rootshell.kai.Pairing1` at `/dev/rootshell/kai`.
//! A second `kai` started by the `lmgw-pair:` scheme handler hands its link over with `Pair` and
//! exits. The link travels only inside that method call: it is never logged, and no reply or error
//! echoes it.
//!
//! Any process of the session may call `Pair` (the session bus lets every peer of the same user
//! through), so the caller is not trusted: `Pair` only hands the link to the core, which asks the
//! owner in Kai's own dialog before anything is stored (K13). It answers at once, so the hand-off
//! never waits on the owner and the answer says nothing about the wallet: every link that parses is
//! taken while no other link waits, and refused, with [`WAITING`], while one does ([`PairGate`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use zbus::fdo::RequestNameFlags;

use crate::core::Command;
use crate::pairing::PairingLink;

pub const BUS_NAME: &str = "dev.rootshell.kai";
pub const OBJECT_PATH: &str = "/dev/rootshell/kai";
pub const INTERFACE: &str = "dev.rootshell.kai.Pairing1";
/// `Pair`'s answer for a link it takes.
pub const AWAITING: &str = "awaiting the owner's confirmation in Kai";
/// `Pair`'s refusal while another link waits for the owner (review KAI-2 K2-4).
pub const WAITING: &str = "a pairing is already waiting for confirmation";

/// One pairing link waits for the owner at a time (review KAI-2 K2-4, the owner's decision
/// 2026-10-07). `Pair` claims the gate before it hands a link to the core, and the core releases it
/// once that link no longer waits (answered, or paired by paste). While it is claimed, every
/// further link is refused, so nothing swaps under the owner's pointer and arrivals cannot flood.
#[derive(Debug, Clone, Default)]
pub struct PairGate(Arc<AtomicBool>);

impl PairGate {
    /// Takes the gate for a new link; `false`: a link waits already.
    pub fn claim(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// The core: a link waits (whoever handed it over).
    pub fn hold(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// The core: no link waits any more.
    pub fn release(&self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct PairingService {
    cmds: mpsc::UnboundedSender<Command>,
    gate: PairGate,
}

impl PairingService {
    pub fn new(cmds: mpsc::UnboundedSender<Command>, gate: PairGate) -> Self {
        Self { cmds, gate }
    }
}

#[zbus::interface(name = "dev.rootshell.kai.Pairing1")]
impl PairingService {
    /// Takes a `lmgw-pair:` link and answers at once that it awaits the owner's confirmation. A
    /// link that does not parse (a v1 link from before TLS among them) is refused with the reason;
    /// so is every link while another one waits ([`WAITING`]).
    async fn pair(&self, link: String) -> zbus::fdo::Result<String> {
        let link =
            PairingLink::parse(&link).map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        if !self.gate.claim() {
            tracing::warn!("a pairing link was handed over on D-Bus while another waits: refused");
            return Err(zbus::fdo::Error::Failed(WAITING.to_owned()));
        }
        tracing::info!("a pairing link was handed over on D-Bus (the caller is not authenticated)");
        if self.cmds.send(Command::Arrived(link)).is_err() {
            self.gate.release();
            return Err(zbus::fdo::Error::Failed("Kai is quitting".into()));
        }
        Ok(AWAITING.to_owned())
    }
}

/// Serves the interface, then claims the name. `Ok(true)`: this process is now the running Kai.
/// `Ok(false)`: another process owns the name; the interface is withdrawn again.
pub async fn claim(conn: &zbus::Connection, service: PairingService) -> zbus::Result<bool> {
    conn.object_server().at(OBJECT_PATH, service).await?;
    match conn
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(_) => Ok(true),
        Err(zbus::Error::NameTaken) => {
            conn.object_server()
                .remove::<PairingService, _>(OBJECT_PATH)
                .await?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    #[error("no Kai is running")]
    NoInstance,
    #[error("the running Kai refused the link: {0}")]
    Refused(String),
    #[error(
        "the running Kai did not answer within {} s (pairing.handoff_timeout_s); if it took the \
         link, it asks for your confirmation in its own dialog", .0.as_secs()
    )]
    Timeout(Duration),
    #[error("D-Bus: {0}")]
    Bus(String),
}

/// Hands `link` to the running Kai (K13) and returns its answer.
pub async fn hand_off(
    conn: &zbus::Connection,
    link: &str,
    timeout: Duration,
) -> Result<String, HandoffError> {
    let body = (link,);
    let call = conn.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), "Pair", &body);
    match tokio::time::timeout(timeout, call).await {
        Err(_) => Err(HandoffError::Timeout(timeout)),
        Ok(Ok(reply)) => reply
            .body()
            .deserialize::<String>()
            .map_err(|e| HandoffError::Bus(e.to_string())),
        Ok(Err(zbus::Error::MethodError(name, message, _))) => match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => Err(HandoffError::NoInstance),
            _ => Err(HandoffError::Refused(
                message.unwrap_or_else(|| name.to_string()),
            )),
        },
        Ok(Err(e)) => Err(HandoffError::Bus(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command as Process, Stdio};

    use super::*;
    use crate::pairing::tests::{TOKEN, good};

    /// A private `dbus-daemon` for the test, killed on drop. Nothing touches the session bus.
    struct PrivateBus {
        child: Child,
        address: String,
        _dir: tempfile::TempDir,
    }

    impl PrivateBus {
        fn start() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let conf = dir.path().join("bus.conf");
            std::fs::write(
                &conf,
                format!(
                    r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <listen>unix:path={}/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                    dir.path().display()
                ),
            )
            .unwrap();
            let mut child = Process::new("dbus-daemon")
                .arg(format!("--config-file={}", conf.display()))
                .args(["--nofork", "--nopidfile", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon");
            let mut address = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            Self {
                child,
                address: address.trim().to_owned(),
                _dir: dir,
            }
        }

        async fn connect(&self) -> zbus::Connection {
            zbus::connection::Builder::address(self.address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap()
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Takes the links the service hands to the core, recording the device names.
    fn fake_core(mut rx: mpsc::UnboundedReceiver<Command>) -> tokio::task::JoinHandle<Vec<String>> {
        tokio::spawn(async move {
            let mut got = Vec::new();
            while let Some(cmd) = rx.recv().await {
                if let Command::Arrived(link) = cmd {
                    assert_eq!(link.token.expose(), TOKEN);
                    got.push(link.name);
                }
            }
            got
        })
    }

    #[tokio::test]
    async fn a_second_kai_hands_its_link_to_the_running_one_which_answers_at_once() {
        let bus = PrivateBus::start();
        let running = bus.connect().await;
        let (tx, rx) = mpsc::unbounded_channel();
        assert!(
            claim(&running, PairingService::new(tx, PairGate::default()))
                .await
                .unwrap(),
            "the first kai owns the name"
        );
        let core = fake_core(rx);

        let second = bus.connect().await;
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        assert!(
            !claim(&second, PairingService::new(tx2, PairGate::default()))
                .await
                .unwrap(),
            "the second kai does not"
        );

        let timeout = Duration::from_secs(5);
        assert_eq!(
            hand_off(&second, &good(), timeout).await,
            Ok(AWAITING.to_owned())
        );

        // A link from before TLS is refused by the running Kai, with a reason that does not echo
        // its key.
        let key = "lmgw-device-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let bad = format!("lmgw-pair:?v=1&url=http%3A%2F%2Fh&name=n&key={key}");
        match hand_off(&second, &bad, timeout).await {
            Err(HandoffError::Refused(msg)) => {
                assert!(msg.contains("from a pairing before TLS"), "{msg}");
                assert!(!msg.contains(&key[12..]), "{msg}");
            }
            other => panic!("{other:?}"),
        }
        // So is a link to a plain http address (the device link is TLS only).
        let plain = good().replace("https%3A", "http%3A");
        match hand_off(&second, &plain, timeout).await {
            Err(HandoffError::Refused(msg)) => assert!(msg.contains("https"), "{msg}"),
            other => panic!("{other:?}"),
        }

        // Once the running Kai is gone, a hand-off finds nobody.
        running.release_name(BUS_NAME).await.unwrap();
        assert_eq!(
            hand_off(&second, &good(), timeout).await,
            Err(HandoffError::NoInstance)
        );
        running
            .object_server()
            .remove::<PairingService, _>(OBJECT_PATH)
            .await
            .unwrap();
        assert_eq!(core.await.unwrap(), vec!["desktop".to_owned()]);
        assert!(
            rx2.try_recv().is_err(),
            "the second kai's own service never got a command"
        );
    }

    #[tokio::test]
    async fn the_answer_never_waits_on_the_core() {
        // Nobody takes the core's commands (a busy core, or an owner who never answers): the
        // hand-off is answered anyway.
        let bus = PrivateBus::start();
        let running = bus.connect().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        assert!(
            claim(&running, PairingService::new(tx, PairGate::default()))
                .await
                .unwrap()
        );
        let second = bus.connect().await;
        let t0 = std::time::Instant::now();
        assert_eq!(
            hand_off(&second, &good(), Duration::from_secs(5)).await,
            Ok(AWAITING.to_owned())
        );
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    }

    /// Review KAI-2 K2-4: while one link waits for the owner, every further link is refused with
    /// the reason, at once; once the core released the gate, the next link is taken.
    #[tokio::test]
    async fn a_link_while_another_waits_is_refused() {
        let bus = PrivateBus::start();
        let running = bus.connect().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate = PairGate::default();
        assert!(
            claim(&running, PairingService::new(tx, gate.clone()))
                .await
                .unwrap()
        );
        let second = bus.connect().await;
        let timeout = Duration::from_secs(5);
        assert_eq!(
            hand_off(&second, &good(), timeout).await,
            Ok(AWAITING.to_owned())
        );
        for _ in 0..3 {
            assert_eq!(
                hand_off(&second, &good(), timeout).await,
                Err(HandoffError::Refused(WAITING.to_owned()))
            );
        }
        assert!(matches!(rx.try_recv(), Ok(Command::Arrived(_))));
        assert!(
            rx.try_recv().is_err(),
            "the refused links never reached the core"
        );
        // The owner answered: the core releases the gate.
        gate.release();
        assert_eq!(
            hand_off(&second, &good(), timeout).await,
            Ok(AWAITING.to_owned())
        );
        assert!(matches!(rx.try_recv(), Ok(Command::Arrived(_))));
    }

    /// A Kai whose D-Bus service hangs.
    struct Hung;

    #[zbus::interface(name = "dev.rootshell.kai.Pairing1")]
    impl Hung {
        async fn pair(&self, _link: String) -> zbus::fdo::Result<String> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_hung_kai_times_out_with_the_setting_named() {
        let bus = PrivateBus::start();
        let running = bus.connect().await;
        running.object_server().at(OBJECT_PATH, Hung).await.unwrap();
        running.request_name(BUS_NAME).await.unwrap();
        let second = bus.connect().await;
        let err = hand_off(&second, &good(), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("pairing.handoff_timeout_s"),
            "{err}"
        );
    }
}
