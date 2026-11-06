//! Firefox's window as `firefox_ui` reads it and `firefox_press` presses in it (the parent's
//! module doc): which of the window's elements Kai reads, which it leaves out, and their lines.
//!
//! Built against Firefox 159's tree on a private accessibility bus (kai-firefox WP12 and its
//! review round): the window is a `frame`; its direct children are the tool bars
//! (`toolbar-menubar`, `TabsToolbar`, `nav-bar`, `notifications-toolbar`), the tab panels
//! (`tabbrowser-tabpanels`: per tab a scroll pane holding the tab's page and, while a page's
//! `alert`, `confirm` or `prompt` is open, its dialog, a document of Firefox's own,
//! `chrome://global/content/commonDialog.xhtml`, role `dialog`, named "The page at <origin> says:"),
//! and, while open, the prompts and popups: `notification-popup` (role `alert`), whose children
//! are the open prompt's own (Firefox does not expose the prompt's element: its description,
//! class `popup-notification-description`, "Allow 127.0.0.1:8080 to access your location?", its
//! check box, its buttons), `appMenu-popup` (a `panel`), and menus. Elements are told by these
//! ids and classes, which Firefox exposes as AT-SPI's `id` and `class` object attributes.
//!
//! **Read: an allow-list** ([`parts`]), everything else left out and counted:
//! - the page's dialog: a `commonDialog.xhtml` document in the tab's own panel (so a background
//!   tab's dialog is never read; another document there, such as a protocol handler's dialog, is
//!   left out);
//! - Firefox's prompt for a site ([`Prompt::Permission`]): a prompt whose description names the
//!   tab's site (Firefox names the requesting site's host and port) and that holds nothing but the
//!   generic prompt's parts and the camera, microphone, speaker, screen, serial port and canvas
//!   choices; a site's request to share the screen or a window ([`Prompt::Screen`]) is read and
//!   never pressed;
//! - the notification bars: `notifications-toolbar` and a bar (`alert`, `info bar`) in the tab's
//!   own panel;
//! - the main tool bars (`toolbar-menubar` with its open menus, `TabsToolbar`, `nav-bar`), each
//!   without the tab strip;
//! - the menus: `appMenu-popup`'s main view (its items by their `appMenu-` ids; a sub-view's, the
//!   history, the bookmarks, the synced and recently closed tabs, are not read) and any open menu.
//!
//! **Left out**, each counted in the answer: the tab strip's tabs (other tabs' titles: `tab_list`
//! names them under the site rules), the bookmarks tool bar, every entry of the user's history and
//! bookmarks in a menu (Firefox's class `bookmark-item`) and the recently closed tabs and windows,
//! the address bar's suggestions, the password manager's prompt and the logins' autocomplete,
//! the save prompts of cards and addresses, every other prompt (an add-on's, a sign-in's, one that
//! names no site or another than the tab's), every panel not listed (the all-tabs list, the
//! downloads, the extensions, the site's information), any page or document inside a part, and
//! anything below a password field. An add-on's prompt is never pressed either ([`Prompt::Addon`];
//! `super::never`). The text of Firefox's own fields (the address bar, the search and find bars) is
//! never read, only their names; a page dialog's field (`prompt()`'s text) is the page's and is
//! read.

use kai_desktop::caps::accessibility::Element;

use crate::host::confirm::{quoted, shown_text};

/// Where an element of Firefox's window is (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// A page's `alert`, `confirm` or `prompt` (its words are the page's).
    Dialog,
    /// Firefox's prompt for a site (a permission).
    Prompt,
    /// A notification bar.
    Bar,
    /// A tool bar.
    Toolbar,
    /// A menu of Firefox's.
    Menu,
    /// One of Firefox's own pages (`about:`, Reader view, `view-source:`) in the tab.
    OwnPage,
}

impl Part {
    /// The answer's words for the part.
    pub fn words(self) -> &'static str {
        match self {
            Part::Dialog => "the page's dialog",
            Part::Prompt => "Firefox's prompt for a site",
            Part::Bar => "Firefox's notification bar",
            Part::Toolbar => "Firefox's tool bar",
            Part::Menu => "Firefox's menu",
            Part::OwnPage => "Firefox's own page",
        }
    }
}

