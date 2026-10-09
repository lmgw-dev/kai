//! Raw Wayland probes: the globals this binary is offered, the window management protocol's
//! windows, and `org_kde_kwin_fake_input`.

use std::collections::BTreeMap;
use std::time::Duration;

use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use crate::proto::fake::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;
use crate::proto::pwm::org_kde_plasma_window::{self, OrgKdePlasmaWindow};
use crate::proto::pwm::org_kde_plasma_window_management::{self, OrgKdePlasmaWindowManagement};

#[derive(Default, Debug, Clone)]
pub struct Win {
    pub uuid: String,
    pub title: String,
    pub app_id: String,
    pub resource_name: String,
    pub pid: u32,
    pub state: u32,
    pub geometry: Option<(i32, i32, u32, u32)>,
    pub client_geometry: Option<(i32, i32, u32, u32)>,
    pub initial: bool,
}

#[derive(Default)]
struct State {
    uuids: Vec<String>,
    wins: BTreeMap<String, Win>,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event,
             _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for State {
    fn event(st: &mut Self, _: &OrgKdePlasmaWindowManagement, ev: org_kde_plasma_window_management::Event,
             _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let org_kde_plasma_window_management::Event::WindowWithUuid { uuid, .. } = ev {
            st.uuids.push(uuid);
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, String> for State {
    fn event(st: &mut Self, _: &OrgKdePlasmaWindow, ev: org_kde_plasma_window::Event, uuid: &String,
             _: &Connection, _: &QueueHandle<Self>) {
        use org_kde_plasma_window::Event as E;
        let w = st.wins.entry(uuid.clone()).or_insert_with(|| Win { uuid: uuid.clone(), ..Default::default() });
        match ev {
            E::TitleChanged { title } => w.title = title,
            E::AppIdChanged { app_id } => w.app_id = app_id,
            E::ResourceNameChanged { resource_name } => w.resource_name = resource_name,
            E::PidChanged { pid } => w.pid = pid,
            E::StateChanged { flags } => w.state = flags,
            E::Geometry { x, y, width, height } => w.geometry = Some((x, y, width, height)),
            E::ClientGeometry { x, y, width, height } => w.client_geometry = Some((x, y, width, height)),
            E::InitialState => w.initial = true,
            _ => {}
        }
    }
}

impl Dispatch<OrgKdeKwinFakeInput, ()> for State {
    fn event(_: &mut Self, _: &OrgKdeKwinFakeInput, _: crate::proto::fake::org_kde_kwin_fake_input::Event,
             _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

fn connect() -> (Connection, GlobalList, EventQueue<State>) {
    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, queue) = registry_queue_init::<State>(&conn).expect("registry");
    (conn, globals, queue)
}

fn advertised(globals: &GlobalList, iface: &str) -> Option<u32> {
    globals.contents().with_list(|l| l.iter().find(|g| g.interface == iface).map(|g| g.version))
}

/// Every global the compositor offers this process, one per line, plus a verdict line for the
/// three restricted interfaces of ruling 16.
pub fn globals() {
    let (_c, globals, _q) = connect();
    let mut names: Vec<(String, u32)> =
        globals.contents().with_list(|l| l.iter().map(|g| (g.interface.clone(), g.version)).collect());
    names.sort();
    for (n, v) in &names {
        println!("global {n} v{v}");
    }
    for r in ["org_kde_plasma_window_management", "zkde_screencast_unstable_v1", "org_kde_kwin_fake_input"] {
        match names.iter().find(|(n, _)| n == r) {
            Some((_, v)) => println!("RESTRICTED {r}: offered v{v}"),
            None => println!("RESTRICTED {r}: NOT offered"),
        }
    }
}

/// The windows the window management protocol reports, by uuid. `activate` asks KWin to activate
/// the window with that uuid afterwards.
pub fn windows(activate: Option<&str>) -> Vec<Win> {
    let (conn, globals, mut q) = connect();
    let qh = q.handle();
    let Some(v) = advertised(&globals, "org_kde_plasma_window_management") else {
        println!("windows: org_kde_plasma_window_management NOT offered to this process");
        return vec![];
    };
    let mgmt: OrgKdePlasmaWindowManagement = globals.bind(&qh, 13..=v.min(18), ()).expect("bind wm");
    let mut st = State::default();
    q.roundtrip(&mut st).unwrap();
    let uuids = st.uuids.clone();
    let mut handles = vec![];
    for u in &uuids {
        handles.push(mgmt.get_window_by_uuid(u.clone(), &qh, u.clone()));
    }
    q.roundtrip(&mut st).unwrap();
    q.roundtrip(&mut st).unwrap();
    if let Some(a) = activate {
        if let Some(h) = handles.iter().find(|h| h.data::<String>().map(|s| s.as_str()) == Some(a)) {
            // state flag 1 = active
            h.set_state(1, 1);
            q.roundtrip(&mut st).unwrap();
            println!("windows: asked KWin to activate {a}");
        } else {
            println!("windows: no window {a}");
        }
    }
    let _ = conn.flush();
    let mut out: Vec<Win> = st.wins.values().cloned().collect();
    out.sort_by_key(|w| uuids.iter().position(|u| *u == w.uuid));
    for w in &out {
        println!(
            "window uuid={} app_id={:?} resource={:?} title={:?} pid={} active={} geometry={:?} client_geometry={:?}",
            w.uuid, w.app_id, w.resource_name, w.title, w.pid, w.state & 1 != 0, w.geometry, w.client_geometry
        );
    }
    out
}

pub enum FakeOp {
    Click(f64, f64),
    Key(Vec<u32>),
    Keysyms(String),
    Combo(Vec<u32>),
}

/// Input through `org_kde_kwin_fake_input`: an absolute click, evdev key taps, a text typed as
/// keysyms (v6's `keyboard_keysym`), or a chord (all down, then all up in reverse).
pub fn fake(op: FakeOp) {
    let (_conn, globals, mut q) = connect();
    let qh = q.handle();
    let Some(v) = advertised(&globals, "org_kde_kwin_fake_input") else {
        println!("fake: org_kde_kwin_fake_input NOT offered to this process");
        std::process::exit(3);
    };
    let fi: OrgKdeKwinFakeInput = globals.bind(&qh, 1..=v.min(6), ()).expect("bind fake input");
    println!("fake: bound org_kde_kwin_fake_input v{}", fi.version());
    fi.authenticate("kai-p3".into(), "WP13 spike".into());
    let mut st = State::default();
    let pause = || std::thread::sleep(Duration::from_millis(15));
    let flush = |q: &mut EventQueue<State>, st: &mut State| {
        q.roundtrip(st).unwrap();
    };
    match op {
        FakeOp::Click(x, y) => {
            fi.pointer_motion_absolute(x, y);
            flush(&mut q, &mut st);
            pause();
            fi.button(0x110, 1);
            flush(&mut q, &mut st);
            pause();
            fi.button(0x110, 0);
            flush(&mut q, &mut st);
            println!("fake: clicked at {x},{y}");
        }
        FakeOp::Key(codes) => {
            for c in codes {
                fi.keyboard_key(c, 1);
                flush(&mut q, &mut st);
                pause();
                fi.keyboard_key(c, 0);
                flush(&mut q, &mut st);
                pause();
            }
            println!("fake: keys sent");
        }
        FakeOp::Combo(codes) => {
            for &c in &codes {
                fi.keyboard_key(c, 1);
                flush(&mut q, &mut st);
                pause();
            }
            for &c in codes.iter().rev() {
                fi.keyboard_key(c, 0);
                flush(&mut q, &mut st);
                pause();
            }
            println!("fake: chord sent");
        }
        FakeOp::Keysyms(text) => {
            if fi.version() < 6 {
                println!("fake: keyboard_keysym needs v6, have v{}", fi.version());
                std::process::exit(4);
            }
            for ch in text.chars() {
                let ks = keysym_of(ch);
                fi.keyboard_keysym(ks, 1);
                flush(&mut q, &mut st);
                pause();
                fi.keyboard_keysym(ks, 0);
                flush(&mut q, &mut st);
                pause();
            }
            println!("fake: {} keysyms sent", text.chars().count());
        }
    }
}

/// X keysym of a character: Latin-1 maps to itself, `\n` to Return, anything else to the
/// Unicode keysym range (0x01000000 + code point), as xkbcommon defines it.
pub fn keysym_of(ch: char) -> u32 {
    match ch {
        '\n' => 0xff0d,
        '\t' => 0xff09,
        c if (c as u32) >= 0x20 && (c as u32) <= 0x7e => c as u32,
        c if (c as u32) >= 0xa0 && (c as u32) <= 0xff => c as u32,
        c => 0x0100_0000 + c as u32,
    }
}
