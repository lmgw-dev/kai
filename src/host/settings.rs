//! The hosted server's configuration (`[host]`, `[policy]`; `crate::config` holds one field each):
//!
//! ```toml
//! [host]                  # the hosted MCP server's link to lmgw (§10.1, PD2)
//! keepalive_s = 15        # Kai's ping on the link; nothing from lmgw for two intervals (or a
//!                         # write it does not take in that long) ends it as dead, said in the
//!                         # tooltip, and Kai links again after the feed's backoff
//! fallback_max_message_mb = 64  # used only with an lmgw that does not announce the link's
//! fallback_max_frame_mb = 16    # limits in its initialize (lmgw/host_limits): lmgw's defaults
//!                         # of mcp.host_max_message_mb and mcp.host_max_frame_mb. Kai splits a
//!                         # message into frames of at most the frame limit, and a tool's result
//!                         # over the message limit is the call's error naming it, not sent;
//!                         # 0: no bound of its own (not both)
//!
//! [policy]                # K15: what Kai's tools do for whom (ruling 28: any tool can be allow)
//! trusted_callers = ["key:laptop"]      # callers treated like the owner for read-only tools
//!                                       # and the overlays: kind:name, kind device, key, agent
//!                                       # or routine (K28: a routine's own calls), or cli (a
//!                                       # `kai tool` from outside any routine), or app (an
//!                                       # MCP app's views: app:<server>, their calls of
//!                                       # tools the server marks destructiveHint: false skip
//!                                       # the dialog; a destructive one still asks unless
//!                                       # policy.tools allows it)
//! trusted_approvers = ["device:phone"]  # an approval in a thread by one of these is not asked
//!                                       # again (the owner, the gateway and Kai always are);
//!                                       # every kind but app
//! [policy.tools]          # per tool: allow | allow_trusted | confirm | deny | off (allow_trusted:
//!                         # unasked for trusted callers, confirmed for the others, though
//!                         # routine_enable switching a routine on is asked whatever; off: not
//!                         # offered at all)
//! clipboard_get = "allow"
//! [policy.levels]         # per tool, who may call it at all: own (Kai's own key) | owner (and the
//! conversation_start = "owner"  # gateway) | trusted (and trusted_callers) | any
//! ```
//!
//! **Trust that reaches every program:** `kai tool` answers any process of this user (`crate::cli`),
//! so trusting `cli` (or a routine, whose name a process of the user can borrow while the routine
//! runs) or setting a tool to `allow` hands that tool to every program of this user, not only to
//! the owner's terminal or the routine. The tooltip says so while `cli` is trusted.
//!
//! **A routine is its folder, now:** a switched-on routine runs whatever its folder holds at the
//! time (no pin, the owner's decision of 2026-10-09), so it is exactly as trustworthy as everything
//! that can write that folder (any program of the user, another routine's script, `run_command`).
//! `routine:<name>` identifies which routine called, not its content: trusting it, or granting the
//! routine `allow`, `allow_trusted` or a wider level, trusts every future version of the folder.
//! Its `routine.toml` itself gets at most confirm and the owner's level (`crate::routines::def`).
//!
//! `policy.widen_by_tools.<tool> = true` (owner's decision, 2026-10-09; `crate::settings::widen`)
//! lets the config tools widen `<tool>`'s state or level (for `run_command` also change
//! `launch.terminal`, a command Kai runs); `policy.widen_trust_by_tools = true`
//! lets them add trusted callers or approvers and switch `config_tools` on. Off by default;
//! narrowing always passes; the tools cannot set either (the settings window and the file can).
//!
//! `policy.config_tools = false` (K30) leaves the owner's configuration tools out of the list
//! (`config_get`, `config_set`, …); on, they are listed like any tool, owner-level and confirmed
//! unless set to `allow`.
//!
//! A tool named in `policy.tools` or `policy.levels` that Kai does not offer is said in the log and
//! the tooltip when the host starts (the set is the contract's and, later, the routines'), not
//! refused here. A `policy.tools` name holding `__` is an MCP app's tool (`<server>__<tool>`,
//! decided for its views' calls): listed in the log at the start, and named when its server's view
//! opens if the server has no such tool. A `policy.levels` one does nothing (said at the start).

use std::collections::BTreeMap;
use std::time::Duration;

use kai_desktop::contract::Level;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HostConfig {
    /// Kai's ping on the link; two intervals without a word from lmgw end it.
    pub keepalive_s: u64,
    /// The link's message limit where lmgw announces none (module doc).
    pub fallback_max_message_mb: u64,
    /// The link's frame limit where lmgw announces none.
    pub fallback_max_frame_mb: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            keepalive_s: 15,
            fallback_max_message_mb: 64,
            fallback_max_frame_mb: 16,
        }
    }
}

impl HostConfig {
    pub fn keepalive(&self) -> Duration {
        Duration::from_secs(self.keepalive_s)
    }

