//! The MCP Apps host bridge (K18, §10.1's bridge, WP12): `overlay_app {server, uri}` shows the
//! `ui://` view of the MCP server labelled `server` (lmgw's tool prefix) in the widgets' sandbox and
//! answers the view's JSON-RPC (MCP Apps, SEP-1865 revision [`mcp_apps::REVISION`]) over
//! `postMessage`.
//!
//! - **The view's document** comes from lmgw's `/mcp` `resources/read` under Kai's key
//!   ([`crate::lmgw_link::mcp`]), so lmgw's scope for the key decides whether Kai reaches the
//!   server. It must be HTML ([`mcp_apps::MIME_TYPE`], or any `text/html`); it is served like
//!   `overlay_html`'s, under §10.1's CSP, with every load but `kai-sandbox:`, `data:` and `blob:`
//!   refused. What its `_meta.ui` asks beyond that (`csp`'s `connectDomains`, `resourceDomains`,
//!   `frameDomains`, `baseUriDomains`; `permissions`) is refused, logged, and named in the show's
//!   answer and `overlay_list`'s notes ([`Resource`]).
//! - **Only frames Kai placed are answered** (K18): Kai's host script (`assets/widgets/app-host.js`,
//!   in Kai's isolated world, only in an `overlay_app` widget) relays a message only when its
//!   `MessageEvent.source` is the view's frame; the GTK side passes a widget's messages only to
//!   its own bridge.
//! - **What a view may do** ([`Bridge::answer`]): `ui/initialize` (Kai's host capabilities: server
//!   tools and resources, logging; no links, no messages into a conversation, no model context,
//!   no display modes but `inline`), `ping`, and, proxied to `/mcp`:
//!   - `tools/call` of a tool **of its own server** that `/mcp` listed to Kai when the view was
//!     shown, by the server's own name (Kai adds the label: `<server>__<name>`), and only one
//!     whose `_meta.ui.visibility` includes `"app"` (MCP Apps: the default is both). A tool not
//!     marked `readOnlyHint` asks the owner first (K15's dialog, as `app '<server>'`; Deny is the
//!     default; hiding the view closes the dialog and denies). **One dialog per view at a time:**
//!     another such call while one is open is answered `-32000` at once, nobody asked.
//!   - `resources/read` of its own server's URIs only: a URI is put into the server's namespace
//!     (`ui://card/x` → `ui://<server>__card/x`, `file:///x` → `file://<server>__/x`, lmgw's rule,
//!     api §7.2), so it cannot name another server's; a URI with no authority (`urn:…`) is
//!     refused.
//!   - `resources/list` and `resources/templates/list`: lmgw's lists cut to the view's own
//!     server's entries, each URI (template) answered in the server's own terms.
//!
//!   **Overlapping prefixes** (lmgw lets one server's prefix be `fx` and another's `fx__x` or
//!   `fx_`): a tool's own name, and a URI's own authority, that starts with `_` or holds `__` is
//!   refused (and left out of the lists), since `fx` + `__` + `x__y` is also `fx__x`'s `y`. Kai's
//!   interim rule; telling the servers apart is lmgw's (its prefixes), the owner's call.
//!
//!   Every other request is `-32601`; notifications (`ui/notifications/size-changed`,
//!   `notifications/message`) are taken and change nothing (the size is the configuration's,
//!   §10.1). Kai's one request to the view is `ui/resource-teardown` ([`teardown`]) before the
//!   view goes.
//! - **Who decides a view's call** ([`decide`], K15 by caller): the caller is `app:<server>`.
//!   `policy.tools.<server>__<name>` decides first (`allow`, `confirm`, `deny`, `off`); without
//!   it a read-only tool runs, and a state-changing one runs for an app in
//!   `policy.trusted_callers` (`app:<server>`; decided by the owner, 2026-10-09) when its server
//!   marks it `destructiveHint: false`, and asks the owner for every other (a destructive tool
//!   asks even a trusted app: MCP's default for a tool that is not read-only is destructive).
//!   A `policy.tools.<server>__…` name the server's list does not hold is named in the view's
//!   notes and the log when its view opens.
//! - **The call's input and result** ([`ToolRun`]): a view shown for a tool call in Kai's
//!   conversation (`super::thread_apps`) gets `ui/notifications/tool-input` with the call's
//!   arguments once it said `ui/notifications/initialized` **and Kai knows the call runs**
//!   ([`Progress::runs`]: never for a call that waits for an approval), then `…/tool-result` (the
//!   call's result as MCP's `CallToolResult`: the content blocks as the server sent them,
//!   `structuredContent`, `isError`) or `…/tool-cancelled` (alone, without the input, for a call
//!   that never ran: declined, or its turn ended first); its `ui/initialize` answer names the
//!   tool (`hostContext.toolInfo.tool`, in its server's own terms) and the call
//!   (`hostContext.toolInfo.id`, the frames' `call_id`, when lmgw sent one). `overlay_app {server,
//!   uri}` passes no tool input or result (§10.1, K22): such a view gets none of the three.
//! - **Sizes:** a message from the view and every answer from lmgw are bounded by
//!   `overlay.app_max_mb` (default 64, lmgw's own MCP message default), the error naming it; Kai's
//!   app script refuses a bigger message before it leaves the view's page, and the GTK side
//!   before it reads it.
//! - **Requests in flight:** at most `overlay.app_max_calls` (default 16) per view, from the
//!   message to its answer handed to the view; one more is answered `-32000` naming the key. So a
//!   view's answers waiting for it are bounded too.
//! - **Lifetime:** the bridge runs while the widget does; hidden, the GTK side drops its end, which
//!   stops every call in flight (a dialog closes) and ends Kai's `/mcp` session.
//! - **The log** names the server, the tool and the decision, never arguments, results or a
//!   view's message.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lmgw_client::mcp_apps::{self, ToolServer};
use serde_json::{Map, Value, json};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

use crate::host::confirm::{ConfirmRequest, Confirmer, shown_text};
use crate::host::policy::{Caller, Decision, Policy, SharedPolicy};
use crate::host::settings::{Named, ToolState};
use crate::lmgw_link::Credentials;
use crate::lmgw_link::mcp::{Bound, McpError, McpSession};
use crate::screen_lock::ScreenLock;
use crate::voice::link::Pending;

/// Kai's host script for an `overlay_app` view (after `host.js`, in Kai's world).
pub const HOST_SCRIPT: &str = include_str!("../../assets/widgets/app-host.js");

/// What a view's message may carry when it is not JSON-RPC Kai answers (a message over the bound
/// is `-32600`, answered by Kai's app script).
const NO_METHOD: i64 = -32601;
const BAD_PARAMS: i64 = -32602;
const INTERNAL: i64 = -32603;
/// The owner said no, or could not be asked (MCP Apps: an implementation-defined error).
const DENIED: i64 = -32000;

/// lmgw's own tool labels (`lmgw__`, `docs__`, `kb__`): no southbound server has them.
pub(super) const RESERVED: [&str; 3] = ["lmgw", "docs", "kb"];

/// A tool's own name, or a URI's own authority, that cannot be another server's under an
/// overlapping prefix (module doc): it neither starts with `_` nor holds `__`.
fn own_part_ok(own: &str) -> bool {
    !own.starts_with('_') && !own.contains("__")
}