/// What an open prompt of Firefox's is (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// A site asking for a permission: read; pressed only after the owner's yes with its words.
    Permission,
    /// A site asking to share the screen or a window: read, never pressed.
    Screen,
    /// About an add-on (its install, its permissions, an optional permission): not read, never
    /// pressed.
    Addon,
    /// Anything else (the password manager's, a card's or an address's, a sign-in's, one that
    /// names no site or another than the tab's): not read.
    Other,
}

/// Why Kai never presses in a screen-sharing prompt (`super::never`).
pub const NEVER_SCREEN: &str =
    "Firefox's prompt for a site to share the screen or a window, which Kai never presses";
/// Why Kai never presses in an add-on's prompt (`super::never`).
pub const NEVER_ADDON: &str = "Firefox's prompt about an add-on (its install or its permissions, \
                               private windows and user scripts included), which Kai never \
                               presses";

/// Ids of Firefox's window that are left out whole (module doc).
const LEFT_OUT_IDS: &[&str] = &[
    "tabbrowser-tabs",
    "PersonalToolbar",
    "urlbar-results",
    "urlbar-view",
    "PopupAutoComplete",
    "password-notification",
    "sidebar-box",
    "sidebar-main",
    "historyUndoMenu",
    "historyUndoPopup",
    "historyUndoWindowMenu",
    "historyUndoWindowPopup",
    "sync-tabs-menuitem",
];

/// The main tool bars, read without their left-out parts.
const TOOLBARS: &[&str] = &["toolbar-menubar", "TabsToolbar", "nav-bar"];

/// The application menu's main view, by its items' ids (prefixes): its sub-views are not read.
const APP_MENU: &[&str] = &[
    "appMenu-new-",
    "appMenu-history-button",
    "appMenu-bookmarks-button",
    "appMenu-downloads-button",
    "appMenu-passwords-button",
    "appMenu-extensions-themes-button",
    "appMenu-unified-extensions-button",
    "appMenu-print-button",
    "appMenu-save-file-button",
    "appMenu-find-button",
    "appMenu-translate-button",
    "appMenu-zoom",
    "appMenu-fullscreen-button",
    "appMenu-settings-button",
    "appMenu-more-button",
    "appMenu-help-button",
    "appMenu-quit-button",
    "appMenu-profiles-button",
    "appMenu-tab-groups-button",
];

/// The ids a site's permission prompt holds (prefixes): the generic prompt's buttons and the
/// camera, microphone, speaker, screen, serial port and canvas choices (module doc). A prompt with
/// any other id is not one.
const PERMISSION_IDS: &[&str] = &[
    "main-button",
    "webRTC-shareDevices-notification-description",
    "webRTC-selectCamera",
    "webRTC-selectMicrophone",
    "webRTC-selectSpeaker",
    "webRTC-selectWindow",
    "webRTC-all-windows-shared",
    "webRTC-preview",
    "preview-container",
    "stop-preview-button",
    "popup-select-",
    "webSerial-",
    "webserial-",
    "canvas-permissions-prompt-warning",
];

/// The ids of a screen-sharing prompt's choices.
const SCREEN_IDS: &[&str] = &[
    "webRTC-selectWindow",
    "webRTC-all-windows-shared",
    "webRTC-preview",
];

/// Roles whose text is a field's value (module doc: never read in Firefox's own UI).
const FIELDS: &[&str] = &["entry", "combo box", "text", "password text", "spin button"];

/// Roles that are worth a line without a name (a control).
const CONTROLS: &[&str] = &[
    "button",
    "check box",
    "check menu item",
    "combo box",
    "entry",
    "link",
    "menu item",
    "password text",
    "radio button",
    "radio menu item",
    "slider",
    "spin button",
    "toggle button",
];

/// The roles of a notification bar.
const BARS: &[&str] = &["alert", "info bar", "notification"];

/// The page dialog's document (module doc).
pub const COMMON_DIALOG: &str = "chrome://global/content/commonDialog.xhtml";

