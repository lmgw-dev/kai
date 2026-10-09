//! Kai's accessibility switch (the owner's rule, 2026-10-09; `fdo::atspi::switch`) against a fake
//! `org.a11y.Status` whose properties can be written, as at-spi-bus-launcher's: on during
//! conversation mode, around a read with the earlier value put back, an owner's change in between
//! never clobbered, a switch that was on left alone, `ScreenReaderEnabled` never written, the lock,
//! the crash marker, and a read that waits for its app's announcement instead of sleeping.

use std::time::Instant;

use super::*;
use crate::desktop::fdo::atspi::switch::{A11ySwitch, Held, MARKER, SwitchMode, SwitchSettings};

/// Every write the fake got, from anyone: (property, value).
type Writes = Arc<Mutex<Vec<(&'static str, bool)>>>;

/// What happens when `IsEnabled` goes on: an app that joins the bus (Qt's bridge).
type OnEnable = Arc<dyn Fn() + Send + Sync>;

/// How slow the fake launcher is: its answer to a read of `IsEnabled`, to a write, and its
/// `PropertiesChanged` after a write (at-spi-bus-launcher announces its GSettings key's change,
/// which can come after the write's reply, even after the next write).
#[derive(Debug, Default, Clone, Copy)]
struct Delays {
    get: Duration,
    set: Duration,
    echo: Duration,
}

type SharedDelays = Arc<Mutex<Delays>>;

struct Status {
    enabled: bool,
    reader: bool,
    writes: Writes,
    on_enable: Option<OnEnable>,
    delays: SharedDelays,
    /// The launcher's connection: `PropertiesChanged` is emitted by hand, after `delays.echo`.
    conn: zbus::Connection,
}

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property(emits_changed_signal = "false"), name = "IsEnabled")]
    async fn is_enabled(&self) -> bool {
        let wait = self.delays.lock().unwrap().get;
        tokio::time::sleep(wait).await;
        self.enabled
    }
    #[zbus(property, name = "IsEnabled")]
    async fn set_is_enabled(&mut self, on: bool) {
        let delays = *self.delays.lock().unwrap();
        tokio::time::sleep(delays.set).await;
        self.writes.lock().unwrap().push(("IsEnabled", on));
        let was = self.enabled;
        self.enabled = on;
        if on
            && !was
            && let Some(f) = &self.on_enable
        {
            f();
        }
        let conn = self.conn.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delays.echo).await;
            let _ = conn
                .emit_signal(
                    None::<()>,
                    "/org/a11y/bus",
                    "org.freedesktop.DBus.Properties",
                    "PropertiesChanged",
                    &(
                        "org.a11y.Status",
                        HashMap::from([("IsEnabled", Value::from(on))]),
                        Vec::<String>::new(),
                    ),
                )
                .await;
        });
    }
    #[zbus(property, name = "ScreenReaderEnabled")]
    fn screen_reader_enabled(&self) -> bool {
        self.reader
    }
    #[zbus(property, name = "ScreenReaderEnabled")]
    fn set_screen_reader_enabled(&mut self, on: bool) {
        self.writes
            .lock()
            .unwrap()
            .push(("ScreenReaderEnabled", on));
        self.reader = on;
    }
}

/// The registry's root, whose apps can join later.
struct LiveRegistry(Arc<Mutex<Vec<(String, OwnedObjectPath)>>>);

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl LiveRegistry {
    #[zbus(name = "GetChildren")]
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.0.lock().unwrap().clone()
    }
}

struct Rig {
    session: PrivateBus,
    _a11y: PrivateBus,
    writes: Writes,
    delays: SharedDelays,
    /// Each value the switch held, as the test reads it.
    _held: Vec<zbus::Connection>,
    dir: tempfile::TempDir,
}

impl Rig {
    async fn is_enabled(&self) -> bool {
        let c = self.session.connect().await;
        crate::desktop::property::<bool>(
            &c,
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Status",
            "IsEnabled",
            T,
        )
        .await
        .unwrap()
    }

