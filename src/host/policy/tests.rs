//! K15's table: caller kind × annotations × per-tool state × trusted callers and approvers × the
//! caller level.

use kai_desktop::contract::{Ask, Hints, Kind, Level, Param, ToolSpec};

use super::*;
use crate::host::settings::LevelSetting;

const TEXT: Param = Param::new("text", Kind::Text);
const READ: ToolSpec = ToolSpec {
    name: "clipboard_get",
    description: "The clipboard's text.",
    ..ToolSpec::READ
};
const CHANGE: ToolSpec = ToolSpec {
    name: "clipboard_set",
    description: "Put text on the clipboard.",
    params: &[TEXT],
    ..ToolSpec::CHANGE
};
const DESTRUCTIVE: ToolSpec = ToolSpec {
    name: "suspend",
    description: "Suspend the machine.",
    hints: Hints {
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    },
    ..ToolSpec::CHANGE
};
const OVERLAY: ToolSpec = ToolSpec {
    name: "overlay_text",
    description: "Show text over the desktop.",
    params: &[TEXT],
    ask: Ask::Trusted,
    ..ToolSpec::CHANGE
};
const ALWAYS: ToolSpec = ToolSpec {
    name: "type_text",
    description: "Type text into the focused window.",
    params: &[TEXT],
    ask: Ask::Always,
    ..ToolSpec::CHANGE
};
const OWN: ToolSpec = ToolSpec {
    name: "conversation_start",
    description: "Listen for a conversation until it ends.",
    level: Level::Own,
    ..ToolSpec::CHANGE
};
const OWNER: ToolSpec = ToolSpec {
    name: "config_set",
    description: "Change one of Kai's settings.",
    level: Level::Owner,
    ..ToolSpec::CHANGE
};

fn named(kind: &str, name: &str) -> Caller {
    Caller::Named(Named {
        kind: kind.into(),
        name: name.into(),
    })
}

fn callers() -> Vec<(&'static str, Caller)> {
    vec![
        ("owner", Caller::Owner),
        ("gateway", Caller::Gateway),
        ("own", Caller::Own),
        ("trusted key", named("key", "laptop")),
        ("key", named("key", "script")),
        ("agent", named("agent", "folder-chat")),
        ("device", named("device", "phone")),
        ("unknown", Caller::Unknown(String::new())),
    ]
}

fn policy(edit: impl FnOnce(&mut PolicyConfig)) -> Policy {
    let mut c = PolicyConfig {
        trusted_callers: vec!["key:laptop".into()],
        trusted_approvers: vec!["device:phone".into()],
        ..Default::default()
    };
    edit(&mut c);
    c.validate().unwrap();
    Policy::new(&c)
}

#[derive(Debug, PartialEq, Eq)]
enum D {
    Allow,
    Ask,
    Deny,
}

fn short(d: Decision) -> D {
    match d {
        Decision::Allow => D::Allow,
        Decision::Confirm { .. } => D::Ask,
        Decision::Deny(_) => D::Deny,
    }
}

const TRUSTED: [&str; 4] = ["owner", "gateway", "own", "trusted key"];

/// No state set: K15's defaults by annotations and the contract's default class.
#[test]
fn the_defaults_follow_the_annotations_and_the_caller() {
    let p = policy(|_| {});
    for (who, c) in callers() {
        let trusted = TRUSTED.contains(&who);
        let d = |spec| short(p.decide(spec, &c, None));
        assert_eq!(
            d(&READ),
            if trusted { D::Allow } else { D::Ask },
            "read-only, {who}"
        );
        assert_eq!(d(&CHANGE), D::Ask, "state-changing, {who}");
        assert_eq!(d(&DESTRUCTIVE), D::Ask, "destructive, {who}");
        assert_eq!(
            d(&OVERLAY),
            if trusted { D::Allow } else { D::Ask },
            "overlay (PD4), {who}"
        );
        assert_eq!(d(&ALWAYS), D::Ask, "confirmed by default, {who}");
        assert_eq!(
            d(&OWN),
            if who == "own" { D::Ask } else { D::Deny },
            "Kai's own session only, {who}"
        );
        assert_eq!(
            d(&OWNER),
            if ["owner", "own"].contains(&who) {
                D::Ask
            } else {
                D::Deny
            },
            "owner-only, {who}"
        );
    }
}