/// The authority of `rest` (what follows `scheme://`): up to the path, query or fragment.
fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

/// `scheme://rest` → `(scheme, rest)`, with an RFC 3986 scheme (a letter first, then letters,
/// digits, `+`, `-`, `.`): lmgw's own split (`mcp/resources/uri.rs`), so Kai namespaces exactly
/// the URIs lmgw does.
fn split(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = uri.split_once("://")?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some((scheme, rest))
}

/// `overlay_app`'s arguments, checked: the server's label and the view's namespaced URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub server: String,
    pub uri: String,
}

/// The label lmgw lists Kai's own hosted tools under (`hello.hosts_label`), while the link is up:
/// no MCP server's, so no view is shown under it ([`target`]).
static HOSTS_LABEL: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The link said Kai's hosted tools' label (`crate::core`, at each `Connected`).
pub fn set_hosts_label(label: Option<&str>) {
    *HOSTS_LABEL.write().unwrap_or_else(|e| e.into_inner()) = label.map(str::to_owned);
}

/// `server` is a label lmgw lists tools under (`<server>__<tool>`); `uri` a `ui://` URI in its
/// namespace as `/mcp` serves it (`ui://<server>__card/x`; a tool's `_meta.ui.resourceUri` there,
/// or the Chat `tool` frame's `ui_resource`). Neither lmgw's own labels nor Kai's own
/// (`hello.hosts_label`) is an MCP server's.
pub fn target(server: &str, uri: &str) -> Result<Target, String> {
    let own = HOSTS_LABEL
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    target_beside(server, uri, own.as_deref())
}

/// [`target`] with Kai's own label given (`None`: not known, the link is down).
pub fn target_beside(server: &str, uri: &str, kais: Option<&str>) -> Result<Target, String> {
    let server = server.trim();
    if server.is_empty()
        || !server
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(
            "overlay_app: server must be the server's label as lmgw lists its tools (letters, \
             digits, _ and -)"
                .into(),
        );
    }
    if let Some(r) = RESERVED
        .iter()
        .find(|r| server == **r || server.starts_with(&format!("{r}__")))
    {
        return Err(format!(
            "overlay_app: {r} is lmgw's own label (its built-in tools), no MCP server's"
        ));
    }
    if kais.is_some_and(|k| k == server) {
        return Err(format!(
            "overlay_app: {server} is the label of Kai's own tools (lmgw's hosts_label), no MCP \
             server's"
        ));
    }
    let uri = uri.trim();
    let Some(rest) = uri.strip_prefix("ui://") else {
        return Err("overlay_app: uri must be a ui:// resource".into());
    };
    let Some(own) = rest.strip_prefix(&format!("{server}__")) else {
        return Err(format!(
            "overlay_app: uri is not in server {server}'s namespace as lmgw's /mcp names it \
             (ui://{server}__…); a server without a tool prefix cannot be told apart from the \
             others, so Kai shows only prefixed servers' views"
        ));
    };
    if !own_part_ok(authority(own)) {
        return Err(format!(
            "overlay_app: uri's own authority (after {server}__) starts with _ or holds __, so it \
             could be another server's whose prefix starts with {server}_ (lmgw allows such \
             prefixes); Kai shows no such view"
        ));
    }
    Ok(Target {
        server: server.to_owned(),
        uri: uri.to_owned(),
    })
}

/// One tool of the view's server as `/mcp` lists it to Kai.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewTool {
    /// The name `/mcp` lists the tool under: `<server>__<name>`, or `<server's name>__<name>` when
    /// a collision moved it (or the owner's rename). What Kai calls and what `policy.tools` keys.
    pub exposed: String,
    /// The tool's name as its server lists it: the name its view calls it by.
    pub own: String,
    /// `<label>__<own>` when that is not [`ViewTool::exposed`] (a moved or renamed tool): the name
    /// the tool answered to before, so a `policy.tools` entry written for it still counts
    /// ([`decide`]).
    pub alias: Option<String>,
    /// The name of the server whose tool it is, when lmgw's list says ([`ToolServer::name`]):
    /// servers that share a tool prefix share the label a view is known by, not this.
    pub by: Option<String>,
    /// `_meta.ui.visibility` includes `"app"` (MCP Apps: absent is both).
    pub app: bool,
    pub read_only: bool,
    /// MCP's `destructiveHint` (its default is true for a tool that is not read-only).
    pub destructive: bool,
    /// The description's first line, for the owner's dialog.
    pub description: String,
}

/// What a view may reach: its server's tools as listed when it was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub server: String,
    pub tools: Vec<ViewTool>,
    /// The list carried lmgw's `_meta["lmgw/server"]` stamps: the tools are the stamped ones of
    /// this server, by their own names, whatever `/mcp` calls them. An older lmgw's list has none:
    /// the tools are the ones named `<server>__…`.
    pub stamped: bool,
}

/// `_meta.ui.visibility` as a list; a single string is a list of one, anything else is absent
/// (lmgw's reading, api §7.5).
fn visibility(tool: &Value) -> Option<Vec<String>> {
    match &tool["_meta"]["ui"]["visibility"] {
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        Value::String(s) => Some(vec![s.clone()]),
        _ => None,
    }
}

