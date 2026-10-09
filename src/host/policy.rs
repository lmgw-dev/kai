//! K15's policy: whether a call runs, is asked about (PD4) or is refused, from the tool's contract
//! entry (its annotations, its default and its caller level, K23), the caller and approver lmgw
//! stamped (api §5.5) and the owner's settings (`[policy]`, ruling 28).
//!
//! In order:
//! 1. **Off** (`policy.tools.<tool> = "off"`): the tool is not offered, so a call is for no tool.
//! 2. **The caller level** (`policy.levels.<tool>`, else the contract's): a caller below it is
//!    denied outright, not asked (K27: a remote caller switching on the room microphone is the
//!    case a dialog is not enough for).
//! 3. **The per-tool state:** `deny` refuses, `allow` runs, `allow_trusted` runs for trusted callers
//!    (as step 4's read-only default), `confirm` asks; the last two unless a trusted approver
//!    approved the call in its thread.
//! 4. **No state set, K15's default:** a trusted approval runs the call (where it applies, below),
//!    K24's confirmed-by-default tools included, which are asked for every other call; a read-only
//!    tool (and an overlay, PD4) runs for trusted callers and is asked for the others; every other
//!    tool is asked.
//!
//! A trusted approval is F's decision (K15: F is the path for thread calls; PD4: from WP19 a thread
//! call carrying `lmgw/approval` is decided by F), K24's tools included: the owner already
//! confirmed that call in the thread, so a second dialog would ask the owner twice, and a remote approval
//! with nobody at the desk would time out on Kai's dialog. The thread's approval shows what K24's
//! input tools act on: a call approved in lmgw names its window (`type_text`, `key_press`,
//! `ui_press` and every window action refuse an omitted or `active` window then), so the arguments
//! the approver saw carry the target. (K24's tools follow WP8's reading of PD4.) One call is
//! left out ([`approval_applies`]): `routine_enable` switching a routine on, whose own dialog shows
//! the routine in full and offers to grant what it asks; a thread's approval saw only the tool's
//! arguments.
//! There only `policy.tools.routine_enable = "allow"` (ruling 28) skips Kai's dialog:
//! `allow_trusted` does not ([`Policy::decide_call`]), so no trusted caller switches a routine on
//! unasked and without the box that grants what it asks.
//!
//! **A self-approval is no approval** (WP19; lmgw stamps every real decision, also one the call's
//! own caller made, and leaves it to the device, client-apps §6.6): an approval whose approver is
//! the call's own caller counts only when that caller is the owner, the gateway or Kai's own key
//! ([`Policy::approval_counts`]). Otherwise a trusted approver's key that starts a turn could
//! approve its own calls, and an approver trusted to judge others' calls would judge its own.
//!
//! Trusting `cli` (or a routine) in `policy.trusted_callers`, or setting a tool to `allow`, hands
//! that trust to every program of this user: `kai tool` reaches Kai from any process of the user,
//! and a caller's name identifies it, it is no boundary (`crate::cli`). The tooltip says so while
//! `cli` is trusted ([`Policy::open_notes`]).
//!
//! Trusted callers: the owner, the gateway on the owner's behalf, Kai's own key, and
//! `policy.trusted_callers`. Trusted approvers: the owner, the gateway, Kai's own key, and
//! `policy.trusted_approvers`. An unknown caller (no or an unreadable `_meta`) is the least
//! trusted.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};

use kai_desktop::contract::{Ask, Level, ToolSpec};
use lmgw_client::mcp_host::{CallerKind, Principal};

use super::confirm::shown_text;
use super::registry::Args;
use super::settings::{Named, PolicyConfig, ToolState};

/// Whether a trusted approver's approval (F) may decide this call (module doc): every call, K24's
/// confirmed-by-default tools included, but `routine_enable` switching a routine on.
pub fn approval_applies(spec: &ToolSpec, args: &Args) -> bool {
    !(spec.name == kai_desktop::contract::routines::ENABLE && args.boolean("on") == Some(true))
}

