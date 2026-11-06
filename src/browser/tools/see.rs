//! The screenshots (kai-firefox design §5.3): `page_see` (the tab's viewport as Firefox draws it)
//! and `element_see` (cropped to one element's part in view), over `page.see@1` and
//! `element.see@1`, as a line and an MCP image block (PNG, base64), like the desktop's
//! `see_window`. Both are `ToolSpec::returns_images`: K25's setting of who looks at screenshots
//! applies to them (the provider is listed through `crate::delegate::screenshots::HostView`).
//!
//! - **The line** says the image's size, the CSS pixels it covers and the scale between them as
//!   measured from the image (never the device pixel ratio), an element's box, whether it was
//!   scrolled into view first and whether part of it is cut off.
//! - **Site rules:** a page of a site Kai does not reach is never captured (the preparation), and
//!   one that went to such a site while the call ran is not shown; a tab whose page changed during
//!   the capture answers `page_changed`, never an image. A child frame of such a site, by its own
//!   origin, is blanked out of the image before it leaves Kai: the extension names each frame's
//!   visible part in image pixels (`frames`, a tree, in the crop's pixels for `element_see`), and
//!   the origin of the document it showed before the capture where that was another
//!   (`origin_before`, kai-firefox WP7's second review): a frame either of whose origins is such a
//!   site, or opaque while some site is `none` (the strictest rule there is), is blanked, and so
//!   is the box of a frame whose place its parent could not say (`unmeasured`: the part of the
//!   nearest measured frame around it). Kai decodes the PNG row by row, fills those boxes grey
//!   and encodes each row again as it goes, keeping the image's colour space (`sRGB`, `iCCP`,
//!   `gAMA`, `cHRM`); the image's size must be the one the answer says, which the boxes are
//!   measured against. Where the extension does not say where the frames are and some site is
//!   `none`, Kai cannot tell what the image holds and does not show it; where it does not say
//!   where such a frame is, the same.
//! - What Kai draws (hint labels, the ring) is hidden for the capture; frames that did not answer
//!   the hiding in time are counted (`not_hidden`), since Kai's labels may show there.

use std::sync::Arc;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::lines::{Element, element, number};
use super::{Inner, checked, failure, read, send, target};
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Tab, head, or_default};
use crate::host::registry::{Block, Call, Reply};
use crate::voice::link::Pending;

pub const PAGE: &str = "page.see@1";
pub const ELEMENT: &str = "element.see@1";

/// What blanks a frame out: a mid grey, opaque (also a window in the desktop's `see_screen`).
pub const GREY: u8 = 0x80;

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default)]
struct Size {
    width: f64,
    height: f64,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default)]
struct Scale {
    x: f64,
    y: f64,
}

/// A box: in image pixels for a frame (`frames`), in the view's CSS pixels for an element.
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// `page.see@1`'s answer, and `element.see@1`'s with its element.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Shot {
    tab: Tab,
    #[serde(deserialize_with = "or_default")]
    png: String,
    width: u64,
    height: u64,
    view: Size,
    scale: Scale,
    not_hidden: Vec<Value>,
    #[serde(flatten)]
    element: Element,
    #[serde(rename = "box")]
    shown: Option<Rect>,
    scrolled: bool,
    clipped: bool,
}

/// Where Kai cannot tell what an image holds of the frames of sites it does not reach.
const UNTOLD: &str = "Kai for Firefox did not say where the page's frames are in the image, and \
                      Kai's site rules keep some sites from Kai, so Kai does not show it (an \
                      update of the extension says where they are)";

/// The boxes (image pixels) of the frames of `none` sites in an answer (module doc), or why Kai
/// cannot tell what the image holds.
pub fn unreached(answer: &Value, sites: &SiteRules) -> Result<Vec<Rect>, String> {
    let mut out = Vec::new();
    match answer.get("frames") {
        None | Some(Value::Null) if sites.strictest() == Rule::None => Err(UNTOLD.to_owned()),
        None | Some(Value::Null) => Ok(out),
        Some(frames) => {
            frames_walk(frames, sites, &mut out)?;
            Ok(out)
        }
    }
}