/// A listed tool as the view's: `exposed` its listed name, `own` its server's name for it.
fn view_tool(
    t: &Value,
    exposed: &str,
    own: String,
    alias: Option<String>,
    by: Option<String>,
) -> ViewTool {
    let read_only = t["annotations"]["readOnlyHint"].as_bool() == Some(true);
    ViewTool {
        exposed: exposed.to_owned(),
        own,
        alias,
        by,
        app: visibility(t).is_none_or(|v| v.iter().any(|s| s == "app")),
        read_only,
        destructive: !read_only && t["annotations"]["destructiveHint"].as_bool() != Some(false),
        description: t["description"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

impl Rules {
    /// From `/mcp`'s `tools/list` result: the tools named `<server>__…`.
    pub fn of_list(server: &str, list: &Value) -> Rules {
        let listed = || list["tools"].as_array().into_iter().flatten();
        if listed().any(|t| ToolServer::of_tool(t).is_some()) {
            return Rules::of_stamps(server, list);
        }
        let prefix = format!("{server}__");
        let tools = list["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let exposed = t["name"].as_str()?;
                // A name that could be another server's (`<server>__x__y` is also `x`'s under
                // prefix `<server>__x`) is not the view's.
                let own = exposed
                    .strip_prefix(&prefix)
                    .filter(|n| !n.is_empty() && own_part_ok(n))?;
                Some(view_tool(t, exposed, own.to_owned(), None, None))
            })
            .collect();
        Rules {
            server: server.to_owned(),
            tools,
            stamped: false,
        }
    }

    /// From a list lmgw stamped (`_meta["lmgw/server"]`, [`ToolServer`]): the tools of the server
    /// whose `label` is `server` (the view's server: Kai's `overlay_app` and a call's frame name
    /// the label), or, when no stamp has that label, whose `name` is. A tool is the server's by
    /// its stamp, not by its exposed name, so a tool a collision moved to `<server name>__x` is
    /// the view's, and another server's `<label>__x` never is. The tool's own name comes from the
    /// stamp.
    fn of_stamps(server: &str, list: &Value) -> Rules {
        let stamped: Vec<(&Value, ToolServer)> = list["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| Some((t, ToolServer::of_tool(t)?)))
            .collect();
        let by_label = stamped.iter().any(|(_, s)| s.label == server);
        let tools = stamped
            .iter()
            .filter(|(_, s)| {
                if by_label {
                    s.label == server
                } else {
                    s.name == server
                }
            })
            .filter_map(|(t, s)| {
                let exposed = t["name"].as_str().filter(|n| !n.is_empty())?;
                let before = format!("{}__{}", s.label, s.tool);
                let alias = (before != exposed).then_some(before);
                Some(view_tool(
                    t,
                    exposed,
                    s.tool.clone(),
                    alias,
                    Some(s.name.clone()),
                ))
            })
            .collect();
        Rules {
            server: server.to_owned(),
            tools,
            stamped: true,
        }
    }

    /// The view of a tool call's tool `exposed`: when the list is stamped and names that tool's
    /// server, only that server's tools stay. Servers that share a tool prefix share the label a
    /// view is known by; the call says which of them the view is for.
    pub fn narrow_to_call(&mut self, exposed: &str) {
        let by = self
            .tools
            .iter()
            .find(|t| t.exposed == exposed)
            .and_then(|t| t.by.clone());
        if let Some(by) = by {
            self.tools.retain(|t| t.by.as_deref() == Some(by.as_str()));
        }
    }

    /// The tool a view's `tools/call` names (the server's own name), if the view may call it.
    pub fn tool(&self, name: &str) -> Result<&ViewTool, String> {
        if self.stamped {
            // The stamp says each tool's own name, so no overlapping prefix can make `name`
            // another server's: the view's tool is the stamped one of that name.
            let mut same = self.tools.iter().filter(|t| t.own == name);
            let (Some(t), more) = (same.next(), same.next()) else {
                return Err(format!(
                    "{} offers Kai no tool of that name (a view calls only its own server's \
                     tools, as lmgw's /mcp listed them to Kai when the view was shown)",
                    self.server
                ));
            };
            if more.is_some() {
                return Err(format!(
                    "more than one server under {} lists a tool of that name, and Kai cannot tell \
                     which the view means (no single tool of that name)",
                    self.server
                ));
            }
            if !t.app {
                return Err(format!(
                    "{} is not for views (its _meta.ui.visibility leaves out \"app\")",
                    t.exposed
                ));
            }
            return Ok(t);
        }
        if !own_part_ok(name) {
            return Err(format!(
                "a view calls no tool whose name starts with _ or holds __: under another \
                 server's prefix ({}_… or {}__…, which lmgw allows) it could be that server's",
                self.server, self.server
            ));
        }
        let exposed = format!("{}__{name}", self.server);
        let Some(t) = self.tools.iter().find(|t| t.exposed == exposed) else {
            return Err(format!(
                "{} offers Kai no tool of that name (a view calls only its own server's tools, as \
                 lmgw's /mcp listed them to Kai when the view was shown)",
                self.server
            ));
        };
        if !t.app {
            return Err(format!(
                "{exposed} is not for views (its _meta.ui.visibility leaves out \"app\")"
            ));
        }
        Ok(t)
    }

    /// The URI a view's `resources/read` names, in its server's namespace (lmgw's rule:
    /// `scheme://<server>__<rest>`; one already in it stays).
    pub fn resource(&self, uri: &str) -> Result<String, String> {
        let Some((scheme, rest)) = split(uri).filter(|(_, rest)| !rest.is_empty()) else {
            return Err(
                "a view reads only its own server's resources, and a URI without an authority \
                 cannot be told to be one"
                    .into(),
            );
        };
        let prefix = format!("{}__", self.server);
        let own = rest.strip_prefix(&prefix).unwrap_or(rest);
        if !own_part_ok(authority(own)) {
            return Err(format!(
                "a view reads only its own server's resources, and a URI whose authority starts \
                 with _ or holds __ could be another server's (a prefix {}_… or {}__…, which \
                 lmgw allows)",
                self.server, self.server
            ));
        }
        Ok(format!("{scheme}://{prefix}{own}"))
    }

    /// A URI (or URI template) of lmgw's lists in the server's own terms, if it is the server's:
    /// [`Rules::resource`]'s inverse.
    pub fn own(&self, uri: &str) -> Option<String> {
        let (scheme, rest) = split(uri)?;
        let own = rest.strip_prefix(&self.server)?.strip_prefix("__")?;
        own_part_ok(authority(own)).then(|| format!("{scheme}://{own}"))
    }
}

/// The caller a view's calls run as: `app:<server>` (`policy.trusted_callers` names it so).
pub fn caller(server: &str) -> Caller {
    Caller::Named(Named {
        kind: "app".into(),
        name: server.to_owned(),
    })
}

/// How strict a state is: the order `strictest` picks by.
fn strictness(state: ToolState) -> u8 {
    match state {
        ToolState::Allow => 0,
        ToolState::AllowTrusted => 1,
        ToolState::Confirm => 2,
        ToolState::Deny => 3,
        ToolState::Off => 4,
    }
}

/// The `policy.tools` entry that decides a view's call, and the name it is written under: the
/// tool's exposed name's, and, for a tool a collision moved or the owner renamed, also its
/// `<label>__<own>` spelling's (the name it answered to before); the strictest wins. The old
/// spelling only **tightens** (`confirm`, `deny`, `off`): that name may belong to another server's
/// tool now (the one the collision left it to), so an `allow` written under it is not carried
/// over, never loosening this tool by another server's setting.
fn effective_state<'a>(policy: &Policy, tool: &'a ViewTool) -> (&'a str, Option<ToolState>) {
    let own = policy
        .state(&tool.exposed)
        .map(|s| (tool.exposed.as_str(), s));
    let carried = tool.alias.as_deref().and_then(|a| {
        let state = policy.state(a)?;
        matches!(state, ToolState::Off | ToolState::Deny | ToolState::Confirm).then_some((a, state))
    });
    match own
        .into_iter()
        .chain(carried)
        .max_by_key(|(_, s)| strictness(*s))
    {
        Some((name, state)) => (name, Some(state)),
        None => (tool.exposed.as_str(), None),
    }
}

/// K15 for a view's call of `tool` as `caller` (module doc): `policy.tools.<exposed name>` first,
/// then read-only runs, a trusted app's call of a tool its server marks `destructiveHint: false`
/// runs (decided by the owner, 2026-10-09; the destructive limit is provisional, a draft of
/// 2026-10-09), every other asks.
pub fn decide(policy: &Policy, tool: &ViewTool, caller: &Caller) -> Decision {
    let (name, state) = effective_state(policy, tool);
    match state {
        Some(state @ (ToolState::Off | ToolState::Deny)) => Decision::Deny(format!(
            "{name} is switched off for Kai's views (policy.tools.{name} = \"{}\")",
            if state == ToolState::Off {
                "off"
            } else {
                "deny"
            }
        )),
        Some(ToolState::Allow) => Decision::Allow,
        Some(ToolState::AllowTrusted) if policy.trusted_caller(caller) => Decision::Allow,
        Some(ToolState::AllowTrusted) => Decision::Confirm {
            why: "policy.tools says allow_trusted and the app is not in policy.trusted_callers",
        },
        Some(ToolState::Confirm) => Decision::Confirm {
            why: "policy.tools says confirm",
        },
        None if tool.read_only => Decision::Allow,
        None if policy.trusted_caller(caller) && !tool.destructive => Decision::Allow,
        None if policy.trusted_caller(caller) => Decision::Confirm {
            why: "it is destructive (MCP's destructiveHint, true unless the server says false): \
                  a trusted app's views are asked for such a tool unless policy.tools allows it",
        },
        None => Decision::Confirm {
            why: "it changes state and the app is not in policy.trusted_callers (K15)",
        },
    }
}

/// How a tool call a view was shown for ended (`super::thread_apps`).
#[derive(Clone, PartialEq)]
pub enum Outcome {
    /// Its result as MCP's `CallToolResult` (`content`, `structuredContent`, `isError`), and the
    /// `(server_label, ui_resource)` lmgw's `tool` frame named, if it named both.
    Result {
        result: Value,
        ui: Option<(String, String)>,
    },
    /// It never ran to its end; the reason goes to the view.
    Cancelled(String),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the result: it is the caller's content.
        match self {
            Outcome::Result { ui, .. } => write!(f, "Result(ui: {ui:?})"),
            Outcome::Cancelled(why) => write!(f, "Cancelled({why})"),
        }
    }
}

/// Where a tool call a view is shown for stands, as `super::thread_apps` learns it from the frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// Kai knows the call runs (or ran): its arguments may go to the view. A call that waits for
    /// the owner's approval, or whose turn may still stop on one, does not run yet.
    pub runs: bool,
    /// How it ended, once it did (a result means it ran).
    pub outcome: Option<Outcome>,
}

