//! The screen lock's sources over private buses: a fake screen saver service (`GetActive`,
//! `ActiveChanged`) and a fake logind (the user's display session, its `LockedHint` with
//! `PropertiesChanged`); either one locks, both must say unlocked.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, Value};

use super::{Facts, ScreenLock, watch};
use crate::core::Command;
use crate::desktop::tests::bus::PrivateBus;

const T: Duration = Duration::from_secs(5);

#[test]
fn either_source_locks_and_both_must_unlock() {
    let f = |logind, saver| Facts { logind, saver }.locked();
    assert!(!f(None, None));
    assert!(!f(Some(false), Some(false)));
    assert!(f(Some(true), Some(false)));
    assert!(f(Some(false), Some(true)));
    assert!(f(None, Some(true)));
    assert!(f(Some(true), None));
}

#[tokio::test]
async fn the_shared_flag_wakes_a_waiter_at_the_lock() {
    let s = ScreenLock::default();
    assert!(!s.is_locked());
    let waiter = tokio::spawn({
        let s = s.clone();
        async move { s.locked().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiter.is_finished());
    assert!(s.set(true));
    assert!(!s.set(true), "no change");
    tokio::time::timeout(T, waiter).await.unwrap().unwrap();
    assert!(s.is_locked());
    // While locked, a new waiter resolves at once.
    tokio::time::timeout(T, s.locked()).await.unwrap();
}

struct Saver(Arc<AtomicBool>);

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl Saver {
    #[zbus(name = "GetActive")]
    fn get_active(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// `user/self`'s `Display`: another of the user's sessions (`_99`), which Kai must not take for
/// its own.
struct User;

#[zbus::interface(name = "org.freedesktop.login1.User")]
impl User {
    #[zbus(property, name = "Display")]
    fn display(&self) -> (String, OwnedObjectPath) {
        (
            "99".into(),
            OwnedObjectPath::try_from("/org/freedesktop/login1/session/_399").unwrap(),
        )
    }
}

const OWN: &str = "/org/freedesktop/login1/session/_32";

/// logind's manager: session `2` is Kai's (`_32`), and so is the session of Kai's PID.
struct Manager;

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    #[zbus(name = "GetSession")]
    fn get_session(&self, id: String) -> zbus::fdo::Result<OwnedObjectPath> {
        match id.as_str() {
            "2" => Ok(OwnedObjectPath::try_from(OWN).unwrap()),
            "99" => Ok(OwnedObjectPath::try_from("/org/freedesktop/login1/session/_399").unwrap()),
            _ => Err(zbus::fdo::Error::Failed(format!("No session '{id}' known"))),
        }
    }
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        if pid == std::process::id() {
            Ok(OwnedObjectPath::try_from(OWN).unwrap())
        } else {
            Err(zbus::fdo::Error::Failed(
                "PID does not belong to any session".into(),
            ))
        }
    }
}

/// A session; its `LockedHint` is writable here (logind's own is set by `SetLockedHint`), so the
/// test changes it as a locker would and zbus emits `PropertiesChanged`; the shared value changes
/// it with no signal (a signal Kai missed).
struct Session(Arc<AtomicBool>);

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(property, name = "LockedHint")]
    fn locked_hint(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    #[zbus(property, name = "LockedHint")]
    fn set_locked_hint(&mut self, on: bool) {
        self.0.store(on, Ordering::SeqCst);
    }
}

/// A screen saver service (KScreenLocker's, in KWin) on `bus`.
async fn serve_saver(bus: &PrivateBus, active: &Arc<AtomicBool>) -> zbus::Connection {
    let c = bus.connect().await;
    for path in ["/ScreenSaver", "/org/freedesktop/ScreenSaver"] {
        c.object_server()
            .at(path, Saver(active.clone()))
            .await
            .unwrap();
    }
    c.request_name("org.freedesktop.ScreenSaver").await.unwrap();
    c
}

/// logind on `bus`: Kai's session `_32` with `hint`, and another session `_399` (locked: were it
/// taken for Kai's, Kai would be locked).
async fn serve_logind(bus: &PrivateBus, hint: &Arc<AtomicBool>) -> zbus::Connection {
    let c = bus.connect().await;
    let o = c.object_server();
    o.at("/org/freedesktop/login1/user/self", User)
        .await
        .unwrap();
    o.at("/org/freedesktop/login1", Manager).await.unwrap();
    o.at(OWN, Session(hint.clone())).await.unwrap();
    o.at(
        "/org/freedesktop/login1/session/_399",
        Session(Arc::new(AtomicBool::new(true))),
    )
    .await
    .unwrap();
    c.request_name("org.freedesktop.login1").await.unwrap();
    c
}

/// A service leaves the bus (its process ended): its name goes with its connection.
async fn leave(c: zbus::Connection, name: &str) {
    c.release_name(name).await.unwrap();
    drop(c);
}

struct Rig {
    session: PrivateBus,
    system: PrivateBus,
    saver: Option<zbus::Connection>,
    active: Arc<AtomicBool>,
    login: Option<zbus::Connection>,
    hint: Arc<AtomicBool>,
    cmds: mpsc::UnboundedReceiver<Command>,
    screen: ScreenLock,
    task: tokio::task::JoinHandle<()>,
}

impl Rig {
    /// The locker: the screen saver's state and its signal (KScreenLocker emits it on both paths).
    async fn saver(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
        let saver = self.saver.as_ref().expect("a screen saver service");
        for path in ["/ScreenSaver", "/org/freedesktop/ScreenSaver"] {
            saver
                .emit_signal(
                    None::<()>,
                    path,
                    "org.freedesktop.ScreenSaver",
                    "ActiveChanged",
                    &(active,),
                )
                .await
                .unwrap();
        }
    }

    async fn hint(&self, on: bool) {
        let c = self.system.connect().await;
        crate::desktop::call::<_, ()>(
            &c,
            "org.freedesktop.login1",
            OWN,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(
                "org.freedesktop.login1.Session",
                "LockedHint",
                Value::from(on),
            ),
            T,
        )
        .await
        .unwrap();
    }

    /// The screen saver service restarts (KWin's restart), saying `active` when it is back.
    async fn restart_saver(&mut self, active: bool) {
        if let Some(c) = self.saver.take() {
            leave(c, "org.freedesktop.ScreenSaver").await;
        }
        self.active.store(active, Ordering::SeqCst);
        self.saver = Some(serve_saver(&self.session, &self.active).await);
    }

    /// logind restarts, its hint `hint` when it is back.
    async fn restart_logind(&mut self, hint: bool) {
        if let Some(c) = self.login.take() {
            leave(c, "org.freedesktop.login1").await;
        }
        self.hint.store(hint, Ordering::SeqCst);
        self.login = Some(serve_logind(&self.system, &self.hint).await);
    }

    /// The next lock command, within `T`: the shared flag when it came.
    async fn next(&mut self) -> bool {
        match tokio::time::timeout(T, self.cmds.recv()).await {
            Ok(Some(Command::ScreenLock)) => self.screen.is_locked(),
            other => panic!("no lock command: {:?}", other.map(|c| c.is_some())),
        }
    }

    /// No lock command for a moment.
    async fn quiet(&mut self) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(self.cmds.try_recv().is_err(), "an unexpected command");
    }
}

