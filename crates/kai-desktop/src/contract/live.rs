//! The live checks' tools (`just live-check-host`, WP8; `just live-check-approvals`, WP19): built
//! only with the `live-test` feature, so no installed Kai ever offers them. A mock model's turn calls
//! one through lmgw and reads its answer. `live_note` changes state (by its annotations), so K15
//! asks about it unless a trusted approver approved the call. `live_slow` is K26's open-ended class
//! (`just live-check-jobs`, WP18): it answers after the seconds it was given, as a job.

use super::{Area, Class, Kind, Param, ToolSpec};

pub const AREA: Area = Area {
    name: "live",
    tools: &[ToolSpec {
        name: "live_echo",
        description: "Answer with the text it was given (Kai's live checks only).",
        params: &[Param::new("text", Kind::Text)],
        ..ToolSpec::READ
    },
    ToolSpec {
        name: "live_note",
        description: "Keep a note (Kai's live checks only; it changes nothing).",
        params: &[Param::new("text", Kind::Text)],
        ..ToolSpec::CHANGE
    },
    ToolSpec {
        name: "live_slow",
        description: "Answer with the text after the given seconds, as a job (Kai's live checks only).",
        params: &[Param::new("text", Kind::Text), Param::new("seconds", Kind::Integer)],
        class: Class::Async,
        ..ToolSpec::READ
    }],
};
