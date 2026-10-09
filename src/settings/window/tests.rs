//! WP21b's tests: what the scheme handler serves and refuses, the page's wire types against the
//! real ones, the CSP, the zoom. The window itself is `live_tests.rs`.

use serde_json::json;

use super::route::{Answer, Incoming, Routed, route};
use super::{assets, next_scale, tools};
use crate::settings::{self, Request};

/// The frontend's own copy of the wire types (`ui/src/wire.rs`), compiled here, so the tests read
/// Kai's real answers with exactly what the page reads them with.
#[path = "../../../ui/src/wire.rs"]
#[allow(dead_code)]
pub(super) mod wire;

fn incoming<'a>(method: &'a str, uri: &'a str, body: &'a [u8]) -> Incoming<'a> {
    Incoming {
        method,
        uri,
        origin: None,
        fetch_site: None,
        body,
    }
}

fn answer(r: Routed) -> Answer {
    match r {
        Routed::Answer(a) => a,
        other => panic!("not answered by the route: {other:?}"),
    }
}

const LIST: &[u8] = br#"{"op":"list","section":null}"#;

#[test]
fn another_origin_is_refused_and_runs_nothing() {
    for uri in [
        "kai://app/api/apply",
        "kai://app/ring.html",
        "kai://settings.evil/api/apply",
        "kai://other/api/tools",
        "https://settings/api/apply",
        "http://127.0.0.1:8001/api/apply",
        "file:///etc/passwd",
    ] {
        for method in ["GET", "POST"] {
            let a = answer(route(&incoming(method, uri, LIST)));
            assert_eq!(a.status, 403, "{method} {uri}");
        }
    }
    assert_eq!(
        answer(route(&incoming("POST", "not a url", LIST))).status,
        400
    );
}

#[test]
fn a_request_from_another_origin_or_site_is_refused_even_on_this_host() {
    for (origin, site) in [
        (Some("kai://app"), None),
        (Some("https://example.org"), None),
        (Some("null"), None),
        (None, Some("cross-site")),
        (None, Some("same-site")),
        (Some("kai://settings"), Some("cross-site")),
    ] {
        let req = Incoming {
            origin,
            fetch_site: site,
            ..incoming("POST", "kai://settings/api/apply", LIST)
        };
        assert_eq!(answer(route(&req)).status, 403, "{origin:?} {site:?}");
    }
    let ok = Incoming {
        origin: Some("kai://settings"),
        fetch_site: Some("same-origin"),
        ..incoming("POST", "kai://settings/api/apply", LIST)
    };
    assert!(matches!(
        route(&ok),
        Routed::Apply(Request::List { section: None })
    ));
}

#[test]
fn only_the_operations_are_reachable() {
    // Every operation of `settings::Request` reaches the core as itself.
    let ops = [
        json!({"op": "list", "section": "voice"}),
        json!({"op": "get", "key": "voice.mode"}),
        json!({"op": "set", "key": "voice.grace_s", "value": "45"}),
        json!({"op": "unset", "key": "voice.grace_s"}),
        json!({"op": "tool_state", "tool": "run_command", "state": "allow"}),
        json!({"op": "tool_level", "tool": "run_command", "level": null}),
        json!({"op": "provider", "capability": "windows", "provider": "fdo"}),
    ];
    for op in &ops {
        let body = op.to_string();
        match route(&incoming(
            "POST",
            "kai://settings/api/apply",
            body.as_bytes(),
        )) {
            Routed::Apply(req) => assert_eq!(serde_json::to_value(&req).unwrap(), *op),
            other => panic!("{op}: {other:?}"),
        }
    }
    // Switching a routine off reaches the core; on is refused (the page does not show routines;
    // the tray and routine_enable show one in full first).
    let off = json!({"op": "routine", "name": "backup", "enabled": false});
    assert!(matches!(
        route(&incoming(
            "POST",
            "kai://settings/api/apply",
            off.to_string().as_bytes()
        )),
        Routed::Apply(Request::Routine { enabled: false, .. })
    ));
    let on = json!({"op": "routine", "name": "backup", "enabled": true});
    let a = answer(route(&incoming(
        "POST",
        "kai://settings/api/apply",
        on.to_string().as_bytes(),
    )));
    assert_eq!(a.status, 403, "{on}");
    assert!(String::from_utf8_lossy(&a.body).contains("Routines menu"));
    // Nothing else is an operation: unknown ones, malformed bodies, other paths and methods.
    for body in [
        &br#"{"op":"quit"}"#[..],
        br#"{"op":"set","key":"a.b"}"#,
        br#"{"op":"tool_state","tool":"x","state":"maybe"}"#,
        b"",
        b"[]",
        b"not json",
    ] {
        let a = answer(route(&incoming("POST", "kai://settings/api/apply", body)));
        assert_eq!(a.status, 400, "{}", String::from_utf8_lossy(body));
    }
    for (method, path, status) in [
        ("GET", "/api/apply", 405),
        ("PUT", "/api/apply", 405),
        ("POST", "/api/tools", 405),
        ("DELETE", "/api/tools", 405),
        ("GET", "/api/quit", 404),
        ("GET", "/api/", 404),
        ("POST", "/api/other", 404),
        ("POST", "/", 404),
        ("POST", "/index.html", 404),
    ] {
        let uri = format!("kai://settings{path}");
        let a = answer(route(&incoming(method, &uri, LIST)));
        assert_eq!(a.status, status, "{method} {path}");
    }
}

