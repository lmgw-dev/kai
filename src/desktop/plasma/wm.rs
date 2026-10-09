//! The window management protocol (`org_kde_plasma_window_management`, ruling 16): where KWin
//! grants it to this binary (a `.desktop` naming its path, WP13 (d)), the window facts come from it,
//! live, on a Wayland connection of Kai's own in a thread of its own; the outputs' names and
//! logical geometry come from `wl_output` v4 and xdg-output on the same connection. KWin judges
//! the grant when the connection binds, so a Kai started before its `.desktop` existed keeps
//! KWin scripting until it is restarted.
//!
//! The table is what the events said last; [`snapshot`] turns it into a [`Snapshot`] with the
//! desktops' names and the current desktop, activity and output, which KWin's D-Bus says without a
//! grant. A window is listed once KWin sent its initial state, unless it asks to be left out of the
//! task bar (as a task manager shows them).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kai_desktop::caps::windows::{Output, Snapshot, Window};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::protocol::wm::org_kde_plasma_stacking_order::{self, OrgKdePlasmaStackingOrder};
use super::protocol::wm::org_kde_plasma_window::{self, OrgKdePlasmaWindow};
use super::protocol::wm::org_kde_plasma_window_management::{self, OrgKdePlasmaWindowManagement};
use super::protocol::xdg_output::zxdg_output_manager_v1::{self, ZxdgOutputManagerV1};
use super::protocol::xdg_output::zxdg_output_v1::{self, ZxdgOutputV1};

/// The protocol's state bits (its `state` enum).
pub mod state {
    pub const ACTIVE: u32 = 0x1;
    pub const MINIMIZED: u32 = 0x2;
    pub const MAXIMIZED: u32 = 0x4;
    pub const FULLSCREEN: u32 = 0x8;
    pub const ON_ALL_DESKTOPS: u32 = 0x40;
    pub const SKIP_TASKBAR: u32 = 0x1000;
}

/// One window as the events said it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub title: String,
    pub app_id: String,
    pub pid: u32,
    pub state: u32,
    /// Virtual desktop ids.
    pub desktops: Vec<String>,
    pub geometry: Option<(i32, i32, u32, u32)>,
    /// KWin sent its initial state.
    pub initial: bool,
}

/// One output as `wl_output` and xdg-output said it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutFacts {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// The current mode in pixels (the scale is pixels over logical width).
    pub mode_width: i32,
    pub mode_height: i32,
    /// `wl_output`'s integer scale, for a compositor without xdg-output.
    pub wl_scale: i32,
}

#[derive(Debug, Default)]
pub struct Table {
    /// By uuid without braces.
    pub windows: HashMap<String, Facts>,
    /// Uuids without braces, bottom to top, as KWin last sent the stacking order.
    pub stacking: Vec<String>,
    /// By the `wl_output` global's name.
    pub outputs: HashMap<u32, OutFacts>,
    /// The connection ended: why.
    pub ended: Option<String>,
}

/// Why the protocol is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// KWin does not offer this binary the global (no grant).
    NotGranted,
    /// No display, or the connection failed.
    Failed(String),
}

/// The live connection (module doc).
#[derive(Clone)]
pub struct Wm {
    table: Arc<Mutex<Table>>,
}

impl Wm {
    /// Connects, binds and reads the first state, waiting at most `timeout` for it (named when it
    /// passes); the connection then lives in its own thread until it ends.
    pub fn start(timeout: Duration) -> Result<Wm, Unavailable> {
        let table = Arc::new(Mutex::new(Table::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = table.clone();
        std::thread::Builder::new()
            .name("kai-wm".into())
            .spawn(move || run(shared, tx))
            .map_err(|e| Unavailable::Failed(format!("cannot start its thread: {e}")))?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(Wm { table }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Unavailable::Failed(format!(
                "the compositor did not answer within {} s (dbus.timeout_s)",
                timeout.as_secs()
            ))),
        }
    }

    /// Why the connection ended, once it did.
    pub fn ended(&self) -> Option<String> {
        self.table.lock().expect("never poisoned").ended.clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&Table) -> R) -> R {
        f(&self.table.lock().expect("never poisoned"))
    }
}

