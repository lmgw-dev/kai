//! The Windows capability on Plasma.
//!
//! - **The facts:** from the window management protocol where KWin grants it ([`super::wm`], live,
//!   with the desktops' names, the current desktop and KWin's active output from KWin's D-Bus);
//!   else, and when that connection ended, from KWin scripting (WP13 (f): no grant, about 5 ms a
//!   round trip, one consistent snapshot). Both list the windows a task bar shows, by the same
//!   ids, which ScreenShot2 takes too; `placed` (where each window is, for `see_screen`'s
//!   blanking) lists every window, those a task bar leaves out included, with its frame widened
//!   outward to whole logical pixels and by one more on each side ([`widened`]): the script reads
//!   KWin's fractional frame, the protocol says it rounded to whole pixels (Qt's `toRect`, off by
//!   at most 0.75), so both cover the window, one way. A window KWin names without a pid, a
//!   caption or a frame (an unmanaged X11 window) is kept with what it has (no frame: a Firefox
//!   window then fails `see_screen` closed), never failing the whole list.
//! - **The effects** running (`effects`, for `see_screen` under the site rules): KWin's
//!   `activeEffects` (`org.kde.kwin.Effects` at `/Effects`).
//! - **The actions:** KWin scripting, granted or not: one path for every action, tiling included
//!   (the protocol has no tiling). Reading a geometry back in the script that set it is stale
//!   (Wayland configures asynchronously), so an action answers what it asked KWin to do, and the
//!   next snapshot shows the result.
//!
//! The current activity's name comes from `org.kde.ActivityManager` when it runs.

use std::time::Duration;

use kai_desktop::caps::windows::{
    Action, Done, Output, Placed, Placement, Snapshot, Window, Windows,
};
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

/// Every window KWin draws with its frame, topmost first, those a task bar leaves out included
/// (`Windows::placed`).
const PLACED: &str = r#"
function run() {
    const order = workspace.stackingOrder.slice().reverse();
    const windows = order.filter(w => w && !w.deleted).map(w => {
        const g = w.frameGeometry || {};
        return { id: w.internalId ? bare(w.internalId) : null,
            app: w.desktopFileName || w.resourceClass || "", title: w.caption, pid: w.pid,
            desktops: w.onAllDesktops ? [] : (w.desktops || []).map(d => d.name),
            minimized: w.minimized, x: g.x, y: g.y, width: g.width, height: g.height };
    });
    return { windows: windows,
        desktop: workspace.currentDesktop ? workspace.currentDesktop.name : "" };
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
struct RawPlacement {
    /// Each read on its own ([`RawPlaced::of`]), so one odd window does not fail the list.
    windows: Vec<serde_json::Value>,
    #[serde(default)]
    desktop: Option<String>,
}

/// One window of `PLACED`, every field as KWin may leave it out or null.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPlaced {
    id: Option<String>,
    app: Option<String>,
    title: Option<String>,
    pid: Option<i64>,
    desktops: Option<Vec<Option<String>>>,
    minimized: Option<bool>,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
}

impl RawPlaced {
    /// One window as `placed` answers it (module doc), or `None` for an entry that is no window
    /// at all.
    fn of(v: serde_json::Value) -> Option<Placed> {
        let w: RawPlaced = serde_json::from_value(v).ok()?;
        let frame = match (w.x, w.y, w.width, w.height) {
            (Some(x), Some(y), Some(width), Some(height))
                if [x, y, width, height].iter().all(|v| v.is_finite()) =>
            {
                Some(widened(x, y, width, height))
            }
            _ => None,
        };
        Some(Placed {
            window: Window {
                id: w.id.unwrap_or_default(),
                app: w.app.unwrap_or_default(),
                title: w.title.unwrap_or_default(),
                pid: w.pid.and_then(|p| u32::try_from(p).ok()).unwrap_or(0),
                desktops: w
                    .desktops
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect(),
                minimized: w.minimized.unwrap_or(false),
                ..Window::default()
            },
            frame,
        })
    }
}

