//! Kai's browser tools (kai-firefox design D2, D3, §5, §7.3): what the model sees of Firefox
//! through Kai for Firefox. Kai owns the tools; the extension owns operations (D2), so a tool, a
//! description or a policy change needs no new extension. Served by Kai's browser provider
//! (`src/browser/`), which lists a tool only while a linked extension serves every operation it
//! needs (D3, the union over the linked profiles) and, for [`HANDS_AND_EYES_ONLY`], only while the
//! hands-and-eyes switch is on. The tools of the area go last in Kai's list, so a list that changes
//! when Firefox starts or quits keeps the cached prefix of every other tool.
//!
//! Common parameters:
//! - `tab`: a tab id, or a fragment of its title or URL, which Kai resolves like K22's `window`; the
//!   user's tab when omitted (D9: the active tab of the most recently focused Firefox window). An
//!   ambiguous fragment is an error listing the candidates.
//! - `ref`: an element's number (D4), unique for a Firefox run; it names its tab, frame and
//!   document, so element tools take no `tab`.
//!
//! Every result from a page starts with the tab's title and URL, so page text is never taken for
//! the user's words. Read-only tools carry `readOnlyHint`; tools that act on a page change state.
//! `page_script`, `page_script_main` and `element_file` are confirmed by default (K15, K24's class);
//! `page_see` and `element_see` return images (K25's setting of who looks at screenshots).
//!
//! `firefox_ui` and `firefox_press` (kai-firefox §3, WP12) reach Firefox's own UI, a page's
//! `alert`, `confirm` and `prompt`, and Firefox's own pages through the desktop's accessibility,
//! where the extension cannot; they are listed only where the desktop offers it. `firefox_press`
//! is not among the switch's tools ([`BY_SWITCH`]): it stays confirmed by default.

use super::{Area, Ask, Hints, Kind, Param, ToolSpec};

const TAB: Param = Param::new("tab", Kind::Text)
    .optional()
    .described("a tab id, or a fragment of its title or URL; default: the user's tab");
const TAB_NAMED: Param =
    Param::new("tab", Kind::Text).described("a tab id, or a fragment of its title or URL");
const REF: Param = Param::new("ref", Kind::Integer)
    .described("an element's ref, as the page and hint tools give it");
const URL: Param = Param::new("url", Kind::Text).described("a URL, or words to search for");
const ON: Param = Param::new("on", Kind::Boolean);
const TO_SCREEN: Param =
    Param::new("to", Kind::Text).described("down, up (a screen each), top or bottom");

/// Reads: changes nothing, reaches nothing outside the desktop.
const READ: ToolSpec = ToolSpec::READ;

/// Reads that may ask a server again (a PDF's bytes, the source of a page).
const READ_NET: ToolSpec = ToolSpec {
    hints: Hints {
        open_world: true,
        ..READ.hints
    },
    ..READ
};

/// Acts on a page or a tab; a second call does it again.
const ACT: ToolSpec = ToolSpec {
    hints: Hints {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    },
    ..ToolSpec::CHANGE
};

/// Acts and may load pages from the network (a navigation, a press that submits).
const ACT_NET: ToolSpec = ToolSpec {
    hints: Hints {
        open_world: true,
        ..ACT.hints
    },
    ..ACT
};

/// Sets a state: a second call with the same arguments changes nothing more.
const SET: ToolSpec = ToolSpec {
    hints: Hints {
        idempotent: true,
        ..ACT.hints
    },
    ..ACT
};

