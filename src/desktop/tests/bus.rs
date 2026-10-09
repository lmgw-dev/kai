//! Private buses and fakes of the desktop's D-Bus services for the providers' tests.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use zbus::object_server::ObjectServer;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as ZValue};

/// Temp directories under target/, not /tmp (RAM).
pub fn temp_dir() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

/// A `dbus-daemon` of the test's own that activates nothing, killed on drop.
pub struct PrivateBus {
    child: Child,
    pub address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    /// `None` (the test says it skipped) where there is no `dbus-daemon`.
    pub fn start() -> Option<Self> {
        let dir = temp_dir();
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
        let mut child = match Command::new("dbus-daemon")
            .arg(format!("--config-file={}", conf.display()))
            .args(["--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipped: no dbus-daemon ({e})");
                return None;
            }
        };
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Some(Self {
            child,
            address: address.trim().to_owned(),
            _dir: dir,
        })
    }

    pub async fn connect(&self) -> zbus::Connection {
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

/// What a fake KWin script does when run.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Answers by what the script asks: a snapshot, or an action's word (`gone` for the window
    /// id `gone`).
    Answer,
    /// Answers as a script that threw.
    Throw(String),
    /// Never answers.
    Silent,
    /// Answers after this long.
    Late(Duration),
    /// Answers as KWin, with a token that is not the run's.
    WrongToken,
}

#[derive(Debug, Default)]
pub struct KwinState {
    pub mode: Option<Mode>,
    /// Each loaded script: its plugin name, its file and the file's mode bits, its text.
    pub loaded: Vec<(String, std::path::PathBuf, u32, String)>,
    pub unloaded: Vec<String>,
    /// The most scripts that were loaded at once.
    pub most_at_once: usize,
}

pub type Kwin = Arc<Mutex<KwinState>>;

pub struct FakeScripting {
    pub state: Kwin,
    next: i32,
}

/// The snapshot the fake KWin's scripting gives.
pub fn kwin_snapshot() -> Value {
    json!({
        "windows": [
            {"id": "w1", "app": "org.kde.dolphin", "title": "Home — Dolphin", "pid": 10,
             "output": "Virtual-0", "desktops": ["Desktop 1"], "active": true, "minimized": false,
             "maximized": false, "fullscreen": false},
            {"id": "w2", "app": "firefox", "title": "News", "pid": 11, "output": "Virtual-1",
             "desktops": [], "active": false, "minimized": true, "maximized": false,
             "fullscreen": false}
        ],
        "outputs": [
            {"name": "Virtual-0", "x": 0, "y": 0, "width": 1280, "height": 720, "scale": 1,
             "active": true},
            {"name": "Virtual-1", "x": 1280, "y": 0, "width": 1280, "height": 720, "scale": 1,
             "active": false}
        ],
        "desktops": ["Desktop 1", "Desktop 2"],
        "desktop": "Desktop 1",
        "activity": "00000000-0000-0000-0000-000000000000"
    })
}

/// The `ARGS` a script was written with.
pub fn args_of(source: &str) -> Value {
    let start = source.find("const ARGS = ").unwrap() + "const ARGS = ".len();
    let end = source[start..].find(";\n").unwrap() + start;
    serde_json::from_str(&source[start..end]).unwrap()
}

#[zbus::interface(name = "org.kde.kwin.Scripting")]
impl FakeScripting {
    #[zbus(name = "loadScript")]
    async fn load_script(
        &mut self,
        path: String,
        plugin: String,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> i32 {
        use std::os::unix::fs::PermissionsExt;
        let file = std::path::PathBuf::from(&path);
        let mode = std::fs::metadata(&file)
            .map(|m| m.permissions().mode())
            .unwrap_or(0);
        let source = std::fs::read_to_string(&file).unwrap_or_default();
        {
            let mut state = self.state.lock().unwrap();
            state.loaded.push((plugin, file, mode, source.clone()));
            let now = state.loaded.len() - state.unloaded.len();
            state.most_at_once = state.most_at_once.max(now);
        }
        let id = self.next;
        self.next += 1;
        server
            .at(
                format!("/Scripting/Script{id}"),
                FakeScript {
                    state: self.state.clone(),
                    source,
                },
            )
            .await
            .unwrap();
        id
    }

    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin: String) -> bool {
        self.state.lock().unwrap().unloaded.push(plugin);
        true
    }
}

struct FakeScript {
    state: Kwin,
    source: String,
}

#[zbus::interface(name = "org.kde.kwin.Script")]
impl FakeScript {
    #[zbus(name = "run")]
    async fn run(&self, #[zbus(connection)] conn: &zbus::Connection) {
        let args = args_of(&self.source);
        let mode = self
            .state
            .lock()
            .unwrap()
            .mode
            .clone()
            .unwrap_or(Mode::Answer);
        let answer = match &mode {
            Mode::Silent => return,
            Mode::Throw(e) => json!({"ok": false, "error": e}),
            Mode::Answer | Mode::Late(_) | Mode::WrongToken => match args.get("action") {
                None => json!({"ok": true, "value": kwin_snapshot()}),
                Some(_) if args["window"] == "gone" => json!({"ok": true, "value": {"gone": true}}),
                Some(a) => {
                    json!({"ok": true, "value": {"done": format!("did {}", a.as_str().unwrap())}})
                }
            },
        };
        let delay = match mode {
            Mode::Late(d) => d,
            _ => Duration::ZERO,
        };
        let conn = conn.clone();
        let reply = args["reply"].clone();
        let token = match mode {
            Mode::WrongToken => "0".repeat(32),
            _ => reply["id"].as_str().unwrap().to_owned(),
        };
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = conn
                .call_method(
                    Some(reply["service"].as_str().unwrap()),
                    reply["path"].as_str().unwrap(),
                    Some(reply["iface"].as_str().unwrap()),
                    "Result",
                    &(token.as_str(), answer.to_string()),
                )
                .await;
        });
    }
}

