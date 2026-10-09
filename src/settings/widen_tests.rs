//! The widening guard (owner's decision, 2026-10-09): a write through the config tools that
//! widens a permission is refused unless the owner permitted it for that tool; narrowing always
//! passes; the opt-in is not the tools' to set; the owner's own UI is not guarded. An unset returns
//! to the default Kai really applies (a routine's from its loaded spec); a name Kai does not know
//! is judged conservatively; a command Kai runs (`launch.terminal`) is `run_command`'s.

use std::path::{Path, PathBuf};

use super::*;

fn temp(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kai").join("config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    (dir, file)
}

fn tool(file: &Path, request: Request) -> Result<Outcome, Refusal> {
    apply_from(file, request, Source::Tool)
}

fn state(t: &str, s: Option<ToolState>) -> Request {
    Request::ToolState {
        tool: t.into(),
        state: s,
    }
}

fn level(t: &str, l: Option<LevelSetting>) -> Request {
    Request::ToolLevel {
        tool: t.into(),
        level: l,
    }
}

fn set(key: &str, value: &str) -> Request {
    Request::Set {
        key: key.into(),
        value: value.into(),
    }
}

fn text(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

#[test]
fn widening_is_refused_by_default_and_names_how_to_permit_it() {
    let (_d, file) = temp("");
    let before = text(&file);
    // run_command is confirmed by default: allow widens it. A level toward any widens too.
    let e = tool(&file, state("run_command", Some(ToolState::Allow))).unwrap_err();
    assert!(
        e.0.contains("policy.widen_by_tools.run_command = true") && e.0.contains("settings window"),
        "{e}"
    );
    let e = tool(&file, level("config_set", Some(LevelSetting::Any))).unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.config_set"), "{e}");
    // The same through the generic key.
    let e = tool(&file, set("policy.tools.run_command", "allow")).unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.run_command"), "{e}");
    // Trust: a list entry, config_tools on.
    for (key, value) in [
        ("policy.trusted_callers", "[\"key:laptop\"]"),
        ("policy.trusted_approvers", "[\"device:phone\"]"),
        ("policy.config_tools", "true"),
    ] {
        let e = tool(&file, set(key, value)).unwrap_err();
        assert!(
            e.0.contains("policy.widen_trust_by_tools = true"),
            "{key}: {e}"
        );
    }
    assert_eq!(text(&file), before, "a refusal writes nothing");
}

#[test]
fn the_owner_permits_widening_per_tool() {
    let (_d, file) = temp("[policy.widen_by_tools]\nrun_command = true\n");
    // Permitted for run_command: its state may widen.
    tool(&file, state("run_command", Some(ToolState::Allow))).unwrap();
    // A level: config_set is owner-level; any widens it, refused without the opt-in, then passes.
    let e = tool(&file, level("config_set", Some(LevelSetting::Any))).unwrap_err();
    assert!(
        e.0.contains("policy.levels.config_set: the default -> any"),
        "{e}"
    );
    let (_d, file) = temp("[policy.widen_by_tools]\nconfig_set = true\n");
    tool(&file, level("config_set", Some(LevelSetting::Any))).unwrap();
    assert!(text(&file).contains("config_set = \"any\""));
    let (_d, file) = temp("[policy.widen_by_tools]\nrun_command = true\n");
    // Not for another tool.
    let e = tool(&file, state("type_text", Some(ToolState::Allow))).unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.type_text"), "{e}");
    // A false is no permission.
    let (_d, file) = temp("[policy.widen_by_tools]\nrun_command = false\n");
    assert!(tool(&file, state("run_command", Some(ToolState::Allow))).is_err());
    // Trust has its own permission.
    let (_d, file) = temp("[policy]\nwiden_trust_by_tools = true\n");
    tool(&file, set("policy.trusted_callers", "[\"key:laptop\"]")).unwrap();
    tool(&file, set("policy.config_tools", "true")).unwrap();
    assert!(tool(&file, state("run_command", Some(ToolState::Allow))).is_err());
}

#[test]
fn narrowing_always_passes() {
    let (_d, file) = temp(
        "[policy]\ntrusted_callers = [\"key:laptop\", \"agent:a\"]\nconfig_tools = true\n\
         [policy.tools]\nrun_command = \"allow\"\nclipboard_get = \"allow\"\n\
         [policy.levels]\nrun_command = \"any\"\nconfig_set = \"any\"\n",
    );
    tool(&file, state("run_command", Some(ToolState::Confirm))).unwrap();
    tool(&file, state("run_command", Some(ToolState::Deny))).unwrap();
    tool(&file, state("run_command", Some(ToolState::Off))).unwrap();
    tool(&file, level("run_command", Some(LevelSetting::Owner))).unwrap();
    // Back to the default (confirm for run_command) narrows from allow; so does an unset level.
    tool(&file, state("clipboard_get", None)).unwrap();
    tool(&file, level("config_set", None)).unwrap();
    tool(&file, set("policy.trusted_callers", "[\"key:laptop\"]")).unwrap();
    tool(&file, set("policy.config_tools", "false")).unwrap();
    tool(
        &file,
        Request::Unset {
            key: "policy.trusted_callers".into(),
        },
    )
    .unwrap();
    // Not a permission at all.
    tool(&file, set("voice.grace_s", "45")).unwrap();
}

#[test]
fn a_default_that_is_wider_counts_when_it_returns() {
    // clipboard_get is allowed for trusted callers by default; a deny narrows below it, and
    // taking the deny away widens back to the default.
    let (_d, file) = temp("[policy.tools]\nclipboard_get = \"deny\"\n");
    let e = tool(&file, state("clipboard_get", None)).unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.clipboard_get"), "{e}");
    let e = tool(&file, state("clipboard_get", Some(ToolState::Confirm))).unwrap_err();
    assert!(e.0.contains("widens"), "{e}");
}

#[test]
fn the_tools_cannot_set_the_opt_in() {
    let (_d, file) = temp("");
    let before = text(&file);
    for request in [
        set("policy.widen_by_tools.run_command", "true"),
        set("policy.widen_trust_by_tools", "true"),
    ] {
        let e = tool(&file, request).unwrap_err();
        assert!(e.0.contains("only the owner permits widening"), "{e}");
    }
    assert_eq!(text(&file), before);
    // Revoking is narrowing: false and unset pass.
    let (_d, file) = temp(
        "[policy.widen_by_tools]\nrun_command = true\n[policy]\nwiden_trust_by_tools = true\n",
    );
    tool(&file, set("policy.widen_by_tools.run_command", "false")).unwrap();
    tool(&file, set("policy.widen_trust_by_tools", "false")).unwrap();
}

#[test]
fn the_owners_own_ui_widens_freely() {
    let (_d, file) = temp("");
    apply(&file, state("run_command", Some(ToolState::Allow))).unwrap();
    apply(&file, level("run_command", Some(LevelSetting::Any))).unwrap();
    apply(&file, set("policy.trusted_callers", "[\"key:laptop\"]")).unwrap();
    apply(&file, set("policy.config_tools", "true")).unwrap();
    // The opt-in is the owner's to set: here, and it then lets the tools widen that tool.
    apply(&file, set("policy.widen_by_tools.type_text", "true")).unwrap();
    let c = Config::load(&file).unwrap();
    assert!(c.policy.widen_by_tools["type_text"]);
    assert!(
        entry_value(&file, "policy.widen_by_tools.type_text"),
        "listed like any key"
    );
    tool(&file, state("type_text", Some(ToolState::Allow))).unwrap();
}

fn entry_value(file: &Path, key: &str) -> bool {
    match apply(file, Request::Get { key: key.into() })
        .unwrap()
        .response
    {
        Response::Entry { entry } => entry.value == serde_json::json!(true),
        other => panic!("{other:?}"),
    }
}

/// A spec as a routine with `routine.toml` `text` loads (its defaults capped, `routines::def`).
fn routine_spec(name: &str, text: &str) -> &'static kai_desktop::contract::ToolSpec {
    crate::routines::def::parse(std::path::Path::new("/nowhere"), name, text)
        .unwrap()
        .spec
}

