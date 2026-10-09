//! The Windows capability on Plasma.
//!
//! - **The facts:** from the window management protocol where KWin grants it ([`super::wm`], live,
//!   with the desktops' names, the current desktop and KWin's active output from KWin's D-Bus);
//!   else, and when that connection ended, from KWin scripting (WP13 (f): no grant, about 5 ms a
//!   round trip, one consistent snapshot). Both list the windows a task bar shows, by the same
//!   ids, which ScreenShot2 takes too.
//! - **The actions:** KWin scripting, granted or not: one path for every action, tiling included
//!   (the protocol has no tiling). Reading a geometry back in the script that set it is stale
//!   (Wayland configures asynchronously), so an action answers what it asked KWin to do, and the
//!   next snapshot shows the result.
//!
//! The current activity's name comes from `org.kde.ActivityManager` when it runs.

use std::time::Duration;

use kai_desktop::caps::windows::{Action, Done, Output, Snapshot, Window, Windows};
use kai_desktop::caps::{Failure, Fut};
use serde::Deserialize;
use serde_json::json;

use super::kwin_script::KwinScript;
use super::wm::{Extra, Wm};
use crate::desktop::{call, property};
use crate::host::registry::SessionBus;

/// The snapshot: normal windows and dialogs, topmost first.
const SNAPSHOT: &str = r#"
function run() {
    const active = workspace.activeWindow;
    const order = workspace.stackingOrder.slice().reverse();
    const windows = order.filter(w => (w.normalWindow || w.dialog) && !w.skipTaskbar && !w.deleted).map(w => ({
        id: bare(w.internalId), app: w.desktopFileName || w.resourceClass || "", title: w.caption,
        pid: w.pid, output: w.output ? w.output.name : "",
        desktops: w.onAllDesktops ? [] : w.desktops.map(d => d.name),
        active: w === active, minimized: w.minimized, maximized: w.maximizeMode === 3,
        fullscreen: w.fullScreen }));
    const outputs = workspace.screens.map(s => ({ name: s.name, x: s.geometry.x, y: s.geometry.y,
        width: s.geometry.width, height: s.geometry.height, scale: s.devicePixelRatio,
        active: s === workspace.activeScreen }));
    return { windows: windows, outputs: outputs, desktops: workspace.desktops.map(d => d.name),
        desktop: workspace.currentDesktop ? workspace.currentDesktop.name : "",
        activity: workspace.currentActivity || "" };
}
"#;

/// One action on one window: ARGS.window, ARGS.action, ARGS.desktop (a position from 0),
/// ARGS.output, ARGS.side. A
/// window that is gone answers `{gone: true}`; otherwise the word for what it asked KWin to do.
const ACT: &str = r#"
const SLOTS = { "left": "slotWindowQuickTileLeft", "right": "slotWindowQuickTileRight",
    "top": "slotWindowQuickTileTop", "bottom": "slotWindowQuickTileBottom",
    "top-left": "slotWindowQuickTileTopLeft", "top-right": "slotWindowQuickTileTopRight",
    "bottom-left": "slotWindowQuickTileBottomLeft", "bottom-right": "slotWindowQuickTileBottomRight" };
function run() {
    const w = byId(ARGS.window);
    if (!w) return { gone: true };
    switch (ARGS.action) {
    case "activate":
        if (w.minimized) w.minimized = false;
        workspace.activeWindow = w;
        return { done: "activated" };
    case "close":
        w.closeWindow();
        return { done: "asked to close" };
    case "minimize":
        w.minimized = true;
        return { done: "minimized" };
    case "maximize": {
        const on = w.maximizeMode !== 3;
        w.setMaximize(on, on);
        return { done: on ? "maximized" : "restored" };
    }
    case "fullscreen": {
        const on = !w.fullScreen;
        w.fullScreen = on;
        return { done: on ? "fullscreen" : "out of fullscreen" };
    }
    case "to_desktop": {
        const d = workspace.desktops[ARGS.desktop];
        if (!d) throw new Error("that virtual desktop is gone");
        w.desktops = [d];
        return { done: "moved to " + d.name };
    }
    case "to_output": {
        const s = workspace.screens.find(s => s.name === ARGS.output);
        if (!s) throw new Error("the output is gone");
        workspace.sendClientToScreen(w, s);
        return { done: "moved to " + s.name };
    }
    case "tile":
        if (ARGS.side === "none") {
            w.tile = null;
            if (w.maximizeMode !== 0) w.setMaximize(false, false);
            return { done: "untiled" };
        }
        if (w.minimized) w.minimized = false;
        workspace.activeWindow = w;
        workspace[SLOTS[ARGS.side]]();
        return { done: "tiled " + ARGS.side };
    }
    throw new Error("no such action");
}
"#;

