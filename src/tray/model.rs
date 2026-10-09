//! The tray as one model value (§5.1, §7, K9), and its pure rendering.
//!
//! The core owns a [`TrayModel`] and publishes it; [`render`] turns it into what the tray shows.
//! This build reaches: starting, not paired, wallet locked, wallet error, paired but not connected,
//! and revoked, disabled or expired (§3.3, raised through the `lmgw_link` seam). The feed adds connected, the hold badge
//! and unreachable. The voice (WP4) adds the voice states (§5.1 3–7), the timing line, the
//! listening, mute, mode and conversation entries and the shortcut note. The audio part (WP3) adds
//! the Echo mode radio, the Microphone and Speaker menus, and the echo and PipeWire notes. The ring
//! (WP5) adds the Overlays menu with its one setting. The conversation (WP6) adds Open in lmgw and
//! the folder's notes (the lookup, the owner's steps for a folder that is not ready, a short
//! retention). lmgw's admin tools add their level's note when the device has them.

use crate::audio::{Device, DeviceList, EchoMode, EchoStatus};
use crate::lmgw_link::{AdminLevel, admin_note};
use crate::voice::VoiceMode;

/// Icon names (K9): Kai's own hicolor set, graphite and blue; grey when Kai cannot serve.
pub const ICON_OFFLINE: &str = "kai-tray-offline";
pub const ICON_IDLE: &str = "kai-tray-idle";
/// Listening (the owner speaks, the talk key is held, or listening is on and quiet, K11).
pub const ICON_LISTENING: &str = "kai-tray-listening";
pub const ICON_THINKING: &str = "kai-tray-thinking";
pub const ICON_SPEAKING: &str = "kai-tray-speaking";
pub const ICON_MUTED: &str = "kai-tray-muted";
/// The hold is an amber badge on whichever icon shows, not a state of its own (K9).
pub const BADGE_HELD: &str = "kai-tray-held";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    pub pairing: Pairing,
    /// "Start with Plasma": the autostart entry exists (§13).
    pub autostart: bool,
    /// The last pairing attempt's error, shown until the next attempt.
    pub pair_error: Option<String>,
    /// A pairing link from the scheme handler on its way to the owner's confirmation (K13).
    pub link_wait: Option<LinkWait>,
    /// The last "Start with Plasma" error, shown until the next switch.
    pub autostart_error: Option<String>,
    pub audio: AudioModel,
    pub overlay: OverlayModel,
    pub voice: VoiceModel,
    /// The global shortcuts' note (§8): not bound, no key set, the portal missing.
    pub shortcuts: Option<String>,
    /// The conversation's folder notes (§4): the lookup, the owner's steps, a short retention.
    pub conversation: Vec<String>,
    /// The hosted tools' lines (WP8): linked as `<label>`, not granted, offline; `[policy]`
    /// settings that name no tool; PD5's step.
    pub tools: Vec<String>,
    /// The desktop's capability table (K23, WP14): set by the tray from `crate::desktop`'s line,
    /// never by the core.
    pub desktop: Vec<String>,
    /// The Personality submenu (WP22): lmgw's profiles and the one the conversation talks with.
    pub personality: PersonalityModel,
}

/// The Personality radio (WP22): Default, then the profiles by name. No options: no submenu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersonalityModel {
    pub options: Vec<PersonalityOption>,
    pub selected: usize,
}

/// One item of the Personality radio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalityOption {
    /// The profile's id; `None`: Default.
    pub id: Option<i64>,
    pub label: String,
}

/// What the voice shows (§5.1): Kai's own states around the voice machine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceShown {
    /// Listening is off and no session runs.
    #[default]
    Off,
    /// A session is being opened (the thread asked for, the bind, `session.created`).
    Connecting,
    /// Listening is on and nothing happens. In continuous listening the ring shows it too, in its
    /// listening state (K11); push-to-talk is in standby between presses and shows no ring.
    Quiet,
    Listening,
    Thinking,
    Speaking,
    Interrupted,
}

/// The voice part of the tray (§5.1, §7), from `Voice::model`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceModel {
    /// The screen is locked (`crate::screen_lock`): Kai is not active.
    pub locked: bool,
    /// Listening is on.
    pub on: bool,
    pub mode: VoiceMode,
    pub muted: bool,
    pub shown: VoiceShown,
    /// The conversation's thread title.
    pub thread: Option<String>,
    /// The last turn's timing line (§7).
    pub timing: Option<String>,
    /// The session saw the GPU hold hold a stage (the feed's hold is `Link::Up::held`).
    pub held: bool,
    pub notes: Vec<String>,
}

/// The overlay settings the tray shows (§7 item 8, §16 answer 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayModel {
    /// `overlay.ring_over_fullscreen`.
    pub ring_over_fullscreen: bool,
    /// Saving it failed.
    pub error: Option<String>,
    /// The ring is down after its web process ended twice, and when Kai tries again (§9.4).
    pub note: Option<String>,
    /// The tool-controlled overlays shown (WP10): `(id, label)`, set by the tray from
    /// `crate::widgets::tray_entries`, never by the core.
    pub widgets: Vec<(u64, String)>,
}

impl Default for OverlayModel {
    fn default() -> Self {
        Self {
            ring_over_fullscreen: true,
            error: None,
            note: None,
            widgets: Vec::new(),
        }
    }
}