fn policy(text: &str) -> Config {
    Config::parse(text, Path::new("kai.toml")).unwrap()
}

/// `old -> new` through the guard, with `backup` and friends as loaded routines.
fn routine_check(old: &str, new: &str) -> Result<(), Refusal> {
    const STEPS: &str = "description = \"x.\"\nkind = \"steps\"\n[[steps]]\ntool = \"notify\"\n";
    let backup = routine_spec("backup", STEPS);
    let trusted = routine_spec("trusted_one", &format!("policy = \"trusted\"\n{STEPS}"));
    let reader = routine_spec("reader", &format!("read_only = true\n{STEPS}"));
    let own = routine_spec("own_one", &format!("level = \"own\"\n{STEPS}"));
    let spec_of = move |n: &str| match n {
        "backup" => Some(backup),
        "trusted_one" => Some(trusted),
        "reader" => Some(reader),
        "own_one" => Some(own),
        other => widen::offered(other),
    };
    widen::check(&policy(old), &policy(new), &spec_of)
}

/// A routine's default is its loaded spec's: at most confirm and the owner's level, whatever
/// `routine.toml` asks (level unset, `policy = "trusted"`, `read_only`). So unsetting an owner's
/// entry returns to that, and setting what it asks is a widening.
#[test]
fn a_routines_default_is_its_loaded_spec() {
    // Level unset in routine.toml (it asks any): Kai's default is owner.
    routine_check("[policy.levels]\nbackup = \"owner\"\n", "").unwrap();
    let e = routine_check("[policy.levels]\nbackup = \"own\"\n", "").unwrap_err();
    assert!(
        e.0.contains("policy.levels.backup: own -> the default"),
        "{e}"
    );
    let e = routine_check("", "[policy.levels]\nbackup = \"any\"\n").unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.backup"), "{e}");
    let e = routine_check("", "[policy.levels]\nbackup = \"trusted\"\n").unwrap_err();
    assert!(e.0.contains("widens who may call backup"), "{e}");
    // Narrowing an owner's any to trusted passes; unsetting it back to the default narrows.
    routine_check(
        "[policy.levels]\nbackup = \"any\"\n",
        "[policy.levels]\nbackup = \"trusted\"\n",
    )
    .unwrap();
    routine_check("[policy.levels]\nbackup = \"any\"\n", "").unwrap();
    // policy = "trusted" and read_only: asked, not given; the default is confirm.
    for name in ["trusted_one", "reader"] {
        routine_check(&format!("[policy.tools]\n{name} = \"confirm\"\n"), "").unwrap();
        let e = routine_check("", &format!("[policy.tools]\n{name} = \"allow_trusted\"\n"))
            .unwrap_err();
        assert!(
            e.0.contains(&format!("policy.widen_by_tools.{name}")),
            "{e}"
        );
        let e = routine_check(&format!("[policy.tools]\n{name} = \"deny\"\n"), "").unwrap_err();
        assert!(e.0.contains("deny -> the default"), "{e}");
    }
    routine_check("[policy.tools]\nreader = \"allow_trusted\"\n", "").unwrap();
    // level = "own" is honoured as written: owner widens it.
    assert!(routine_check("", "[policy.levels]\nown_one = \"owner\"\n").is_err());
    // The owner's opt-in permits it, as for any tool.
    routine_check(
        "[policy.widen_by_tools]\nbackup = true\n",
        "[policy.widen_by_tools]\nbackup = true\n[policy.levels]\nbackup = \"any\"\n",
    )
    .unwrap();
}