#[test]
fn hidden_and_foreign_paths_are_not_files() {
    for path in [
        "/.stage/x",
        "/.hidden",
        "/../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "//index.html",
        "/a//b",
        "/nothing-like-this.js",
    ] {
        let uri = format!("kai://settings{path}");
        let a = answer(route(&incoming("GET", &uri, b"")));
        assert!(a.status == 404 || a.status == 403, "{path}: {}", a.status);
        assert!(a.csp.is_none(), "{path}");
    }
}

#[test]
fn the_tool_list_is_the_contract_and_says_what_the_defaults_are() {
    let all = tools::all();
    let expected: usize = kai_desktop::contract::tools().count();
    assert_eq!(all.len(), expected);
    let get = |n: &str| {
        all.iter()
            .find(|t| t.name == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert_eq!(get("run_command").default_state, "confirm");
    assert_eq!(get("config_set").area, "config");
    assert!(get("config_set").config_tool && !get("run_command").config_tool);
    assert_eq!(get("window_list").default_state, "allow if trusted");
    assert_eq!(get("conversation_start").default_level, "own");
    let mut names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len(), "one row per tool");
    // The route serves exactly that, and the page reads it.
    let a = answer(route(&incoming("GET", "kai://settings/api/tools", b"")));
    assert_eq!(a.status, 200);
    assert_eq!(a.mime, "application/json");
    let read: Vec<wire::ToolInfo> = serde_json::from_slice(&a.body).unwrap();
    assert_eq!(read.len(), all.len());
    assert_eq!(read[0].name, all[0].name);
}

#[test]
fn the_page_reads_what_the_operations_answer_and_sends_what_they_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kai").join("config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[voice]\ngrace_s = 30\n").unwrap();
    // Requests: the page's, read by Kai's.
    let sent = [
        (
            wire::Request::ToolState {
                tool: "run_command".into(),
                state: Some("allow".into()),
            },
            json!({"op": "tool_state", "tool": "run_command", "state": "allow"}),
        ),
        (
            wire::Request::Provider {
                capability: "windows".into(),
                provider: None,
            },
            json!({"op": "provider", "capability": "windows", "provider": null}),
        ),
        (
            wire::Request::Set {
                key: "voice.grace_s".into(),
                value: "45".into(),
            },
            json!({"op": "set", "key": "voice.grace_s", "value": "45"}),
        ),
        (
            wire::Request::List { section: None },
            json!({"op": "list", "section": null}),
        ),
        (
            wire::Request::Unset { key: "a.b".into() },
            json!({"op": "unset", "key": "a.b"}),
        ),
        (
            wire::Request::ToolLevel {
                tool: "run_command".into(),
                level: Some("owner".into()),
            },
            json!({"op": "tool_level", "tool": "run_command", "level": "owner"}),
        ),
    ];
    for (page, expected) in sent {
        let text = serde_json::to_value(&page).unwrap();
        assert_eq!(text, expected);
        let kai: Request = serde_json::from_value(text).unwrap();
        assert_eq!(serde_json::to_value(&kai).unwrap(), expected);
    }
    // Answers: Kai's, read by the page's.
    let listed = settings::apply(&file, Request::List { section: None }).unwrap();
    let wire::Response::Entries { entries } =
        serde_json::from_value(serde_json::to_value(&listed.response).unwrap()).unwrap()
    else {
        panic!("a list answers with entries");
    };
    let grace = entries.iter().find(|e| e.key == "voice.grace_s").unwrap();
    assert_eq!((grace.value.clone(), grace.set), (json!(30), true));
    assert!(grace.choices.is_empty());
    // The page reads a fixed-word setting's words from Kai's answer (its select's options).
    let weather = entries
        .iter()
        .find(|e| e.key == "overlay.weather.position")
        .unwrap();
    assert_eq!(weather.choices.len(), 9);
    assert!(weather.choices.iter().any(|w| w == "bottom-left"));
    assert!(
        entries
            .iter()
            .any(|e| e.key == "desktop.windows" && e.value.is_null() && !e.set)
    );
    let changed = settings::apply(
        &file,
        Request::ToolState {
            tool: "run_command".into(),
            state: serde_json::from_value(json!("allow")).unwrap(),
        },
    )
    .unwrap();
    let read: wire::Response =
        serde_json::from_value(serde_json::to_value(&changed.response).unwrap()).unwrap();
    assert!(matches!(
        read,
        wire::Response::Changed { effect: wire::Effect::Now, ref entry } if entry.key == "policy.tools.run_command" && entry.set
    ));
    let changed = settings::apply(
        &file,
        Request::Set {
            key: "voice.grace_s".into(),
            value: "45".into(),
        },
    )
    .unwrap();
    let read: wire::Response =
        serde_json::from_value(serde_json::to_value(&changed.response).unwrap()).unwrap();
    assert!(matches!(
        read,
        wire::Response::Changed {
            effect: wire::Effect::Restart,
            ..
        }
    ));
    // A refusal's body: what the handler writes, read by the page.
    let refused = Answer::refused(422, "host.keepalive_s: must be at least 1");
    let r: wire::Refused = serde_json::from_slice(&refused.body).unwrap();
    assert!(r.refused.starts_with("host.keepalive_s"));
}

