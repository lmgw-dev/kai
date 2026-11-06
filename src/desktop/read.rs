//! The tools of `contract::read` (WP15) over the chosen services: seeing (`see_window`,
//! `see_screen`), reading (`read_window`, `read_selection`), files (`find_files`, `recent_files`)
//! and the clipboard (`clipboard_get`, `clipboard_history`).
//!
//! - **Images** go as MCP image content (`image/png`, base64) at the capture's real resolution,
//!   after one text line naming what they show ([`image`]). lmgw sends them to the model by the
//!   answering alias's vision (egress §8.2); Kai knows no model, so it scales nothing.
//! - **Text results** are compact lines (K22): a desktop text that could forge a line is quoted
//!   (`resolve::quoted`, `shown_text`); nothing is cut. Where the result is one text the owner
//!   asked for whole (the selection, the clipboard), it is that text as it is.
//! - **`find_files`' limit** is a visible setting (`search.limit`), said in the result whenever the
//!   index has more matches, never a hidden cap (WP13 (g)).
//! - **Kai's browser site rules** (`super::sites`): `see_window`, `read_window` and
//!   `read_selection` refuse a Firefox window that shows a site Kai does not reach, checked before
//!   and after (`see_window` also one Firefox does not draw now, or whose tab in front has not
//!   settled); `read_window` and `read_selection` leave out every page and frame they read whose
//!   address is such a site's; `see_screen` blanks such windows grey. An answer about a Firefox
//!   window they let through says what the check does not cover (the tab strip, a capture's
//!   frames).
//! - **Nothing of what they read is logged**, at any level.

pub mod image;

use std::path::PathBuf;

use kai_desktop::caps::Failure;
use kai_desktop::caps::accessibility::{Element, Target, Tree};
use kai_desktop::caps::capture::Pixels;
use kai_desktop::caps::windows::Window;
use serde::{Deserialize, Serialize};

use super::resolve::{self, quoted, shown_text, window_line};
use super::sites;
use super::tools::Services;
use crate::browser::shown::Reach;
use crate::host::registry::{Args, Block, Reply};

pub use kai_desktop::contract::read::AREA;

/// `[search]`: the `find_files` limit.
///
/// ```toml
/// [search]
/// limit = 100     # find_files: at most this many files per search; the result says when the
///                 # index has more; 0: every match
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SearchConfig {
    pub limit: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { limit: 100 }
    }
}

/// `[accessibility]`: how hard `read_window` and `read_selection` may press the target app, and
/// when Kai switches accessibility on (`crate::desktop::fdo::atspi::switch`, the owner's rule of
/// 2026-10-09).
///
/// ```toml
/// [accessibility]
/// concurrency = 16   # AT-SPI calls in flight at once per walk (a throughput bound, not an
///                    # output bound: the whole tree is still read, only slower); 0: no bound
/// switch = "conversation"  # when Kai switches org.a11y.Status IsEnabled on (never
///                    # ScreenReaderEnabled), putting back the value it had after:
///                    # conversation: while conversation mode is active, and around each read
///                    # otherwise; read: only around each read; never: Kai does not switch it
/// wait_ms = 5000     # after Kai switched it on, how long a read waits for the window's app to
///                    # join the accessibility bus (the read goes on as soon as it joins)
/// action_settle_ms = 100  # an element whose first read of its actions said none (Firefox does
///                    # this for a link or a button, kai-firefox WP0 (d2): 0, then 1 about 100 ms
///                    # later) is read again this long after, before Kai takes it for one without
///                    # an action (ui_press, and Kai's presses in Firefox through accessibility)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AccessibilityConfig {
    pub concurrency: usize,
    pub switch: crate::desktop::fdo::atspi::switch::SwitchMode,
    pub wait_ms: u64,
    pub action_settle_ms: u64,
}

