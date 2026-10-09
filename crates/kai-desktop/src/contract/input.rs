//! The input tools (WP16c, K24's confirmed-by-default list): `ui_press`, `type_text` and
//! `key_press`. They act as the owner on whatever has the keyboard focus or the element, so K15
//! confirms every one for every caller (`Ask::Always`); the owner's per-tool policy can set any of
//! them to `allow` (ruling 28). Served by Kai's desktop module; each is listed only where its
//! capabilities are served ([`needs`], K23). Typing and keys need Windows too: the confirmation
//! names the window with the keyboard focus, and the call stops when the focus leaves it.
//!
//! - `ui_press {window?, element}`: runs the default action of an element through accessibility,
//!   preferred over pixel input. `element` is a name as `read_window` shows it, or the role and the
//!   quoted name (`button "Save"`); more than one match is an error that lists them.
//! - `type_text {text, window?}`: types the text into the window that has the keyboard focus, as
//!   the keys of the characters (so it does not depend on the keyboard layout). A `window` names
//!   the one that must have it (a call that does not is refused, nothing is typed).
//! - `key_press {keys, window?}`: presses chords of key names (`ctrl+a`, `alt+tab`, `enter`), one
//!   after the other; the names are the keys of a US keyboard. `window` as for `type_text`.
//!
//! A call that lmgw's thread approved before it arrived must name its window (not omit it, not
//! `active`): the active window at arrival is not what the owner saw when they approved.
//!
//! None of them returns what it typed or pressed, and Kai logs none of it.

use super::{Area, Ask, Hints, Kind, Param, ToolSpec, WINDOW};
use crate::caps::Capability;

const ACT: ToolSpec = ToolSpec {
    ask: Ask::Always,
    hints: Hints {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    },
    ..ToolSpec::CHANGE
};

pub const AREA: Area = Area {
    name: "input",
    tools: &[
        ToolSpec {
            name: "ui_press",
            description: "Press a button, link, menu item or other control of a window through \
                          accessibility.",
            params: &[
                WINDOW,
                Param::new("element", Kind::Text).described(
                    "its name as read_window shows it, or its role and quoted name: button \
                     \"Save\"",
                ),
            ],
            ..ACT
        },
        ToolSpec {
            name: "type_text",
            description: "Type text into whatever has the keyboard focus.",
            params: &[Param::new("text", Kind::Text), WINDOW],
            ..ACT
        },
        ToolSpec {
            name: "key_press",
            description: "Press keys or chords in the focused window, such as ctrl+a or alt+tab \
                          enter.",
            params: &[Param::new("keys", Kind::Text).described(
                "chords of key names joined by +, separated by spaces; US keyboard names",
            ), WINDOW],
            ..ACT
        },
    ],
};

/// The capabilities `name` needs; a tool its desktop cannot serve is not listed (K23).
pub fn needs(name: &str) -> &'static [Capability] {
    match name {
        "ui_press" => &[Capability::Accessibility, Capability::Windows],
        "type_text" | "key_press" => &[Capability::Input, Capability::Windows],
        _ => &[],
    }
}
