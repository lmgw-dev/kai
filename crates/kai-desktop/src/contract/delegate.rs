//! The delegates (ruling 21, K25): domain tools with one plain-language parameter, each served by a
//! loop Kai runs with a second model (the delegate's alias, §16 Q13) and the fine-grained tools of
//! the desktop module (`windows`, `read`, `session`), under the policy of whoever called it. The
//! delegate sees the screenshots; the caller gets a short answer: done, partial or failed, and what
//! it did. Served by Kai's `src/delegate/`, each listed only where the desktop serves the tools it
//! needs.
//!
//! K26: `look` and `find` take a few seconds, so they are synchronous. `arrange_windows {goal}` and
//! `operate_app {app, goal}` are open-ended: they join this file as `Class::Async` once Kai has
//! asynchronous jobs (WP18), not before.

use super::{Area, Kind, Param, ToolSpec, WINDOW};

const QUESTION: Param = Param::new("question", Kind::Text);
const WHAT: Param = Param::new("what", Kind::Text)
    .described("a file, a window or something recently used, in plain words");

pub const AREA: Area = Area {
    name: "delegate",
    tools: &[
        ToolSpec {
            name: "look",
            description: "Look at a window or the screen and answer a question about what it shows.",
            params: &[QUESTION, WINDOW],
            ..ToolSpec::READ
        },
        ToolSpec {
            name: "find",
            description: "Find something on this computer described in plain words.",
            params: &[WHAT],
            ..ToolSpec::READ
        },
    ],
};