/// KWin's name with its scripting.
pub async fn kwin(bus: &PrivateBus) -> (zbus::Connection, Kwin) {
    let state = Kwin::default();
    let c = bus.connect().await;
    c.object_server()
        .at(
            "/Scripting",
            FakeScripting {
                state: state.clone(),
                next: 0,
            },
        )
        .await
        .unwrap();
    c.request_name("org.kde.KWin").await.unwrap();
    (c, state)
}

struct ScreenSaver;

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    #[zbus(name = "GetActive")]
    fn get_active(&self) -> bool {
        false
    }
    #[zbus(name = "GetSessionIdleTime")]
    fn get_session_idle_time(&self) -> u32 {
        7
    }
}

struct Notifications;

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[zbus(property, name = "Inhibited")]
    fn inhibited(&self) -> bool {
        true
    }
}

struct MprisRoot(&'static str);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    #[zbus(property, name = "Identity")]
    fn identity(&self) -> String {
        self.0.into()
    }
}

struct MprisPlayer;

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    #[zbus(property, name = "PlaybackStatus")]
    fn playback_status(&self) -> String {
        "Playing".into()
    }
    #[zbus(property, name = "Metadata")]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            (
                "xesam:title".to_owned(),
                OwnedValue::try_from(ZValue::from("Song")).unwrap(),
            ),
            (
                "xesam:artist".to_owned(),
                OwnedValue::try_from(ZValue::from(vec!["A", "B"])).unwrap(),
            ),
        ])
    }
}

/// Plasma's session services on `bus`: the screen saver, the notification server with
/// `Inhibited`, an MPRIS player that answers (Spotify) and one that has no player interface.
pub async fn plasma_services(bus: &PrivateBus) -> Vec<zbus::Connection> {
    let saver = bus.connect().await;
    saver
        .object_server()
        .at("/ScreenSaver", ScreenSaver)
        .await
        .unwrap();
    saver
        .request_name("org.freedesktop.ScreenSaver")
        .await
        .unwrap();
    let notes = bus.connect().await;
    notes
        .object_server()
        .at("/org/freedesktop/Notifications", Notifications)
        .await
        .unwrap();
    notes
        .request_name("org.freedesktop.Notifications")
        .await
        .unwrap();
    vec![saver, notes]
}

pub async fn mpris_players(bus: &PrivateBus) -> Vec<zbus::Connection> {
    let spotify = bus.connect().await;
    spotify
        .object_server()
        .at("/org/mpris/MediaPlayer2", MprisRoot("Spotify"))
        .await
        .unwrap();
    spotify
        .object_server()
        .at("/org/mpris/MediaPlayer2", MprisPlayer)
        .await
        .unwrap();
    spotify
        .request_name("org.mpris.MediaPlayer2.spotify")
        .await
        .unwrap();
    let mute = bus.connect().await;
    mute.request_name("org.mpris.MediaPlayer2.vlc.instance42")
        .await
        .unwrap();
    vec![spotify, mute]
}

struct LoginUser;

#[zbus::interface(name = "org.freedesktop.login1.User")]
impl LoginUser {
    #[zbus(property, name = "Display")]
    fn display(&self) -> (String, OwnedObjectPath) {
        (
            "2".into(),
            OwnedObjectPath::try_from("/org/freedesktop/login1/session/_32").unwrap(),
        )
    }
}

struct LoginSession;

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    #[zbus(property, name = "LockedHint")]
    fn locked_hint(&self) -> bool {
        true
    }
    #[zbus(property, name = "IdleHint")]
    fn idle_hint(&self) -> bool {
        true
    }
    #[zbus(property, name = "IdleSinceHint")]
    fn idle_since_hint(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        (now - Duration::from_secs(60)).as_micros() as u64
    }
}

struct DisplayDevice;

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl DisplayDevice {
    #[zbus(property, name = "IsPresent")]
    fn is_present(&self) -> bool {
        true
    }
    #[zbus(property, name = "Percentage")]
    fn percentage(&self) -> f64 {
        55.5
    }
    #[zbus(property, name = "State")]
    fn state(&self) -> u32 {
        1
    }
}

/// logind (the user's graphical session, locked, idle a minute) and UPower (55.5 %, charging) on a
/// bus standing in for the system bus.
pub async fn system_services(bus: &PrivateBus) -> Vec<zbus::Connection> {
    let login = bus.connect().await;
    login
        .object_server()
        .at("/org/freedesktop/login1/user/self", LoginUser)
        .await
        .unwrap();
    login
        .object_server()
        .at("/org/freedesktop/login1/session/_32", LoginSession)
        .await
        .unwrap();
    login.request_name("org.freedesktop.login1").await.unwrap();
    let upower = bus.connect().await;
    upower
        .object_server()
        .at(
            "/org/freedesktop/UPower/devices/DisplayDevice",
            DisplayDevice,
        )
        .await
        .unwrap();
    upower.request_name("org.freedesktop.UPower").await.unwrap();
    vec![login, upower]
}
