//! The StatusNotifierItem (§7) through ksni on tokio. It renders the core's [`TrayModel`] and sends
//! menu choices back as [`Command`]s. Left click toggles listening, middle click mutes (§7). The
//! listening mode, Echo mode, Microphone and Speaker entries are radio submenus; the device lists
//! follow PipeWire live (the audio thread's events update the model). Overlays holds the ring's
//! check item "Ring over fullscreen windows", then the tool-controlled overlays shown, each with its
//! Hide, and Hide all (WP10; they go straight to `crate::widgets`, not through the core). Routines
//! (WP20, K28), before Check for Updates while any routine folder or switch exists: Run <name> for
//! each that is on and can run from here, Stop <name> for each run from here or a key that has not
//! ended, then one check item per routine to switch it on (Kai shows it in full first) or off (a
//! changed one says so and is off), and Forget <name> for a switch whose folder is gone; they go
//! straight to `crate::routines`. Jobs (WP18, K26: nothing runs unseen), beside Routines while any
//! job is known: one submenu per job (its tool, id, start and caller) with the switch whether its
//! result is spoken and Cancel for a running one; they go straight to `crate::jobs`.

mod icons;
pub mod model;

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip, TrayMethods};
use tokio::sync::{mpsc, watch};

use crate::audio::EchoMode;
use crate::core::Command;
use crate::voice::VoiceMode;
use model::{DeviceKind, HIDE_ALL, MenuEntry, RING_OVER_FULLSCREEN, TrayModel, TrayView, render};

struct KaiTray {
    view: TrayView,
    cmds: mpsc::UnboundedSender<Command>,
    routines: Vec<crate::routines::TrayRoutine>,
    jobs: Vec<crate::jobs::TrayJob>,
}