/// One part of the window as read: its kind, its element (with the left-out ones cut), and for a
/// prompt what it is and the sites it names.
#[derive(Debug, Clone)]
pub struct Read {
    pub part: Part,
    pub root: Element,
    /// The prompt's kind, for [`Part::Prompt`].
    pub prompt: Option<Prompt>,
    /// The sites the prompt's description names (host and port, as Firefox writes them).
    pub hosts: Vec<String>,
}

impl Read {
    /// Why Kai never presses in it, where it does not (module doc).
    pub fn never(&self) -> Option<&'static str> {
        match self.prompt {
            Some(Prompt::Screen) => Some(NEVER_SCREEN),
            Some(Prompt::Addon) => Some(NEVER_ADDON),
            _ => None,
        }
    }
}

/// Firefox's window around a tab, as [`parts`] takes it.
#[derive(Debug, Clone, Default)]
pub struct Parts {
    /// The parts read.
    pub read: Vec<Read>,
    /// The prompts not read that Kai never presses (an add-on's): only to say why a control named
    /// in one is not pressed.
    pub unread: Vec<Read>,
    /// The page's dialogs, as their documents' elements, to be read whole by the caller.
    pub dialogs: Vec<Element>,
    /// How many elements were left out.
    pub left: usize,
}

/// The parts of `frame` (Firefox's window, read with details and without its documents) for the
/// tab whose page is the shown document at `page` (its place) and whose site is `site` (its host
/// and port as Firefox names a site in a prompt) (module doc).
pub fn parts(frame: &Element, page: &(String, String), site: Option<&str>) -> Parts {
    let mut out = Parts::default();
    for c in &frame.children {
        if c.id == "tabbrowser-tabpanels" {
            // The tab's own panel: the one holding its page.
            if let Some(panel) = panel_of(c, page) {
                in_panel(panel, page, &mut out);
            }
            continue;
        }
        if c.role == "tool bar" {
            let part = if c.id == "notifications-toolbar" {
                Part::Bar
            } else if TOOLBARS.contains(&c.id.as_str()) {
                Part::Toolbar
            } else {
                // The bookmarks tool bar, a tool bar Kai cannot place.
                out.left += 1;
                continue;
            };
            let mut root = c.clone();
            out.left += cut(&mut root, false);
            out.read.push(read(part, root));
            continue;
        }
        if !c.showing || c.id == "a11y-announcement" {
            continue;
        }
        if c.id == "notification-popup" {
            let mut root = c.clone();
            let kind = prompt(&root, site);
            let hosts = description(&root)
                .map(|d| named_hosts(&d))
                .unwrap_or_default();
            match kind {
                Prompt::Permission | Prompt::Screen => {
                    out.left += cut(&mut root, false);
                    out.read.push(Read {
                        part: Part::Prompt,
                        root,
                        prompt: Some(kind),
                        hosts,
                    });
                }
                Prompt::Addon => {
                    out.left += 1;
                    out.unread.push(Read {
                        part: Part::Prompt,
                        root,
                        prompt: Some(kind),
                        hosts,
                    });
                }
                Prompt::Other => out.left += 1,
            }
            continue;
        }
        if c.id == "appMenu-popup" {
            let mut root = c.clone();
            let before = root.children.len();
            root.children
                .retain(|k| APP_MENU.iter().any(|p| k.id.starts_with(p)));
            out.left += before - root.children.len();
            out.left += cut(&mut root, false);
            out.read.push(read(Part::Menu, root));
            continue;
        }
        if matches!(c.role.as_str(), "menu" | "popup menu")
            && !LEFT_OUT_IDS.contains(&c.id.as_str())
        {
            let mut root = c.clone();
            out.left += cut(&mut root, false);
            out.read.push(read(Part::Menu, root));
            continue;
        }
        // A part Kai does not read (a panel not listed, the developer tools, a split view).
        out.left += 1;
    }
    out
}

fn read(part: Part, root: Element) -> Read {
    Read {
        part,
        root,
        prompt: None,
        hosts: Vec::new(),
    }
}

/// The child of the tab panels holding the page at `page`.
fn panel_of<'a>(panels: &'a Element, page: &(String, String)) -> Option<&'a Element> {
    panels.children.iter().find(|c| holds(c, page))
}