/// A frame in logical pixels, from KWin's (fractional, or rounded by the protocol) one: widened
/// outward to whole pixels, then by one more pixel on each side (module doc), so it covers the
/// window's frame however KWin rounded it.
pub fn widened(x: f64, y: f64, width: f64, height: f64) -> (i32, i32, u32, u32) {
    let (x0, y0) = (x.floor() - 1.0, y.floor() - 1.0);
    let (x1, y1) = (
        (x + width.max(0.0)).ceil() + 1.0,
        (y + height.max(0.0)).ceil() + 1.0,
    );
    (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
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

    /// From the protocol's table where it is live, else one script run; either way each frame
    /// [`widened`].
    fn placed(&self) -> Fut<'_, Placement> {
        Box::pin(async move {
            if let Some(wm) = self.live() {
                let extra = self.extra().await?;
                return Ok(wm.with(|table| super::wm::placed(table, &extra)));
            }
            let raw: RawPlacement = parse(self.script.run(PLACED, json!({})).await?)?;
            Ok(Placement {
                windows: raw.windows.into_iter().filter_map(RawPlaced::of).collect(),
                desktop: raw.desktop.unwrap_or_default(),
            })
        })
    }

    /// KWin's `activeEffects` (module doc).
    fn effects(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            property::<Vec<String>>(
                &bus,
                "org.kde.KWin",
                "/Effects",
                "org.kde.kwin.Effects",
                "activeEffects",
                self.timeout,
            )
            .await
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Qt 6's `QRectF::toRect`, which KWin's window management protocol sends a frame through.
    fn to_rect(x: f64, y: f64, w: f64, h: f64) -> (i32, i32, u32, u32) {
        let round = |v: f64| v.round() as i32;
        let (nx, ny) = (round(x), round(y));
        let nw = round(w + (x - f64::from(nx)) / 2.0);
        let nh = round(h + (y - f64::from(ny)) / 2.0);
        (nx, ny, nw as u32, nh as u32)
    }

    /// `frame` holds `(x, y, w, h)` and is at most `slack` logical pixels wider on each side.
    fn covers(frame: (i32, i32, u32, u32), x: f64, y: f64, w: f64, h: f64, slack: f64) -> bool {
        let (fx0, fy0) = (f64::from(frame.0), f64::from(frame.1));
        let (fx1, fy1) = (fx0 + f64::from(frame.2), fy0 + f64::from(frame.3));
        fx0 <= x
            && fy0 <= y
            && fx1 >= x + w
            && fy1 >= y + h
            && x - fx0 <= slack
            && y - fy0 <= slack
            && fx1 - (x + w) <= slack
            && fy1 - (y + h) <= slack
    }

    /// At 125 % a window's frame in logical pixels has fractions (whole physical pixels / 1.25):
    /// the script's frame and the protocol's rounded one are widened the same way and both cover
    /// it, and on a capture at 1.25 its every physical pixel.
    #[test]
    fn both_paths_widen_a_fractional_frame_at_125_percent_to_cover_it() {
        for (px, py, pw, ph) in [(101, 51, 1001, 751), (3, 7, 9, 13), (0, 0, 2560, 1440)] {
            let s = 1.25;
            let (x, y, w, h) = (
                f64::from(px) / s,
                f64::from(py) / s,
                f64::from(pw) / s,
                f64::from(ph) / s,
            );
            let script = widened(x, y, w, h);
            let (rx, ry, rw, rh) = to_rect(x, y, w, h);
            let protocol = widened(f64::from(rx), f64::from(ry), f64::from(rw), f64::from(rh));
            for frame in [script, protocol] {
                assert!(
                    covers(frame, x, y, w, h, 2.0),
                    "{frame:?} for {x} {y} {w} {h}"
                );
                // On the capture: logical × 1.25 holds every physical pixel of the window.
                let c = |v: i32| f64::from(v) * s;
                assert!(c(frame.0) <= f64::from(px) && c(frame.1) <= f64::from(py));
                assert!(c(frame.0 + frame.2 as i32) >= f64::from(px + pw));
                assert!(c(frame.1 + frame.3 as i32) >= f64::from(py + ph));
            }
        }
        // Whole frames: one pixel more on each side either way.
        assert_eq!(widened(10.0, 20.0, 300.0, 200.0), (9, 19, 302, 202));
        assert_eq!(widened(-5.5, 0.25, 10.0, 0.5), (-7, -1, 13, 3));
    }

    /// A manual read-only check against the running session's KWin (never in the gate): loads
    /// `PLACED` once through KWin scripting (the script only reads `workspace.stackingOrder`,
    /// then KWin unloads it), reads `activeEffects`, and prints each window's frame and the
    /// effects, to compare with `kscreen-doctor -o`. No input, no capture, no window changed.
    /// Run: `cargo test --workspace placed_on_this_session -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "reads the session's own KWin; run by hand"]
    async fn placed_on_this_session() {
        let t = Duration::from_secs(5);
        let dir = crate::desktop::tests::temp_dir();
        let bus = SessionBus::lazy(t);
        let script = KwinScript::new(bus.clone(), dir.path().join("kwin"), t);
        let windows = PlasmaWindows::new(None, script, bus, t);
        let p = windows.placed().await.unwrap();
        println!("current desktop: {:?}", p.desktop);
        for w in &p.windows {
            println!(
                "{:<38} pid {:>7} drawn {:<5} frame {:?} app {:?}",
                w.window.id,
                w.window.pid,
                p.drawn(&w.window),
                w.frame,
                w.window.app
            );
        }
        println!("active effects: {:?}", windows.effects().await.unwrap());
    }

    /// One window KWin names without a pid, a caption or a frame does not fail the list.
    #[test]
    fn odd_windows_are_kept_with_what_they_have() {
        let raw: RawPlacement = serde_json::from_value(json!({
            "windows": [
                {"id": "a", "app": "org.mozilla.firefox", "title": "Inbox — Mozilla Firefox",
                 "pid": 42, "desktops": ["Main"], "minimized": false,
                 "x": 10.4, "y": 20, "width": 300, "height": 200},
                {"id": "b", "app": "", "title": null, "pid": null, "desktops": [null],
                 "x": 0, "y": 0, "width": 10, "height": 10},
                {"id": "c", "app": "firefox", "title": "x", "pid": -1},
                null,
                "not a window"
            ],
            "desktop": null
        }))
        .unwrap();
        let p: Vec<Placed> = raw.windows.into_iter().filter_map(RawPlaced::of).collect();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].window.pid, 42);
        assert_eq!(p[0].window.desktops, ["Main"]);
        assert_eq!(p[0].frame, Some((9, 19, 303, 202)));
        assert_eq!((p[1].window.pid, p[1].window.title.as_str()), (0, ""));
        assert!(p[1].window.desktops.is_empty());
        assert_eq!(p[1].frame, Some((-1, -1, 12, 12)));
        assert_eq!(
            p[2].frame, None,
            "no frame: see_screen fails closed on a Firefox one"
        );
        assert_eq!(p[2].window.pid, 0);
    }
}
