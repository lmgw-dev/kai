//! The session and media providers' actions against fakes of the services on private buses (a
//! `dbus-daemon` for the session and one for the system, activating nothing) and fake programs
//! (`pactl`, `wl-copy`) in a temp directory under target/. The session's own services are never
//! touched: nothing here locks, suspends, mutes, pauses or dims the machine the tests run on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kai_desktop::caps::Failure;
use kai_desktop::caps::audio::Audio;
use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::media::{Control, Media};
use kai_desktop::caps::notify::Notify;
use kai_desktop::caps::session::Session;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::bus::{PrivateBus, temp_dir};
use crate::desktop::SystemBus;
use crate::desktop::fdo::audio::Pactl;
use crate::desktop::fdo::clipboard::WlCopy;
use crate::desktop::fdo::media::Mpris;
use crate::desktop::fdo::notify::FdoNotify;
use crate::desktop::fdo::session::{Logind, find_backlight, percent_of, raw_step};
use crate::desktop::plasma::night_light::NightLightControl;
use crate::desktop::plasma::session::{PlasmaNotify, PlasmaSession};
use crate::host::registry::SessionBus;

const T: Duration = Duration::from_secs(3);

type Log = Arc<Mutex<Vec<String>>>;

fn log_of(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

async fn wait_for(log: &Log, what: &str) {
    for _ in 0..200 {
        if log.lock().unwrap().iter().any(|l| l.starts_with(what)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no {what:?} in {:?}", log_of(log));
}

// --- the fakes -----------------------------------------------------------------------------

struct Saver(Log);

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl Saver {
    #[zbus(name = "Lock")]
    fn lock(&self) {
        self.0.lock().unwrap().push("saver.lock".into());
    }
}

/// What the fakes of the inhibiting services do on request: answer `Inhibit` late, refuse the
/// release.
#[derive(Default)]
struct Knobs {
    inhibit_delay_ms: AtomicU64,
    fail_release: AtomicBool,
}

impl Knobs {
    async fn delay(&self) {
        let ms = self.inhibit_delay_ms.load(Ordering::SeqCst);
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }
    fn release(&self) -> zbus::fdo::Result<()> {
        if self.fail_release.load(Ordering::SeqCst) {
            Err(zbus::fdo::Error::Failed("refused".into()))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct NotesState {
    /// The owner's own do not disturb (not Kai's).
    manual: bool,
    cookies: Vec<u32>,
}

struct Notes(Log, Arc<Mutex<NotesState>>, Arc<Knobs>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notes {
    #[allow(clippy::too_many_arguments)]
    #[zbus(name = "Notify")]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire: i32,
    ) -> u32 {
        // The urgency is logged where it was sent, so the lines of the plain cases stay as they are.
        let urgency = hints
            .get("urgency")
            .and_then(|u| u8::try_from(u).ok())
            .map(|u| format!("|urgency {u}"))
            .unwrap_or_default();
        self.0.lock().unwrap().push(format!(
            "notify {app_name}|{replaces_id}|{summary}|{body}|{}|{expire}{urgency}",
            actions.join(",")
        ));
        42
    }
    #[zbus(name = "CloseNotification")]
    fn close(&self, id: u32) {
        self.0.lock().unwrap().push(format!("close {id}"));
    }
    #[zbus(name = "Inhibit")]
    async fn inhibit(
        &self,
        app: String,
        _reason: String,
        _hints: HashMap<String, OwnedValue>,
    ) -> u32 {
        // The server acts first and answers after the delay.
        self.1.lock().unwrap().cookies.push(7);
        self.0.lock().unwrap().push(format!("inhibit {app}"));
        self.2.delay().await;
        7
    }
    #[zbus(name = "UnInhibit")]
    fn uninhibit(&self, cookie: u32) -> zbus::fdo::Result<()> {
        self.2.release()?;
        self.1.lock().unwrap().cookies.retain(|c| *c != cookie);
        self.0.lock().unwrap().push(format!("uninhibit {cookie}"));
        Ok(())
    }
    #[zbus(property, name = "Inhibited")]
    fn inhibited(&self) -> bool {
        let s = self.1.lock().unwrap();
        s.manual || !s.cookies.is_empty()
    }
}

struct Player {
    log: Log,
    name: &'static str,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(name = "PlayPause")]
    fn play_pause(&self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} PlayPause", self.name));
    }
    #[zbus(name = "Next")]
    fn next(&self) {
        self.log.lock().unwrap().push(format!("{} Next", self.name));
    }
    #[zbus(name = "Previous")]
    fn previous(&self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} Previous", self.name));
    }
    #[zbus(property, name = "PlaybackStatus")]
    fn status(&self) -> String {
        if self.name == "spotify" {
            "Playing"
        } else {
            "Paused"
        }
        .into()
    }
}

struct Root(&'static str);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property, name = "Identity")]
    fn identity(&self) -> String {
        self.0.into()
    }
}

struct Displays;

#[zbus::interface(name = "org.kde.ScreenBrightness")]
impl Displays {
    #[zbus(property, name = "DisplaysDBusNames")]
    fn names(&self) -> Vec<String> {
        vec!["display0".into(), "display1".into()]
    }
}

struct Display(Log, &'static str, i32);

#[zbus::interface(name = "org.kde.ScreenBrightness.Display")]
impl Display {
    #[zbus(property, name = "MaxBrightness")]
    fn max(&self) -> i32 {
        self.2
    }
    #[zbus(name = "SetBrightness")]
    fn set(&self, brightness: i32, flags: u32) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} brightness {brightness} flags {flags}", self.1));
    }
}

struct Night {
    knobs: Arc<Knobs>,
    log: Log,
    enabled: bool,
    cookies: Mutex<Vec<u32>>,
}

#[zbus::interface(name = "org.kde.KWin.NightLight")]
impl Night {
    #[zbus(property, name = "available")]
    fn available(&self) -> bool {
        true
    }
    #[zbus(property, name = "enabled")]
    fn enabled(&self) -> bool {
        self.enabled
    }
    #[zbus(property, name = "inhibited")]
    fn inhibited(&self) -> bool {
        !self.cookies.lock().unwrap().is_empty()
    }
    #[zbus(name = "inhibit")]
    async fn inhibit(&self) -> u32 {
        self.cookies.lock().unwrap().push(3);
        self.log.lock().unwrap().push("night inhibit".into());
        self.knobs.delay().await;
        3
    }
    #[zbus(name = "uninhibit")]
    fn uninhibit(&self, cookie: u32) -> zbus::fdo::Result<()> {
        self.knobs.release()?;
        self.cookies.lock().unwrap().retain(|c| *c != cookie);
        self.log
            .lock()
            .unwrap()
            .push(format!("night uninhibit {cookie}"));
        Ok(())
    }
}

struct Manager(Log);

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    #[zbus(name = "Suspend")]
    fn suspend(&self, interactive: bool) {
        self.0
            .lock()
            .unwrap()
            .push(format!("suspend {interactive}"));
    }
}

struct User;

#[zbus::interface(name = "org.freedesktop.login1.User")]
impl User {
    #[zbus(property, name = "Display")]
    fn display(&self) -> (String, OwnedObjectPath) {
        (
            "2".into(),
            OwnedObjectPath::try_from("/org/freedesktop/login1/session/_32").unwrap(),
        )
    }
}

struct LoginSession(Log);

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    #[zbus(name = "Lock")]
    fn lock(&self) {
        self.0.lock().unwrap().push("login.lock".into());
    }
    #[zbus(name = "SetBrightness")]
    fn set_brightness(&self, subsystem: String, name: String, value: u32) {
        self.0
            .lock()
            .unwrap()
            .push(format!("login.brightness {subsystem} {name} {value}"));
    }
}

struct World {
    session: PrivateBus,
    system: PrivateBus,
    log: Log,
    notes_state: Arc<Mutex<NotesState>>,
    knobs: Arc<Knobs>,
    /// The notification server's connection: it sends the signals.
    notes: zbus::Connection,
    _held: Vec<zbus::Connection>,
    dir: tempfile::TempDir,
}

async fn serve<I: zbus::object_server::Interface>(
    bus: &PrivateBus,
    path: &str,
    name: Option<&str>,
    iface: I,
) -> zbus::Connection {
    let c = bus.connect().await;
    c.object_server().at(path, iface).await.unwrap();
    if let Some(n) = name {
        c.request_name(n).await.unwrap();
    }
    c
}

async fn world(night_enabled: bool) -> Option<World> {
    let session = PrivateBus::start()?;
    let system = PrivateBus::start()?;
    let log = Log::default();
    let notes_state = Arc::new(Mutex::new(NotesState::default()));
    let knobs = Arc::new(Knobs::default());
    let mut held = vec![
        serve(
            &session,
            "/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            Saver(log.clone()),
        )
        .await,
    ];
    let notes = serve(
        &session,
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        Notes(log.clone(), notes_state.clone(), knobs.clone()),
    )
    .await;
    let brightness = serve(
        &session,
        "/org/kde/ScreenBrightness",
        Some("org.kde.ScreenBrightness"),
        Displays,
    )
    .await;
    for (name, max) in [("display0", 100), ("display1", 255)] {
        brightness
            .object_server()
            .at(
                format!("/org/kde/ScreenBrightness/{name}"),
                Display(log.clone(), name, max),
            )
            .await
            .unwrap();
    }
    held.push(brightness);
    held.push(
        serve(
            &session,
            "/org/kde/KWin/NightLight",
            Some("org.kde.KWin"),
            Night {
                knobs: knobs.clone(),
                log: log.clone(),
                enabled: night_enabled,
                cookies: Mutex::default(),
            },
        )
        .await,
    );
    for (name, status) in [("spotify", "Spotify"), ("vlc", "VLC")] {
        let c = serve(
            &session,
            "/org/mpris/MediaPlayer2",
            None,
            Player {
                log: log.clone(),
                name,
            },
        )
        .await;
        c.object_server()
            .at("/org/mpris/MediaPlayer2", Root(status))
            .await
            .unwrap();
        c.request_name(format!("org.mpris.MediaPlayer2.{name}"))
            .await
            .unwrap();
        held.push(c);
    }
    let login = serve(
        &system,
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1"),
        Manager(log.clone()),
    )
    .await;
    login
        .object_server()
        .at("/org/freedesktop/login1/user/self", User)
        .await
        .unwrap();
    login
        .object_server()
        .at(
            "/org/freedesktop/login1/session/_32",
            LoginSession(log.clone()),
        )
        .await
        .unwrap();
    held.push(login);
    Some(World {
        session,
        system,
        log,
        notes_state,
        knobs,
        notes,
        _held: held,
        dir: temp_dir(),
    })
}

impl World {
    async fn session_bus(&self) -> SessionBus {
        SessionBus::connected(self.session.connect().await, T)
    }

    fn system_bus(&self) -> SystemBus {
        SystemBus::new(Some(self.system.address.clone()), T)
    }

    async fn plasma_session(&self) -> PlasmaSession {
        let bus = self.session_bus().await;
        PlasmaSession {
            night: NightLightControl::new(bus.clone(), T),
            bus,
            system: Some(self.system_bus()),
            timeout: T,
            features: kai_desktop::caps::session::Features {
                brightness: true,
                night_light: true,
            },
        }
    }
}

// --- Session -------------------------------------------------------------------------------

#[tokio::test]
async fn plasma_locks_suspends_and_dims_every_screen() {
    let Some(w) = world(true).await else { return };
    let s = w.plasma_session().await;
    s.lock_screen().await.unwrap();
    s.suspend().await.unwrap();
    // 40 % of 100 and of 255: each display in its own range, the answer the first one's.
    assert_eq!(s.set_brightness(40).await.unwrap(), 40);
    // Never fully dark: 0 % is the lowest step.
    assert_eq!(s.set_brightness(0).await.unwrap(), 1);
    assert_eq!(
        log_of(&w.log),
        [
            "saver.lock",
            "suspend false",
            "display0 brightness 40 flags 0",
            "display1 brightness 102 flags 0",
            "display0 brightness 1 flags 0",
            "display1 brightness 1 flags 0",
        ]
    );
}

#[tokio::test]
async fn night_light_is_paused_and_resumed_with_kais_own_cookie() {
    let Some(w) = world(true).await else { return };
    let s = w.plasma_session().await;
    assert!(!s.set_night_light(false).await.unwrap().on);
    // A second pause asks KWin for nothing more.
    assert!(!s.set_night_light(false).await.unwrap().on);
    assert!(s.set_night_light(true).await.unwrap().on);
    assert_eq!(log_of(&w.log), ["night inhibit", "night uninhibit 3"]);
}

#[tokio::test]
async fn night_light_switched_off_in_the_settings_cannot_be_resumed() {
    let Some(w) = world(false).await else { return };
    let s = w.plasma_session().await;
    match s.set_night_light(true).await {
        Err(Failure::Refused(m)) => assert!(m.contains("system settings"), "{m}"),
        other => panic!("{:?}", other.map(|n| n.on)),
    }
}

#[tokio::test]
async fn logind_locks_suspends_and_sets_the_first_firmware_backlight() {
    let Some(w) = world(true).await else { return };
    let dir = w.dir.path().join("backlight");
    for (name, kind, max) in [("acpi_video0", "firmware", 10), ("intel", "raw", 1000)] {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), kind).unwrap();
        std::fs::write(d.join("max_brightness"), format!("{max}\n")).unwrap();
    }
    let l = Logind {
        system: w.system_bus(),
        timeout: T,
        backlight: dir.clone(),
    };
    assert!(l.features().brightness && !l.features().night_light);
    l.lock_screen().await.unwrap();
    l.suspend().await.unwrap();
    assert_eq!(l.set_brightness(30).await.unwrap(), 30);
    assert_eq!(
        log_of(&w.log),
        [
            "login.lock",
            "suspend false",
            "login.brightness backlight acpi_video0 3"
        ]
    );
    assert!(matches!(
        l.set_night_light(true).await,
        Err(Failure::Refused(_))
    ));
    let none = Logind {
        backlight: w.dir.path().join("missing"),
        ..l
    };
    assert!(!none.features().brightness);
    assert!(matches!(
        none.set_brightness(10).await,
        Err(Failure::Refused(_))
    ));
}

