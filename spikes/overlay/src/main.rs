// Kai overlay spike: layer-shell OVERLAY surfaces (bottom-right) each hosting a WebKitGTK view
// that renders lmgw's ring.js unchanged. Usage: overlay --help
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};
use javascriptcore::ValueExt;
use webkit2gtk::{
    glib, gio, SecurityManagerExt, SettingsExt, URISchemeRequestExt, UserContentManager,
    UserContentManagerExt, WebContext, WebContextExt, WebView, WebViewExt, WebViewExtManual,
};

const INDEX: &str = include_str!("../assets/index.html");
const ADAPTER: &str = include_str!("../assets/adapter.js");
const ENGINE: &str = include_str!("../assets/viz/engine.js");
const RING: &str = include_str!("../assets/viz/ring.js");

struct Opts {
    mode: String,      // ring | click | widgets | gpu
    n: usize,          // surfaces
    ctx: String,       // shared | separate | related
    scenario: String,  // cycle | speaking | idle | listening | thinking
    bg: String,        // disc | lum
    size: i32,
    secs: u64,
    through: bool,     // empty input region (click mode: false => one rect)
    hz: u64,
}

fn parse() -> Opts {
    let mut o = Opts { mode: "ring".into(), n: 1, ctx: "shared".into(), scenario: "cycle".into(), bg: "disc".into(),
        size: 320, secs: 20, through: true, hz: 60 };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < a.len() {
        let v = |i: usize| a.get(i + 1).cloned().unwrap_or_default();
        match a[i].as_str() {
            "--mode" => { o.mode = v(i); i += 1 }
            "--n" => { o.n = v(i).parse().unwrap(); i += 1 }
            "--ctx" => { o.ctx = v(i); i += 1 }
            "--scenario" => { o.scenario = v(i); i += 1 }
            "--bg" => { o.bg = v(i); i += 1 }
            "--size" => { o.size = v(i).parse::<i32>().unwrap().min(320); i += 1 }
            "--secs" => { o.secs = v(i).parse::<u64>().unwrap().min(60); i += 1 }
            "--hz" => { o.hz = v(i).parse().unwrap(); i += 1 }
            "--interactive" => o.through = false,
            _ => { eprintln!("overlay --mode ring|click|widgets|gpu --n N --ctx shared|separate|related --scenario cycle|speaking|idle|listening|thinking --bg disc|lum --size PX(<=320) --secs S(<=60) --hz N [--interactive]"); std::process::exit(2) }
        }
        i += 1;
    }
    o
}

/// speech-like synthetic bands (0..1) and level
fn synth(t: f64) -> (f64, Vec<f64>) {
    let syl = (0.5 + 0.5 * (t * std::f64::consts::TAU * 3.7).sin()).powf(1.5);
    let env = syl * (0.6 + 0.4 * (t * 0.9).sin());
    let bands = (0..48).map(|b| {
        let shape = 0.85 * (-(b as f64) / 28.0).exp() + 0.15;
        (env * shape * (0.65 + 0.35 * ((b as f64) * 0.7 + t * 6.0).sin())).clamp(0.0, 1.0)
    }).collect();
    (env * 0.8, bands)
}

fn arr(v: &[f64]) -> String {
    let mut s = String::from("[");
    for (i, x) in v.iter().enumerate() { if i > 0 { s.push(',') } s.push_str(&format!("{:.3}", x)) }
    s.push(']'); s
}

fn serve(ctx: &WebContext) {
    let sm = ctx.security_manager().expect("security manager");
    sm.register_uri_scheme_as_secure("kai");
    sm.register_uri_scheme_as_cors_enabled("kai");
    ctx.register_uri_scheme("kai", |req| {
        let path = req.path().map(|p| p.to_string()).unwrap_or_default();
        let (body, ty) = match path.as_str() {
            "/index.html" => (INDEX, "text/html"),
            "/adapter.js" => (ADAPTER, "text/javascript"),
            "/viz/engine.js" => (ENGINE, "text/javascript"),
            "/viz/ring.js" => (RING, "text/javascript"),
            _ => { req.finish_error(&mut glib::Error::new(gio::IOErrorEnum::NotFound, "404")); return }
        };
        let s = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(body.as_bytes()));
        req.finish(&s, body.len() as i64, Some(ty));
    });
}

