//! K30's guard (owner's decision, 2026-10-09): a write through the owner configuration tools that
//! WIDENS a permission is refused unless the owner permitted it for that tool.
//!
//! What widens: a tool's state toward `allow` (`policy.tools.<tool>`, also by unsetting it when the
//! default is wider), a tool's level toward `any` (`policy.levels.<tool>`), an entry added to
//! `policy.trusted_callers` or `policy.trusted_approvers`, `policy.config_tools` switched on, a
//! permission-like boolean switched on ([`PERMISSION_FLAGS`]: `overlay.fetch_private_addresses`,
//! also by unsetting it while it is off, its default being on; the trust opt-in covers it;
//! `accessibility.switch` toward `conversation`, K33, counts as two such steps), and a
//! change of a key whose value Kai runs as a command ([`COMMANDS`]: `launch.terminal`, which
//! `app_launch` puts before an application's command line, so writing it is `run_command`'s
//! power). Narrowing writes always pass, and so does every other key.
//!
//! **The default an unset entry returns to** is the tool's as Kai offers it now: the contract's for
//! Kai's own tools, the loaded spec for a routine (`crate::routines::spec`, which holds at most
//! confirm and the owner's level, whatever its `routine.toml` asks: `crate::routines::def`). A name
//! Kai knows neither way (a routine not written yet, a typo) is judged conservatively: its default
//! counts as the narrowest before a write and as the widest after it, so setting anything but
//! `deny` or `off` on it, or taking its entry away, widens.
//!
//! The owner's permission is `policy.widen_by_tools.<tool> = true` for a tool's state and level
//! (and `run_command`'s for [`COMMANDS`]), and `policy.widen_trust_by_tools = true` for the trust
//! lists, `config_tools` and those booleans. Neither can be switched on by the tools (that would be
//! self-widening): they are the owner's, through the settings window or the file. The window and
//! the tray are the owner's own UI and are not guarded.

use kai_desktop::contract::{self, Ask, ToolSpec};

use super::Refusal;
use crate::config::Config;
use crate::desktop::fdo::atspi::switch::SwitchMode;
use crate::host::settings::{LevelSetting, ToolState};

/// Booleans that are a permission when on, with the words said about each: switching one on
/// widens trust like a trust list's entry (`policy.widen_trust_by_tools`). The loaded value is the
/// effective one, so unsetting a key whose default is on counts as switching it on.
type PermissionFlag = (&'static str, fn(&Config) -> bool, &'static str);
const PERMISSION_FLAGS: &[PermissionFlag] = &[
    (
        "overlay.fetch_private_addresses",
        |c| c.overlay.fetch_private_addresses,
        "lets the image and page fetches reach loopback, link-local and LAN addresses",
    ),
    // `accessibility.switch` (K33) as two steps: `never` to `read`, and on to `conversation`.
    (
        "accessibility.switch",
        |c| c.accessibility.switch != SwitchMode::Never,
        "lets Kai switch org.a11y.Status IsEnabled on around its reads (every Qt app's tree on the \
         accessibility bus meanwhile)",
    ),
    (
        "accessibility.switch",
        |c| c.accessibility.switch == SwitchMode::Conversation,
        "lets Kai hold org.a11y.Status IsEnabled on through conversation mode (every Qt app's tree \
         on the accessibility bus meanwhile)",
    ),
];

/// Keys whose value Kai runs as a command, with the tool whose power writing them is.
pub const COMMANDS: &[(&str, &str)] = &[("launch.terminal", "run_command")];

/// The value of a [`COMMANDS`] key in `c`.
fn command_value(c: &Config, key: &str) -> Option<Option<Vec<String>>> {
    match key {
        "launch.terminal" => Some(c.launch.terminal.clone()),
        _ => None,
    }
}

/// The spec an unset entry of `name` returns to (module doc): Kai's own tool, or a routine as
/// loaded now.
pub(super) fn offered(name: &str) -> Option<&'static ToolSpec> {
    contract::find(name)
        .map(|(_, spec)| spec)
        .or_else(|| crate::routines::spec(name))
}

/// Which side of the write a value is on: a default Kai does not know is the narrowest before
/// and the widest after (module doc).
#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

impl Side {
    fn unknown(self) -> u8 {
        match self {
            Side::Before => 0,
            Side::After => 3,
        }
    }
}

/// How wide a tool's state is: off and deny run nothing, confirm asks, allow-if-trusted runs for
/// the trusted callers, allow runs for every caller the level admits.
fn state_rank(spec: Option<&ToolSpec>, state: Option<ToolState>, side: Side) -> u8 {
    match state {
        Some(ToolState::Off | ToolState::Deny) => 0,
        Some(ToolState::Confirm) => 1,
        Some(ToolState::AllowTrusted) => 2,
        Some(ToolState::Allow) => 3,
        // `Policy::decide`'s `None` arms, in their order.
        None => match spec {
            Some(spec) => match spec.ask {
                Ask::Always => 1,
                Ask::Trusted => 2,
                Ask::Hints if spec.hints.read_only => 2,
                Ask::Hints => 1,
            },
            None => side.unknown(),
        },
    }
}

fn level_rank(spec: Option<&ToolSpec>, level: Option<LevelSetting>, side: Side) -> u8 {
    match level {
        Some(LevelSetting::Own) => 0,
        Some(LevelSetting::Owner) => 1,
        Some(LevelSetting::Trusted) => 2,
        Some(LevelSetting::Any) => 3,
        None => match spec {
            Some(spec) => match spec.level {
                contract::Level::Own => 0,
                contract::Level::Owner => 1,
                contract::Level::Trusted => 2,
                contract::Level::Any => 3,
            },
            None => side.unknown(),
        },
    }
}