pub const AREA: Area = Area {
    name: "browser",
    tools: &[
        // §5.1 Where the user is
        ToolSpec {
            name: "browser_context",
            description: "Where the user is in Firefox: their tab, the focused element, the \
                          selection, what is under the pointer, how far down the page, an open \
                          dialog, the number of tabs, a reading aloud, and what was announced \
                          since the last call.",
            ..READ
        },
        // §5.2 Tabs and navigation
        ToolSpec {
            name: "tab_list",
            description: "List Firefox's tabs: id, title, URL and state.",
            ..READ
        },
        ToolSpec {
            name: "tab_open",
            description: "Open a new tab with a URL, or search with the user's search engine for \
                          text that is no URL.",
            params: &[
                URL,
                Param::new("sandbox", Kind::Boolean)
                    .optional()
                    .described("true: in Kai's own container, without the user's cookies"),
            ],
            ..ACT_NET
        },
        ToolSpec {
            name: "tab_go",
            description: "Load a URL in a tab, or search there for text that is no URL.",
            params: &[URL, TAB],
            ..ACT_NET
        },
        ToolSpec {
            name: "tab_switch",
            description: "Bring a tab to the front and focus its window.",
            params: &[TAB_NAMED],
            ..SET
        },
        ToolSpec {
            name: "tab_close",
            description: "Close a tab.",
            params: &[TAB],
            // What the page held (a form's input) can be lost.
            hints: Hints {
                destructive: true,
                ..SET.hints
            },
            ..SET
        },
        ToolSpec {
            name: "tab_back",
            description: "Go back to the tab's previous page.",
            params: &[TAB],
            ..ACT_NET
        },
        ToolSpec {
            name: "tab_forward",
            description: "Go forward to the tab's next page.",
            params: &[TAB],
            ..ACT_NET
        },
        ToolSpec {
            name: "tab_reload",
            description: "Reload the tab's page.",
            params: &[TAB],
            hints: Hints {
                idempotent: true,
                ..ACT_NET.hints
            },
            ..ACT_NET
        },
        ToolSpec {
            name: "tab_mute",
            description: "Mute or unmute a tab.",
            params: &[ON, TAB],
            ..SET
        },
        // §5.3 Eyes: reading
        ToolSpec {
            name: "page_outline",
            description: "List the page's landmarks and headings, each with its ref and level; \
                          the one in view is marked.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_controls",
            description: "List the links, buttons, fields and other controls in view, across \
                          frames: ref, role, name, value or state, label and error.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_links",
            description: "List every link of the whole page.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_fields",
            description: "List every form field of the whole page.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_text",
            description: "Read the page's article whole, or a PDF's text page by page; says what it \
                          left out, a PDF's scanned pages and lines that do not read as text \
                          (a font without a usable encoding) included.",
            params: &[TAB],
            ..READ_NET
        },
        ToolSpec {
            name: "page_text_all",
            description: "Read all of the page's text in reading order, frames included.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_text_visible",
            description: "Read the text in view, in reading order.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "element_text",
            description: "Read one element's text; for a heading, its whole section.",
            params: &[REF],
            ..READ
        },
        ToolSpec {
            name: "table_read",
            description: "Read a table as lines, each cell with its row and column headers.",
            params: &[REF],
            ..READ
        },
        ToolSpec {
            name: "page_selection",
            description: "Read the text selected in the page, whole; a password field's selection \
                          is never shown.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "page_find",
            description: "Find text in the page: the number of matches, and each with its \
                          element's ref and sentence; Firefox highlights them. Password and card \
                          fields are not searched.",
            params: &[Param::new("text", Kind::Text), TAB],
            ..READ
        },
        ToolSpec {
            name: "page_see",
            description: "See the page: a screenshot of the visible part.",
            params: &[TAB],
            returns_images: true,
            ..READ
        },
        ToolSpec {
            name: "element_see",
            description: "See one element (an image, a chart, a canvas): a screenshot cropped to \
                          it.",
            params: &[REF],
            returns_images: true,
            ..READ
        },
        // §5.4 Hands: acting
        ToolSpec {
            name: "element_press",
            description: "Press a link, button or other control: scroll it into view, focus it \
                          and activate it.",
            params: &[REF],
            ..ACT_NET
        },
        ToolSpec {
            name: "element_fill",
            description: "Replace a field's value with text.",
            params: &[REF, Param::new("text", Kind::Text)],
            ..SET
        },
        ToolSpec {
            name: "element_choose",
            description: "Pick an option of a select, listbox or radio group by its label.",
            params: &[REF, Param::new("option", Kind::Text)],
            ..SET
        },
        ToolSpec {
            name: "element_check",
            description: "Check or uncheck a checkbox or switch.",
            params: &[REF, ON],
            ..SET
        },
        ToolSpec {
            name: "element_file",
            description: "Give a file input a file from this computer, without a file picker.",
            params: &[
                REF,
                Param::new("path", Kind::Text).described("the file's path on this computer"),
            ],
            // It sends a local file to a site.
            ask: Ask::Always,
            hints: Hints {
                open_world: true,
                ..SET.hints
            },
            ..SET
        },
        ToolSpec {
            name: "element_focus",
            description: "Move the keyboard focus to an element, the caret to a field's end.",
            params: &[REF],
            ..SET
        },
        ToolSpec {
            name: "element_hover",
            description: "Move the pointer over an element, for menus a script opens on hover; \
                          menus made with CSS :hover do not react.",
            params: &[REF],
            ..SET
        },
        ToolSpec {
            name: "element_drag",
            description: "Drag an element onto another.",
            params: &[
                REF,
                Param::new("to", Kind::Integer).described("the ref of the element to drop it on"),
            ],
            ..ACT
        },
        ToolSpec {
            name: "element_show",
            description: "Scroll an element into view.",
            params: &[REF],
            ..SET
        },
        ToolSpec {
            name: "element_scroll",
            description: "Scroll inside a scroll container.",
            params: &[REF, TO_SCREEN],
            ..ACT
        },
        ToolSpec {
            name: "page_scroll",
            description: "Scroll the page.",
            params: &[TO_SCREEN, TAB],
            ..ACT
        },
        ToolSpec {
            name: "page_key",
            description: "Send keys to the page's focused element as synthetic key events.",
            params: &[
                Param::new("keys", Kind::Text)
                    .described("key names joined by +, separated by spaces: Escape, ctrl+k"),
                TAB,
            ],
            ..ACT_NET
        },
        ToolSpec {
            name: "hints_show",
            description: "Show numbered labels over the controls in view, the numbers being their \
                          refs, and list those controls; the labels stay until hints_hide or the \
                          page is left and follow a scroll, and a control whose label shows \
                          another number than its ref says hint N.",
            params: &[TAB],
            ..SET
        },
        ToolSpec {
            name: "hints_hide",
            description: "Remove the numbered labels.",
            params: &[TAB],
            ..SET
        },
        ToolSpec {
            name: "element_point",
            description: "Ring an element for a moment, to ask: this one?",
            params: &[REF],
            ..ACT
        },
        // §5.5 Firefox's own features
        ToolSpec {
            name: "page_zoom",
            description: "Set the tab's zoom; Firefox zooms every tab of the site alike. \
                          browser_context reads the zoom.",
            params: &[
                Param::new("percent", Kind::Integer).described("the zoom to set, in percent"),
                TAB,
            ],
            ..SET
        },
        ToolSpec {
            name: "reader_view",
            description: "Turn Reader view on or off for a tab.",
            params: &[ON, TAB],
            ..SET
        },
        ToolSpec {
            name: "bookmarks_find",
            description: "Find bookmarks: title, URL and when added.",
            params: &[Param::new("query", Kind::Text).described("words of the title or URL")],
            ..READ
        },
        ToolSpec {
            name: "history_find",
            description: "Find pages in Firefox's history: title, URL, when last visited and \
                          how often; Kai's own visits count too.",
            params: &[Param::new("query", Kind::Text).described("words of the title or URL")],
            ..READ
        },
        ToolSpec {
            name: "downloads_list",
            description: "List Firefox's downloads: file, path, size, state and when.",
            ..READ
        },
        // §5.6 Source and DOM
        ToolSpec {
            name: "page_source",
            description: "Read the HTML the server sent for the page, fetched again: from the \
                          browser's cache when it has a copy, else from the server, which repeats \
                          the page's request (a link that does something when opened, such as an \
                          unsubscribe, does it again); says which answered.",
            params: &[TAB],
            ..READ_NET
        },
        ToolSpec {
            name: "page_dom",
            description: "Read the live document as HTML, whole, with its shadow roots and frames.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "element_dom",
            description: "Read one element's live HTML.",
            params: &[REF],
            ..READ
        },
        // §5.7 Styles
        ToolSpec {
            name: "page_css",
            description: "Add a style sheet to the page until it is left, every declaration made \
                          !important; answers its id and what the parser did not understand.",
            params: &[Param::new("css", Kind::Text), TAB],
            ..ACT
        },
        ToolSpec {
            name: "page_css_remove",
            description: "Remove a style sheet page_css added.",
            params: &[Param::new("id", Kind::Integer).described("the id page_css answered")],
            ..SET
        },
        // §5.8 Scripts
        ToolSpec {
            name: "page_script",
            description: "Run JavaScript on the page's DOM in an isolated world, without the page's \
                          own variables; the last expression's value comes back as JSON, so end \
                          with plain data (no DOM node or function) and never with a top-level \
                          return. Top-level let, const and class end with the call; var and \
                          function declarations stay for later calls.",
            params: &[Param::new("code", Kind::Text), TAB],
            ask: Ask::Always,
            hints: Hints {
                destructive: true,
                ..ACT_NET.hints
            },
            ..ACT_NET
        },
        ToolSpec {
            name: "page_script_main",
            description: "Run JavaScript in the page's own world, where its scripts and variables \
                          live; the last expression's value comes back as JSON, so end with plain \
                          data (no DOM node or function) and never with a top-level return. \
                          Top-level let, const and class end with the call; var and function \
                          declarations become the page's globals.",
            params: &[Param::new("code", Kind::Text), TAB],
            ask: Ask::Always,
            hints: Hints {
                destructive: true,
                ..ACT_NET.hints
            },
            ..ACT_NET
        },
        // §7.3 Reading aloud
        ToolSpec {
            name: "read_aloud",
            description: "Read the page aloud from the focused element, else from the top of the \
                          view.",
            params: &[TAB],
            ..ACT
        },
        ToolSpec {
            name: "read_aloud_from",
            description: "Read the page aloud from an element on.",
            params: &[REF],
            ..ACT
        },
        ToolSpec {
            name: "read_pause",
            description: "Pause reading aloud.",
            ..SET
        },
        ToolSpec {
            name: "read_resume",
            description: "Resume reading aloud from the start of the paragraph it paused in.",
            ..SET
        },
        ToolSpec {
            name: "read_skip",
            description: "Skip paragraphs while reading aloud, from the one read now; it reads \
                          on from there.",
            params: &[Param::new("by", Kind::Integer)
                .described("paragraphs to skip; negative goes back")],
            ..ACT
        },
        // §3, WP12: Firefox's own UI, through the desktop's accessibility
        ToolSpec {
            name: "firefox_ui",
            description: "What Firefox shows around a page in front: a dialog the page opened \
                          (alert, confirm, prompt), Firefox's prompts (a site asking for a \
                          permission), notification bars, an open menu and the tool bars.",
            params: &[TAB],
            ..READ
        },
        ToolSpec {
            name: "firefox_press",
            description: "Press a control of Firefox's own, as firefox_ui lists it (a page \
                          dialog's OK, a permission prompt's Allow), or one in Firefox's own \
                          pages (about:, Reader view).",
            params: &[
                Param::new("control", Kind::Text)
                    .described("a name, or a role and a quoted name: button \"OK\""),
                TAB,
            ],
            ..ACT
        },
    ],
};

