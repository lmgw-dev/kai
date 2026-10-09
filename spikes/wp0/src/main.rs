// Kai WP0 probe host: one binary, one --mode per question (see ../overlay/RESULTS.md, WP0 section).
//   ring|click|swatch|feed  a layer-shell overlay (bottom-right, <= 320 px, <= 60 s) hosting the ring page
//   resize                  (d) surface and input region across set_size_request / resize, trace with WAYLAND_DEBUG
//   esm                     (e) file:// vs kai:// ES modules and CSP, offscreen
//   life                    (i)(o) WebView creation to first frame, PSS with no view and after destroying it
// Usage: wp0 --help
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use javascriptcore::ValueExt;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use webkit2gtk::{
    gio, glib, SecurityManagerExt, SettingsExt, URISchemeRequestExt, URISchemeResponse, URISchemeResponseExt,
    UserContentManager, UserContentManagerExt, WebContext, WebContextExt, WebView, WebViewExt, WebViewExtManual,
};

const INDEX: &str = include_str!("../assets/index.html");
const ADAPTER: &str = include_str!("../assets/adapter.js");
const ENGINE: &str = include_str!("../assets/viz/engine.js");
const RING: &str = include_str!("../assets/viz/ring.js");
const ESM: &str = include_str!("../assets/esm.html");
const ESM_BOOT: &str = include_str!("../assets/esm-boot.js");
const CSP_STRICT: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'";

struct Opts(HashMap<String, String>);
impl Opts {
    fn s(&self, k: &str, d: &str) -> String { self.0.get(k).cloned().unwrap_or_else(|| d.to_string()) }
    fn n(&self, k: &str, d: i64) -> i64 { self.0.get(k).and_then(|v| v.parse().ok()).unwrap_or(d) }
    fn has(&self, k: &str) -> bool { self.0.contains_key(k) }
}
fn parse() -> Opts {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut m = HashMap::new();
    let mut i = 0;
    while i < a.len() {
        let k = a[i].trim_start_matches("--").to_string();
        if k == "help" || !a[i].starts_with("--") {
            eprintln!("wp0 --mode ring|click|swatch|feed|resize|esm|life [--size PX<=320] [--secs S<=60] [--layer overlay|top|bottom] [--scenario speaking|idle|cycle]\n\
                 [--interactive] [--reapply] [--transport eval|b64|async|fetch|none] [--hz N] [--fullscreen R,G,B [--fs-delay MS]] [--assets DIR] [--cycles N]");
            std::process::exit(2);
        }
        if i + 1 < a.len() && !a[i + 1].starts_with("--") { m.insert(k, a[i + 1].clone()); i += 2 } else { m.insert(k, "1".into()); i += 1 }
    }
    Opts(m)
}

/// speech-like synthetic bands (0..1) and level, the spike's generator
fn synth(t: f64) -> (f64, Vec<f64>) {
    let syl = (0.5 + 0.5 * (t * std::f64::consts::TAU * 3.7).sin()).powf(1.5);
    let env = syl * (0.6 + 0.4 * (t * 0.9).sin());
    let bands = (0..48).map(|b| {
        let shape = 0.85 * (-(b as f64) / 28.0).exp() + 0.15;
        (env * shape * (0.65 + 0.35 * ((b as f64) * 0.7 + t * 6.0).sin())).clamp(0.0, 1.0)
    }).collect();
    (env * 0.8, bands)
}
fn arr(v: &[f64]) -> String { format!("[{}]", v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(",")) }
/// 98 f32 LE: level out, level in, 48 bands out, 48 bands in (state "speaking")
fn frame_bytes(t: f64) -> Vec<u8> {
    let (l, b) = synth(t);
    let mut v: Vec<f32> = vec![l as f32, 0.0];
    v.extend(b.iter().map(|x| *x as f32));
    v.extend(std::iter::repeat(0.0f32).take(48));
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
fn b64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char); s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