/// A trusted approval skips the question (K15, PD4: F decided the call); an untrusted one changes
/// nothing. (`decide` alone; the host drops the approver first where it does not apply,
/// `an_approval_does_not_switch_a_routine_on`.)
#[test]
fn a_trusted_approver_is_not_asked_again() {
    let p = policy(|_| {});
    for (approver, trusted) in [
        (Caller::Owner, true),
        (Caller::Gateway, true),
        (Caller::Own, true),
        (named("device", "phone"), true),
        (named("key", "laptop"), false),
        (named("agent", "x"), false),
        (Caller::Unknown("robot".into()), false),
    ] {
        for (who, c) in callers() {
            let d = |spec| short(p.decide(spec, &c, Some(&approver)));
            // An untrusted approval leaves the plain default: asked; so does a device's approval
            // of its own call (`a_self_approval_is_no_approval`).
            let own_call = approver == c && !TRUSTED[..3].contains(&who);
            let want = if trusted && !own_call {
                D::Allow
            } else {
                D::Ask
            };
            assert_eq!(d(&CHANGE), want, "{approver:?} approved {who}'s change");
            assert_eq!(
                d(&DESTRUCTIVE),
                want,
                "{approver:?} approved {who}'s destructive"
            );
            assert_eq!(
                d(&ALWAYS),
                want,
                "{approver:?} approved {who}'s confirmed-by-default call"
            );
            // The caller level still holds: an approval does not widen it.
            assert_eq!(
                d(&OWN),
                if who != "own" { D::Deny } else { want },
                "{approver:?} approved {who}'s own-only call"
            );
        }
    }
}

/// WP19's self-approval rule: an approval whose approver is the call's own caller is no approval,
/// unless that caller is the owner, the gateway or Kai's own key; another trusted approver's
/// approval of the same call counts as before.
#[test]
fn a_self_approval_is_no_approval() {
    let p = policy(|c| {
        c.trusted_approvers.push("key:laptop".into());
        c.trusted_approvers.push("cli".into());
    });
    for (caller, counts) in [
        (Caller::Owner, true),
        (Caller::Gateway, true),
        (Caller::Own, true),
        (named("device", "phone"), false),
        (named("key", "laptop"), false),
        (Caller::Named(Named::cli()), false),
    ] {
        assert_eq!(
            p.approval_counts(&caller, &caller),
            counts,
            "{caller:?} approving its own call"
        );
        let d = |spec| short(p.decide(spec, &caller, Some(&caller)));
        let want = if counts { D::Allow } else { D::Ask };
        assert_eq!(d(&CHANGE), want, "{caller:?}'s own approval of its change");
        assert_eq!(d(&ALWAYS), want, "{caller:?}'s own approval, K24's class");
        // Another trusted approver decides the same call.
        assert_eq!(
            short(p.decide(&CHANGE, &caller, Some(&Caller::Owner))),
            D::Allow,
            "the owner approved {caller:?}'s call"
        );
    }
    // The same kind with another name is another principal: its approval counts.
    let phone = named("device", "phone");
    assert!(p.approval_counts(&named("device", "tablet"), &phone));
    assert_eq!(
        short(p.decide(&CHANGE, &named("device", "tablet"), Some(&phone))),
        D::Allow
    );
    // An explicit confirm is skipped by another's approval only.
    let confirm = policy(|c| {
        c.tools.insert(CHANGE.name.into(), ToolState::Confirm);
    });
    assert_eq!(short(confirm.decide(&CHANGE, &phone, Some(&phone))), D::Ask);
    assert_eq!(
        short(confirm.decide(&CHANGE, &phone, Some(&Caller::Owner))),
        D::Allow
    );
}