/// The audio side of the tray (§6, §7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioModel {
    /// The chosen echo mode (`audio.echo_mode`).
    pub mode: EchoMode,
    /// What it amounts to, once the audio thread reported.
    pub echo: Option<EchoStatus>,
    pub devices: DeviceList,
    /// The picked microphone and speaker (`audio.input`, `audio.output`); `None`: Default.
    pub input: Option<String>,
    pub output: Option<String>,
    /// PipeWire is not reachable: why, and the retry.
    pub link: Option<String>,
    /// Saving a choice failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    /// Reading the pairing from the wallet at start.
    Checking,
    /// No key for the configured `lmgw.url`, or no `lmgw.url`.
    NotPaired,
    /// §2: the wallet is locked. Kai checks again every `retry_s`.
    WalletLocked { retry_s: u64 },
    /// The Secret Service failed (not running, timed out); tried again every `retry_s`.
    WalletError { message: String, retry_s: u64 },
    /// A key is stored for `url`.
    Paired { url: String, link: Link },
    /// §3.3: the key was rotated or deleted (`key_unknown`, 401 `device_key_unknown`), or
    /// revoked as another kind of key; Kai stops retrying until it is paired again.
    Revoked { reason: String },
    /// §3.3: the device is disabled on lmgw (401 `device_disabled`, the `revoked` kind
    /// `device_disabled`); `reason` is lmgw's message. Kai stops retrying and keeps the key, which
    /// works once the owner enables the device: Try again tries it (review KAI-3 K3-6).
    Disabled { reason: String },
    /// §3.3: the key passed its expiry (401 `key_expired`, the `revoked` kind `key_expired`);
    /// `reason` is lmgw's message. As for [`Pairing::Disabled`], Kai stops retrying and keeps the
    /// key, which works once the owner moves its expiry: Try again tries it (lmgw 7d6d18c).
    Expired { reason: String },
}

/// The state line of [`Pairing::Disabled`].
pub const DISABLED: &str =
    "this device was disabled on lmgw: enable it on Usage → Keys, or pair again";

/// The state line of [`Pairing::Expired`].
pub const EXPIRED: &str =
    "this device's key expired on lmgw: move its expiry on Usage → Keys, or pair again";

/// A clicked or handed-over pairing link that Kai holds in memory (K13): it waits for the wallet
/// to open, then for the owner's answer in Kai's confirmation dialog. The pairing Kai has stays as
/// it is meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkWait {
    Wallet { url: String },
    Owner { url: String },
}

/// The lmgw link of a paired Kai (the feed, §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Not connected; `note` says why, and when the link retries.
    Down { note: String },
    /// The feed is up: `hello` named the device, its hold, the label Kai's hosted tools go under
    /// (§10.1, phase 2; kept, not shown), and the device's level of lmgw's admin tools (shown
    /// when it is not off; `state` moves it).
    Up {
        device: String,
        held: bool,
        hosts_label: Option<String>,
        admin: AdminLevel,
    },
}

impl TrayModel {
    pub fn starting(autostart: bool) -> Self {
        Self {
            pairing: Pairing::Checking,
            autostart,
            pair_error: None,
            link_wait: None,
            autostart_error: None,
            audio: AudioModel::default(),
            overlay: OverlayModel::default(),
            voice: VoiceModel::default(),
            shortcuts: None,
            conversation: Vec::new(),
            tools: Vec::new(),
            desktop: Vec::new(),
            personality: PersonalityModel::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Microphone,
    Speaker,
}

/// One radio item of the Microphone or Speaker menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOption {
    /// `None`: Default (follows the system default).
    pub node: Option<String>,
    pub label: String,
}

/// A menu entry of §7 that exists in this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// 1. Start listening / Stop listening.
    Listening {
        on: bool,
    },
    /// 2. Mute, a check item.
    Mute {
        checked: bool,
    },
    /// 3. Automatic / Push-to-talk, a radio.
    Mode {
        selected: VoiceMode,
    },
    /// 4. Continue "<title>" (informational).
    Continue {
        title: String,
    },
    /// 4. New conversation.
    NewConversation,
    /// 5. Open in lmgw: the thread's dashboard page.
    OpenInLmgw,
    /// 5b. Personality (WP22), a radio submenu: Default and lmgw's profiles by name.
    Personality {
        options: Vec<PersonalityOption>,
        selected: usize,
    },
    /// 6. Echo mode, a radio submenu.
    Echo {
        selected: EchoMode,
    },
    /// `device` mode playing elsewhere than the default output: the warning's offer (§6.2).
    UseHalfDuplex,
    /// 7. Microphone, Speaker: Default plus the PipeWire nodes.
    Devices {
        kind: DeviceKind,
        options: Vec<DeviceOption>,
        selected: usize,
    },
    /// Item 8, Overlays: the ring's layer setting, then the tool-controlled overlays shown, each
    /// with its Hide, and Hide all (§10.1: nothing lingers unseen).
    Overlays {
        ring_over_fullscreen: bool,
        widgets: Vec<(u64, String)>,
    },
    Separator,
    /// A disabled device's, and an expired key's: the stored key is tried again (§3.3).
    TryAgain,
    PairWithLmgw,
    StartWithPlasma {
        checked: bool,
    },
    /// The configuration window (WP21b, K29).
    Settings,
    /// Kai's self-updater (§13, 2026-10-08): the updater answers in a notification.
    CheckForUpdates,
    Quit,
}

