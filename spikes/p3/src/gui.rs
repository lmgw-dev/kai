//! GTK probes: a plain test window (a typing target and a capture subject) and a layer-shell
//! surface with a chosen keyboard mode (the approval overlay's question).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk_layer_shell::LayerShell;

thread_local! {
    static T0: Instant = Instant::now();
}

fn log(what: &str) {
    let ms = T0.with(|t| t.elapsed().as_millis());
    println!("t={ms:>6} {what}");
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn css(win: &gtk::Window, rgb: (u8, u8, u8)) {
    let p = gtk::CssProvider::new();
    let s = format!("window {{ background-color: rgb({},{},{}); }} entry {{ margin: 8px; }}", rgb.0, rgb.1, rgb.2);
    p.load_from_data(s.as_bytes()).unwrap();
    win.style_context().add_provider(&p, gtk::STYLE_PROVIDER_PRIORITY_USER);
}

fn key_logging(win: &gtk::Window, tag: &'static str, on_key: impl Fn(&str) + 'static) {
    win.add_events(gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::FOCUS_CHANGE_MASK | gdk::EventMask::BUTTON_PRESS_MASK);
    win.connect_key_press_event(move |_, ev| {
        let kv = ev.keyval();
        let name = kv.name().map(|s| s.to_string()).unwrap_or_default();
        log(&format!("{tag} key {name} unicode={:?} hw={} state={:?}", kv.to_unicode(), ev.hardware_keycode(), ev.state()));
        on_key(&name);
        glib::Propagation::Proceed
    });
    win.connect_focus_in_event(move |_, _| {
        log(&format!("{tag} FOCUS IN (keyboard enter)"));
        glib::Propagation::Proceed
    });
    win.connect_focus_out_event(move |_, _| {
        log(&format!("{tag} FOCUS OUT (keyboard leave)"));
        glib::Propagation::Proceed
    });
    win.connect_button_press_event(move |_, ev| {
        log(&format!("{tag} button {} at {:?}", ev.button(), ev.position()));
        glib::Propagation::Proceed
    });
}

pub fn testwin(app_id: &str, title: &str, rgb: (u8, u8, u8), size: (i32, i32), secs: u64) {
    glib::set_prgname(Some(app_id));
    gtk::init().expect("gtk");
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title(title);
    win.set_default_size(size.0, size.1);
    css(&win, rgb);
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let entry = gtk::Entry::new();
    bx.pack_start(&entry, false, false, 0);
    win.add(&bx);
    key_logging(&win, "testwin", |_| {});
    entry.connect_changed(|e| log(&format!("testwin TEXT {:?}", e.text().as_str())));
    win.connect_destroy(|_| gtk::main_quit());
    win.show_all();
    entry.grab_focus();
    log(&format!("testwin mapped app_id={app_id} title={title:?} pid={}", std::process::id()));
    glib::timeout_add_local_once(Duration::from_secs(secs), gtk::main_quit);
    gtk::main();
    log(&format!("testwin final TEXT {:?}", entry.text().as_str()));
}

pub fn overlay(kb: &str, layer: &str, secs: u64, delay_ms: u64) {
    gtk::init().expect("gtk");
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.init_layer_shell();
    win.set_namespace("kai-approval");
    win.set_layer(match layer {
        "top" => gtk_layer_shell::Layer::Top,
        _ => gtk_layer_shell::Layer::Overlay,
    });
    let mode = match kb {
        "none" => gtk_layer_shell::KeyboardMode::None,
        "exclusive" => gtk_layer_shell::KeyboardMode::Exclusive,
        _ => gtk_layer_shell::KeyboardMode::OnDemand,
    };
    win.set_keyboard_mode(mode);
    // Centre of the output: no anchors.
    win.set_default_size(400, 160);
    win.set_size_request(400, 160);
    css(&win, (200, 120, 20));
    let answered = Rc::new(RefCell::new(None::<&'static str>));
    let a2 = answered.clone();
    key_logging(&win, "overlay", move |name| {
        let verdict = match name {
            "Return" | "KP_Enter" => Some("APPROVE"),
            "Escape" => Some("DECLINE"),
            _ => None,
        };
        if let Some(v) = verdict {
            if a2.borrow().is_none() {
                *a2.borrow_mut() = Some(v);
                log(&format!("overlay ANSWER {v}"));
                glib::timeout_add_local_once(Duration::from_millis(200), gtk::main_quit);
            }
        }
    });
    let w2 = win.clone();
    let show = move || {
        w2.show_all();
        log(&format!("overlay mapped keyboard_mode={:?} layer={:?}", w2.keyboard_mode(), w2.layer()));
    };
    if delay_ms > 0 {
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), show);
    } else {
        show();
    }
    glib::timeout_add_local_once(Duration::from_secs(secs), gtk::main_quit);
    gtk::main();
    log(&format!("overlay closed answer={:?}", answered.borrow()));
}