fn serve(ctx: &WebContext, t0: Instant) {
    let sm = ctx.security_manager().expect("security manager");
    sm.register_uri_scheme_as_secure("kai");
    sm.register_uri_scheme_as_cors_enabled("kai");
    ctx.register_uri_scheme("kai", move |req| {
        let path = req.path().map(|p| p.to_string()).unwrap_or_default();
        let uri = req.uri().map(|u| u.to_string()).unwrap_or_default();
        let csp = uri.split("csp=").nth(1).map(|s| s.split('&').next().unwrap_or("").to_string()).unwrap_or_default();
        let (mut body, ty): (Vec<u8>, &str) = match path.as_str() {
            "/index.html" => (INDEX.into(), "text/html"),
            "/adapter.js" => (ADAPTER.into(), "text/javascript"),
            "/viz/engine.js" => (ENGINE.into(), "text/javascript"),
            "/viz/ring.js" => (RING.into(), "text/javascript"),
            "/esm-boot.js" => (ESM_BOOT.into(), "text/javascript"),
            "/esm.html" => {
                let h = if csp == "meta" { format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP_STRICT}\">") } else { String::new() };
                (ESM.replace("<!--CSP-->", &h).into(), "text/html")
            }
            "/feed" => (frame_bytes(t0.elapsed().as_secs_f64()), "application/octet-stream"),
            _ => { req.finish_error(&mut glib::Error::new(gio::IOErrorEnum::NotFound, "404")); return }
        };
        let len = body.len() as i64;
        body.shrink_to_fit();
        let s = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(body));
        if csp == "header" {
            let resp = URISchemeResponse::new(&s, len);
            resp.set_content_type(ty);
            let h = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
            h.append("Content-Security-Policy", CSP_STRICT);
            resp.set_http_headers(h);
            req.finish_with_response(&resp);
        } else {
            req.finish(&s, len, Some(ty));
        }
    });
}

fn view_with(ctx: &WebContext, ucm: &UserContentManager) -> WebView {
    let v = WebView::new_with_context_and_user_content_manager(ctx, ucm);
    v.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    if let Some(s) = WebViewExt::settings(&v) { s.set_enable_developer_extras(false); }
    v
}
fn messages(ucm: &UserContentManager, tag: &str, t0: Instant, sink: impl Fn(&str) + 'static) {
    ucm.register_script_message_handler("kai");
    let tag = tag.to_string();
    ucm.connect_script_message_received(Some("kai"), move |_, r| {
        if let Some(v) = r.js_value() {
            let s = v.to_str().to_string();
            println!("[{tag} +{:.3}s] {}", t0.elapsed().as_secs_f32(), s);
            sink(&s);
        }
    });
}

fn region_for(o: &Opts, h: i32) -> cairo::Region {
    // GDK3 sends an EMPTY region as wl_surface.set_input_region(nil) = "whole surface takes input": 1x1 px stands in for click-through.
    if o.has("interactive") { cairo::Region::create_rectangle(&cairo::RectangleInt::new(10, h - 50, 120, 40)) }
    else { cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, 1, 1)) }
}
fn apply_region(w: &gtk::Window, o: &Opts, tag: &str) {
    if let Some(gw) = w.window() {
        let (aw, ah) = (w.allocated_width(), w.allocated_height());
        let r = region_for(o, ah);
        eprintln!("[wp0] apply_region({tag}) alloc {aw}x{ah} interactive={}", o.has("interactive"));
        gw.input_shape_combine_region(&r, 0, 0);
    }
}