    /// The limits a link runs under until lmgw announces its own.
    pub fn fallback_limits(&self) -> super::limits::LinkLimits {
        super::limits::LinkLimits::fallback(
            self.fallback_max_message_mb,
            self.fallback_max_frame_mb,
        )
    }
}

/// K15's per-tool state (the configuration window says on, ask, deny, off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolState {
    /// Runs for every caller its level admits, with no question.
    Allow,
    /// Runs unasked for the trusted callers and is confirmed for the others, as K15's default of a
    /// read-only tool (the window's "allow if trusted"); the owner's way to grant a routine what
    /// its `routine.toml` asks with `policy = "trusted"` or `read_only = true` (K28).
    #[serde(rename = "allow_trusted")]
    AllowTrusted,
    /// The owner is asked (PD4), unless a trusted approver approved the call in its thread (F
    /// decided it, K15).
    Confirm,
    /// Refused for every caller.
    Deny,
    /// Not offered: not in the tool list (`notifications/tools/list_changed` when it changes).
    Off,
}

/// `policy.levels`' words for the contract's [`Level`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelSetting {
    Own,
    Owner,
    Trusted,
    Any,
}

impl From<LevelSetting> for Level {
    fn from(l: LevelSetting) -> Self {
        match l {
            LevelSetting::Own => Level::Own,
            LevelSetting::Owner => Level::Owner,
            LevelSetting::Trusted => Level::Trusted,
            LevelSetting::Any => Level::Any,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PolicyConfig {
    /// `kind:name` entries (`key:laptop`, `agent:folder-chat`, `device:phone`).
    pub trusted_callers: Vec<String>,
    /// `kind:name` entries, as `trusted_callers`.
    pub trusted_approvers: Vec<String>,
    pub tools: BTreeMap<String, ToolState>,
    pub levels: BTreeMap<String, LevelSetting>,
    /// K30: the owner's configuration tools (`contract::config`) are offered (§16 Q17: off).
    pub config_tools: bool,
    /// The owner's permission (2026-10-09) for the config tools to WIDEN what `<tool>` may do
    /// (its state toward allow, its level toward any): `widen_by_tools.key_press = true`. Absent
    /// or false: such a write through `config_set`, `config_unset`, `tool_state_set` or
    /// `tool_level_set` is refused (for `run_command` also a change of `launch.terminal`). Only the
    /// owner sets it, in the settings window or the file; the tools never.
    pub widen_by_tools: BTreeMap<String, bool>,
    /// The same for trust: adding to `trusted_callers` or `trusted_approvers`, switching
    /// `config_tools` on. Owner-only, like `widen_by_tools`.
    pub widen_trust_by_tools: bool,
}

/// The principal kinds a trust list may name: the owner, the gateway and Kai itself are always
/// trusted and need no entry. `routine` is K28's: a routine's own calls (its steps, its goal's, its
/// script's `kai tool`), `routine:<name>`. `app` is an MCP app's views, `app:<server>` (their calls
/// through Kai's bridge, `crate::widgets::app_bridge`).
pub const NAMED_KINDS: [&str; 5] = ["device", "key", "agent", "routine", "app"];

/// K28's caller for a `kai tool` from outside any routine's scope (the owner's terminal, any other
/// program of the user): the entry `cli`, with no name.
pub const CLI: &str = "cli";

/// A trust list's entry, `kind:name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Named {
    pub kind: String,
    pub name: String,
}

impl Named {
    /// `kind:name` (or `cli`), each part trimmed: `"app: fx"` is `app:fx`, as a caller's name is
    /// compared whole.
    pub fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim();
        if entry == CLI {
            return Some(Self::cli());
        }
        let (kind, name) = entry.split_once(':')?;
        let (kind, name) = (kind.trim(), name.trim());
        (NAMED_KINDS.contains(&kind) && !name.is_empty()).then(|| Self {
            kind: kind.to_owned(),
            name: name.to_owned(),
        })
    }
}

impl Named {
    /// The caller `cli` (K28).
    pub fn cli() -> Self {
        Self {
            kind: CLI.to_owned(),
            name: String::new(),
        }
    }