/// A name Kai offers no tool for (a routine not written yet, a typo): any entry but deny or off is
/// a widening, and so is taking an entry away.
#[test]
fn a_name_kai_does_not_know_is_judged_conservatively() {
    let check = |old: &str, new: &str| widen::check(&policy(old), &policy(new), &|_| None);
    for (old, new) in [
        ("", "[policy.tools]\nlater = \"confirm\"\n"),
        ("", "[policy.tools]\nlater = \"allow\"\n"),
        ("[policy.tools]\nlater = \"deny\"\n", ""),
        ("", "[policy.levels]\nlater = \"owner\"\n"),
        ("[policy.levels]\nlater = \"own\"\n", ""),
    ] {
        let e = check(old, new).unwrap_err();
        assert!(
            e.0.contains("Kai offers no tool of that name now"),
            "{old} -> {new}: {e}"
        );
    }
    for (old, new) in [
        ("", "[policy.tools]\nlater = \"deny\"\n"),
        ("", "[policy.tools]\nlater = \"off\"\n"),
        ("", "[policy.levels]\nlater = \"own\"\n"),
        (
            "[policy.tools]\nlater = \"allow\"\n",
            "[policy.tools]\nlater = \"confirm\"\n",
        ),
        // Nothing is wider than allow.
        ("[policy.tools]\nlater = \"allow\"\n", ""),
    ] {
        check(old, new).unwrap();
    }
    // Through the operations: config_set and config_unset of such a name.
    let (_d, file) = temp("[policy.tools]\nnot_a_tool_yet = \"deny\"\n");
    let e = tool(
        &file,
        Request::Unset {
            key: "policy.tools.not_a_tool_yet".into(),
        },
    )
    .unwrap_err();
    assert!(e.0.contains("policy.widen_by_tools.not_a_tool_yet"), "{e}");
    let e = tool(&file, set("policy.levels.not_a_tool_yet", "any")).unwrap_err();
    assert!(e.0.contains("widens who may call"), "{e}");
}

