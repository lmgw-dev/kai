//! WP15's providers against fakes on private buses (KWin's ScreenShot2, Klipper, the AT-SPI bus
//! with an app's tree, a `baloosearch6` stand-in, a recent list in a temp directory), and the fake
//! capabilities the tools' tests (`read::tests`) and the contract test use. The session's own
//! KWin, clipboard, accessibility bus and index are never touched.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kai_desktop::caps::accessibility::{
    Accessibility, Found as Element, Pressed, Query, Target, Tree,
};
use kai_desktop::caps::capture::{Capture, Pixels};
use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::search::{Found, Recent, Recents, Search};
use kai_desktop::caps::{Capability, Failure, Fut};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

use super::bus::{PrivateBus, kwin, temp_dir};
use crate::desktop::fdo::atspi::{Atspi, role_name};
use crate::desktop::plasma::baloo::Baloo;
use crate::desktop::plasma::capture::KwinCapture;
use crate::desktop::plasma::klipper::Klipper;
use crate::desktop::probe::ProviderName;
use crate::desktop::read::ReadEnv;
use crate::desktop::settings::DesktopConfig;
use crate::desktop::tools::Services;
use crate::desktop::{DesktopTools, Env, SystemBus};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Changes, Entry, Provider, SessionBus};

const T: Duration = Duration::from_secs(2);

// ---- Fake capabilities for the tools' tests ----

/// Captures of a fixed size: a window's 4x2, a screen's 6x3; the ids and outputs asked for are
/// recorded; `fail` answers every capture with it.
#[derive(Clone, Default)]
pub struct FakeCapture {
    pub asked: Arc<Mutex<Vec<String>>>,
    pub fail: Option<Failure>,
}

pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Pixels {
    Pixels {
        width,
        height,
        rgba: rgba.repeat((width * height) as usize),
    }
}

impl Capture for FakeCapture {
    fn window<'a>(&'a self, id: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap()
                .push(format!("window {}", id.unwrap_or("active")));
            match &self.fail {
                Some(f) => Err(f.clone()),
                None => Ok(solid(4, 2, [10, 20, 30, 255])),
            }
        })
    }

    fn screen<'a>(&'a self, output: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap()
                .push(format!("screen {}", output.unwrap_or("active")));
            match &self.fail {
                Some(f) => Err(f.clone()),
                None => Ok(solid(6, 3, [1, 2, 3, 255])),
            }
        })
    }
}

/// A fixed tree and selection; the targets asked for are recorded.
#[derive(Clone, Default)]
pub struct FakeAccess {
    pub tree: Tree,
    pub selection: Vec<String>,
    pub asked: Arc<Mutex<Vec<Target>>>,
    /// What `find` answers (none: a refusal), the finds asked for, and the presses.
    pub found: Option<Element>,
    pub finds: Arc<Mutex<Vec<(Target, Query)>>>,
    pub presses: Arc<Mutex<Vec<Element>>>,
}

impl Accessibility for FakeAccess {
    fn tree<'a>(&'a self, target: &'a Target) -> Fut<'a, Tree> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(target.clone());
            Ok(self.tree.clone())
        })
    }

    fn find<'a>(&'a self, target: &'a Target, query: &'a Query) -> Fut<'a, Element> {
        Box::pin(async move {
            self.finds
                .lock()
                .unwrap()
                .push((target.clone(), query.clone()));
            self.found
                .clone()
                .ok_or_else(|| Failure::Refused("no pressable element matches".into()))
        })
    }

    fn press<'a>(&'a self, found: &'a Element) -> Fut<'a, Pressed> {
        Box::pin(async move {
            self.presses.lock().unwrap().push(found.clone());
            Ok(Pressed {
                role: found.role.clone(),
                name: found.name.clone(),
                action: "click".into(),
            })
        })
    }

    fn selection<'a>(&'a self, target: &'a Target) -> Fut<'a, Vec<String>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(target.clone());
            Ok(self.selection.clone())
        })
    }
}

/// A search of fixed answers; the queries and limits asked for are recorded.
#[derive(Clone, Default)]
pub struct FakeSearch {
    pub indexes: bool,
    pub found: Found,
    pub recent: Vec<Recent>,
    pub recent_private: usize,
    pub asked: Arc<Mutex<Vec<(String, usize)>>>,
}

impl Search for FakeSearch {
    fn indexes(&self) -> bool {
        self.indexes
    }

    fn find<'a>(&'a self, query: &'a str, limit: usize) -> Fut<'a, Found> {
        Box::pin(async move {
            self.asked.lock().unwrap().push((query.to_owned(), limit));
            Ok(self.found.clone())
        })
    }

    fn recent(&self) -> Fut<'_, Recents> {
        Box::pin(async move {
            Ok(Recents {
                entries: self.recent.clone(),
                private_skipped: self.recent_private,
            })
        })
    }
}

pub struct FakeClipboard(pub String, pub Vec<String>);

impl Clipboard for FakeClipboard {
    fn set<'a>(&'a self, _: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn reads(&self) -> bool {
        true
    }

    fn text(&self) -> Fut<'_, String> {
        Box::pin(async move { Ok(self.0.clone()) })
    }

    fn history(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async move { Ok(self.1.clone()) })
    }
}

/// The read capabilities, faked (with an index), over `base`.
pub fn with_reads(base: Services) -> Services {
    Services {
        capture: Some(Arc::new(FakeCapture::default())),
        accessibility: Some(Arc::new(FakeAccess::default())),
        search: Some(Arc::new(FakeSearch {
            indexes: true,
            ..Default::default()
        })),
        clipboard: Some(Arc::new(FakeClipboard("x".into(), vec![]))),
        find_limit: 100,
        ..base
    }
}