#[test]
fn a_backlight_step_is_never_zero_and_the_percent_is_what_it_really_is() {
    assert_eq!(raw_step(0, 100), 1);
    assert_eq!(raw_step(100, 255), 255);
    assert_eq!(raw_step(50, 3), 2);
    assert_eq!(raw_step(200, 100), 100, "capped at the device's highest");
    assert_eq!(percent_of(1, 255), 0);
    assert_eq!(percent_of(255, 255), 100);
    assert_eq!(percent_of(1, 0), 0);
    assert_eq!(
        find_backlight(std::path::Path::new("/nonexistent/kai")),
        None
    );
}

// --- Media ---------------------------------------------------------------------------------

#[tokio::test]
async fn mpris_controls_reach_the_player_by_its_bus_name_only() {
    let Some(w) = world(true).await else { return };
    let m = Mpris {
        bus: w.session_bus().await,
        timeout: T,
    };
    let players = m.players().await.unwrap();
    let ids: Vec<&str> = players.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "org.mpris.MediaPlayer2.spotify",
            "org.mpris.MediaPlayer2.vlc"
        ]
    );
    m.control("org.mpris.MediaPlayer2.vlc", Control::Next)
        .await
        .unwrap();
    m.control("org.mpris.MediaPlayer2.spotify", Control::PlayPause)
        .await
        .unwrap();
    m.control("org.mpris.MediaPlayer2.spotify", Control::Previous)
        .await
        .unwrap();
    assert_eq!(
        log_of(&w.log),
        ["vlc Next", "spotify PlayPause", "spotify Previous"]
    );
    // A bus name that is not a player is refused before it is called.
    assert!(matches!(
        m.control("org.freedesktop.login1", Control::Next).await,
        Err(Failure::Refused(_))
    ));
}