/// The tools that exist only for hands and eyes (D3): listed only while the hands-and-eyes switch
/// is on, so the usual list is the other 49.
pub const HANDS_AND_EYES_ONLY: [&str; 8] = [
    "page_links",
    "page_fields",
    "table_read",
    "read_aloud",
    "read_aloud_from",
    "read_pause",
    "read_resume",
    "read_skip",
];

/// The tools the hands-and-eyes switch lets trusted callers run unasked (§6.4): every
/// state-changing tool of §5.2, §5.4 and §7.3 but `element_file`, which sends a local file to a
/// site. An explicit `policy.tools` state still wins. §5.5 (`page_zoom`, `reader_view`), §5.7 and
/// §5.8 are not among them, so they keep K15's default.
pub const BY_SWITCH: [&str; 27] = [
    // §5.2
    "tab_open",
    "tab_go",
    "tab_switch",
    "tab_close",
    "tab_back",
    "tab_forward",
    "tab_reload",
    "tab_mute",
    // §5.4
    "element_press",
    "element_fill",
    "element_choose",
    "element_check",
    "element_focus",
    "element_hover",
    "element_drag",
    "element_show",
    "element_scroll",
    "page_scroll",
    "page_key",
    "hints_show",
    "hints_hide",
    "element_point",
    // §7.3
    "read_aloud",
    "read_aloud_from",
    "read_pause",
    "read_resume",
    "read_skip",
];

