//! Kai's browser site rules on the desktop's tools (`crate::desktop::sites`), over a fake linked
//! Firefox (`crate::browser::shown::fake`) and fake desktop capabilities: each tool on a Firefox
//! window that shows a `none`, a `read` or an `act` site and on one Kai cannot tell, the checks
//! after a capture and after a dialog, `see_screen`'s blanking by the windows' frames, the fast
//! path that lists nothing, and the provider's wiring; the keys ([`keys`]), the pages read and
//! pressed in ([`pages`]) and the captures' own checks ([`captures`]) have files of their own.

mod captures;
mod keys;
mod pages;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use kai_desktop::caps::Fut;
use kai_desktop::caps::accessibility::{Element, Found, Tree};
use kai_desktop::caps::capture::{Capture, Pixels};
use kai_desktop::caps::windows::{Output, Placed, Placement, Snapshot, Window};
use serde_json::{Value, json};

use super::FakeWindows;
use super::input::FakeInput;
use super::read::{FakeAccess, solid};
use crate::browser::shown::GAP;
use crate::browser::shown::fake::{FakeFirefox, context, front, listing, shown};
use crate::desktop::tools::{self, Ctx, Services};
use crate::host::registry::{Args, Block, Origin, Reply};

const PID: u32 = 4242;
const MAIL: &str = "mail.example";
const SHOP: &str = "shop.example";

fn win(id: &str, app: &str, title: &str, pid: u32, active: bool) -> Window {
    Window {
        id: id.into(),
        app: app.into(),
        title: title.into(),
        pid,
        output: "DP-1".into(),
        desktops: vec!["Main".into()],
        active,
        ..Default::default()
    }
}

/// Firefox's windows: ff1 shows the mail, ff2 the shop, ff3 the news; ffx is a Firefox without
/// Kai for Firefox; kon a terminal. `active` has the focus.
fn snap(active: &str) -> Snapshot {
    let ff = "org.mozilla.firefox";
    let windows = [
        ("ff1", ff, "Inbox — Mozilla Firefox", PID),
        ("ff2", ff, "Lamps — Mozilla Firefox", PID),
        ("ff3", ff, "News — Mozilla Firefox", PID),
        ("ffx", "firefox", "Bank — Mozilla Firefox", 7),
        ("kon", "org.kde.konsole", "build — Konsole", 9),
    ]
    .into_iter()
    .map(|(id, app, title, pid)| win(id, app, title, pid, id == active))
    .collect();
    Snapshot {
        windows,
        outputs: vec![
            Output {
                name: "DP-1".into(),
                width: 2000,
                height: 1000,
                scale: 1.0,
                active: true,
                ..Default::default()
            },
            Output {
                name: "HDMI-A-1".into(),
                x: 2000,
                width: 1000,
                height: 500,
                scale: 1.0,
                ..Default::default()
            },
        ],
        desktops: vec!["Main".into(), "Mail".into()],
        desktop: "Main".into(),
        activity: String::new(),
    }
}

/// The tabs in front of the extension's windows 1 to 3.
fn tabs() -> Value {
    listing(
        vec![
            front(3, 1, "Inbox", "https://mail.example/inbox", true),
            front(5, 2, "Lamps", "https://shop.example/lamps", false),
            front(7, 3, "News", "https://news.example/", false),
        ],
        &[1, 2, 3],
        None,
    )
}

/// What runs after a capture.
type Then = Option<Box<dyn Fn() + Send>>;

/// Captures: a window 4x2, DP-1 20x10 and HDMI-A-1 10x5 (a tenth of their logical size), each
/// recorded; `then` runs after each capture (a tab switched while it ran).
#[derive(Clone, Default)]
struct Shots {
    asked: Arc<Mutex<Vec<String>>>,
    then: Arc<Mutex<Then>>,
    screen: Option<Pixels>,
}