struct Client {
    table: Arc<Mutex<Table>>,
    wm: Option<OrgKdePlasmaWindowManagement>,
    xdg: Option<ZxdgOutputManagerV1>,
    /// The stacking order being received.
    stacking: Vec<String>,
    /// Output proxies by global name, for their release when the global goes.
    outputs: HashMap<u32, (WlOutput, Option<ZxdgOutputV1>)>,
}

fn bare(uuid: &str) -> String {
    uuid.trim_start_matches('{')
        .trim_end_matches('}')
        .to_owned()
}

impl Client {
    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().expect("never poisoned")
    }

    fn bind_output(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        let output: WlOutput = registry.bind(name, version.min(4), qh, name);
        let xdg = self
            .xdg
            .as_ref()
            .map(|m| m.get_xdg_output(&output, qh, name));
        self.table().outputs.insert(name, OutFacts::default());
        self.outputs.insert(name, (output, xdg));
    }
}

fn run(table: Arc<Mutex<Table>>, ready: std::sync::mpsc::Sender<Result<(), Unavailable>>) {
    let fail = |e: String| Unavailable::Failed(e);
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(fail(format!("no Wayland display: {e}"))));
            return;
        }
    };
    let (globals, mut queue) = match registry_queue_init::<Client>(&conn) {
        Ok(g) => g,
        Err(e) => {
            let _ = ready.send(Err(fail(format!("the compositor's globals: {e}"))));
            return;
        }
    };
    let qh = queue.handle();
    let mut client = Client {
        table: table.clone(),
        wm: None,
        xdg: None,
        stacking: Vec::new(),
        outputs: HashMap::new(),
    };
    // At least 17 (the protocol says so); the bindings know up to 21.
    match globals.bind::<OrgKdePlasmaWindowManagement, _, _>(&qh, 17..=21, ()) {
        Ok(wm) => client.wm = Some(wm),
        Err(_) => {
            let _ = ready.send(Err(Unavailable::NotGranted));
            return;
        }
    }
    client.xdg = globals
        .bind::<ZxdgOutputManagerV1, _, _>(&qh, 1..=3, ())
        .ok();
    let outputs: Vec<(u32, u32)> = globals.contents().with_list(|l| {
        l.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| (g.name, g.version))
            .collect()
    });
    for (name, version) in outputs {
        client.bind_output(globals.registry(), name, version, &qh);
    }
    // The windows, then each window's initial state.
    for _ in 0..3 {
        if let Err(e) = queue.roundtrip(&mut client) {
            let _ = ready.send(Err(fail(format!("the compositor's first state: {e}"))));
            return;
        }
    }
    let _ = ready.send(Ok(()));
    let ended = loop {
        if let Err(e) = queue.blocking_dispatch(&mut client) {
            break e.to_string();
        }
    };
    tracing::warn!(
        "desktop: the window management connection ended: {ended}; KWin scripting serves"
    );
    client.table().ended = Some(ended);
}

