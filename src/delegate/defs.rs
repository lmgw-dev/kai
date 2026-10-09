//! Kai's delegates: each a contract entry (`kai_desktop::contract::delegate`) with its loop's
//! definition, and the tools it needs to be listed (K23: a delegate that cannot work here is not
//! offered).
//!
//! **Adding one** (`arrange_windows`, `operate_app`, `routine_create`): its contract entry, a
//! [`Def`] and a line in [`DELEGATES`]. A `Class::Async` entry is left out of the list until Kai
//! has asynchronous jobs (WP18, K26); then its handler starts a job that runs the same loop
//! (`super::run_task`) and delivers its answer late.

use kai_delegate::{ContextStep, Def};
use kai_desktop::contract::{ToolSpec, delegate::AREA};

/// One delegate.
pub struct Delegate {
    pub spec: &'static ToolSpec,
    pub def: &'static Def,
    /// It is listed while the desktop serves at least one of these.
    pub needs_any: &'static [&'static str],
    /// It works from screenshots: an alias that lmgw lists as without vision fails it up front.
    pub vision: bool,
}

const WINDOWS: ContextStep = ContextStep {
    tool: "window_list",
    pass: &[],
    heading: "Open windows",
};

pub const LOOK: Def = Def {
    name: "look",
    instructions: "Answer a question about what a window or the screen shows. The window the \
                   call named (else the active window) is below as a screenshot. See another \
                   window or a whole output, or read a window's text through accessibility, when \
                   the question needs it. Answer what was asked, in the question's language, and \
                   nothing more.",
    examples: &[
        "question: what time does the clock show?\n(the screenshot shows a clock at 14:02)\n\
         done: The clock shows 14:02.",
        "question: is the build in the terminal finished?\n-> read_window {window: konsole}\n\
         partial: The tests passed; the build's own last lines are scrolled out of view.",
    ],
    tools: &[
        "desktop_context",
        "window_list",
        "see_window",
        "see_screen",
        "read_window",
        "read_selection",
    ],
    context: &[
        WINDOWS,
        ContextStep {
            tool: "see_window",
            pass: &[("window", "window")],
            heading: "The window",
        },
    ],
};

pub const FIND: Def = Def {
    name: "find",
    instructions: "Find what is described: a file, a folder, a window, or something used \
                   recently. Search the file index with a few distinctive words, and another \
                   wording when the first finds nothing; look at the recent files and the open \
                   windows. Answer with the path or the window id and one line on why it is the \
                   one; name the best two or three when it is not clear which is meant.",
    examples: &[
        "what: the electricity bill, spring\n-> find_files {query: electricity bill}\n\
         done: ~/Documents/Invoices/2026-04-electricity.pdf (the only electricity bill, April 2026).",
    ],
    tools: &[
        "window_list",
        "find_files",
        "recent_files",
        "desktop_context",
    ],
    context: &[WINDOWS],
};

/// Every delegate, in the contract's order.
pub static DELEGATES: [Delegate; 2] = [
    Delegate {
        spec: &AREA.tools[0],
        def: &LOOK,
        needs_any: &["see_window"],
        vision: true,
    },
    Delegate {
        spec: &AREA.tools[1],
        def: &FIND,
        needs_any: &["find_files", "recent_files"],
        vision: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Each delegate is its contract entry, and the tools it names are the desktop's.
    #[test]
    fn the_delegates_match_the_contract() {
        assert_eq!(DELEGATES.len(), AREA.tools.len());
        for d in &DELEGATES {
            assert_eq!(d.spec.name, d.def.name);
            for t in d.def.tools.iter().chain(d.needs_any) {
                let (area, _) = kai_desktop::contract::find(t).unwrap_or_else(|| panic!("{t}"));
                assert!(["windows", "read", "session"].contains(&area.name), "{t}");
            }
            for c in d.def.context {
                assert!(d.def.tools.contains(&c.tool), "{}", c.tool);
                for (from, to) in c.pass {
                    assert!(d.spec.param(from).is_some(), "{from}");
                    assert!(
                        kai_desktop::contract::find(c.tool)
                            .unwrap()
                            .1
                            .param(to)
                            .is_some()
                    );
                }
            }
        }
    }
}