/// Who a call runs as, from `_meta["lmgw/caller"]` (or an approver from `lmgw/approval.by`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Owner,
    /// lmgw's own runs on the owner's behalf (the dashboard's threads, in-process agents).
    Gateway,
    /// Kai's own device key: its voice session and its own requests.
    Own,
    /// Another device, a client key, an agent, a routine (K28): `kind:name` as the trust lists name
    /// them; `cli` (no name) for a `kai tool` from outside any routine.
    Named(Named),
    /// No caller stamped, or one this build cannot read (a newer gateway's kind, kept as sent).
    Unknown(String),
}

impl Caller {
    /// The principal lmgw stamped; `own_device` is Kai's own device name (the feed's
    /// `hello.principal.name`), `None` until the feed said it.
    pub fn from_principal(p: Option<&Principal>, own_device: Option<&str>) -> Self {
        let Some(p) = p else {
            return Caller::Unknown(String::new());
        };
        let named = |kind: &str| {
            Caller::Named(Named {
                kind: kind.to_owned(),
                name: p.name.clone(),
            })
        };
        match &p.kind {
            CallerKind::Owner => Caller::Owner,
            CallerKind::Gateway => Caller::Gateway,
            CallerKind::Device if own_device == Some(p.name.as_str()) => Caller::Own,
            CallerKind::Device => named("device"),
            CallerKind::Key => named("key"),
            CallerKind::Agent => named("agent"),
            // Anonymous and a kind this build does not know: the least trusted (K15).
            CallerKind::Anonymous => Caller::Unknown("anonymous".into()),
            CallerKind::Unknown(k) => Caller::Unknown(k.clone()),
            // A kind lmgw-client knows and Kai does not yet: by its wire name, least trusted.
            other => Caller::Unknown(
                serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            ),
        }
    }

    /// For the log and the confirmation: "the owner", "key 'laptop'". A name holding a line break
    /// or a bidi control is shown JSON-quoted and escaped (`confirm::shown_text`).
    pub fn describe(&self) -> String {
        fn name(s: &str) -> String {
            match shown_text(s) {
                Cow::Borrowed(s) => format!("'{s}'"),
                Cow::Owned(quoted) => quoted,
            }
        }
        match self {
            Caller::Owner => "the owner".into(),
            Caller::Gateway => "lmgw (on the owner's behalf)".into(),
            Caller::Own => "Kai itself".into(),
            Caller::Named(n) if n.kind == super::settings::CLI => {
                "the command line (kai tool)".into()
            }
            Caller::Named(n) => format!("{} {}", n.kind, name(&n.name)),
            Caller::Unknown(k) if k.is_empty() => "a caller lmgw did not name".into(),
            Caller::Unknown(k) if k == "anonymous" => "an anonymous caller".into(),
            Caller::Unknown(k) => format!("a caller of kind {}", name(k)),
        }
    }
}

/// What happens to one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// The owner is asked (PD4); `why` says what asks (the log).
    Confirm {
        why: &'static str,
    },
    /// Refused; the reason goes back to the caller as the tool's error.
    Deny(String),
}

/// The policy the host and the providers read live: a change (WP21a's operations) replaces the
/// value, and every later decision and listing sees it.
pub type SharedPolicy = std::sync::Arc<std::sync::RwLock<Policy>>;

/// The owner's `[policy]`, read once (a change makes a new one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    trusted_callers: HashSet<Named>,
    trusted_approvers: HashSet<Named>,
    tools: BTreeMap<String, ToolState>,
    levels: BTreeMap<String, Level>,
    config_tools: bool,
}

impl Policy {
    /// `config` was validated (`crate::config`): an entry that does not read is skipped.
    pub fn new(config: &PolicyConfig) -> Self {
        let named = |list: &[String]| list.iter().filter_map(|e| Named::parse(e)).collect();
        Self {
            trusted_callers: named(&config.trusted_callers),
            trusted_approvers: named(&config.trusted_approvers),
            tools: config.tools.clone(),
            levels: config
                .levels
                .iter()
                .map(|(k, v)| (k.clone(), Level::from(*v)))
                .collect(),
            config_tools: config.config_tools,
        }
    }