impl MenuEntry {
    pub fn label(&self) -> &'static str {
        match self {
            MenuEntry::Listening { on: false } => "Start listening",
            MenuEntry::Listening { on: true } => "Stop listening",
            MenuEntry::Mute { .. } => "Mute",
            MenuEntry::Mode { .. } => "Listening mode",
            MenuEntry::Continue { .. } => "Continue",
            MenuEntry::NewConversation => "New conversation",
            MenuEntry::OpenInLmgw => "Open in lmgw",
            MenuEntry::Personality { .. } => "Personality",
            MenuEntry::Echo { .. } => "Echo mode",
            MenuEntry::UseHalfDuplex => "Use half duplex (Kai's speaker is not the default output)",
            MenuEntry::Devices {
                kind: DeviceKind::Microphone,
                ..
            } => "Microphone",
            MenuEntry::Devices {
                kind: DeviceKind::Speaker,
                ..
            } => "Speaker",
            MenuEntry::Overlays { .. } => "Overlays",
            MenuEntry::Separator => "",
            MenuEntry::TryAgain => "Try again",
            MenuEntry::PairWithLmgw => "Pair with lmgw…",
            MenuEntry::Settings => "Settings…",
            MenuEntry::StartWithPlasma { .. } => "Start with Plasma",
            MenuEntry::CheckForUpdates => "Check for Updates…",
            MenuEntry::Quit => "Quit",
        }
    }
}

/// Default (naming the current default) plus the devices; a pick that is not present stays listed,
/// marked, so the choice is visible.
pub fn device_options(
    devices: &[Device],
    default: Option<&str>,
    pick: Option<&str>,
) -> (Vec<DeviceOption>, usize) {
    let default_label = match default.and_then(|d| DeviceList::label_of(devices, d)) {
        Some(l) => format!("Default ({l})"),
        None => "Default".to_owned(),
    };
    let mut options = vec![DeviceOption {
        node: None,
        label: default_label,
    }];
    options.extend(devices.iter().map(|d| DeviceOption {
        node: Some(d.name.clone()),
        label: d.label.clone(),
    }));
    let selected = match pick {
        None => 0,
        Some(p) => match options.iter().position(|o| o.node.as_deref() == Some(p)) {
            Some(i) => i,
            None => {
                options.push(DeviceOption {
                    node: Some(p.to_owned()),
                    label: format!("{p} (not present)"),
                });
                options.len() - 1
            }
        },
    };
    (options, selected)
}

fn audio_menu(a: &AudioModel) -> Vec<MenuEntry> {
    let mut menu = vec![MenuEntry::Echo { selected: a.mode }];
    if a.echo.as_ref().is_some_and(|e| e.offer_half_duplex) {
        menu.push(MenuEntry::UseHalfDuplex);
    }
    let d = &a.devices;
    for (kind, list, default, pick) in [
        (
            DeviceKind::Microphone,
            &d.inputs,
            &d.default_input,
            &a.input,
        ),
        (
            DeviceKind::Speaker,
            &d.outputs,
            &d.default_output,
            &a.output,
        ),
    ] {
        let (options, selected) = device_options(list, default.as_deref(), pick.as_deref());
        menu.push(MenuEntry::Devices {
            kind,
            options,
            selected,
        });
    }
    menu
}

fn audio_notes(a: &AudioModel) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(n) = &a.link {
        notes.push(format!("audio: {n}"));
    }
    if let Some(n) = a.echo.as_ref().and_then(|e| e.note.clone()) {
        notes.push(n);
    }
    if let Some(e) = &a.error {
        notes.push(format!("audio: {e}"));
    }
    notes
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub icon: &'static str,
    pub badge: Option<&'static str>,
    pub title: String,
    /// The state, the tooltip's first line.
    pub status: String,
    /// Further tooltip lines.
    pub notes: Vec<String>,
    pub menu: Vec<MenuEntry>,
}

/// The check item's label inside Overlays.
pub const RING_OVER_FULLSCREEN: &str = "Ring over fullscreen windows";

/// The item inside Overlays that hides every tool-controlled overlay.
pub const HIDE_ALL: &str = "Hide all";

/// The voice's entries (§7 items 1–4), at the top of a paired Kai's menu.
fn voice_menu(v: &VoiceModel, personality: &PersonalityModel) -> Vec<MenuEntry> {
    let mut menu = vec![
        MenuEntry::Listening { on: v.on },
        MenuEntry::Mute { checked: v.muted },
        MenuEntry::Mode { selected: v.mode },
    ];
    if let Some(t) = &v.thread {
        menu.push(MenuEntry::Continue { title: t.clone() });
    }
    menu.push(MenuEntry::NewConversation);
    menu.push(MenuEntry::OpenInLmgw);
    if !personality.options.is_empty() {
        menu.push(MenuEntry::Personality {
            options: personality.options.clone(),
            selected: personality.selected,
        });
    }
    menu.push(MenuEntry::Separator);
    menu
}