impl Progress {
    /// Ended with this outcome; a result means it ran.
    pub fn ended(runs: bool, outcome: Outcome) -> Progress {
        Progress {
            runs: runs || matches!(outcome, Outcome::Result { .. }),
            outcome: Some(outcome),
        }
    }
}

/// The tool call in Kai's conversation a view is shown for: what the bridge sends it unasked.
pub struct ToolRun {
    /// The call's arguments (`ui/notifications/tool-input`), sent once it [`Progress::runs`].
    pub arguments: Map<String, Value>,
    /// The tool as `/mcp` lists it, in its server's own terms (`hostContext.toolInfo`).
    pub tool: Option<Value>,
    /// The call's id as lmgw's frames carry it (`hostContext.toolInfo.id`), verbatim.
    pub call_id: Option<String>,
    pub progress: watch::Receiver<Progress>,
}

/// A notification to the view.
fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// The view's `tool-input`, then its `tool-result` or `tool-cancelled`, once it said
/// `ui/notifications/initialized` (MCP Apps' lifecycle; `tool-input` at most once and before the
/// result). The input goes only once the call runs ([`Progress::runs`]); a call that ends without
/// having run (declined, its turn ended while it waited) gets `tool-cancelled` alone.
///
/// Nothing is delivered while the screen is locked (K32): the delivery waits for the unlock, then
/// sends what it holds (a call that ended while locked delivers its result after the unlock, if
/// the view still exists; the bridge ending drops it).
async fn deliver(
    run: ToolRun,
    mut initialized: watch::Receiver<bool>,
    out: async_channel::Sender<String>,
    screen: ScreenLock,
) {
    if initialized.wait_for(|i| *i).await.is_err() {
        return;
    }
    let mut progress = run.progress;
    // Nobody follows the call any more (Kai's voice session went).
    let gone = || Outcome::Cancelled("Kai no longer follows the call".into());
    let now = match progress.wait_for(|p| p.runs || p.outcome.is_some()).await {
        Ok(p) => p.clone(),
        Err(_) => Progress::ended(false, gone()),
    };
    if now.runs || matches!(now.outcome, Some(Outcome::Result { .. })) {
        let input = notification(
            "ui/notifications/tool-input",
            json!({"arguments": run.arguments}),
        );
        screen.unlocked().await;
        if out.send(input.to_string()).await.is_err() {
            return;
        }
    }
    let ended = match now.outcome {
        Some(o) => o,
        None => match progress.wait_for(|p| p.outcome.is_some()).await {
            Ok(p) => p.outcome.clone().unwrap_or_else(gone),
            Err(_) => gone(),
        },
    };
    let last = match ended {
        Outcome::Result { result, .. } => notification("ui/notifications/tool-result", result),
        Outcome::Cancelled(reason) => {
            notification("ui/notifications/tool-cancelled", json!({"reason": reason}))
        }
    };
    screen.unlocked().await;
    let _ = out.send(last.to_string()).await;
}

/// The view's document and what was refused of what its `_meta.ui` asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub html: String,
    /// One line per refusal (the show's answer, `overlay_list`).
    pub notes: Vec<String>,
}

/// The view's document from `resources/read`'s result for `uri`: the content of that URI (or the
/// only one), as text or base64 `blob`, of an HTML type.
pub fn resource(read: &Value, uri: &str) -> Result<Resource, String> {
    let contents = read["contents"].as_array().cloned().unwrap_or_default();
    let c = contents
        .iter()
        .find(|c| c["uri"] == uri)
        .or_else(|| (contents.len() == 1).then(|| &contents[0]))
        .ok_or_else(|| format!("overlay_app: lmgw's read of {uri} holds no content for it"))?;
    let mime = c["mimeType"].as_str().unwrap_or_default();
    let base = mime.split(';').next().unwrap_or_default().trim();
    if !base.eq_ignore_ascii_case("text/html") {
        return Err(format!(
            "overlay_app: {uri} is not an MCP Apps view (its type is {}, not {})",
            if mime.is_empty() { "not given" } else { mime },
            mcp_apps::MIME_TYPE
        ));
    }
    let html = match (c["text"].as_str(), c["blob"].as_str()) {
        (Some(t), _) => t.to_owned(),
        (None, Some(b)) => String::from_utf8(
            lmgw_client::base64::decode(b)
                .ok_or_else(|| format!("overlay_app: {uri}'s blob is not base64"))?,
        )
        .map_err(|_| format!("overlay_app: {uri} is not UTF-8 text"))?,
        _ => return Err(format!("overlay_app: {uri} has neither text nor a blob")),
    };
    if html.trim().is_empty() {
        return Err(format!("overlay_app: {uri} is empty"));
    }
    Ok(Resource {
        notes: refused(&c["_meta"]["ui"]),
        html,
    })
}

/// What a view's `_meta.ui` asks that Kai's sandbox refuses (§10.1): each CSP list with its
/// entries, each permission.
pub fn refused(ui: &Value) -> Vec<String> {
    let mut notes = Vec::new();
    for key in [
        "connectDomains",
        "resourceDomains",
        "frameDomains",
        "baseUriDomains",
    ] {
        let asked: Vec<String> = ui["csp"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|d| shown_text(d).into_owned())
            .collect();
        if !asked.is_empty() {
            notes.push(format!(
                "refused: csp {key} {} (a view reaches no network)",
                asked.join(", ")
            ));
        }
    }
    if let Some(p) = ui["permissions"].as_object().filter(|p| !p.is_empty()) {
        let names: Vec<String> = p.keys().map(|k| shown_text(k).into_owned()).collect();
        notes.push(format!(
            "refused: permissions {} (a view gets none)",
            names.join(", ")
        ));
    }
    notes
}