    /// The owner flips the switch (System Settings, a screen reader): a write of its own.
    async fn owner_sets(&self, on: bool) {
        let c = self.session.connect().await;
        crate::desktop::call::<_, ()>(
            &c,
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.freedesktop.DBus.Properties",
            "Set",
            &("org.a11y.Status", "IsEnabled", Value::from(on)),
            T,
        )
        .await
        .unwrap();
    }

    async fn switch(&self, mode: SwitchMode, wait: Duration) -> A11ySwitch {
        A11ySwitch::new(
            SessionBus::connected(self.session.connect().await, T),
            T,
            SwitchSettings { mode, wait },
            Some(self.dir.path().join(MARKER)),
        )
    }

    fn marker(&self) -> bool {
        self.dir.path().join(MARKER).exists()
    }

    fn slow(&self, delays: Delays) {
        *self.delays.lock().unwrap() = delays;
    }

    fn reader_written(&self) -> bool {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| *p == "ScreenReaderEnabled")
    }
}

/// A session bus with the launcher's `org.a11y.Bus` and `org.a11y.Status` (`IsEnabled` as
/// `enabled`), an accessibility bus with the registry; the test app ([`app_tree`], this process)
/// joins `joins_after` after `IsEnabled` goes on (`None`: it never joins), announced with
/// `ChildrenChanged` as at-spi2-registryd announces an app.
async fn rig(enabled: bool, joins_after: Option<Duration>) -> Option<Rig> {
    let session = PrivateBus::start()?;
    let a11y = PrivateBus::start()?;
    let app = a11y.connect().await;
    let root = serve(&app, app_tree(), "/org/a11y/atspi/accessible/root".into()).await;
    let apps: Arc<Mutex<Vec<(String, OwnedObjectPath)>>> = Arc::default();
    let registry = a11y.connect().await;
    registry
        .object_server()
        .at(
            "/org/a11y/atspi/accessible/root",
            LiveRegistry(apps.clone()),
        )
        .await
        .unwrap();
    registry
        .request_name("org.a11y.atspi.Registry")
        .await
        .unwrap();
    let writes = Writes::default();
    let delays = SharedDelays::default();
    let on_enable: Option<OnEnable> = joins_after.map(|after| {
        let registry = registry.clone();
        let apps = apps.clone();
        let root = root.clone();
        Arc::new(move || {
            let registry = registry.clone();
            let apps = apps.clone();
            let root = root.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                apps.lock().unwrap().push(root.clone());
                registry
                    .emit_signal(
                        None::<()>,
                        "/org/a11y/atspi/accessible/root",
                        "org.a11y.atspi.Event.Object",
                        "ChildrenChanged",
                        &(
                            "add",
                            0i32,
                            0i32,
                            Value::from(root.0.as_str()),
                            HashMap::<String, Value>::new(),
                        ),
                    )
                    .await
                    .unwrap();
            });
        }) as OnEnable
    });
    let launcher = session.connect().await;
    let objects = launcher.object_server();
    objects
        .at(
            "/org/a11y/bus",
            A11yBus {
                address: a11y.address.clone(),
            },
        )
        .await
        .unwrap();
    objects
        .at(
            "/org/a11y/bus",
            Status {
                enabled,
                reader: false,
                writes: writes.clone(),
                on_enable,
                delays: delays.clone(),
                conn: launcher.clone(),
            },
        )
        .await
        .unwrap();
    launcher.request_name("org.a11y.Bus").await.unwrap();
    Some(Rig {
        session,
        _a11y: a11y,
        writes,
        delays,
        _held: vec![launcher, app, registry],
        dir: temp_dir(),
    })
}

/// Settles a spawned settle.
async fn settled(h: Option<tokio::task::JoinHandle<()>>) {
    h.expect("inside a runtime").await.unwrap();
}

/// Conversation mode: on while it is active, back off at its end; only `IsEnabled` is written.
#[tokio::test]
async fn conversation_mode_switches_it_on_and_back_off() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    settled(s.set_conversation(true)).await;
    assert!(r.is_enabled().await);
    assert!(matches!(s.held(), Held::On { .. }));
    assert!(r.marker(), "the marker says the switch is Kai's");
    settled(s.set_conversation(false)).await;
    assert!(!r.is_enabled().await);
    assert_eq!(s.held(), Held::Nothing);
    assert!(!r.marker());
    assert_eq!(
        s.written(),
        [("IsEnabled", true), ("IsEnabled", false)],
        "ScreenReaderEnabled is never written"
    );
    assert!(!r.reader_written());
}