/// The two services as given (`saver_active`: `None`, no screen saver service at start), the watch
/// started with `session_id` as `XDG_SESSION_ID` on a flag not known yet, as Kai starts it.
async fn rig_with(saver_active: Option<bool>, hint: bool, session_id: Option<&str>) -> Option<Rig> {
    let session = PrivateBus::start()?;
    let system = PrivateBus::start()?;
    let active = Arc::new(AtomicBool::new(saver_active.unwrap_or(false)));
    let saver = match saver_active {
        Some(_) => Some(serve_saver(&session, &active).await),
        None => None,
    };
    let hint = Arc::new(AtomicBool::new(hint));
    let login = Some(serve_logind(&system, &hint).await);
    let (tx, cmds) = mpsc::unbounded_channel();
    let screen = ScreenLock::unknown();
    assert!(screen.is_locked(), "not known yet: locked");
    let task = tokio::spawn(watch(
        tx,
        screen.clone(),
        Some(session.connect().await),
        Some(system.connect().await),
        session_id.map(str::to_owned),
        T,
    ));
    Some(Rig {
        session,
        system,
        saver,
        active,
        login,
        hint,
        cmds,
        screen,
        task,
    })
}

/// Both services up, Kai in session `2`; the first look (unlocked) taken.
async fn rig(saver_active: bool, hint: bool) -> Option<Rig> {
    let mut r = rig_with(Some(saver_active), hint, Some("2")).await?;
    let first = r.next().await;
    assert_eq!(first, saver_active || hint, "the first look is published");
    Some(r)
}

/// The screen saver's `ActiveChanged` locks and unlocks (Plasma's locker's own signal).
#[tokio::test]
async fn the_screen_savers_signal_locks_and_unlocks() {
    let Some(mut r) = rig(false, false).await else {
        return;
    };
    r.quiet().await;
    r.saver(true).await;
    assert!(r.next().await);
    assert!(
        r.screen.is_locked(),
        "the shared flag is set by the watch itself"
    );
    r.quiet().await;
    r.saver(false).await;
    assert!(!r.next().await);
    assert!(!r.screen.is_locked());
    r.task.abort();
}

/// logind's `LockedHint` locks too; while it holds, the screen saver's unlock does not unlock.
#[tokio::test]
async fn loginds_hint_locks_and_both_must_say_unlocked() {
    let Some(mut r) = rig(false, false).await else {
        return;
    };
    r.hint(true).await;
    assert!(r.next().await);
    r.saver(true).await;
    r.quiet().await;
    r.saver(false).await;
    r.quiet().await;
    assert!(r.screen.is_locked(), "logind still says locked");
    r.hint(false).await;
    assert!(!r.next().await);
    r.task.abort();
}