fn holds(e: &Element, at: &(String, String)) -> bool {
    e.at.as_ref() == Some(at) || e.children.iter().any(|c| holds(c, at))
}

/// The page dialogs and the notification bars in the tab's panel; any other document there is
/// left out.
fn in_panel(e: &Element, page: &(String, String), out: &mut Parts) {
    for c in &e.children {
        if c.document.is_some() {
            if c.at.as_ref() == Some(page) {
                continue;
            }
            let common = c
                .document
                .as_deref()
                .is_some_and(|d| d.split(['?', '#']).next() == Some(COMMON_DIALOG));
            if c.role == "dialog" && common {
                out.dialogs.push(c.clone());
            } else {
                out.left += 1;
            }
            continue;
        }
        if c.showing && BARS.contains(&c.role.as_str()) {
            let mut root = c.clone();
            out.left += cut(&mut root, false);
            out.read.push(read(Part::Bar, root));
            continue;
        }
        in_panel(c, page, out);
    }
}

/// Every id and class token below `e`, `e`'s included.
fn ids_and_classes<'a>(e: &'a Element, ids: &mut Vec<&'a str>, classes: &mut Vec<&'a str>) {
    if !e.id.is_empty() {
        ids.push(&e.id);
    }
    classes.extend(e.class.split_whitespace());
    for c in &e.children {
        ids_and_classes(c, ids, classes);
    }
}

/// The open prompt's description (its words that name the site).
fn description(root: &Element) -> Option<String> {
    fn find(e: &Element) -> Option<&Element> {
        if e.class
            .split_whitespace()
            .any(|k| k == "popup-notification-description")
        {
            return Some(e);
        }
        e.children.iter().find_map(find)
    }
    let d = find(root)?;
    Some(if d.text.is_empty() {
        d.name.clone()
    } else {
        d.text.clone()
    })
}

/// What the open prompt below `root` (`notification-popup`) is, for a tab whose site is `site`
/// (module doc).
pub fn prompt(root: &Element, site: Option<&str>) -> Prompt {
    let (mut ids, mut classes) = (Vec::new(), Vec::new());
    for c in &root.children {
        ids_and_classes(c, &mut ids, &mut classes);
    }
    if ids
        .iter()
        .any(|i| i.starts_with("addon-") || i.starts_with("popup-notification-xpinstall"))
        || classes.iter().any(|c| c.starts_with("addon-"))
    {
        return Prompt::Addon;
    }
    if ids
        .iter()
        .any(|i| !PERMISSION_IDS.iter().any(|p| i.starts_with(p)))
        || classes.iter().any(|c| c.contains("capture"))
    {
        return Prompt::Other;
    }
    let named = description(root)
        .map(|d| named_hosts(&d))
        .unwrap_or_default();
    let Some(site) = site else {
        return Prompt::Other;
    };
    if !named.iter().any(|h| same_site(h, site)) {
        return Prompt::Other;
    }
    if ids
        .iter()
        .any(|i| SCREEN_IDS.iter().any(|p| i.starts_with(p)))
    {
        Prompt::Screen
    } else {
        Prompt::Permission
    }
}

/// `a` and `b` name the same site in a prompt (Firefox may leave out a leading `www.`).
fn same_site(a: &str, b: &str) -> bool {
    let strip = |s: &str| s.strip_prefix("www.").unwrap_or(s).to_ascii_lowercase();
    strip(a) == strip(b)
}

/// The sites a prompt's words name (Firefox writes a site's host and port: "Allow
/// 127.0.0.1:8080 to access your location?"), in any language: each word that reads as a host,
/// with a dot or a port, or `localhost`; as `host[:port]`.
pub fn named_hosts(words: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in words.split_whitespace() {
        let w = w.trim_matches(|c: char| {
            matches!(
                c,
                ':' | ','
                    | '.'
                    | ';'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '«'
                    | '»'
                    | '"'
                    | '“'
                    | '”'
                    | '„'
                    | '\''
                    | '‘'
                    | '’'
                    | '¿'
                    | '¡'
            )
        });
        if w.is_empty()
            || w.contains(['/', '@', '\\'])
            || !(w.contains('.') || w.contains(':') || w.eq_ignore_ascii_case("localhost"))
        {
            continue;
        }
        let Ok(u) = url::Url::parse(&format!("http://{w}/")) else {
            continue;
        };
        let Some(host) = u.host_str() else { continue };
        let named = match u.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_owned(),
        };
        if !out.contains(&named) {
            out.push(named);
        }
    }
    out
}