/// A thread's approval (F) by a trusted approver decides K24's confirmed-by-default tools too
/// (K15, PD4 as WP8 built it; restored after WP19's review):
/// a call approved in lmgw names its window, so the approver saw the target. An untrusted
/// approver's changes nothing, and with no approval Kai asks.
#[test]
fn a_trusted_approval_decides_k24s_tools() {
    for name in ["type_text", "key_press", "ui_press", "run_command"] {
        let spec = kai_desktop::contract::find(name).unwrap().1;
        assert_eq!(spec.ask, Ask::Always, "{name}");
        assert!(approval_applies(spec, &Args::default()), "{name}");
    }
    // As the host decides it.
    let decide = |p: &Policy, approver: Option<&Caller>| {
        let approver = approver.filter(|_| approval_applies(&ALWAYS, &Args::default()));
        short(p.decide_call(&ALWAYS, &Args::default(), &Caller::Own, approver))
    };
    let p = policy(|_| {});
    assert_eq!(
        decide(&p, Some(&Caller::Own)),
        D::Allow,
        "Kai's card approved it"
    );
    assert_eq!(decide(&p, Some(&Caller::Owner)), D::Allow, "the dashboard");
    assert_eq!(decide(&p, None), D::Ask, "no approval: K24's dialog");
    assert_eq!(
        decide(&p, Some(&named("key", "laptop"))),
        D::Ask,
        "an untrusted approver"
    );
}

/// A thread's approval (F) does not switch a routine on: `routine_enable {on: true}` is asked in
/// Kai's own dialog, which shows the routine; only `allow` skips it. Off, and every other tool, go
/// by the approval as before.
#[test]
fn an_approval_does_not_switch_a_routine_on() {
    let enable = kai_desktop::contract::find(kai_desktop::contract::routines::ENABLE)
        .unwrap()
        .1;
    let args = |on: bool| {
        let raw = serde_json::json!({"name": "backup", "on": on});
        Args::read(enable, raw.as_object().cloned()).unwrap()
    };
    assert!(!approval_applies(enable, &args(true)));
    assert!(approval_applies(enable, &args(false)));
    assert!(approval_applies(&CHANGE, &Args::default()));
    // As the host decides it: the approver dropped first.
    let decide = |p: &Policy, on: bool| {
        let approver = approval_applies(enable, &args(on)).then_some(&Caller::Owner);
        short(p.decide(enable, &Caller::Owner, approver))
    };
    let p = policy(|_| {});
    assert_eq!(decide(&p, true), D::Ask, "F approved it; Kai still asks");
    assert_eq!(decide(&p, false), D::Allow, "off: F's approval stands");
    let allowed = policy(|c| {
        c.tools.insert(enable.name.to_owned(), ToolState::Allow);
    });
    assert_eq!(
        decide(&allowed, true),
        D::Allow,
        "ruling 28: the owner's allow"
    );
}

/// Trusting cli is said in the tooltip: it reaches every program of the user.
#[test]
fn trusting_cli_is_said() {
    assert!(policy(|_| {}).open_notes().is_empty());
    let p = policy(|c| {
        c.trusted_callers.push("cli".into());
        c.trusted_approvers.push("cli".into());
    });
    let notes = p.open_notes();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes
            .iter()
            .all(|n| n.contains("every program of this user"))
    );
}

