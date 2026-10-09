//! The session and media tools (WP16b, K24's state-changing list): the media players' controls,
//! the output volume, notifications and questions, the clipboard, the screen lock, do not disturb,
//! Night Light, the brightness and suspend. Served by Kai's desktop module; each is listed only
//! where its capability is served ([`needs`], K23).
//!
//! Every one changes state, so K15 confirms it by default for every caller and the owner's per-tool
//! policy can set it to `allow`; `suspend` alone is destructive (what the machine was doing stops).

use super::{Area, Hints, Kind, Param, ToolSpec};
use crate::caps::Capability;

const PLAYER: Param = Param::new("player", Kind::Text)
    .optional()
    .described("a player's name as desktop_context shows it; default: the one playing");
const ON: Param = Param::new("on", Kind::Boolean);
const PERCENT: Param = Param::new("percent", Kind::Integer).described("0 to 100");

/// A state-changing tool a second identical call does not change further.
const SET: ToolSpec = ToolSpec {
    hints: Hints {
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
    ..ToolSpec::CHANGE
};

pub const AREA: Area = Area {
    name: "session",
    tools: &[
        ToolSpec {
            name: "media_play_pause",
            description: "Play a media player, or pause it if it is playing.",
            params: &[PLAYER],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "media_next",
            description: "Skip to the next track of a media player.",
            params: &[PLAYER],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "media_previous",
            description: "Go back to the previous track of a media player.",
            params: &[PLAYER],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "volume_set",
            description: "Set the volume of an audio output, 0 to 100 percent.",
            params: &[
                PERCENT,
                Param::new("device", Kind::Text)
                    .optional()
                    .described("an output's name or description; default: the default output"),
            ],
            ..SET
        },
        ToolSpec {
            name: "mute",
            description: "Mute the default audio output, or unmute it.",
            params: &[ON],
            ..SET
        },
        ToolSpec {
            name: "notify",
            description: "Show the owner a desktop notification.",
            params: &[Param::new("title", Kind::Text), Param::new("body", Kind::Text)],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "ask",
            description: "Ask the owner a question with a button per choice and wait for the \
                          answer.",
            params: &[
                Param::new("question", Kind::Text),
                Param::new("choices", Kind::TextList),
            ],
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "clipboard_set",
            description: "Put text on the clipboard, replacing what it holds.",
            params: &[Param::new("text", Kind::Text)],
            ..SET
        },
        ToolSpec {
            name: "lock_screen",
            description: "Lock the screen.",
            ..SET
        },
        ToolSpec {
            name: "dnd",
            description: "Switch do not disturb on or off.",
            params: &[ON],
            ..SET
        },
        ToolSpec {
            name: "night_light",
            description: "Resume Night Light, or pause it.",
            params: &[ON],
            ..SET
        },
        ToolSpec {
            name: "brightness_set",
            description: "Set the screen brightness, 0 to 100 percent (never fully dark).",
            params: &[PERCENT],
            ..SET
        },
        ToolSpec {
            name: "suspend",
            description: "Suspend the machine to memory.",
            hints: Hints {
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: false,
            },
            ..ToolSpec::CHANGE
        },
    ],
};

/// The capability whose service `name` needs; a tool its desktop cannot serve is not listed (K23).
pub fn needs(name: &str) -> Option<Capability> {
    Some(match name {
        "media_play_pause" | "media_next" | "media_previous" => Capability::Media,
        "volume_set" | "mute" => Capability::Audio,
        "notify" | "ask" | "dnd" => Capability::Notify,
        "clipboard_set" => Capability::Clipboard,
        "lock_screen" | "suspend" | "brightness_set" | "night_light" => Capability::Session,
        _ => return None,
    })
}
