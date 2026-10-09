//! The desktop module without a desktop: fake capabilities for the tools (`tools::tests`), and the
//! providers against fakes of the D-Bus services on private buses (a `dbus-daemon` for the session
//! and one for the system, in a temp directory, activating nothing): KWin's scripting, the screen
//! saver, the notification server, an MPRIS player, logind and UPower. The session's own bus,
//! KWin and display are never touched.

mod actions;
mod approved;
pub(crate) mod bus;
pub(crate) mod input;
mod own_windows;
mod probe;
pub(crate) mod read;
mod script;

pub(crate) use bus::temp_dir;

use std::sync::{Arc, Mutex};

use kai_desktop::caps::audio::{Audio, Device};
use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::launch::{Launch, Opening, Ran, RunBounds, Started};
use kai_desktop::caps::media::{Control, Media, Playback, Player};
use kai_desktop::caps::notify::{Asked, Dnd, Notify};
use kai_desktop::caps::session::{Battery, Charge, Features, Lock, NightLight, Session};
use kai_desktop::caps::windows::{Action, Done, Output, Snapshot, Window, Windows};
use kai_desktop::caps::{Failure, Fut};

use super::tools::Services;
use crate::host::registry::Entry;

/// Windows from a fixed snapshot; the actions it was asked for are recorded; `fail` answers every
/// call with that failure.
#[derive(Clone, Default)]
pub struct FakeWindows {
    pub snap: Snapshot,
    pub acts: Arc<Mutex<Vec<(String, Action)>>>,
    pub fail: Option<Failure>,
}

impl Windows for FakeWindows {
    fn snapshot(&self) -> Fut<'_, Snapshot> {
        Box::pin(async move {
            match &self.fail {
                Some(f) => Err(f.clone()),
                None => Ok(self.snap.clone()),
            }
        })
    }

    fn act<'a>(&'a self, id: &'a str, action: &'a Action) -> Fut<'a, Done> {
        Box::pin(async move {
            self.acts
                .lock()
                .unwrap()
                .push((id.to_owned(), action.clone()));
            Ok(match action {
                Action::ToggleMaximize => "maximized".to_owned(),
                _ => "done".to_owned(),
            })
        })
    }
}

pub struct FakeSession(pub Result<Lock, Failure>, pub Option<Battery>);

impl Session for FakeSession {
    fn lock(&self) -> Fut<'_, Lock> {
        Box::pin(async move { self.0.clone() })
    }
    fn battery(&self) -> Fut<'_, Option<Battery>> {
        Box::pin(async move { Ok(self.1.clone()) })
    }
    fn features(&self) -> Features {
        Features {
            brightness: true,
            night_light: true,
        }
    }
    fn lock_screen(&self) -> Fut<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn suspend(&self) -> Fut<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn set_brightness(&self, percent: u32) -> Fut<'_, u32> {
        Box::pin(async move { Ok(percent) })
    }
    fn set_night_light(&self, on: bool) -> Fut<'_, NightLight> {
        Box::pin(async move { Ok(NightLight { on }) })
    }
}

pub struct FakeMedia(pub Vec<Player>);

impl Media for FakeMedia {
    fn players(&self) -> Fut<'_, Vec<Player>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn control<'a>(&'a self, _: &'a str, _: Control) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub struct FakeNotify(pub bool);

impl Notify for FakeNotify {
    fn can_dnd(&self) -> bool {
        true
    }
    fn dnd(&self) -> Fut<'_, bool> {
        Box::pin(async move { Ok(self.0) })
    }
    fn set_dnd(&self, on: bool) -> Fut<'_, Dnd> {
        Box::pin(async move { Ok(Dnd { on }) })
    }
    fn show<'a>(&'a self, _: &'a str, _: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn ask<'a>(&'a self, _: &'a str, _: &'a [String]) -> Fut<'a, Asked> {
        Box::pin(async {
            Ok(Asked {
                choice: Some(0),
                under_dnd: false,
            })
        })
    }
}

pub struct FakeAudio;

