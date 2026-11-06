//! `see_screen` under Kai's site rules (the parent's module doc): what to blank, by the windows'
//! frames before and after the capture, and the compositor's effects that refuse a capture.
//!
//! - **The effects** ([`moving`]): KWin draws a window away from its frame while an effect such
//!   as the Overview, the desktop grid, a desktop switch's slide, a minimize, maximize or close
//!   animation, wobbly windows or the zoom runs, and shows thumbnails of windows in some. While
//!   some site is `none`, `see_screen` reads the running effects before the capture and again
//!   after it and does not show the capture while one runs that is not known to leave every
//!   window where its frame is ([`STILL`]); the answer names it. An effect Kai does not know
//!   counts as moving.

use kai_desktop::caps::capture::Pixels;
use kai_desktop::caps::windows::{Output, Window};

use super::{Services, rules, unsettled_after};
use crate::browser::shown::{GAP, Reach, window_name};
use crate::browser::sites::Rule;
use crate::browser::tools::see::{GREY, Rect, fill_row, pixel_boxes};

/// A frame in the compositor's logical coordinates: x, y, width, height.
pub type Frame = (i32, i32, u32, u32);

/// KWin's effects that leave every window where its frame is and draw no window's picture
/// elsewhere (module doc): blurs and tints behind or over windows, the cursor's and the pointer's
/// marks, popups' and tooltips' own animations, a dimmed screen, a frozen app's grey, the
/// screenshot effect that serves Kai's own captures.
pub const STILL: &[&str] = &[
    "backgroundcontrast",
    "blur",
    "colorblindnesscorrection",
    "colorpicker",
    "contrast",
    "desktopchangeosd",
    "dialogparent",
    "dimscreen",
    "fadingpopups",
    "frozenapp",
    "hidecursor",
    "invert",
    "morphingpopups",
    "mouseclick",
    "mousemark",
    "outputlocator",
    "screenedge",
    "screenshot",
    "shakecursor",
    "showfps",
    "slidingpopups",
    "snaphelper",
    "startupfeedback",
    "systembell",
    "touchpoints",
    "trackmouse",
];

/// The running effects of `effects` that may draw a window away from its frame (module doc).
pub fn moving(effects: &[String]) -> Vec<&str> {
    effects
        .iter()
        .map(String::as_str)
        .filter(|e| !STILL.contains(e))
        .collect()
}

/// What `see_screen` blanks of one look at the desktop ([`blanks`]).
#[derive(Debug, Default, Clone)]
pub struct Blanks {
    /// The windows it blanks, by id, with their frames.
    pub frames: Vec<(String, Frame)>,
    /// The Firefox windows it lets through, by their frames (`None`: where the desktop did not
    /// say): the answer says what the check does not cover where one is on the capture.
    pub passed: Vec<Option<Frame>>,
    /// Tabs in front activated after this time count as not settled (`browser.see_settle_ms`
    /// before the capture's first look).
    pub unsettled_after: i64,
}

/// What `see_screen` must blank now (module doc); `Ok(None)` while no site is `none`, with no call
/// made. `first`: the look before the capture, for the look after it (the same settling time).
pub async fn blanks(services: &Services, first: Option<&Blanks>) -> Result<Option<Blanks>, String> {
    let Some((shown, sites)) = rules(services, Reach::See) else {
        return Ok(None);
    };
    let windows = services.windows.as_ref().ok_or(
        "Kai's site rules keep some sites from Kai, and this desktop offers Kai no window list, \
         so Kai cannot tell where Firefox's windows are; see_screen did not run",
    )?;
    let effects = windows.effects().await.map_err(|e| {
        format!(
            "Kai's site rules keep some sites from Kai, and Kai cannot tell whether the desktop \
             draws windows away from where they are ({e}); see_screen did not run"
        )
    })?;
    let running = moving(&effects);
    if !running.is_empty() {
        let names: Vec<String> = running
            .iter()
            .map(|e| crate::host::confirm::shown_text(e).into_owned())
            .collect();
        let word = if names.len() == 1 {
            "effect"
        } else {
            "effects"
        };
        return Err(format!(
            "the desktop runs the {word} {}, which may draw windows away from where Kai blanks \
             them, and Kai's site rules keep some sites from Kai; see_screen did not run (it can \
             once the desktop is still)",
            names.join(", ")
        ));
    }
    let placement = windows.placed().await.map_err(|e| {
        format!(
            "Kai's site rules keep some sites from Kai, and Kai cannot tell where Firefox's \
             windows are ({e}); see_screen did not run"
        )
    })?;
    let drawn: Vec<_> = placement
        .windows
        .iter()
        .filter(|p| placement.drawn(&p.window))
        .collect();
    let all: Vec<&Window> = placement.windows.iter().map(|p| &p.window).collect();
    let after = first.map_or_else(|| unsettled_after(shown), |b| b.unsettled_after);
    let look = shown.look(sites, &all, Some(after)).await;
    let mut out = Blanks {
        unsettled_after: after,
        ..Blanks::default()
    };
    for p in drawn {
        match (look.rule(&p.window), p.frame) {
            (None, _) => {}
            (Some(Rule::None), Some(f)) => out.frames.push((p.window.id.clone(), f)),
            (Some(Rule::None), None) => {
                return Err(format!(
                    "{} shows a site Kai does not reach, or one Kai cannot tell (Kai's site \
                     rules), and the desktop does not say where it is, so Kai cannot blank it; \
                     see_screen did not run",
                    window_name(&p.window)
                ));
            }
            (Some(_), f) => out.passed.push(f),
        }
    }
    Ok(Some(out))
}

