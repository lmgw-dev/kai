//! The personality (ruling 29, K31, WP22): the owner says "talk shorter, be calm" and Kai switches
//! its conversation to one of lmgw's personality profiles. The profiles are lmgw's Chat feature;
//! the tool names one, so the core runs it (PD3). It changes state, so K15's hints decide: it is
//! confirmed unless the owner sets it to `allow` (ruling 28); by default only the owner and Kai's
//! own key may call it, since it changes how the owner's assistant sounds.

use super::{Area, Hints, Kind, Level, Param, ToolSpec};

const NAME: Param = Param::new("name", Kind::Text).described(
    "a profile's name, any case; default is the gateway's own: no profile, or the one it names \
     for new conversations",
);

pub const AREA: Area = Area {
    name: "personality",
    tools: &[ToolSpec {
        name: "personality_set",
        description: "Switch the conversation to a personality profile, from its next response.",
        params: &[NAME],
        hints: Hints {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        level: Level::Owner,
        ..ToolSpec::CHANGE
    }],
};