impl Audio for FakeAudio {
    fn outputs(&self) -> Fut<'_, Vec<Device>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn set_volume<'a>(&'a self, _: &'a str, _: u32) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn set_mute<'a>(&'a self, _: &'a str, _: bool) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub struct FakeClipboard;

impl Clipboard for FakeClipboard {
    fn set<'a>(&'a self, _: &'a str) -> Fut<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub fn window(id: &str, app: &str, title: &str) -> Window {
    Window {
        id: id.into(),
        app: app.into(),
        title: title.into(),
        output: "DP-1".into(),
        desktops: vec!["Main".into()],
        ..Default::default()
    }
}

/// Two outputs, two desktops, three windows (the second one active).
pub fn snapshot() -> Snapshot {
    let mut active = window("bbbb", "org.kde.konsole", "build — Konsole");
    active.active = true;
    active.maximized = true;
    let mut other = window("cccc", "firefox", "Mail");
    other.output = "HDMI-A-1".into();
    other.desktops = vec![];
    other.minimized = true;
    Snapshot {
        windows: vec![
            active,
            window("aaaa", "org.kde.dolphin", "Home — Dolphin"),
            other,
        ],
        outputs: vec![
            Output {
                name: "DP-1".into(),
                width: 2048,
                height: 1152,
                scale: 1.25,
                active: true,
                ..Default::default()
            },
            Output {
                name: "HDMI-A-1".into(),
                x: 2048,
                width: 1920,
                height: 1080,
                scale: 1.0,
                ..Default::default()
            },
        ],
        desktops: vec!["Main".into(), "Mail".into()],
        desktop: "Main".into(),
        activity: "Work".into(),
    }
}

/// A Launch that starts nothing: it records each call and answers with `started` and `ran`.
#[derive(Default)]
pub struct FakeLaunch {
    pub calls: Mutex<Vec<String>>,
    pub started: Started,
    pub ran: Option<Ran>,
}

impl Launch for FakeLaunch {
    fn open_plan<'a>(&'a self, target: &'a str) -> Fut<'a, Opening> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("open_plan {target}"));
        Box::pin(async move {
            if target.ends_with(".desktop") {
                return Err(Failure::Refused("the fake opens no launcher".into()));
            }
            Ok(Opening {
                id: self.started.id.clone(),
                name: self.started.name.clone(),
                mime: self.started.mime.clone().unwrap_or_default(),
                notes: Vec::new(),
                plan: Arc::new(target.to_owned()),
            })
        })
    }
    fn open_start<'a>(&'a self, opening: &'a Opening) -> Fut<'a, Started> {
        let target = opening
            .plan
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        self.calls
            .lock()
            .unwrap()
            .push(format!("open_start {target}"));
        Box::pin(async move { Ok(self.started.clone()) })
    }
    fn app_launch<'a>(&'a self, app: &'a str, urls: &'a [String]) -> Fut<'a, Started> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("app_launch {app} {urls:?}"));
        Box::pin(async move { Ok(self.started.clone()) })
    }
    fn run<'a>(
        &'a self,
        argv: &'a [String],
        cwd: Option<&'a str>,
        bounds: &'a RunBounds,
    ) -> Fut<'a, Ran> {
        self.calls.lock().unwrap().push(format!(
            "run {argv:?} {cwd:?} keep {:?} stop {:?}",
            bounds.keep,
            bounds.stop_at.as_ref().map(|(_, s)| s.as_str())
        ));
        Box::pin(async move {
            self.ran
                .clone()
                .ok_or_else(|| Failure::Refused("the fake runs nothing".into()))
        })
    }
}

/// Every service, faked.
pub fn all_services(windows: FakeWindows) -> Services {
    read::with_reads(Services {
        windows: Some(Arc::new(windows)),
        session: Some(Arc::new(FakeSession(
            Ok(Lock {
                locked: Some(false),
                idle_s: Some(42),
            }),
            Some(Battery {
                percent: 80.6,
                charge: Charge::Discharging,
            }),
        ))),
        media: Some(Arc::new(FakeMedia(vec![
            Player {
                id: "org.mpris.MediaPlayer2.spotify".into(),
                name: "Spotify".into(),
                playback: Playback::Playing,
                title: "Song".into(),
                artists: vec!["A".into(), "B".into()],
            },
            Player {
                id: "org.mpris.MediaPlayer2.vlc".into(),
                name: "VLC".into(),
                playback: Playback::Stopped,
                title: "Old".into(),
                artists: vec![],
            },
        ]))),
        notify: Some(Arc::new(FakeNotify(true))),
        audio: Some(Arc::new(FakeAudio)),
        clipboard: Some(Arc::new(FakeClipboard)),
        launch: Some(Arc::new(FakeLaunch::default())),
        input: Some(input::fake_input()),
        ..Services::default()
    })
}

/// The entries of every desktop tool, as a desktop with every capability lists them (WP8's
/// contract test: every contract tool has a provider in this build).
pub fn full_entries() -> Vec<Entry> {
    super::entries(&all_services(FakeWindows {
        snap: snapshot(),
        ..Default::default()
    }))
}