/// Cuts what is left out (module doc) below `e`, and the text of Firefox's own fields unless
/// `page_words` (a page's dialog); how many were left out. An element cut whole becomes empty
/// (no role), which the lines skip.
pub fn cut(e: &mut Element, page_words: bool) -> usize {
    let mut n = 0;
    if !page_words && FIELDS.contains(&e.role.as_str()) {
        e.text.clear();
        e.value = None;
    }
    if e.role == "password text" {
        e.text.clear();
        e.children.clear();
    }
    let mut kept = Vec::new();
    for mut c in std::mem::take(&mut e.children) {
        if LEFT_OUT_IDS.contains(&c.id.as_str())
            || c.class.split_whitespace().any(|k| k == "bookmark-item")
            || c.document.is_some()
        {
            n += 1;
            continue;
        }
        n += cut(&mut c, page_words);
        kept.push(c);
    }
    e.children = kept;
    n
}

/// Whether a password field is below `e` (a sign-in dialog, `super::never`).
pub fn has_password(e: &Element) -> bool {
    e.role == "password text" || e.children.iter().any(has_password)
}

/// The lines of `e` and below, indented by `depth` (as `read_window`'s, without the empty ones).
pub fn lines(e: &Element, depth: usize, out: &mut Vec<String>) {
    let says = !e.name.is_empty() || !e.text.is_empty() || e.value.is_some();
    let shown = says || CONTROLS.contains(&e.role.as_str());
    if shown && !e.role.is_empty() {
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
        out.push(line);
    }
    let next = if shown { depth + 1 } else { depth };
    for c in &e.children {
        // A control's own label (Firefox 159 gives its prompts' buttons one) is its name already.
        if CONTROLS.contains(&e.role.as_str())
            && c.role == "label"
            && c.name == e.name
            && c.children.is_empty()
        {
            continue;
        }
        lines(c, next, out);
    }
}

/// The pressable elements below `e`, in the tree's order.
pub fn pressables<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
    if e.pressable && !e.name.is_empty() && e.at.is_some() {
        out.push(e);
    }
    for c in &e.children {
        pressables(c, out);
    }
}

/// What a dialog or a prompt says, quoted: its texts and names in order, and each check box with
/// whether it is checked; a control's own label is not taken again.
pub fn says(e: &Element, out: &mut Vec<String>) {
    match e.role.as_str() {
        "label" | "paragraph" | "static" => {
            let w = if e.text.is_empty() { &e.name } else { &e.text };
            if !w.is_empty() {
                out.push(quoted(w));
            }
        }
        "check box" => {
            out.push(format!(
                "check box {} ({})",
                quoted(&e.name),
                if e.states.contains(&"checked") {
                    "checked"
                } else {
                    "not checked"
                }
            ));
            return;
        }
        "button" | "link" | "menu item" | "toggle button" => return,
        _ => {}
    }
    for c in &e.children {
        says(c, out);
    }
}

/// The origin a dialog's title names (Firefox: "The page at https://shop.example says:"), in any
/// language: the first word that parses as a URL with a host.
pub fn named_origin(title: &str) -> Option<String> {
    title.split_whitespace().find_map(|w| {
        let w = w.trim_end_matches([':', ',', '.', ';', ')', '»', '"', '“', '”']);
        let u = url::Url::parse(w).ok()?;
        u.host_str()?;
        Some(u.origin().ascii_serialization())
    })
}

/// The buttons of a dialog: a page's choice when more than one.
pub fn buttons(e: &Element) -> usize {
    usize::from(e.role == "button") + e.children.iter().map(buttons).sum::<usize>()
}

#[cfg(test)]
mod tests;