/// The answer to the view's `ui/initialize`: Kai's host capabilities and context.
pub fn initialize_result(width: u32, height: u32) -> Value {
    json!({
        "protocolVersion": mcp_apps::REVISION,
        "hostCapabilities": {
            "serverTools": {},
            "serverResources": {},
            "logging": {},
        },
        "hostInfo": {"name": "kai", "version": env!("CARGO_PKG_VERSION")},
        "hostContext": {
            "theme": "dark",
            "styles": {"variables": {
                "--color-background-primary": "#232629",
                "--color-text-primary": "#eff0f1",
                "--color-border-primary": "#3daee9",
                "--font-sans": "\"Noto Sans\", sans-serif",
            }},
            "displayMode": "inline",
            "availableDisplayModes": ["inline"],
            "containerDimensions": {"width": width, "height": height},
            "platform": "desktop",
            "deviceCapabilities": {"touch": false, "hover": true},
        },
    })
}

/// `/mcp`, as the bridge uses it (a fake in the tests).
pub trait Upstream: Send + Sync + 'static {
    fn request(&self, method: &'static str, params: Value) -> Pending<Result<Value, McpError>>;
    fn close(&self) -> Pending<()>;
}

/// An [`McpSession`] the bridge's tasks share.
pub struct Shared(pub Arc<McpSession>);

impl Upstream for Shared {
    fn request(&self, method: &'static str, params: Value) -> Pending<Result<Value, McpError>> {
        let s = self.0.clone();
        Box::pin(async move { s.request(method, params).await })
    }

    fn close(&self) -> Pending<()> {
        let s = self.0.clone();
        Box::pin(async move { s.close().await })
    }
}

/// The bridge's ends the GTK side holds for a widget: the view's messages in, Kai's answers out.
/// Dropping `to_bridge` ends the bridge.
#[derive(Clone)]
pub struct Ends {
    pub to_bridge: mpsc::UnboundedSender<String>,
    pub from_bridge: async_channel::Receiver<String>,
}

impl std::fmt::Debug for Ends {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ends")
    }
}

impl PartialEq for Ends {
    fn eq(&self, other: &Self) -> bool {
        self.to_bridge.same_channel(&other.to_bridge)
    }
}

impl Eq for Ends {}

/// How many requests of a view may be in flight, and the key that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calls {
    pub max: usize,
    /// The setting as an error names it (`overlay.app_max_calls = 16`).
    pub says: String,
}

/// One view's bridge.
pub struct Bridge {
    upstream: Arc<dyn Upstream>,
    rules: Rules,
    confirm: Arc<dyn Confirmer>,
    bound: Bound,
    calls: Calls,
    /// The view's size as the configuration gives it (`hostContext.containerDimensions`).
    size: (u32, u32),
    /// The owner's dialog for one of this view's calls is open (one at a time).
    asking: AtomicBool,
    /// K15's policy, live (`[policy]`): who the view's calls run for ([`decide`]).
    policy: SharedPolicy,
    /// The tool call the view was shown for, if any ([`ToolRun`]), until [`Bridge::spawn`].
    run: Option<ToolRun>,
    /// That call's `{id, tool}`, for `ui/initialize`'s `hostContext.toolInfo`.
    tool_info: Option<Value>,
    /// The view said `ui/notifications/initialized`.
    initialized: watch::Sender<bool>,
    /// The screen lock (K32): while locked, every request that would reach lmgw is refused and
    /// nothing is delivered unasked.
    screen: ScreenLock,
}

/// A JSON-RPC error answer.
fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The id of Kai's one request to a view, `ui/resource-teardown` (the view's own requests' ids
/// are its own: Kai's answers carry them, never this).
pub const TEARDOWN_ID: &str = "kai-teardown";

/// `ui/resource-teardown`, which Kai sends the view before the view goes (MCP Apps: the host MUST
/// send it before tearing the view down). Kai does not wait for the answer (the spec's SHOULD):
/// a hide is immediate, nothing lingers unseen (§10.1).
pub fn teardown(reason: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": TEARDOWN_ID,
        "method": "ui/resource-teardown",
        "params": {"reason": reason},
    })
    .to_string()
}

/// The open dialog's mark, cleared when the call stops waiting for it (answered, or the view
/// gone).
struct Asking<'a>(&'a AtomicBool);

impl<'a> Asking<'a> {
    fn take(flag: &'a AtomicBool) -> Option<Asking<'a>> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Asking(flag))
    }
}

impl Drop for Asking<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Bridge {
    pub fn new(
        upstream: Arc<dyn Upstream>,
        rules: Rules,
        confirm: Arc<dyn Confirmer>,
        bound: Bound,
        calls: Calls,
        size: (u32, u32),
    ) -> Bridge {
        Bridge {
            upstream,
            rules,
            confirm,
            bound,
            calls,
            size,
            asking: AtomicBool::new(false),
            policy: SharedPolicy::default(),
            run: None,
            tool_info: None,
            initialized: watch::channel(false).0,
            screen: ScreenLock::default(),
        }
    }

    /// The screen lock the bridge refuses by (without it: never locked).
    pub fn with_screen(mut self, screen: ScreenLock) -> Bridge {
        self.screen = screen;
        self
    }

    /// K15's live policy for the view's calls (without it: `[policy]`'s defaults).
    pub fn with_policy(mut self, policy: SharedPolicy) -> Bridge {
        self.policy = policy;
        self
    }

    /// The tool call the view is shown for: its input and result go to the view.
    pub fn with_run(mut self, run: ToolRun) -> Bridge {
        self.tool_info = run.tool.clone().map(|tool| match &run.call_id {
            Some(id) => json!({"id": id, "tool": tool}),
            None => json!({"tool": tool}),
        });
        self.run = Some(run);
        self
    }

    /// Runs the bridge until the GTK side drops its end; returns that end. At most
    /// `calls.max` messages are in flight, each until its answer is handed to the GTK side.
    pub fn spawn(mut self) -> Ends {
        let run = self.run.take();
        let (to_bridge, mut inbox) = mpsc::unbounded_channel::<String>();
        let max = self.calls.max.max(1);
        // Every message in flight holds a permit until its answer is taken (the channel's room),
        // and the refusal of one over the bound waits for room too.
        let (outbox, from_bridge) = async_channel::bounded::<String>(max + 1);
        let permits = Arc::new(Semaphore::new(max));
        let initialized = self.initialized.subscribe();
        let bridge = Arc::new(self);
        tokio::spawn(async move {
            let mut calls = tokio::task::JoinSet::new();
            // The call's input and result, unasked; it ends with the bridge like any call.
            if let Some(run) = run {
                calls.spawn(deliver(
                    run,
                    initialized,
                    outbox.clone(),
                    bridge.screen.clone(),
                ));
            }
            loop {
                tokio::select! {
                    m = inbox.recv() => match m {
                        Some(text) => match permits.clone().try_acquire_owned() {
                            Ok(permit) => {
                                let (b, out) = (bridge.clone(), outbox.clone());
                                calls.spawn(async move {
                                    if let Some(a) = b.answer(&text).await {
                                        let _ = out.send(a.to_string()).await;
                                    }
                                    drop(permit);
                                });
                            }
                            Err(_) => {
                                if let Some(a) = bridge.busy(&text)
                                    && outbox.send(a.to_string()).await.is_err()
                                {
                                    break;
                                }
                            }
                        },
                        None => break,
                    },
                    Some(_) = calls.join_next(), if !calls.is_empty() => {}
                }
            }
            // The view is gone: every call in flight stops (a dialog closes), the session ends.
            calls.abort_all();
            while calls.join_next().await.is_some() {}
            tracing::debug!(server = bridge.rules.server, "app view: the bridge ends");
            bridge.upstream.close().await;
        });
        Ends {
            to_bridge,
            from_bridge,
        }
    }