    /// The routine `name`'s own calls (K28).
    pub fn routine(name: &str) -> Self {
        Self {
            kind: "routine".to_owned(),
            name: name.to_owned(),
        }
    }
}

/// A key that breaks its rule: the key, the value as written and the rule (`crate::config` turns it
/// into its error).
pub type Invalid = (&'static str, String, String);

impl HostConfig {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.keepalive_s == 0 {
            return Err((
                "host.keepalive_s",
                "0".into(),
                "must be at least 1 (seconds)".into(),
            ));
        }
        if self.fallback_max_message_mb == 0 && self.fallback_max_frame_mb == 0 {
            return Err((
                "host.fallback_max_frame_mb",
                "0".into(),
                "cannot be 0 together with host.fallback_max_message_mb (a frame would be \
                 unbounded)"
                    .into(),
            ));
        }
        Ok(())
    }
}

impl PolicyConfig {
    pub fn validate(&self) -> Result<(), Invalid> {
        for (key, list) in [
            ("policy.trusted_callers", &self.trusted_callers),
            ("policy.trusted_approvers", &self.trusted_approvers),
        ] {
            if let Some(bad) = list.iter().find(|e| Named::parse(e).is_none()) {
                return Err((
                    key,
                    format!("{bad:?}"),
                    format!(
                        "each entry is kind:name with kind one of {}, or {CLI} (the owner, the \
                         gateway and Kai itself are always trusted)",
                        NAMED_KINDS.join(", ")
                    ),
                ));
            }
        }
        // An app's views make calls; they approve none (a thread's approval is a principal's).
        if let Some(bad) = self
            .trusted_approvers
            .iter()
            .find(|e| Named::parse(e).is_some_and(|n| n.kind == "app"))
        {
            return Err((
                "policy.trusted_approvers",
                format!("{bad:?}"),
                "app is a kind of trusted_callers only: an app's views make calls, they approve \
                 none"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_entries_are_kind_and_name() {
        assert_eq!(
            Named::parse("key:laptop"),
            Some(Named {
                kind: "key".into(),
                name: "laptop".into()
            })
        );
        assert_eq!(Named::parse("agent:a:b").unwrap().name, "a:b");
        for bad in ["laptop", "owner:x", "key:", "key: ", ":x", "Key:x"] {
            assert_eq!(Named::parse(bad), None, "{bad}");
        }
        let p = PolicyConfig {
            trusted_approvers: vec!["device:phone".into(), "phone".into()],
            ..Default::default()
        };
        let (key, value, rule) = p.validate().unwrap_err();
        assert_eq!(key, "policy.trusted_approvers");
        assert_eq!(value, "\"phone\"");
        assert!(rule.contains("device, key, agent, routine, app, or cli"));
        assert_eq!(Named::parse("cli"), Some(Named::cli()));
        assert_eq!(
            Named::parse("routine:backup"),
            Some(Named::routine("backup"))
        );
        assert_eq!(Named::parse("cli:x"), None);
        assert_eq!(Named::parse("app:fx").map(|n| n.kind), Some("app".into()));
        assert_eq!(
            Named::parse(" app : fx "),
            Named::parse("app:fx"),
            "trimmed: a caller's name is compared whole"
        );
        assert_eq!(Named::parse(" cli "), Some(Named::cli()));
        let p = PolicyConfig {
            trusted_callers: vec!["app:fx".into()],
            trusted_approvers: vec!["app:fx".into()],
            ..Default::default()
        };
        let (key, value, rule) = p.validate().unwrap_err();
        assert_eq!(
            (key, value.as_str()),
            ("policy.trusted_approvers", "\"app:fx\"")
        );
        assert!(rule.contains("trusted_callers only"), "{rule}");
        assert!(
            HostConfig {
                keepalive_s: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let both = HostConfig {
            fallback_max_message_mb: 0,
            fallback_max_frame_mb: 0,
            ..Default::default()
        };
        assert_eq!(both.validate().unwrap_err().0, "host.fallback_max_frame_mb");
        let d = HostConfig::default().fallback_limits();
        assert_eq!(
            (d.message, d.frame, d.announced),
            (Some(64 << 20), Some(16 << 20), false)
        );
    }

    #[test]
    fn the_sections_read_from_the_configuration() {
        let parse = |t: &str| crate::config::Config::parse(t, std::path::Path::new("kai.toml"));
        let c = parse(
            "[host]\nkeepalive_s = 5\n[policy]\ntrusted_callers = [\"key:laptop\"]\n\
             [policy.tools]\nclipboard_get = \"allow\"\ntype_text = \"off\"\n\
             [policy.levels]\nconversation_start = \"owner\"\n",
        )
        .unwrap();
        assert_eq!(c.host.keepalive(), Duration::from_secs(5));
        assert_eq!(c.policy.trusted_callers, ["key:laptop"]);
        assert_eq!(c.policy.tools["clipboard_get"], ToolState::Allow);
        assert_eq!(c.policy.tools["type_text"], ToolState::Off);
        assert_eq!(c.policy.levels["conversation_start"], LevelSetting::Owner);
        assert_eq!(parse("").unwrap().host, HostConfig::default());
        for (text, says) in [
            ("[policy.tools]\nx = \"maybe\"\n", "maybe"),
            ("[policy.levels]\nx = \"everyone\"\n", "everyone"),
            ("[policy]\ntrusted = []\n", "trusted"),
            ("[host]\nkeepalive_s = 0\n", "host.keepalive_s"),
            (
                "[policy]\ntrusted_callers = [\"laptop\"]\n",
                "policy.trusted_callers",
            ),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