/// `frame` on a `w` by `h` capture of `output` (image pixels), or why it cannot be placed.
fn on_capture(frame: Frame, output: &Output, w: u32, h: u32) -> Result<Rect, String> {
    if output.width == 0 || output.height == 0 {
        return Err(format!(
            "the desktop names no size for output {}, so Kai cannot tell where Firefox's windows \
             are on its capture (Kai's site rules); see_screen did not show it",
            crate::host::confirm::shown_text(&output.name)
        ));
    }
    let (sx, sy) = (
        f64::from(w) / f64::from(output.width),
        f64::from(h) / f64::from(output.height),
    );
    Ok(Rect {
        x: f64::from(frame.0 - output.x) * sx,
        y: f64::from(frame.1 - output.y) * sy,
        width: f64::from(frame.2) * sx,
        height: f64::from(frame.3) * sy,
    })
}

/// A frame covers some pixel of a `w` by `h` capture.
fn covers(r: &Rect, w: u32, h: u32) -> bool {
    pixel_boxes(std::slice::from_ref(r), w, h)
        .first()
        .is_some_and(|&(x0, y0, x1, y1)| x0 < x1 && y0 < y1)
}

/// What [`blank`] did: how many windows it blanked on the capture, and whether a Firefox window it
/// let through is on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Blanked {
    pub windows: usize,
    pub passed: bool,
}

/// `pixels`, a capture of `output`, with every frame of `looks` filled grey (module doc): the
/// union of the looks before and after the capture.
pub fn blank(pixels: &mut Pixels, output: &Output, looks: &[Blanks]) -> Result<Blanked, String> {
    let (w, h) = (pixels.width, pixels.height);
    let mut rects = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for (id, frame) in looks.iter().flat_map(|b| &b.frames) {
        let r = on_capture(*frame, output, w, h)?;
        if covers(&r, w, h) {
            if !ids.contains(&id.as_str()) {
                ids.push(id);
            }
            rects.push(r);
        }
    }
    let mut passed = false;
    for f in looks.iter().flat_map(|b| &b.passed) {
        passed |= match f {
            Some(f) => covers(&on_capture(*f, output, w, h)?, w, h),
            None => true,
        };
    }
    if !rects.is_empty() {
        let boxes = pixel_boxes(&rects, w, h);
        let row = w as usize * 4;
        let fill = [GREY, GREY, GREY, 0xFF];
        if row > 0 {
            for (y, line) in pixels.rgba.chunks_exact_mut(row).enumerate() {
                fill_row(line, y, &boxes, &fill);
            }
        }
    }
    Ok(Blanked {
        windows: ids.len(),
        passed,
    })
}

/// The lines `see_screen` adds for what [`blank`] did.
pub fn screen_notes(b: &Blanked) -> Vec<String> {
    let mut out = Vec::new();
    match b.windows {
        0 => {}
        1 => out.push(
            "1 Firefox window that shows a site Kai does not reach, or one Kai cannot tell, is \
             blanked out grey with whatever lies over it (Kai's site rules)"
                .to_owned(),
        ),
        n => out.push(format!(
            "{n} Firefox windows that show a site Kai does not reach, or one Kai cannot tell, are \
             blanked out grey with whatever lies over them (Kai's site rules)"
        )),
    }
    if b.passed {
        out.push(GAP.to_owned());
    }
    out
}