    /// The answer to a request that came while `calls.max` were in flight (`None` for anything
    /// else: a notification needs none).
    fn busy(&self, text: &str) -> Option<Value> {
        if text.len() as u64 > self.bound.bytes {
            return None;
        }
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) else {
            return None;
        };
        let (Some(id), Some(_)) = (m.get("id"), m.get("method").and_then(Value::as_str)) else {
            return None;
        };
        tracing::info!(
            server = self.rules.server,
            "app view: a request over the bound of requests in flight; refused"
        );
        Some(error(
            id,
            DENIED,
            format!(
                "the view has {} requests in flight already ({}); wait for their answers",
                self.calls.max, self.calls.says
            ),
        ))
    }

    /// The answer to one message of the view (`None` for a notification or a response).
    pub async fn answer(&self, text: &str) -> Option<Value> {
        let server = &self.rules.server;
        if text.len() as u64 > self.bound.bytes {
            // Kai's app script refuses such a message before it is posted, and the GTK side
            // before it reads it; this is the last of the three. Its id is not read.
            tracing::warn!(
                server,
                bytes = text.len(),
                "app view: a message over {}; ignored",
                self.bound.says
            );
            return None;
        }
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) else {
            tracing::debug!(
                server,
                "app view: a message that is not a JSON-RPC object; ignored"
            );
            return None;
        };
        let id = m.get("id").cloned();
        let method = m.get("method").and_then(Value::as_str);
        let (Some(id), Some(method)) = (id, method) else {
            // A notification, or a response (to `ui/resource-teardown`, Kai's only request).
            if method == Some("ui/notifications/initialized") {
                self.initialized.send_replace(true);
            }
            if let Some(n) = method {
                let known = [
                    "ui/notifications/initialized",
                    "ui/notifications/size-changed",
                    "notifications/message",
                ];
                if known.contains(&n) {
                    tracing::debug!(server, notification = n, "app view: a notification");
                }
            }
            return None;
        };
        let params = m.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "ui/initialize" => {
                let mut r = initialize_result(self.size.0, self.size.1);
                if let Some(info) = self.tool_info.clone() {
                    r["hostContext"]["toolInfo"] = info;
                }
                ok(&id, r)
            }
            "ping" => ok(&id, json!({})),
            "ui/request-display-mode" => ok(&id, json!({"mode": "inline"})),
            // Every request that would reach lmgw: refused while the screen is locked (or not yet
            // known to be unlocked), nothing forwarded.
            "tools/call" | "resources/read" | "resources/list" | "resources/templates/list"
                if self.screen.is_locked() =>
            {
                tracing::info!(
                    server,
                    method,
                    "app view: a request refused, the screen is locked"
                );
                error(&id, DENIED, crate::screen_lock::REFUSAL)
            }
            "tools/call" => self.call(&id, &params).await,
            "resources/read" => self.read(&id, &params).await,
            "resources/list" => self.list(&id, &params, Listing::Resources).await,
            "resources/templates/list" => self.list(&id, &params, Listing::Templates).await,
            other => {
                tracing::debug!(server, "app view: a request Kai does not offer");
                error(
                    &id,
                    NO_METHOD,
                    format!("Kai's overlays do not offer {}", shown_text(other)),
                )
            }
        })
    }

    async fn call(&self, id: &Value, params: &Value) -> Value {
        let server = &self.rules.server;
        let Some(name) = params["name"].as_str() else {
            return error(id, BAD_PARAMS, "tools/call needs a name");
        };
        let arguments = match &params["arguments"] {
            Value::Null => Map::new(),
            Value::Object(a) => a.clone(),
            _ => return error(id, BAD_PARAMS, "tools/call's arguments must be an object"),
        };
        let tool = match self.rules.tool(name) {
            Ok(t) => t.clone(),
            Err(why) => {
                tracing::info!(
                    server,
                    "app view: a tool call refused (not its server's, or not for views)"
                );
                return error(id, BAD_PARAMS, why);
            }
        };
        let caller = caller(server);
        let decision = {
            let policy = self.policy.read().unwrap_or_else(|e| e.into_inner());
            decide(&policy, &tool, &caller)
        };
        match decision {
            Decision::Allow => {}
            Decision::Deny(why) => {
                tracing::info!(
                    server,
                    tool = tool.exposed,
                    "app view: a call refused by policy.tools"
                );
                return error(id, DENIED, why);
            }
            Decision::Confirm { why } => {
                if let Err(e) = self.ask(id, &tool, &caller, &arguments, why).await {
                    return e;
                }
            }
        }
        tracing::info!(
            server,
            tool = tool.exposed,
            read_only = tool.read_only,
            "app view: a call"
        );
        let sent = json!({"name": tool.exposed, "arguments": arguments});
        match self.upstream.request("tools/call", sent).await {
            Ok(result) => ok(id, result),
            Err(e) => self.failed(id, &tool.exposed, e),
        }
    }

    /// The owner's dialog for a view's call (one per view at a time); `Err` is the answer to the
    /// view when it did not run.
    async fn ask(
        &self,
        id: &Value,
        tool: &ViewTool,
        caller: &Caller,
        arguments: &Map<String, Value>,
        why: &'static str,
    ) -> Result<(), Value> {
        let server = &self.rules.server;
        tracing::debug!(
            server,
            tool = tool.exposed,
            why,
            "app view: the owner is asked"
        );
        // One dialog per view: a view cannot stack the owner's screen with them.
        let Some(_asking) = Asking::take(&self.asking) else {
            tracing::info!(
                server,
                tool = tool.exposed,
                "app view: a call refused, a confirmation is already open"
            );
            return Err(error(
                id,
                DENIED,
                format!(
                    "a confirmation is already open for this view; {} was not asked, call it \
                         again once that one is answered",
                    tool.exposed
                ),
            ));
        };
        let (reply, answer) = oneshot::channel();
        self.confirm.ask(ConfirmRequest {
            tool: tool.exposed.clone(),
            description: tool.description.clone(),
            caller: format!("the view of {}", caller.describe()),
            lines: lines(arguments),
            destructive: tool.destructive,
            deadline: None,
            options: Vec::new(),
            notes: Vec::new(),
            reply,
        });
        match answer.await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    server,
                    tool = tool.exposed,
                    "app view: the owner denied a call"
                );
                return Err(error(
                    id,
                    DENIED,
                    format!("the owner denied {}", tool.exposed),
                ));
            }
            Err(_) => {
                tracing::warn!(
                    server,
                    tool = tool.exposed,
                    "app view: the confirmation could not be shown; denied"
                );
                return Err(error(
                    id,
                    DENIED,
                    format!(
                        "{} needs the owner's yes, and Kai could not ask",
                        tool.exposed
                    ),
                ));
            }
        }
        Ok(())
    }

    async fn read(&self, id: &Value, params: &Value) -> Value {
        let server = &self.rules.server;
        let Some(asked) = params["uri"].as_str() else {
            return error(id, BAD_PARAMS, "resources/read needs a uri");
        };
        let uri = match self.rules.resource(asked) {
            Ok(u) => u,
            Err(why) => {
                tracing::info!(server, "app view: a read refused (not its server's)");
                return error(id, BAD_PARAMS, why);
            }
        };
        tracing::info!(server, "app view: a read");
        match self
            .upstream
            .request("resources/read", json!({"uri": uri}))
            .await
        {
            Ok(mut result) => {
                // The view asked in its server's own terms: its URI comes back as it asked.
                if let Some(contents) = result["contents"].as_array_mut() {
                    for c in contents {
                        if c["uri"] == uri.as_str() {
                            c["uri"] = Value::String(asked.to_owned());
                        }
                    }
                }
                ok(id, result)
            }
            Err(e) => self.failed(id, "the read", e),
        }
    }

    /// `resources/list` or `resources/templates/list`: lmgw's list cut to the view's own
    /// server's entries, in its own terms. A cursor passes on (lmgw answers in one page and
    /// refuses one, `-32602`).
    async fn list(&self, id: &Value, params: &Value, which: Listing) -> Value {
        let server = &self.rules.server;
        let mut sent = json!({});
        if let Some(c) = params.get("cursor").filter(|c| !c.is_null()) {
            sent["cursor"] = c.clone();
        }
        let (method, key, field) = which.names();
        tracing::info!(server, "app view: {method}");
        match self.upstream.request(method, sent).await {
            Ok(mut result) => {
                if !result.is_object() {
                    result = json!({});
                }
                let own: Vec<Value> = result[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        let own = self.rules.own(item.get(field)?.as_str()?)?;
                        let mut item = item.clone();
                        item[field] = Value::String(own);
                        Some(item)
                    })
                    .collect();
                result[key] = Value::Array(own);
                ok(id, result)
            }
            Err(e) => self.failed(id, method, e),
        }
    }

    fn failed(&self, id: &Value, what: &str, e: McpError) -> Value {
        match e {
            McpError::Rpc {
                code,
                message,
                data,
            } => {
                let mut v = error(id, code, message);
                if let Some(d) = data {
                    v["error"]["data"] = d;
                }
                v
            }
            McpError::Link(text) => {
                tracing::warn!(
                    server = self.rules.server,
                    "app view: {what} did not reach lmgw: {text}"
                );
                error(id, INTERNAL, text)
            }
        }
    }
}