/// `launch.terminal` is run before an application's command line (`app_launch`): writing it is
/// `run_command`'s power, permitted by run_command's opt-in only.
#[test]
fn a_command_kai_runs_is_run_commands_to_change() {
    let (_d, file) = temp("");
    let before = text(&file);
    let e = tool(
        &file,
        set("launch.terminal", "[\"sh\", \"-c\", \"id\", \"--\"]"),
    )
    .unwrap_err();
    assert!(
        e.0.contains("launch.terminal") && e.0.contains("policy.widen_by_tools.run_command"),
        "{e}"
    );
    assert_eq!(text(&file), before);
    let (_d, file) = temp("[launch]\nterminal = [\"konsole\", \"-e\"]\n");
    let e = tool(
        &file,
        Request::Unset {
            key: "launch.terminal".into(),
        },
    )
    .unwrap_err();
    assert!(e.0.contains("run_command"), "{e}");
    // The other launch keys are no command.
    tool(&file, set("launch.command_timeout_s", "30")).unwrap();
    // Permitted for run_command, or the owner's own UI.
    let (_d, file) = temp("[policy.widen_by_tools]\nrun_command = true\n");
    tool(&file, set("launch.terminal", "[\"konsole\", \"-e\"]")).unwrap();
    let (_d, file) = temp("");
    apply(&file, set("launch.terminal", "[\"konsole\", \"-e\"]")).unwrap();
}

/// A permission-like boolean (`overlay.fetch_private_addresses`, on by default) follows the trust
/// opt-in: switching it on, or unsetting it while it is off, is refused; switching it off passes.
#[test]
fn a_permission_flag_widens_when_it_goes_on() {
    let (_d, file) = temp("[overlay]\nfetch_private_addresses = false\n");
    let e = tool(&file, set("overlay.fetch_private_addresses", "true")).unwrap_err();
    assert!(e.0.contains("fetch_private_addresses"), "{}", e.0);
    assert!(
        e.0.contains("policy.widen_trust_by_tools = true"),
        "{}",
        e.0
    );
    // Back to its default (on) widens as well.
    let e = tool(
        &file,
        Request::Unset {
            key: "overlay.fetch_private_addresses".into(),
        },
    )
    .unwrap_err();
    assert!(e.0.contains("fetch_private_addresses"), "{}", e.0);
    assert!(text(&file).contains("false"));
    // Narrowing passes, from the default too; an unrelated key passes.
    let (_d, on) = temp("");
    tool(&on, set("overlay.fetch_private_addresses", "false")).unwrap();
    tool(&on, set("overlay.fetch_timeout_s", "20")).unwrap();
    // The trust opt-in permits it.
    let (_d, ok) =
        temp("[policy]\nwiden_trust_by_tools = true\n[overlay]\nfetch_private_addresses = false\n");
    tool(&ok, set("overlay.fetch_private_addresses", "true")).unwrap();
    // The owner's own UI is not guarded.
    let (_d, own) = temp("[overlay]\nfetch_private_addresses = false\n");
    apply(&own, set("overlay.fetch_private_addresses", "true")).unwrap();
}

/// `accessibility.switch` (K33) widens toward `conversation`, step by step; narrowing passes.
#[test]
fn the_accessibility_switch_widens_toward_conversation() {
    let (_d, file) = temp("[accessibility]\nswitch = \"never\"\n");
    for to in ["read", "conversation"] {
        let e = tool(&file, set("accessibility.switch", &format!("\"{to}\""))).unwrap_err();
        assert!(e.0.contains("accessibility.switch"), "{}", e.0);
        assert!(
            e.0.contains("policy.widen_trust_by_tools = true"),
            "{}",
            e.0
        );
    }
    let (_d, read) = temp("[accessibility]\nswitch = \"read\"\n");
    let e = tool(&read, set("accessibility.switch", "\"conversation\"")).unwrap_err();
    assert!(e.0.contains("conversation mode"), "{}", e.0);
    // Unset returns to the default, conversation: wider than read.
    assert!(
        tool(
            &read,
            Request::Unset {
                key: "accessibility.switch".into()
            }
        )
        .is_err()
    );
    // Narrowing passes.
    let (_d, on) = temp("");
    tool(&on, set("accessibility.switch", "\"read\"")).unwrap();
    tool(&on, set("accessibility.switch", "\"never\"")).unwrap();
    // The trust opt-in permits it; the owner's own UI is not guarded.
    let (_d, ok) =
        temp("[policy]\nwiden_trust_by_tools = true\n[accessibility]\nswitch = \"never\"\n");
    tool(&ok, set("accessibility.switch", "\"conversation\"")).unwrap();
    let (_d, own) = temp("[accessibility]\nswitch = \"never\"\n");
    apply(&own, set("accessibility.switch", "\"conversation\"")).unwrap();
}