/// The window with the keyboard focus, any window; `null` when none has it.
const ACTIVE: &str = r#"
function run() {
    const w = workspace.activeWindow;
    if (!w) return null;
    return { id: bare(w.internalId), app: w.desktopFileName || w.resourceClass || "", title: w.caption,
        pid: w.pid };
}
"#;

#[derive(Deserialize)]
struct RawActive {
    id: String,
    app: String,
    title: String,
    pid: u32,
}

#[derive(Deserialize)]
struct RawSnapshot {
    windows: Vec<RawWindow>,
    outputs: Vec<RawOutput>,
    desktops: Vec<String>,
    desktop: String,
    activity: String,
}

#[derive(Deserialize)]
struct RawWindow {
    id: String,
    app: String,
    title: String,
    pid: u32,
    output: String,
    desktops: Vec<String>,
    active: bool,
    minimized: bool,
    maximized: bool,
    fullscreen: bool,
}

#[derive(Deserialize)]
struct RawOutput {
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    active: bool,
}

#[derive(Deserialize)]
struct RawDone {
    #[serde(default)]
    gone: bool,
    #[serde(default)]
    done: String,
}

/// The Windows capability on Plasma (module doc).
pub struct PlasmaWindows {
    /// The window management protocol's connection, where KWin granted it.
    wm: Option<Wm>,
    script: KwinScript,
    bus: SessionBus,
    timeout: Duration,
    /// The protocol's end was logged.
    said_ended: std::sync::atomic::AtomicBool,
}

const ACTIVITIES: &str = "org.kde.ActivityManager";
const ACTIVITIES_PATH: &str = "/ActivityManager/Activities";
const ACTIVITIES_IFACE: &str = "org.kde.ActivityManager.Activities";

