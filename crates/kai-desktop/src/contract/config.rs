//! K30's owner configuration tools (ruling 27): every setting of Kai's configuration file as an
//! operation, offered as tools. The core runs them (PD3, `src/settings/`); the configuration
//! window (K29) calls the same operations.
//!
//! They are owner-level (K15's caller level: the owner, the gateway on the owner's behalf and
//! Kai's own key); the owner can widen a level and set any of them to `allow` (ruling 28). They
//! are offered only while `policy.config_tools` is on (default off, §16 Q17): see
//! [`is_config_tool`], which Kai's policy asks. No tool reads a secret, and none sets one.

use super::{Area, Hints, Kind, Level, Param, ToolSpec};

const KEY: Param = Param::new("key", Kind::Text)
    .described("dotted, like voice.grace_s or policy.tools.type_text");
const VALUE: Param = Param::new("value", Kind::Text)
    .described("a TOML value: 30, true, \"text\", [\"a\", \"b\"]; plain text is read as a string");
const SECTION: Param = Param::new("section", Kind::Text)
    .optional()
    .described("a section like voice or policy.tools; default: every setting");
const TOOL: Param = Param::new("tool", Kind::Text);
const STATE: Param = Param::new("state", Kind::Text)
    .described("allow, confirm, deny, off (not offered) or default");
const LEVEL: Param = Param::new("level", Kind::Text)
    .described("own, owner, trusted, any or default: who may call it at all");
const CAPABILITY: Param = Param::new("capability", Kind::Text);
const PROVIDER: Param = Param::new("provider", Kind::Text)
    .described("plasma, fdo, none (no tools of it) or default: the probe's choice");

const CHANGE: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: true,
    open_world: false,
};

pub const AREA: Area = Area {
    name: "config",
    tools: &[
        ToolSpec {
            name: "config_list",
            description: "List Kai's settings with their values and whether they are set or default.",
            params: &[SECTION],
            level: Level::Owner,
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "config_get",
            description: "Read one setting of Kai.",
            params: &[KEY],
            level: Level::Owner,
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "config_set",
            description: "Set one setting of Kai; the file's comments stay.",
            params: &[KEY, VALUE],
            hints: CHANGE,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "config_unset",
            description: "Remove one setting from Kai's configuration, so its default applies.",
            params: &[KEY],
            hints: CHANGE,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "tool_state_set",
            description: "Set what Kai does when a tool is called: allow, confirm, deny or off.",
            params: &[TOOL, STATE],
            hints: CHANGE,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "tool_level_set",
            description: "Set who may call a tool of Kai at all.",
            params: &[TOOL, LEVEL],
            hints: CHANGE,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "provider_set",
            description: "Choose which provider serves a desktop capability.",
            params: &[CAPABILITY, PROVIDER],
            hints: CHANGE,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
    ],
};

/// The tool belongs to this area, or is `routine_enable` (K28: a routine's switch is configuration
/// too); the policy hides these while `policy.config_tools` is off.
pub fn is_config_tool(name: &str) -> bool {
    AREA.tools.iter().any(|t| t.name == name) || name == super::routines::ENABLE
}

#[cfg(test)]
mod tests {
    use super::super::Ask;
    use super::*;

    #[test]
    fn the_tools_are_owner_level_and_ask_by_hints() {
        for t in AREA.tools {
            assert_eq!(t.level, Level::Owner, "{}", t.name);
            assert_eq!(t.ask, Ask::Hints, "{}", t.name);
            assert_eq!(t.hints.read_only, t.name.ends_with("_list") || t.name == "config_get");
        }
    }
}