/// The icon and the state line of a connected Kai (§5.1's order: muted, listening, thinking,
/// speaking, idle).
fn voice_state(v: &VoiceModel) -> (&'static str, String) {
    let ptt = v.mode == VoiceMode::PushToTalk;
    match v.shown {
        _ if v.locked => (ICON_IDLE, "screen locked: Kai is not active".to_owned()),
        _ if v.muted => (
            ICON_MUTED,
            if v.on {
                "muted: Kai hears silence".to_owned()
            } else {
                "muted (voice off)".to_owned()
            },
        ),
        VoiceShown::Listening | VoiceShown::Interrupted => (ICON_LISTENING, "listening".to_owned()),
        VoiceShown::Thinking => (ICON_THINKING, "thinking".to_owned()),
        VoiceShown::Speaking => (ICON_SPEAKING, "speaking".to_owned()),
        VoiceShown::Quiet if ptt => (
            ICON_LISTENING,
            "listening on: hold the talk key to talk".to_owned(),
        ),
        VoiceShown::Quiet => (ICON_LISTENING, "listening on".to_owned()),
        VoiceShown::Connecting => (ICON_IDLE, "connecting the voice".to_owned()),
        VoiceShown::Off => (ICON_IDLE, "idle (voice off)".to_owned()),
    }
}

pub fn render(model: &TrayModel) -> TrayView {
    use MenuEntry::*;
    let unpaired_menu = vec![PairWithLmgw, Settings, CheckForUpdates, Quit];
    let mut paired_menu = audio_menu(&model.audio);
    paired_menu.extend([
        Overlays {
            ring_over_fullscreen: model.overlay.ring_over_fullscreen,
            widgets: model.overlay.widgets.clone(),
        },
        Separator,
        PairWithLmgw,
        StartWithPlasma {
            checked: model.autostart,
        },
        Settings,
        CheckForUpdates,
        Quit,
    ]);
    let retry_note = |s: u64| format!("Kai checks again every {s} s (wallet.retry_s)");

    let (icon, badge, status, mut notes, menu) = match &model.pairing {
        Pairing::Checking => (
            ICON_OFFLINE,
            None,
            "reading the pairing from the wallet".to_owned(),
            vec![],
            unpaired_menu,
        ),
        Pairing::NotPaired => (
            ICON_OFFLINE,
            None,
            "not paired".to_owned(),
            vec!["pair with lmgw from this menu, or click a pairing link".to_owned()],
            unpaired_menu,
        ),
        Pairing::WalletLocked { retry_s } => (
            ICON_OFFLINE,
            None,
            "wallet locked: unlock KWallet".to_owned(),
            vec![retry_note(*retry_s)],
            paired_menu,
        ),
        Pairing::WalletError { message, retry_s } => (
            ICON_OFFLINE,
            None,
            "the wallet is not available".to_owned(),
            vec![message.clone(), retry_note(*retry_s)],
            paired_menu,
        ),
        Pairing::Paired {
            url,
            link: Link::Down { note },
        } => {
            let mut menu = voice_menu(&model.voice, &model.personality);
            menu.extend(paired_menu);
            let mut notes = vec![format!("paired with lmgw at {url}"), note.clone()];
            if model.voice.muted {
                notes.push("muted".to_owned());
            }
            notes.extend(model.voice.notes.iter().cloned());
            notes.extend(model.tools.iter().cloned());
            notes.extend(model.desktop.iter().cloned());
            (ICON_OFFLINE, None, "not connected".to_owned(), notes, menu)
        }
        Pairing::Paired {
            url,
            link:
                Link::Up {
                    device,
                    held,
                    admin,
                    ..
                },
        } => {
            let v = &model.voice;
            let (icon, status) = voice_state(v);
            let mut notes = Vec::new();
            if let Some(t) = &v.timing {
                notes.push(t.clone());
            }
            notes.push(format!("device '{device}' on {url}"));
            notes.extend(admin_note(admin));
            let held = *held || v.held;
            if held {
                notes.push("GPU hold: local models are paused".to_owned());
            }
            notes.extend(v.notes.iter().cloned());
            notes.extend(model.conversation.iter().cloned());
            notes.extend(model.tools.iter().cloned());
            notes.extend(model.desktop.iter().cloned());
            let mut menu = voice_menu(v, &model.personality);
            menu.extend(paired_menu);
            (icon, held.then_some(BADGE_HELD), status, notes, menu)
        }
        Pairing::Revoked { reason } => (
            ICON_OFFLINE,
            None,
            format!("this device's key was revoked ({reason}); pair again"),
            vec![],
            unpaired_menu,
        ),
        Pairing::Disabled { reason } => (
            ICON_OFFLINE,
            None,
            DISABLED.to_owned(),
            vec![
                format!("lmgw: {reason}"),
                "Kai keeps its key: once the device is enabled, Try again connects".to_owned(),
            ],
            vec![TryAgain, PairWithLmgw, Settings, CheckForUpdates, Quit],
        ),
        Pairing::Expired { reason } => (
            ICON_OFFLINE,
            None,
            EXPIRED.to_owned(),
            vec![
                format!("lmgw: {reason}"),
                "Kai keeps its key: once its expiry is moved, Try again connects".to_owned(),
            ],
            vec![TryAgain, PairWithLmgw, Settings, CheckForUpdates, Quit],
        ),
    };
    // Shown whatever the pairing: app AEC switches the system's defaults even unpaired.
    notes.extend(audio_notes(&model.audio));
    // Overlays a tool showed stay reachable whatever the pairing does since (§10.1).
    let mut menu = menu;
    let widgets = &model.overlay.widgets;
    if !widgets.is_empty() {
        notes.push(match widgets.len() {
            1 => "1 overlay shown (Overlays hides it)".to_owned(),
            n => format!("{n} overlays shown (Overlays hides them)"),
        });
        if !menu.iter().any(|e| matches!(e, Overlays { .. })) {
            let at = menu.iter().position(|e| *e == Settings).unwrap_or(0);
            menu.insert(
                at,
                Overlays {
                    ring_over_fullscreen: model.overlay.ring_over_fullscreen,
                    widgets: widgets.clone(),
                },
            );
        }
    }
    match &model.link_wait {
        Some(LinkWait::Wallet { url }) => notes.push(format!(
            "a pairing link for {url} waits for the wallet: unlock KWallet, then Kai asks you to \
             confirm it"
        )),
        Some(LinkWait::Owner { url }) => notes.push(format!(
            "a pairing link for {url} waits for your answer in Kai's dialog"
        )),
        None => {}
    }
    if let Some(e) = &model.pair_error {
        notes.push(format!("pairing failed: {e}"));
    }
    if let Some(e) = &model.autostart_error {
        notes.push(format!("Start with Plasma: {e}"));
    }
    if let Some(e) = &model.overlay.error {
        notes.push(format!("overlay: {e}"));
    }
    if let Some(n) = &model.overlay.note {
        notes.push(format!("ring: {n}"));
    }
    if let Some(n) = &model.shortcuts {
        notes.push(format!("shortcuts: {n}"));
    }
    let title = match (&model.pairing, &model.voice.thread) {
        (Pairing::Paired { .. }, Some(t)) if !t.is_empty() => format!("Kai: {t}"),
        _ => "Kai".to_owned(),
    };
    TrayView {
        icon,
        badge,
        title,
        status,
        notes,
        menu,
    }
}

