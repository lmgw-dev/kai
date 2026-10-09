//! The tool contract (K22, K23): every tool Kai's hosted MCP server offers is declared here, once,
//! as a [`ToolSpec`]: its name, its one-line description, at most two plain parameters, its MCP
//! annotations, K15's default and its caller level, and K26's class. Kai's host renders the specs
//! as MCP tools and derives K15's policy from them (`src/host/policy.rs`); nothing in Kai keeps a
//! second list.
//!
//! **Adding tools.** One contract file per area (`conversation.rs`, `overlay.rs`, …) holds a
//! `pub const AREA: Area`; [`areas!`] below gets one line naming it. Where its calls run is Kai's
//! side: the core's areas are one line in `src/core/calls.rs`, every other module's tools come from
//! a provider, one line in `src/host/registry.rs`.
//!
//! **The rules** ([`check`], PD6, K22, K23), checked by a test over every area:
//! - a name of lowercase letters, digits and single underscores that names no desktop (a tool list
//!   must not depend on where Kai runs, K23); lmgw lists it as `<label>__<name>`;
//! - one purpose, a one-line description, a parameter described only where its name is not enough;
//! - at most two parameters, each a plain value (text, integer, number, boolean) or a list of
//!   texts: no nested object, no parameter that applies only in some mode;
//! - the MCP annotations always stated (`readOnlyHint`, `destructiveHint`, `idempotentHint`,
//!   `openWorldHint`): K15's policy reads them, not a hand list.

use serde_json::{Map, Value, json};

/// A parameter's JSON type: a plain value, or a list of texts (`run_command`'s argv). Nothing
/// nested (K22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Integer,
    Number,
    Boolean,
    TextList,
}

impl Kind {
    fn schema(self) -> Value {
        match self {
            Kind::Text => json!({"type": "string"}),
            Kind::Integer => json!({"type": "integer"}),
            Kind::Number => json!({"type": "number"}),
            Kind::Boolean => json!({"type": "boolean"}),
            Kind::TextList => json!({"type": "array", "items": {"type": "string"}}),
        }
    }

    /// The type's name in an error about a call's argument.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Text => "a text",
            Kind::Integer => "an integer",
            Kind::Number => "a number",
            Kind::Boolean => "true or false",
            Kind::TextList => "a list of texts",
        }
    }
}

/// One parameter of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    /// An omitted optional parameter has the default the description names (K22: `window`
    /// defaults to the active window).
    pub required: bool,
    /// Only where the name is not enough (K22).
    pub description: Option<&'static str>,
}

impl Param {
    /// A required parameter of `kind`, with no description.
    pub const fn new(name: &'static str, kind: Kind) -> Self {
        Self {
            name,
            kind,
            required: true,
            description: None,
        }
    }

    pub const fn optional(self) -> Self {
        Self {
            required: false,
            ..self
        }
    }

    pub const fn described(self, description: &'static str) -> Self {
        Self {
            description: Some(description),
            ..self
        }
    }
}

/// The word a `window` parameter takes for the active window (K22; decided by the owner
/// 2026-10-09). It is the default too; saying it lets a call name the active window explicitly.
pub const ACTIVE: &str = "active";

/// The `window` parameter of every tool that addresses a window (K22): optional; `active` (the
/// default), a window id, an app name or a fragment of the title, which Kai resolves (an ambiguous
/// one is an error listing the candidates). Never a limit on what else the tool may read.
pub const WINDOW: Param = Param::new("window", Kind::Text)
    .optional()
    .described("active (the default), a window id, an app name or a title fragment");

/// The MCP tool annotations, always stated (K15, K23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// `readOnlyHint`: the tool changes nothing.
    pub read_only: bool,
    /// `destructiveHint`: what it changes cannot be undone simply (`suspend`). Only for a tool that
    /// is not read-only.
    pub destructive: bool,
    /// `idempotentHint`: calling it twice with the same arguments does what one call does.
    pub idempotent: bool,
    /// `openWorldHint`: it reaches outside Kai and the desktop (the network).
    pub open_world: bool,
}