impl PlasmaWindows {
    pub fn new(wm: Option<Wm>, script: KwinScript, bus: SessionBus, timeout: Duration) -> Self {
        Self {
            wm,
            script,
            bus,
            timeout,
            said_ended: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The activity's name from the activity manager; its id when that does not answer, and
    /// nothing for KWin's null activity (no activity manager).
    async fn activity_name(&self, id: &str) -> String {
        if id.is_empty() || id.chars().all(|c| c == '0' || c == '-') {
            return String::new();
        }
        let Ok(bus) = self.bus.get().await else {
            return id.to_owned();
        };
        call::<_, String>(
            &bus,
            ACTIVITIES,
            ACTIVITIES_PATH,
            ACTIVITIES_IFACE,
            "ActivityName",
            &(id,),
            self.timeout,
        )
        .await
        .unwrap_or_else(|_| id.to_owned())
    }

    /// The live protocol's connection, unless it ended (said in the log once).
    fn live(&self) -> Option<&Wm> {
        let wm = self.wm.as_ref()?;
        if wm.ended().is_some() {
            if !self
                .said_ended
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                tracing::info!("desktop: window facts from KWin scripting from now on");
            }
            return None;
        }
        Some(wm)
    }

    /// What the protocol's table lacks, from KWin's D-Bus: the desktops' names and the current
    /// one (required), the activity and the active output (when they answer).
    async fn extra(&self) -> Result<Extra, Failure> {
        let bus = self.bus.get().await.map_err(Failure::Failed)?;
        const KWIN: &str = "org.kde.KWin";
        const VDM: &str = "/VirtualDesktopManager";
        const VDM_IFACE: &str = "org.kde.KWin.VirtualDesktopManager";
        let t = self.timeout;
        let desktops =
            property::<Vec<(u32, String, String)>>(&bus, KWIN, VDM, VDM_IFACE, "desktops", t);
        let current = property::<String>(&bus, KWIN, VDM, VDM_IFACE, "current", t);
        let activity = call::<_, String>(
            &bus,
            ACTIVITIES,
            ACTIVITIES_PATH,
            ACTIVITIES_IFACE,
            "CurrentActivity",
            &(),
            t,
        );
        let output = call::<_, String>(
            &bus,
            KWIN,
            "/KWin",
            "org.kde.KWin",
            "activeOutputName",
            &(),
            t,
        );
        let (desktops, current, activity, output) =
            tokio::join!(desktops, current, activity, output);
        let mut desktops = desktops?;
        desktops.sort_by_key(|(position, _, _)| *position);
        let activity = match activity {
            Ok(id) => self.activity_name(&id).await,
            Err(_) => String::new(),
        };
        Ok(Extra {
            desktops: desktops
                .into_iter()
                .map(|(_, id, name)| (id, name))
                .collect(),
            current: current?,
            activity,
            active_output: output.unwrap_or_default(),
        })
    }
}

fn parse<T: for<'de> Deserialize<'de>>(v: serde_json::Value) -> Result<T, Failure> {
    serde_json::from_value(v)
        .map_err(|e| Failure::Failed(format!("KWin's answer was not what Kai expects: {e}")))
}

impl Windows for PlasmaWindows {
    fn snapshot(&self) -> Fut<'_, Snapshot> {
        Box::pin(async move {
            if let Some(wm) = self.live() {
                let extra = self.extra().await?;
                return Ok(wm.with(|table| super::wm::snapshot(table, &extra)));
            }
            let raw: RawSnapshot = parse(self.script.run(SNAPSHOT, json!({})).await?)?;
            let activity = self.activity_name(&raw.activity).await;
            Ok(Snapshot {
                windows: raw
                    .windows
                    .into_iter()
                    .map(|w| Window {
                        id: w.id,
                        app: w.app,
                        title: w.title,
                        pid: w.pid,
                        output: w.output,
                        desktops: w.desktops,
                        active: w.active,
                        minimized: w.minimized,
                        maximized: w.maximized,
                        fullscreen: w.fullscreen,
                    })
                    .collect(),
                outputs: raw
                    .outputs
                    .into_iter()
                    .map(|o| Output {
                        name: o.name,
                        x: o.x,
                        y: o.y,
                        width: o.width,
                        height: o.height,
                        scale: o.scale,
                        active: o.active,
                    })
                    .collect(),
                desktops: raw.desktops,
                desktop: raw.desktop,
                activity,
            })
        })
    }

    /// From the protocol's table where it is live (no call at all), else one script run (about
    /// 5 ms).
    fn active(&self) -> Fut<'_, Option<Window>> {
        Box::pin(async move {
            if let Some(wm) = self.live() {
                return Ok(wm.with(super::wm::active));
            }
            let raw: Option<RawActive> = parse(self.script.run(ACTIVE, json!({})).await?)?;
            Ok(raw.map(|w| Window {
                id: w.id,
                app: w.app,
                title: w.title,
                pid: w.pid,
                active: true,
                ..Window::default()
            }))
        })
    }

    fn act<'a>(&'a self, id: &'a str, action: &'a Action) -> Fut<'a, Done> {
        Box::pin(async move {
            let args = match action {
                Action::Activate => json!({"action": "activate"}),
                Action::Close => json!({"action": "close"}),
                Action::Minimize => json!({"action": "minimize"}),
                Action::ToggleMaximize => json!({"action": "maximize"}),
                Action::ToggleFullscreen => json!({"action": "fullscreen"}),
                Action::ToDesktop(at) => json!({"action": "to_desktop", "desktop": at}),
                Action::ToOutput(o) => json!({"action": "to_output", "output": o}),
                Action::Tile(side) => json!({"action": "tile", "side": side.key()}),
            };
            let mut args = args;
            args["window"] = json!(id);
            let done: RawDone = parse(self.script.run(ACT, args).await?)?;
            if done.gone {
                return Err(Failure::Refused("the window closed meanwhile".into()));
            }
            Ok(done.done)
        })
    }
}