#[cfg(test)]
mod tests {
    use super::MenuEntry::*;
    use super::*;

    fn model(pairing: Pairing) -> TrayModel {
        TrayModel {
            pairing,
            autostart: true,
            pair_error: None,
            link_wait: None,
            autostart_error: None,
            audio: AudioModel::default(),
            overlay: OverlayModel::default(),
            voice: VoiceModel::default(),
            shortcuts: None,
            conversation: Vec::new(),
            tools: Vec::new(),
            desktop: Vec::new(),
            personality: PersonalityModel::default(),
        }
    }

    /// The paired menu's last entries.
    fn tail(v: &TrayView) -> &[MenuEntry] {
        &v.menu[v.menu.len() - 4..]
    }

    fn up() -> TrayModel {
        model(Pairing::Paired {
            url: "http://h".into(),
            link: Link::Up {
                device: "desktop".into(),
                held: false,
                hosts_label: None,
                admin: AdminLevel::Off,
            },
        })
    }

    fn dev(name: &str, label: &str) -> Device {
        Device {
            name: name.into(),
            label: label.into(),
        }
    }

    /// WP14: the desktop's capability line follows the tools lines while paired, up or down.
    #[test]
    fn the_desktop_capability_line_follows_the_tools_lines() {
        let line = "desktop: windows plasma, media fdo, session plasma, notify plasma".to_owned();
        let mut m = up();
        m.tools = vec!["tools: linked as kai (10 tools)".into()];
        m.desktop = vec![line.clone()];
        let notes = render(&m).notes;
        let at = notes.iter().position(|n| *n == line).unwrap();
        assert_eq!(notes[at - 1], "tools: linked as kai (10 tools)");
        m.pairing = Pairing::Paired {
            url: "http://h".into(),
            link: Link::Down {
                note: "lmgw is not reachable".into(),
            },
        };
        assert!(render(&m).notes.contains(&line));
        m.pairing = Pairing::NotPaired;
        assert!(!render(&m).notes.contains(&line), "only for a paired Kai");
    }

    #[test]
    fn not_paired_is_grey_and_offers_only_pair_updates_and_quit() {
        let v = render(&model(Pairing::NotPaired));
        assert_eq!(
            (v.icon, v.badge, v.status.as_str()),
            (ICON_OFFLINE, None, "not paired")
        );
        assert_eq!(v.menu, vec![PairWithLmgw, Settings, CheckForUpdates, Quit]);
        assert_eq!(v.title, "Kai");
        assert_eq!(
            render(&model(Pairing::Checking)).menu,
            vec![PairWithLmgw, Settings, CheckForUpdates, Quit]
        );
    }

    #[test]
    fn a_locked_wallet_is_not_shown_as_not_paired() {
        let v = render(&model(Pairing::WalletLocked { retry_s: 5 }));
        assert_eq!(v.icon, ICON_OFFLINE);
        assert_eq!(v.status, "wallet locked: unlock KWallet");
        assert_eq!(v.notes, vec!["Kai checks again every 5 s (wallet.retry_s)"]);
        assert_eq!(
            tail(&v),
            [
                StartWithPlasma { checked: true },
                Settings,
                CheckForUpdates,
                Quit
            ]
        );
        let mut m = model(Pairing::WalletLocked { retry_s: 5 });
        m.link_wait = Some(LinkWait::Wallet {
            url: "http://h:1".into(),
        });
        let v = render(&m);
        assert!(
            v.notes[1].contains("http://h:1 waits for the wallet"),
            "{:?}",
            v.notes
        );
        m.link_wait = Some(LinkWait::Owner {
            url: "http://h:1".into(),
        });
        assert!(render(&m).notes[1].contains("waits for your answer"));
    }