fn overlay_window(o: &Rc<Opts>, view: &WebView) -> gtk::Window {
    let size = o.n("size", 320).min(320) as i32;
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title("kai-wp0-overlay");
    win.set_decorated(false);
    win.set_app_paintable(true);
    if let Some(vis) = gdk::Screen::default().and_then(|s| s.rgba_visual()) { win.set_visual(Some(&vis)); }
    win.init_layer_shell();
    win.set_layer(match o.s("layer", "overlay").as_str() { "top" => Layer::Top, "bottom" => Layer::Bottom, _ => Layer::Overlay });
    win.set_namespace("kai-wp0");
    win.set_keyboard_mode(KeyboardMode::None);
    win.set_exclusive_zone(-1);
    win.set_anchor(Edge::Bottom, true);
    win.set_anchor(Edge::Right, true);
    win.set_layer_shell_margin(Edge::Right, 24);
    win.set_layer_shell_margin(Edge::Bottom, 24);
    win.set_default_size(size, size);
    win.set_size_request(size, size);
    win.add(view);
    let o1 = o.clone();
    win.connect_map_event(move |w, _| { apply_region(w, &o1, "map-event"); glib::Propagation::Proceed });
    if o.has("reapply") {
        let o2 = o.clone();
        win.connect_size_allocate(move |w, _| { apply_region(w, &o2, "size-allocate"); });
    }
    win
}