impl Default for AccessibilityConfig {
    fn default() -> Self {
        Self {
            concurrency: 16,
            switch: Default::default(),
            wait_ms: 5000,
            action_settle_ms: 100,
        }
    }
}

impl AccessibilityConfig {
    pub fn switch_settings(&self) -> crate::desktop::fdo::atspi::switch::SwitchSettings {
        crate::desktop::fdo::atspi::switch::SwitchSettings {
            mode: self.switch,
            wait: std::time::Duration::from_millis(self.wait_ms),
        }
    }
}

/// What the reading providers need of the environment: the XDG directories they read and the
/// programs they run. Empty in the unit tests' inert environment (nothing of the session's is
/// read); the tests set their own.
#[derive(Debug, Clone, Default)]
pub struct ReadEnv {
    /// `$XDG_DATA_HOME` (`recently-used.xbel`).
    pub data_home: PathBuf,
    /// `$XDG_CONFIG_HOME` (`baloofilerc`).
    pub config_home: PathBuf,
    /// `$HOME` (`$HOME` in `baloofilerc`'s folders).
    pub home: PathBuf,
    /// `baloosearch6` as found on `PATH`.
    pub baloosearch: Option<PathBuf>,
    /// `search.limit`.
    pub find_limit: usize,
    /// `accessibility.concurrency` (0: no bound).
    pub a11y_concurrency: usize,
    /// `accessibility.action_settle_ms`.
    pub a11y_action_settle_ms: u64,
}

impl ReadEnv {
    /// The running Kai's.
    #[cfg_attr(test, allow(dead_code))]
    pub fn from_env(search: &SearchConfig, accessibility: &AccessibilityConfig) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let xdg = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        Self {
            data_home: xdg("XDG_DATA_HOME", ".local/share"),
            config_home: xdg("XDG_CONFIG_HOME", ".config"),
            baloosearch: super::in_path("baloosearch6"),
            home: home.clone(),
            find_limit: search.limit,
            a11y_concurrency: accessibility.concurrency,
            a11y_action_settle_ms: accessibility.action_settle_ms,
        }
    }
}

/// The tool `name` of this area has what it needs here (K23: the others are not listed).
pub fn serves(services: &Services, name: &str) -> bool {
    match name {
        "see_window" | "see_screen" => services.capture.is_some(),
        "read_window" | "read_selection" => {
            services.accessibility.is_some() && services.windows.is_some()
        }
        "find_files" => services.search.as_ref().is_some_and(|s| s.indexes()),
        "recent_files" => services.search.is_some(),
        "clipboard_get" | "clipboard_history" => {
            services.clipboard.as_ref().is_some_and(|c| c.reads())
        }
        _ => false,
    }
}

/// Runs the tool `name` (one of `contract::read`).
pub async fn run(services: &Services, name: &str, args: &Args) -> Reply {
    let result = match name {
        "see_window" => see_window(services, args).await,
        "see_screen" => see_screen(services, args).await,
        "read_window" => read_window(services, args).await.map(Reply::text),
        "read_selection" => read_selection(services).await,
        "find_files" => find_files(services, args).await.map(Reply::text),
        "recent_files" => recent_files(services).await.map(Reply::text),
        "clipboard_get" => clipboard_get(services).await.map(Reply::text),
        "clipboard_history" => clipboard_history(services).await.map(Reply::text),
        other => Err(format!("Kai's desktop module has no tool {other}")),
    };
    result.unwrap_or_else(Reply::error)
}

fn say(f: Failure) -> String {
    f.to_string()
}

/// Not listed without the capability (K23): only a race with a relist gets here.
fn missing(what: &str) -> String {
    format!("this desktop offers Kai no {what}")
}

