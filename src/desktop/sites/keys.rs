//! `type_text` and `key_press` under Kai's site rules (the parent's module doc).
//!
//! - **Chords Firefox takes for itself** ([`chords`]): in a Firefox window, while some site is
//!   `read` or `none`, `key_press` refuses (before the owner is asked) the chords that switch or
//!   move tabs (Ctrl+Tab, Ctrl+Page Up and Down, Alt and Ctrl with a digit), put the focus in the
//!   address or search bar (Ctrl+L, Alt+D, F6, Ctrl+K, Ctrl+E, Ctrl+J), open, reopen or close tabs
//!   and windows (Ctrl+T, Ctrl+N, Ctrl+Shift+P, Ctrl+W, Ctrl+F4, Ctrl+U), go back, forward or home
//!   (Alt+Left, Alt+Right, Alt+Home), or open Firefox's own panels and windows, which show other
//!   sites' titles and addresses (the history and bookmarks sidebars and toolbar, the Library,
//!   the downloads, Page Info, the add-ons, the developer tools). Tab moves belong to
//!   `tab_switch`, addresses to `tab_go` and `tab_open`, the history to `tab_back` and
//!   `tab_forward`, which apply the rules themselves; the refusal says so. A chord bound to these
//!   in a changed Firefox configuration is not told apart yet.
//! - **The desktop's own shortcuts** ([`chords`]): while some site is `none`, `key_press` refuses,
//!   in any window, the chords of KWin's and Plasma's default shortcuts that move, switch or
//!   preview windows (any chord with Meta, Alt+Tab, Alt+`, Alt with a function key, Ctrl with
//!   F1 to F4 and F7 to F12, Ctrl+Alt with an arrow, Ctrl+Alt+Esc, Ctrl+Esc, Print): a window then
//!   drawn away from its frame is not blanked by `see_screen`. Window state changes go through the
//!   window tools. A shortcut the owner bound to another chord is not told apart yet.
//! - **While the keys go** ([`KeyWatch`]): before every chord and character the input tools'
//!   focus guard hands this the window with the focus. Its title is compared with the one last
//!   checked; at the first unit (once the focus is back after the dialog) and whenever it changed,
//!   the window is listed and checked again. And before every chord, and before the first
//!   character and each one after a Tab or a line break, the frame with the keyboard focus is
//!   checked by its own site (`Shown::focus_refusal`).

use std::sync::Mutex;

use kai_desktop::caps::input::Chord;
use kai_desktop::caps::windows::Window;

use super::{Services, refusal, rules};
use crate::browser::shown::Reach;

const CTRL: u32 = 29;
const SHIFT: u32 = 42;
const ALT: u32 = 56;
const ALTGR: u32 = 100;
const META: u32 = 125;

/// A chord's modifiers and its other keys.
fn split(c: &Chord) -> (Vec<u32>, Vec<u32>) {
    c.0.iter()
        .partition(|k| matches!(**k, CTRL | SHIFT | ALT | ALTGR | META))
}

/// The evdev codes of the digit keys 1 to 9 and 0.
fn digit(k: u32) -> bool {
    (2..=11).contains(&k)
}

/// The evdev code of F`n`.
fn f(n: u32) -> u32 {
    match n {
        1..=10 => 58 + n,
        11 | 12 => 76 + n,
        _ => 170 + n,
    }
}