/// Ruling 28: every tool can be set to allow, the confirmed-by-default ones included; deny refuses
/// for every caller; confirm asks even the owner; off is not offered.
#[test]
fn the_per_tool_state_overrides_the_default() {
    let p = policy(|c| {
        for (t, s) in [
            ("clipboard_get", ToolState::Confirm),
            ("clipboard_set", ToolState::Allow),
            ("suspend", ToolState::Deny),
            ("type_text", ToolState::Allow),
            ("overlay_text", ToolState::Off),
        ] {
            c.tools.insert(t.into(), s);
        }
    });
    for (who, c) in callers() {
        let d = |spec| short(p.decide(spec, &c, None));
        assert_eq!(d(&READ), D::Ask, "confirm asks {who}");
        assert_eq!(d(&CHANGE), D::Allow, "allow runs for {who}");
        assert_eq!(d(&DESTRUCTIVE), D::Deny, "deny refuses {who}");
        assert_eq!(d(&ALWAYS), D::Allow, "K24's tool allowed for {who}");
        assert_eq!(d(&OVERLAY), D::Deny, "off is no tool for {who}");
    }
    assert!(!p.listed(&OVERLAY));
    assert!(p.listed(&READ) && p.listed(&DESTRUCTIVE));
    // An explicit confirm is still skipped by a trusted approval.
    assert_eq!(
        short(p.decide(&READ, &named("key", "script"), Some(&Caller::Owner))),
        D::Allow
    );
    // Allow does not lift the caller level.
    let p = policy(|c| {
        c.tools
            .insert("conversation_start".into(), ToolState::Allow);
    });
    assert_eq!(short(p.decide(&OWN, &Caller::Owner, None)), D::Deny);
    assert_eq!(short(p.decide(&OWN, &Caller::Own, None)), D::Allow);
}

/// `allow_trusted` (K28's grant of what a routine asks): unasked for the trusted callers, asked for
/// every other unless a trusted approver approved the call, on any tool, a state-changing and a
/// confirmed-by-default one included.
#[test]
fn allow_trusted_runs_for_the_trusted_callers_only() {
    let p = policy(|c| {
        for t in ["clipboard_set", "type_text"] {
            c.tools.insert(t.into(), ToolState::AllowTrusted);
        }
    });
    for (who, c) in callers() {
        let want = if TRUSTED.contains(&who) {
            D::Allow
        } else {
            D::Ask
        };
        for spec in [&CHANGE, &ALWAYS] {
            assert_eq!(
                short(p.decide(spec, &c, None)),
                want,
                "{}, {who}",
                spec.name
            );
        }
    }
    assert_eq!(
        short(p.decide(&CHANGE, &named("key", "script"), Some(&Caller::Owner))),
        D::Allow,
        "a trusted approval decides"
    );
    let c: PolicyConfig = toml::from_str("[tools]\nclipboard_set = \"allow_trusted\"\n").unwrap();
    assert_eq!(c.tools["clipboard_set"], ToolState::AllowTrusted);
}

/// Ruling 28, K27: the caller level is a default the owner can widen (or narrow).
#[test]
fn the_caller_level_can_be_widened() {
    let widened = |level: LevelSetting| {
        policy(|c| {
            c.levels.insert("conversation_start".into(), level);
            c.tools
                .insert("conversation_start".into(), ToolState::Allow);
        })
    };
    let reach = |p: &Policy| -> Vec<&'static str> {
        callers()
            .into_iter()
            .filter(|(_, c)| short(p.decide(&OWN, c, None)) == D::Allow)
            .map(|(w, _)| w)
            .collect()
    };
    assert_eq!(reach(&widened(LevelSetting::Own)), ["own"]);
    assert_eq!(
        reach(&widened(LevelSetting::Owner)),
        ["owner", "gateway", "own"]
    );
    assert_eq!(
        reach(&widened(LevelSetting::Trusted)),
        ["owner", "gateway", "own", "trusted key"]
    );
    assert_eq!(reach(&widened(LevelSetting::Any)).len(), callers().len());
    // Narrowed: a read-only tool for Kai only.
    let p = policy(|c| {
        c.levels.insert("clipboard_get".into(), LevelSetting::Own);
    });
    let Decision::Deny(why) = p.decide(&READ, &Caller::Owner, None) else {
        panic!("the owner is below own");
    };
    assert!(
        why.contains("policy.levels.clipboard_get = \"own\"") && why.contains("the owner"),
        "{why}"
    );
}