/// The two lists a view may ask for.
#[derive(Debug, Clone, Copy)]
enum Listing {
    Resources,
    Templates,
}

impl Listing {
    /// The method, the result's list and each entry's URI field.
    fn names(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Listing::Resources => ("resources/list", "resources", "uri"),
            Listing::Templates => (
                "resources/templates/list",
                "resourceTemplates",
                "uriTemplate",
            ),
        }
    }
}

/// K22's compact lines of a call's arguments for the owner's dialog (never logged).
fn lines(args: &Map<String, Value>) -> Vec<String> {
    args.iter()
        .map(|(k, v)| {
            let shown = match v {
                Value::String(s) => shown_text(s).into_owned(),
                other => crate::host::registry::json_shown(other),
            };
            format!("{}: {shown}", shown_text(k))
        })
        .collect()
}

/// What `overlay_app` needs besides its arguments: the pairing's credentials, the HTTP client, the
/// owner's dialog, K15's policy and the bounds. The views shown for a call in Kai's conversation
/// (`super::thread_apps`) open through it too.
#[derive(Clone)]
pub struct Apps {
    pub creds: watch::Receiver<Option<Credentials>>,
    pub client: reqwest::Client,
    /// `lmgw.timeout_s`, for connecting.
    pub connect: std::time::Duration,
    pub bound: Bound,
    /// `overlay.app_max_calls`.
    pub calls: Calls,
    pub confirm: Arc<dyn Confirmer>,
    /// `[policy]`, live: who a view's calls run for ([`decide`]).
    pub policy: SharedPolicy,
    /// `/mcp`'s tool list as last fetched, so a call in Kai's conversation whose tool has no view
    /// opens no session to learn it ([`lists::Lists`]).
    pub lists: Arc<lists::Lists>,
    /// The screen lock (K32): its views' requests to lmgw are refused while locked.
    pub screen: ScreenLock,
}

/// The show's body: the view's document and its bridge, with the refusals as notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBody {
    /// The view's server's label: Kai's strip on an interactive view names it (§10.1).
    pub server: String,
    pub html: String,
    pub ends: Ends,
    pub notes: Vec<String>,
    /// `overlay.app_max_mb`, for Kai's app script and the GTK side (a view's message).
    pub bound: Bound,
}

/// A tool call in Kai's conversation, for [`Apps::open_for_call`].
pub struct CallView {
    /// The tool's exposed name (`<server>__<name>`), as lmgw's `tool` frame names it.
    pub name: String,
    pub arguments: Map<String, Value>,
    /// The call's id as lmgw's frames carry it (`hostContext.toolInfo.id`); `None` from an lmgw
    /// before 2026-10-09.
    pub call_id: Option<String>,
    pub progress: watch::Receiver<Progress>,
    /// The `(server_label, ui_resource)` the call's `ready` or `result` frame named; `None` (an
    /// lmgw before 2026-10-09, at `ready`): `/mcp`'s tool list tells (the tool's
    /// `_meta.ui.resourceUri`).
    pub known: Option<(String, String)>,
}

/// A tool's UI resource as `/mcp` lists it: `_meta.ui.resourceUri`, or the deprecated flat
/// `_meta["ui/resourceUri"]` (still read, as lmgw does).
pub fn resource_uri(tool: &Value) -> Option<&str> {
    tool["_meta"]["ui"]["resourceUri"]
        .as_str()
        .or_else(|| tool["_meta"]["ui/resourceUri"].as_str())
}

/// The tool named `exposed` in `/mcp`'s list, in its server's own terms (its own name, its UI
/// resource's URI as the server wrote it): `hostContext.toolInfo.tool`.
fn own_tool(rules: &Rules, list: &Value, exposed: &str) -> Option<Value> {
    let mut tool = list["tools"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == exposed)?
        .clone();
    // The server's own name for it: the stamp's when lmgw stamped the list (a moved tool is no
    // `<server>__…`), else the exposed name without the label.
    let own = match rules.tools.iter().find(|t| t.exposed == exposed) {
        Some(t) => t.own.clone(),
        None => exposed
            .strip_prefix(&format!("{}__", rules.server))?
            .to_owned(),
    };
    tool["name"] = Value::String(own);
    for pointer in ["/_meta/ui/resourceUri", "/_meta/ui~1resourceUri"] {
        if let Some(uri) = tool.pointer_mut(pointer)
            && let Some(o) = uri.as_str().and_then(|u| rules.own(u))
        {
            *uri = Value::String(o);
        }
    }
    Some(tool)
}

impl Apps {
    async fn session(&self) -> Result<Arc<McpSession>, String> {
        let Some(creds) = self.creds.borrow().clone() else {
            return Err("overlay_app: Kai is not paired with lmgw".into());
        };
        let session =
            McpSession::open(self.client.clone(), creds, self.connect, self.bound.clone())
                .await
                .map_err(|e| format!("overlay_app: {}", e.text()))?;
        // Dropped from here on (this open cancelled: the show's deadline, the call's), the session
        // ends itself (`McpSession`'s drop sends the DELETE).
        Ok(Arc::new(session))
    }

