//! The providers' probes and the choice against fake services on private buses.

use std::time::Duration;

use kai_desktop::caps::Capability;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::bus::{PrivateBus, kwin, mpris_players, plasma_services, system_services, temp_dir};
use crate::desktop::probe::{Chosen, ProviderName};
use crate::desktop::settings::{Choice, DesktopConfig};
use crate::desktop::{DesktopTools, Env, SystemBus};
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Changes, Entry, Provider, SessionBus};

const T: Duration = Duration::from_secs(2);

struct Buses {
    session: PrivateBus,
    system: PrivateBus,
    _held: Vec<zbus::Connection>,
    dir: tempfile::TempDir,
}

/// A Plasma session's services (KWin's scripting, the screen saver, the notification server, two
/// MPRIS players) and a system bus with logind and UPower.
async fn buses() -> Option<Buses> {
    let session = PrivateBus::start()?;
    let system = PrivateBus::start()?;
    let mut held = vec![kwin(&session).await.0];
    held.extend(plasma_services(&session).await);
    held.extend(mpris_players(&session).await);
    held.extend(system_services(&system).await);
    Some(Buses {
        session,
        system,
        _held: held,
        dir: temp_dir(),
    })
}

async fn env(b: &Buses, desktop: &str) -> Env {
    Env {
        current_desktop: desktop.into(),
        session: Some(SessionBus::connected(b.session.connect().await, T)),
        system: Some(SystemBus::new(Some(b.system.address.clone()), T)),
        wayland: false,
        timeout: T,
        runtime_dir: b.dir.path().to_owned(),
        ..Env::inert()
    }
}

fn provider_of(chosen: &[Chosen], cap: Capability) -> (Option<ProviderName>, String) {
    let c = chosen.iter().find(|c| c.capability == cap).unwrap();
    (c.provider, c.note.clone())
}

async fn call(entries: &[Entry], name: &str) -> (String, bool) {
    let e = entries.iter().find(|e| e.spec.name == name).unwrap();
    let r = (e.handler)(Call::new(
        e.spec,
        Args::read(e.spec, json!({}).as_object().cloned()).unwrap(),
        Caller::Owner,
        CancellationToken::new(),
    ))
    .await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

/// On Plasma, `plasma` serves windows, the session and do not disturb, `fdo` the media; the
/// window tools and `desktop_context` read the fakes end to end (KWin's answer through Kai's own
/// object, the battery through UPower, the players through MPRIS).
#[tokio::test]
async fn on_plasma_the_plasma_provider_wins_where_it_serves() {
    let Some(b) = buses().await else { return };
    let d = DesktopTools::new(
        env(&b, "KDE").await,
        DesktopConfig::default(),
        Changes::new().0,
        false,
    );
    d.probe_now().await;
    let chosen = d.chosen();
    use ProviderName::*;
    let (p, note) = provider_of(&chosen, Capability::Windows);
    assert_eq!(p, Some(Plasma));
    assert_eq!(
        note,
        "KWin scripting for the window facts and the actions; the window management protocol not \
         tried (no Wayland display)"
    );
    assert_eq!(provider_of(&chosen, Capability::Session).0, Some(Plasma));
    assert_eq!(provider_of(&chosen, Capability::Notify).0, Some(Plasma));
    assert_eq!(
        provider_of(&chosen, Capability::Media),
        (Some(Fdo), "MPRIS".into())
    );
    assert_eq!(provider_of(&chosen, Capability::Capture).0, None);
    let entries = d.entries();
    // The ten window tools, the media controls, do not disturb with the notifications, the lock
    // and suspend: no brightness or Night Light on these fakes, no audio or clipboard program.
    assert_eq!(entries.len(), 18);
    let (text, err) = call(&entries, "desktop_context").await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "active window: org.kde.dolphin \"Home — Dolphin\" (id w1)\n\
         outputs: Virtual-0 1280x720 at 0,0 (active); Virtual-1 1280x720 at 1280,0\n\
         virtual desktop: Desktop 1 (1 of 2)\n\
         screen: unlocked, idle 7 s\n\
         battery: 56 %, charging\n\
         do not disturb: on\n\
         media: Spotify playing \"Song\" by A, B; vlc did not say what it plays"
    );
    let (text, err) = call(&entries, "window_list").await;
    assert!(!err);
    assert_eq!(
        text,
        "w1 org.kde.dolphin \"Home — Dolphin\" (Virtual-0, Desktop 1, active)\n\
         w2 firefox \"News\" (Virtual-1, all desktops, minimized)"
    );
    let (text, err) = call(&entries, "window_close").await;
    assert_eq!(
        (text.as_str(), err),
        ("did close: w1 org.kde.dolphin \"Home — Dolphin\"", false)
    );
}