/// What a chord does in Firefox that the browser tools do under the rules, and which tools.
fn firefox_class(c: &Chord) -> Option<&'static str> {
    let (mods, keys) = split(c);
    let [key] = keys.as_slice() else {
        return None;
    };
    let key = *key;
    let has = |m: u32| mods.contains(&m);
    let (ctrl, alt, shift) = (has(CTRL), has(ALT), has(SHIFT));
    const TAB: &str =
        "switches or moves Firefox's tab, which tab_switch does under Kai's site rules";
    const ADDRESS: &str = "puts the focus in Firefox's address or search bar, and tab_go and \
                           tab_open load an address under Kai's site rules";
    const OPENS: &str = "opens, reopens or closes Firefox's tabs or windows, which tab_open and \
                         tab_close do under Kai's site rules";
    const HISTORY: &str = "goes back, forward or home in Firefox's history, which tab_back, \
                           tab_forward and tab_go do under Kai's site rules";
    const PANELS: &str = "opens one of Firefox's own panels or windows, which show other sites' \
                          titles and addresses";
    match (ctrl, alt, key) {
        // Tab, Page Up, Page Down.
        (true, false, 15 | 104 | 109) => Some(TAB),
        (false, true, k) if digit(k) => Some(TAB),
        (true, false, k) if digit(k) && k != 11 => Some(TAB),
        // L, D, F6, K, E, J.
        (true, false, 38 | 37 | 18) if !shift => Some(ADDRESS),
        (true, false, 36) if !shift => Some(ADDRESS),
        (false, true, 32) => Some(ADDRESS),
        (_, _, 64) => Some(ADDRESS),
        // T, N, P (private window), W, F4, U (the source in a new tab).
        (true, false, 20 | 49 | 17 | 22) => Some(OPENS),
        (true, false, 25) if shift => Some(OPENS),
        (true, false, 62) => Some(OPENS),
        // Left, Right, Home.
        (false, true, 105 | 106 | 102) => Some(HISTORY),
        // H, B (sidebars, the bookmarks toolbar), O and Y (the Library, the downloads), I (Page
        // Info), A (the add-ons), the developer tools (I, J, K, C, E, M, Z with Shift; F12).
        (true, false, 35 | 48) => Some(PANELS),
        (true, false, 24 | 21 | 30 | 46 | 50 | 44 | 23 | 37 | 18 | 36) if shift => Some(PANELS),
        (true, false, 23) => Some(PANELS),
        (_, _, k) if k == f(12) => Some(PANELS),
        _ => None,
    }
}

/// A chord of the desktop's own shortcuts that move, switch or preview windows (module doc).
fn desktop_chord(c: &Chord) -> bool {
    let (mods, keys) = split(c);
    let has = |m: u32| mods.contains(&m);
    if has(META) {
        return true;
    }
    let [key] = keys.as_slice() else {
        // Modifiers alone (Meta alone opens the launcher; caught above).
        return false;
    };
    let key = *key;
    let fkey = (1..=24).find(|n| f(*n) == key);
    let (ctrl, alt) = (has(CTRL), has(ALT));
    match (ctrl, alt) {
        // Print, with any modifier: the screenshot tool.
        _ if key == 99 => true,
        // Alt+Tab, Alt+`, Alt+Space (KRunner), Alt with a function key.
        (false, true) => key == 15 || key == 41 || key == 57 || fkey.is_some(),
        // Ctrl+F1 to F4 (desktops), Ctrl+F7 to F12 (the Overview's views, peek at the desktop),
        // Ctrl+Esc (the system monitor).
        (true, false) => {
            key == 1 || fkey.is_some_and(|n| (1..=4).contains(&n) || (7..=12).contains(&n))
        }
        // Ctrl+Alt with an arrow (desktops), Ctrl+Alt+Esc (kill a window), Ctrl+Alt+Del.
        (true, true) => matches!(key, 103 | 105 | 106 | 108 | 1 | 111),
        (false, false) => false,
    }
}

/// Refuses `keys` for `key_press` into `w` before the owner is asked (module doc).
pub fn chords(services: &Services, w: &Window, keys: &[Chord], tool: &str) -> Result<(), String> {
    if rules(services, Reach::See).is_some()
        && let Some(n) = keys.iter().position(desktop_chord)
    {
        return Err(format!(
            "chord {} is one of the desktop's own shortcuts that move, switch or preview windows \
             (Meta, Alt+Tab, Alt or Ctrl with a function key, Ctrl+Alt with an arrow, Print), \
             which may draw a window away from where see_screen blanks it while Kai's site rules \
             keep some sites from Kai; the window tools change windows; {tool} did not run",
            n + 1
        ));
    }
    if let Some((shown, _)) = rules(services, Reach::Act)
        && shown.firefox(w)
    {
        for (n, c) in keys.iter().enumerate() {
            if let Some(what) = firefox_class(c) {
                return Err(format!(
                    "chord {} {what}, while Kai's site rules let Kai only read some sites or \
                     keep them from Kai; {tool} did not run",
                    n + 1
                ));
            }
        }
    }
    Ok(())
}

/// The checks while an input call's keys go into a Firefox window under the rules (module doc).
pub struct KeyWatch {
    services: Services,
    /// Before which units the frame with the focus is checked.
    frames: Vec<bool>,
    /// The title last checked, and the unit it was checked before.
    last: Mutex<Option<(String, usize)>>,
}