#[test]
fn the_caller_reads_from_the_meta() {
    // As lmgw stamps them: the kind read by lmgw-client (`CallerKind`, an unknown kind as sent).
    let p = |kind: &str, name: &str| -> Principal {
        serde_json::from_value(serde_json::json!({"kind": kind, "name": name})).unwrap()
    };
    let own = Some("desktop");
    assert_eq!(
        Caller::from_principal(Some(&p("owner", "owner")), own),
        Caller::Owner
    );
    assert_eq!(
        Caller::from_principal(Some(&p("gateway", "")), own),
        Caller::Gateway
    );
    assert_eq!(
        Caller::from_principal(Some(&p("device", "desktop")), own),
        Caller::Own
    );
    // Until the feed named Kai's device, its own calls are a device's.
    assert_eq!(
        Caller::from_principal(Some(&p("device", "desktop")), None),
        named("device", "desktop")
    );
    assert_eq!(
        Caller::from_principal(Some(&p("key", "desktop")), own),
        named("key", "desktop"),
        "a key named like Kai's device is not Kai"
    );
    assert_eq!(
        Caller::from_principal(Some(&p("robot", "r")), own),
        Caller::Unknown("robot".into())
    );
    assert_eq!(
        Caller::from_principal(None, own),
        Caller::Unknown(String::new())
    );
    // lmgw stamps a caller without a credential as anonymous: the least trusted, like an unknown
    // kind.
    let anonymous = Caller::from_principal(Some(&p("anonymous", "")), own);
    assert_eq!(anonymous, Caller::Unknown("anonymous".into()));
    let pol = policy(|_| {});
    assert_eq!(short(pol.decide(&READ, &anonymous, None)), D::Ask);
    assert_eq!(short(pol.decide(&OWNER, &anonymous, None)), D::Deny);
    assert_eq!(
        short(pol.decide(&CHANGE, &named("key", "script"), Some(&anonymous))),
        D::Ask,
        "an anonymous approver approves nothing"
    );
    assert_eq!(named("agent", "x").describe(), "agent 'x'");
    assert_eq!(
        named("key", "a\nthe owner").describe(),
        r#"key "a\nthe owner""#,
        "a name cannot forge a line"
    );
    assert_eq!(anonymous.describe(), "an anonymous caller");
}

#[test]
fn names_the_policy_holds_that_no_tool_has_are_found() {
    let p = policy(|c| {
        c.tools.insert("clipboard_get".into(), ToolState::Allow);
        c.tools.insert("clipbaord_set".into(), ToolState::Allow);
        c.levels.insert("nothing".into(), LevelSetting::Any);
    });
    let offered: HashSet<&str> = ["clipboard_get", "clipboard_set"].into();
    assert_eq!(
        p.unknown_names(&offered),
        ["policy.levels.nothing", "policy.tools.clipbaord_set"]
    );
}

/// An MCP app's tool (`<server>__<tool>`) in `policy.tools` is checked when its view opens, not at
/// the host's start; in `policy.levels` it does nothing (no level applies to an app's tools).
#[test]
fn an_apps_tool_names_are_exempt_only_in_policy_tools() {
    let p = policy(|c| {
        c.tools.insert("fx__bump".into(), ToolState::Allow);
        c.levels.insert("fx__bump".into(), LevelSetting::Any);
        c.tools.insert("clipboard_get".into(), ToolState::Allow);
    });
    let offered: HashSet<&str> = ["clipboard_get"].into();
    assert_eq!(p.unknown_names(&offered), ["policy.levels.fx__bump"]);
    assert_eq!(p.app_tool_names(), ["fx__bump"]);
}

