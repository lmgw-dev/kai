//! K22's resolution of a tool's `window`, `desktop` and `output` against a snapshot: `window` is
//! omitted for the active window, or a window id, an app name or a fragment of the title. An
//! ambiguous match is an error that lists every candidate, one line each; a miss says where to
//! look. Errors never echo what the caller wrote (K22).
//!
//! In order, the first rule that matches anything decides:
//! 1. omitted, blank or `active` (any case; `kai_desktop::contract::ACTIVE`, decided by the owner
//!    2026-10-09): the active window. The word wins over a title that contains it; such a window is
//!    named by its id or app;
//! 2. the id, with or without braces, any case;
//! 3. the app: its whole app id, or the app id's last dotted part (`dolphin` for
//!    `org.kde.dolphin`), any case;
//! 4. a fragment of the title, any case.
//!
//! **Kai's own windows** (its configuration window, its confirmations, the ring and the overlays)
//! are never acted on ([`not_kais`]): a window whose app id is Kai's, or whose process is Kai or a
//! process Kai started itself (its WebKit processes, at any depth). The window tools, the input
//! tools (the window that would receive the keys) and `ui_press` refuse them, so a tool call that
//! was allowed or confirmed cannot answer Kai's own dialog or change its settings. Reading them
//! (`window_list`, `see_window`) is fine.

use kai_desktop::caps::windows::{Snapshot, Window};

pub use crate::host::confirm::{quoted, shown_text};

/// The live checks' test windows (test builds only): children of the test process that stand in
/// for the owner's apps, so the window tools act on them as on any app's.
#[cfg(test)]
pub(crate) static STAND_INS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// A test window's process is gone (killed and reaped): it is no stand-in any more, so a later
/// process that gets its pid is not taken for one.
#[cfg(test)]
pub(crate) fn stand_in_gone(pid: u32) {
    STAND_INS
        .lock()
        .expect("never poisoned")
        .retain(|p| *p != pid);
}

/// The process `pid` is Kai or descends from Kai (module doc): its parent chain in `/proc` reaches
/// Kai's pid. A pid of 0 (the compositor names none) is no process.
pub fn own_process(pid: u32) -> bool {
    #[cfg(test)]
    if STAND_INS.lock().expect("never poisoned").contains(&pid) {
        return false;
    }
    let me = std::process::id();
    let mut seen = std::collections::HashSet::new();
    let mut p = pid;
    while p > 1 && seen.insert(p) {
        if p == me {
            return true;
        }
        let Some(parent) = parent_of(p) else {
            return false;
        };
        p = parent;
    }
    false
}

/// The parent pid from `/proc/<pid>/stat` (the fourth field, after the name in parentheses).
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// One of Kai's own windows (module doc).
pub fn is_kais(w: &Window) -> bool {
    w.app == crate::APP_ID || own_process(w.pid)
}

/// The refusal for one of Kai's own windows (module doc).
pub fn not_kais(w: &Window) -> Result<(), String> {
    if is_kais(w) {
        return Err(format!(
            "{} is one of Kai's own windows; Kai's tools do not act on them (reading them is fine)",
            window_line(w)
        ));
    }
    Ok(())
}

/// A window's line in a list and in a candidate list: `<id> <app> "<title>"`. The title is always
/// quoted, a name only where it holds a line break or a deceptive character, so no desktop text
/// can forge a line of the result; nothing is cut.
pub fn window_line(w: &Window) -> String {
    format!(
        "{} {} {}",
        shown_text(&w.id),
        shown_text(&w.app),
        quoted(&w.title)
    )
}

fn ambiguous(found: &[&Window]) -> String {
    let mut text = format!(
        "{} windows match; name one by its id (window_list lists them all):",
        found.len()
    );
    for w in found {
        text.push('\n');
        text.push_str(&window_line(w));
    }
    text
}