impl Shots {
    fn after(&self, f: impl Fn() + Send + 'static) {
        *self.then.lock().unwrap() = Some(Box::new(f));
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    fn done(&self) {
        if let Some(f) = &*self.then.lock().unwrap() {
            f();
        }
    }
}

impl Capture for Shots {
    fn window<'a>(&'a self, id: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap()
                .push(format!("window {}", id.unwrap_or("active")));
            self.done();
            Ok(solid(4, 2, [10, 20, 30, 255]))
        })
    }

    fn screen<'a>(&'a self, output: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(async move {
            let o = output.unwrap_or("active");
            self.asked.lock().unwrap().push(format!("screen {o}"));
            self.done();
            Ok(match (&self.screen, o) {
                (Some(p), _) => p.clone(),
                (None, "HDMI-A-1") => solid(10, 5, [1, 2, 3, 255]),
                (None, _) => solid(20, 10, [1, 2, 3, 255]),
            })
        })
    }
}

/// A desktop with Firefox's windows, the fakes and Kai's site rules over `fx`.
struct Desk {
    fx: Arc<FakeFirefox>,
    windows: FakeWindows,
    shots: Shots,
    access: FakeAccess,
    input: FakeInput,
}

/// The address of the tab in front of the window `id` in [`tabs`].
fn url_of(id: &str) -> &'static str {
    match id {
        "ff1" => "https://mail.example/inbox",
        "ff2" => "https://shop.example/lamps",
        _ => "https://news.example/",
    }
}

impl Desk {
    fn new(active: &str) -> Self {
        let fx = FakeFirefox::start(Some(PID), tabs());
        // The focused element is on the page itself (`context.get@1`).
        fx.set_context(context(
            url_of(active),
            true,
            json!({"ref": 1, "role": "textbox", "name": "q"}),
            0,
        ));
        Desk {
            fx: Arc::new(fx),
            windows: FakeWindows {
                snap: snap(active),
                ..Default::default()
            },
            shots: Shots::default(),
            access: FakeAccess {
                tree: Tree {
                    root: Element {
                        role: "frame".into(),
                        name: "page".into(),
                        ..Default::default()
                    },
                    unanswered: 0,
                },
                selection: vec!["a selected line".into()],
                found: Some(Found {
                    role: "push button".into(),
                    name: "Send".into(),
                    at: (":1.5".into(), "/a/1".into()),
                    documents: vec!["https://news.example/".into()],
                }),
                ..Default::default()
            },
            input: FakeInput::default(),
        }
    }

    fn services(&self, none: &[&str], read: &[&str]) -> Services {
        Services {
            windows: Some(Arc::new(self.windows.clone())),
            capture: Some(Arc::new(self.shots.clone())),
            accessibility: Some(Arc::new(self.access.clone())),
            input: Some(Arc::new(self.input.clone())),
            sites: Some(shown(&self.fx.links, none, read)),
            ..Services::default()
        }
    }

    fn placed_asked(&self) -> usize {
        self.windows.placed_asked.load(Ordering::SeqCst)
    }
}

fn args(name: &str, v: Value) -> Args {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    Args::read(spec, v.as_object().cloned()).unwrap()
}

async fn read(s: &Services, name: &str, v: Value) -> Reply {
    crate::desktop::read::run(s, name, &args(name, v)).await
}

fn first(r: &Reply) -> &str {
    match &r.blocks[0] {
        Block::Text(t) => t,
        other => panic!("{other:?}"),
    }
}

/// The input tools as the host runs them: the preparation, then the call with what it prepared.
async fn act(s: &Services, name: &'static str, v: Value) -> Result<String, String> {
    act_after(s, name, v, || {}).await
}

/// [`act`] with `meanwhile` run between the preparation and the call (the dialog's wait).
async fn act_after(
    s: &Services,
    name: &'static str,
    v: Value,
    meanwhile: impl FnOnce(),
) -> Result<String, String> {
    let a = args(name, v);
    let p = tools::input::prepare(s, name)(&a, Origin::default()).await?;
    meanwhile();
    let ctx = Ctx {
        prepared: Some(p),
        ..Ctx::default()
    };
    let r = tools::run_with(s, name, &a, &ctx).await;
    let text = first(&r).to_owned();
    if r.error { Err(text) } else { Ok(text) }
}

const NONE_SEE: &str = "Firefox's window ff1 shows a site Kai does not reach (Kai's site rules)";