/// Kai started on a locked screen: locked at once.
#[tokio::test]
async fn a_start_on_a_locked_screen_is_locked_at_once() {
    let Some(mut r) = rig(true, false).await else {
        return;
    };
    assert!(r.screen.is_locked());
    r.saver(false).await;
    assert!(!r.next().await);
    r.task.abort();
}

/// Kai's session is its own: `XDG_SESSION_ID`'s, else the one of Kai's PID (an unknown id falls
/// back to the PID), never `user/self`'s `Display` (here another session, locked).
#[tokio::test]
async fn kais_own_session_is_followed() {
    for id in [Some("2"), None, Some("7")] {
        let Some(mut r) = rig_with(Some(false), false, id).await else {
            return;
        };
        assert!(!r.next().await, "{id:?}: not the Display session's lock");
        r.hint(true).await;
        assert!(r.next().await, "{id:?}: Kai's session's hint locks");
        r.hint(false).await;
        assert!(!r.next().await, "{id:?}");
        r.task.abort();
    }
}

/// KWin restarts (the locker and its screen saver service with it) and the unlock's signal was
/// lost: the service leaving takes its lock with it, the new owner is asked again and followed.
#[tokio::test]
async fn a_screen_saver_service_that_restarts_is_followed_again() {
    let Some(mut r) = rig(false, false).await else {
        return;
    };
    r.saver(true).await;
    assert!(r.next().await);
    // Back unlocked; the unlock's signal never came.
    r.restart_saver(false).await;
    assert!(!r.next().await, "unlocked with the old owner gone");
    r.quiet().await;
    // The new owner's signals are followed.
    r.saver(true).await;
    assert!(r.next().await);
    // Back locked after a restart while locked: asked again, still locked.
    r.restart_saver(true).await;
    assert!(!r.next().await, "the old owner's lock went with it");
    assert!(r.next().await, "the new owner says locked");
    r.saver(false).await;
    assert!(!r.next().await);
    r.task.abort();
}

/// logind restarts and its hint's change was lost: asked again and followed after.
#[tokio::test]
async fn logind_restarting_is_followed_again() {
    let Some(mut r) = rig(false, false).await else {
        return;
    };
    r.hint(true).await;
    assert!(r.next().await);
    r.restart_logind(false).await;
    assert!(!r.next().await);
    r.quiet().await;
    r.hint(true).await;
    assert!(r.next().await, "the new logind's signal is followed");
    r.hint(false).await;
    assert!(!r.next().await);
    r.task.abort();
}

/// A screen saver service that appears after Kai's start is picked up and followed.
#[tokio::test]
async fn a_screen_saver_service_that_appears_later_is_followed() {
    let Some(mut r) = rig_with(None, false, Some("2")).await else {
        return;
    };
    assert!(!r.next().await, "the first look: logind's alone");
    r.restart_saver(true).await;
    assert!(r.next().await, "the new service says locked");
    r.saver(false).await;
    assert!(!r.next().await);
    r.task.abort();
}

/// A signal one source missed: when the two disagree after the other's change, the one that
/// stayed is read again, so a lost unlock does not hold the lock.
#[tokio::test]
async fn a_missed_signal_is_read_again_when_the_sources_disagree() {
    let Some(mut r) = rig(false, false).await else {
        return;
    };
    r.hint(true).await;
    assert!(r.next().await);
    r.saver(true).await;
    r.quiet().await;
    // logind's unlock without its signal.
    r.hint.store(false, Ordering::SeqCst);
    r.saver(false).await;
    assert!(!r.next().await, "logind asked again: unlocked");
    r.task.abort();
}

/// Without either source the watch ends, saying so; its first look is published (unlocked), so
/// a lock not known yet does not stay.
#[tokio::test]
async fn no_source_no_lock() {
    let (tx, mut cmds) = mpsc::unbounded_channel();
    let screen = ScreenLock::unknown();
    tokio::time::timeout(T, watch(tx, screen.clone(), None, None, None, T))
        .await
        .unwrap();
    assert!(matches!(cmds.try_recv(), Ok(Command::ScreenLock)));
    assert!(!screen.is_locked());
}

/// A flag that was never looked at is locked; `default` (no watch) is not.
#[test]
fn not_known_yet_counts_as_locked() {
    assert!(ScreenLock::unknown().is_locked());
    assert!(!ScreenLock::default().is_locked());
    let s = ScreenLock::unknown();
    assert!(s.set(false), "the first look always changes it");
    assert!(!s.is_locked());
    let s = ScreenLock::unknown();
    assert!(s.set(true));
    assert!(s.is_locked());
}