fn main() {
    // NVIDIA rule: before GTK or any thread starts; an explicit value wins.
    if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none()
        && (std::path::Path::new("/dev/nvidiactl").exists() || std::path::Path::new("/sys/module/nvidia").exists())
    {
        std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
    }
    let o = Rc::new(parse());
    let secs = o.n("secs", 20).min(60) as u64;
    let mode = o.s("mode", "ring");
    let start = Instant::now();
    if mode == "life" { println!("[life] pss_before_gtk_init {}", pss_report("bare-before-gtk")); }
    gtk::init().expect("gtk init");
    println!("pid {} mode {mode} layer_shell_supported={}", std::process::id(), gtk_layer_shell::is_supported());
    let css = gtk::CssProvider::new();
    css.load_from_data(b"window, window.background { background: transparent; }").unwrap();
    gtk::StyleContext::add_provider_for_screen(&gdk::Screen::default().unwrap(), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

    match mode.as_str() {
        "esm" => esm(o.clone(), start),
        "life" => life(o.clone(), start),
        "plain" => plain(o.clone()),
        "nav" => nav(o.clone(), start),
        _ => ring_modes(o.clone(), start, secs),
    }
    glib::timeout_add_local_once(Duration::from_secs(if mode == "esm" || mode == "life" { 55 } else { secs }), || gtk::main_quit());
    gtk::main();
    println!("exit");
}

// ---------- ring | click | swatch | feed | resize ----------
fn ring_modes(o: Rc<Opts>, start: Instant, secs: u64) {
    let mode = o.s("mode", "ring");
    let ctx = WebContext::default().unwrap();
    serve(&ctx, start);
    let ready = Rc::new(Cell::new(false));
    let ucm = UserContentManager::new();
    let r2 = ready.clone();
    messages(&ucm, "page", start, move |s| { if s.contains("\"ev\":\"ready\"") { r2.set(true) } });
    let view = view_with(&ctx, &ucm);
    let win = overlay_window(&o, &view);

    // optional fullscreen test window (a normal xdg toplevel) opened after the overlay is up
    if o.has("fullscreen") {
        let c: Vec<f64> = o.s("fullscreen", "20,60,30").split(',').map(|x| x.parse::<f64>().unwrap_or(0.0) / 255.0).collect();
        let delay = o.n("fs-delay", 1500) as u64;
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            let fw = gtk::Window::new(gtk::WindowType::Toplevel);
            fw.set_title("kai-wp0-fullscreen-test");
            fw.set_app_paintable(true);
            fw.connect_draw(move |_, cr| { cr.set_source_rgb(c[0], c[1], c[2]); let _ = cr.paint(); glib::Propagation::Proceed });
            fw.fullscreen();
            fw.show_all();
            eprintln!("[wp0] fullscreen window shown");
            let fw2 = fw.clone();
            glib::timeout_add_local_once(Duration::from_secs(secs.saturating_sub(1).max(1)), move || fw2.close());
        });
    }
    win.show_all();
    let mode_q = if mode == "feed" { "ring" } else { mode.as_str() };
    view.load_uri(&format!("kai://app/index.html?mode={mode_q}&transport={}", o.s("transport", "eval")));

    if mode == "resize" {
        let (w2, v2) = (win.clone(), view.clone());
        let step = |ms: u64, label: &'static str, f: Box<dyn Fn()>| {
            glib::timeout_add_local_once(Duration::from_millis(ms), move || { eprintln!("[wp0] ---- {label}"); f(); });
        };
        let w = w2.clone(); step(2000, "A set_size_request(240,240)", Box::new(move || w.set_size_request(240, 240)));
        let w = w2.clone(); step(4000, "B set_size_request(320,320)", Box::new(move || w.set_size_request(320, 320)));
        let w = w2.clone(); step(6000, "C set_size_request(200,160)", Box::new(move || w.set_size_request(200, 160)));
        let w = w2.clone(); step(8000, "D set_size_request(320,320)", Box::new(move || w.set_size_request(320, 320)));
        let w = w2.clone(); step(10000, "E gtk_window_resize(260,260) (size request stays 320 -> no-op expected)", Box::new(move || w.resize(260, 260)));
        let w = w2.clone(); let v = v2.clone(); step(12000, "F view.set_size_request(280,280) with window request cleared", Box::new(move || { w.set_size_request(-1, -1); v.set_size_request(280, 280); w.resize(1, 1); }));
        let w = w2.clone(); step(14000, "G window request back to 320", Box::new(move || { v2.set_size_request(-1, -1); w.set_size_request(320, 320); }));
    }

    // feed
    let transport = o.s("transport", "eval");
    let hz = o.n("hz", 30).max(1) as u64;
    let scenario = o.s("scenario", if mode == "feed" { "speaking" } else { "cycle" });
    let states = ["idle", "listening", "thinking", "speaking"];
    let cur = Rc::new(RefCell::new(String::new()));
    let frames = Rc::new(Cell::new(0u64));
    let (v3, ready3) = (view.clone(), ready.clone());
    let ready_at = Rc::new(Cell::new(None::<Instant>));
    glib::timeout_add_local(Duration::from_millis(1000 / hz), move || {
        if !ready3.get() { return glib::ControlFlow::Continue; }
        let t0 = *ready_at.get().get_or_insert_with(|| { let n = Instant::now(); ready_at.set(Some(n)); n }).borrow_t();
        let t = t0.elapsed().as_secs_f64();
        let st = match scenario.as_str() { "cycle" => states[((t / 4.0) as usize) % 4], s => s };
        if *cur.borrow() != st {
            *cur.borrow_mut() = st.to_string();
            println!("[host +{t:.1}s] state -> {st}");
            v3.evaluate_javascript(&format!("__kai.setState('{st}')"), None, None, gio::Cancellable::NONE, |_| {});
        }
        if st == "speaking" || st == "listening" {
            match transport.as_str() {
                "none" | "fetch" => {}
                "b64" => v3.evaluate_javascript(&format!("__kai.feedB64('{}')", b64(&frame_bytes(t))), None, None, gio::Cancellable::NONE, |_| {}),
                "async" => {
                    let mut d = HashMap::new();
                    d.insert("b".to_string(), b64(&frame_bytes(t)).to_variant());
                    v3.call_async_javascript_function("__kai.feedB64(b)", Some(&d.to_variant()), None, None, gio::Cancellable::NONE, |r| {
                        if let Err(e) = r { eprintln!("async err {e}") }
                    });
                }
                _ => {
                    let (l, b) = synth(t);
                    let z = vec![0.0; 48];
                    let js = if st == "speaking" { format!("__kai.feed({l:.3},0,{},{})", arr(&b), arr(&z)) } else { format!("__kai.feed(0,{l:.3},{},{})", arr(&z), arr(&b)) };
                    v3.evaluate_javascript(&js, None, None, gio::Cancellable::NONE, |_| {});
                }
            }
        }
        frames.set(frames.get() + 1);
        glib::ControlFlow::Continue
    });
    let v4 = view.clone();
    glib::timeout_add_local_once(Duration::from_secs(secs.saturating_sub(1).max(1)), move || {
        v4.evaluate_javascript("__kai.stats()", None, None, gio::Cancellable::NONE, |r| { if let Ok(v) = r { println!("stats {}", v.to_str()) } });
    });
    // keep refs alive for the process lifetime
    std::mem::forget((win, view, ucm, ctx));
}
trait BorrowT { fn borrow_t(&self) -> &Instant; }
impl BorrowT for Instant { fn borrow_t(&self) -> &Instant { self } }