/// K15's default for a tool, before the owner's per-tool state (ruling 28: every tool can be set to
/// `allow`). "Trusted callers" are the owner, the gateway, Kai's own key and
/// `policy.trusted_callers`; a trusted approval is an F approval whose approver is the owner, the
/// gateway, Kai's own key or in `policy.trusted_approvers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// From the hints: a read-only tool is allowed for trusted callers and confirmed for the
    /// others; a state-changing one is confirmed for every caller. A trusted approval skips the
    /// confirmation either way.
    Hints,
    /// Allowed for trusted callers and confirmed for the others, though the tool changes state:
    /// PD4's overlay tools (§10.1: "untrusted callers are confirmed"). A trusted approval skips the
    /// confirmation.
    Trusted,
    /// K24's confirmed by default (`run_command`, `ui_press`, `type_text`, `key_press`): confirmed
    /// for every caller, the owner included, unless a trusted approval in the thread decided the
    /// call: that approval is the confirmation (K15 as built), so there is no second dialog.
    Always,
}

/// Who may call the tool at all (K15's caller level, K27, K30). A caller below the level is denied
/// outright, not confirmed. It is a default the owner can widen (ruling 28, `policy.levels`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Kai's own key only (K27: conversation mode switches on the room microphone).
    Own,
    /// The owner, the gateway on the owner's behalf, and Kai's own key (K30's configuration tools).
    Owner,
    /// Those and `policy.trusted_callers`.
    Trusted,
    /// Every caller lmgw lets reach Kai's label (api §5.6).
    Any,
}

impl Level {
    /// The level's word in the configuration (`policy.levels`).
    pub fn key(self) -> &'static str {
        match self {
            Level::Own => "own",
            Level::Owner => "owner",
            Level::Trusted => "trusted",
            Level::Any => "any",
        }
    }
}

/// K26's class, decided when the tool is designed: a synchronous call answers when it is done; an
/// asynchronous one answers "started, <id>" at once (WP18, with lmgw's MCP Tasks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Sync,
    Async,
}

/// One hosted tool (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
    pub hints: Hints,
    pub ask: Ask,
    pub level: Level,
    pub class: Class,
    /// The reply carries images (`see_window`, `see_screen`): what `delegate.screenshots =
    /// "delegate"` takes out of the callers' list.
    pub returns_images: bool,
}

impl ToolSpec {
    /// A read-only, synchronous tool for any caller, with no parameters: a base to write a spec
    /// from (`ToolSpec { name, description, ..ToolSpec::READ }`).
    pub const READ: ToolSpec = ToolSpec {
        name: "",
        description: "",
        params: &[],
        hints: Hints {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        ask: Ask::Hints,
        level: Level::Any,
        class: Class::Sync,
        returns_images: false,
    };

    /// A state-changing, synchronous tool for any caller, with no parameters (see [`Self::READ`]).
    pub const CHANGE: ToolSpec = ToolSpec {
        hints: Hints {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        ..ToolSpec::READ
    };

    /// The JSON schema of the tool's arguments, as MCP's `inputSchema` carries it: an object of the
    /// parameters, the required ones listed, nothing else accepted.
    pub fn input_schema(&self) -> Map<String, Value> {
        let mut properties = Map::new();
        for p in self.params {
            let mut schema = p.kind.schema();
            if let (Some(d), Some(obj)) = (p.description, schema.as_object_mut()) {
                obj.insert("description".into(), Value::String(d.into()));
            }
            properties.insert(p.name.into(), schema);
        }
        let required: Vec<Value> = self
            .params
            .iter()
            .filter(|p| p.required)
            .map(|p| Value::String(p.name.into()))
            .collect();
        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(properties));
        if !required.is_empty() {
            schema.insert("required".into(), Value::Array(required));
        }
        schema.insert("additionalProperties".into(), Value::Bool(false));
        schema
    }

    pub fn param(&self, name: &str) -> Option<&'static Param> {
        self.params.iter().find(|p| p.name == name)
    }
}

/// One contract file's tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    /// The file's name (`conversation`): Kai routes the core's areas by it (`src/core/calls.rs`).
    pub name: &'static str,
    pub tools: &'static [ToolSpec],
}