fn given(args: &Args, name: &str) -> Option<String> {
    args.text(name)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A capture's reply: its line and `notes` (one line each), then the image.
async fn image_reply(line: String, notes: &[String], pixels: Pixels) -> Result<Reply, String> {
    let size = format!("{}x{} pixels", pixels.width, pixels.height);
    let base64 = tokio::task::spawn_blocking(move || image::png_base64(&pixels))
        .await
        .map_err(|e| format!("Kai could not encode the capture: {e}"))??;
    let mut text = format!("{line}, {size}");
    for n in notes {
        text.push('\n');
        text.push_str(n);
    }
    Ok(Reply {
        blocks: vec![
            Block::Text(text),
            Block::Image {
                mime: "image/png".into(),
                base64,
            },
        ],
        error: false,
    })
}

async fn see_window(services: &Services, args: &Args) -> Result<Reply, String> {
    let capture = services
        .capture
        .as_ref()
        .ok_or_else(|| missing("screen capture"))?;
    let (line, id, checked) = match &services.windows {
        Some(w) => {
            let snap = w.snapshot().await.map_err(say)?;
            let win = resolve::window(&snap, args.text("window"))?;
            let checked = sites::check_capture(services, &snap, win, "see_window").await?;
            (window_line(win), Some(win.id.clone()), checked)
        }
        // Without the window list a window is the active one or named by its id.
        None => {
            sites::unlisted(services, "see_window")?;
            match given(args, "window") {
                Some(id) if !id.eq_ignore_ascii_case(kai_desktop::contract::ACTIVE) => {
                    ("the window".to_owned(), Some(id), None)
                }
                _ => ("the active window".to_owned(), None, None),
            }
        }
    };
    let pixels = capture.window(id.as_deref()).await.map_err(say)?;
    let mut notes = Vec::new();
    if let Some(c) = &checked {
        c.again(services, "see_window").await?;
        notes.extend(c.note().map(str::to_owned));
    }
    image_reply(line, &notes, pixels).await
}

async fn see_screen(services: &Services, args: &Args) -> Result<Reply, String> {
    let capture = services
        .capture
        .as_ref()
        .ok_or_else(|| missing("screen capture"))?;
    let asked = given(args, "output");
    let (output, placed) = match &services.windows {
        Some(w) => {
            let snap = w.snapshot().await.map_err(say)?;
            let name = match &asked {
                Some(q) => Some(resolve::output(&snap, q)?),
                None => snap
                    .outputs
                    .iter()
                    .find(|o| o.active)
                    .map(|o| o.name.clone()),
            };
            let placed = snap
                .outputs
                .into_iter()
                .find(|o| Some(&o.name) == name.as_ref());
            (name, placed)
        }
        None => (asked, None),
    };
    // Kai's site rules (`sites`): what to blank, before the capture and again after it.
    let before = match sites::blanks(services, None).await? {
        Some(b) => Some((
            b,
            placed.ok_or(
                "Kai's site rules keep some sites from Kai, and the desktop does not say where \
                 the output is, so Kai cannot blank Firefox's windows on it; see_screen did not \
                 run",
            )?,
        )),
        None => None,
    };
    let mut pixels = capture.screen(output.as_deref()).await.map_err(say)?;
    let mut notes = Vec::new();
    if let Some((before, out)) = before {
        let after = sites::blanks(services, Some(&before))
            .await?
            .unwrap_or_default();
        let (back, done) = tokio::task::spawn_blocking(move || {
            let done = sites::blank(&mut pixels, &out, &[before, after]);
            (pixels, done)
        })
        .await
        .map_err(|e| format!("Kai could not blank Firefox's windows out of the capture: {e}"))?;
        pixels = back;
        notes = sites::screen_notes(&done?);
    }
    let line = match &output {
        Some(name) => format!("output {}", shown_text(name)),
        None => "the active output".to_owned(),
    };
    image_reply(line, &notes, pixels).await
}

/// The window `window` names and the accessibility target for it.
async fn target(services: &Services, window: Option<&str>) -> Result<(Window, Target), String> {
    let windows = services
        .windows
        .as_ref()
        .ok_or_else(|| missing("window list"))?;
    let snap = windows.snapshot().await.map_err(say)?;
    let win = resolve::window(&snap, window)?.clone();
    let target = Target {
        pid: win.pid,
        title: win.title.clone(),
        exact: sites::exact(services, &win),
    };
    Ok((win, target))
}

async fn read_window(services: &Services, args: &Args) -> Result<String, String> {
    let access = services
        .accessibility
        .as_ref()
        .ok_or_else(|| missing("accessibility"))?;
    let (win, target) = target(services, args.text("window")).await?;
    let checked = sites::check(services, &win, Reach::See, "read_window").await?;
    let mut tree = access.tree(&target).await.map_err(say)?;
    let left_out = sites::pages::prune(services, checked.as_ref(), &mut tree);
    let mut text = tree_lines(&win, &tree);
    if let Some(c) = &checked {
        c.again(services, "read_window").await?;
        for n in sites::pages::left_out_note(left_out)
            .into_iter()
            .chain(c.note().map(str::to_owned))
        {
            text.push('\n');
            text.push_str(&n);
        }
    }
    Ok(text)
}

/// The roles shown even without a name or text: what a model may want to press or fill.
const CONTROLS: &[&str] = &[
    "button",
    "check box",
    "check menu item",
    "combo box",
    "entry",
    "link",
    "menu item",
    "page tab",
    "password text",
    "radio button",
    "radio menu item",
    "slider",
    "spin button",
    "switch",
    "toggle button",
];

/// `read_window`'s lines: the window's line, then one line per element that says something
/// (a name, a text, a value) or is a control, two spaces deeper than the element above it that
/// was shown; an element that says nothing is left out and its children take its place.
pub fn tree_lines(win: &Window, tree: &Tree) -> String {
    let mut out = vec![window_line(win)];
    element_lines(&tree.root, 0, &mut out);
    if tree.unanswered > 0 {
        out.push(format!(
            "({} elements did not answer and are missing)",
            tree.unanswered
        ));
    }
    out.join("\n")
}

fn element_lines(e: &Element, depth: usize, out: &mut Vec<String>) {
    if e.role.is_empty() && !e.note.is_empty() {
        out.push(format!("{}{}", "  ".repeat(depth), e.note));
        return;
    }
    let says = !e.name.is_empty() || !e.text.is_empty() || e.value.is_some() || !e.note.is_empty();
    let shown = says || CONTROLS.contains(&e.role.as_str());
    if shown {
        let mut line = format!("{}{}", "  ".repeat(depth), shown_text(&e.role));
        if !e.name.is_empty() {
            line.push(' ');
            line.push_str(&quoted(&e.name));
        }
        if !e.text.is_empty() && e.text != e.name {
            line.push_str(": ");
            line.push_str(&quoted(&e.text));
        }
        if let Some(v) = e.value {
            line.push_str(&format!(" = {v}"));
        }
        if !e.states.is_empty() {
            line.push_str(&format!(" ({})", e.states.join(", ")));
        }
        if !e.note.is_empty() {
            line.push(' ');
            line.push_str(&e.note);
        }
        out.push(line);
    }
    let next = if shown { depth + 1 } else { depth };
    for c in &e.children {
        element_lines(c, next, out);
    }
}

/// The selection as it is; where a Firefox window was let through by Kai's site rules, what their
/// check does not cover follows in a block of its own, so the selection's text stays whole.
async fn read_selection(services: &Services) -> Result<Reply, String> {
    let access = services
        .accessibility
        .as_ref()
        .ok_or_else(|| missing("accessibility"))?;
    let (win, target) = target(services, None).await?;
    let checked = sites::check(services, &win, Reach::See, "read_selection").await?;
    let mut tree = access.selection(&target).await.map_err(say)?;
    let left_out = sites::pages::prune(services, checked.as_ref(), &mut tree);
    let mut notes: Vec<String> = Vec::new();
    if let Some(c) = &checked {
        c.again(services, "read_selection").await?;
        notes.extend(sites::pages::left_out_note(left_out));
        notes.extend(c.note().map(str::to_owned));
    }
    // A page or frame left out says so in the notes, not as a piece of the selection.
    let pieces: Vec<String> = tree
        .texts()
        .into_iter()
        .filter(|p| !p.is_empty() && p != sites::pages::LEFT_OUT)
        .collect();
    let mut reply = if pieces.is_empty() {
        Reply::text(format!(
            "nothing is selected in the active window ({})",
            window_line(&win)
        ))
    } else {
        Reply::text(pieces.join("\n"))
    };
    if !notes.is_empty() {
        reply.blocks.push(Block::Text(notes.join("\n")));
    }
    Ok(reply)
}

async fn find_files(services: &Services, args: &Args) -> Result<String, String> {
    let search = services
        .search
        .as_ref()
        .filter(|s| s.indexes())
        .ok_or_else(|| missing("file index"))?;
    let query = given(args, "query").ok_or_else(|| "query is empty".to_owned())?;
    let limit = services.find_limit;
    let found = search.find(&query, limit).await.map_err(say)?;
    let mut lines: Vec<String> = found
        .paths
        .iter()
        .map(|p| shown_text(p).into_owned())
        .collect();
    if lines.is_empty() {
        lines.push("no file matches".into());
    }
    if found.more {
        lines.push(format!(
            "the first {limit} matches; the index has more (search.limit)"
        ));
    }
    lines.extend(found.notes);
    Ok(lines.join("\n"))
}

async fn recent_files(services: &Services) -> Result<String, String> {
    let search = services
        .search
        .as_ref()
        .ok_or_else(|| missing("recent files list"))?;
    let recents = search.recent().await.map_err(say)?;
    let skipped = recents.private_skipped;
    let mut lines: Vec<String> = recents
        .entries
        .iter()
        .map(|r| {
            let mut line = format!("{} {}", when(&r.used), shown_text(&r.location));
            if !r.apps.is_empty() {
                let apps: Vec<String> = r.apps.iter().map(|a| shown_text(a).into_owned()).collect();
                line.push_str(&format!(" ({})", apps.join(", ")));
            }
            line
        })
        .collect();
    if lines.is_empty() && skipped == 0 {
        return Ok("no recently used files".into());
    }
    if skipped > 0 {
        lines.push(format!(
            "({skipped} entries marked private by their writer are not listed)"
        ));
    }
    Ok(lines.join("\n"))
}

/// An ISO 8601 time to the minute (`2026-10-09T12:02:11.5Z` → `2026-10-09 12:02 UTC`); anything
/// else as it is.
pub fn when(iso: &str) -> String {
    let b = iso.as_bytes();
    let shape = b.len() >= 16
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && iso.is_char_boundary(16);
    if !shape {
        return shown_text(iso).into_owned();
    }
    let zone = if iso.ends_with('Z') { " UTC" } else { "" };
    format!("{} {}{zone}", &iso[..10], &iso[11..16])
}

async fn clipboard_get(services: &Services) -> Result<String, String> {
    let clip = services
        .clipboard
        .as_ref()
        .ok_or_else(|| missing("clipboard"))?;
    let text = clip.text().await.map_err(say)?;
    Ok(if text.is_empty() {
        "Klipper's clipboard entry holds no text".into()
    } else {
        text
    })
}

async fn clipboard_history(services: &Services) -> Result<String, String> {
    let clip = services
        .clipboard
        .as_ref()
        .ok_or_else(|| missing("clipboard"))?;
    let items = clip.history().await.map_err(say)?;
    if items.is_empty() {
        return Ok("the clipboard history is empty".into());
    }
    let lines: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t.is_empty() {
                format!("{} (no text)", i + 1)
            } else {
                format!("{} {}", i + 1, quoted(t))
            }
        })
        .collect();
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