    /// `overlay_app {server, uri}`: the document read and the bridge started; `size` is the
    /// configured width and height. No tool input or result (§10.1, K22).
    pub async fn open(&self, server: &str, uri: &str, size: (u32, u32)) -> Result<AppBody, String> {
        let t = target(server, uri)?;
        let session = self.session().await?;
        let upstream = Shared(session.clone());
        // The read first: it connects the server if it was not (api §7.5), so the list holds its
        // tools.
        let opened = async {
            let doc = read(&upstream, &t).await?;
            Ok::<_, String>((doc, list(&upstream).await?))
        };
        match opened.await {
            Ok((doc, list)) => {
                if let Some(c) = self.creds.borrow().clone() {
                    self.lists.put(&c, &list);
                }
                let rules = Rules::of_list(&t.server, &list);
                Ok(self.start(upstream, &t, doc, rules, size, None))
            }
            Err(e) => {
                session.close().await;
                Err(e)
            }
        }
    }

    /// The view of a tool call in Kai's conversation (`super::thread_apps`): `Ok(None)` when the
    /// tool names no UI resource. Its server is the tool's label (`<server>__…`), so a view of a
    /// server without a tool prefix is not shown, as for `overlay_app`.
    pub async fn open_for_call(
        &self,
        call: CallView,
        size: (u32, u32),
    ) -> Result<Option<AppBody>, String> {
        // The list as last fetched says the tool has no view: no session to learn it again.
        let creds = self.creds.borrow().clone();
        if call.known.is_none()
            && let Some(c) = &creds
            && self.lists.seen(c, &call.name) == lists::Seen::NoView
        {
            return Ok(None);
        }
        let session = self.session().await?;
        let upstream = Shared(session.clone());
        let opened = async {
            match &call.known {
                Some((server, uri)) => {
                    let t = target(server, uri)?;
                    let doc = read(&upstream, &t).await?;
                    Ok::<_, String>((list(&upstream).await?, Some((t, doc))))
                }
                None => {
                    // The call runs now, so its server is connected: the list first, to learn
                    // whether the tool has a view at all.
                    let list = list(&upstream).await?;
                    let tool = list["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|t| t["name"] == call.name.as_str());
                    // The server is the stamp's label (a moved tool's name starts with the
                    // server's name), else the name's prefix.
                    let label = tool
                        .and_then(ToolServer::of_tool)
                        .map(|s| s.label)
                        .or_else(|| call.name.split_once("__").map(|(l, _)| l.to_owned()));
                    let (Some(uri), Some(server)) = (tool.and_then(resource_uri), label) else {
                        return Ok((list, None));
                    };
                    let t = target(&server, uri)?;
                    let doc = read(&upstream, &t).await?;
                    Ok((list, Some((t, doc))))
                }
            }
        };
        let (list, (t, doc)) = match opened.await {
            Ok((list, found)) => {
                if let Some(c) = &creds {
                    self.lists.put(c, &list);
                }
                match found {
                    Some(v) => (list, v),
                    None => {
                        session.close().await;
                        return Ok(None);
                    }
                }
            }
            Err(e) => {
                session.close().await;
                return Err(e);
            }
        };
        let mut rules = Rules::of_list(&t.server, &list);
        let call_name = call.name;
        // Servers that share a prefix share the label: the call says whose view this is.
        rules.narrow_to_call(&call_name);
        let run = ToolRun {
            tool: own_tool(&rules, &list, &call_name),
            arguments: call.arguments,
            call_id: call.call_id,
            progress: call.progress,
        };
        Ok(Some(self.start(upstream, &t, doc, rules, size, Some(run))))
    }

    /// The bridge started on an open session.
    fn start(
        &self,
        upstream: Shared,
        t: &Target,
        doc: Resource,
        rules: Rules,
        size: (u32, u32),
        run: Option<ToolRun>,
    ) -> AppBody {
        let mut notes = doc.notes;
        for note in &notes {
            tracing::warn!(server = t.server, "app view: {note}");
        }
        // `policy.tools.<server>__…` names the server's list does not hold: said now, the first
        // time they could be checked (the host's start cannot, `crate::host::policy`).
        let unmatched: Vec<String> = {
            let policy = self.policy.read().unwrap_or_else(|e| e.into_inner());
            policy
                .app_tool_names()
                .into_iter()
                .filter(|n| {
                    n.strip_prefix(&t.server)
                        .is_some_and(|rest| rest.starts_with("__"))
                        && !rules
                            .tools
                            .iter()
                            .any(|tool| tool.exposed == *n || tool.alias.as_deref() == Some(*n))
                })
                .map(str::to_owned)
                .collect()
        };
        // A moved or renamed tool's old spelling that holds a setting: said, since only the
        // tightening states carry over to the name lmgw lists it under ([`effective_state`]).
        {
            let policy = self.policy.read().unwrap_or_else(|e| e.into_inner());
            for tool in &rules.tools {
                if let Some(a) = &tool.alias
                    && let Some(state) = policy.state(a)
                {
                    let note = if matches!(
                        state,
                        ToolState::Off | ToolState::Deny | ToolState::Confirm
                    ) {
                        format!(
                            "policy.tools.{a}: {} is listed as {} now; the setting still applies \
                             to it (the stricter of the two names wins)",
                            tool.own, tool.exposed
                        )
                    } else {
                        format!(
                            "policy.tools.{a}: {} is listed as {} now; an allow written for the \
                             old name does not carry over, write policy.tools.{}",
                            tool.own, tool.exposed, tool.exposed
                        )
                    };
                    tracing::warn!(server = t.server, "app view: {note}");
                    notes.push(note);
                }
            }
        }
        for name in unmatched {
            let note = format!(
                "policy.tools.{name}: {} offers Kai no such tool (its /mcp list), so the setting \
                 does nothing",
                t.server
            );
            tracing::warn!(server = t.server, "app view: {note}");
            notes.push(note);
        }
        tracing::info!(
            server = t.server,
            tools = rules.tools.iter().filter(|t| t.app).count(),
            call = run.is_some(),
            "app view: its document read, its bridge starts"
        );
        let mut bridge = Bridge::new(
            Arc::new(upstream),
            rules,
            self.confirm.clone(),
            self.bound.clone(),
            self.calls.clone(),
            size,
        )
        .with_policy(self.policy.clone())
        .with_screen(self.screen.clone());
        if let Some(run) = run {
            bridge = bridge.with_run(run);
        }
        AppBody {
            server: t.server.clone(),
            html: doc.html,
            ends: bridge.spawn(),
            notes,
            bound: self.bound.clone(),
        }
    }
}

/// The view's document, read through `/mcp`.
async fn read(upstream: &Shared, t: &Target) -> Result<Resource, String> {
    let read = upstream
        .request("resources/read", json!({"uri": t.uri}))
        .await
        .map_err(|e| format!("overlay_app: lmgw's read of {}: {}", t.uri, e.text()))?;
    resource(&read, &t.uri)
}

/// `/mcp`'s tool list for Kai's key.
async fn list(upstream: &Shared) -> Result<Value, String> {
    upstream
        .request("tools/list", json!({}))
        .await
        .map_err(|e| format!("overlay_app: lmgw's tool list: {}", e.text()))
}

pub mod lists;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod stamped_tests;