fn main() {
    // NVIDIA rule: before GTK or any thread starts; an explicit value wins.
    if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none()
        && (std::path::Path::new("/dev/nvidiactl").exists() || std::path::Path::new("/sys/module/nvidia").exists())
    {
        std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
    }
    let o = Rc::new(parse());
    gtk::init().expect("gtk init");
    println!("pid {} layer_shell_supported={} v{}.{}.{}", std::process::id(), gtk_layer_shell::is_supported(),
        gtk_layer_shell::major_version(), gtk_layer_shell::minor_version(), gtk_layer_shell::micro_version());

    let css = gtk::CssProvider::new();
    css.load_from_data(b"window, window.background { background: transparent; }").unwrap();
    gtk::StyleContext::add_provider_for_screen(&gdk::Screen::default().unwrap(), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

    let shared = WebContext::default().unwrap();
    serve(&shared);
    let views: Rc<RefCell<Vec<WebView>>> = Rc::new(RefCell::new(vec![]));
    let wins: Rc<RefCell<Vec<gtk::Window>>> = Rc::new(RefCell::new(vec![]));

    let start = Instant::now();
    let ready = Rc::new(Cell::new(0usize));
    let ready_at = Rc::new(Cell::new(0f64));
    for i in 0..o.n {
        let (ready_c, ready_at_c, n) = (ready.clone(), ready_at.clone(), o.n);
        let ucm = UserContentManager::new();
        ucm.register_script_message_handler("kai");
        let idx = i;
        let t0 = start;
        ucm.connect_script_message_received(Some("kai"), move |_, r| {
            if let Some(v) = r.js_value() {
                let s = v.to_str().to_string();
                println!("[view {idx} +{:.2}s] {}", t0.elapsed().as_secs_f32(), s);
                if s.contains("\"ev\":\"ready\"") {
                    ready_c.set(ready_c.get() + 1);
                    if ready_c.get() == n { ready_at_c.set(start.elapsed().as_secs_f64()); }
                }
            }
        });
        let view: WebView = match o.ctx.as_str() {
            "separate" => {
                let c = WebContext::new_ephemeral();
                serve(&c);
                WebView::new_with_context_and_user_content_manager(&c, &ucm)
            }
            "related" if i > 0 => WebView::builder()
                .related_view(&views.borrow()[0])
                .user_content_manager(&ucm)
                .build(),
            _ => WebView::new_with_context_and_user_content_manager(&shared, &ucm),
        };
        view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        if let Some(s) = WebViewExt::settings(&view) { s.set_enable_developer_extras(false); }

        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_title("kai-overlay");
        win.set_decorated(false);
        win.set_app_paintable(true);
        if let Some(vis) = gdk::Screen::default().and_then(|s| s.rgba_visual()) { win.set_visual(Some(&vis)); }
        win.init_layer_shell();
        win.set_layer(Layer::Overlay);
        win.set_namespace("kai-overlay");
        win.set_keyboard_mode(KeyboardMode::None);
        win.set_exclusive_zone(-1);
        win.set_anchor(Edge::Bottom, true);
        win.set_anchor(Edge::Right, true);
        let (col, row) = (i as i32 % 2, i as i32 / 2);
        win.set_layer_shell_margin(Edge::Right, 24 + col * (o.size + 8));
        win.set_layer_shell_margin(Edge::Bottom, 24 + row * (o.size + 8));
        win.set_default_size(o.size, o.size);
        win.set_size_request(o.size, o.size);
        win.add(&view);

        let through = o.through;
        let size = o.size;
        let mode = o.mode.clone();
        win.connect_map_event(move |w, _| {
            if let Some(gw) = w.window() {
                let region = if through && mode != "click" {
                    // GDK3 sends an EMPTY region as wl_surface.set_input_region(nil) = "whole surface takes input".
                    // A 1x1 px region in the corner is the closest to click-through.
                    cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, 1, 1))
                } else if mode == "click" && !through {
                    cairo::Region::create_rectangle(&cairo::RectangleInt::new(10, size - 50, 120, 40))
                } else { return glib::Propagation::Proceed };
                gw.input_shape_combine_region(&region, 0, 0);
            }
            glib::Propagation::Proceed
        });
        win.show_all();

        let url = if o.mode == "gpu" { "webkit://gpu".to_string() } else {
            format!("kai://app/index.html?mode={}&bg={}", o.mode, o.bg) };
        view.load_uri(&url);
        views.borrow_mut().push(view);
        wins.borrow_mut().push(win);
    }

    if o.mode == "gpu" {
        let vs = views.clone();
        glib::timeout_add_local_once(Duration::from_secs(4), move || {
            for v in vs.borrow().iter().take(1) {
                v.evaluate_javascript("document.body.innerText", None, None, gio::Cancellable::NONE, |r| {
                    match r { Ok(v) => println!("---- webkit://gpu ----\n{}\n----", v.to_str()), Err(e) => println!("gpu js error: {e}") }
                });
            }
        });
    } else {
        // state cycle + 60 Hz feed from the host (starts when every page reported ready)
        let ready_at = ready_at.clone();
        let cur = Rc::new(RefCell::new(String::new()));
        let states = ["idle", "listening", "thinking", "speaking"];
        let (vs, o2, cur2) = (views.clone(), o.clone(), cur.clone());
        let ready = ready.clone();
        let frames = Rc::new(Cell::new(0u64));
        glib::timeout_add_local(Duration::from_millis(1000 / o.hz.max(1)), move || {
            if ready.get() < o2.n { return glib::ControlFlow::Continue; }
            let t = start.elapsed().as_secs_f64() - ready_at.get();
            let st = match o2.scenario.as_str() {
                "cycle" => states[((t / 4.0) as usize) % 4],
                s => s,
            };
            if *cur2.borrow() != st {
                *cur2.borrow_mut() = st.to_string();
                println!("[host +{t:.1}s] state -> {st}");
                for v in vs.borrow().iter() {
                    v.evaluate_javascript(&format!("__kai.setState('{st}')"), None, None, gio::Cancellable::NONE, |_| {});
                }
            }
            let (l, b) = synth(t);
            let z = vec![0.0; 48];
            let js = match st {
                "speaking" => format!("__kai.feed({l:.3},0,{},{})", arr(&b), arr(&z)),
                "listening" => format!("__kai.feed(0,{l:.3},{},{})", arr(&z), arr(&b)),
                _ => String::new(),
            };
            if !js.is_empty() {
                for v in vs.borrow().iter() { v.evaluate_javascript(&js, None, None, gio::Cancellable::NONE, |_| {}); }
            } else if frames.get() % 60 == 0 {
                for v in vs.borrow().iter() { v.evaluate_javascript("__kai.feed(0,0,[],[])", None, None, gio::Cancellable::NONE, |_| {}); }
            }
            frames.set(frames.get() + 1);
            glib::ControlFlow::Continue
        });
        let vs = views.clone();
        glib::timeout_add_local_once(Duration::from_secs(o.secs.saturating_sub(1).max(1)), move || {
            for v in vs.borrow().iter().take(1) {
                v.evaluate_javascript("__kai.stats()", None, None, gio::Cancellable::NONE, |r| {
                    if let Ok(v) = r { println!("stats {}", v.to_str()) }
                });
            }
        });
    }
    glib::timeout_add_local_once(Duration::from_secs(o.secs), || gtk::main_quit());
    gtk::main();
    println!("exit");
}