impl Dispatch<WlRegistry, GlobalListContents> for Client {
    fn event(
        client: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                client.bind_output(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some((output, xdg)) = client.outputs.remove(&name) {
                    if let Some(x) = xdg {
                        x.destroy();
                    }
                    if output.version() >= 3 {
                        output.release();
                    }
                }
                client.table().outputs.remove(&name);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for Client {
    fn event(
        client: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut table = client.table();
        let Some(o) = table.outputs.get_mut(name) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                o.mode_width = width;
                o.mode_height = height;
            }
            wl_output::Event::Scale { factor } => o.wl_scale = factor,
            wl_output::Event::Geometry { x, y, .. } if o.width == 0 => {
                // xdg-output's logical position wins once it came.
                o.x = x;
                o.y = y;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, u32> for Client {
    fn event(
        client: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut table = client.table();
        let Some(o) = table.outputs.get_mut(name) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                o.x = x;
                o.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                o.width = width;
                o.height = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for Client {
    fn event(
        client: &mut Self,
        wm: &OrgKdePlasmaWindowManagement,
        event: org_kde_plasma_window_management::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            org_kde_plasma_window_management::Event::WindowWithUuid { uuid, .. } => {
                let id = bare(&uuid);
                client.table().windows.entry(id.clone()).or_default();
                wm.get_window_by_uuid(uuid, qh, id);
            }
            org_kde_plasma_window_management::Event::StackingOrderChanged2 => {
                client.stacking.clear();
                wm.get_stacking_order(qh, ());
            }
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qh: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        // The deprecated `window` event creates a window object nobody uses.
        let _ = opcode;
        qh.make_data::<OrgKdePlasmaWindow, _>(String::new())
    }
}

impl Dispatch<OrgKdePlasmaStackingOrder, ()> for Client {
    fn event(
        client: &mut Self,
        _: &OrgKdePlasmaStackingOrder,
        event: org_kde_plasma_stacking_order::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            org_kde_plasma_stacking_order::Event::Window { uuid } => {
                client.stacking.push(bare(&uuid))
            }
            org_kde_plasma_stacking_order::Event::Done => {
                let order = std::mem::take(&mut client.stacking);
                client.table().stacking = order;
            }
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, String> for Client {
    fn event(
        client: &mut Self,
        window: &OrgKdePlasmaWindow,
        event: org_kde_plasma_window::Event,
        id: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use org_kde_plasma_window::Event as E;
        if id.is_empty() {
            // The deprecated `window` event's object: nobody uses it.
            window.destroy();
            return;
        }
        let mut table = client.table();
        if let E::Unmapped = event {
            table.windows.remove(id);
            table.stacking.retain(|s| s != id);
            drop(table);
            window.destroy();
            return;
        }
        let Some(w) = table.windows.get_mut(id) else {
            return;
        };
        match event {
            E::TitleChanged { title } => w.title = title,
            E::AppIdChanged { app_id } => w.app_id = app_id,
            E::PidChanged { pid } => w.pid = pid,
            E::StateChanged { flags } => w.state = flags,
            E::Geometry {
                x,
                y,
                width,
                height,
            } => w.geometry = Some((x, y, width, height)),
            E::VirtualDesktopEntered { id } => {
                if !w.desktops.contains(&id) {
                    w.desktops.push(id);
                }
            }
            E::VirtualDesktopLeft { is } => w.desktops.retain(|d| *d != is),
            E::InitialState => w.initial = true,
            _ => {}
        }
    }
}

/// What KWin's D-Bus adds to the table: the desktops (id, name) in order, the current desktop's
/// id, the current activity's name and KWin's active output.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    pub desktops: Vec<(String, String)>,
    pub current: String,
    pub activity: String,
    pub active_output: String,
}

/// An output's logical size and scale: xdg-output's size, the scale as pixels over it; without
/// xdg-output, the mode over `wl_output`'s integer scale.
fn logical(o: &OutFacts) -> (u32, u32, f64) {
    if o.width > 0 && o.height > 0 {
        let scale = if o.mode_width > 0 {
            (f64::from(o.mode_width) / f64::from(o.width) * 100.0).round() / 100.0
        } else {
            1.0
        };
        return (o.width as u32, o.height as u32, scale);
    }
    let s = o.wl_scale.max(1);
    (
        (o.mode_width.max(0) / s) as u32,
        (o.mode_height.max(0) / s) as u32,
        f64::from(s),
    )
}

/// An output's logical rectangle, as [`logical`] has it.
fn rect(o: &OutFacts) -> (i64, i64, i64, i64) {
    let (w, h, _) = logical(o);
    (o.x as i64, o.y as i64, w as i64, h as i64)
}

/// The output a window is on: the one holding its centre, else the one it overlaps most.
fn output_of(geometry: Option<(i32, i32, u32, u32)>, outputs: &[&OutFacts]) -> String {
    let Some((x, y, w, h)) = geometry else {
        return String::new();
    };
    let (w, h) = (w as i64, h as i64);
    let (cx, cy) = (x as i64 + w / 2, y as i64 + h / 2);
    let inside = |o: &&&OutFacts| {
        let (ox, oy, ow, oh) = rect(o);
        cx >= ox && cx < ox + ow && cy >= oy && cy < oy + oh
    };
    if let Some(o) = outputs.iter().find(inside) {
        return o.name.clone();
    }
    let overlap = |o: &&&OutFacts| {
        let (ox, oy, ow, oh) = rect(o);
        let ix = (x as i64 + w).min(ox + ow) - (x as i64).max(ox);
        let iy = (y as i64 + h).min(oy + oh) - (y as i64).max(oy);
        ix.max(0) * iy.max(0)
    };
    outputs
        .iter()
        .filter(|o| overlap(o) > 0)
        .max_by_key(overlap)
        .map(|o| o.name.clone())
        .unwrap_or_default()
}

/// The table and `extra` as one [`Snapshot`] (module doc).
pub fn snapshot(table: &Table, extra: &Extra) -> Snapshot {
    let mut outs: Vec<&OutFacts> = table
        .outputs
        .values()
        .filter(|o| !o.name.is_empty())
        .collect();
    outs.sort_by_key(|o| (o.x, o.y));
    let name_of = |id: &str| {
        extra
            .desktops
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    // Topmost first: the stacking order reversed, then any window it did not name yet.
    let mut order: Vec<&String> = table.stacking.iter().rev().collect();
    let mut rest: Vec<&String> = table
        .windows
        .keys()
        .filter(|k| !table.stacking.contains(k))
        .collect();
    rest.sort();
    order.extend(rest);
    let windows = order
        .into_iter()
        .filter_map(|id| table.windows.get(id).map(|f| (id, f)))
        .filter(|(_, f)| f.initial && f.state & state::SKIP_TASKBAR == 0)
        .map(|(id, f)| Window {
            id: id.clone(),
            app: f.app_id.clone(),
            title: f.title.clone(),
            pid: f.pid,
            output: output_of(f.geometry, &outs),
            desktops: if f.state & state::ON_ALL_DESKTOPS != 0 {
                Vec::new()
            } else {
                f.desktops.iter().map(|d| name_of(d)).collect()
            },
            active: f.state & state::ACTIVE != 0,
            minimized: f.state & state::MINIMIZED != 0,
            maximized: f.state & state::MAXIMIZED != 0,
            fullscreen: f.state & state::FULLSCREEN != 0,
        })
        .collect();
    let outputs = outs
        .iter()
        .map(|o| {
            let (width, height, scale) = logical(o);
            Output {
                name: o.name.clone(),
                x: o.x,
                y: o.y,
                width,
                height,
                scale,
                active: o.name == extra.active_output,
            }
        })
        .collect();
    Snapshot {
        windows,
        outputs,
        desktops: extra.desktops.iter().map(|(_, n)| n.clone()).collect(),
        desktop: name_of(&extra.current),
        activity: extra.activity.clone(),
    }
}

/// The window with the keyboard focus in the table, any window (a task bar's or not): its id,
/// app, title and pid (`Windows::active`).
pub fn active(table: &Table) -> Option<Window> {
    table
        .windows
        .iter()
        .find(|(_, f)| f.initial && f.state & state::ACTIVE != 0)
        .map(|(id, f)| Window {
            id: id.clone(),
            app: f.app_id.clone(),
            title: f.title.clone(),
            pid: f.pid,
            active: true,
            ..Window::default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(name: &str, x: i32, w: i32, mode: i32) -> OutFacts {
        OutFacts {
            name: name.into(),
            x,
            y: 0,
            width: w,
            height: 1152,
            mode_width: mode,
            mode_height: 0,
            wl_scale: 1,
        }
    }

    /// The focused window is any window the table holds, a panel's too; none is `None`.
    #[test]
    fn the_active_window_is_read_from_the_table() {
        let mut t = Table::default();
        let facts = |title: &str, state: u32| Facts {
            title: title.into(),
            app_id: "org.kde.plasmashell".into(),
            pid: 9,
            state,
            initial: true,
            ..Default::default()
        };
        t.windows.insert("a".into(), facts("one", 0));
        assert_eq!(active(&t), None);
        t.windows.insert(
            "k".into(),
            facts("KRunner", state::ACTIVE | state::SKIP_TASKBAR),
        );
        let w = active(&t).unwrap();
        assert_eq!(
            (w.id.as_str(), w.app.as_str(), w.title.as_str(), w.pid),
            ("k", "org.kde.plasmashell", "KRunner", 9)
        );
        // A window KWin has not finished announcing is not taken.
        t.windows.get_mut("k").unwrap().initial = false;
        assert_eq!(active(&t), None);
    }

    /// The protocol's table as a snapshot: topmost first, the task bar's windows only, the states
    /// from the bits, desktops by name, each window's output by its centre, the scale from the
    /// mode.
    #[test]
    fn the_table_reads_as_a_snapshot() {
        let mut t = Table::default();
        t.outputs.insert(7, out("DP-1", 0, 2048, 2560));
        t.outputs.insert(9, out("HDMI-A-1", 2048, 1920, 1920));
        let facts = |title: &str, state: u32, geo: (i32, i32, u32, u32)| Facts {
            title: title.into(),
            app_id: "org.kde.konsole".into(),
            pid: 4,
            state,
            desktops: vec!["d1".into()],
            geometry: Some(geo),
            initial: true,
        };
        t.windows.insert(
            "a".into(),
            facts("one", state::ACTIVE | state::MAXIMIZED, (0, 0, 2048, 1152)),
        );
        // Mostly on the second output, its centre there.
        t.windows.insert(
            "b".into(),
            facts("two", state::ON_ALL_DESKTOPS, (1900, 10, 800, 600)),
        );
        t.windows.insert(
            "c".into(),
            facts("panel", state::SKIP_TASKBAR, (0, 0, 10, 10)),
        );
        let mut d = facts("new", state::MINIMIZED, (5000, 0, 10, 10));
        d.initial = false;
        t.windows.insert("d".into(), d.clone());
        t.stacking = vec!["b".into(), "c".into(), "a".into()];
        let extra = Extra {
            desktops: vec![("d1".into(), "Main".into()), ("d2".into(), "Mail".into())],
            current: "d2".into(),
            activity: "Work".into(),
            active_output: "HDMI-A-1".into(),
        };
        let s = snapshot(&t, &extra);
        let ids: Vec<&str> = s.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "b"],
            "topmost first; the panel and a window not yet shown left out"
        );
        let a = &s.windows[0];
        assert!(a.active && a.maximized && !a.minimized && !a.fullscreen);
        assert_eq!(
            (a.output.as_str(), a.desktops.clone()),
            ("DP-1", vec!["Main".to_owned()])
        );
        let b = &s.windows[1];
        assert_eq!(b.output, "HDMI-A-1");
        assert!(b.desktops.is_empty(), "on all desktops");
        assert_eq!(s.outputs.len(), 2);
        assert_eq!(
            (
                s.outputs[0].name.as_str(),
                s.outputs[0].scale,
                s.outputs[0].active
            ),
            ("DP-1", 1.25, false)
        );
        assert!(s.outputs[1].active);
        assert_eq!((s.desktop.as_str(), s.activity.as_str()), ("Mail", "Work"));
        assert_eq!(s.desktops, ["Main", "Mail"]);
        // A window off every output names none; one KWin has not stacked yet comes last.
        d.initial = true;
        d.state = 0;
        t.windows.insert("d".into(), d);
        let s = snapshot(&t, &extra);
        assert_eq!(
            s.windows.last().map(|w| (w.id.as_str(), w.output.as_str())),
            Some(("d", ""))
        );
        // Without xdg-output: the mode over wl_output's scale.
        let bare = OutFacts {
            name: "eDP-1".into(),
            mode_width: 2880,
            mode_height: 1800,
            wl_scale: 2,
            ..Default::default()
        };
        assert_eq!(logical(&bare), (1440, 900, 2.0));
    }
}