#[tokio::test]
async fn see_window_refuses_a_none_site_and_says_the_gap_on_the_others() {
    let d = Desk::new("kon");
    let s = d.services(&[MAIL], &[SHOP]);
    let r = read(&s, "see_window", json!({"window": "ff1"})).await;
    assert!(r.error);
    assert_eq!(first(&r), format!("{NONE_SEE}; see_window did not run"));
    assert!(d.shots.asked().is_empty(), "nothing captured");
    // A read site and an act site are seen, with the line on what was not checked.
    for id in ["ff2", "ff3"] {
        let r = read(&s, "see_window", json!({"window": id})).await;
        assert!(!r.error, "{}", first(&r));
        let lines: Vec<&str> = first(&r).lines().collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].ends_with("Mozilla Firefox\", 4x2 pixels"));
        assert_eq!(lines[1], GAP);
        assert!(matches!(r.blocks[1], Block::Image { .. }));
    }
    // A Firefox without Kai for Firefox: Kai cannot tell, and some site is none.
    let r = read(&s, "see_window", json!({"window": "ffx"})).await;
    assert!(r.error);
    assert!(
        first(&r).starts_with(
            "Kai cannot tell which site Firefox's window ffx shows (no Firefox linked with Kai \
             for Firefox owns it)"
        ),
        "{}",
        first(&r)
    );
    assert!(!first(&r).contains("Bank"), "never the window's title");
    // Another app's window: as before, no note.
    let r = read(&s, "see_window", json!({"window": "kon"})).await;
    assert_eq!(
        first(&r),
        "kon org.kde.konsole \"build — Konsole\", 4x2 pixels"
    );
    assert_eq!(d.shots.asked(), ["window ff2", "window ff3", "window kon"]);
    // Without the window list Kai cannot tell any window.
    let bare = Services {
        windows: None,
        ..s.clone()
    };
    let r = read(&bare, "see_window", json!({})).await;
    assert!(r.error && first(&r).starts_with("this desktop offers Kai no window list"));
}

/// A tab that went to a `none` site while the capture ran: the capture is not shown.
#[tokio::test]
async fn a_tab_switched_during_the_capture_withholds_it() {
    let d = Desk::new("kon");
    let s = d.services(&[MAIL], &[]);
    let fx = d.fx.clone();
    d.shots.after(move || {
        fx.set(listing(
            vec![front(7, 3, "News", "https://mail.example/", false)],
            &[3],
            None,
        ))
    });
    let r = read(&s, "see_window", json!({"window": "ff3"})).await;
    assert!(r.error);
    assert_eq!(
        first(&r),
        "Firefox's window ff3 shows a site Kai does not reach (Kai's site rules) by the time \
         see_window had it, so Kai does not say what it got"
    );
    assert_eq!(r.blocks.len(), 1, "no image");
}

#[tokio::test]
async fn read_window_and_read_selection_refuse_a_none_site() {
    let d = Desk::new("ff1");
    let s = d.services(&[MAIL], &[SHOP]);
    let r = read(&s, "read_window", json!({"window": "ff1"})).await;
    assert_eq!(
        (first(&r), r.error),
        (
            format!("{NONE_SEE}; read_window did not run").as_str(),
            true
        )
    );
    let r = read(&s, "read_selection", json!({})).await;
    assert_eq!(
        (first(&r), r.error),
        (
            format!("{NONE_SEE}; read_selection did not run").as_str(),
            true
        )
    );
    assert!(
        d.access.asked.lock().unwrap().is_empty(),
        "nothing was read"
    );
    // Reading a read site: the tree with the line at its end, the selection whole and the line
    // in a block of its own.
    let r = read(&s, "read_window", json!({"window": "ff2"})).await;
    assert!(!r.error);
    assert_eq!(
        first(&r),
        format!("ff2 org.mozilla.firefox \"Lamps — Mozilla Firefox\"\nframe \"page\"\n{GAP}")
    );
    let d2 = Desk::new("ff2");
    let s2 = d2.services(&[MAIL], &[SHOP]);
    let r = read(&s2, "read_selection", json!({})).await;
    assert!(!r.error);
    assert_eq!(
        r.blocks,
        [
            Block::Text("a selected line".into()),
            Block::Text(GAP.into())
        ]
    );
}

