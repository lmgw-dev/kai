//! The routines' own tools (ruling 25, K28). Each routine is a hosted tool too, declared by its
//! `routine.toml` and checked by [`super::check`] when Kai loads it; this file holds only the two
//! tools that work on routines:
//!
//! - `routine_create {goal}`: a delegate (K25) that writes a draft routine, a bash script by
//!   default, on the alias the configuration names. The draft starts disabled.
//! - `routine_enable {name, on}`: switches a routine on or off. Confirmed by default for every
//!   caller (ruling 28 lets the owner set it to `allow`): the owner's dialog shows the routine in
//!   full, its script included, and what its `routine.toml` asks beyond confirm and the owner's
//!   level, with a box that grants it. A configuration tool (K30): listed only while
//!   `policy.config_tools` is on; the tray's switch reaches it either way.

use super::{Area, Ask, Hints, Kind, Level, Param, ToolSpec};

const GOAL: Param = Param::new("goal", Kind::Text)
    .described("what the routine should do, in plain words; it may name the parameters it takes");
const NAME: Param = Param::new("name", Kind::Text);
const ON: Param = Param::new("on", Kind::Boolean);

/// The name of the enable tool (`contract::config::is_config_tool` names it a configuration tool).
pub const ENABLE: &str = "routine_enable";

pub const AREA: Area = Area {
    name: "routines",
    tools: &[
        ToolSpec {
            name: "routine_create",
            description: "Write a new routine for a goal as a draft, which stays off until the owner enables it.",
            params: &[GOAL],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: ENABLE,
            description: "Switch one of Kai's routines on or off; switching one on shows it to the owner in full first.",
            params: &[NAME, ON],
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            ask: Ask::Always,
            level: Level::Owner,
            ..ToolSpec::CHANGE
        },
    ],
};