// ---- KWin's ScreenShot2 ----

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShotMode {
    Answer,
    NoGrant,
}

/// One capture call: its method, its target, its options as `key=bool`.
pub type ShotCall = (String, String, Vec<String>);

pub struct FakeScreenShot {
    pub mode: Arc<Mutex<ShotMode>>,
    /// Each call: its method, its target and its options' keys with their values.
    pub calls: Arc<Mutex<Vec<ShotCall>>>,
}

/// A 3x2 ARGB32 premultiplied image at stride 16: a row of red, green, blue; a row of transparent
/// black, white and half-transparent white.
const RAW: [u8; 32] = [
    0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 7, 7, 7, 7, //
    0, 0, 0, 0, 255, 255, 255, 255, 128, 128, 128, 128, 7, 7, 7, 7,
];

impl FakeScreenShot {
    fn answer(
        &self,
        method: &str,
        target: &str,
        options: HashMap<String, OwnedValue>,
        pipe: OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, ShotError> {
        let mut opts: Vec<String> = options
            .iter()
            .map(|(k, v)| format!("{k}={}", bool::try_from(v).unwrap_or_default()))
            .collect();
        opts.sort();
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), target.to_owned(), opts));
        if *self.mode.lock().unwrap() == ShotMode::NoGrant {
            return Err(ShotError::NoAuthorized("not authorized".into()));
        }
        // Written after the answer, from a thread of its own, as KWin does.
        let fd: std::os::fd::OwnedFd = pipe.into();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut f = std::fs::File::from(fd);
            let _ = f.write_all(&RAW);
        });
        Ok(HashMap::from([
            ("width".to_owned(), OwnedValue::from(3u32)),
            ("height".to_owned(), OwnedValue::from(2u32)),
            ("stride".to_owned(), OwnedValue::from(16u32)),
            ("format".to_owned(), OwnedValue::from(6u32)),
            (
                "type".to_owned(),
                OwnedValue::try_from(Value::from("raw")).unwrap(),
            ),
        ]))
    }
}

/// KWin's own error names, which `zbus::fdo::Error` cannot carry.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.kde.KWin.ScreenShot2.Error")]
pub enum ShotError {
    NoAuthorized(String),
    InvalidWindow(String),
}

