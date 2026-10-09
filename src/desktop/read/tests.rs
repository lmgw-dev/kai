//! The read tools over fake capabilities: what each lists, says, asks the provider for, and the
//! image blocks they return (decoded back, and a large ultrawide desktop's size against lmgw's
//! default host-link limits).

use std::sync::Arc;

use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Element, Tree};
use kai_desktop::caps::capture::Pixels;
use kai_desktop::caps::search::{Found, Recent};
use serde_json::{Value, json};

use super::*;
use crate::desktop::tests::read::{FakeAccess, FakeCapture, FakeClipboard, FakeSearch, solid};
use crate::desktop::tests::{FakeWindows, all_services, snapshot};
use crate::host::registry::{Args, Block};

async fn call(services: &Services, name: &str, args: Value) -> Reply {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    run(services, name, &args).await
}

async fn text(services: &Services, name: &str, args: Value) -> (String, bool) {
    let r = call(services, name, args).await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

fn windows() -> FakeWindows {
    FakeWindows {
        snap: snapshot(),
        ..Default::default()
    }
}

fn names(s: &Services) -> Vec<&'static str> {
    crate::desktop::entries(s)
        .iter()
        .filter(|e| e.area == "read")
        .map(|e| e.spec.name)
        .collect()
}

/// K23: each read tool is listed only where its capability is served; the accessibility tools
/// need the window list too, and `find_files` an index.
#[test]
fn only_what_the_desktop_supports_is_listed() {
    let all = all_services(windows());
    assert_eq!(names(&all).len(), 8);
    let no_windows = Services {
        windows: None,
        ..all.clone()
    };
    assert!(!names(&no_windows).contains(&"read_window"));
    assert!(!names(&no_windows).contains(&"read_selection"));
    assert!(names(&no_windows).contains(&"see_window"));
    let no_index = Services {
        search: Some(Arc::new(FakeSearch::default())),
        ..all.clone()
    };
    assert!(!names(&no_index).contains(&"find_files"));
    assert!(names(&no_index).contains(&"recent_files"));
    assert!(names(&Services::default()).is_empty());
}

fn decode(base64: &str) -> (png::OutputInfo, Vec<u8>) {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info, buf)
}

/// MCP image blocks: a line naming the window, then the PNG at the capture's own size, opaque as
/// RGB; the window resolved by K22 and captured by its id.
#[tokio::test]
async fn see_window_returns_a_line_and_an_image_block() {
    let capture = FakeCapture::default();
    let s = Services {
        capture: Some(Arc::new(capture.clone())),
        ..all_services(windows())
    };
    let r = call(&s, "see_window", json!({"window": "dolphin"})).await;
    assert!(!r.error);
    let [Block::Text(line), Block::Image { mime, base64 }] = &r.blocks[..] else {
        panic!("{:?}", r.blocks);
    };
    assert_eq!(line, "aaaa org.kde.dolphin \"Home — Dolphin\", 4x2 pixels");
    assert_eq!(mime, "image/png");
    let (info, data) = decode(base64);
    assert_eq!((info.width, info.height), (4, 2));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(&data[..3], [10, 20, 30]);
    call(&s, "see_window", json!({})).await;
    assert_eq!(
        capture.asked.lock().unwrap().clone(),
        ["window aaaa", "window bbbb"],
        "omitted: the active window, by its id"
    );
    let (e, err) = text(&s, "see_window", json!({"window": "konsole x"})).await;
    assert!(err && e.starts_with("no window has"), "{e}");
    // Without the window list: the active window, or the id as given.
    let bare = Services {
        windows: None,
        ..s.clone()
    };
    let r = call(&bare, "see_window", json!({})).await;
    assert!(matches!(&r.blocks[0], Block::Text(t) if t == "the active window, 4x2 pixels"));
    call(&bare, "see_window", json!({"window": "{x-1}"})).await;
    assert_eq!(capture.asked.lock().unwrap()[3], "window {x-1}");
    // `active` is the active window, with the window list and without it.
    let r = call(&bare, "see_window", json!({"window": "Active"})).await;
    assert!(matches!(&r.blocks[0], Block::Text(t) if t == "the active window, 4x2 pixels"));
    call(&s, "see_window", json!({"window": "active"})).await;
    let asked = capture.asked.lock().unwrap().clone();
    assert_eq!(asked[4], asked[2], "as if omitted");
    assert_eq!(asked[5], "window bbbb");
}