    #[test]
    fn a_wallet_error_names_the_error_and_the_retry() {
        let v = render(&model(Pairing::WalletError {
            message: "no answer within 10 s (wallet.timeout_s)".into(),
            retry_s: 7,
        }));
        assert_eq!(
            v.notes,
            vec![
                "no answer within 10 s (wallet.timeout_s)",
                "Kai checks again every 7 s (wallet.retry_s)"
            ]
        );
    }

    #[test]
    fn paired_but_not_connected_is_grey_with_the_url_and_the_reason() {
        let mut m = model(Pairing::Paired {
            url: "http://127.0.0.1:1".into(),
            link: Link::Down {
                note: "no lmgw link in this build yet".into(),
            },
        });
        m.autostart = false;
        let v = render(&m);
        assert_eq!((v.icon, v.status.as_str()), (ICON_OFFLINE, "not connected"));
        assert_eq!(
            v.notes,
            vec![
                "paired with lmgw at http://127.0.0.1:1",
                "no lmgw link in this build yet"
            ]
        );
        assert_eq!(
            tail(&v),
            [
                StartWithPlasma { checked: false },
                Settings,
                CheckForUpdates,
                Quit
            ]
        );
    }

    #[test]
    fn connected_is_idle_and_the_hold_is_a_badge() {
        let up = |held| {
            model(Pairing::Paired {
                url: "http://h".into(),
                link: Link::Up {
                    device: "desktop".into(),
                    held,
                    hosts_label: None,
                    admin: AdminLevel::Off,
                },
            })
        };
        let v = render(&up(false));
        assert_eq!(
            (v.icon, v.badge, v.status.as_str()),
            (ICON_IDLE, None, "idle (voice off)")
        );
        assert_eq!(v.notes, vec!["device 'desktop' on http://h"]);
        let v = render(&up(true));
        assert_eq!((v.icon, v.badge), (ICON_IDLE, Some(BADGE_HELD)));
        assert!(v.notes.iter().any(|n| n.contains("GPU hold")));
    }

    #[test]
    fn the_admin_tools_level_is_a_note_unless_it_is_off() {
        let with = |level: &str| {
            let mut m = up();
            if let Pairing::Paired {
                link: Link::Up { admin, .. },
                ..
            } = &mut m.pairing
            {
                // As the feed reads it: lmgw's typed level.
                *admin = serde_json::from_value(serde_json::json!(level)).unwrap();
            }
            render(&m).notes
        };
        assert_eq!(with("off"), ["device 'desktop' on http://h"]);
        // What the tools may do, lmgw's own level included (lmgw 3ac1155): no "up to".
        assert_eq!(
            with("read_only"),
            ["device 'desktop' on http://h", "admin tools: read only"]
        );
        assert_eq!(
            with("full"),
            ["device 'desktop' on http://h", "admin tools: full"]
        );
        // A level a newer lmgw added is shown by its name, in words.
        assert_eq!(
            with("ask_first"),
            ["device 'desktop' on http://h", "admin tools: ask first"]
        );
        assert_eq!(with(""), ["device 'desktop' on http://h"]);
    }

    #[test]
    fn revoked_says_pair_again_with_the_reason_and_keeps_only_pair() {
        let v = render(&model(Pairing::Revoked {
            reason: "device 'desktop' was rotated — pair it again".into(),
        }));
        assert_eq!(v.icon, ICON_OFFLINE);
        assert_eq!(
            v.status,
            "this device's key was revoked (device 'desktop' was rotated — pair it again); pair again"
        );
        assert_eq!(v.menu, vec![PairWithLmgw, Settings, CheckForUpdates, Quit]);
    }

    /// lmgw 7d6d18c: an expired key waits, as a disabled device does; its expiry can be moved.
    #[test]
    fn expired_says_move_its_expiry_or_pair_again_and_offers_try_again() {
        let v = render(&model(Pairing::Expired {
            reason: "device 'desktop' expired on 2026-10-07".into(),
        }));
        assert_eq!(v.icon, ICON_OFFLINE);
        assert_eq!(
            v.status,
            "this device's key expired on lmgw: move its expiry on Usage → Keys, or pair again"
        );
        assert_eq!(
            v.notes,
            [
                "lmgw: device 'desktop' expired on 2026-10-07",
                "Kai keeps its key: once its expiry is moved, Try again connects"
            ]
        );
        assert_eq!(
            v.menu,
            vec![TryAgain, PairWithLmgw, Settings, CheckForUpdates, Quit]
        );
    }

    /// Review KAI-3 K3-6.
    #[test]
    fn disabled_says_enable_it_or_pair_again_and_offers_try_again() {
        let v = render(&model(Pairing::Disabled {
            reason: "device 'desktop' is disabled — enable it on Usage → Keys".into(),
        }));
        assert_eq!(v.icon, ICON_OFFLINE);
        assert_eq!(
            v.status,
            "this device was disabled on lmgw: enable it on Usage → Keys, or pair again"
        );
        assert_eq!(
            v.notes,
            [
                "lmgw: device 'desktop' is disabled — enable it on Usage → Keys",
                "Kai keeps its key: once the device is enabled, Try again connects"
            ]
        );
        assert_eq!(
            v.menu,
            vec![TryAgain, PairWithLmgw, Settings, CheckForUpdates, Quit]
        );
        assert_eq!(TryAgain.label(), "Try again");
    }