/// `name` is listed only while the hands-and-eyes switch is on ([`HANDS_AND_EYES_ONLY`]).
pub fn hands_and_eyes_only(name: &str) -> bool {
    HANDS_AND_EYES_ONLY.contains(&name)
}

/// The hands-and-eyes switch makes `name`'s default `allow_trusted` ([`BY_SWITCH`]).
pub fn by_switch(name: &str) -> bool {
    BY_SWITCH.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> &'static ToolSpec {
        AREA.tools.iter().find(|t| t.name == name).unwrap()
    }

    /// The design's 55 tools (§5.1 to §5.8, §7.3) and WP12's two (§3), with their parameters, in
    /// their order.
    #[test]
    fn the_area_has_the_designs_tools_and_parameters() {
        let got: Vec<String> = AREA
            .tools
            .iter()
            .map(|t| {
                let params: Vec<String> = t
                    .params
                    .iter()
                    .map(|p| {
                        format!("{}{}", p.name, if p.required { "" } else { "?" })
                    })
                    .collect();
                format!("{} {{{}}}", t.name, params.join(", "))
            })
            .collect();
        let want = [
            "browser_context {}",
            "tab_list {}",
            "tab_open {url, sandbox?}",
            "tab_go {url, tab?}",
            "tab_switch {tab}",
            "tab_close {tab?}",
            "tab_back {tab?}",
            "tab_forward {tab?}",
            "tab_reload {tab?}",
            "tab_mute {on, tab?}",
            "page_outline {tab?}",
            "page_controls {tab?}",
            "page_links {tab?}",
            "page_fields {tab?}",
            "page_text {tab?}",
            "page_text_all {tab?}",
            "page_text_visible {tab?}",
            "element_text {ref}",
            "table_read {ref}",
            "page_selection {tab?}",
            "page_find {text, tab?}",
            "page_see {tab?}",
            "element_see {ref}",
            "element_press {ref}",
            "element_fill {ref, text}",
            "element_choose {ref, option}",
            "element_check {ref, on}",
            "element_file {ref, path}",
            "element_focus {ref}",
            "element_hover {ref}",
            "element_drag {ref, to}",
            "element_show {ref}",
            "element_scroll {ref, to}",
            "page_scroll {to, tab?}",
            "page_key {keys, tab?}",
            "hints_show {tab?}",
            "hints_hide {tab?}",
            "element_point {ref}",
            "page_zoom {percent, tab?}",
            "reader_view {on, tab?}",
            "bookmarks_find {query}",
            "history_find {query}",
            "downloads_list {}",
            "page_source {tab?}",
            "page_dom {tab?}",
            "element_dom {ref}",
            "page_css {css, tab?}",
            "page_css_remove {id}",
            "page_script {code, tab?}",
            "page_script_main {code, tab?}",
            "read_aloud {tab?}",
            "read_aloud_from {ref}",
            "read_pause {}",
            "read_resume {}",
            "read_skip {by}",
            "firefox_ui {tab?}",
            "firefox_press {control, tab?}",
        ];
        assert_eq!(got, want);
        assert_eq!(AREA.tools.len(), 57);
        assert_eq!(AREA.tools.len() - HANDS_AND_EYES_ONLY.len(), 49, "D3");
    }

    #[test]
    fn the_annotations_follow_the_design() {
        for name in ["page_script", "page_script_main", "element_file"] {
            assert_eq!(spec(name).ask, Ask::Always, "{name}: confirmed by default");
        }
        for t in AREA.tools {
            if !["page_script", "page_script_main", "element_file"].contains(&t.name) {
                assert_eq!(t.ask, Ask::Hints, "{}", t.name);
            }
            assert_eq!(
                t.returns_images,
                ["page_see", "element_see"].contains(&t.name),
                "{}",
                t.name
            );
            assert_eq!(t.class, super::super::Class::Sync, "{}", t.name);
            assert_eq!(t.level, super::super::Level::Any, "{}", t.name);
        }
        // §5.1, §5.3 and §5.6 and the finds read; every other tool changes state.
        let reads: Vec<&str> = AREA
            .tools
            .iter()
            .filter(|t| t.hints.read_only)
            .map(|t| t.name)
            .collect();
        assert_eq!(
            reads,
            [
                "browser_context",
                "tab_list",
                "page_outline",
                "page_controls",
                "page_links",
                "page_fields",
                "page_text",
                "page_text_all",
                "page_text_visible",
                "element_text",
                "table_read",
                "page_selection",
                "page_find",
                "page_see",
                "element_see",
                "bookmarks_find",
                "history_find",
                "downloads_list",
                "page_source",
                "page_dom",
                "element_dom",
                "firefox_ui",
            ]
        );
    }

    /// The switch's tools are the state-changing ones of §5.2, §5.4 and §7.3 without
    /// `element_file`, and the hands-and-eyes-only ones are the area's.
    #[test]
    fn the_switch_and_its_tools_are_the_areas() {
        for name in BY_SWITCH {
            assert!(!spec(name).hints.read_only, "{name}");
            assert_ne!(spec(name).ask, Ask::Always, "{name}");
        }
        assert!(!by_switch("element_file"));
        assert!(!by_switch("firefox_press"), "WP12: confirmed by default");
        assert!(!by_switch("page_script"));
        assert!(!by_switch("page_zoom"));
        assert!(!by_switch("tab_list"));
        let acting = |range: std::ops::Range<usize>| {
            AREA.tools[range]
                .iter()
                .filter(|t| !t.hints.read_only && t.name != "element_file")
                .map(|t| t.name)
                .collect::<Vec<_>>()
        };
        let mut want = acting(1..10);
        want.extend(acting(23..38));
        want.extend(acting(50..55));
        assert_eq!(want, BY_SWITCH);
        for name in HANDS_AND_EYES_ONLY {
            spec(name);
        }
    }
}