/// The window `query` names (module doc).
pub fn window<'a>(snap: &'a Snapshot, query: Option<&str>) -> Result<&'a Window, String> {
    let q = query.map(str::trim).unwrap_or_default();
    if implicit(query) {
        return snap
            .windows
            .iter()
            .find(|w| w.active)
            .ok_or_else(|| "no window is active; name one (window_list lists them)".to_owned());
    }
    let q = q.to_lowercase();
    let bare = q.trim_start_matches('{').trim_end_matches('}');
    let rules: [&dyn Fn(&Window) -> bool; 3] = [
        &|w| {
            w.id.trim_start_matches('{')
                .trim_end_matches('}')
                .to_lowercase()
                == bare
        },
        &|w| {
            let app = w.app.to_lowercase();
            !app.is_empty() && (app == q || app.rsplit('.').next() == Some(q.as_str()))
        },
        &|w| w.title.to_lowercase().contains(&q),
    ];
    for rule in rules {
        let found: Vec<&Window> = snap.windows.iter().filter(|w| rule(w)).collect();
        match found.len() {
            0 => continue,
            1 => return Ok(found[0]),
            _ => return Err(ambiguous(&found)),
        }
    }
    Err("no window has that id, app or title (window_list lists them)".into())
}

/// Whether `query` leaves the window to the moment of the call: omitted, blank or `active`.
pub fn implicit(query: Option<&str>) -> bool {
    let q = query.map(str::trim).unwrap_or_default();
    q.is_empty() || q.eq_ignore_ascii_case(kai_desktop::contract::ACTIVE)
}

/// A call lmgw's thread approved (`Origin::approved_in_lmgw`) was approved before it arrived, so
/// the active window now may be one the owner never saw (lmgw's dashboard, which is not Kai's
/// own): it must name its window. Decided 2026-10-09 (review of WP14/WP16c).
pub fn explicit_if_approved(
    query: Option<&str>,
    origin: crate::host::registry::Origin,
) -> Result<(), String> {
    if origin.approved_in_lmgw && implicit(query) {
        return Err(
            "this call was approved in lmgw before it ran, and the active window may \
                    not be the one it was approved for; name the window by its id or app \
                    (window_list lists them)"
                .into(),
        );
    }
    Ok(())
}

/// The virtual desktop `query` names, by its number (from 1) or its name, any case: its position
/// in the snapshot's desktops (from 0).
pub fn desktop(snap: &Snapshot, query: &str) -> Result<usize, String> {
    let q = query.trim();
    let by_number = q
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&i| i < snap.desktops.len());
    let by_name = || {
        snap.desktops
            .iter()
            .position(|d| d.to_lowercase() == q.to_lowercase())
    };
    by_number.or_else(by_name).ok_or_else(|| {
        let list: Vec<String> = snap
            .desktops
            .iter()
            .enumerate()
            .map(|(i, d)| format!("{} {}", i + 1, shown_text(d)))
            .collect();
        format!(
            "no such virtual desktop; the desktops are: {}",
            list.join(", ")
        )
    })
}

/// The output `query` names, any case.
pub fn output(snap: &Snapshot, query: &str) -> Result<String, String> {
    let q = query.trim().to_lowercase();
    snap.outputs
        .iter()
        .find(|o| o.name.to_lowercase() == q)
        .map(|o| o.name.clone())
        .ok_or_else(|| {
            let names: Vec<String> = snap
                .outputs
                .iter()
                .map(|o| shown_text(&o.name).into_owned())
                .collect();
            format!("no such output; the outputs are: {}", names.join(", "))
        })
}

#[cfg(test)]
mod tests {
    use kai_desktop::caps::windows::Output;

    use super::*;

    fn win(id: &str, app: &str, title: &str, active: bool) -> Window {
        Window {
            id: id.into(),
            app: app.into(),
            title: title.into(),
            active,
            ..Default::default()
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            windows: vec![
                win("aaaa-1", "org.kde.dolphin", "Home — Dolphin", false),
                win("bbbb-2", "org.kde.konsole", "build: cargo — Konsole", true),
                win("cccc-3", "org.kde.konsole", "logs — Konsole", false),
                win("dddd-4", "firefox", "Release notes \"6.7\"\nnext", false),
            ],
            outputs: vec![
                Output {
                    name: "DP-1".into(),
                    ..Default::default()
                },
                Output {
                    name: "HDMI-A-1".into(),
                    ..Default::default()
                },
            ],
            desktops: vec!["Desktop 1".into(), "Mail".into()],
            ..Default::default()
        }
    }