fn frames_walk(frames: &Value, sites: &SiteRules, out: &mut Vec<Rect>) -> Result<(), String> {
    let Some(list) = frames.as_array() else {
        return Err(UNTOLD.to_owned());
    };
    for f in list {
        let origin = f.get("origin").and_then(Value::as_str);
        let url = f
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| !u.trim().is_empty());
        // The image shows the document before or the one after: the stricter rule of the two (an
        // opaque `origin_before`, `null`, the strictest there is).
        let before = f
            .get("origin_before")
            .map_or(Rule::Act, |b| sites.frame_rule(b.as_str()));
        let rule = sites
            .frame_rule(origin)
            .max(url.map_or(Rule::Act, |u| sites.rule(u)))
            .max(before);
        if rule == Rule::None {
            match f.get("box") {
                // Nothing of it in view.
                Some(Value::Null) => {}
                Some(b) => out.push(
                    serde_json::from_value::<Rect>(b.clone()).map_err(|_| UNTOLD.to_owned())?,
                ),
                None => return Err(UNTOLD.to_owned()),
            }
        }
        if let Some(children) = f.get("frames").filter(|c| !c.is_null()) {
            frames_walk(children, sites, out)?;
        }
    }
    Ok(())
}

/// `png` (a PNG file's bytes, `size` pixels as the answer says) with each box of `rects` (image
/// pixels, widened to whole pixels and cut to the image) filled [`GREY`], encoded again (module
/// doc): row by row, so what Kai holds is a row of the image, not the image; an interlaced image,
/// whose rows come in seven passes, is held whole.
pub fn blank(png_bytes: &[u8], size: (u64, u64), rects: &[Rect]) -> Result<Vec<u8>, String> {
    let failed = |e: &dyn std::fmt::Display| {
        format!("Kai could not blank the frames out of the image ({e}), so it does not show it")
    };
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    // No guessed cap of the decoder's: what Kai holds is a row (an interlaced image whole), sized
    // from the header, which must name the size the answer does.
    dec.set_limits(png::Limits { bytes: usize::MAX });
    let mut reader = dec.read_info().map_err(|e| failed(&e))?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    if (u64::from(w), u64::from(h)) != size {
        return Err(format!(
            "Kai for Firefox named its image {}x{} pixels, but the image is {w}x{h}, so Kai cannot \
             tell where the frames it blanks are, and does not show it",
            size.0, size.1
        ));
    }
    let mut out_info = png::Info::with_size(w, h);
    out_info.srgb = info.srgb;
    out_info.icc_profile = info.icc_profile.clone();
    out_info.source_gamma = info.gama_chunk;
    out_info.source_chromaticities = info.chrm_chunk;
    let interlaced = info.interlaced;
    let (color, _) = reader.output_color_type();
    out_info.color_type = color;
    out_info.bit_depth = png::BitDepth::Eight;
    // After the expansion an image is grey or RGB, with or without alpha (an indexed one is RGB).
    let fill: &[u8] = match color.samples() {
        1 => &[GREY],
        2 => &[GREY, 0xFF],
        3 => &[GREY, GREY, GREY],
        _ => &[GREY, GREY, GREY, 0xFF],
    };
    let line = reader
        .output_line_size(w)
        .ok_or_else(|| failed(&"a row of it does not fit in memory"))?;
    let boxes = pixel_boxes(rects, w, h);
    let grey = |row: &mut [u8], y: usize| fill_row(row, y, &boxes, fill);
    let mut out = Vec::new();
    let enc = png::Encoder::with_info(&mut out, out_info).map_err(|e| failed(&e))?;
    let mut writer = enc.write_header().map_err(|e| failed(&e))?;
    let mut stream = writer.stream_writer().map_err(|e| failed(&e))?;
    let held = |n: usize| {
        let mut buf = Vec::new();
        buf.try_reserve_exact(n).map_err(|_| {
            failed(&format!(
                "the {n} bytes it needs in memory could not be had"
            ))
        })?;
        buf.resize(n, 0);
        Ok::<_, String>(buf)
    };
    if interlaced {
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| failed(&"it does not fit in memory"))?;
        let mut buf = held(size)?;
        reader.next_frame(&mut buf).map_err(|e| failed(&e))?;
        for (y, row) in buf.chunks_exact_mut(line).enumerate() {
            grey(row, y);
        }
        std::io::Write::write_all(&mut stream, &buf).map_err(|e| failed(&e))?;
    } else {
        let mut row = held(line)?;
        let mut y = 0;
        while reader.read_row(&mut row).map_err(|e| failed(&e))?.is_some() {
            grey(&mut row, y);
            std::io::Write::write_all(&mut stream, &row).map_err(|e| failed(&e))?;
            y += 1;
        }
    }
    stream.finish().map_err(|e| failed(&e))?;
    writer.finish().map_err(|e| failed(&e))?;
    Ok(out)
}

