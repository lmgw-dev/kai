//! The desktop's overview and the windows (WP14, K24): `desktop_context`, `window_list` and the
//! window actions. Served by Kai's desktop module (`src/desktop/`), listed only where the desktop
//! has the capability they use (K23): the window tools need Windows; `desktop_context` is listed
//! when any of its parts (Windows, Session, Notify, Media) is served, and says what it can.
//!
//! `window` is optional everywhere (K22, [`super::WINDOW`]): the active window by default or as
//! `active`, else a window id from `window_list`, an app name or a fragment of the title, which Kai
//! resolves; an ambiguous one is an error listing the candidates.

use super::{Area, Hints, Kind, Param, ToolSpec, WINDOW};

const DESKTOP: Param =
    Param::new("desktop", Kind::Text).described("a virtual desktop's name or number");
const OUTPUT: Param = Param::new("output", Kind::Text).described("an output's name");
const SIDE: Param = Param::new("side", Kind::Text).described(
    "left, right, top, bottom, top-left, top-right, bottom-left, bottom-right, or none to untile",
);

/// A window action: changes state, undone by another action, nothing outside the desktop.
const ACTION: ToolSpec = ToolSpec {
    params: &[WINDOW],
    hints: Hints {
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ..ToolSpec::CHANGE
};

/// A toggle: a second call undoes the first.
const TOGGLE: ToolSpec = ToolSpec {
    hints: Hints {
        idempotent: false,
        ..ACTION.hints
    },
    ..ACTION
};

pub const AREA: Area = Area {
    name: "windows",
    tools: &[
        ToolSpec {
            name: "desktop_context",
            description: "What is on the desktop now: the active window, outputs, virtual desktop, \
                          lock and idle, do not disturb, media playing, battery.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "window_list",
            description: "List the open windows, topmost first: id, app, title and state.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "window_activate",
            description: "Bring a window to the front and focus it.",
            ..ACTION
        },
        ToolSpec {
            name: "window_close",
            description: "Close a window, as its close button does (the app may ask to save).",
            // Closing can lose work the app does not ask about (a terminal, a crashed saver).
            hints: Hints {
                destructive: true,
                ..ACTION.hints
            },
            ..ACTION
        },
        ToolSpec {
            name: "window_minimize",
            description: "Minimize a window.",
            ..ACTION
        },
        ToolSpec {
            name: "window_maximize",
            description: "Maximize a window, or restore it if it is maximized.",
            ..TOGGLE
        },
        ToolSpec {
            name: "window_fullscreen",
            description: "Put a window into fullscreen, or take it out if it is fullscreen.",
            ..TOGGLE
        },
        ToolSpec {
            name: "window_to_desktop",
            description: "Move a window to another virtual desktop.",
            params: &[WINDOW, DESKTOP],
            ..ACTION
        },
        ToolSpec {
            name: "window_to_output",
            description: "Move a window to another output (monitor).",
            params: &[WINDOW, OUTPUT],
            ..ACTION
        },
        ToolSpec {
            name: "window_tile",
            description: "Tile a window to a side or corner of its output, or untile it.",
            params: &[WINDOW, SIDE],
            ..ACTION
        },
    ],
};

/// The tools that need the Windows capability: every one but `desktop_context`.
pub fn needs_windows(name: &str) -> bool {
    name != "desktop_context"
}