impl ksni::Tray for KaiTray {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        self.view.title.clone()
    }

    // Each icon goes by name and as pixmaps; the host falls back to the pixmaps when the name does
    // not resolve (see `icons`).
    fn icon_name(&self) -> String {
        self.view.icon.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icons::pixmaps(self.view.icon)
    }

    /// Left click (§7): listening on or off.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::ToggleListening);
    }

    /// Middle click (§7, ruling 11): mute.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::ToggleMute);
    }

    fn overlay_icon_name(&self) -> String {
        self.view.badge.unwrap_or_default().into()
    }

    fn overlay_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.view.badge.map(icons::pixmaps).unwrap_or_default()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: self.view.icon.into(),
            icon_pixmap: icons::pixmaps(self.view.icon),
            title: self.view.title.clone(),
            description: tooltip_markup(&self.view),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items = Vec::new();
        for entry in &self.view.menu {
            if matches!(entry, MenuEntry::CheckForUpdates) {
                if !self.routines.is_empty() {
                    items.push(MenuItem::Separator);
                    items.push(routines_menu(&self.routines));
                }
                if !self.jobs.is_empty() {
                    items.push(MenuItem::Separator);
                    items.push(jobs_menu(&self.jobs));
                }
                items.push(MenuItem::Separator);
            }
            items.push(match entry {
                MenuEntry::Listening { on } => {
                    let on = *on;
                    StandardItem {
                        label: entry.label().into(),
                        icon_name: if on {
                            "media-playback-stop"
                        } else {
                            "audio-input-microphone"
                        }
                        .into(),
                        activate: Box::new(move |t: &mut Self| t.send(Command::SetListening(!on))),
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Mute { checked } => {
                    let checked = *checked;
                    CheckmarkItem {
                        label: entry.label().into(),
                        checked,
                        activate: Box::new(move |t: &mut Self| t.send(Command::SetMuted(!checked))),
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Mode { selected } => SubMenu {
                    label: entry.label().into(),
                    submenu: vec![
                        RadioGroup {
                            selected: VoiceMode::ALL
                                .iter()
                                .position(|m| m == selected)
                                .unwrap_or(0),
                            select: Box::new(|t: &mut Self, i| {
                                t.send(Command::SetVoiceMode(VoiceMode::ALL[i]))
                            }),
                            options: VoiceMode::ALL
                                .iter()
                                .map(|m| RadioItem {
                                    label: m.label().into(),
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
                MenuEntry::Continue { title } => StandardItem {
                    label: menu_text(&format!("Continue \"{title}\"")),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                MenuEntry::NewConversation => StandardItem {
                    label: entry.label().into(),
                    icon_name: "list-add".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::NewConversation)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::OpenInLmgw => StandardItem {
                    label: entry.label().into(),
                    icon_name: "internet-web-browser".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::OpenInLmgw)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::Personality { options, selected } => {
                    let ids: Vec<Option<i64>> = options.iter().map(|o| o.id).collect();
                    SubMenu {
                        label: entry.label().into(),
                        submenu: vec![
                            RadioGroup {
                                selected: *selected,
                                select: Box::new(move |t: &mut Self, i| {
                                    if let Some(id) = ids.get(i) {
                                        t.send(Command::SetPersonality(*id))
                                    }
                                }),
                                options: options
                                    .iter()
                                    .map(|o| RadioItem {
                                        label: menu_text(&o.label),
                                        ..Default::default()
                                    })
                                    .collect(),
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Echo { selected } => SubMenu {
                    label: entry.label().into(),
                    submenu: vec![
                        RadioGroup {
                            selected: EchoMode::ALL
                                .iter()
                                .position(|m| m == selected)
                                .unwrap_or(0),
                            select: Box::new(|t: &mut Self, i| {
                                t.send(Command::SetEchoMode(EchoMode::ALL[i]))
                            }),
                            options: EchoMode::ALL
                                .iter()
                                .map(|m| RadioItem {
                                    label: m.label().into(),
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
                MenuEntry::UseHalfDuplex => StandardItem {
                    label: entry.label().into(),
                    icon_name: "dialog-warning".into(),
                    activate: Box::new(|t: &mut Self| {
                        t.send(Command::SetEchoMode(EchoMode::HalfDuplex))
                    }),
                    ..Default::default()
                }
                .into(),
                MenuEntry::Devices {
                    kind,
                    options,
                    selected,
                } => {
                    let nodes: Vec<Option<String>> =
                        options.iter().map(|o| o.node.clone()).collect();
                    let kind = *kind;
                    SubMenu {
                        label: entry.label().into(),
                        submenu: vec![
                            RadioGroup {
                                selected: *selected,
                                select: Box::new(move |t: &mut Self, i| {
                                    let node = nodes.get(i).cloned().flatten();
                                    t.send(match kind {
                                        DeviceKind::Microphone => Command::SetInput(node),
                                        DeviceKind::Speaker => Command::SetOutput(node),
                                    })
                                }),
                                options: options
                                    .iter()
                                    .map(|o| RadioItem {
                                        label: menu_text(&o.label),
                                        ..Default::default()
                                    })
                                    .collect(),
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Overlays {
                    ring_over_fullscreen,
                    widgets,
                } => {
                    let on = *ring_over_fullscreen;
                    let mut submenu: Vec<MenuItem<Self>> = vec![
                        CheckmarkItem {
                            label: RING_OVER_FULLSCREEN.into(),
                            checked: on,
                            activate: Box::new(move |t: &mut Self| {
                                t.send(Command::SetRingOverFullscreen(!on))
                            }),
                            ..Default::default()
                        }
                        .into(),
                    ];
                    if !widgets.is_empty() {
                        submenu.push(MenuItem::Separator);
                        for (id, label) in widgets {
                            let id = *id;
                            submenu.push(
                                StandardItem {
                                    label: menu_text(&format!("Hide {label}")),
                                    activate: Box::new(move |_: &mut Self| {
                                        crate::widgets::hide(Some(id))
                                    }),
                                    ..Default::default()
                                }
                                .into(),
                            );
                        }
                        submenu.push(
                            StandardItem {
                                label: HIDE_ALL.into(),
                                icon_name: "edit-clear-all".into(),
                                activate: Box::new(|_: &mut Self| crate::widgets::hide(None)),
                                ..Default::default()
                            }
                            .into(),
                        );
                    }
                    SubMenu {
                        label: entry.label().into(),
                        submenu,
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Separator => MenuItem::Separator,
                MenuEntry::TryAgain => StandardItem {
                    label: entry.label().into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::TryAgain)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::PairWithLmgw => StandardItem {
                    label: entry.label().into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::OpenPairDialog)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::StartWithPlasma { checked } => {
                    let checked = *checked;
                    CheckmarkItem {
                        label: entry.label().into(),
                        checked,
                        activate: Box::new(move |t: &mut Self| {
                            t.send(Command::SetAutostart(!checked))
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                MenuEntry::Settings => StandardItem {
                    label: entry.label().into(),
                    icon_name: "configure".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::OpenSettings)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::CheckForUpdates => StandardItem {
                    label: entry.label().into(),
                    icon_name: "system-software-update".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::CheckForUpdates)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::Quit => StandardItem {
                    label: entry.label().into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::Quit)),
                    ..Default::default()
                }
                .into(),
            });
        }
        items
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::warn!(
            ?reason,
            "the tray host went away; the item comes back when it returns"
        );
        true
    }

    fn watcher_online(&self) {
        tracing::info!("the tray host is back");
    }
}

/// The Routines submenu (module doc).
fn routines_menu(routines: &[crate::routines::TrayRoutine]) -> MenuItem<KaiTray> {
    use crate::routines::{Trigger, trigger};
    let mut submenu: Vec<MenuItem<KaiTray>> = Vec::new();
    for r in routines.iter().filter(|r| r.runnable) {
        let name = r.name.clone();
        submenu.push(
            StandardItem {
                label: menu_text(&format!("Run {}", r.name)),
                icon_name: "media-playback-start".into(),
                activate: Box::new(move |_: &mut KaiTray| trigger(Trigger::Run(name.clone()))),
                ..Default::default()
            }
            .into(),
        );
    }
    for r in routines.iter().filter(|r| r.running) {
        let name = r.name.clone();
        submenu.push(
            StandardItem {
                label: menu_text(&format!("Stop {}", r.name)),
                icon_name: "media-playback-stop".into(),
                activate: Box::new(move |_: &mut KaiTray| trigger(Trigger::Stop(name.clone()))),
                ..Default::default()
            }
            .into(),
        );
    }
    if !submenu.is_empty() {
        submenu.push(MenuItem::Separator);
    }
    for r in routines.iter().filter(|r| r.orphan) {
        let name = r.name.clone();
        submenu.push(
            StandardItem {
                label: menu_text(&format!(
                    "Forget the switch of {} ({})",
                    r.name,
                    r.note.as_deref().unwrap_or("its folder is gone")
                )),
                icon_name: "edit-delete".into(),
                activate: Box::new(move |_: &mut KaiTray| trigger(Trigger::Forget(name.clone()))),
                ..Default::default()
            }
            .into(),
        );
    }
    for r in routines.iter().filter(|r| !r.orphan) {
        let (name, on) = (r.name.clone(), r.on);
        let label = match &r.note {
            Some(note) => format!("{} ({note})", r.name),
            None => r.name.clone(),
        };
        submenu.push(
            CheckmarkItem {
                label: menu_text(&label),
                checked: on,
                activate: Box::new(move |_: &mut KaiTray| {
                    trigger(Trigger::Switch(name.clone(), !on))
                }),
                ..Default::default()
            }
            .into(),
        );
    }
    SubMenu {
        label: "Routines".into(),
        submenu,
        ..Default::default()
    }
    .into()
}

/// The Jobs submenu (module doc).
fn jobs_menu(jobs: &[crate::jobs::TrayJob]) -> MenuItem<KaiTray> {
    let submenu = jobs
        .iter()
        .map(|j| {
            let mut items: Vec<MenuItem<KaiTray>> = Vec::new();
            let (id, on) = (j.id.clone(), j.announce);
            items.push(
                CheckmarkItem {
                    label: "Speak the result".into(),
                    checked: on,
                    activate: Box::new(move |_: &mut KaiTray| crate::jobs::tray_announce(&id, !on)),
                    ..Default::default()
                }
                .into(),
            );
            if j.running {
                let id = j.id.clone();
                items.push(
                    StandardItem {
                        label: "Cancel".into(),
                        icon_name: "process-stop".into(),
                        activate: Box::new(move |_: &mut KaiTray| crate::jobs::tray_cancel(&id)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
            SubMenu {
                label: menu_text(&j.label),
                submenu: items,
                ..Default::default()
            }
            .into()
        })
        .collect();
    let running = jobs.iter().filter(|j| j.running).count();
    SubMenu {
        label: format!("Jobs ({running} running)"),
        submenu,
        ..Default::default()
    }
    .into()
}

impl KaiTray {
    fn send(&self, cmd: Command) {
        // Fails only while Kai quits.
        let _ = self.cmds.send(cmd);
    }
}

/// The tooltip's jobs line while any job runs (K26: nothing runs unseen).
fn jobs_line(jobs: &[crate::jobs::TrayJob]) -> Option<String> {
    let running = jobs.iter().filter(|j| j.running).count();
    match running {
        0 => None,
        1 => Some("1 job running (the menu's Jobs)".to_owned()),
        n => Some(format!("{n} jobs running (the menu's Jobs)")),
    }
}

/// The tooltip body: the SNI description takes a subset of HTML, so text is escaped and lines are
/// joined with `<br/>`.
pub fn tooltip_markup(view: &TrayView) -> String {
    std::iter::once(&view.status)
        .chain(&view.notes)
        .map(|l| escape(l))
        .collect::<Vec<_>>()
        .join("<br/>")
}

/// A label as a menu shows it literally: an underscore marks an access key, so device names
/// (`alsa_output.usb_…`) get theirs doubled.
fn menu_text(s: &str) -> String {
    s.replace('_', "__")
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Runs the tray until the core drops the model's sender (quit), then removes the item.
pub async fn run(mut model: watch::Receiver<TrayModel>, cmds: mpsc::UnboundedSender<Command>) {
    // The desktop's capability table (WP14) and the tool-controlled overlays (WP10) join the
    // core's model here.
    let mut desktop = crate::desktop::tray_lines();
    let mut widgets = crate::widgets::tray_entries();
    let shown = |model: &mut watch::Receiver<TrayModel>,
                 desktop: &mut watch::Receiver<Vec<String>>,
                 widgets: &mut watch::Receiver<Vec<(u64, String)>>| {
        let mut m = model.borrow_and_update().clone();
        m.desktop = desktop.borrow_and_update().clone();
        m.overlay.widgets = widgets.borrow_and_update().clone();
        render(&m)
    };
    let mut routines = crate::routines::tray_entries();
    let mut jobs = crate::jobs::tray_entries();
    let mut view = shown(&mut model, &mut desktop, &mut widgets);
    view.notes.extend(jobs_line(&jobs.borrow()));
    let tray = KaiTray {
        view,
        cmds,
        routines: routines.borrow_and_update().clone(),
        jobs: jobs.borrow_and_update().clone(),
    };
    // At login the tray host may come up after Kai; the item then appears when it does.
    let handle = match tray.assume_sni_available(true).spawn().await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("cannot show the tray item: {e}");
            return;
        }
    };
    tracing::info!("tray item registered");
    loop {
        tokio::select! {
            changed = model.changed() => if changed.is_err() { break },
            changed = desktop.changed() => if changed.is_err() { break },
            changed = widgets.changed() => if changed.is_err() { break },
            changed = routines.changed() => if changed.is_err() { break },
            changed = jobs.changed() => if changed.is_err() { break },
        }
        let mut view = shown(&mut model, &mut desktop, &mut widgets);
        let listed = routines.borrow_and_update().clone();
        let job_list = jobs.borrow_and_update().clone();
        view.notes.extend(jobs_line(&job_list));
        tracing::debug!(status = %view.status, "tray");
        if handle
            .update(|t| {
                t.view = view;
                t.routines = listed;
                t.jobs = job_list;
            })
            .await
            .is_none()
        {
            tracing::warn!("the tray service ended");
            return;
        }
    }
    handle.shutdown().await;
    tracing::debug!("tray item removed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_is_escaped_markup() {
        let v = TrayView {
            icon: model::ICON_OFFLINE,
            badge: None,
            title: "Kai".into(),
            status: "this device's key was revoked (a <b> & c); pair again".into(),
            notes: vec!["second".into()],
            menu: vec![],
        };
        assert_eq!(
            tooltip_markup(&v),
            "this device's key was revoked (a &lt;b&gt; &amp; c); pair again<br/>second"
        );
    }
}