/// The words of a state (`allow`, `default`).
fn state_word(s: Option<ToolState>) -> String {
    match s {
        Some(ToolState::Allow) => "allow",
        Some(ToolState::AllowTrusted) => "allow_trusted",
        Some(ToolState::Confirm) => "confirm",
        Some(ToolState::Deny) => "deny",
        Some(ToolState::Off) => "off",
        None => "the default",
    }
    .to_owned()
}

fn level_word(l: Option<LevelSetting>) -> String {
    match l {
        Some(LevelSetting::Own) => "own",
        Some(LevelSetting::Owner) => "owner",
        Some(LevelSetting::Trusted) => "trusted",
        Some(LevelSetting::Any) => "any",
        None => "the default",
    }
    .to_owned()
}

/// Said after "the default" of a name Kai does not know (module doc).
fn unknown_note(spec: Option<&ToolSpec>) -> &'static str {
    match spec {
        None => " (Kai offers no tool of that name now, so a change of it counts as widening)",
        Some(_) => "",
    }
}

fn tool_hint(tool: &str) -> String {
    format!(
        "config tools do not widen permissions unless the owner permits it for this tool: set \
         policy.widen_by_tools.{tool} = true in Kai's settings window (Tools page) or in the \
         configuration file"
    )
}

const TRUST_HINT: &str = "config tools do not widen trust unless the owner permits it: set \
     policy.widen_trust_by_tools = true in Kai's settings window or in the configuration file";

/// Whether the change from `old` to `new` is one the tools may make; the refusal names the first
/// widening that the owner has not permitted (`old`'s opt-ins count, never `new`'s). `spec_of`
/// gives the default an unset entry returns to ([`offered`] in Kai; the tests give theirs).
pub(super) fn check(
    old_config: &Config,
    new_config: &Config,
    spec_of: &dyn Fn(&str) -> Option<&'static ToolSpec>,
) -> Result<(), Refusal> {
    let (old, new) = (&old_config.policy, &new_config.policy);
    let refuse = |why: String| Err(Refusal(why));
    // The opt-ins themselves: switching one on is the owner's.
    for (tool, on) in &new.widen_by_tools {
        if *on && old.widen_by_tools.get(tool) != Some(&true) {
            return refuse(format!(
                "policy.widen_by_tools.{tool}: only the owner permits widening, in Kai's settings \
                 window or the configuration file; the config tools never set it"
            ));
        }
    }
    if new.widen_trust_by_tools && !old.widen_trust_by_tools {
        return refuse(
            "policy.widen_trust_by_tools: only the owner permits widening, in Kai's settings \
             window or the configuration file; the config tools never set it"
                .into(),
        );
    }
    let permitted = |tool: &str| old.widen_by_tools.get(tool) == Some(&true);
    let mut tools: Vec<&String> = old.tools.keys().chain(new.tools.keys()).collect();
    tools.sort();
    tools.dedup();
    for tool in tools {
        let spec = spec_of(tool);
        let (was, is) = (old.tools.get(tool).copied(), new.tools.get(tool).copied());
        if state_rank(spec, is, Side::After) > state_rank(spec, was, Side::Before)
            && !permitted(tool)
        {
            return refuse(format!(
                "policy.tools.{tool}: {} -> {}{} widens what {tool} may do; {}",
                state_word(was),
                state_word(is),
                unknown_note(spec),
                tool_hint(tool)
            ));
        }
    }
    let mut levels: Vec<&String> = old.levels.keys().chain(new.levels.keys()).collect();
    levels.sort();
    levels.dedup();
    for tool in levels {
        let spec = spec_of(tool);
        let (was, is) = (old.levels.get(tool).copied(), new.levels.get(tool).copied());
        if level_rank(spec, is, Side::After) > level_rank(spec, was, Side::Before)
            && !permitted(tool)
        {
            return refuse(format!(
                "policy.levels.{tool}: {} -> {}{} widens who may call {tool}; {}",
                level_word(was),
                level_word(is),
                unknown_note(spec),
                tool_hint(tool)
            ));
        }
    }
    for (key, tool) in COMMANDS {
        if command_value(old_config, key) != command_value(new_config, key) && !permitted(tool) {
            return refuse(format!(
                "{key}: Kai runs it as a command, so changing it is {tool}'s power; {}",
                tool_hint(tool)
            ));
        }
    }
    if !old.widen_trust_by_tools {
        for (key, was, is) in [
            (
                "policy.trusted_callers",
                &old.trusted_callers,
                &new.trusted_callers,
            ),
            (
                "policy.trusted_approvers",
                &old.trusted_approvers,
                &new.trusted_approvers,
            ),
        ] {
            if let Some(added) = is.iter().find(|e| !was.contains(e)) {
                return refuse(format!(
                    "{key}: adding {added:?} widens trust; {TRUST_HINT}"
                ));
            }
        }
        for (key, value, what) in PERMISSION_FLAGS {
            if value(new_config) && !value(old_config) {
                return refuse(format!(
                    "{key}: switching it on {what}, which widens trust; {TRUST_HINT}"
                ));
            }
        }
        if new.config_tools && !old.config_tools {
            return refuse(format!(
                "policy.config_tools: switching the config tools on widens what may change the \
                 configuration; {TRUST_HINT}"
            ));
        }
    }
    Ok(())
}