#[zbus::interface(name = "org.kde.KWin.ScreenShot2")]
impl FakeScreenShot {
    #[zbus(name = "CaptureWindow")]
    fn capture_window(
        &self,
        handle: String,
        options: HashMap<String, OwnedValue>,
        pipe: OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, ShotError> {
        if handle == "gone" {
            return Err(ShotError::InvalidWindow("no such window".into()));
        }
        self.answer("CaptureWindow", &handle, options, pipe)
    }

    #[zbus(name = "CaptureActiveWindow")]
    fn capture_active_window(
        &self,
        options: HashMap<String, OwnedValue>,
        pipe: OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, ShotError> {
        self.answer("CaptureActiveWindow", "", options, pipe)
    }

    #[zbus(name = "CaptureScreen")]
    fn capture_screen(
        &self,
        name: String,
        options: HashMap<String, OwnedValue>,
        pipe: OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, ShotError> {
        self.answer("CaptureScreen", &name, options, pipe)
    }

    #[zbus(name = "CaptureActiveScreen")]
    fn capture_active_screen(
        &self,
        options: HashMap<String, OwnedValue>,
        pipe: OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, ShotError> {
        self.answer("CaptureActiveScreen", "", options, pipe)
    }
}

/// ScreenShot2 on the fake KWin's connection.
pub async fn screenshot(kwin: &zbus::Connection) -> FakeScreenShot {
    let shot = FakeScreenShot {
        mode: Arc::new(Mutex::new(ShotMode::Answer)),
        calls: Arc::default(),
    };
    kwin.object_server()
        .at(
            "/org/kde/KWin/ScreenShot2",
            FakeScreenShot {
                mode: shot.mode.clone(),
                calls: shot.calls.clone(),
            },
        )
        .await
        .unwrap();
    shot
}

/// KWin's capture end to end: each method with its options, the raw image read from the pipe as
/// straight RGBA, the refusals as §11.1 and K22 say them.
#[tokio::test]
async fn kwins_screenshot2_is_read_through_its_pipe() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (kwin_conn, _state) = kwin(&bus).await;
    let shot = screenshot(&kwin_conn).await;
    let session = SessionBus::connected(bus.connect().await, T);
    assert_eq!(
        crate::desktop::plasma::capture::serves(&session.get().await.unwrap(), T).await,
        Ok(true)
    );
    // Generous: under the whole suite's load the read's end of file came up to a second late (a
    // process another test forks holds a copy of the pipe until it execs), past a 2 s bound once.
    let cap = KwinCapture {
        bus: session.clone(),
        timeout: Duration::from_secs(20),
    };
    let p = cap.window(Some("{abc}")).await.unwrap();
    assert_eq!((p.width, p.height), (3, 2));
    assert_eq!(
        p.rgba,
        [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, //
            0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 128
        ]
    );
    cap.window(None).await.unwrap();
    cap.screen(Some("DP-1")).await.unwrap();
    cap.screen(None).await.unwrap();
    let calls = shot.calls.lock().unwrap().clone();
    let window_opts = [
        "include-cursor=false",
        "include-decoration=false",
        "include-shadow=false",
        "native-resolution=true",
    ];
    let screen_opts = [
        "include-cursor=true",
        "include-shadow=false",
        "native-resolution=true",
    ];
    assert_eq!(
        calls,
        [
            (
                "CaptureWindow".into(),
                "{abc}".into(),
                window_opts.map(String::from).to_vec()
            ),
            (
                "CaptureActiveWindow".into(),
                "".into(),
                window_opts.map(String::from).to_vec()
            ),
            (
                "CaptureScreen".into(),
                "DP-1".into(),
                screen_opts.map(String::from).to_vec()
            ),
            (
                "CaptureActiveScreen".into(),
                "".into(),
                screen_opts.map(String::from).to_vec()
            ),
        ]
    );
    let e = cap.window(Some("gone")).await.unwrap_err();
    assert_eq!(
        e,
        Failure::Refused("that window is gone or cannot be captured".into())
    );
    *shot.mode.lock().unwrap() = ShotMode::NoGrant;
    let e = cap.screen(None).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "needs the installed Kai: KWin grants org.kde.KWin.ScreenShot2 per .desktop file"
    );
}

/// A KWin that takes the call and never writes: the capture ends at `dbus.timeout_s`, said.
#[tokio::test]
async fn a_capture_kwin_never_sends_ends_at_the_timeout() {
    struct Mute(Arc<Mutex<Vec<std::os::fd::OwnedFd>>>);
    #[zbus::interface(name = "org.kde.KWin.ScreenShot2")]
    impl Mute {
        #[zbus(name = "CaptureActiveScreen")]
        fn capture_active_screen(
            &self,
            _options: HashMap<String, OwnedValue>,
            pipe: OwnedFd,
        ) -> HashMap<String, OwnedValue> {
            // Holds the pipe open, writes nothing.
            self.0.lock().unwrap().push(pipe.into());
            HashMap::new()
        }
    }
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let kwin_conn = bus.connect().await;
    let held = Arc::new(Mutex::new(Vec::new()));
    kwin_conn
        .object_server()
        .at("/org/kde/KWin/ScreenShot2", Mute(held.clone()))
        .await
        .unwrap();
    kwin_conn.request_name("org.kde.KWin").await.unwrap();
    let cap = KwinCapture {
        bus: SessionBus::connected(bus.connect().await, Duration::from_secs(1)),
        timeout: Duration::from_secs(1),
    };
    let started = std::time::Instant::now();
    let e = cap.screen(None).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "KWin did not send the capture within 1 s (dbus.timeout_s)"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(held.lock().unwrap().len(), 1);
}

// ---- Klipper ----

struct FakeKlipper(Arc<Mutex<Vec<String>>>);

#[zbus::interface(name = "org.kde.klipper.klipper")]
impl FakeKlipper {
    #[zbus(name = "getClipboardContents")]
    fn get_clipboard_contents(&self) -> String {
        "current\ntext".into()
    }
    #[zbus(name = "getClipboardHistoryMenu")]
    fn get_clipboard_history_menu(&self) -> Vec<String> {
        vec!["current\ntext".into(), "".into(), "older".into()]
    }
    #[zbus(name = "setClipboardContents")]
    fn set_clipboard_contents(&self, text: String) {
        self.0.lock().unwrap().push(text);
    }
}

pub async fn klipper(bus: &PrivateBus) -> (zbus::Connection, Arc<Mutex<Vec<String>>>) {
    let set = Arc::new(Mutex::new(Vec::new()));
    let c = bus.connect().await;
    c.object_server()
        .at("/klipper", FakeKlipper(set.clone()))
        .await
        .unwrap();
    c.request_name("org.kde.klipper").await.unwrap();
    (c, set)
}

#[tokio::test]
async fn klipper_gives_the_clipboard_and_its_history() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (_k, set) = klipper(&bus).await;
    let k = Klipper {
        bus: SessionBus::connected(bus.connect().await, T),
        timeout: T,
    };
    assert_eq!(k.text().await.unwrap(), "current\ntext");
    assert_eq!(k.history().await.unwrap(), ["current\ntext", "", "older"]);
    assert!(k.reads());
    k.set("put\nthis").await.unwrap();
    assert_eq!(set.lock().unwrap().clone(), ["put\nthis"]);
}

// ---- AT-SPI ----

/// The `DoAction` calls a fake element got: (name, action index).
type Done = Arc<Mutex<Vec<(&'static str, i32)>>>;

/// One element of the fake app's tree.
#[derive(Clone, Default)]
struct Spec {
    role: u32,
    name: &'static str,
    states: &'static [u32],
    text: Option<&'static str>,
    /// Selected ranges of `text`.
    selections: &'static [(i32, i32)],
    value: Option<f64>,
    children: Vec<Spec>,
    /// `ChildCount`, where it differs from `children` (an element that manages its descendants).
    child_count: Option<i32>,
    /// A reference to an element of another application, served by nobody: (bus name, path).
    foreign: Option<(&'static str, &'static str)>,
    /// What the calls to this element tell the test.
    gauge: Option<Arc<Gauge>>,
    /// The element's actions (the Action interface), by name.
    actions: &'static [&'static str],
    /// The `DoAction` calls this element got: (name, action index).
    done: Option<Done>,
    /// `DoAction` does not return for this long (a handler that waits in a dialog).
    hangs: Option<Duration>,
}

/// Counts the calls a fake tree gets: how many `GetRole` are in flight at once (each takes a
/// moment), the most seen, and the `GetChildren` calls per path.
#[derive(Default)]
struct Gauge {
    inflight: std::sync::atomic::AtomicUsize,
    most: std::sync::atomic::AtomicUsize,
    children_asked: Mutex<Vec<String>>,
}

struct Node {
    spec: Spec,
    path: String,
    /// The children's (bus name, path).
    kids: Vec<(String, OwnedObjectPath)>,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Node {
    #[zbus(name = "GetChildren")]
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        if let Some(g) = &self.spec.gauge {
            g.children_asked.lock().unwrap().push(self.path.clone());
        }
        self.kids.clone()
    }
    #[zbus(name = "GetRole")]
    async fn get_role(&self) -> u32 {
        use std::sync::atomic::Ordering::SeqCst;
        if let Some(g) = &self.spec.gauge {
            let now = g.inflight.fetch_add(1, SeqCst) + 1;
            g.most.fetch_max(now, SeqCst);
            tokio::time::sleep(Duration::from_millis(15)).await;
            g.inflight.fetch_sub(1, SeqCst);
        }
        self.spec.role
    }
    #[zbus(property, name = "ChildCount")]
    fn child_count(&self) -> i32 {
        self.spec.child_count.unwrap_or(self.kids.len() as i32)
    }
    #[zbus(name = "GetState")]
    fn get_state(&self) -> Vec<u32> {
        let mut words = vec![0u32, 0u32];
        for &s in self.spec.states {
            words[(s / 32) as usize] |= 1 << (s % 32);
        }
        words
    }
    #[zbus(name = "GetInterfaces")]
    fn get_interfaces(&self) -> Vec<String> {
        let mut i = vec!["org.a11y.atspi.Accessible".to_owned()];
        if self.spec.text.is_some() {
            i.push("org.a11y.atspi.Text".into());
        }
        if self.spec.value.is_some() {
            i.push("org.a11y.atspi.Value".into());
        }
        if !self.spec.actions.is_empty() {
            i.push("org.a11y.atspi.Action".into());
        }
        i
    }
    #[zbus(property, name = "Name")]
    fn name(&self) -> String {
        self.spec.name.into()
    }
}

struct TextNode(Spec);

#[zbus::interface(name = "org.a11y.atspi.Text")]
impl TextNode {
    #[zbus(name = "GetText")]
    fn get_text(&self, start: i32, end: i32) -> String {
        let t: Vec<char> = self.0.text.unwrap_or_default().chars().collect();
        let end = if end < 0 { t.len() } else { end as usize };
        t[start as usize..end.min(t.len())].iter().collect()
    }
    #[zbus(name = "GetNSelections")]
    fn get_n_selections(&self) -> i32 {
        self.0.selections.len() as i32
    }
    #[zbus(name = "GetSelection")]
    fn get_selection(&self, i: i32) -> (i32, i32) {
        self.0.selections[i as usize]
    }
}

struct ActionNode(Spec);

#[zbus::interface(name = "org.a11y.atspi.Action")]
impl ActionNode {
    #[zbus(property, name = "NActions")]
    fn n_actions(&self) -> i32 {
        self.0.actions.len() as i32
    }
    #[zbus(name = "GetName")]
    fn get_name(&self, i: i32) -> String {
        self.0.actions[i as usize].into()
    }
    #[zbus(name = "DoAction")]
    async fn do_action(&self, i: i32) -> bool {
        if let Some(d) = &self.0.done {
            d.lock().unwrap().push((self.0.name, i));
        }
        if let Some(h) = self.0.hangs {
            tokio::time::sleep(h).await;
        }
        true
    }
}

struct ValueNode(f64);

#[zbus::interface(name = "org.a11y.atspi.Value")]
impl ValueNode {
    #[zbus(property, name = "CurrentValue")]
    fn current_value(&self) -> f64 {
        self.0
    }
}

/// Serves `spec` and its children at `path` and below on `c`; returns its (bus name, path).
fn serve<'a>(
    c: &'a zbus::Connection,
    spec: Spec,
    path: String,
) -> futures_util::future::BoxFuture<'a, (String, OwnedObjectPath)> {
    Box::pin(async move {
        let mut kids = Vec::new();
        for (i, k) in spec.children.iter().enumerate() {
            match k.foreign {
                Some((dest, p)) => {
                    kids.push((dest.to_owned(), OwnedObjectPath::try_from(p).unwrap()))
                }
                None => kids.push(serve(c, k.clone(), format!("{path}/{i}")).await),
            }
        }
        let objects = c.object_server();
        if spec.text.is_some() {
            objects
                .at(path.as_str(), TextNode(spec.clone()))
                .await
                .unwrap();
        }
        if let Some(v) = spec.value {
            objects.at(path.as_str(), ValueNode(v)).await.unwrap();
        }
        if !spec.actions.is_empty() {
            objects
                .at(path.as_str(), ActionNode(spec.clone()))
                .await
                .unwrap();
        }
        objects
            .at(
                path.as_str(),
                Node {
                    spec,
                    path: path.clone(),
                    kids,
                },
            )
            .await
            .unwrap();
        (
            c.unique_name().unwrap().to_string(),
            OwnedObjectPath::try_from(path).unwrap(),
        )
    })
}