/// Around a read in `read` mode (push-to-talk and other modes): on for the read, the earlier value
/// back after; conversation mode alone switches nothing in that mode.
#[tokio::test]
async fn a_read_switches_it_on_around_itself_and_puts_it_back() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r.switch(SwitchMode::Read, Duration::from_secs(5)).await;
    settled(s.set_conversation(true)).await;
    assert!(
        !r.is_enabled().await,
        "read mode: conversation mode switches nothing"
    );
    let hold = s.hold().await;
    assert!(r.is_enabled().await);
    // A second read meanwhile shares it: the first one's end leaves it on.
    let second = s.hold().await;
    hold.release().await;
    assert!(r.is_enabled().await, "another read still holds it");
    second.release().await;
    assert!(!r.is_enabled().await);
    // A dropped hold puts it back too.
    drop(s.hold().await);
    tokio::time::timeout(T, async {
        while r.is_enabled().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("put back after the drop");
    assert!(!r.reader_written());
}

/// A switch that was on already is not Kai's: Kai never writes it, during or after.
#[tokio::test]
async fn a_switch_that_was_on_is_left_alone() {
    let Some(r) = rig(true, None).await else {
        return;
    };
    let s = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    settled(s.set_conversation(true)).await;
    assert_eq!(s.held(), Held::NotKais);
    let hold = s.hold().await;
    hold.release().await;
    settled(s.set_conversation(false)).await;
    assert!(r.is_enabled().await, "still on");
    assert!(s.written().is_empty(), "{:?}", s.written());
    assert!(r.writes.lock().unwrap().is_empty());
    assert!(!r.marker());
}

/// The owner's change in between wins: switched off while Kai held it on, Kai leaves it off and
/// does not switch it back; switched on again by the owner, Kai does not switch it off at the end.
#[tokio::test]
async fn the_owners_change_in_between_is_never_clobbered() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    settled(s.set_conversation(true)).await;
    assert!(r.is_enabled().await);
    r.owner_sets(false).await;
    tokio::time::timeout(T, async {
        while s.held() != Held::NotKais {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Kai saw the owner's change");
    assert!(!r.marker(), "the switch is not Kai's any more");
    // A read meanwhile does not switch it on again (the same period).
    s.hold().await.release().await;
    assert!(!r.is_enabled().await, "the owner's off stays");
    r.owner_sets(true).await;
    settled(s.set_conversation(false)).await;
    assert!(r.is_enabled().await, "the owner's on stays");
    assert_eq!(s.written(), [("IsEnabled", true)], "Kai's only write");
    assert!(!r.reader_written());
}

/// `never`: nothing is written, whatever happens.
#[tokio::test]
async fn never_switches_nothing() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r.switch(SwitchMode::Never, Duration::from_secs(5)).await;
    settled(s.set_conversation(true)).await;
    s.hold().await.release().await;
    assert!(!r.is_enabled().await);
    assert!(r.writes.lock().unwrap().is_empty());
}

/// The lock puts it back, and Kai's exit does; after the exit nothing switches it on.
#[tokio::test]
async fn the_lock_and_the_exit_put_it_back() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    settled(s.set_conversation(true)).await;
    assert!(r.is_enabled().await);
    settled(s.set_locked(true)).await;
    assert!(!r.is_enabled().await, "locked: put back");
    let hold = s.hold().await;
    assert!(!r.is_enabled().await, "no read switches it on while locked");
    hold.release().await;
    settled(s.set_locked(false)).await;
    assert!(
        r.is_enabled().await,
        "conversation mode still on in this switch's view"
    );
    s.shutdown().await;
    assert!(!r.is_enabled().await, "the exit puts it back");
    s.hold().await.release().await;
    assert!(!r.is_enabled().await, "nothing after the exit");
    assert!(!r.marker());
    assert!(!r.reader_written());
}

/// A crash leaves the marker: the next start switches a switch that still reads on off.
#[tokio::test]
async fn the_next_start_puts_back_what_a_crashed_kai_left() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let crashed = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    settled(crashed.set_conversation(true)).await;
    assert!(r.is_enabled().await && r.marker());
    std::mem::forget(crashed);
    let next = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    next.recover().await;
    assert!(!r.is_enabled().await);
    assert!(!r.marker());
    // No marker: nothing to recover, an owner's on stays.
    r.owner_sets(true).await;
    next.recover().await;
    assert!(r.is_enabled().await);
}