/// K30: each configuration tool is for the owner and Kai's own key; the gateway's runs (dashboard
/// turns, in-process agents, internal keys) and every other caller are refused.
#[test]
fn the_config_tools_admit_only_the_owner_and_kais_own_key() {
    let p = policy(|_| {});
    for spec in kai_desktop::contract::config::AREA.tools {
        for (who, c) in callers() {
            let d = short(p.decide(spec, &c, None));
            if ["owner", "own"].contains(&who) {
                assert_ne!(d, D::Deny, "{} for {who}", spec.name);
            } else {
                assert_eq!(d, D::Deny, "{} for {who}", spec.name);
            }
        }
    }
}

/// Widening a configuration tool's level (ruling 28) lets the gateway in again.
#[test]
fn a_widened_config_tool_admits_the_gateway() {
    let spec = &kai_desktop::contract::config::AREA.tools[0];
    let p = policy(|c| {
        c.levels.insert(spec.name.into(), LevelSetting::Trusted);
    });
    assert_ne!(short(p.decide(spec, &Caller::Gateway, None)), D::Deny);
}

/// The gateway's exclusion is the configuration tools' own; another owner-level tool keeps K15's
/// reading (the owner, the gateway on its behalf, Kai's own key).
#[test]
fn another_owner_level_tool_still_admits_the_gateway() {
    let spec = ToolSpec {
        name: "some_owner_tool",
        ..OWNER
    };
    let p = policy(|_| {});
    assert_ne!(short(p.decide(&spec, &Caller::Gateway, None)), D::Deny);
}

/// lmgw's stamp as `server.rs` reads it (lmgw-client's `CallMeta::from_meta`) into K15's callers:
/// each part on its own, a missing or unreadable caller unknown, absent `timeout_ms` no deadline.
#[test]
fn the_meta_reads_as_lmgw_stamps_it() {
    use lmgw_client::mcp_host::CallMeta;
    use serde_json::json;

    let read = |v: serde_json::Value| CallMeta::from_meta(v.as_object().unwrap());
    let own = Some("desktop");
    let m = read(json!({
        "lmgw/caller": {"kind": "key", "name": "laptop"},
        "lmgw/approval": {"decision": "approved", "by": {"kind": "owner", "name": "owner"}},
        "lmgw/timeout_ms": 60000,
        "progressToken": 7
    }));
    assert_eq!(
        Caller::from_principal(m.caller.as_ref(), own),
        named("key", "laptop")
    );
    let by = &m.approval.as_ref().unwrap().by;
    assert_eq!(Caller::from_principal(Some(by), own), Caller::Owner);
    assert_eq!(m.timeout_ms, Some(60000));
    let m = read(json!({"lmgw/approval": null, "lmgw/caller": "owner"}));
    assert_eq!(
        m,
        CallMeta::default(),
        "null and unreadable parts are absent"
    );
    assert_eq!(
        Caller::from_principal(m.caller.as_ref(), own),
        Caller::Unknown(String::new())
    );
    // A missing caller keeps the other parts.
    let m = read(json!({
        "lmgw/approval": {"decision": "approved", "by": {"kind": "owner", "name": "o"}},
        "lmgw/timeout_ms": 5000
    }));
    assert_eq!(
        (m.caller.is_none(), m.approval.is_some(), m.timeout_ms),
        (true, true, Some(5000))
    );
    // A caller without a name does not read: an unknown caller, the safe side.
    let m = read(json!({"lmgw/caller": {"kind": "owner"}}));
    assert_eq!(
        Caller::from_principal(m.caller.as_ref(), own),
        Caller::Unknown(String::new())
    );
    let m = read(json!({"lmgw/caller": {"kind": "anonymous", "name": ""}}));
    assert_eq!(
        Caller::from_principal(m.caller.as_ref(), own),
        Caller::Unknown("anonymous".into())
    );
    assert_eq!(m.timeout_ms, None, "absent is no deadline");
}
