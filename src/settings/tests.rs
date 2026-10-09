use std::path::PathBuf;

use serde_json::json;

use super::*;

const FILE: &str = "\
# Kai's configuration: these notes stay.
[voice]
# how long a hold-to-talk session stays open
grace_s = 30   # half a minute
mode = \"automatic\"

[audio]
echo_mode = \"app\"   # the headset echoes

# the last word
";

fn temp(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kai").join("config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    (dir, file)
}

fn run(file: &Path, request: Request) -> Result<Outcome, Refusal> {
    apply(file, request)
}

fn set(file: &Path, key: &str, value: &str) -> Result<Outcome, Refusal> {
    run(
        file,
        Request::Set {
            key: key.into(),
            value: value.into(),
        },
    )
}

fn unset(file: &Path, key: &str) -> Result<Outcome, Refusal> {
    run(file, Request::Unset { key: key.into() })
}

fn text(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

fn entry(file: &Path, key: &str) -> Entry {
    match run(file, Request::Get { key: key.into() })
        .unwrap()
        .response
    {
        Response::Entry { entry } => entry,
        other => panic!("{other:?}"),
    }
}

/// WP21a: a write changes the value and nothing else (§2): the comments above a key, after it and
/// at the end stay, and the core is handed the configuration as it now reads.
#[test]
fn a_round_trip_keeps_the_files_comments() {
    let (_dir, file) = temp(FILE);
    let out = set(&file, "voice.grace_s", "45").unwrap();
    assert_eq!(
        text(&file),
        FILE.replace("grace_s = 30   # half", "grace_s = 45   # half")
    );
    assert_eq!(out.config.unwrap().voice.grace_s, 45);
    assert!(matches!(
        out.response,
        Response::Changed {
            effect: Effect::Restart,
            ..
        }
    ));
    // A key the file lacks joins its section; a section the file lacks is added.
    set(&file, "voice.on_at_start", "true").unwrap();
    set(&file, "shortcuts.hold_ms", "250").unwrap();
    let t = text(&file);
    assert!(t.contains("# how long a hold-to-talk session stays open\ngrace_s = 45"));
    assert!(t.contains("on_at_start = true"));
    assert!(t.contains("[shortcuts]\nhold_ms = 250"));
    assert!(t.contains("# the last word"));
    // An unset removes the key and keeps its neighbours' comments.
    unset(&file, "voice.on_at_start").unwrap();
    unset(&file, "shortcuts.hold_ms").unwrap();
    assert!(!text(&file).contains("on_at_start"));
    assert!(text(&file).contains("# the headset echoes"));
    assert!(text(&file).contains("# the last word"));
    assert_eq!(Config::load(&file).unwrap().voice.grace_s, 45);
}

/// A tool's state is a key under `policy.tools`; the tables on the way carry no empty header, and
/// `default` takes the key away again.
#[test]
fn a_tool_state_is_written_under_policy_tools_and_leaves_with_default() {
    let (_dir, file) = temp(FILE);
    let out = run(
        &file,
        Request::ToolState {
            tool: "config_set".into(),
            state: Some(ToolState::Allow),
        },
    )
    .unwrap();
    assert!(matches!(
        out.response,
        Response::Changed {
            effect: Effect::Now,
            ..
        }
    ));
    assert!(text(&file).contains("[policy.tools]\nconfig_set = \"allow\"\n"));
    assert!(!text(&file).contains("[policy]\n"));
    assert_eq!(
        out.config.unwrap().policy.tools["config_set"],
        ToolState::Allow
    );
    run(
        &file,
        Request::ToolLevel {
            tool: "conversation_start".into(),
            level: Some(LevelSetting::Owner),
        },
    )
    .unwrap();
    assert!(text(&file).contains("[policy.levels]\nconversation_start = \"owner\""));
    run(
        &file,
        Request::ToolState {
            tool: "config_set".into(),
            state: None,
        },
    )
    .unwrap();
    assert!(!text(&file).contains("config_set"));
    run(
        &file,
        Request::Provider {
            capability: "windows".into(),
            provider: Some(Choice::Fdo),
        },
    )
    .unwrap();
    assert!(text(&file).contains("[desktop]\nwindows = \"fdo\""));
}

/// A value the loader would not accept is refused with the loader's message, and the file is
/// byte for byte what it was.
#[test]
fn refusals_say_the_loaders_message_and_write_nothing() {
    let (_dir, file) = temp(FILE);
    for (key, value, says) in [
        (
            "voice.keepalive_s",
            "0",
            "voice.keepalive_s = 0: must be at least 1",
        ),
        ("voice.grace_s", "\"soon\"", "invalid type"),
        ("voice.grace_s", "-3", "invalid value"),
        ("voice.nothing", "1", "unknown field `nothing`"),
        ("audio.echo_mode", "loud", "loud"),
        (
            "policy.trusted_callers",
            "[\"laptop\"]",
            "policy.trusted_callers",
        ),
        ("policy.tools.type_text", "maybe", "maybe"),
        ("host.keepalive_s", "{ a = 1 }", "a table"),
        (
            "voice.grace_s",
            "1\n[policy]\nconfig_tools = true",
            "a value is one line",
        ),
        ("voice", "1", "no setting"),
        ("grace_s", "1", "is no setting"),
        ("voice.\"x\"", "1", "is no setting"),
        ("lmgw.url", "\"http://x\"", "written by pairing"),
    ] {
        let e = set(&file, key, value).unwrap_err().to_string();
        assert!(e.contains(says), "{key} = {value}: {e}");
        assert_eq!(text(&file), FILE, "{key} = {value}");
    }
    for (request, says) in [
        (
            Request::ToolState {
                tool: "no_such_tool".into(),
                state: Some(ToolState::Off),
            },
            "offers no tool no_such_tool",
        ),
        (
            Request::Provider {
                capability: "teleport".into(),
                provider: None,
            },
            "no capability teleport",
        ),
        (
            Request::Get {
                key: "voice".into(),
            },
            "is a section",
        ),
        (
            Request::Get {
                key: "nothing.here".into(),
            },
            "no setting",
        ),
        (
            Request::List {
                section: Some("nothing".into()),
            },
            "no section nothing",
        ),
    ] {
        let e = run(&file, request).unwrap_err().to_string();
        assert!(e.contains(says), "{e}");
    }
    assert_eq!(text(&file), FILE);
    // A file that does not load is not made worse, and the reason is the loader's.
    let (_dir, broken) = temp("[voice]\ngrace_s = \"x\"\n");
    assert!(
        set(&broken, "voice.mode", "push_to_talk")
            .unwrap_err()
            .0
            .contains("grace_s")
    );
}

/// Text that is no TOML value is a string, and so is a text a string-typed key was given as a
/// number; a value that reads as TOML but does not fit falls back to the string only when that
/// fits.
#[test]
fn plain_text_is_a_string() {
    let (_dir, file) = temp(FILE);
    set(&file, "conversation.folder_name", "Home helper").unwrap();
    assert!(text(&file).contains("folder_name = \"Home helper\""));
    set(&file, "audio.input", "5").unwrap();
    assert_eq!(entry(&file, "audio.input").value, json!("5"));
    set(&file, "voice.on_at_start", "true").unwrap();
    assert_eq!(entry(&file, "voice.on_at_start").value, json!(true));
    set(&file, "policy.trusted_callers", "[\"key:laptop\"]").unwrap();
    assert_eq!(
        entry(&file, "policy.trusted_callers").value,
        json!(["key:laptop"])
    );
}

/// The listing and reading tell what is set and what is the default, with the default's value.
#[test]
fn get_and_list_tell_set_from_default() {
    let (_dir, file) = temp(FILE);
    let e = entry(&file, "voice.grace_s");
    assert_eq!((e.value, e.set), (json!(30), true));
    let e = entry(&file, "voice.keepalive_s");
    assert_eq!((e.value, e.set), (json!(15), false));
    let e = entry(&file, "audio.output");
    assert_eq!((e.value, e.set), (json!(null), false));
    let e = entry(&file, "policy.tools.type_text");
    assert_eq!((e.value, e.set), (json!(null), false));
    let Response::Entries { entries } = run(
        &file,
        Request::List {
            section: Some("voice".into()),
        },
    )
    .unwrap()
    .response
    else {
        panic!()
    };
    assert!(entries.iter().all(|e| e.key.starts_with("voice.")));
    assert!(
        entries
            .iter()
            .any(|e| e.key == "voice.conversation_timeout_s")
    );
    assert_eq!(entries.iter().filter(|e| e.set).count(), 2);
    let Response::Entries { entries: all } = run(&file, Request::List { section: None })
        .unwrap()
        .response
    else {
        panic!()
    };
    for section in [
        "lmgw",
        "audio",
        "overlay.text",
        "policy",
        "desktop",
        "search",
        "accessibility",
    ] {
        assert!(
            all.iter()
                .any(|e| e.key.starts_with(&format!("{section}.")))
                || section == "policy",
            "{section}"
        );
    }
    // Unsetting what the file lacks changes nothing.
    let out = unset(&file, "voice.keepalive_s").unwrap();
    assert!(matches!(out.response, Response::Unchanged { .. }));
    assert!(out.config.is_none());
    assert_eq!(text(&file), FILE);
}

/// Nothing secret is readable, listed or writable: the guard by name, and a tripwire over every
/// leaf the configuration has today (a future secret must be decided on, not listed by accident).
#[test]
fn secrets_are_never_returned() {
    let (_dir, file) = temp(FILE);
    for key in [
        "lmgw.api_key",
        "wallet.token",
        "pairing.secret",
        "x.password",
        "a.b.device-key",
        "lmgw.apikey",
        "lmgw.API_KEY",
        "lmgw.apiKey",
        "lmgw.ApiKey",
        "x.passwd",
        "x.PWD",
        "x.auth",
        "x.AUTH_HEADER",
        "x.Bearer-Token",
    ] {
        assert!(secret_key(key), "{key}");
        assert!(
            set(&file, key, "\"hunter2\"")
                .unwrap_err()
                .0
                .contains("never reads or writes"),
            "{key}"
        );
        let e = run(&file, Request::Get { key: key.into() }).unwrap_err().0;
        assert!(e.contains("never read here"), "{e}");
        assert!(unset(&file, key).is_err());
    }
    // A tool's name is not the setting's: `key_press` is a tool.
    assert!(!secret_key("policy.tools.key_press"));
    assert!(!secret_key("policy.tools.KEY_PRESS"));
    assert!(!secret_key("policy.levels.key_press"));
    assert!(!secret_key("host.keepalive_s"));
    assert!(!secret_key("x.author"));
    assert!(!secret_key("x.max_tokens"));
    for (key, _) in tree::leaves(&Config::default()) {
        assert!(
            !secret_key(&key),
            "{key} looks like a secret; decide how it is kept"
        );
    }
    // And the listing hides one that the file could not load anyway.
    let Response::Entries { entries } = run(&file, Request::List { section: None })
        .unwrap()
        .response
    else {
        panic!()
    };
    assert!(entries.iter().all(|e| !secret_key(&e.key)));
}

/// The tools' arguments become requests, and the answers read as compact lines.
#[test]
fn the_tools_map_to_requests_and_answer_in_lines() {
    use kai_desktop::contract::config::AREA;

    use crate::host::registry::Args;
    let args = |tool: &str, v: serde_json::Value| {
        let spec = AREA.tools.iter().find(|t| t.name == tool).unwrap();
        Args::read(spec, v.as_object().cloned()).unwrap()
    };
    let req = |tool: &str, v| tools::request(tool, &args(tool, v));
    assert_eq!(
        req(
            "tool_state_set",
            json!({"tool": "type_text", "state": "Allow"})
        )
        .unwrap(),
        Request::ToolState {
            tool: "type_text".into(),
            state: Some(ToolState::Allow)
        }
    );
    assert_eq!(
        req(
            "tool_state_set",
            json!({"tool": "type_text", "state": "default"})
        )
        .unwrap(),
        Request::ToolState {
            tool: "type_text".into(),
            state: None
        }
    );
    let e = req(
        "tool_level_set",
        json!({"tool": "type_text", "level": "everyone"}),
    )
    .unwrap_err();
    assert!(e.contains("own, owner, trusted, any or default"), "{e}");
    assert_eq!(
        req("config_list", json!({})).unwrap(),
        Request::List { section: None }
    );
    let (_dir, file) = temp(FILE);
    let said = |request| tools::render(&run(&file, request).unwrap().response);
    assert_eq!(
        said(Request::Get {
            key: "voice.grace_s".into()
        }),
        "voice.grace_s = 30"
    );
    assert_eq!(
        said(Request::Get {
            key: "voice.keepalive_s".into()
        }),
        "voice.keepalive_s = 15 (default)"
    );
    assert_eq!(
        said(Request::Get {
            key: "audio.input".into()
        }),
        "audio.input (not set)"
    );
    assert_eq!(
        said(Request::Set {
            key: "voice.grace_s".into(),
            value: "31".into()
        }),
        "voice.grace_s = 31 (applies at Kai's next start)"
    );
    assert_eq!(
        said(Request::Unset {
            key: "voice.grace_s".into()
        }),
        "voice.grace_s = 30 unset, back to the default (applies at Kai's next start)"
    );
}

/// The request and the answers are serde, for the window's scheme handler.
#[test]
fn the_operations_are_serde() {
    let r: Request =
        serde_json::from_value(json!({"op": "set", "key": "voice.grace_s", "value": "5"})).unwrap();
    assert_eq!(
        r,
        Request::Set {
            key: "voice.grace_s".into(),
            value: "5".into()
        }
    );
    let r: Request =
        serde_json::from_value(json!({"op": "tool_state", "tool": "type_text", "state": "off"}))
            .unwrap();
    assert!(matches!(
        r,
        Request::ToolState {
            state: Some(ToolState::Off),
            ..
        }
    ));
    let (_dir, file) = temp(FILE);
    let out = run(
        &file,
        Request::Get {
            key: "voice.grace_s".into(),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&out.response).unwrap(),
        json!({"kind": "entry", "entry": {"key": "voice.grace_s", "value": 30, "set": true}})
    );
}

/// A fixed-word setting carries the words its type takes; any other carries none.
#[test]
fn an_anchor_key_carries_the_nine_positions_and_a_text_key_none() {
    let (_dir, file) = temp(FILE);
    let weather = entry(&file, "overlay.weather.position");
    let positions: Vec<&str> = crate::widgets::kinds::POSITIONS
        .iter()
        .map(|(_, w)| *w)
        .collect();
    assert_eq!(weather.value, json!("top-right"));
    assert_eq!(weather.choices, positions);
    // A number, a text, an unset text and an unset number take any value of their type.
    for key in [
        "voice.grace_s",
        "conversation.folder_name",
        "audio.input",
        "conversation.folder_id",
        "overlay.ring_over_fullscreen",
        "launch.terminal",
    ] {
        assert_eq!(entry(&file, key).choices, Vec::<String>::new(), "{key}");
    }
    // An unset optional enum has its words too, and a map's entry has its value type's.
    assert_eq!(
        entry(&file, "desktop.capture").choices,
        ["plasma", "fdo", "none"]
    );
    set(&file, "policy.tools.run_command", "allow").unwrap();
    assert_eq!(
        entry(&file, "policy.tools.run_command").choices,
        ["allow", "allow_trusted", "confirm", "deny", "off"]
    );
    // On the wire only where there are words.
    let wire = serde_json::to_value(entry(&file, "voice.grace_s")).unwrap();
    assert!(wire.get("choices").is_none(), "{wire}");
}

/// Every enum-typed key of the configuration has choices, and every choice is a word `set` takes
/// (the tripwire for serde's message: if it changes shape, no key has words and this fails).
#[test]
fn every_enum_key_has_its_words_and_set_takes_each() {
    let (_dir, file) = temp(FILE);
    let Response::Entries { entries } = run(&file, Request::List { section: None })
        .unwrap()
        .response
    else {
        panic!("not a list");
    };
    let with: Vec<&Entry> = entries.iter().filter(|e| !e.choices.is_empty()).collect();
    let keys: Vec<&str> = with.iter().map(|e| e.key.as_str()).collect();
    let mut expected = vec![
        "accessibility.switch",
        "approvals.anchor",
        "audio.echo_mode",
        "delegate.screenshots",
        "overlay.app.position",
        "overlay.emote.position",
        "overlay.html.position",
        "overlay.image.position",
        "overlay.text.position",
        "overlay.weather.position",
        "ring.anchor",
        "voice.mode",
    ];
    let desktop: Vec<String> = Capability::ALL
        .iter()
        .map(|c| format!("desktop.{}", c.key()))
        .collect();
    expected.extend(desktop.iter().map(String::as_str));
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(sorted, expected);
    for e in with {
        for word in &e.choices {
            let out = set(&file, &e.key, word);
            assert!(out.is_ok(), "{} = {word}: {out:?}", e.key);
            assert_eq!(entry(&file, &e.key).value, json!(word), "{}", e.key);
        }
    }
}