/// A box of whole pixels: its left, top, right and bottom edges (the right and bottom ones
/// excluded).
pub type PixelBox = (usize, usize, usize, usize);

/// `rects` (image pixels) widened to whole pixels and cut to a `w` by `h` image.
pub fn pixel_boxes(rects: &[Rect], w: u32, h: u32) -> Vec<PixelBox> {
    let (fw, fh) = (f64::from(w), f64::from(h));
    rects
        .iter()
        .map(|r| {
            let (x0, y0) = (
                r.x.floor().clamp(0.0, fw) as usize,
                r.y.floor().clamp(0.0, fh) as usize,
            );
            (
                x0,
                y0,
                ((r.x + r.width).ceil().clamp(0.0, fw) as usize).max(x0),
                ((r.y + r.height).ceil().clamp(0.0, fh) as usize).max(y0),
            )
        })
        .collect()
}

/// Row `y` of an image of `fill.len()` samples a pixel, with the part of each box of `boxes` in
/// it set to `fill` (also the desktop's `see_screen`, `crate::desktop::read::sites`).
pub fn fill_row(row: &mut [u8], y: usize, boxes: &[PixelBox], fill: &[u8]) {
    let channels = fill.len();
    for &(x0, y0, x1, y1) in boxes {
        if (y0..y1).contains(&y) {
            for px in row[x0 * channels..x1 * channels].chunks_exact_mut(channels) {
                px.copy_from_slice(fill);
            }
        }
    }
}

/// The scale as the line says it: image pixels per CSS pixel.
fn scale(s: Scale) -> String {
    if s.x == s.y {
        format!("scale {}: image pixels per CSS pixel", number(s.x))
    } else {
        format!(
            "scale {} across, {} down: image pixels per CSS pixel",
            number(s.x),
            number(s.y)
        )
    }
}

/// The lines after the first: the frames blanked out, Kai's drawings that may show.
fn notes(blanked: usize, not_hidden: usize) -> Vec<String> {
    let mut out = Vec::new();
    match blanked {
        0 => {}
        1 => out.push("1 frame of a site Kai does not reach is blanked out grey".to_owned()),
        n => out.push(format!(
            "{n} frames of sites Kai does not reach are blanked out grey"
        )),
    }
    match not_hidden {
        0 => {}
        n => out.push(format!(
            "Kai's hint labels or ring may show in {} that did not answer in time to hide \
             them (Kai for Firefox's setting content_ms)",
            if n == 1 {
                "1 frame".to_owned()
            } else {
                format!("{n} frames")
            }
        )),
    }
    out
}

/// Sends `op` for the prepared tab or element and makes the reply: `line` words the answer.
fn see(
    inner: Arc<Inner>,
    call: Call,
    op: &'static str,
    line: fn(&Shot, &SiteRules, Option<i64>) -> String,
) -> Pending<Reply> {
    Box::pin(async move {
        let r = async {
            let t = target(&call)?;
            let v = match send(&inner, &call, &t, op, Map::new()).await {
                Ok(v) => v,
                Err(e) => {
                    return Err(
                        failure(&inner, &t.link, e, t.element.as_ref(), &call.cancel).await,
                    );
                }
            };
            let sites = inner.browser().sites;
            checked(&call, &v, &sites)?;
            let boxes = unreached(&v, &sites)?;
            let shot: Shot = read(op, v)?;
            if shot.png.trim().is_empty() {
                return Err(format!(
                    "Kai for Firefox answered {op} without an image (an update of the extension \
                     may be needed)"
                ));
            }
            let base64 = if boxes.is_empty() {
                shot.png.clone()
            } else {
                let png = shot.png.clone();
                let boxes = boxes.clone();
                let size = (shot.width, shot.height);
                tokio::task::spawn_blocking(move || {
                    let engine = base64::engine::general_purpose::STANDARD;
                    let bytes = engine.decode(png.as_bytes()).map_err(|e| {
                        format!("Kai for Firefox sent an image Kai cannot read ({e})")
                    })?;
                    blank(&bytes, size, &boxes).map(|b| engine.encode(b))
                })
                .await
                .map_err(|e| format!("Kai could not blank the frames out of the image: {e}"))??
            };
            let mut text = vec![line(&shot, &sites, t.element.map(|a| a.kai))];
            text.extend(notes(boxes.len(), shot.not_hidden.len()));
            Ok(Reply {
                blocks: vec![
                    Block::Text(text.join("\n")),
                    Block::Image {
                        mime: "image/png".into(),
                        base64,
                    },
                ],
                error: false,
            })
        }
        .await;
        r.unwrap_or_else(Reply::error)
    })
}