/// The registry's root: the apps.
struct Registry(Vec<(String, OwnedObjectPath)>);

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Registry {
    #[zbus(name = "GetChildren")]
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.0.clone()
    }
}

struct A11yBus {
    address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl A11yBus {
    #[zbus(name = "GetAddress")]
    fn get_address(&self) -> String {
        self.address.clone()
    }
}

struct A11yStatus {
    enabled: bool,
    reader: bool,
}

#[zbus::interface(name = "org.a11y.Status")]
impl A11yStatus {
    #[zbus(property, name = "IsEnabled")]
    fn is_enabled(&self) -> bool {
        self.enabled
    }
    #[zbus(property, name = "ScreenReaderEnabled")]
    fn screen_reader_enabled(&self) -> bool {
        self.reader
    }
}

const VISIBLE: u32 = 30;
const SHOWN: &[u32] = &[VISIBLE, 8];

fn app_tree() -> Spec {
    let frame = Spec {
        role: 23,
        name: "Kai Test",
        states: SHOWN,
        children: vec![
            Spec {
                role: 39,
                states: SHOWN,
                children: vec![
                    Spec {
                        role: 29,
                        name: "Name:",
                        states: SHOWN,
                        text: Some("Name:"),
                        ..Default::default()
                    },
                    Spec {
                        role: 79,
                        states: &[VISIBLE, 8, 12],
                        text: Some("typed\ntext"),
                        ..Default::default()
                    },
                    Spec {
                        role: 40,
                        name: "Password",
                        states: SHOWN,
                        text: Some("hunter2"),
                        selections: &[(0, 7)],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            Spec {
                role: 73,
                states: SHOWN,
                text: Some("Hello world, again"),
                selections: &[(6, 11), (13, 18)],
                ..Default::default()
            },
            Spec {
                role: 43,
                name: "Save",
                states: &[VISIBLE],
                ..Default::default()
            },
            Spec {
                role: 51,
                name: "Volume",
                states: SHOWN,
                value: Some(42.5),
                ..Default::default()
            },
            Spec {
                role: 39,
                name: "hidden page",
                states: &[8],
                children: vec![Spec {
                    role: 29,
                    name: "never shown",
                    states: SHOWN,
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    Spec {
        role: 75,
        name: "kai-test",
        states: SHOWN,
        children: vec![
            frame,
            Spec {
                role: 23,
                name: "Other window",
                states: SHOWN,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

struct A11y {
    session: PrivateBus,
    _a11y: PrivateBus,
    _held: Vec<zbus::Connection>,
}

/// A session bus with `org.a11y.Bus`, and an accessibility bus with the registry and one app (this
/// process, so its pid is ours) serving [`app_tree`].
async fn a11y(enabled: bool) -> Option<A11y> {
    a11y_with(enabled, app_tree(), None).await
}

/// [`a11y`] serving `tree`; `address` is what `GetAddress` answers where it is not the real one.
async fn a11y_with(enabled: bool, tree: Spec, address: Option<&str>) -> Option<A11y> {
    a11y_switches(enabled, false, tree, address).await
}

/// [`a11y_with`] with `ScreenReaderEnabled` set as `reader`.
async fn a11y_switches(
    enabled: bool,
    reader: bool,
    tree: Spec,
    address: Option<&str>,
) -> Option<A11y> {
    let session = PrivateBus::start()?;
    let a11y = PrivateBus::start()?;
    let launcher = session.connect().await;
    let objects = launcher.object_server();
    let address = address.map_or_else(|| a11y.address.clone(), str::to_owned);
    objects
        .at("/org/a11y/bus", A11yBus { address })
        .await
        .unwrap();
    objects
        .at("/org/a11y/bus", A11yStatus { enabled, reader })
        .await
        .unwrap();
    launcher.request_name("org.a11y.Bus").await.unwrap();
    let app = a11y.connect().await;
    let root = serve(&app, tree, "/org/a11y/atspi/accessible/root".into()).await;
    let registry = a11y.connect().await;
    registry
        .object_server()
        .at("/org/a11y/atspi/accessible/root", Registry(vec![root]))
        .await
        .unwrap();
    registry
        .request_name("org.a11y.atspi.Registry")
        .await
        .unwrap();
    Some(A11y {
        session,
        _a11y: a11y,
        _held: vec![launcher, app, registry],
    })
}

fn atspi(b: &A11y, session: zbus::Connection) -> Atspi {
    let _ = b;
    Atspi {
        bus: SessionBus::connected(session, T),
        timeout: T,
        concurrency: 16,
        switch: None,
    }
}

/// The window found by its process and title; its visible elements with their roles, names,
/// texts, values and states; a hidden page left out; a password field's text never read.
#[tokio::test]
async fn at_spi_reads_a_windows_visible_tree() {
    let Some(b) = a11y(true).await else { return };
    let a = atspi(&b, b.session.connect().await);
    let target = Target {
        pid: std::process::id(),
        title: "Kai Test".into(),
    };
    let tree = a.tree(&target).await.unwrap();
    assert_eq!(tree.unanswered, 0);
    let win = kai_desktop::caps::windows::Window {
        id: "w1".into(),
        app: "kai-test".into(),
        title: "Kai Test".into(),
        ..Default::default()
    };
    assert_eq!(
        crate::desktop::read::tree_lines(&win, &tree),
        "w1 kai-test \"Kai Test\"\n\
         frame \"Kai Test\"\n  \
           label \"Name:\"\n  \
           entry: \"typed\\ntext\" (focused)\n  \
           password text \"Password\"\n  \
           paragraph: \"Hello world, again\"\n  \
           button \"Save\" (disabled)\n  \
           slider \"Volume\" = 42.5"
    );
    let sel = a.selection(&target).await.unwrap();
    assert_eq!(sel, ["world\nagain"], "never the password field's");
    // A window whose title does not match picks by part, else errs listing the app's windows.
    let e = a
        .tree(&Target {
            pid: std::process::id(),
            title: "Third".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "none of the app's windows on the accessibility bus is that window; they are: \"Kai \
         Test\", \"Other window\""
    );
}

/// An app that is not on the accessibility bus: the error says the switch's state, and names
/// the switch precisely while it is off (WP13 (a): Qt apps follow `IsEnabled`).
#[tokio::test]
async fn an_app_off_the_accessibility_bus_is_said_with_the_switch() {
    let off_bus = Target {
        pid: 1,
        title: "x".into(),
    };
    let Some(b) = a11y(false).await else { return };
    let a = atspi(&b, b.session.connect().await);
    let e = a.tree(&off_bus).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "the window's app is not on the accessibility bus; accessibility is off: Qt apps join it \
         once org.a11y.Status IsEnabled is true (org.a11y.Bus at /org/a11y/bus on the session \
         bus, kept as the GSettings key org.gnome.desktop.interface toolkit-accessibility; \
         ScreenReaderEnabled is not needed), which Kai does not switch (accessibility.switch = \
         \"never\"); Electron and Chromium apps join only when started with \
         --force-renderer-accessibility"
    );
    let Some(b) = a11y(true).await else { return };
    let a = atspi(&b, b.session.connect().await);
    let e = a.tree(&off_bus).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "the window's app is not on the accessibility bus; accessibility is on (org.a11y.Status \
         IsEnabled): Electron and Chromium apps join only when started with \
         --force-renderer-accessibility"
    );
}

/// The error names the switch that is on: `ScreenReaderEnabled` alone is not `IsEnabled`.
#[tokio::test]
async fn the_error_names_the_switch_that_is_on() {
    let off_bus = Target {
        pid: 1,
        title: "x".into(),
    };
    let Some(b) = a11y_switches(false, true, app_tree(), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let e = a.tree(&off_bus).await.unwrap_err().to_string();
    assert!(
        e.contains("accessibility is on (org.a11y.Status ScreenReaderEnabled)"),
        "{e}"
    );
    assert!(!e.contains("IsEnabled"), "{e}");
}

const MANAGES: u32 = 31;

fn kai_test_target() -> Target {
    Target {
        pid: std::process::id(),
        title: "Kai Test".into(),
    }
}

/// An app with one window `Kai Test` holding `kids`.
fn window_of(kids: Vec<Spec>) -> Spec {
    Spec {
        role: 75,
        name: "kai-test",
        states: SHOWN,
        children: vec![Spec {
            role: 23,
            name: "Kai Test",
            states: SHOWN,
            children: kids,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn lines_of(tree: &Tree) -> String {
    let win = kai_desktop::caps::windows::Window {
        id: "w1".into(),
        app: "kai-test".into(),
        title: "Kai Test".into(),
        ..Default::default()
    };
    crate::desktop::read::tree_lines(&win, tree)
}

/// An element that manages its descendants is not asked for them (a 100k-row table would freeze
/// its app); its line says how many children it has and that they were not walked, and the
/// selection's answer says so too.
#[tokio::test]
async fn an_element_managing_its_descendants_is_not_walked_and_says_so() {
    let gauge = Arc::new(Gauge::default());
    let table = Spec {
        role: 55,
        name: "Big",
        states: &[VISIBLE, 8, MANAGES],
        child_count: Some(100_000),
        gauge: Some(gauge.clone()),
        children: vec![Spec {
            role: 29,
            name: "never asked",
            states: SHOWN,
            gauge: Some(gauge.clone()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let after = Spec {
        role: 29,
        name: "after",
        states: SHOWN,
        ..Default::default()
    };
    let Some(b) = a11y_with(true, window_of(vec![table, after]), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a.tree(&kai_test_target()).await.unwrap();
    assert_eq!(tree.unanswered, 0);
    assert_eq!(
        lines_of(&tree),
        format!(
            "w1 kai-test \"Kai Test\"\n{} \"Kai Test\"\n  {} \"Big\" (100000 children, not \
             walked: the element manages its descendants itself)\n  label \"after\"",
            role_name(23),
            role_name(55)
        )
    );
    let sel = a.selection(&kai_test_target()).await.unwrap();
    assert_eq!(
        sel,
        ["(100000 children, not walked: the element manages its descendants itself)"]
    );
    assert!(
        gauge.children_asked.lock().unwrap().is_empty(),
        "GetChildren was never called on it"
    );
}

/// At most `concurrency` calls of one walk are in flight, and the whole tree is still read.
#[tokio::test]
async fn a_walks_calls_in_flight_stay_bounded() {
    let gauge = Arc::new(Gauge::default());
    let kids: Vec<Spec> = (0..40)
        .map(|_| Spec {
            role: 29,
            name: "item",
            states: SHOWN,
            gauge: Some(gauge.clone()),
            ..Default::default()
        })
        .collect();
    let Some(b) = a11y_with(true, window_of(kids), None).await else {
        return;
    };
    let mut a = atspi(&b, b.session.connect().await);
    a.concurrency = 3;
    let tree = a.tree(&kai_test_target()).await.unwrap();
    assert_eq!(tree.unanswered, 0);
    assert_eq!(tree.root.children.len(), 40, "nothing cut for the bound");
    let most = gauge.most.load(std::sync::atomic::Ordering::SeqCst);
    assert!((2..=3).contains(&most), "most in flight: {most}");
    // 0: no bound; the walk is the same, and wider.
    gauge.most.store(0, std::sync::atomic::Ordering::SeqCst);
    a.concurrency = 0;
    let tree = a.tree(&kai_test_target()).await.unwrap();
    assert_eq!(tree.root.children.len(), 40);
    assert!(gauge.most.load(std::sync::atomic::Ordering::SeqCst) > 3);
}

/// Nothing below a password field is read: its text, its children's texts and values, and no
/// selection of any of it.
#[tokio::test]
async fn nothing_inside_a_password_field_is_read() {
    let password = Spec {
        role: 40,
        name: "Password",
        states: SHOWN,
        text: Some("hunter2"),
        selections: &[(0, 7)],
        children: vec![Spec {
            role: 29,
            name: "shown dots",
            states: SHOWN,
            text: Some("secret child"),
            selections: &[(0, 6)],
            children: vec![Spec {
                role: 51,
                name: "deeper",
                states: SHOWN,
                text: Some("secret grandchild"),
                value: Some(7.0),
                selections: &[(0, 6)],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let Some(b) = a11y_with(true, window_of(vec![password]), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a.tree(&kai_test_target()).await.unwrap();
    let lines = lines_of(&tree);
    assert!(lines.contains("\"Password\""), "{lines}");
    assert!(lines.contains("\"deeper\""), "{lines}");
    for secret in ["hunter2", "secret", "= 7"] {
        assert!(!lines.contains(secret), "{secret} leaked: {lines}");
    }
    assert!(a.selection(&kai_test_target()).await.unwrap().is_empty());
}

/// A reference to another application's element is shown as one line and never followed.
#[tokio::test]
async fn a_reference_into_another_application_is_not_followed() {
    let foreign = Spec {
        foreign: Some((":1.9999", "/org/a11y/atspi/accessible/42")),
        ..Default::default()
    };
    let mine = Spec {
        role: 29,
        name: "mine",
        states: SHOWN,
        ..Default::default()
    };
    let Some(b) = a11y_with(true, window_of(vec![mine, foreign]), None).await else {
        return;
    };
    let a = atspi(&b, b.session.connect().await);
    let tree = a.tree(&kai_test_target()).await.unwrap();
    assert_eq!(tree.unanswered, 0, "it was not even asked");
    assert_eq!(
        lines_of(&tree),
        format!(
            "w1 kai-test \"Kai Test\"\n{} \"Kai Test\"\n  label \"mine\"\n  \
             (element of another application, not read)",
            role_name(23)
        )
    );
}

/// Only a unix: address is used; the others are errors that say so.
#[tokio::test]
async fn only_a_unix_accessibility_bus_address_is_used() {
    for bad in [
        "unixexec:path=/usr/bin/touch,argv1=/tmp/x",
        "tcp:host=example.org,port=1",
        "unix:guid=abcd",
        "unix:path=/run/a;unixexec:path=/bin/true",
        "",
    ] {
        let Some(b) = a11y_with(true, app_tree(), Some(bad)).await else {
            return;
        };
        let a = atspi(&b, b.session.connect().await);
        let e = a.tree(&kai_test_target()).await.unwrap_err();
        assert!(
            e.to_string()
                .starts_with("the accessibility bus gave an address Kai does not connect to"),
            "{bad}: {e}"
        );
    }
}

/// A Flatpak app is told apart by its root's `.flatpak-info`.
#[test]
fn a_flatpak_process_is_recognised_by_its_root() {
    let proc = temp_dir();
    std::fs::create_dir_all(proc.path().join("77/root")).unwrap();
    std::fs::create_dir_all(proc.path().join("78/root")).unwrap();
    std::fs::write(proc.path().join("77/root/.flatpak-info"), "[Application]\n").unwrap();
    assert!(crate::desktop::fdo::atspi::flatpak(proc.path(), 77));
    assert!(!crate::desktop::fdo::atspi::flatpak(proc.path(), 78));
    assert!(!crate::desktop::fdo::atspi::flatpak(proc.path(), 79));
}

// ---- Baloo ----

/// A `baloosearch6` stand-in: records its arguments, prints `n` paths and an `Elapsed:` line. Run
/// once with `--kai-selftest` until it is not "text file busy" (another test's fork can hold it
/// open for writing for a moment, as in `actions.rs`).
fn fake_baloosearch(dir: &std::path::Path, n: usize) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("baloosearch6");
    let args = dir.join("args");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ \"$1\" = --kai-selftest ] && exit 0\nprintf '%s\\n' \"$@\" > '{}'\ni=1\n\
             while [ $i -le {n} ]; do echo \"/home/u/docs/file $i.txt\"; i=$((i+1)); done\n\
             echo 'Elapsed: 0.1 msecs'\n",
            args.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(&script)
            .arg("--kai-selftest")
            .status()
        {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => break,
        }
    }
    script
}

#[tokio::test]
async fn baloo_runs_baloosearch_with_the_limit_and_says_the_index_state() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let dir = temp_dir();
    std::fs::write(
        dir.path().join("baloofilerc"),
        "[General]\nfolders[$e]=$HOME/docs/\n",
    )
    .unwrap();
    let program = fake_baloosearch(dir.path(), 5);
    let baloo = Baloo {
        program,
        config_home: dir.path().to_owned(),
        home: "/home/u".into(),
        bus: SessionBus::connected(bus.connect().await, T),
        timeout: T,
        recent: crate::desktop::fdo::recent::RecentList::in_data_home(dir.path()),
    };
    let found = baloo.find("-rf report", 3).await.unwrap();
    assert_eq!(
        found.paths,
        [
            "/home/u/docs/file 1.txt",
            "/home/u/docs/file 2.txt",
            "/home/u/docs/file 3.txt"
        ]
    );
    assert!(found.more);
    assert_eq!(
        found.notes,
        [
            "indexed folders: /home/u/docs",
            "Baloo's indexer is not running: files changed since it stopped are missing or stale"
        ]
    );
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(
        args, "-l\n4\n--\n-rf report\n",
        "the query is one argument after --"
    );
    let found = baloo.find("x", 5).await.unwrap();
    assert_eq!((found.paths.len(), found.more), (5, false));
    let found = baloo.find("x", 0).await.unwrap();
    assert_eq!((found.paths.len(), found.more), (5, false));
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(args, "--\nx\n", "no limit: no -l");
    // A limit with no room for "one past" is no limit, not an overflow.
    let found = baloo.find("x", usize::MAX).await.unwrap();
    assert_eq!((found.paths.len(), found.more), (5, false));
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert_eq!(args, "--\nx\n");
    // The indexer running: no note about it.
    let indexer = bus.connect().await;
    indexer.request_name("org.kde.baloo").await.unwrap();
    let found = baloo.find("x", 0).await.unwrap();
    assert_eq!(found.notes, ["indexed folders: /home/u/docs"]);
}

// ---- The probe and the tools end to end ----

async fn tool(entries: &[Entry], name: &str, args: serde_json::Value) -> Vec<Block> {
    let e = entries
        .iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    (e.handler)(Call::new(
        e.spec,
        Args::read(e.spec, args.as_object().cloned()).unwrap(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await
    .blocks
}

/// On Plasma with ScreenShot2, Klipper, baloosearch6 and the recent list, the plasma provider
/// serves Capture, Search and Clipboard and every read tool but the accessibility ones is listed;
/// `see_window` resolves the window through KWin's scripting and captures it by its id.
#[tokio::test]
async fn on_plasma_the_read_tools_are_listed_and_answer() {
    let Some(session) = PrivateBus::start() else {
        return;
    };
    let (kwin_conn, _state) = kwin(&session).await;
    let shot = screenshot(&kwin_conn).await;
    let _k = klipper(&session).await;
    let dir = temp_dir();
    std::fs::write(
        dir.path().join("recently-used.xbel"),
        "<xbel><bookmark href=\"file:///home/u/a.txt\" modified=\"2026-10-09T08:00:00Z\"/></xbel>",
    )
    .unwrap();
    let env = Env {
        current_desktop: "KDE".into(),
        session: Some(SessionBus::connected(session.connect().await, T)),
        system: Some(SystemBus::new(Some(session.address.clone()), T)),
        wayland: false,
        timeout: T,
        runtime_dir: dir.path().to_owned(),
        read: ReadEnv {
            data_home: dir.path().to_owned(),
            config_home: dir.path().to_owned(),
            home: "/home/u".into(),
            baloosearch: Some(fake_baloosearch(dir.path(), 2)),
            find_limit: 100,
            a11y_concurrency: 16,
        },
        ..Env::inert()
    };
    let d = DesktopTools::new(env, DesktopConfig::default(), Changes::new().0, false);
    d.probe_now().await;
    let chosen = d.chosen();
    let of = |cap| {
        chosen
            .iter()
            .find(|c| c.capability == cap)
            .map(|c| c.provider)
            .unwrap()
    };
    assert_eq!(of(Capability::Capture), Some(ProviderName::Plasma));
    assert_eq!(of(Capability::Search), Some(ProviderName::Plasma));
    assert_eq!(of(Capability::Clipboard), Some(ProviderName::Plasma));
    assert_eq!(of(Capability::Accessibility), None);
    let entries = d.entries();
    let reads: Vec<&str> = entries
        .iter()
        .filter(|e| e.area == "read")
        .map(|e| e.spec.name)
        .collect();
    assert_eq!(
        reads,
        [
            "see_window",
            "see_screen",
            "find_files",
            "recent_files",
            "clipboard_get",
            "clipboard_history"
        ]
    );
    let blocks = tool(&entries, "see_window", json!({"window": "firefox"})).await;
    match &blocks[..] {
        [Block::Text(t), Block::Image { mime, base64 }] => {
            assert_eq!(t, "w2 firefox \"News\", 3x2 pixels");
            assert_eq!(mime, "image/png");
            assert!(!base64.is_empty());
        }
        other => panic!("{other:?}"),
    }
    let blocks = tool(&entries, "see_screen", json!({})).await;
    assert!(
        matches!(&blocks[..], [Block::Text(t), Block::Image { .. }] if t == "output Virtual-0, 3x2 pixels"),
        "{blocks:?}"
    );
    let calls: Vec<(String, String)> = shot
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| (c.0.clone(), c.1.clone()))
        .collect();
    assert_eq!(
        calls,
        [
            ("CaptureWindow".into(), "w2".into()),
            ("CaptureScreen".into(), "Virtual-0".into())
        ]
    );
    let text = |b: Vec<Block>| match &b[..] {
        [Block::Text(t)] => t.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(tool(&entries, "find_files", json!({"query": "file"})).await),
        "/home/u/docs/file 1.txt\n/home/u/docs/file 2.txt\nindexed folders: /home/u\n\
         Baloo's indexer is not running: files changed since it stopped are missing or stale"
    );
    assert_eq!(
        text(tool(&entries, "recent_files", json!({})).await),
        "2026-10-09 08:00 UTC /home/u/a.txt"
    );
    assert_eq!(
        text(tool(&entries, "clipboard_history", json!({})).await),
        "1 \"current\\ntext\"\n2 (no text)\n3 \"older\""
    );
}

/// Elsewhere, fdo serves the recent files (no index: `find_files` is not listed) and AT-SPI.
#[tokio::test]
async fn without_plasma_fdo_serves_recent_files_and_accessibility() {
    let Some(b) = a11y(true).await else { return };
    let dir = temp_dir();
    let env = Env {
        current_desktop: "GNOME".into(),
        session: Some(SessionBus::connected(b.session.connect().await, T)),
        system: None,
        wayland: false,
        timeout: T,
        runtime_dir: dir.path().to_owned(),
        read: ReadEnv {
            data_home: dir.path().to_owned(),
            ..Default::default()
        },
        ..Env::inert()
    };
    let d = DesktopTools::new(env, DesktopConfig::default(), Changes::new().0, false);
    d.probe_now().await;
    let chosen = d.chosen();
    let of = |cap| {
        chosen
            .iter()
            .find(|c| c.capability == cap)
            .map(|c| (c.provider, c.note.clone()))
            .unwrap()
    };
    assert_eq!(
        of(Capability::Search),
        (
            Some(ProviderName::Fdo),
            "the recent files from recently-used.xbel; no file index".into()
        )
    );
    assert_eq!(
        of(Capability::Accessibility),
        (Some(ProviderName::Fdo), "AT-SPI".into())
    );
    let names: Vec<&str> = d.entries().iter().map(|e| e.spec.name).collect();
    // No windows here: read_window and read_selection need them to find the window.
    assert_eq!(
        names,
        [
            "desktop_context",
            "media_play_pause",
            "media_next",
            "media_previous",
            "recent_files"
        ]
    );
}

mod press;
mod switch;