#[tokio::test]
async fn the_input_tools_refuse_read_and_none_sites_and_unknown_firefox_windows() {
    for (active, refusal) in [
        ("ff1", NONE_SEE.to_owned()),
        (
            "ff2",
            "Firefox's window ff2 shows a site where Kai may only read (Kai's site rules)".into(),
        ),
        (
            "ffx",
            "Kai cannot tell which site Firefox's window ffx shows (no Firefox linked with Kai \
             for Firefox owns it), and Kai's site rules keep some sites from Kai"
                .into(),
        ),
    ] {
        let d = Desk::new(active);
        let s = d.services(&[MAIL], &[SHOP]);
        for (tool, v) in [
            ("type_text", json!({"text": "hello"})),
            ("key_press", json!({"keys": "enter"})),
            ("ui_press", json!({"element": "Send"})),
        ] {
            assert_eq!(
                act(&s, tool, v).await,
                Err(format!("{refusal}; {tool} did not run")),
                "{active} {tool}"
            );
        }
        assert!(d.input.sent_nothing());
        assert!(
            d.access.finds.lock().unwrap().is_empty(),
            "ui_press looked nothing up"
        );
    }
    // An act site, and another app's window.
    for active in ["ff3", "kon"] {
        let d = Desk::new(active);
        let s = d.services(&[MAIL], &[SHOP]);
        assert_eq!(
            act(&s, "type_text", json!({"text": "hello"})).await,
            Ok("typed 5 characters".into())
        );
        assert_eq!(
            act(&s, "key_press", json!({"keys": "enter"})).await,
            Ok("pressed 1 chord".into())
        );
        assert!(
            act(&s, "ui_press", json!({"element": "Send"}))
                .await
                .is_ok()
        );
    }
    // With only read sites, Kai cannot tell the unlinked Firefox's: still no acting in it.
    let d = Desk::new("ffx");
    let s = d.services(&[], &[SHOP]);
    let e = act(&s, "type_text", json!({"text": "hello"}))
        .await
        .unwrap_err();
    assert!(
        e.ends_with("Kai's site rules let Kai only read some sites; type_text did not run"),
        "{e}"
    );
}

/// The tab in front changed to a read site while the dialog waited: the call is refused when it
/// runs, and nothing is typed.
#[tokio::test]
async fn an_input_call_is_checked_again_after_its_dialog() {
    let d = Desk::new("ff3");
    let s = d.services(&[MAIL], &[SHOP]);
    let fx = d.fx.clone();
    let r = act_after(&s, "type_text", json!({"text": "hello"}), move || {
        fx.set(listing(
            vec![front(7, 3, "News", "https://shop.example/news", false)],
            &[3],
            None,
        ))
    })
    .await;
    assert_eq!(
        r,
        Err(
            "Firefox's window ff3 shows a site where Kai may only read (Kai's site rules); \
             type_text did not run"
                .into()
        )
    );
    assert!(d.input.sent_nothing());
}

/// No `none` site: the seeing and reading tools do as before, list no tabs and ask nothing of
/// the windows' places; no `read` site either: the input tools neither.
#[tokio::test]
async fn without_rules_that_could_refuse_nothing_changes_and_nothing_is_listed() {
    for (none, read_only) in [(&[][..], &[SHOP][..]), (&[][..], &[][..])] {
        let d = Desk::new("ff1");
        let s = d.services(none, read_only);
        let r = read(&s, "see_window", json!({"window": "ff1"})).await;
        assert_eq!(
            first(&r),
            "ff1 org.mozilla.firefox \"Inbox — Mozilla Firefox\", 4x2 pixels"
        );
        assert_eq!(
            first(&read(&s, "see_screen", json!({})).await),
            "output DP-1, 20x10 pixels"
        );
        assert!(
            !read(&s, "read_window", json!({"window": "ffx"}))
                .await
                .error
        );
        assert_eq!(
            read(&s, "read_selection", json!({})).await.blocks,
            [Block::Text("a selected line".into())]
        );
        assert_eq!(d.fx.lists(), 0);
        assert_eq!(d.placed_asked(), 0);
    }
    let d = Desk::new("ff2");
    let s = d.services(&[], &[]);
    assert!(act(&s, "type_text", json!({"text": "hi"})).await.is_ok());
    assert!(
        act(&s, "ui_press", json!({"element": "Send"}))
            .await
            .is_ok()
    );
    assert_eq!(d.fx.lists(), 0);
}

