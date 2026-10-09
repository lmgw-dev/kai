//! Kai's asynchronous jobs (ruling 22, K26, WP18): a tool of the open-ended class
//! ([`super::Class::Async`]) answers "started, job <id>" at once and runs on as a job; its result
//! enters the chat later, and Kai speaks it at a quiet moment or notifies it. These tools show the
//! jobs and act on them, so nothing runs unseen. They act on Kai's core (PD3: a job's announce).

use super::{Area, Ask, Hints, Kind, Param, ToolSpec};

/// A job's id, as its start (`started, job <id>`) and `jobs` name it.
const ID: Param = Param::new("id", Kind::Text).described("the job's id, as its start names it");

pub const AREA: Area = Area {
    name: "jobs",
    tools: &[
        ToolSpec {
            name: "jobs",
            description: "List Kai's jobs: the running ones, and the finished ones whose result has not reached the chat yet.",
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "job_cancel",
            description: "Cancel one of Kai's running jobs.",
            params: &[ID],
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            // Cancelling stops work and starts none: trusted callers without a dialog.
            ask: Ask::Trusted,
            ..ToolSpec::CHANGE
        },
        ToolSpec {
            name: "job_announce",
            description: "Choose whether Kai speaks a job's result when it is done, or only notifies it.",
            params: &[
                ID,
                Param::new("on", Kind::Boolean).described("true: spoken; false: a notification"),
            ],
            hints: Hints {
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            ask: Ask::Trusted,
            ..ToolSpec::CHANGE
        },
    ],
};