/// One line per contract file: `name,` declares `mod name;` and lists its `AREA`, in this order.
macro_rules! areas {
    ($($(#[$attr:meta])* $area:ident,)*) => {
        $($(#[$attr])* pub mod $area;)*

        /// Every area, in the order [`areas!`] lists them (the order of Kai's tool list).
        #[allow(clippy::vec_init_then_push)]
        pub fn areas() -> Vec<&'static Area> {
            #[allow(unused_mut)]
            let mut all: Vec<&'static Area> = Vec::new();
            $($(#[$attr])* all.push(&$area::AREA);)*
            all
        }
    };
}

areas! {
    conversation,
    overlay,
    #[cfg(feature = "live-test")]
    live,
    windows,
    session,
    read,
    launch,
    config,
    delegate,
    input,
    personality,
    routines,
    jobs,
}

/// Every tool of every area, in the areas' order.
pub fn tools() -> impl Iterator<Item = &'static ToolSpec> {
    areas().into_iter().flat_map(|a| a.tools.iter())
}

/// The area and spec of the tool `name`.
pub fn find(name: &str) -> Option<(&'static Area, &'static ToolSpec)> {
    areas()
        .into_iter()
        .find_map(|a| a.tools.iter().find(|t| t.name == name).map(|t| (a, t)))
}

/// Desktop, compositor and display-server names a tool name must not carry (K23): a tool's name
/// is the same on every desktop. Compared against the name's `_`-separated words.
pub const DESKTOP_NAMES: &[&str] = &[
    "kde", "plasma", "kwin", "krunner", "klipper", "baloo", "gnome", "mutter", "wlroots", "sway",
    "hyprland", "xfce", "lxqt", "cinnamon", "mate", "budgie", "cosmic", "x11", "xorg", "wayland",
];

/// The most parameters a tool takes (K22).
pub const MAX_PARAMS: usize = 2;

fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__")
        && !name.ends_with('_')
}

fn one_line(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains(['\n', '\r']) && text.trim() == text
}

/// What breaks PD6's rules in `spec` (module doc): empty when it keeps them. Each problem names the
/// tool and the rule.
pub fn check(spec: &ToolSpec) -> Vec<String> {
    let mut problems = Vec::new();
    let tool = spec.name;
    if !plain_name(tool) {
        problems.push(format!(
            "{tool:?}: a name is lowercase letters, digits and single underscores, starting with a \
             letter"
        ));
    }
    for word in tool.split('_') {
        if DESKTOP_NAMES.contains(&word) {
            problems.push(format!("{tool}: the name names a desktop ({word}, K23)"));
        }
    }
    if !one_line(spec.description) {
        problems.push(format!("{tool}: the description is not one line"));
    }
    if spec.params.len() > MAX_PARAMS {
        problems.push(format!(
            "{tool}: {} parameters, at most {MAX_PARAMS} (K22)",
            spec.params.len()
        ));
    }
    for (i, p) in spec.params.iter().enumerate() {
        if !plain_name(p.name) {
            problems.push(format!(
                "{tool}: the parameter {:?} is not a plain name",
                p.name
            ));
        }
        if spec.params[..i].iter().any(|q| q.name == p.name) {
            problems.push(format!("{tool}: the parameter {} twice", p.name));
        }
        if p.description.is_some_and(|d| !one_line(d)) {
            problems.push(format!(
                "{tool}: the parameter {}'s description is not one line",
                p.name
            ));
        }
    }
    // The rendered schema, as lmgw sees it: no parameter is an object (K22), whatever `Kind` gains.
    if let Some(Value::Object(props)) = spec.input_schema().get("properties") {
        for (name, schema) in props {
            let nested = |s: &Value| s.get("type").and_then(Value::as_str) == Some("object");
            if nested(schema) || schema.get("items").is_some_and(nested) {
                problems.push(format!(
                    "{tool}: the parameter {name} is a nested object (K22)"
                ));
            }
        }
    }
    if spec.hints.read_only && spec.hints.destructive {
        problems.push(format!("{tool}: read-only and destructive at once"));
    }
    // Read-only and confirmed for every caller (`Ask::Always`) is a narrowing, kept: a routine's
    // default when Kai does not honour what its `routine.toml` asks (K28).
    if spec.hints.read_only && spec.ask == Ask::Trusted {
        problems.push(format!(
            "{tool}: a read-only tool takes K15's default from its hints (Ask::Hints) or is \
             confirmed for every caller (Ask::Always)"
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PD6: every area's tools keep the rules, and no two tools share a name.
    #[test]
    fn every_tool_keeps_the_rules() {
        let mut seen = std::collections::HashSet::new();
        for spec in tools() {
            assert_eq!(check(spec), Vec::<String>::new());
            assert!(seen.insert(spec.name), "{} twice", spec.name);
        }
        let mut names = std::collections::HashSet::new();
        for area in areas() {
            assert!(plain_name(area.name), "{}", area.name);
            assert!(names.insert(area.name), "the area {} twice", area.name);
        }
    }

    const TEXT: Param = Param::new("text", Kind::Text);
    const MIME: Param = Param::new("mime", Kind::Text);
    const PRIMARY: Param = Param::new("primary", Kind::Boolean);
    const TWO_LINES: Param = Param::new("text", Kind::Text).described("two\nlines");
    const ARGV: Param = Param::new("argv", Kind::TextList);
    const CWD: Param = Param::new("cwd", Kind::Text)
        .optional()
        .described("default: the home directory");

    /// The checker catches each rule it states, so a passing contract means something.
    #[test]
    fn the_checker_catches_each_broken_rule() {
        let good = ToolSpec {
            name: "clipboard_set",
            description: "Put text on the clipboard.",
            params: &[TEXT],
            ..ToolSpec::CHANGE
        };
        assert!(check(&good).is_empty(), "{:?}", check(&good));
        let broken = |spec: ToolSpec, rule: &str| {
            let problems = check(&spec);
            assert!(
                problems.iter().any(|p| p.contains(rule)),
                "{}: {rule} not caught: {problems:?}",
                spec.name
            );
        };
        broken(
            ToolSpec {
                name: "Clipboard",
                ..good
            },
            "lowercase",
        );
        broken(
            ToolSpec {
                name: "clip__set",
                ..good
            },
            "single underscores",
        );
        broken(
            ToolSpec {
                name: "kwin_tile",
                ..good
            },
            "names a desktop (kwin",
        );
        broken(
            ToolSpec {
                name: "plasma_dnd",
                ..good
            },
            "names a desktop (plasma",
        );
        broken(
            ToolSpec {
                description: "Put text\non the clipboard.",
                ..good
            },
            "not one line",
        );
        broken(
            ToolSpec {
                description: "",
                ..good
            },
            "not one line",
        );
        broken(
            ToolSpec {
                params: &[TEXT, MIME, PRIMARY],
                ..good
            },
            "3 parameters, at most 2",
        );
        broken(
            ToolSpec {
                params: &[TEXT, TEXT],
                ..good
            },
            "the parameter text twice",
        );
        broken(
            ToolSpec {
                params: &[TWO_LINES],
                ..good
            },
            "description is not one line",
        );
        broken(
            ToolSpec {
                hints: Hints {
                    read_only: true,
                    destructive: true,
                    idempotent: true,
                    open_world: false,
                },
                ..good
            },
            "read-only and destructive",
        );
        // "automate" is no desktop, though it holds one's letters.
        assert!(
            check(&ToolSpec {
                name: "automate_mate_free",
                ..good
            })
            .iter()
            .any(|p| p.contains("(mate"))
        );
        assert!(
            check(&ToolSpec {
                name: "automate",
                ..good
            })
            .is_empty()
        );
    }

    #[test]
    fn the_schema_lists_plain_parameters_and_their_required_ones() {
        let spec = ToolSpec {
            name: "run_command",
            description: "Run a program.",
            params: &[ARGV, CWD],
            ask: Ask::Always,
            ..ToolSpec::CHANGE
        };
        assert_eq!(
            Value::Object(spec.input_schema()),
            json!({
                "type": "object",
                "properties": {
                    "argv": {"type": "array", "items": {"type": "string"}},
                    "cwd": {"type": "string", "description": "default: the home directory"}
                },
                "required": ["argv"],
                "additionalProperties": false
            })
        );
        let none = ToolSpec {
            name: "jobs",
            description: "List the running jobs.",
            ..ToolSpec::READ
        };
        assert_eq!(
            Value::Object(none.input_schema()),
            json!({"type": "object", "properties": {}, "additionalProperties": false})
        );
    }
}
