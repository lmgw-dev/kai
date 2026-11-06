//! The Windows capability (K23, K24): the windows, the outputs and the virtual desktops as one
//! snapshot, and the window actions. On Plasma it is KWin's (`src/desktop/plasma/windows.rs`).

use super::Fut;

/// One window. `id` is the compositor's stable handle (on Plasma the window management uuid without
/// braces, which KWin scripting's `internalId` and ScreenShot2's `CaptureWindow` take too).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    pub id: String,
    /// The app id: the desktop file's name where the app has one, else its window class.
    pub app: String,
    pub title: String,
    pub pid: u32,
    /// The output it is on (its connector name); empty when the compositor names none.
    pub output: String,
    /// The virtual desktops it is on by name; empty: all of them.
    pub desktops: Vec<String>,
    pub active: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// One output, in the compositor's logical coordinates.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Output {
    /// The connector name (`DP-1`).
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The scale factor (1.25 for 125 %).
    pub scale: f64,
    /// The compositor's active output (the focused window's, or the pointer's).
    pub active: bool,
}

/// Everything [`Windows::snapshot`] reads in one go, so the parts agree with each other.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// Topmost first.
    pub windows: Vec<Window>,
    pub outputs: Vec<Output>,
    /// The virtual desktops' names, in their order.
    pub desktops: Vec<String>,
    /// The current virtual desktop's name.
    pub desktop: String,
    /// The current activity's name; empty when the desktop has none or did not say.
    pub activity: String,
}

/// A window and where the compositor draws it ([`Windows::placed`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placed {
    /// Its facts as a snapshot names them (`output` may be empty, `active` unset).
    pub window: Window,
    /// Its frame (title bar and borders included, its shadow not) in the compositor's logical
    /// coordinates, as [`Output`]'s are: x, y, width, height. `None` where the compositor has not
    /// said it.
    pub frame: Option<(i32, i32, u32, u32)>,
}

/// Every window the compositor draws, and where ([`Windows::placed`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placement {
    /// Topmost first, the windows a task bar leaves out included (Firefox's Picture-in-Picture).
    pub windows: Vec<Placed>,
    /// The current virtual desktop's name: a window on others only is not drawn.
    pub desktop: String,
}

/// `w` may be on the screen now while `desktop` is the current virtual desktop: not minimized, and
/// on every desktop or on the current one (or the current one has no name). Activities are not
/// told apart, so a window of another activity counts as drawn.
pub fn drawn_on(desktop: &str, w: &Window) -> bool {
    !w.minimized
        && (w.desktops.is_empty() || desktop.is_empty() || w.desktops.iter().any(|d| d == desktop))
}

impl Placement {
    /// `w` may be on the screen now ([`drawn_on`]).
    pub fn drawn(&self, w: &Window) -> bool {
        drawn_on(&self.desktop, w)
    }
}

impl Snapshot {
    /// `w` may be on the screen now ([`drawn_on`]).
    pub fn drawn(&self, w: &Window) -> bool {
        drawn_on(&self.desktop, w)
    }
}

/// A quick-tile position (`window_tile`'s `side`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Untiled: back to the size it had before.
    None,
}

impl Side {
    pub const ALL: [Side; 9] = [
        Side::Left,
        Side::Right,
        Side::Top,
        Side::Bottom,
        Side::TopLeft,
        Side::TopRight,
        Side::BottomLeft,
        Side::BottomRight,
        Side::None,
    ];

    /// Its word in `window_tile`'s `side`.
    pub fn key(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
            Side::TopLeft => "top-left",
            Side::TopRight => "top-right",
            Side::BottomLeft => "bottom-left",
            Side::BottomRight => "bottom-right",
            Side::None => "none",
        }
    }

    /// `side` as a model may write it: case and `-`, `_` or a space between the words do not
    /// matter.
    pub fn parse(side: &str) -> Option<Side> {
        let norm: String = side
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c == '_' || c == ' ' { '-' } else { c })
            .collect();
        Side::ALL.into_iter().find(|s| s.key() == norm)
    }
}

/// One window action (`window_activate` … `window_tile`). Where it names a desktop or an output,
/// it is one from a snapshot taken just before (Kai resolves it there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Activate,
    Close,
    Minimize,
    /// Maximized if it is not, restored if it is (the title bar's button).
    ToggleMaximize,
    /// Into fullscreen if it is not, out of it if it is.
    ToggleFullscreen,
    /// Onto this virtual desktop only: its position in [`Snapshot::desktops`] (from 0), so two
    /// desktops of one name stay apart.
    ToDesktop(usize),
    /// Onto the output of this name.
    ToOutput(String),
    Tile(Side),
}

/// What an action did, in a word the result line uses ("maximized", "restored").
pub type Done = String;

/// The Windows capability. `window_list`, `desktop_context`'s window part and the window actions
/// (WP14).
pub trait Windows: Send + Sync {
    /// The windows, outputs and desktops now.
    fn snapshot(&self) -> Fut<'_, Snapshot>;

    /// The window that has the keyboard focus now, any window (a task bar's or not; `None`: no
    /// window has it). The input tools ask it before every event they send (WP16c), so a provider
    /// answers it more cheaply than a snapshot where it can; this default reads a snapshot. Only
    /// `id`, `app`, `title`, `pid` and `active` need to be set.
    fn active(&self) -> Fut<'_, Option<Window>> {
        Box::pin(async move {
            Ok(self
                .snapshot()
                .await?
                .windows
                .into_iter()
                .find(|w| w.active))
        })
    }

    /// Runs `action` on the window `id` (a [`Window::id`] from a snapshot). A window that closed
    /// meanwhile is [`super::Failure::Refused`].
    fn act<'a>(&'a self, id: &'a str, action: &'a Action) -> Fut<'a, Done>;

    /// Every window the compositor draws with its frame, those a task bar leaves out included:
    /// `see_screen` blanks by it what Kai's browser site rules keep from Kai (Kai's
    /// `crate::browser::shown`). A provider that cannot say where its windows are answers
    /// [`super::Failure::Refused`] (this default).
    fn placed(&self) -> Fut<'_, Placement> {
        Box::pin(async {
            Err(super::Failure::Refused(
                "the desktop does not say where its windows are".into(),
            ))
        })
    }

    /// The compositor's effects that are running now, by name (on Plasma KWin's `activeEffects`:
    /// an effect counts while it paints, such as the Overview or a window's minimize animation),
    /// so `see_screen` can tell that windows may be drawn away from their frames. A provider that
    /// cannot say answers [`super::Failure::Refused`] (this default).
    fn effects(&self) -> Fut<'_, Vec<String>> {
        Box::pin(async {
            Err(super::Failure::Refused(
                "the desktop does not say which of its effects are running".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_parse_as_a_model_writes_them() {
        assert_eq!(Side::parse("Top Left"), Some(Side::TopLeft));
        assert_eq!(Side::parse("bottom_right"), Some(Side::BottomRight));
        assert_eq!(Side::parse(" LEFT "), Some(Side::Left));
        assert_eq!(Side::parse("none"), Some(Side::None));
        assert_eq!(Side::parse("middle"), None);
        for s in Side::ALL {
            assert_eq!(Side::parse(s.key()), Some(s));
        }
    }
}