// --- Notify --------------------------------------------------------------------------------

#[tokio::test]
async fn do_not_disturb_is_inhibited_by_kai_and_released_by_kai_only() {
    let Some(w) = world(true).await else { return };
    let n = PlasmaNotify::new(w.session_bus().await, T);
    assert!(n.can_dnd());
    assert!(n.set_dnd(true).await.unwrap().on);
    // Once more: one inhibition.
    assert!(n.set_dnd(true).await.unwrap().on);
    assert!(!n.set_dnd(false).await.unwrap().on);
    // Off when nothing of Kai's is held asks the server for nothing.
    assert!(!n.set_dnd(false).await.unwrap().on);
    // The owner's own do not disturb is not Kai's to switch off.
    w.notes_state.lock().unwrap().manual = true;
    assert!(n.set_dnd(true).await.unwrap().on);
    assert!(n.set_dnd(false).await.unwrap().on);
    assert_eq!(
        log_of(&w.log),
        [
            "inhibit dev.rootshell.kai",
            "uninhibit 7",
            "inhibit dev.rootshell.kai",
            "uninhibit 7"
        ]
    );
}

#[tokio::test]
async fn a_cancelled_inhibit_still_leaves_its_cookie_to_release() {
    let Some(w) = world(true).await else { return };
    w.knobs.inhibit_delay_ms.store(400, Ordering::SeqCst);
    let n = Arc::new(PlasmaNotify::new(w.session_bus().await, T));
    let night = Arc::new(w.plasma_session().await);
    // lmgw cancels both calls after the services acted and before they answered.
    let a = {
        let n = n.clone();
        tokio::spawn(async move { n.set_dnd(true).await })
    };
    let b = {
        let s = night.clone();
        tokio::spawn(async move { s.set_night_light(false).await })
    };
    wait_for(&w.log, "inhibit").await;
    wait_for(&w.log, "night inhibit").await;
    a.abort();
    b.abort();
    let _ = (a.await, b.await);
    w.knobs.inhibit_delay_ms.store(0, Ordering::SeqCst);
    // The next call sees the cookie the late answers carried and releases it.
    assert!(!n.set_dnd(false).await.unwrap().on);
    assert!(night.set_night_light(true).await.unwrap().on);
    assert_eq!(
        log_of(&w.log),
        [
            "inhibit dev.rootshell.kai",
            "night inhibit",
            "uninhibit 7",
            "night uninhibit 3"
        ]
    );
}