#[tokio::test]
async fn see_screen_names_the_output_it_captured() {
    let capture = FakeCapture::default();
    let s = Services {
        capture: Some(Arc::new(capture.clone())),
        ..all_services(windows())
    };
    let r = call(&s, "see_screen", json!({})).await;
    assert!(
        matches!(&r.blocks[..], [Block::Text(t), Block::Image { .. }] if t == "output DP-1, 6x3 pixels")
    );
    let r = call(&s, "see_screen", json!({"output": "hdmi-a-1"})).await;
    assert!(matches!(&r.blocks[0], Block::Text(t) if t == "output HDMI-A-1, 6x3 pixels"));
    let (e, err) = text(&s, "see_screen", json!({"output": "left"})).await;
    assert!(err);
    assert_eq!(e, "no such output; the outputs are: DP-1, HDMI-A-1");
    assert_eq!(
        capture.asked.lock().unwrap().clone(),
        ["screen DP-1", "screen HDMI-A-1"]
    );
    // A missing grant is §11.1's error, the tool's text.
    let refused = Services {
        capture: Some(Arc::new(FakeCapture {
            fail: Some(crate::desktop::plasma::capture::not_granted()),
            ..Default::default()
        })),
        ..s
    };
    let (e, err) = text(&refused, "see_screen", json!({})).await;
    assert!(err);
    assert_eq!(
        e,
        "needs the installed Kai: KWin grants org.kde.KWin.ScreenShot2 per .desktop file"
    );
}