/// Waits (within `T`) until `f` holds.
async fn until(what: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A switch that Kai put back and does not own, with no read holding it.
async fn back_off(r: &Rig, s: &A11ySwitch) {
    until("the switch back off and not Kai's", async || {
        !r.is_enabled().await && s.held() == Held::Nothing && s.reads() == 0 && !r.marker()
    })
    .await;
}

/// A read dropped while the switch settles (lmgw's cancel, the lock stopping the call, a
/// deadline), at the switch's read and at its write: the read still ends, the switch-on that went
/// out is owned and put back, and nothing holds it after: a lock and an unlock in `read` mode do
/// not switch it on again.
#[tokio::test]
async fn a_read_dropped_while_the_switch_settles_still_ends() {
    for at in ["read", "write"] {
        let Some(r) = rig(false, None).await else {
            return;
        };
        let s = r.switch(SwitchMode::Read, Duration::from_secs(5)).await;
        let slow = Duration::from_millis(300);
        r.slow(match at {
            "read" => Delays {
                get: slow,
                ..Delays::default()
            },
            _ => Delays {
                set: slow,
                ..Delays::default()
            },
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), s.hold())
                .await
                .is_err(),
            "{at}: dropped while it settles"
        );
        r.slow(Delays::default());
        back_off(&r, &s).await;
        settled(s.set_locked(true)).await;
        settled(s.set_locked(false)).await;
        assert!(!r.is_enabled().await, "{at}: nothing holds it after");
        assert_eq!(s.reads(), 0, "{at}");
        let written = s.written();
        assert!(
            written.is_empty() || written == [("IsEnabled", true), ("IsEnabled", false)],
            "{at}: {written:?}"
        );
        assert!(!r.reader_written());
    }
}

/// A read's end, and Kai's quit, dropped while the switch is put back: it is put back all the
/// same, the marker goes, and after the quit nothing switches it on.
#[tokio::test]
async fn a_restore_dropped_half_way_still_puts_it_back() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r.switch(SwitchMode::Read, Duration::from_secs(5)).await;
    let hold = s.hold().await;
    assert!(r.is_enabled().await);
    r.slow(Delays {
        set: Duration::from_millis(300),
        ..Delays::default()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), hold.release())
            .await
            .is_err()
    );
    back_off(&r, &s).await;

    let s = r
        .switch(SwitchMode::Conversation, Duration::from_secs(5))
        .await;
    r.slow(Delays::default());
    settled(s.set_conversation(true)).await;
    assert!(r.is_enabled().await && r.marker());
    r.slow(Delays {
        set: Duration::from_millis(300),
        ..Delays::default()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), s.shutdown())
            .await
            .is_err()
    );
    until("put back at the quit", async || {
        !r.is_enabled().await && !r.marker()
    })
    .await;
    r.slow(Delays::default());
    s.hold().await.release().await;
    assert!(!r.is_enabled().await, "nothing after the quit");
}

