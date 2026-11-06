//! The peer check's matcher against the cgroups Plasma, systemd and the owner's own tools make.

use crate::browser::peer::{
    DEFAULT_SCOPES, Peer, check, glob, innermost_unit, matching_unit, too_wide,
};

fn defaults() -> Vec<String> {
    DEFAULT_SCOPES.iter().map(|s| (*s).to_owned()).collect()
}

fn under_app_slice(unit: &str) -> String {
    format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n")
}

#[test]
fn the_glob_matches_whole_names() {
    assert!(glob("app-firefox-*.scope", "app-firefox-1234.scope"));
    assert!(glob("app-firefox-*.scope", "app-firefox-.scope"));
    assert!(!glob("app-firefox-*.scope", "app-firefox-1234.scope.bak"));
    assert!(!glob("app-firefox-*.scope", "xapp-firefox-1.scope"));
    assert!(glob("*", ""));
    assert!(glob("a*b*c", "aXbYbZc"));
    assert!(!glob("a*b*c", "aXbYbZ"));
    assert!(glob("app-?.scope", "app-x.scope"));
    assert!(!glob("app-?.scope", "app-xy.scope"));
    assert!(glob("app-ä?-*", "app-äö-1"), "characters, not bytes");
    assert!(!glob("App-firefox-*", "app-firefox-1"), "case counts");
    assert!(glob(
        "app-userapp\\x2dFirefox\\x2d*",
        "app-userapp\\x2dFirefox\\x2dAB12CD@0123.service"
    ));
}

/// Firefox as Plasma starts it is let in by the defaults; Firefox from a terminal, an IDE, a
/// test harness or Kai itself is not.
#[test]
fn the_defaults_take_firefox_as_plasma_starts_it() {
    let scopes = defaults();
    for unit in [
        // From a link Plasma opened, through the "userapp" entry it writes.
        "app-userapp\\x2dFirefox\\x2dAB12CD@0123456789abcdef0123456789abcdef.service",
        // From the launcher or task manager (KIO's transient service), and autostart.
        "app-org.mozilla.firefox@0b6f3c8e2f0a4d7c9b8a1e2d3c4b5a69.service",
        "app-org.mozilla.firefox@autostart.service",
        // As a scope.
        "app-org.mozilla.firefox-48211.scope",
        // Where the desktop id is plain `firefox`.
        "app-firefox@autostart.service",
        "app-firefox-48211.scope",
    ] {
        assert_eq!(
            matching_unit(&under_app_slice(unit), &scopes),
            Some(unit),
            "{unit}"
        );
    }
    for unit in [
        "app-org.kde.konsole-61734.scope",
        "app-com.microsoft.VSCode-202194.scope",
        "app-dev.rootshell.kai-dev1730000000.scope",
        "app-dev.rootshell.kai.routine.backup-4d2a91c.scope",
        // Another Mozilla app, whose escaped dash is not the random part's.
        "app-org.mozilla.firefox\\x2desr@1.service",
        "app-org.mozilla.Thunderbird@1.service",
    ] {
        assert_eq!(
            matching_unit(&under_app_slice(unit), &scopes),
            None,
            "{unit}"
        );
    }
    // The unit is the innermost .service or .scope, on any line; a cgroup below it is not one.
    let nested = "1:net_cls:/\n0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                  app-firefox-1.scope/sub\n";
    assert_eq!(matching_unit(nested, &scopes), Some("app-firefox-1.scope"));
    assert_eq!(matching_unit("0::/\n", &scopes), None);
    assert_eq!(matching_unit("", &scopes), None);
}

/// Only the unit the process runs in counts: the units and slices above it, which every program
/// of the session is under, never match, whatever the pattern.
#[test]
fn only_the_innermost_unit_counts() {
    let konsole = under_app_slice("app-org.kde.konsole-61734.scope");
    for wide in [
        "user@1000.service",
        "*.service",
        "app.slice",
        "app.*",
        "user*",
    ] {
        assert_eq!(
            matching_unit(&konsole, &[wide.to_owned()]),
            None,
            "{wide} must not match an ancestor"
        );
    }
    assert_eq!(
        matching_unit(&konsole, &["*.scope".to_owned()]),
        Some("app-org.kde.konsole-61734.scope")
    );
    assert_eq!(
        innermost_unit("/user.slice/user-1000.slice/user@1000.service/app.slice/a.scope/x/y"),
        Some("a.scope")
    );
    assert_eq!(
        innermost_unit("/user.slice/user-1000.slice/user@1000.service/init.scope"),
        Some("init.scope")
    );
    assert_eq!(innermost_unit("/user.slice/user-1000.slice"), None);
    assert_eq!(innermost_unit("/"), None);
}

/// The settings' check: a pattern the session's own units or slices pass is too wide, the
/// defaults are not.
#[test]
fn a_pattern_the_whole_session_passes_is_too_wide() {
    for p in DEFAULT_SCOPES {
        assert_eq!(too_wide(p, 1000), None, "{p}");
    }
    for (p, says) in [
        ("*.service", "user@1000.service"),
        ("user@1000.service", "user@1000.service"),
        ("user-1000*", "user-1000.slice"),
        ("app*", "app.slice"),
        ("background.slic?", "background.slice"),
        ("app-firefox.slice", "names a slice"),
    ] {
        let why = too_wide(p, 1000).unwrap_or_else(|| panic!("{p} passed"));
        assert!(why.contains(says), "{p}: {why}");
    }
    assert_eq!(too_wide("user@1001.service", 1000), None, "another user's");
}

#[test]
fn the_check_says_why() {
    let scopes = defaults();
    let peer = |unit: &str| Peer {
        pid: 42,
        cgroup: format!("1:net_cls:/\n{}", under_app_slice(unit)),
        exe: None,
        parent: None,
    };
    let firefox = peer("app-firefox-1.scope");
    assert_eq!(
        check(1000, 1000, Some(&firefox), &scopes),
        Ok("app-firefox-1.scope".into())
    );
    let e = check(1001, 1000, Some(&firefox), &scopes).unwrap_err();
    assert!(e.contains("user 1001"), "{e}");
    let e = check(1000, 1000, None, &scopes).unwrap_err();
    assert!(e.contains("could not read which program"), "{e}");
    let e = check(1000, 1000, Some(&firefox), &[]).unwrap_err();
    assert!(e.contains("empty"), "{e}");
    let konsole = peer("app-org.kde.konsole-1.scope");
    let e = check(1000, 1000, Some(&konsole), &scopes).unwrap_err();
    assert!(
        e.contains(
            "/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.konsole-1.scope"
        ),
        "the unified line, not net_cls's: {e}"
    );
    assert_eq!(konsole.exe_or_unknown(), "(unknown)");
}

/// This test process's own record, through `/proc`.
#[test]
fn the_peer_is_read_from_proc() {
    let me = std::process::id() as i32;
    let p = crate::browser::peer::proc_peer(me).unwrap();
    assert_eq!(p.pid, me);
    assert!(!p.cgroup.is_empty());
    assert!(p.exe.is_some());
    // Its parent, as Firefox is kai-browser-host's (the window it owns, `crate::browser::desk`).
    assert_eq!(p.parent, Some(std::os::unix::process::parent_id() as i32));
    assert_eq!(
        crate::browser::peer::parent_of("Name:\tx\nPPid:\t 812\nTracerPid:\t0\n"),
        Some(812)
    );
    assert_eq!(crate::browser::peer::parent_of("Name:\tx\n"), None);
}