// ---------- nav: what does WebKitGTK do with a lmgw-pair: link by itself? (m, pre-test) ----------
fn nav(o: Rc<Opts>, start: Instant) {
    use webkit2gtk::{NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionType, URIRequestExt, PolicyDecisionExt};
    let ctx = WebContext::new_ephemeral();
    let ucm = UserContentManager::new();
    messages(&ucm, "nav", start, |_| {});
    let view = view_with(&ctx, &ucm);
    view.connect_decide_policy(|_, d, t| {
        if matches!(t, PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction) {
            if let Some(nd) = d.clone().downcast::<NavigationPolicyDecision>().ok() {
                if let Some(a) = nd.navigation_action() { if let Some(r) = a.request() {
                    let uri = r.uri().map(|u| u.to_string()).unwrap_or_default();
                    // never print the whole URI: it may carry a key (K13)
                    println!("decide-policy {:?} scheme={} nav_type={:?} user_gesture={}", t, uri.split(':').next().unwrap_or(""), a.navigation_type(), a.is_user_gesture());
                } }
            }
        }
        false // default: webkit decides (use)
    });
    view.connect_load_failed(|_, _, uri, e| { println!("load-failed scheme={} err={}", uri.split(':').next().unwrap_or(""), e); false });
    view.connect_create(|_, a| { println!("create (window.open / target=_blank) scheme={}", a.request().and_then(|r| r.uri()).map(|u| u.to_string().split(':').next().unwrap_or("").to_string()).unwrap_or_default()); None });
    let win = overlay_window(&o, &view);
    win.show_all();
    view.load_html("<a id=a href='lmgw-pair:?v=1&url=http://127.0.0.1:1&name=t&key=TESTKEY'>pair</a> <a id=b target=_blank href='lmgw-pair:?v=1&key=TESTKEY'>pair2</a>", Some("kai://app/"));
    let v = view.clone();
    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
        println!("-- JS a.click()");
        v.evaluate_javascript("document.getElementById('a').click()", None, None, gio::Cancellable::NONE, |_| {});
    });
    let v = view.clone();
    glib::timeout_add_local_once(Duration::from_millis(3000), move || {
        println!("-- JS b.click() (target=_blank)");
        v.evaluate_javascript("document.getElementById('b').click()", None, None, gio::Cancellable::NONE, |_| {});
    });
    let v = view.clone();
    glib::timeout_add_local_once(Duration::from_millis(4500), move || {
        println!("-- JS window.open");
        v.evaluate_javascript("window.open('lmgw-pair:?v=1&key=TESTKEY')", None, None, gio::Cancellable::NONE, |_| {});
    });
    std::mem::forget((win, view, ucm, ctx));
}

