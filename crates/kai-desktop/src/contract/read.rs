//! Seeing and reading (WP15, K24's read-only tools, §11.1's vision): `see_window`, `see_screen`,
//! `read_window`, `read_selection`, `find_files`, `recent_files`, `clipboard_get`,
//! `clipboard_history`. Served by Kai's desktop module (`src/desktop/read.rs`), each listed only
//! where the desktop has the capability it uses (K23):
//! - `see_window`, `see_screen`: Capture (an image at the capture's real resolution, MCP's image
//!   content, with one line naming what it shows);
//! - `read_window`, `read_selection`: Accessibility and Windows (the window's accessible tree as
//!   indented lines; the selected text);
//! - `find_files`: Search where the provider has a file index; `recent_files`: Search;
//! - `clipboard_get`, `clipboard_history`: Clipboard.
//!
//! `window` resolves as everywhere (K22, [`super::WINDOW`]). Where the owner has the delegate look
//! at screenshots (`delegate.screenshots = "delegate"`, 2026-10-09), Kai's host lists `see_window`
//! and `see_screen` to the `look` delegate only, not to its callers.

use super::{Area, Kind, Param, ToolSpec, WINDOW};

const OUTPUT: Param = Param::new("output", Kind::Text)
    .optional()
    .described("an output's name; default: the active output");
const QUERY: Param = Param::new("query", Kind::Text)
    .described("words in the file's name or content; filename:<pattern> for names only");

pub const AREA: Area = Area {
    name: "read",
    tools: &[
        ToolSpec {
            name: "see_window",
            description: "See a window: a screenshot of its content.",
            params: &[WINDOW],
            returns_images: true,
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "see_screen",
            description: "See a whole output (monitor): a screenshot with the pointer.",
            params: &[OUTPUT],
            returns_images: true,
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "read_window",
            description: "Read a window's text and controls through accessibility, as indented \
                          lines.",
            params: &[WINDOW],
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "read_selection",
            description: "Read the text selected in the active window.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "find_files",
            description: "Find files in the desktop's file index by name or content.",
            params: &[QUERY],
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "recent_files",
            description: "List the recently used files, newest first.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "clipboard_get",
            description: "Read the text on the clipboard.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "clipboard_history",
            description: "List the clipboard's history, newest first.",
            ..ToolSpec::READ
        },
    ],
};