    /// The tooltip's notes on trust that reaches every program of the user (module doc): `cli`
    /// in `policy.trusted_callers` or `policy.trusted_approvers`.
    pub fn open_notes(&self) -> Vec<String> {
        let cli = Named::cli();
        let mut notes = Vec::new();
        if self.trusted_callers.contains(&cli) {
            notes.push(
                "policy.trusted_callers names cli: every program of this user calling kai tool \
                 is trusted"
                    .to_owned(),
            );
        }
        if self.trusted_approvers.contains(&cli) {
            notes.push(
                "policy.trusted_approvers names cli: every program of this user calling kai tool \
                 is a trusted approver"
                    .to_owned(),
            );
        }
        notes
    }

    /// The tool is offered: not `off`, and K30's configuration tools only while
    /// `policy.config_tools` is on.
    pub fn listed(&self, spec: &ToolSpec) -> bool {
        self.tools.get(spec.name) != Some(&ToolState::Off)
            && (self.config_tools || !kai_desktop::contract::config::is_config_tool(spec.name))
    }

    /// The tools `[policy]` names that `offered` does not hold: said when the host starts. A
    /// `policy.tools` name holding `__` is an MCP app's tool ([`Policy::app_tool_names`]), checked
    /// when its view opens, not here; a `policy.levels` one is unknown like any (no level applies
    /// to an app's tools: their views' calls are decided by `policy.tools` and the trust lists).
    pub fn unknown_names<'a>(&'a self, offered: &'a HashSet<&str>) -> Vec<String> {
        let tools = self
            .tools
            .keys()
            .filter(|k| !k.contains("__"))
            .map(|k| format!("policy.tools.{k}"));
        let mut names: Vec<String> = tools
            .chain(self.levels.keys().map(|k| format!("policy.levels.{k}")))
            .filter(|key| {
                let (_, tool) = key
                    .split_once('.')
                    .and_then(|(_, rest)| rest.split_once('.'))
                    .unwrap_or_default();
                !offered.contains(tool)
            })
            .collect();
        names.sort();
        names
    }

    /// The `policy.tools` names of MCP apps' tools (`<server>__<tool>`, the name lmgw exposes):
    /// a view's calls are decided by them (`crate::widgets::app_bridge::decide`), and whether the
    /// server has such a tool is known only when its view opens.
    pub fn app_tool_names(&self) -> Vec<&str> {
        self.tools
            .keys()
            .map(String::as_str)
            .filter(|k| k.contains("__"))
            .collect()
    }

    /// The level the tool's callers need: the owner's setting, else the contract's.
    pub fn level(&self, spec: &ToolSpec) -> Level {
        self.levels.get(spec.name).copied().unwrap_or(spec.level)
    }

    /// `policy.tools.<tool>`, if set (an MCP app's tool by its exposed name too: app_bridge).
    pub fn state(&self, tool: &str) -> Option<ToolState> {
        self.tools.get(tool).copied()
    }

    /// The caller is trusted: the owner, the gateway, Kai's own key, or `policy.trusted_callers`.
    pub fn trusted_caller(&self, c: &Caller) -> bool {
        match c {
            Caller::Owner | Caller::Gateway | Caller::Own => true,
            Caller::Named(n) => self.trusted_callers.contains(n),
            Caller::Unknown(_) => false,
        }
    }

    fn trusted_approver(&self, c: &Caller) -> bool {
        match c {
            Caller::Owner | Caller::Gateway | Caller::Own => true,
            Caller::Named(n) => self.trusted_approvers.contains(n),
            Caller::Unknown(_) => false,
        }
    }

    /// `approver`'s approval of `caller`'s call decides it (module doc): a trusted approver, and
    /// not the caller itself unless that is the owner, the gateway or Kai's own key.
    pub fn approval_counts(&self, caller: &Caller, approver: &Caller) -> bool {
        let own_class = matches!(caller, Caller::Owner | Caller::Gateway | Caller::Own);
        self.trusted_approver(approver) && (approver != caller || own_class)
    }

    /// `config_tool`: K30's configuration tools, whose owner level leaves out the gateway (its
    /// dashboard turns, agent runs and internal keys are not the owner); widening the level
    /// (`policy.levels`, ruling 28) is the owner's way to let more callers in.
    fn admits(&self, level: Level, c: &Caller, config_tool: bool) -> bool {
        match level {
            Level::Own => *c == Caller::Own,
            Level::Owner if config_tool => matches!(c, Caller::Owner | Caller::Own),
            Level::Owner => matches!(c, Caller::Owner | Caller::Gateway | Caller::Own),
            Level::Trusted => self.trusted_caller(c),
            Level::Any => true,
        }
    }

    /// [`Policy::decide`] for a call with `args`: `routine_enable` switching a routine on is
    /// asked about whatever the state says short of `allow` (`allow_trusted` too), because its
    /// dialog is where the owner sees the routine and grants what it asks; trusted callers
    /// (the gateway included) do not skip it.
    pub fn decide_call(
        &self,
        spec: &ToolSpec,
        args: &Args,
        caller: &Caller,
        approver: Option<&Caller>,
    ) -> Decision {
        let d = self.decide(spec, caller, approver);
        if d == Decision::Allow
            && !approval_applies(spec, args)
            && self.state(spec.name) != Some(ToolState::Allow)
        {
            return Decision::Confirm {
                why: "switching a routine on is shown to the owner first (only \
                      policy.tools.routine_enable = \"allow\" skips the dialog)",
            };
        }
        d
    }

    /// The decision for `caller` calling `spec`; `approver` is who approved it in its thread
    /// (`lmgw/approval`), if anyone did (module doc).
    pub fn decide(&self, spec: &ToolSpec, caller: &Caller, approver: Option<&Caller>) -> Decision {
        let tool = spec.name;
        let state = self.tools.get(tool).copied();
        if state == Some(ToolState::Off) {
            return Decision::Deny(format!(
                "Kai does not offer {tool} (policy.tools.{tool} = \"off\")"
            ));
        }
        let level = self.level(spec);
        let config_tool = kai_desktop::contract::config::is_config_tool(tool);
        if !self.admits(level, caller, config_tool) {
            let who = match level {
                Level::Own => "Kai's own session",
                Level::Owner if config_tool => "the owner and Kai's own key",
                Level::Owner => "the owner and Kai itself",
                Level::Trusted => "the owner, Kai itself and policy.trusted_callers",
                Level::Any => unreachable!("any admits every caller"),
            };
            return Decision::Deny(format!(
                "{tool} is offered to {who} only (policy.levels.{tool} = \"{}\"), not to {}",
                level.key(),
                caller.describe()
            ));
        }
        let approved = approver.is_some_and(|a| self.approval_counts(caller, a));
        match state {
            Some(ToolState::Deny) => Decision::Deny(format!(
                "{tool} is switched off for every caller (policy.tools.{tool} = \"deny\")"
            )),
            Some(ToolState::Allow) => Decision::Allow,
            Some(ToolState::AllowTrusted) if approved || self.trusted_caller(caller) => {
                Decision::Allow
            }
            Some(ToolState::AllowTrusted) => Decision::Confirm {
                why: "policy.tools says allow_trusted, for an untrusted caller",
            },
            Some(ToolState::Confirm) if approved => Decision::Allow,
            Some(ToolState::Confirm) => Decision::Confirm {
                why: "policy.tools says confirm",
            },
            Some(ToolState::Off) => unreachable!("handled above"),
            None if approved => Decision::Allow,
            // A routine (not in the contract): its routine.toml gets at most confirmation (K28).
            None if spec.ask == Ask::Always && kai_desktop::contract::find(tool).is_none() => {
                Decision::Confirm {
                    why: "routine.toml asks at most for confirmation (K28)",
                }
            }
            None if spec.ask == Ask::Always => Decision::Confirm {
                why: "confirmed by default (K24)",
            },
            None if (spec.hints.read_only || spec.ask == Ask::Trusted)
                && self.trusted_caller(caller) =>
            {
                Decision::Allow
            }
            None if spec.hints.read_only || spec.ask == Ask::Trusted => Decision::Confirm {
                why: "an untrusted caller (K15)",
            },
            None => Decision::Confirm {
                why: "it changes state (K15)",
            },
        }
    }
}

#[cfg(test)]
mod tests;