// ---------- plain: a layer surface without WebKit (multi-output probe) ----------
fn plain(o: Rc<Opts>) {
    let d = gdk::Display::default().unwrap();
    for i in 0..d.n_monitors() {
        if let Some(m) = d.monitor(i) { let g = m.geometry(); println!("monitor {i}: {:?} {:?} {}x{}+{}+{} scale {} primary {}", m.model(), m.manufacturer(), g.width(), g.height(), g.x(), g.y(), m.scale_factor(), m.is_primary()); }
    }
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_app_paintable(true);
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace("kai-wp0-plain");
    win.set_anchor(Edge::Bottom, true);
    win.set_anchor(Edge::Right, true);
    win.set_exclusive_zone(-1);
    win.set_keyboard_mode(KeyboardMode::None);
    win.set_default_size(200, 200);
    win.set_size_request(200, 200);
    if o.has("output-n") {
        let n = o.n("output-n", 0) as i32;
        if let Some(m) = d.monitor(n) { win.set_monitor(&m); println!("explicit monitor {n}"); }
    } else { println!("output = null (compositor's choice)"); }
    win.connect_draw(|_, cr| { cr.set_source_rgb(0.9, 0.5, 0.1); let _ = cr.paint(); glib::Propagation::Proceed });
    win.show_all();
    let w2 = win.clone();
    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
        if let Some(gw) = w2.window() { let m = w2.display().monitor_at_window(&gw); println!("GDK says the surface is on monitor {:?}", m.map(|m| { let g = m.geometry(); (g.x(), g.y(), g.width(), g.height()) })); }
    });
    std::mem::forget(win);
}

// ---------- esm ----------
fn esm(o: Rc<Opts>, start: Instant) {
    let assets = o.s("assets", &format!("{}/assets", std::env::current_dir().unwrap().display()));
    // name, url, file-access, universal-access
    let cases: Vec<(String, String, bool, bool)> = vec![
        ("file:// default".into(), format!("file://{assets}/esm.html"), false, false),
        ("file:// allow_file_access_from_file_urls".into(), format!("file://{assets}/esm.html"), true, false),
        ("file:// + universal access".into(), format!("file://{assets}/esm.html"), true, true),
        ("kai:// no CSP".into(), "kai://app/esm.html".into(), false, false),
        ("kai:// CSP meta (strict)".into(), "kai://app/esm.html?csp=meta".into(), false, false),
        ("kai:// CSP header (strict)".into(), "kai://app/esm.html?csp=header".into(), false, false),
    ];
    glib::MainContext::default().spawn_local(async move {
        for (name, url, fa, ua) in cases {
            let ctx = WebContext::new_ephemeral();
            serve(&ctx, start);
            let got: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![]));
            let ucm = UserContentManager::new();
            let g2 = got.clone();
            messages(&ucm, "esm", start, move |s| g2.borrow_mut().push(s.to_string()));
            let view = view_with(&ctx, &ucm);
            if let Some(s) = WebViewExt::settings(&view) { s.set_allow_file_access_from_file_urls(fa); s.set_allow_universal_access_from_file_urls(ua); }
            let g3 = got.clone();
            view.connect_load_failed(move |_, _, uri, e| { g3.borrow_mut().push(format!("load-failed {uri}: {e}")); false });
            let win = overlay_window(&o, &view); // 100 px layer surface, ~1 s per case (the offscreen window has no GL context)
            win.show_all();
            println!("---- case: {name}  url {url}");
            view.load_uri(&url);
            for _ in 0..80 { // up to 4 s
                glib::timeout_future(Duration::from_millis(50)).await;
                if got.borrow().iter().any(|m| m.contains("raf-ok") || m.contains("load-failed")) { break; }
            }
            glib::timeout_future(Duration::from_millis(300)).await;
            let g = got.borrow();
            let has = |k: &str| g.iter().any(|m| m.contains(k));
            println!("RESULT {name}: module_ok={} raf_ok={} inline_script_ran={} module_error_event={} csp_violation={} other={:?}", has("esm-ok"), has("raf-ok"), has("inline-ran"), has("module-error"), has("\"ev\":\"csp\""),
                g.iter().filter(|m| m.contains("throw") || m.contains("load-failed")).collect::<Vec<_>>());
            drop(g);
            win.close();
        }
        gtk::main_quit();
    });
}