/// A tiny xorshift, so the synthetic desktop is the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A large ultrawide desktop (5760x1440): a gradient wallpaper and six windows of text lines
/// (glyph-like noise in the lines), about what an ultrawide desktop with several apps holds.
fn desktop_ultrawide() -> Pixels {
    let (w, h) = (5760usize, 1440usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[
                (20 + x * 40 / w) as u8,
                (30 + y * 60 / h) as u8,
                (60 + (x + y) * 50 / (w + h)) as u8,
                255,
            ]);
        }
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..6 {
        let (x0, y0, ww, wh) = (40 + i * 840, 60 + (i % 2) * 120, 800, 1200);
        for y in y0..y0 + wh {
            for x in x0..x0 + ww {
                let o = (y * w + x) * 4;
                let in_text = (y - y0) % 24 < 16 && (y - y0) > 40 && (x - x0) % 400 < 360;
                let glyph = in_text && rng.next().is_multiple_of(3);
                let c = if y < y0 + 32 {
                    [60, 60, 70]
                } else if glyph {
                    [30, 30, 30]
                } else {
                    [250, 250, 250]
                };
                rgba[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    }
    Pixels {
        width: w as u32,
        height: h as u32,
        rgba,
    }
}

/// lmgw's default host-link limits (api §5.1: realtime's, `mcp.host_max_message_mb` 64 and
/// `mcp.host_max_frame_mb` 16; Kai sends each message as one frame, so the frame size binds).
const MAX_MESSAGE: usize = 64 << 20;
const MAX_FRAME: usize = 16 << 20;

/// The size of the JSON-RPC response carrying the line and the image as rmcp sends them.
fn message_size(line: &str, base64: &str) -> usize {
    use rmcp::model::{CallToolResult, ContentBlock};
    let result = CallToolResult::success(vec![
        ContentBlock::text(line.to_owned()),
        ContentBlock::image(base64.to_owned(), "image/png".to_owned()),
    ]);
    let msg = json!({"jsonrpc": "2.0", "id": 1, "result": result});
    serde_json::to_vec(&msg).unwrap().len()
}

/// A large ultrawide capture at its real resolution fits the default `mcp.host_max_message_mb` (and
/// the frame); even incompressible noise of that size fits the message limit. The figures print.
#[test]
fn a_large_ultrawide_capture_fits_lmgws_default_limits() {
    let desktop = desktop_ultrawide();
    let started = std::time::Instant::now();
    let b64 = image::png_base64(&desktop).unwrap();
    let size = message_size("output Virtual-0, 5760x1440 pixels", &b64);
    println!(
        "ultrawide desktop: {size} bytes as a message ({:.1} MiB), encoded in {} ms",
        size as f64 / (1 << 20) as f64,
        started.elapsed().as_millis()
    );
    assert!(size < MAX_FRAME, "{size}");
    let mut rng = Rng(7);
    let noise = Pixels {
        width: 5760,
        height: 1440,
        rgba: (0..5760 * 1440)
            .flat_map(|_| {
                let v = rng.next().to_le_bytes();
                [v[0], v[1], v[2], 255]
            })
            .collect(),
    };
    let size = message_size("x", &image::png_base64(&noise).unwrap());
    println!(
        "ultrawide noise: {size} bytes ({:.1} MiB)",
        size as f64 / (1 << 20) as f64
    );
    assert!(size < MAX_MESSAGE, "{size}");
}

#[test]
fn an_image_with_transparency_keeps_its_alpha() {
    let mut p = solid(2, 1, [255, 0, 0, 255]);
    p.rgba[7] = 0;
    let (info, data) = decode(&image::png_base64(&p).unwrap());
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(data, [255, 0, 0, 255, 255, 0, 0, 0]);
    let bad = Pixels {
        width: 2,
        height: 2,
        rgba: vec![0; 3],
    };
    assert!(image::png(&bad).is_err());
}

fn el(role: &str, name: &str, text: &str, children: Vec<Element>) -> Element {
    Element {
        role: role.into(),
        name: name.into(),
        text: text.into(),
        children,
        ..Default::default()
    }
}

/// `read_window`: the window's line, then the elements that say something or are controls,
/// indented under the shown element above them; a name or text cannot forge a line; the missing
/// elements counted.
#[tokio::test]
async fn read_window_says_the_tree_in_indented_lines() {
    let mut entry = el("entry", "", "", vec![]);
    entry.states = vec!["focused"];
    let tree = Tree {
        root: el(
            "frame",
            "Notes",
            "",
            vec![el(
                "panel",
                "",
                "",
                vec![
                    el("label", "Line\nforged", "Line\nforged", vec![]),
                    entry,
                    el("filler", "", "", vec![el("heading", "", "Title", vec![])]),
                ],
            )],
        ),
        unanswered: 2,
    };
    let access = FakeAccess {
        tree,
        ..Default::default()
    };
    let s = Services {
        accessibility: Some(Arc::new(access.clone())),
        ..all_services(windows())
    };
    let (t, err) = text(&s, "read_window", json!({"window": "dolphin"})).await;
    assert!(!err);
    assert_eq!(
        t,
        "aaaa org.kde.dolphin \"Home — Dolphin\"\n\
         frame \"Notes\"\n  \
           label \"Line\\nforged\"\n  \
           entry (focused)\n  \
           heading: \"Title\"\n\
         (2 elements did not answer and are missing)"
    );
    let asked = access.asked.lock().unwrap()[0].clone();
    assert_eq!(asked.title, "Home — Dolphin");
}

#[tokio::test]
async fn read_selection_reads_the_active_windows_selection() {
    let access = FakeAccess {
        selection: vec!["one".into(), "".into(), "two\nlines".into()],
        ..Default::default()
    };
    let s = Services {
        accessibility: Some(Arc::new(access.clone())),
        ..all_services(windows())
    };
    assert_eq!(
        text(&s, "read_selection", json!({})).await,
        ("one\ntwo\nlines".into(), false)
    );
    assert_eq!(access.asked.lock().unwrap()[0].title, "build — Konsole");
    let none = Services {
        accessibility: Some(Arc::new(FakeAccess::default())),
        ..s
    };
    assert_eq!(
        text(&none, "read_selection", json!({})).await.0,
        "nothing is selected in the active window (bbbb org.kde.konsole \"build — Konsole\")"
    );
}

/// `find_files` passes `search.limit`, says when the index has more and what the provider says
/// about the index; `recent_files` and the clipboard in compact lines.
#[tokio::test]
async fn files_and_the_clipboard_in_compact_lines() {
    let search = FakeSearch {
        indexes: true,
        found: Found {
            paths: vec!["/home/u/a.txt".into(), "/home/u/b\nc.txt".into()],
            more: true,
            notes: vec!["indexed folders: /home/u".into()],
        },
        recent: vec![
            Recent {
                location: "/home/u/r.pdf".into(),
                used: "2026-10-09T12:02:11.5Z".into(),
                apps: vec!["okular".into(), "kate".into()],
            },
            Recent {
                location: "https://example.org/".into(),
                used: "odd".into(),
                apps: vec![],
            },
        ],
        recent_private: 2,
        ..Default::default()
    };
    let s = Services {
        search: Some(Arc::new(search.clone())),
        clipboard: Some(Arc::new(FakeClipboard(
            "a\nb".into(),
            vec!["a\nb".into(), "".into(), "z".into()],
        ))),
        find_limit: 2,
        ..all_services(windows())
    };
    assert_eq!(
        text(&s, "find_files", json!({"query": " report "})).await,
        (
            "/home/u/a.txt\n\"/home/u/b\\nc.txt\"\nthe first 2 matches; the index has more \
             (search.limit)\nindexed folders: /home/u"
                .into(),
            false
        )
    );
    assert_eq!(search.asked.lock().unwrap()[0], ("report".into(), 2));
    let (e, err) = text(&s, "find_files", json!({"query": "  "})).await;
    assert!(err && e == "query is empty");
    assert_eq!(
        text(&s, "recent_files", json!({})).await.0,
        "2026-10-09 12:02 UTC /home/u/r.pdf (okular, kate)\nodd https://example.org/\n\
         (2 entries marked private by their writer are not listed)"
    );
    assert_eq!(text(&s, "clipboard_get", json!({})).await.0, "a\nb");
    assert_eq!(
        text(&s, "clipboard_history", json!({})).await.0,
        "1 \"a\\nb\"\n2 (no text)\n3 \"z\""
    );
    let empty = Services {
        search: Some(Arc::new(FakeSearch {
            indexes: true,
            ..Default::default()
        })),
        clipboard: Some(Arc::new(FakeClipboard(String::new(), vec![]))),
        ..s
    };
    assert_eq!(
        text(&empty, "find_files", json!({"query": "x"})).await.0,
        "no file matches"
    );
    assert_eq!(
        text(&empty, "recent_files", json!({})).await.0,
        "no recently used files"
    );
    assert_eq!(
        text(&empty, "clipboard_get", json!({})).await.0,
        "Klipper's clipboard entry holds no text"
    );
    assert_eq!(
        text(&empty, "clipboard_history", json!({})).await.0,
        "the clipboard history is empty"
    );
    let failing = Services {
        capture: Some(Arc::new(FakeCapture {
            fail: Some(Failure::Failed("KWin is not running (org.kde.KWin)".into())),
            ..Default::default()
        })),
        ..empty
    };
    assert_eq!(
        text(&failing, "see_screen", json!({})).await,
        ("KWin is not running (org.kde.KWin)".into(), true)
    );
}

/// An image in a reply is a flagged tool's (`ToolSpec::returns_images`, which `delegate.screenshots`
/// filters by): every read tool that can be called without arguments is run over fakes, and the
/// flagged ones do answer with an image.
#[tokio::test]
async fn only_flagged_tools_answer_with_images() {
    let s = Services {
        capture: Some(Arc::new(FakeCapture::default())),
        ..all_services(windows())
    };
    let mut flagged = 0;
    for spec in kai_desktop::contract::read::AREA.tools {
        let Ok(args) = Args::read(spec, None) else {
            continue;
        };
        let r = run(&s, spec.name, &args).await;
        let has_image = r.blocks.iter().any(|b| matches!(b, Block::Image { .. }));
        assert_eq!(has_image, spec.returns_images, "{}", spec.name);
        flagged += usize::from(has_image);
    }
    assert_eq!(flagged, 2, "see_window and see_screen");
}
