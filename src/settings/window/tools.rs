//! `GET /api/tools`: the tools Kai could offer, as the policy table lists them. The contract's
//! facts, which no setting changes (`kai_desktop::contract`); the owner's `policy.tools` and
//! `policy.levels` come from `/api/apply`'s list, and the page lays the two side by side.

use kai_desktop::contract::{self, Ask};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub area: String,
    pub description: String,
    pub read_only: bool,
    /// What a call does with no `policy.tools` entry (`Policy::decide`, K15), in a few words.
    pub default_state: String,
    pub default_level: String,
    pub config_tool: bool,
}

/// Every tool of every area, in the contract's order.
pub fn all() -> Vec<ToolInfo> {
    contract::areas()
        .into_iter()
        .flat_map(|area| {
            area.tools.iter().map(|spec| ToolInfo {
                name: spec.name.to_owned(),
                area: area.name.to_owned(),
                description: spec.description.to_owned(),
                read_only: spec.hints.read_only,
                default_state: default_state(spec.ask, spec.hints.read_only).to_owned(),
                default_level: spec.level.key().to_owned(),
                config_tool: contract::config::is_config_tool(spec.name),
            })
        })
        .collect()
}

/// `Policy::decide`'s `None` arms: "allow if trusted" is allowed for the trusted callers (the owner,
/// the gateway, Kai's own key, `policy.trusted_callers`) and confirmed for every other.
fn default_state(ask: Ask, read_only: bool) -> &'static str {
    match ask {
        Ask::Always => "confirm",
        Ask::Trusted => "allow if trusted",
        Ask::Hints if read_only => "allow if trusted",
        Ask::Hints => "confirm",
    }
}