/// `page_see`.
pub fn page_see(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    see(inner, call, PAGE, |s, sites, _| {
        format!(
            "{}: the visible part, {}x{} pixels for {}x{} CSS pixels ({})",
            head(&s.tab, sites),
            s.width,
            s.height,
            number(s.view.width),
            number(s.view.height),
            scale(s.scale)
        )
    })
}

/// `element_see`: the element as Kai numbers it.
pub fn element_see(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    see(inner, call, ELEMENT, |s, sites, kai| {
        let mut e = s.element.clone();
        if let Some(n) = kai {
            e.id = n;
        }
        let mut line = format!(
            "{}: {}, {}x{} pixels ({})",
            head(&s.tab, sites),
            element(&e),
            s.width,
            s.height,
            scale(s.scale)
        );
        if let Some(b) = s.shown {
            line.push_str(&format!(
                "; its part in view at {},{}, {}x{} CSS pixels of the {}x{} view",
                number(b.x),
                number(b.y),
                number(b.width),
                number(b.height),
                number(s.view.width),
                number(s.view.height)
            ));
        }
        if s.scrolled {
            line.push_str("; scrolled into view first");
        }
        if s.clipped {
            line.push_str(
                "; part of it is outside the view or cut off, so the image shows the part in view",
            );
        }
        line
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::browser::settings::Sites;

    /// A PNG of `w` by `h` pixels of `color` and `depth`, its samples `data` (a palette for an
    /// indexed one), `info` as it writes it.
    fn png_of(
        w: u32,
        h: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        data: &[u8],
        with: impl FnOnce(&mut png::Info<'static>),
    ) -> Vec<u8> {
        let mut info = png::Info::with_size(w, h);
        info.color_type = color;
        info.bit_depth = depth;
        with(&mut info);
        let mut out = Vec::new();
        let enc = png::Encoder::with_info(&mut out, info).unwrap();
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(data).unwrap();
        wr.finish().unwrap();
        out
    }

    fn rgba(w: u32, h: u32) -> Vec<u8> {
        png_of(
            w,
            h,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &vec![0xFF; (w * h * 4) as usize],
            |_| {},
        )
    }

    /// The decoded image (8 bits per sample) and its samples per pixel, and its header.
    fn pixels(png_bytes: &[u8]) -> (Vec<u8>, usize, png::Info<'static>) {
        let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (buf, info.color_type.samples(), r.info().clone())
    }

    /// The first sample of each pixel, row by row.
    fn firsts(px: &[u8], n: usize) -> Vec<u8> {
        px.iter().step_by(n).copied().collect()
    }

    /// A 2x2 box at 1,1 of a 4x4 image: pixels (1,1), (2,1), (1,2), (2,2).
    const BOX: Rect = Rect {
        x: 1.0,
        y: 1.0,
        width: 2.0,
        height: 2.0,
    };
    const W: u8 = 0xFF;
    const G: u8 = GREY;
    const BOXED: [u8; 16] = [W, W, W, W, W, G, G, W, W, G, G, W, W, W, W, W];

    #[test]
    fn a_frame_of_a_none_site_is_blanked_out_of_the_image() {
        let png = rgba(4, 4);
        let out = blank(
            &png,
            (4, 4),
            &[Rect {
                x: 0.5,
                y: 1.0,
                width: 1.2,
                height: 1.0,
            }],
        )
        .unwrap();
        let (px, n, info) = pixels(&out);
        assert_eq!((info.width, info.height, n), (4, 4, 4));
        let at = |x: usize, y: usize| &px[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        // x 0.5..1.7 widens to pixels 0 and 1, row 1 only.
        assert_eq!(at(0, 1), [GREY, GREY, GREY, 0xFF]);
        assert_eq!(at(1, 1), [GREY, GREY, GREY, 0xFF]);
        assert_eq!(at(2, 1), [0xFF; 4]);
        assert_eq!(at(0, 0), [0xFF; 4]);
        assert_eq!(at(0, 2), [0xFF; 4]);
        // A box past the image is cut to it; one turned inside out blanks nothing.
        let out = blank(
            &png,
            (4, 4),
            &[
                Rect {
                    x: 3.0,
                    y: -2.0,
                    width: 9.0,
                    height: 3.0,
                },
                Rect {
                    x: 2.0,
                    y: 2.0,
                    width: -2.0,
                    height: -1.0,
                },
            ],
        )
        .unwrap();
        let (px, _, _) = pixels(&out);
        assert_eq!(&px[3 * 4..4 * 4], [GREY, GREY, GREY, 0xFF]);
        assert_eq!(px.iter().filter(|b| **b == GREY).count(), 3);
        assert!(blank(b"no png", (4, 4), &[]).is_err());
    }

    /// The image must be the size the answer names, which the boxes are measured against.
    #[test]
    fn an_image_of_another_size_than_the_answer_says_is_not_shown() {
        let err = blank(&rgba(4, 4), (5, 4), &[BOX]).unwrap_err();
        assert!(
            err.starts_with("Kai for Firefox named its image 5x4 pixels, but the image is 4x4"),
            "{err}"
        );
    }

    /// RGB, grey, 16-bit, indexed and interlaced images are blanked alike, grey on each sample of
    /// colour, alpha opaque.
    #[test]
    fn every_kind_of_png_is_blanked() {
        use png::{BitDepth, ColorType};
        // RGB, 8 bits.
        let out = blank(
            &png_of(4, 4, ColorType::Rgb, BitDepth::Eight, &[W; 48], |_| {}),
            (4, 4),
            &[BOX],
        )
        .unwrap();
        let (px, n, _) = pixels(&out);
        assert_eq!(n, 3);
        assert_eq!(firsts(&px, 3), BOXED);
        assert_eq!(&px[(4 + 1) * 3..(4 + 2) * 3], [GREY; 3]);
        // Grey, 8 bits.
        let out = blank(
            &png_of(
                4,
                4,
                ColorType::Grayscale,
                BitDepth::Eight,
                &[W; 16],
                |_| {},
            ),
            (4, 4),
            &[BOX],
        )
        .unwrap();
        let (px, n, _) = pixels(&out);
        assert_eq!((n, px.as_slice()), (1, BOXED.as_slice()));
        // Grey with alpha, 16 bits: read as 8.
        let out = blank(
            &png_of(
                4,
                4,
                ColorType::GrayscaleAlpha,
                BitDepth::Sixteen,
                &[0xFF; 64],
                |_| {},
            ),
            (4, 4),
            &[BOX],
        )
        .unwrap();
        let (px, n, info) = pixels(&out);
        assert_eq!((n, info.bit_depth), (2, BitDepth::Eight));
        assert_eq!(firsts(&px, 2), BOXED);
        assert!(px.iter().skip(1).step_by(2).all(|a| *a == 0xFF), "opaque");
        // RGB, 16 bits.
        let out = blank(
            &png_of(4, 4, ColorType::Rgb, BitDepth::Sixteen, &[0xFF; 96], |_| {}),
            (4, 4),
            &[BOX],
        )
        .unwrap();
        let (px, n, _) = pixels(&out);
        assert_eq!((n, firsts(&px, 3)), (3, BOXED.to_vec()));
        // Indexed, 2 bits a pixel: a white palette entry, written out as RGB.
        let out = blank(
            &png_of(4, 4, ColorType::Indexed, BitDepth::Two, &[0; 4], |i| {
                i.palette = Some(vec![0xFF, 0xFF, 0xFF, 0, 0, 0].into())
            }),
            (4, 4),
            &[BOX],
        )
        .unwrap();
        let (px, n, info) = pixels(&out);
        assert_eq!((n, info.color_type), (3, ColorType::Rgb));
        assert_eq!(firsts(&px, 3), BOXED);
        // Interlaced (Adam7), grey: held whole, written out plain.
        let out = blank(&interlaced_grey(4, 4, &[W; 16]), (4, 4), &[BOX]).unwrap();
        let (px, n, info) = pixels(&out);
        assert_eq!((n, info.interlaced), (1, false));
        assert_eq!(px, BOXED);
    }

    /// The image's colour space goes with it: sRGB, or an ICC profile with gamma and
    /// chromaticities.
    #[test]
    fn the_colour_space_is_kept() {
        let srgb = png_of(
            4,
            4,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0xFF; 64],
            |i| i.srgb = Some(png::SrgbRenderingIntent::Perceptual),
        );
        let (_, _, info) = pixels(&blank(&srgb, (4, 4), &[BOX]).unwrap());
        assert_eq!(info.srgb, Some(png::SrgbRenderingIntent::Perceptual));
        let chrm = png::SourceChromaticities::new(
            (0.3127, 0.3290),
            (0.64, 0.33),
            (0.30, 0.60),
            (0.15, 0.06),
        );
        let icc = png_of(
            4,
            4,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0xFF; 64],
            |i| {
                i.icc_profile = Some(b"not a real profile".to_vec().into());
                i.source_gamma = Some(png::ScaledFloat::new(0.5));
                i.source_chromaticities = Some(chrm);
            },
        );
        let (_, _, info) = pixels(&blank(&icc, (4, 4), &[BOX]).unwrap());
        assert_eq!(
            info.icc_profile.as_deref(),
            Some(b"not a real profile".as_slice())
        );
        assert_eq!(info.gama_chunk, Some(png::ScaledFloat::new(0.5)));
        assert_eq!(info.chrm_chunk, Some(chrm));
    }

    /// An interlaced 8-bit grey PNG of `samples` (row by row), written by hand: the png crate
    /// writes no interlaced images. Stored deflate blocks, CRC-32 and Adler-32 computed here.
    fn interlaced_grey(w: u32, h: u32, samples: &[u8]) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        (c >> 1) ^ 0xEDB8_8320
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc32(&body).to_be_bytes());
        }
        // Adam7's passes: first column, first row, column step, row step.
        let mut raw = Vec::new();
        for (x0, y0, dx, dy) in [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ] {
            let xs: Vec<u32> = (x0..w).step_by(dx).collect();
            if xs.is_empty() {
                continue;
            }
            for y in (y0..h).step_by(dy) {
                raw.push(0);
                raw.extend(xs.iter().map(|x| samples[(y * w + x) as usize]));
            }
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        let len = raw.len() as u16;
        let mut zlib = vec![0x78, 0x01, 0x01];
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend(&raw);
        zlib.extend(((b << 16) | a).to_be_bytes());
        let mut ihdr = Vec::new();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 0, 0, 0, 1]);
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn the_hand_written_interlaced_png_reads_as_written() {
        let samples: Vec<u8> = (0..16).map(|i| i * 16).collect();
        let png = interlaced_grey(4, 4, &samples);
        let (px, n, info) = pixels(&png);
        assert!(info.interlaced);
        assert_eq!((n, px), (1, samples));
    }

    #[test]
    fn which_frames_are_blanked_by_their_own_origin() {
        let none = SiteRules::read(&Sites {
            none: vec!["pay.example".into()],
            ..Default::default()
        })
        .unwrap();
        let b = |x: f64| json!({"x": x, "y": 0, "width": 10, "height": 10});
        let v = json!({"frames": [
            {"frame": 1, "origin": "https://consent.example", "url": "https://consent.example/",
             "box": b(1.0),
             "frames": [{"frame": 2, "origin": "https://pay.example", "url": "https://pay.example/",
                         "box": b(2.0)}]},
            {"frame": 3, "origin": null, "url": "about:blank", "box": b(3.0)},
            {"frame": 4, "origin": "https://pay.example", "url": "https://pay.example/",
             "box": null},
            // It showed pay.example when Kai's drawings were hidden: the image may show it.
            {"frame": 5, "origin": "https://ads.example", "url": "https://ads.example/",
             "origin_before": "https://pay.example", "box": b(5.0)},
            // Opaque before: the strictest rule.
            {"frame": 6, "origin": "https://ads.example", "url": "https://ads.example/",
             "origin_before": null, "box": b(6.0)},
            {"frame": 7, "origin": "https://ads.example", "url": "https://ads.example/",
             "origin_before": "https://news.example", "box": b(7.0)},
            // Where its parent could not say: the part of the frame around it.
            {"frame": 8, "origin": "https://pay.example", "url": "https://pay.example/",
             "unmeasured": true, "box": b(8.0)}
        ]});
        let got: Vec<f64> = unreached(&v, &none).unwrap().iter().map(|r| r.x).collect();
        // The opaque frame gets the strictest rule there is: none.
        assert_eq!(got, [2.0, 3.0, 5.0, 6.0, 8.0]);
        // Without a none site, an opaque origin before blanks nothing.
        let read_only = SiteRules::read(&Sites {
            read: vec!["pay.example".into()],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(unreached(&v, &read_only), Ok(vec![]));
        // Without frames: shown only while no site is none.
        assert_eq!(unreached(&json!({}), &SiteRules::default()), Ok(vec![]));
        assert_eq!(unreached(&json!({}), &none), Err(UNTOLD.to_owned()));
        // A none frame without a box Kai can read.
        let v = json!({"frames": [{"frame": 2, "origin": "https://pay.example"}]});
        assert_eq!(unreached(&v, &none), Err(UNTOLD.to_owned()));
    }
}