    #[test]
    fn a_ring_that_is_down_is_said_in_the_tooltip() {
        let mut m = up();
        m.overlay.note = Some("its web process ended twice, so the ring is down".into());
        let v = render(&m);
        assert!(
            v.notes
                .contains(&"ring: its web process ended twice, so the ring is down".to_owned()),
            "{:?}",
            v.notes
        );
        m.overlay.note = None;
        assert!(!render(&m).notes.iter().any(|n| n.starts_with("ring:")));
    }

    #[test]
    fn errors_are_appended_as_notes() {
        let mut m = model(Pairing::NotPaired);
        m.pair_error = Some("the link has no `key` parameter".into());
        m.autostart_error = Some("cannot write".into());
        let v = render(&m);
        assert_eq!(
            &v.notes[1..],
            [
                "pairing failed: the link has no `key` parameter",
                "Start with Plasma: cannot write"
            ]
        );
    }

    #[test]
    fn the_paired_menu_has_echo_mode_and_the_device_menus_in_section_7_order() {
        let mut m = up();
        m.audio.mode = EchoMode::App;
        m.audio.devices = DeviceList {
            inputs: vec![dev("mic", "USB mic")],
            outputs: vec![dev("hdmi", "HDMI"), dev("spk", "Speakers")],
            default_input: Some("mic".into()),
            default_output: Some("spk".into()),
        };
        m.audio.output = Some("hdmi".into());
        let v = render(&m);
        assert_eq!(
            &v.menu[..6],
            [
                Listening { on: false },
                Mute { checked: false },
                Mode {
                    selected: VoiceMode::Automatic
                },
                NewConversation,
                OpenInLmgw,
                Separator
            ],
            "items 1-5 first (no thread known: no Continue)"
        );
        let v = TrayView {
            menu: v.menu[6..].to_vec(),
            ..v
        };
        assert_eq!(
            v.menu[0],
            Echo {
                selected: EchoMode::App
            }
        );
        let MenuEntry::Devices {
            kind,
            options,
            selected,
        } = &v.menu[1]
        else {
            panic!("{:?}", v.menu)
        };
        assert_eq!((*kind, *selected), (DeviceKind::Microphone, 0));
        assert_eq!(options[0].label, "Default (USB mic)");
        assert_eq!(options[1].node.as_deref(), Some("mic"));
        let MenuEntry::Devices {
            kind,
            options,
            selected,
        } = &v.menu[2]
        else {
            panic!()
        };
        assert_eq!((*kind, *selected), (DeviceKind::Speaker, 1));
        assert_eq!(options[1].label, "HDMI");
        assert_eq!(
            v.menu[3],
            Overlays {
                ring_over_fullscreen: true,
                widgets: vec![]
            },
            "item 8"
        );
        assert_eq!(v.menu[4], Separator);
        assert_eq!(
            tail(&v),
            [
                StartWithPlasma { checked: true },
                Settings,
                CheckForUpdates,
                Quit
            ]
        );
        // Not paired: only pairing, the update check and Quit (§3.1).
        let mut m2 = m.clone();
        m2.pairing = Pairing::NotPaired;
        assert_eq!(
            render(&m2).menu,
            vec![PairWithLmgw, Settings, CheckForUpdates, Quit]
        );
    }

    #[test]
    fn the_overlays_menu_shows_the_setting_and_a_save_error() {
        let mut m = up();
        m.overlay.ring_over_fullscreen = false;
        m.overlay.error = Some("cannot save overlay.ring_over_fullscreen: denied".into());
        let v = render(&m);
        assert!(v.menu.contains(&Overlays {
            ring_over_fullscreen: false,
            widgets: vec![]
        }));
        assert!(
            v.notes
                .iter()
                .any(|n| n == "overlay: cannot save overlay.ring_over_fullscreen: denied")
        );
        m.pairing = Pairing::NotPaired;
        assert_eq!(
            render(&m).menu,
            vec![PairWithLmgw, Settings, CheckForUpdates, Quit],
            "only when paired"
        );
    }

    #[test]
    fn a_pick_that_is_not_present_stays_selected_and_marked() {
        let (options, selected) =
            device_options(&[dev("spk", "Speakers")], Some("gone"), Some("usb"));
        assert_eq!(
            options[0].label, "Default",
            "the default is not in the list"
        );
        assert_eq!(selected, 2);
        assert_eq!(options[2].label, "usb (not present)");
        assert_eq!(options[2].node.as_deref(), Some("usb"));
    }

    #[test]
    fn the_device_warning_offers_half_duplex_and_the_echo_note_shows() {
        let mut m = up();
        m.audio.echo = Some(crate::audio::EchoStatus {
            chosen: crate::audio::EchoMode::Device,
            effective: crate::audio::EchoMode::Device,
            note: Some("echo: Kai plays on 'hdmi', not the default output".into()),
            offer_half_duplex: true,
        });
        m.audio.link = Some("PipeWire is not reachable; retrying every 2 s (audio.retry_s)".into());
        let v = render(&m);
        assert_eq!(
            v.menu[7], UseHalfDuplex,
            "after the voice's six entries and Echo mode"
        );
        assert_eq!(
            &v.notes[1..],
            [
                "audio: PipeWire is not reachable; retrying every 2 s (audio.retry_s)",
                "echo: Kai plays on 'hdmi', not the default output"
            ]
        );
        // The notes show unpaired too (app AEC switches the defaults either way); the menus do not.
        m.pairing = Pairing::NotPaired;
        let v = render(&m);
        assert!(v.notes.iter().any(|n| n.contains("echo")));
        assert_eq!(v.menu, vec![PairWithLmgw, Settings, CheckForUpdates, Quit]);
    }