fn placed(w: Window, frame: Option<(i32, i32, u32, u32)>) -> Placed {
    Placed { window: w, frame }
}

/// The pixels of a decoded RGB PNG, one char per pixel: `#` grey, `.` anything else.
fn pixels_of(r: &Reply) -> Vec<String> {
    let Block::Image { base64, .. } = &r.blocks[1] else {
        panic!("{:?}", r.blocks)
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buf.chunks(info.width as usize * 3)
        .take(info.height as usize)
        .map(|row| {
            row.chunks(3)
                .map(|p| if p == [0x80, 0x80, 0x80] { '#' } else { '.' })
                .collect()
        })
        .collect()
}

/// `see_screen` blanks the frames of Firefox's windows that show a none site or one Kai cannot
/// tell, on the output it captured, scaled to its pixels and widened to whole ones, whatever lies
/// over them; windows not drawn are left alone; the line says how many and what was not checked.
#[tokio::test]
async fn see_screen_blanks_firefox_windows_by_their_frames() {
    let mut d = Desk::new("kon");
    let s0 = snap("kon");
    let w = |id: &str| s0.windows.iter().find(|w| w.id == id).unwrap().clone();
    let mut minimized = w("ff1");
    minimized.id = "ffm".into();
    minimized.minimized = true;
    let mut elsewhere = w("ff1");
    elsewhere.id = "ffe".into();
    elsewhere.desktops = vec!["Mail".into()];
    let mut pip = win(
        "pip",
        "org.mozilla.firefox",
        "Picture-in-Picture",
        PID,
        false,
    );
    pip.desktops = vec![];
    d.windows.placed = Some(Placement {
        windows: vec![
            // The terminal over the mail: blanked with it.
            placed(w("kon"), Some((0, 0, 400, 300))),
            placed(w("ff1"), Some((100, 200, 500, 300))),
            placed(w("ff2"), Some((1000, 0, 500, 500))),
            // Half on DP-1, half on HDMI-A-1.
            placed(w("ffx"), Some((1650, 300, 700, 200))),
            placed(minimized, Some((0, 0, 2000, 1000))),
            placed(elsewhere, Some((0, 0, 2000, 1000))),
            // Picture-in-Picture leaves the task bar out and fits no tab's title.
            placed(pip, Some((1900, 400, 300, 300))),
        ],
        desktop: "Main".into(),
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert!(!r.error, "{}", first(&r));
    assert_eq!(
        first(&r),
        format!(
            "output DP-1, 20x10 pixels\n3 Firefox windows that show a site Kai does not reach, \
             or one Kai cannot tell, are blanked out grey with whatever lies over them (Kai's \
             site rules)\n{GAP}"
        )
    );
    assert_eq!(
        pixels_of(&r),
        [
            "....................",
            "....................",
            ".#####..............",
            ".#####..........####",
            ".#####..........####",
            "...................#",
            "...................#",
            "....................",
            "....................",
            "....................",
        ]
    );
    // The other output: the rest of the unlinked Firefox, and Picture-in-Picture; the shop is not
    // on it, so no line about what was not checked.
    let r = read(&s, "see_screen", json!({"output": "HDMI-A-1"})).await;
    assert_eq!(
        first(&r).lines().nth(1),
        Some(
            "2 Firefox windows that show a site Kai does not reach, or one Kai cannot tell, are \
             blanked out grey with whatever lies over them (Kai's site rules)"
        )
    );
    assert_eq!(first(&r).lines().count(), 2);
    assert_eq!(
        pixels_of(&r),
        [
            "..........",
            "..........",
            "..........",
            "####......",
            "####......",
        ]
    );
    // The places were read before each capture and again after.
    assert_eq!(d.placed_asked(), 4);
}

/// A fractional scale: frames in logical pixels land on whole capture pixels, widened outward.
#[tokio::test]
async fn see_screen_scales_frames_to_the_capture_and_widens_them() {
    let mut d = Desk::new("kon");
    d.windows.snap.outputs = vec![Output {
        name: "DP-1".into(),
        x: 0,
        y: 0,
        width: 16,
        height: 8,
        scale: 1.25,
        active: true,
    }];
    d.windows.placed = Some(Placement {
        windows: vec![placed(
            win(
                "ff1",
                "org.mozilla.firefox",
                "Inbox — Mozilla Firefox",
                PID,
                false,
            ),
            Some((2, 2, 4, 4)),
        )],
        desktop: "Main".into(),
    });
    d.shots.screen = Some(solid(20, 10, [1, 2, 3, 255]));
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    // 2..6 logical at 1.25: 2.5..7.5 pixels, widened to 2..8.
    assert_eq!(
        pixels_of(&r),
        [
            "....................",
            "....................",
            "..######............",
            "..######............",
            "..######............",
            "..######............",
            "..######............",
            "..######............",
            "....................",
            "....................",
        ]
    );
}

/// A tab that went to a none site while the screen was captured is blanked too (the look after).
#[tokio::test]
async fn see_screen_blanks_what_either_look_found() {
    let mut d = Desk::new("kon");
    d.windows.placed = Some(Placement {
        windows: vec![placed(
            win(
                "ff3",
                "org.mozilla.firefox",
                "News — Mozilla Firefox",
                PID,
                false,
            ),
            Some((0, 0, 200, 100)),
        )],
        desktop: "Main".into(),
    });
    let fx = d.fx.clone();
    d.shots.after(move || {
        fx.set(listing(
            vec![front(7, 3, "News", "https://mail.example/", false)],
            &[3],
            None,
        ))
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert_eq!(
        &pixels_of(&r)[..2],
        ["##..................", "...................."]
    );
    assert!(first(&r).contains("1 Firefox window that shows"));
}

/// Where the desktop does not say where its windows are, or where such a window is, Kai does
/// not capture the screen while some site is none.
#[tokio::test]
async fn see_screen_refuses_what_it_cannot_place() {
    let d = Desk::new("kon");
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert!(r.error);
    assert_eq!(
        first(&r),
        "Kai's site rules keep some sites from Kai, and Kai cannot tell where Firefox's windows \
         are (the desktop does not say where its windows are); see_screen did not run"
    );
    let mut d = Desk::new("kon");
    d.windows.placed = Some(Placement {
        windows: vec![placed(
            win(
                "ff1",
                "org.mozilla.firefox",
                "Inbox — Mozilla Firefox",
                PID,
                false,
            ),
            None,
        )],
        desktop: "Main".into(),
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert!(r.error);
    assert!(
        first(&r).starts_with(
            "Firefox's window ff1 shows a site Kai does not reach, or one Kai cannot tell"
        ),
        "{}",
        first(&r)
    );
    assert!(d.shots.asked().is_empty(), "nothing captured");
    // Without the window list.
    let bare = Services {
        windows: None,
        ..s.clone()
    };
    let r = read(&bare, "see_screen", json!({})).await;
    assert!(
        r.error && first(&r).contains("no window list"),
        "{}",
        first(&r)
    );
}

/// The provider Kai runs wires the site rules into the tools' services, and a wiring that would
/// be lost (the provider already shared) stops Kai instead of failing open.
#[tokio::test]
async fn the_provider_wires_the_site_rules() {
    let deps = crate::host::tests::deps();
    let p = crate::desktop::provider(&deps, crate::browser::spans::Spans::of(&deps));
    p.probe_now().await;
    let services = p.services().expect("probed");
    assert!(services.sites.is_some(), "the site rules");
    assert!(services.spans.is_some(), "the input spans");
}

#[test]
#[should_panic(expected = "DesktopTools::with_sites must run before the provider is cloned")]
fn a_late_wiring_stops_kai() {
    let deps = crate::host::tests::deps();
    let d = crate::desktop::DesktopTools::new(
        crate::desktop::Env::inert(),
        crate::desktop::settings::DesktopConfig::default(),
        crate::host::registry::Changes::new().0,
        false,
    );
    let _shared = d.clone();
    let _ = d.with_sites(crate::browser::shown::Shown::new(
        deps.browser.clone(),
        deps.policy.clone(),
    ));
}