#[test]
fn the_page_offers_the_words_kai_accepts() {
    use crate::desktop::settings::Choice;
    use crate::host::settings::{LevelSetting, ToolState};
    let states = |w: &str| serde_json::from_value::<ToolState>(json!(w)).is_ok();
    let levels = |w: &str| serde_json::from_value::<LevelSetting>(json!(w)).is_ok();
    let providers = |w: &str| serde_json::from_value::<Choice>(json!(w)).is_ok();
    type Check<'a> = (&'a [&'a str], &'a dyn Fn(&str) -> bool);
    let checks: [Check; 3] = [
        (&wire::STATES[..], &states),
        (&wire::LEVELS[..], &levels),
        (&wire::PROVIDERS[..], &providers),
    ];
    for (words, accepted) in checks {
        for w in words {
            assert!(accepted(w), "{w}");
        }
    }
    // Neither side lacks a word of the other: the counts agree.
    assert_eq!(wire::STATES.len(), 5);
    assert_eq!(wire::LEVELS.len(), 4);
    assert_eq!(wire::PROVIDERS.len(), 3);
    // The keys pairing writes are read-only in the page and refused by the operations.
    assert_eq!(wire::READ_ONLY_KEYS[..], *crate::settings::PAIRING_KEYS);
}

#[test]
fn the_csp_names_the_inline_script_by_its_hash_and_nothing_else_inline() {
    let page = br#"<html><head><link rel="stylesheet" href="/a.css"/>
<script type="module">
import init from '/a.js';
</script><script src="/b.js"></script></head></html>"#;
    let csp = assets::csp(page);
    // sha256 of "\nimport init from '/a.js';\n", base64.
    let want = {
        use base64::Engine;
        use sha2::Digest;
        base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(b"\nimport init from '/a.js';\n"))
    };
    assert!(csp.contains(&format!("'sha256-{want}'")), "{csp}");
    assert_eq!(
        csp.matches("sha256-").count(),
        1,
        "the external script has no hash: {csp}"
    );
    for need in [
        "default-src 'none'",
        "script-src 'self' 'wasm-unsafe-eval'",
        "connect-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
        // The profile editor's "Speak a sample": an <audio> on an object URL, nothing else.
        "media-src blob:;",
    ] {
        assert!(csp.contains(need), "{need}: {csp}");
    }
    assert!(
        !csp.contains("'unsafe-inline'") && !csp.contains("'unsafe-eval'"),
        "{csp}"
    );
    assert!(!csp.contains("http") && !csp.contains('*'), "{csp}");
}

