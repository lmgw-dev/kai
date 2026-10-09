//! Tool-controlled overlays (ruling 6, ruling 24, §10.1): one show tool per kind, each returning the
//! overlay's id, with an optional flat `position`; `overlay_hide` and `overlay_list`. Showing never
//! replaces an overlay (no update): to change one, hide it and show a new one. Output, margin, size,
//! timeout and whether a kind takes clicks come from Kai's configuration (`[overlay.<kind>]`), not
//! from parameters. Kai's widgets module serves them (`src/widgets/`).
//!
//! PD4: drawing over the desktop changes state, so an untrusted caller is asked; the owner, the
//! gateway and Kai's own key are not ([`Ask::Trusted`]).
//!
//! **A new kind** is one more `show(…)` entry below, before `overlay_hide`: its content parameter
//! first, then [`POSITION`]. Kai's side is one variant in `src/widgets/kinds.rs`. `overlay_app`
//! (WP12, K18) is the one show tool without `position`: its two parameters name the view (K22), so
//! its place is the configuration's (`[overlay.app]`).

use super::{Area, Ask, Hints, Kind, Param, ToolSpec};

/// Every show tool's second parameter.
pub const POSITION: Param = Param::new("position", Kind::Text).optional().described(
    "top-left, top, top-right, left, center, right, bottom-left, bottom or bottom-right; default: Kai's setting overlay.<kind>.position",
);

/// A show adds an overlay: not idempotent (each call shows another one), nothing outside the
/// desktop.
const SHOW: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: false,
};

/// A show that reaches outside the desktop (`overlay_image` and `overlay_weather` fetch,
/// `overlay_app`'s view talks to its MCP server): the one difference from [`SHOW`] is
/// `openWorldHint`.
const SHOW_FETCH: Hints = Hints {
    open_world: true,
    ..SHOW
};

/// Hiding the same id twice leaves the desktop as hiding it once.
const HIDE: Hints = Hints {
    idempotent: true,
    ..SHOW
};

/// A show tool of `name` taking `content` and [`POSITION`].
const fn show(
    name: &'static str,
    description: &'static str,
    content: &'static [Param],
    hints: Hints,
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        params: content,
        hints,
        ask: Ask::Trusted,
        ..ToolSpec::CHANGE
    }
}

pub const AREA: Area = Area {
    name: "overlay",
    tools: &[
        // The show tools, one per kind, before overlay_hide.
        show(
            "overlay_text",
            "Show a text over the desktop; returns its id.",
            &[Param::new("text", Kind::Text), POSITION],
            SHOW,
        ),
        show(
            "overlay_html",
            "Show an HTML document over the desktop (inline scripts, styles and data: images; no network); returns its id.",
            &[Param::new("html", Kind::Text), POSITION],
            SHOW,
        ),
        show(
            "overlay_image",
            "Show an image over the desktop from an http(s) or data: URL (png, jpeg, gif, webp); returns its id.",
            &[Param::new("src", Kind::Text), POSITION],
            SHOW_FETCH,
        ),
        show(
            "overlay_emote",
            "Show a built-in emote over the desktop (smile, laugh, wink, sad, surprised, angry, heart, check, question); returns its id.",
            &[Param::new("name", Kind::Text), POSITION],
            SHOW,
        ),
        show(
            "overlay_weather",
            "Show the current weather and today's range (DWD data through Bright Sky) over the desktop; returns its id.",
            &[
                Param::new("place", Kind::Text).optional().described(
                    "a saved place name or latitude,longitude (e.g. 52.52,13.405); default: home",
                ),
                POSITION,
            ],
            SHOW_FETCH,
        ),
        show(
            "overlay_app",
            "Show an MCP server's app view (its ui:// resource) over the desktop, talking only to that server's tools; returns its id.",
            &[
                Param::new("server", Kind::Text)
                    .described("the server's label, as lmgw prefixes its tools"),
                Param::new("uri", Kind::Text)
                    .described("the view's ui:// URI as lmgw names it (ui://<server>__…)"),
            ],
            SHOW_FETCH,
        ),
        ToolSpec {
            name: "overlay_hide",
            description: "Hide one overlay by its id, or every overlay without one.",
            params: &[Param::new("id", Kind::Integer).optional()],
            hints: HIDE,
            ask: Ask::Trusted,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "overlay_list",
            description: "List the overlays shown now, one line each.",
            ..ToolSpec::READ
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Level, check};

    #[test]
    fn the_overlay_tools_keep_the_rules_and_ruling_24s_shape() {
        let names: Vec<&str> = AREA.tools.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "overlay_text",
                "overlay_html",
                "overlay_image",
                "overlay_emote",
                "overlay_weather",
                "overlay_app",
                "overlay_hide",
                "overlay_list"
            ]
        );
        for t in AREA.tools {
            assert_eq!(check(t), Vec::<String>::new(), "{}", t.name);
            assert_eq!(t.level, Level::Any, "api §5.6 decides who reaches them");
        }
        // Each show tool: its content (required), then the optional flat position; no update.
        for t in &AREA.tools[..5] {
            assert_eq!(t.params.len(), 2, "{}", t.name);
            // overlay_weather's place defaults to home.
            assert_eq!(t.params[0].required, t.name != "overlay_weather");
            assert!(t.params[0].kind == Kind::Text);
            assert_eq!(t.params[1], POSITION);
            assert_eq!(t.ask, Ask::Trusted, "PD4");
            assert!(!t.hints.read_only && !t.hints.idempotent && !t.hints.destructive);
        }
        let fetching: Vec<&str> = AREA
            .tools
            .iter()
            .filter(|t| t.hints.open_world)
            .map(|t| t.name)
            .collect();
        assert_eq!(
            fetching,
            ["overlay_image", "overlay_weather", "overlay_app"],
            "K19's fetches, and a view that talks to its server"
        );
        // overlay_app: its two parameters name the view (K22), no position.
        let app = &AREA.tools[5];
        let params: Vec<(&str, bool)> = app.params.iter().map(|p| (p.name, p.required)).collect();
        assert_eq!(params, [("server", true), ("uri", true)]);
        assert_eq!(app.ask, Ask::Trusted, "PD4");
        assert!(!app.hints.read_only && !app.hints.idempotent && !app.hints.destructive);
        let hide = &AREA.tools[6];
        assert_eq!(hide.params.len(), 1);
        assert!(!hide.params[0].required && hide.params[0].kind == Kind::Integer);
        assert!(hide.hints.idempotent && hide.ask == Ask::Trusted);
        let list = &AREA.tools[7];
        assert!(list.params.is_empty() && list.hints.read_only);
        assert!(!names.iter().any(|n| n.contains("update")), "ruling 24");
    }
}