/// The launcher's signal of Kai's own "off" comes late, after the next read switched it on
/// again (`read_window` then `ui_press`): it is Kai's own, not the owner's, so Kai still owns the
/// switch and puts it back at the second read's end. An owner's "off" after that still ends Kai's
/// ownership.
#[tokio::test]
async fn a_late_signal_of_kais_own_off_is_not_the_owners() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r.switch(SwitchMode::Read, Duration::from_secs(5)).await;
    let late = Duration::from_millis(300);
    r.slow(Delays {
        echo: late,
        ..Delays::default()
    });
    s.hold().await.release().await;
    let second = s.hold().await;
    assert!(matches!(s.held(), Held::On { .. }));
    // Every signal of the three writes has come.
    tokio::time::sleep(late * 3).await;
    assert!(
        matches!(s.held(), Held::On { .. }),
        "the late off is Kai's own: {:?}",
        s.held()
    );
    assert!(r.marker());
    second.release().await;
    assert!(!r.is_enabled().await, "put back at the second read's end");
    assert!(!r.marker());
    assert_eq!(
        s.written(),
        [
            ("IsEnabled", true),
            ("IsEnabled", false),
            ("IsEnabled", true),
            ("IsEnabled", false)
        ]
    );

    // The owner's off, signalled as late, still wins.
    let third = s.hold().await;
    tokio::time::sleep(late * 2).await;
    r.owner_sets(false).await;
    until("Kai saw the owner's off", async || {
        s.held() == Held::NotKais
    })
    .await;
    r.owner_sets(true).await;
    third.release().await;
    tokio::time::sleep(late * 2).await;
    assert!(r.is_enabled().await, "the owner's on stays");
}

fn win() -> Target {
    Target {
        pid: std::process::id(),
        title: "Kai Test".into(),
    }
}

async fn atspi_with(r: &Rig, switch: A11ySwitch) -> Atspi {
    Atspi {
        bus: SessionBus::connected(r.session.connect().await, T),
        timeout: T,
        concurrency: 16,
        switch: Some(switch),
    }
}

/// The read waits for its app's announcement after Kai switched accessibility on (no sleep: it
/// goes on at the announcement, long before `accessibility.wait_ms`), reads, and puts the switch
/// back.
#[tokio::test]
async fn a_read_waits_for_its_app_to_join_then_puts_the_switch_back() {
    let Some(r) = rig(false, Some(Duration::from_millis(400))).await else {
        return;
    };
    let s = r.switch(SwitchMode::Read, Duration::from_secs(20)).await;
    let a = atspi_with(&r, s.clone()).await;
    let began = Instant::now();
    let tree = a.tree(&win()).await.unwrap();
    let took = began.elapsed();
    assert_eq!(tree.root.name, "Kai Test");
    assert!(
        took >= Duration::from_millis(400) && took < Duration::from_secs(5),
        "went on at the announcement: {took:?}"
    );
    assert!(!r.is_enabled().await, "put back after the read");
    assert_eq!(s.written(), [("IsEnabled", true), ("IsEnabled", false)]);
    assert!(!r.reader_written());
}

/// An app that never joins: the read gives up at `accessibility.wait_ms`, its error says Kai
/// switched it on and waited, and the switch is back off.
#[tokio::test]
async fn an_app_that_never_joins_is_said_with_the_wait() {
    let Some(r) = rig(false, None).await else {
        return;
    };
    let s = r.switch(SwitchMode::Read, Duration::from_millis(600)).await;
    let a = atspi_with(&r, s.clone()).await;
    let began = Instant::now();
    let e = a.tree(&win()).await.unwrap_err().to_string();
    assert!(began.elapsed() >= Duration::from_millis(600));
    assert_eq!(
        e,
        "the window's app is not on the accessibility bus; accessibility is on (org.a11y.Status \
         IsEnabled; Kai switched it on and the app did not join within 600 ms \
         (accessibility.wait_ms)): Electron and Chromium apps join only when started with \
         --force-renderer-accessibility"
    );
    assert!(!r.is_enabled().await);
}

/// With the switch at `never` the error names the setting.
#[tokio::test]
async fn never_is_named_in_the_error() {
    let Some(r) = rig(false, Some(Duration::ZERO)).await else {
        return;
    };
    let a = atspi_with(
        &r,
        r.switch(SwitchMode::Never, Duration::from_secs(5)).await,
    )
    .await;
    let e = a.tree(&win()).await.unwrap_err().to_string();
    assert!(
        e.contains(
            "ScreenReaderEnabled is not needed), which Kai does not switch \
             (accessibility.switch = \"never\");"
        ),
        "{e}"
    );
}