    /// The resolution table (WP14's tests): each rule, its order, ambiguity and misses.
    #[test]
    fn a_window_resolves_by_id_app_or_title_in_that_order() {
        let s = snap();
        let id = |q: Option<&str>| window(&s, q).map(|w| w.id.as_str());
        assert_eq!(id(None), Ok("bbbb-2"), "omitted: the active window");
        assert_eq!(id(Some("  ")), Ok("bbbb-2"));
        assert_eq!(id(Some("active")), Ok("bbbb-2"), "the shorthand");
        assert_eq!(id(Some(" Active ")), Ok("bbbb-2"), "any case");
        assert_eq!(
            id(Some("{AAAA-1}")),
            Ok("aaaa-1"),
            "the id, braces and case aside"
        );
        assert_eq!(id(Some("Dolphin")), Ok("aaaa-1"), "the app id's last part");
        assert_eq!(
            id(Some("org.kde.dolphin")),
            Ok("aaaa-1"),
            "the whole app id"
        );
        assert_eq!(id(Some("FIREFOX")), Ok("dddd-4"));
        assert_eq!(id(Some("logs")), Ok("cccc-3"), "a title fragment");
        assert_eq!(id(Some("release NOTES")), Ok("dddd-4"));
        // The app rule matches two windows before the title rule is tried.
        let e = id(Some("konsole")).unwrap_err();
        assert_eq!(
            e,
            "2 windows match; name one by its id (window_list lists them all):\n\
             bbbb-2 org.kde.konsole \"build: cargo — Konsole\"\n\
             cccc-3 org.kde.konsole \"logs — Konsole\""
        );
        let e = id(Some("o")).unwrap_err();
        assert!(e.starts_with("4 windows match"), "{e}");
        assert_eq!(e.lines().count(), 5, "every candidate, none left out");
        assert!(
            e.contains(r#"dddd-4 firefox "Release notes \"6.7\"\nnext""#),
            "a title cannot forge a line: {e}"
        );
        let e = id(Some("thunderbird")).unwrap_err();
        assert_eq!(
            e,
            "no window has that id, app or title (window_list lists them)"
        );
        assert!(!e.contains("thunderbird"), "no echo of the input");
        let none_active = Snapshot {
            windows: vec![win("x", "a", "b", false)],
            ..Default::default()
        };
        assert_eq!(
            window(&none_active, None).unwrap_err(),
            "no window is active; name one (window_list lists them)"
        );
        assert_eq!(
            window(&none_active, Some("active")).unwrap_err(),
            "no window is active; name one (window_list lists them)"
        );
        // The word wins over a title that holds it; the id and the app still name that window.
        let titled = Snapshot {
            windows: vec![
                win("x", "org.kde.kate", "inactive notes", false),
                win("y", "org.kde.konsole", "shell", true),
            ],
            ..Default::default()
        };
        let id = |q: Option<&str>| window(&titled, q).map(|w| w.id.as_str());
        assert_eq!(id(Some("active")), Ok("y"));
        assert_eq!(id(Some("kate")), Ok("x"));
        assert_eq!(
            id(Some("inactive")),
            Ok("x"),
            "a longer fragment is a fragment"
        );
    }

    #[test]
    fn desktops_resolve_by_number_or_name_and_outputs_by_name() {
        let s = snap();
        assert_eq!(desktop(&s, "2"), Ok(1));
        assert_eq!(desktop(&s, " mail "), Ok(1));
        assert_eq!(desktop(&s, "desktop 1"), Ok(0));
        assert_eq!(
            desktop(&s, "3"),
            Err("no such virtual desktop; the desktops are: 1 Desktop 1, 2 Mail".into())
        );
        assert!(desktop(&s, "0").is_err());
        assert_eq!(output(&s, "hdmi-a-1"), Ok("HDMI-A-1".into()));
        assert_eq!(
            output(&s, "left"),
            Err("no such output; the outputs are: DP-1, HDMI-A-1".into())
        );
    }
}