// ---------- life ----------
fn children(root: u32) -> Vec<(u32, String)> {
    let mut map: HashMap<u32, (u32, String)> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            if let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() {
                if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    if let (Some(a), Some(b)) = (s.find('('), s.rfind(')')) {
                        let ppid = s[b + 2..].split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                        map.insert(pid, (ppid, s[a + 1..b].to_string()));
                    }
                }
            }
        }
    }
    let mut out = vec![]; let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for (&c, (pp, n)) in &map { if *pp == p { out.push((c, n.clone())); stack.push(c); } }
    }
    out
}
fn pss_kb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Pss:")).and_then(|l| l.split_whitespace().nth(1).and_then(|x| x.parse().ok()))).unwrap_or(0)
}
fn pss_report(label: &str) -> String {
    let me = std::process::id();
    let mut s = format!("{label}: ui={}MB", pss_kb(me) / 1024);
    let ch = children(me);
    if ch.is_empty() { s += " children=none" }
    for (p, n) in ch { s += &format!(" {n}[{p}]={}MB", pss_kb(p) / 1024); }
    s
}
fn life(o: Rc<Opts>, start: Instant) {
    let cycles = o.n("cycles", 3);
    glib::MainContext::default().spawn_local(async move {
        println!("[life] {}", pss_report("after-gtk-init-no-webkit"));
        let own = o.s("ctx", "default") == "own";
        let mut ctx = WebContext::default().unwrap();
        if !own { serve(&ctx, start); }
        println!("[life] {}", pss_report("webcontext-created-no-view"));
        glib::timeout_future(Duration::from_millis(1500)).await;
        println!("[life] {}", pss_report("webcontext-no-view+1.5s"));
        for k in 1..=cycles {
            if own { ctx = WebContext::new_ephemeral(); serve(&ctx, start); }
            let t0 = Instant::now();
            let ready: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
            let f1: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
            let ucm = UserContentManager::new();
            let (r, f) = (ready.clone(), f1.clone());
            messages(&ucm, &format!("cycle{k}"), start, move |s| {
                if s.contains("\"ev\":\"ready\"") && r.get().is_none() { r.set(Some(t0.elapsed().as_secs_f64() * 1000.0)) }
                if s.contains("\"ev\":\"frame1\"") && f.get().is_none() { f.set(Some(t0.elapsed().as_secs_f64() * 1000.0)) }
            });
            let view = view_with(&ctx, &ucm);
            let win = overlay_window(&o, &view);
            win.show_all();
            view.load_uri("kai://app/index.html?mode=ring");
            for _ in 0..400 { glib::timeout_future(Duration::from_millis(5)).await; if f1.get().is_some() { break; } }
            println!("RESULT cycle {k}: create->ready {:?} ms, create->first-frame {:?} ms", ready.get().map(|x| x.round()), f1.get().map(|x| x.round()));
            view.evaluate_javascript("__kai.setState('speaking')", None, None, gio::Cancellable::NONE, |_| {});
            glib::timeout_future(Duration::from_millis(2500)).await;
            println!("[life] {}", pss_report(&format!("cycle{k}-view-up")));
            win.close();
            drop(win); drop(view); drop(ucm);
            if own { ctx = WebContext::default().unwrap(); } // the per-cycle ephemeral context is released
            let waits: &[u64] = if k == 1 { &[500, 1500, 3000, 5000, 10000] } else { &[500, 2500] };
            let mut t = 0;
            for w in waits {
                glib::timeout_future(Duration::from_millis(*w)).await; t += *w;
                println!("[life] {}", pss_report(&format!("cycle{k}-destroyed+{:.1}s", t as f64 / 1000.0)));
            }
        }
        gtk::main_quit();
    });
}