    fn voice(shown: VoiceShown) -> TrayModel {
        let mut m = up();
        m.voice = VoiceModel {
            on: true,
            shown,
            thread: Some("Kai".into()),
            ..VoiceModel::default()
        };
        m
    }

    #[test]
    fn the_icon_follows_section_5_1s_order() {
        for (shown, icon, status) in [
            (VoiceShown::Off, ICON_IDLE, "idle (voice off)"),
            (VoiceShown::Connecting, ICON_IDLE, "connecting the voice"),
            (VoiceShown::Quiet, ICON_LISTENING, "listening on"),
            (VoiceShown::Listening, ICON_LISTENING, "listening"),
            (VoiceShown::Interrupted, ICON_LISTENING, "listening"),
            (VoiceShown::Thinking, ICON_THINKING, "thinking"),
            (VoiceShown::Speaking, ICON_SPEAKING, "speaking"),
        ] {
            let v = render(&voice(shown));
            assert_eq!((v.icon, v.status.as_str()), (icon, status), "{shown:?}");
        }
        // Muted wins over every voice state; unreachable wins over muted.
        let mut m = voice(VoiceShown::Speaking);
        m.voice.muted = true;
        let v = render(&m);
        assert_eq!(
            (v.icon, v.status.as_str()),
            (ICON_MUTED, "muted: Kai hears silence")
        );
        m.pairing = Pairing::Paired {
            url: "http://h".into(),
            link: Link::Down {
                note: "lmgw is not reachable".into(),
            },
        };
        let v = render(&m);
        assert_eq!(v.icon, ICON_OFFLINE);
        assert!(v.notes.contains(&"muted".to_owned()));
        // Push-to-talk's quiet listening says how to talk.
        let mut m = voice(VoiceShown::Quiet);
        m.voice.mode = VoiceMode::PushToTalk;
        assert_eq!(render(&m).status, "listening on: hold the talk key to talk");
    }

    #[test]
    fn the_tooltip_has_the_thread_the_timing_line_and_the_notes() {
        let mut m = voice(VoiceShown::Quiet);
        m.voice.timing = Some("first audio 800 ms (turn end 400), chat gemma4-e4b".into());
        m.voice.notes = vec!["voice in use on device 'phone'".into()];
        m.voice.held = true;
        m.shortcuts = Some("no key set for Talk".into());
        m.conversation =
            vec!["the folder 'Kai' keeps its past threads by lmgw's global retention: …".into()];
        let v = render(&m);
        assert_eq!(v.title, "Kai: Kai");
        assert_eq!(v.badge, Some(BADGE_HELD), "a held stage shows the badge");
        assert_eq!(
            v.notes,
            [
                "first audio 800 ms (turn end 400), chat gemma4-e4b",
                "device 'desktop' on http://h",
                "GPU hold: local models are paused",
                "voice in use on device 'phone'",
                "the folder 'Kai' keeps its past threads by lmgw's global retention: …",
                "shortcuts: no key set for Talk"
            ]
        );
        assert_eq!(
            &v.menu[..7],
            [
                Listening { on: true },
                Mute { checked: false },
                Mode {
                    selected: VoiceMode::Automatic
                },
                Continue {
                    title: "Kai".into()
                },
                NewConversation,
                OpenInLmgw,
                Separator
            ]
        );
        assert_eq!(v.menu[5].label(), "Open in lmgw");
        assert_eq!(v.menu[0].label(), "Stop listening");
        // Not paired: the title stays "Kai" and only pairing (and the update check) is offered.
        m.pairing = Pairing::NotPaired;
        let v = render(&m);
        assert_eq!((v.title.as_str(), v.menu.len()), ("Kai", 4));
    }

    /// WP10: the overlays a tool showed are listed under Overlays with their Hide, and stay
    /// reachable when the pairing goes (§10.1: nothing lingers unseen).
    #[test]
    fn shown_overlays_are_listed_whatever_the_pairing() {
        let mut m = up();
        let shown = vec![
            (3, "3 text, top-right since 12:00:00".to_owned()),
            (5, "5 html, top-left (loading)".to_owned()),
        ];
        m.overlay.widgets = shown.clone();
        let v = render(&m);
        assert!(v.menu.contains(&Overlays {
            ring_over_fullscreen: true,
            widgets: shown.clone()
        }));
        assert!(
            v.notes
                .iter()
                .any(|n| n == "2 overlays shown (Overlays hides them)")
        );
        m.pairing = Pairing::NotPaired;
        assert_eq!(
            render(&m).menu,
            vec![
                PairWithLmgw,
                Overlays {
                    ring_over_fullscreen: true,
                    widgets: shown
                },
                Settings,
                CheckForUpdates,
                Quit
            ]
        );
        m.overlay.widgets.truncate(1);
        assert!(
            render(&m)
                .notes
                .iter()
                .any(|n| n == "1 overlay shown (Overlays hides it)")
        );
        m.overlay.widgets.clear();
        assert_eq!(
            render(&m).menu,
            vec![PairWithLmgw, Settings, CheckForUpdates, Quit]
        );
    }
}
