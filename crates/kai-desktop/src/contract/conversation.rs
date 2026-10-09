//! Conversation mode (ruling 23, K27): the owner says "enter conversation mode" and Kai keeps
//! listening, with no talk key, for a conversation or multi-step work. Both tools act on Kai's
//! voice, so the core runs them (PD3); by default only Kai's own session may call them (K15's third
//! caller level), since a remote caller must not switch on the room microphone.

use super::{Area, Ask, Hints, Level, ToolSpec};

const HINTS: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: true,
    open_world: false,
};

pub const AREA: Area = Area {
    name: "conversation",
    tools: &[
        ToolSpec {
            name: "conversation_start",
            description: "Keep listening without the talk key, for a conversation or multi-step work.",
            hints: HINTS,
            // Kai's own session is a trusted caller: no dialog in the middle of a voice turn.
            ask: Ask::Trusted,
            level: Level::Own,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "conversation_end",
            description: "Stop listening without the talk key; the configured mode returns.",
            hints: HINTS,
            ask: Ask::Trusted,
            level: Level::Own,
            ..ToolSpec::CHANGE
        },
    ],
};