/// Elsewhere `fdo` serves the session (logind: locked, idle a minute) and the media; no provider
/// serves windows, so only `desktop_context` is listed. The running Kai's provider puts the table
/// in the tray.
#[tokio::test]
async fn without_plasma_fdo_serves_and_the_window_tools_are_not_listed() {
    let Some(b) = buses().await else { return };
    let d = DesktopTools::new(
        env(&b, "GNOME").await,
        DesktopConfig::default(),
        Changes::new().0,
        true,
    );
    d.probe_now().await;
    let chosen = d.chosen();
    let (p, note) = provider_of(&chosen, Capability::Windows);
    assert_eq!(p, None);
    assert_eq!(
        note,
        "plasma: not a Plasma session (XDG_CURRENT_DESKTOP=\"GNOME\")"
    );
    assert_eq!(
        provider_of(&chosen, Capability::Session).0,
        Some(ProviderName::Fdo)
    );
    let entries = d.entries();
    let names: Vec<&str> = entries.iter().map(|e| e.spec.name).collect();
    assert_eq!(
        names,
        [
            "desktop_context",
            "media_play_pause",
            "media_next",
            "media_previous",
            "notify",
            "ask",
            "lock_screen",
            "suspend"
        ],
        "no windows, no do not disturb on the freedesktop server"
    );
    let (text, _) = call(&entries, "desktop_context").await;
    let idle: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("screen: locked, idle "))
        .and_then(|l| l.strip_suffix(" s"))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("{text}"));
    assert!((59..=62).contains(&idle), "{text}");
    assert!(text.contains("battery: 56 %, charging"), "{text}");
    assert!(
        !text.contains("do not disturb"),
        "no provider serves it: {text}"
    );
    assert_eq!(
        crate::desktop::tray_lines().borrow().clone(),
        [
            "desktop: windows none, capture none, accessibility none, launch none, search none, \
             clipboard none, media fdo, audio none, session fdo, notify fdo, input none"
        ]
    );
}

/// `[desktop]` overrides the choice: another provider whose probe passes, none, or a provider that
/// does not serve the capability (then none, said in the table).
#[tokio::test]
async fn the_configuration_overrides_the_choice() {
    let Some(b) = buses().await else { return };
    let config = DesktopConfig {
        session: Some(Choice::Fdo),
        notify: Some(Choice::None),
        windows: Some(Choice::Fdo),
        ..Default::default()
    };
    let d = DesktopTools::new(env(&b, "KDE").await, config, Changes::new().0, false);
    d.probe_now().await;
    let chosen = d.chosen();
    assert_eq!(
        provider_of(&chosen, Capability::Session).0,
        Some(ProviderName::Fdo)
    );
    assert_eq!(
        provider_of(&chosen, Capability::Notify),
        (None, "desktop.notify = none".into())
    );
    assert_eq!(
        provider_of(&chosen, Capability::Windows),
        (
            None,
            "desktop.windows = fdo: it does not serve windows in this build".into()
        )
    );
    // desktop_context, the media controls, the lock and suspend.
    assert_eq!(d.entries().len(), 6);
}

/// Inside a runtime the probe runs in a task: the first list has no desktop tools, the change
/// signal follows, and the next list has them (the host relists and sends `list_changed`).
#[tokio::test]
async fn a_probe_in_a_runtime_signals_its_result() {
    let Some(b) = buses().await else { return };
    let (changes, mut changed) = Changes::new();
    let d = DesktopTools::new(
        env(&b, "KDE").await,
        DesktopConfig::default(),
        changes,
        false,
    );
    assert!(d.entries().is_empty());
    tokio::time::timeout(Duration::from_secs(10), changed.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.entries().len(), 18);
}

/// Outside a runtime (the host built in `main` before the runtime runs it, `kai --tool-list`) the
/// first list already holds the desktop's tools: the probe runs there and then, on the connection
/// the running runtime made.
#[test]
fn outside_a_runtime_the_first_list_is_complete() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let Some(b) = rt.block_on(buses()) else {
        return;
    };
    let env = rt.block_on(env(&b, "KDE"));
    let d = DesktopTools::new(env, DesktopConfig::default(), Changes::new().0, false);
    assert_eq!(d.entries().len(), 18);
    let (text, err) = rt.block_on(call(&d.entries(), "window_list"));
    assert!(!err && text.starts_with("w1 "), "{text}");
}
