//! WP13 probe binary (Kai phase 3, "verify before building"). Every mode talks only to the
//! display and bus in its environment; the drivers (`run-*.sh`) start it inside a nested KWin.
//!
//! p3 globals
//! p3 windows [--activate UUID]
//! p3 fake click X Y | fake key CODE… | fake chord CODE… | fake text TEXT
//! p3 shot window UUID [OUT.ppm] [--decoration] | shot active [OUT] | shot workspace [OUT]
//! p3 testwin APP_ID TITLE R,G,B SECS
//! p3 overlay none|on-demand|exclusive overlay|top SECS [DELAY_MS]

mod gui;
mod proto;
mod shot;
mod wl;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let s: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    match s.as_slice() {
        ["globals"] => wl::globals(),
        ["windows"] => {
            wl::windows(None);
        }
        ["windows", "--activate", u] => {
            wl::windows(Some(u));
        }
        ["fake", "click", x, y] => wl::fake(wl::FakeOp::Click(x.parse().unwrap(), y.parse().unwrap())),
        ["fake", "key", codes @ ..] => wl::fake(wl::FakeOp::Key(codes.iter().map(|c| c.parse().unwrap()).collect())),
        ["fake", "chord", codes @ ..] => wl::fake(wl::FakeOp::Combo(codes.iter().map(|c| c.parse().unwrap()).collect())),
        ["fake", "text", t] => wl::fake(wl::FakeOp::Keysyms(t.to_string())),
        ["shot", "window", u, rest @ ..] => {
            let deco = rest.contains(&"--decoration");
            let out = rest.iter().find(|r| !r.starts_with("--")).copied();
            std::process::exit(shot::shot(shot::Target::Window(u.to_string()), out, deco));
        }
        ["shot", "active", rest @ ..] => std::process::exit(shot::shot(shot::Target::Active, rest.first().copied(), false)),
        ["shot", "workspace", rest @ ..] => {
            std::process::exit(shot::shot(shot::Target::Workspace, rest.first().copied(), false))
        }
        ["testwin", app, title, rgb, secs] => {
            let c: Vec<u8> = rgb.split(',').map(|v| v.parse().unwrap()).collect();
            gui::testwin(app, title, (c[0], c[1], c[2]), (640, 400), secs.parse().unwrap());
        }
        ["overlay", kb, layer, secs, rest @ ..] => {
            let delay = rest.first().map(|d| d.parse().unwrap()).unwrap_or(0);
            gui::overlay(kb, layer, secs.parse().unwrap(), delay);
        }
        _ => {
            eprintln!("usage: see the header of src/main.rs");
            std::process::exit(2);
        }
    }
}