impl KeyWatch {
    /// The watch over keys into `w`, or `None` while no rule could refuse acting or `w` is not
    /// Firefox's. `frames[i]`: the frame with the focus is checked before unit `i`.
    pub fn new(services: &Services, w: &Window, frames: Vec<bool>) -> Option<KeyWatch> {
        let (shown, _) = rules(services, Reach::Act)?;
        shown.firefox(w).then(|| KeyWatch {
            services: services.clone(),
            frames,
            last: Mutex::new(None),
        })
    }

    /// Before unit `done` goes to `now`, the window with the focus (the confirmed one): why it may
    /// not (module doc).
    pub async fn before(&self, now: &Window, done: usize) -> Result<(), String> {
        let (changed, again) = {
            let last = self.last.lock().expect("never poisoned");
            match &*last {
                None => (true, false),
                Some((title, at)) => (title != &now.title, *at == done),
            }
        };
        if again && !changed {
            // The provider asks before a unit more than once (the first, at the start).
            return Ok(());
        }
        if changed && let Some(r) = refusal(&self.services, now, Reach::Act).await? {
            return Err(r);
        }
        if (changed || self.frames.get(done).copied().unwrap_or(false))
            && let Some((shown, _)) = rules(&self.services, Reach::Act)
            && let Some(r) = shown.focus_refusal(now).await
        {
            return Err(r);
        }
        *self.last.lock().expect("never poisoned") = Some((now.title.clone(), done));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kai_desktop::caps::input::parse_keys;

    use super::*;

    fn one(keys: &str) -> Chord {
        parse_keys(keys).unwrap().remove(0)
    }

    #[test]
    fn firefox_keeps_its_tab_address_and_history_chords_under_the_rules() {
        for keys in [
            "ctrl+tab",
            "ctrl+shift+tab",
            "ctrl+pagedown",
            "ctrl+shift+pageup",
            "alt+3",
            "alt+0",
            "ctrl+1",
            "ctrl+9",
            "ctrl+l",
            "alt+d",
            "f6",
            "shift+f6",
            "ctrl+k",
            "ctrl+e",
            "ctrl+j",
            "ctrl+t",
            "ctrl+shift+t",
            "ctrl+n",
            "ctrl+shift+n",
            "ctrl+shift+p",
            "ctrl+w",
            "ctrl+f4",
            "ctrl+u",
            "alt+left",
            "alt+right",
            "alt+home",
            "ctrl+h",
            "ctrl+b",
            "ctrl+shift+b",
            "ctrl+shift+o",
            "ctrl+shift+y",
            "ctrl+i",
            "ctrl+shift+i",
            "ctrl+shift+k",
            "f12",
        ] {
            assert!(firefox_class(&one(keys)).is_some(), "{keys}");
        }
        for keys in [
            "enter",
            "space",
            "tab",
            "shift+tab",
            "ctrl+a",
            "ctrl+c",
            "ctrl+v",
            "ctrl+z",
            "ctrl+f",
            "ctrl+0",
            "ctrl+plus",
            "pagedown",
            "down",
            "f5",
            "ctrl+f5",
            "escape",
            "a",
            "shift+a",
            "ctrl+s",
            "backspace",
            "alt",
        ] {
            assert_eq!(firefox_class(&one(keys)), None, "{keys}");
        }
    }

    #[test]
    fn the_desktops_window_shortcuts_are_told() {
        for keys in [
            "super+w",
            "super",
            "ctrl+super+right",
            "meta+d",
            "alt+tab",
            "alt+shift+tab",
            "alt+`",
            "alt+space",
            "alt+f2",
            "alt+f4",
            "ctrl+f1",
            "ctrl+f4",
            "ctrl+f8",
            "ctrl+f9",
            "ctrl+f10",
            "ctrl+f12",
            "ctrl+alt+left",
            "ctrl+alt+esc",
            "ctrl+esc",
            "print",
            "shift+print",
        ] {
            assert!(desktop_chord(&one(keys)), "{keys}");
        }
        for keys in [
            "ctrl+tab",
            "alt+left",
            "ctrl+f5",
            "ctrl+f6",
            "f5",
            "enter",
            "ctrl+alt+t",
            "alt+d",
            "ctrl+w",
        ] {
            assert!(!desktop_chord(&one(keys)), "{keys}");
        }
    }
}
