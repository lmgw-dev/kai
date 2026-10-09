//! Starting programs (WP16a, K24, K14, ruling 5's "run programs"): `open`, `app_launch` and
//! `run_command`. Served by Kai's desktop module where the Launch capability is (K23). Each
//! program runs in its own app scope, never as part of Kai.
//!
//! `run_command` takes an argv list, not a shell string (K24): a shell is one the caller names
//! (`["sh", "-c", "…"]`). It is confirmed by default (K15, K24; §16 Q12) and, like every tool, may
//! be set to `allow` (ruling 28). K26 puts it in the open-ended class; until lmgw's MCP Tasks
//! (M4, WP18) exist it answers when the command ends, within the call's deadline
//! (`lmgw/timeout_ms`). `open` refuses programs and launchers (executable types): those are
//! `run_command`'s, which the owner confirms.

use super::{Ask, Area, Class, Hints, Kind, Param, ToolSpec};

const TARGET: Param = Param::new("target", Kind::Text)
    .described("a path (absolute, or from the home directory) or a URL");
const APP: Param = Param::new("app", Kind::Text).described("an application's id, name or command");
const URLS: Param = Param::new("urls", Kind::TextList)
    .optional()
    .described("files or URLs to open in it");
const ARGV: Param = Param::new("argv", Kind::TextList)
    .described("the program and its arguments, no shell (for one: [\"sh\", \"-c\", \"…\"])");
const CWD: Param = Param::new("cwd", Kind::Text)
    .optional()
    .described("the working directory; default: the home directory");

/// Starting a program: it changes state and may reach the network (a URL, whatever it does).
const START: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: true,
};

pub const AREA: Area = Area {
    name: "launch",
    tools: &[
        ToolSpec {
            name: "open",
            description: "Open a file, folder or URL with its default application.",
            params: &[TARGET],
            hints: START,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "app_launch",
            description: "Start an installed application, optionally opening files or URLs in it.",
            params: &[APP, URLS],
            hints: START,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "run_command",
            description: "Run a program and return its exit status and output.",
            params: &[ARGV, CWD],
            // Any command: it may change or delete anything the owner can.
            hints: Hints {
                destructive: true,
                ..START
            },
            ask: Ask::Always,
            class: Class::Async,
            ..ToolSpec::CHANGE
        },
    ],
};