#[tokio::test]
async fn an_inhibit_answered_after_the_timeout_is_kept_and_released_later() {
    let Some(w) = world(true).await else { return };
    w.knobs.inhibit_delay_ms.store(400, Ordering::SeqCst);
    let n = PlasmaNotify::new(w.session_bus().await, Duration::from_millis(100));
    let err = n.set_dnd(true).await.expect_err("late");
    assert!(format!("{err:?}").contains("dbus.timeout_s"), "{err:?}");
    w.knobs.inhibit_delay_ms.store(0, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(w.notes_state.lock().unwrap().cookies == [7]);
    assert!(!n.set_dnd(false).await.unwrap().on);
    assert!(w.notes_state.lock().unwrap().cookies.is_empty());
}

#[tokio::test]
async fn a_release_the_service_refuses_keeps_the_cookie() {
    let Some(w) = world(true).await else { return };
    let n = PlasmaNotify::new(w.session_bus().await, T);
    let s = w.plasma_session().await;
    assert!(n.set_dnd(true).await.unwrap().on);
    assert!(!s.set_night_light(false).await.unwrap().on);
    w.knobs.fail_release.store(true, Ordering::SeqCst);
    assert!(n.set_dnd(false).await.is_err());
    assert!(s.set_night_light(true).await.is_err());
    // Both are still held, and the owner's view says so.
    assert!(n.dnd().await.unwrap());
    w.knobs.fail_release.store(false, Ordering::SeqCst);
    assert!(!n.set_dnd(false).await.unwrap().on);
    assert!(s.set_night_light(true).await.unwrap().on);
    assert_eq!(
        log_of(&w.log),
        [
            "inhibit dev.rootshell.kai",
            "night inhibit",
            "uninhibit 7",
            "night uninhibit 3"
        ]
    );
}

#[tokio::test]
async fn the_freedesktop_server_shows_but_has_no_do_not_disturb() {
    let Some(w) = world(true).await else { return };
    let n = FdoNotify {
        bus: w.session_bus().await,
        timeout: T,
    };
    assert!(!n.can_dnd());
    assert!(matches!(n.dnd().await, Err(Failure::Refused(_))));
    assert!(matches!(n.set_dnd(true).await, Err(Failure::Refused(_))));
    n.show("Build done", "all <green>").await.unwrap();
    assert_eq!(
        log_of(&w.log),
        ["notify Kai|0|Build done|all &lt;green&gt;||-1"]
    );
}

async fn emit(w: &World, id: u32, key: &str) {
    w.notes
        .emit_signal(
            None::<&str>,
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "ActionInvoked",
            &(id, key),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn ask_answers_with_the_clicked_choice_and_ignores_a_forged_signal() {
    let Some(w) = world(true).await else { return };
    let n = Arc::new(FdoNotify {
        bus: w.session_bus().await,
        timeout: T,
    });
    let choices = vec!["Yes".to_owned(), "Not now".to_owned()];
    let task = {
        let (n, choices) = (n.clone(), choices.clone());
        tokio::spawn(async move { n.ask("Shall I deploy?", &choices).await })
    };
    wait_for(&w.log, "notify").await;
    assert_eq!(
        log_of(&w.log)[0],
        "notify Kai|0|Shall I deploy?||0,Yes,1,Not now|0",
        "buttons by position, no expiry"
    );
    // Any peer of the session can send the signal; only the server's counts.
    let forger = w.session.connect().await;
    forger
        .emit_signal(
            None::<&str>,
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "ActionInvoked",
            &(42u32, "0"),
        )
        .await
        .unwrap();
    // Another notification's answer, and a key that is no choice, do not count either.
    emit(&w, 41, "0").await;
    emit(&w, 42, "9").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    emit(&w, 42, "1").await;
    assert_eq!(task.await.unwrap().unwrap().choice, Some(1));
}

#[tokio::test]
async fn a_dismissed_question_has_no_answer_and_a_dropped_one_is_closed() {
    let Some(w) = world(true).await else { return };
    let n = Arc::new(FdoNotify {
        bus: w.session_bus().await,
        timeout: T,
    });
    let choices = vec!["Yes".to_owned()];
    let task = {
        let (n, choices) = (n.clone(), choices.clone());
        tokio::spawn(async move { n.ask("Sure?", &choices).await })
    };
    wait_for(&w.log, "notify").await;
    w.notes
        .emit_signal(
            None::<&str>,
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "NotificationClosed",
            &(42u32, 2u32),
        )
        .await
        .unwrap();
    assert_eq!(task.await.unwrap().unwrap().choice, None);
    assert!(!log_of(&w.log).iter().any(|l| l.starts_with("close")));

    // lmgw cancelling the call drops the wait: the question leaves the screen.
    let task = {
        let (n, choices) = (n.clone(), choices.clone());
        tokio::spawn(async move { n.ask("Sure again?", &choices).await })
    };
    wait_for(&w.log, "notify Kai|0|Sure again?").await;
    task.abort();
    let _ = task.await;
    wait_for(&w.log, "close 42").await;
}

#[tokio::test]
async fn ask_under_do_not_disturb_is_critical_and_says_so() {
    let Some(w) = world(true).await else { return };
    let n = Arc::new(PlasmaNotify::new(w.session_bus().await, T));
    let choices = vec!["Yes".to_owned()];
    // Without do not disturb: no urgency hint, nothing to say.
    let task = {
        let (n, c) = (n.clone(), choices.clone());
        tokio::spawn(async move { n.ask("Calm?", &c).await })
    };
    wait_for(&w.log, "notify Kai|0|Calm?").await;
    emit(&w, 42, "0").await;
    let asked = task.await.unwrap().unwrap();
    assert!(!asked.under_dnd);
    // Kai's own do not disturb, and the owner's.
    for kai_own in [true, false] {
        if kai_own {
            n.set_dnd(true).await.unwrap();
        } else {
            n.set_dnd(false).await.unwrap();
            w.notes_state.lock().unwrap().manual = true;
        }
        let q = format!("Shall I {kai_own}?");
        let task = {
            let (n, c, q) = (n.clone(), choices.clone(), q.clone());
            tokio::spawn(async move { n.ask(&q, &c).await })
        };
        wait_for(&w.log, &format!("notify Kai|0|{q}")).await;
        emit(&w, 42, "0").await;
        let asked = task.await.unwrap().unwrap();
        assert_eq!((asked.choice, asked.under_dnd), (Some(0), true));
    }
    let log = log_of(&w.log);
    assert!(
        log.contains(&"notify Kai|0|Calm?||0,Yes|0".to_owned()),
        "{log:?}"
    );
    for q in ["true", "false"] {
        let line = format!("notify Kai|0|Shall I {q}?||0,Yes|0|urgency 2");
        assert!(log.contains(&line), "{line} in {log:?}");
    }
}

#[tokio::test]
async fn ask_ends_with_an_error_when_the_server_that_showed_it_goes_away() {
    let Some(w) = world(true).await else { return };
    let n = Arc::new(FdoNotify {
        bus: w.session_bus().await,
        timeout: T,
    });
    let choices = vec!["Yes".to_owned()];
    let task = {
        let (n, c) = (n.clone(), choices.clone());
        tokio::spawn(async move { n.ask("Still there?", &c).await })
    };
    wait_for(&w.log, "notify").await;
    // Plasma's shell restarts: its connection goes, with the notification.
    w.notes.close().await.unwrap();
    let err = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the wait ended")
        .unwrap()
        .expect_err("an error");
    assert!(format!("{err:?}").contains("went away"), "{err:?}");
}

// --- Audio and Clipboard (fake programs) ---------------------------------------------------

/// A script. A program written just now can be "text file busy" for a moment when another test
/// forks while it is open for writing, so it is run once with `--kai-selftest` (the script exits
/// at once) until the system lets it run.
fn fake_program(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = --kai-selftest ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(&path)
            .arg("--kai-selftest")
            .status()
        {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => break,
        }
    }
    path
}

#[tokio::test]
async fn pactl_lists_outputs_and_changes_the_one_it_was_given() {
    let dir = temp_dir();
    let log = dir.path().join("args");
    let program = fake_program(
        dir.path(),
        "pactl",
        &format!(
            r#"echo "$@" >> {log}
case "$1" in
  get-default-sink) echo "alsa.speakers" ;;
  -f) echo '[{{"name":"alsa.speakers","description":"Speakers"}},{{"name":"alsa.phones","description":""}}]' ;;
  set-sink-mute) [ "$2" = "bad" ] && {{ echo nope >&2; exit 1; }}; exit 0 ;;
esac"#,
            log = log.display()
        ),
    );
    let a = Pactl {
        program,
        timeout: T,
    };
    let outs = a.outputs().await.unwrap();
    assert_eq!(
        outs.iter()
            .map(|d| (d.id.as_str(), d.description.as_str(), d.default))
            .collect::<Vec<_>>(),
        [
            ("alsa.speakers", "Speakers", true),
            ("alsa.phones", "alsa.phones", false)
        ]
    );
    a.set_volume("alsa.phones", 35).await.unwrap();
    a.set_mute("alsa.speakers", true).await.unwrap();
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains("set-sink-volume alsa.phones 35%\n"), "{args}");
    assert!(args.contains("set-sink-mute alsa.speakers 1\n"), "{args}");
    match a.set_mute("bad", false).await {
        Err(Failure::Failed(m)) => assert!(m.contains("nope"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn wl_copy_gets_the_text_on_its_standard_input_never_as_an_argument() {
    let dir = temp_dir();
    let (stdin, argv) = (dir.path().join("stdin"), dir.path().join("argv"));
    let program = fake_program(
        dir.path(),
        "wl-copy",
        &format!(
            r#"echo "$@" > {argv}
cat > {stdin}"#,
            argv = argv.display(),
            stdin = stdin.display()
        ),
    );
    let c = WlCopy {
        program,
        timeout: T,
    };
    c.set("secret text\nsecond line").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&stdin).unwrap(),
        "secret text\nsecond line"
    );
    let argv = std::fs::read_to_string(&argv).unwrap();
    assert!(!argv.contains("secret"), "{argv}");
    let failing = WlCopy {
        program: fake_program(dir.path(), "wl-copy-fail", "cat > /dev/null; exit 3"),
        timeout: T,
    };
    assert!(matches!(
        failing.set("x").await,
        Err(Failure::Failed(m)) if m.contains("wl-copy failed")
    ));
}