#[test]
fn assets_have_their_types_and_a_page_without_a_build_says_so() {
    // The module streams only as application/wasm.
    assert_eq!(super::assets::get("/nothing.wasm").map(|a| a.mime), None);
    let a = answer(route(&incoming("GET", "kai://settings/", b"")));
    if assets::built() {
        assert_eq!(a.status, 200);
        assert_eq!(a.mime, "text/html");
        assert!(
            a.csp
                .as_deref()
                .is_some_and(|c| c.contains("wasm-unsafe-eval"))
        );
    } else {
        assert_eq!(a.status, 404);
        assert!(String::from_utf8_lossy(&a.body).contains("just ui"));
    }
}

#[test]
fn the_window_fits_a_small_screen_and_stays_put_on_a_big_one() {
    assert_eq!(super::fit_size(None), (1100, 760));
    assert_eq!(super::fit_size(Some((2560, 1400))), (1100, 760));
    assert_eq!(super::fit_size(Some((1280, 690))), (1100, 648));
    assert_eq!(super::fit_size(Some((800, 600))), (752, 564));
}

#[test]
fn the_zoom_steps_stay_in_bounds_and_zero_goes_home() {
    assert_eq!(next_scale(1.25, 1), 1.35);
    assert_eq!(next_scale(1.35, -1), 1.25);
    assert_eq!(next_scale(1.7, 0), super::UI_SCALE);
    let mut s = 1.25;
    for _ in 0..100 {
        s = next_scale(s, 1);
    }
    assert_eq!(s, 3.0);
    for _ in 0..100 {
        s = next_scale(s, -1);
    }
    assert_eq!(s, 0.5);
}

/// The Desktop page's remote-control permission (WP16c): the look and the forget reach the core,
/// other methods do not, the origin checks hold, and the page reads what the core answers.
#[test]
fn the_remote_control_permission_is_looked_at_and_forgotten_through_the_core() {
    assert_eq!(
        route(&incoming("GET", "kai://settings/api/remote-control", b"")),
        Routed::RemoteControl { forget: false }
    );
    assert_eq!(
        route(&incoming(
            "POST",
            "kai://settings/api/remote-control/forget",
            b""
        )),
        Routed::RemoteControl { forget: true }
    );
    for (method, path) in [
        ("POST", "/api/remote-control"),
        ("GET", "/api/remote-control/forget"),
        ("DELETE", "/api/remote-control"),
    ] {
        let uri = format!("kai://settings{path}");
        assert_eq!(
            answer(route(&incoming(method, &uri, b""))).status,
            405,
            "{method} {path}"
        );
    }
    let foreign = Incoming {
        origin: Some("kai://app"),
        ..incoming("POST", "kai://settings/api/remote-control/forget", b"")
    };
    assert_eq!(answer(route(&foreign)).status, 403);
    assert_eq!(
        answer(route(&incoming(
            "POST",
            "kai://app/api/remote-control/forget",
            b""
        )))
        .status,
        403
    );

    let dir = tempfile::tempdir().unwrap();
    let file = crate::desktop::fdo::input::token_file(&dir.path().join("state"));
    let read = |r: &super::remote::RemoteControl| -> wire::RemoteControl {
        serde_json::from_slice(&Answer::json(r).body).unwrap()
    };
    let none = super::remote::answer(&file, false).unwrap();
    assert_eq!(
        read(&none),
        wire::RemoteControl {
            stored: false,
            forgot: None
        }
    );
    file.store("token").unwrap();
    assert!(super::remote::answer(&file, false).unwrap().stored);
    let gone = super::remote::answer(&file, true).unwrap();
    assert_eq!(
        read(&gone),
        wire::RemoteControl {
            stored: false,
            forgot: Some(true)
        }
    );
    assert_eq!(
        file.load(),
        None,
        "the file is gone, as --forget-remote-control leaves it"
    );
    let again = super::remote::answer(&file, true).unwrap();
    assert_eq!(again.forgot, Some(false), "nothing left to forget");
}
