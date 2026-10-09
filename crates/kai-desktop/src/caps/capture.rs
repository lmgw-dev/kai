//! The Capture capability (K23, §11.1's vision): `see_window` and `see_screen` (WP15). On Plasma it
//! is KWin's ScreenShot2 (`src/desktop/plasma/capture.rs`).
//!
//! A capture comes back as plain pixels at the compositor's real resolution (an output's physical
//! pixels, not its scaled logical size); Kai encodes them for the model. Nothing is scaled down: Kai
//! does not know which model reads the image (lmgw sends it by the answering alias's capability,
//! egress §8.2).

use super::Fut;

/// Pixels, row by row from the top, 8-bit red, green, blue and alpha each, alpha not
/// premultiplied.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

impl Pixels {
    /// Every pixel is opaque (a screen is; a window may have rounded corners).
    pub fn opaque(&self) -> bool {
        self.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
    }
}

pub trait Capture: Send + Sync {
    /// The window `id` (a `Window::id` from a snapshot), or the active window for `None`: its
    /// content, without the title bar or shadow.
    fn window<'a>(&'a self, id: Option<&'a str>) -> Fut<'a, Pixels>;

    /// The output of this name (its connector name), or the active output for `None`, with the
    /// pointer drawn in.
    fn screen<'a>(&'a self, output: Option<&'a str>) -> Fut<'a, Pixels>;
}
