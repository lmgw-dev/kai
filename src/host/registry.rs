//! The tools Kai offers: each a contract spec ([`kai_desktop::contract`]) with the handler that runs
//! its calls, supplied by providers.
//!
//! **How a tool joins** (§15.1.5, one line each):
//! - its spec goes into a contract file, and the file's area into `contract::areas!`;
//! - a tool that acts on the core (PD3: conversation mode, jobs, the configuration, the
//!   personality) is served by the core's provider: one line in `src/core/calls.rs`'s
//!   `core_tools!` naming the area and the core's method;
//! - every other module (the widgets, the desktop providers, the delegates, the routines) is a
//!   [`Provider`], one line in [`providers`], built from [`Deps`] (the core's commands, the GTK
//!   thread, the session bus, the configuration, the current credentials, the change signal: what
//!   WP10, WP14, WP17 and WP20 need is there already, so none of them adds a field here or a line
//!   in `main.rs`). A provider lists the entries it can serve now (WP14's probe leaves out a tool
//!   the desktop cannot support) and calls [`Changes::signal`] when that set changes (WP20's
//!   routines, a probe's result), which lists the tools again and sends `list_changed`.
//!
//! The list is built from the providers at the host's start and again only on a signal or a policy
//! change, in a fixed order (the providers', then each one's), so it is the same on every
//! reconnect (K22: a changed list costs a full prefill).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use kai_desktop::contract::{Kind, ToolSpec};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::confirm::{quoted, shown_text};
use super::policy::Caller;
use crate::config::Config;
use crate::core::Command;
use crate::lmgw_link::Credentials;
use crate::ui::UiCmd;
use crate::voice::link::Pending;

// The API the tools of later work packages use (WP9 onward): not all of it has a caller in this
// build yet, so `dead_code` is allowed where it says so.

/// One call on its way to its handler: policy and confirmation are done.
#[allow(dead_code)]
pub struct Call {
    pub spec: &'static ToolSpec,
    pub args: Args,
    pub caller: Caller,
    /// Cancelled when lmgw cancels the call (api §5.4) or the link ends: a handler that spawns
    /// work stops it; the handler's future itself is dropped then.
    pub cancel: CancellationToken,
    /// When lmgw stops waiting (`lmgw/timeout_ms` from the call's arrival, the confirmation
    /// included) and cancels the call; `None`: no deadline.
    pub deadline: Option<tokio::time::Instant>,
    /// `lmgw/timeout_ms` as stamped, for a result that names it.
    pub timeout_ms: Option<u64>,
    /// The bytes the result's text may take as JSON (escaped, one text block) within the link's
    /// message limit; `None`: no bound. A larger result is the call's error (`server.rs`).
    pub room: Option<u64>,
    /// The message limit as a result names it (`LinkLimits::message_says`).
    pub room_says: String,
    /// What the tool's [`Preparer`] made before the confirmation, which the dialog showed: the
    /// handler runs that, not a second resolution.
    pub prepared: Option<Prepared>,
    /// Where a script routine streams its stdout as it comes (the CLI's client, `crate::cli`) in
    /// place of keeping it for the result; `None`: kept, within `room`. Bounded: a reader that is
    /// slow holds the script's pipe, not Kai's memory.
    pub out: Option<Out>,
}

/// A stream of stdout pieces ([`Call::out`]).
pub type Out = mpsc::Sender<Vec<u8>>;

impl Call {
    /// A call with no deadline, no size bound and nothing prepared (tests, Kai's own calls).
    #[allow(dead_code)]
    pub fn new(
        spec: &'static ToolSpec,
        args: Args,
        caller: Caller,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            spec,
            args,
            caller,
            cancel,
            deadline: None,
            timeout_ms: None,
            room: None,
            room_says: String::new(),
            prepared: None,
            out: None,
        }
    }
}

/// What a tool resolves before its confirmation (`open`'s application): the lines the dialog shows
/// besides the arguments, and the resolution itself for the handler.
#[derive(Clone)]
pub struct Prepared {
    pub lines: Vec<String>,
    pub state: Arc<dyn std::any::Any + Send + Sync>,
    /// The checkboxes for the dialog (`super::confirm::DialogOption`), read by the handler.
    pub options: Vec<super::confirm::DialogOption>,
    /// Lines the dialog shows under the description, about what allowing the call means beyond
    /// its arguments (`type_text`'s remote-control permission, WP16c).
    pub notes: Vec<String>,
}

/// How a call reached its tool, besides its arguments, for a [`Preparer`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origin {
    /// The call carries lmgw's approval (`_meta` `lmgw/approval`, decision approved): the owner
    /// said yes in a thread before the call arrived, so what an implicit argument (the active
    /// window) would resolve to now is not what the owner saw.
    pub approved_in_lmgw: bool,
    /// Who calls (`None`: not said, the tests): `job_cancel` refuses another caller's job before
    /// anyone is asked.
    pub caller: Option<Caller>,
}

/// Resolves a call before its confirmation; an error refuses the call without asking anyone.
pub type Preparer = Arc<dyn Fn(&Args, Origin) -> Pending<Result<Prepared, String>> + Send + Sync>;

/// A call's arguments, checked against the tool's parameters ([`Args::read`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args(Map<String, Value>);

impl Args {
    /// The arguments as lmgw sent them, checked against `spec`: every required parameter present,
    /// each of its type, no other key; a `null` is an omitted optional parameter. The error names
    /// the parameter and its type, never a value (it is returned to the caller; the log says only
    /// that the arguments did not fit).
    pub fn read(spec: &ToolSpec, raw: Option<Map<String, Value>>) -> Result<Self, String> {
        let tool = spec.name;
        let mut args = Map::new();
        for (key, value) in raw.unwrap_or_default() {
            let Some(p) = spec.param(&key) else {
                let takes: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
                return Err(if takes.is_empty() {
                    format!("{tool} takes no parameters")
                } else {
                    format!(
                        "{tool} has no parameter '{key}' (it takes {})",
                        takes.join(", ")
                    )
                });
            };
            if value.is_null() {
                continue;
            }
            let fits = match p.kind {
                Kind::Text => value.is_string(),
                Kind::Integer => value.is_i64() || value.is_u64(),
                Kind::Number => value.is_number(),
                Kind::Boolean => value.is_boolean(),
                Kind::TextList => value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string)),
            };
            if !fits {
                return Err(format!("{tool}: {} must be {}", p.name, p.kind.noun()));
            }
            args.insert(key, value);
        }
        if let Some(p) = spec
            .params
            .iter()
            .find(|p| p.required && !args.contains_key(p.name))
        {
            return Err(format!("{tool} needs {} ({})", p.name, p.kind.noun()));
        }
        Ok(Self(args))
    }

    /// The argument as sent (a routine's steps hand it on typed).
    pub fn value(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    #[allow(dead_code)]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(Value::as_str)
    }

    #[allow(dead_code)]
    pub fn integer(&self, name: &str) -> Option<i64> {
        self.0.get(name).and_then(Value::as_i64)
    }

    #[allow(dead_code)]
    pub fn number(&self, name: &str) -> Option<f64> {
        self.0.get(name).and_then(Value::as_f64)
    }

    #[allow(dead_code)]
    pub fn boolean(&self, name: &str) -> Option<bool> {
        self.0.get(name).and_then(Value::as_bool)
    }

    #[allow(dead_code)]
    pub fn texts(&self, name: &str) -> Option<Vec<&str>> {
        self.0
            .get(name)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
    }

    /// K22's compact lines for the owner's confirmation (PD4): `name: value`, a text as it is, a
    /// list as JSON (an argv's word boundaries matter), in the parameters' order. A text holding a
    /// line break, a bidi control or an invisible format character is shown JSON-quoted with those
    /// escaped (`confirm::shown_text`), so a caller cannot forge a line of the dialog. Never
    /// shortened, never logged.
    pub fn lines(&self, spec: &ToolSpec) -> Vec<String> {
        spec.params
            .iter()
            .filter_map(|p| {
                let v = self.0.get(p.name)?;
                let shown = match v {
                    Value::String(s) => shown_text(s).into_owned(),
                    other => json_shown(other),
                };
                Some(format!("{}: {shown}", p.name))
            })
            .collect()
    }
}

/// A value as compact JSON, its texts escaped as [`quoted`] does.
pub(crate) fn json_shown(v: &Value) -> String {
    match v {
        Value::String(s) => quoted(s),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(json_shown).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}:{}", quoted(k), json_shown(v)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        other => other.to_string(),
    }
}

/// One block of a tool's result (MCP content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(String),
    /// Base64 data with its mime type (`see_screen`'s capture).
    #[allow(dead_code)]
    Image {
        mime: String,
        base64: String,
    },
}

/// A tool's answer: its blocks (K22: compact lines, never truncated), and whether it is an error the
/// model reads (`isError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub blocks: Vec<Block>,
    pub error: bool,
}

impl Reply {
    #[allow(dead_code)]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            blocks: vec![Block::Text(text.into())],
            error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            blocks: vec![Block::Text(text.into())],
            error: true,
        }
    }
}

/// Runs one call. Its future is dropped when the call is cancelled.
pub type Handler = Arc<dyn Fn(Call) -> Pending<Reply> + Send + Sync>;

/// One offered tool.
#[derive(Clone)]
pub struct Entry {
    /// The contract area it is declared in.
    pub area: &'static str,
    pub spec: &'static ToolSpec,
    pub handler: Handler,
    /// Run after the policy and before the confirmation, when the tool has one.
    pub prepare: Option<Preparer>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("area", &self.area)
            .field("tool", &self.spec.name)
            .finish()
    }
}

/// A module that serves tools (module doc).
pub trait Provider: Send + Sync + 'static {
    /// The entries it can serve now, in its own fixed order.
    ///
    /// **Synchronous**, called at the host's start and at every relist (on a runtime thread, with
    /// the list's lock not held): it must not block, wait or do I/O. A provider whose set depends
    /// on something asynchronous (WP14's D-Bus probe of the desktop's capabilities) runs that in
    /// its own task, caches the result where `entries` reads it, and calls [`Changes::signal`]
    /// once the result is in or changes; until then it lists what it can serve without it.
    fn entries(&self) -> Vec<Entry>;

    /// The provider as `Any`, for one that Kai reaches again through the host
    /// (`super::local::Local::with_provider`: WP20's routines, whose runtime starts later).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// The signal a provider gives when the entries it serves changed.
#[derive(Clone)]
#[allow(dead_code)]
pub struct Changes(Arc<watch::Sender<u64>>);

impl Changes {
    pub fn new() -> (Self, watch::Receiver<u64>) {
        let (tx, rx) = watch::channel(0);
        (Self(Arc::new(tx)), rx)
    }

    #[allow(dead_code)]
    pub fn signal(&self) {
        self.0.send_modify(|n| *n += 1);
    }
}

/// The session bus for a provider (WP14's desktop providers): the connection Kai's main holds, or
/// one made on first use (`kai --tool-list`, which connects nothing it does not need), bounded by
/// `dbus.timeout_s`.
#[derive(Clone)]
pub struct SessionBus {
    cell: Arc<tokio::sync::OnceCell<zbus::Connection>>,
    timeout: Duration,
}

#[allow(dead_code)]
impl SessionBus {
    pub fn connected(bus: zbus::Connection, timeout: Duration) -> Self {
        Self {
            cell: Arc::new(tokio::sync::OnceCell::new_with(Some(bus))),
            timeout,
        }
    }

    pub fn lazy(timeout: Duration) -> Self {
        Self {
            cell: Arc::new(tokio::sync::OnceCell::new()),
            timeout,
        }
    }

    /// The connection, made now if it was not yet (a failure is said and tried again at the next
    /// call).
    pub async fn get(&self) -> Result<zbus::Connection, String> {
        let secs = self.timeout.as_secs();
        self.cell
            .get_or_try_init(|| async {
                match tokio::time::timeout(self.timeout, zbus::Connection::session()).await {
                    Ok(Ok(bus)) => Ok(bus),
                    Ok(Err(e)) => Err(format!("cannot reach the session bus: {e}")),
                    Err(_) => Err(format!(
                        "the session bus did not answer within {secs} s (dbus.timeout_s)"
                    )),
                }
            })
            .await
            .cloned()
    }
}

/// What the providers are built from: everything a providing module of the later work packages
/// needs, so each adds only its line in [`providers`] (§15.1.5).
pub struct Deps {
    /// The core's commands (PD3: `Command::Tool`).
    pub cmds: mpsc::UnboundedSender<Command>,
    /// The GTK thread (WP10's widgets: their `UiCmd`).
    #[allow(dead_code)]
    pub ui: async_channel::Sender<UiCmd>,
    /// The session bus (WP14's desktop providers).
    #[allow(dead_code)]
    pub bus: SessionBus,
    /// The configuration as read at start (WP14's overrides). Settings the core changes while Kai
    /// runs are the core's; a provider that must follow one gets it through `Command::Tool` or its
    /// own change signal.
    #[allow(dead_code)]
    pub config: Arc<Config>,
    /// The current pairing's credentials (WP17): `Some` from each start of the link (a re-pairing
    /// brings new ones), `None` once it stopped (a revocation, quit) or before the first.
    #[allow(dead_code)]
    pub creds: watch::Receiver<Option<Credentials>>,
    /// The host's policy, live: a provider that decides or lists by it reads it at each use.
    #[allow(dead_code)]
    pub policy: super::policy::SharedPolicy,
    /// For a provider whose entries change (WP20's routines, a probe's result).
    #[allow(dead_code)]
    pub changes: Changes,
    /// The screen lock (`crate::screen_lock`): the host refuses every call while locked.
    pub screen: crate::screen_lock::ScreenLock,
    /// K26's job table (WP18), the host's: `job_cancel`'s and `job_announce`'s dialog read it.
    pub jobs: crate::jobs::Jobs,
    /// Kai's accessibility switch (`crate::desktop::fdo::atspi::switch`), shared with the core.
    #[allow(dead_code)]
    pub a11y: crate::desktop::fdo::atspi::switch::A11ySwitch,
}

/// Every provider, in the order of Kai's tool list: one line per providing module.
#[allow(clippy::vec_init_then_push)]
pub fn providers(deps: &Deps) -> Vec<Box<dyn Provider>> {
    let mut all: Vec<Box<dyn Provider>> = Vec::new();
    all.push(Box::new(crate::core::calls::provider(
        deps.cmds.clone(),
        deps.jobs.clone(),
    )));
    let desktop = crate::desktop::provider(deps);
    all.push(Box::new(crate::delegate::host_view(deps, desktop.clone())));
    all.push(Box::new(crate::delegate::provider(deps, desktop)));
    all.push(Box::new(crate::widgets::provider(deps)));
    all.push(Box::new(crate::routines::provider(deps)));
    #[cfg(feature = "live-test")]
    all.push(Box::new(super::live::Live));
    all
}

/// The providers' entries, read again on each [`Registry::entries`].
pub struct Registry {
    providers: Vec<Box<dyn Provider>>,
}

impl Registry {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self { providers }
    }

    /// The provider of type `T`, if one is (`Provider::as_any`).
    pub fn find<T: 'static>(&self) -> Option<&T> {
        self.providers
            .iter()
            .find_map(|p| p.as_any().and_then(|a| a.downcast_ref::<T>()))
    }

    /// Every provider's entries in order. A name a provider before it took already is left out
    /// and logged: the first one serves it.
    pub fn entries(&self) -> Vec<Entry> {
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        for p in &self.providers {
            for e in p.entries() {
                if seen.insert(e.spec.name) {
                    all.push(e);
                } else {
                    tracing::error!(
                        tool = e.spec.name,
                        area = e.area,
                        "host: a second provider offers a tool another one serves already; it is \
                         left out"
                    );
                }
            }
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use kai_desktop::contract::Param;
    use serde_json::json;

    use super::*;

    const TEXT: Param = Param::new("text", Kind::Text);
    const COUNT: Param = Param::new("count", Kind::Integer).optional();
    const ARGV: Param = Param::new("argv", Kind::TextList);
    const SPEC: ToolSpec = ToolSpec {
        name: "say",
        description: "Say a text.",
        params: &[TEXT, COUNT],
        ..ToolSpec::CHANGE
    };
    const RUN: ToolSpec = ToolSpec {
        name: "run",
        description: "Run a program.",
        params: &[ARGV],
        ..ToolSpec::CHANGE
    };

    fn read(spec: &ToolSpec, v: Value) -> Result<Args, String> {
        Args::read(spec, v.as_object().cloned())
    }

    #[test]
    fn arguments_are_checked_against_the_parameters_without_echoing_a_value() {
        let a = read(&SPEC, json!({"text": "hi there", "count": 2})).unwrap();
        assert_eq!(a.text("text"), Some("hi there"));
        assert_eq!(a.integer("count"), Some(2));
        assert_eq!(a.lines(&SPEC), ["text: hi there", "count: 2"]);
        let a = read(&SPEC, json!({"text": "x", "count": null})).unwrap();
        assert_eq!(
            a.integer("count"),
            None,
            "null is an omitted optional parameter"
        );
        for (v, want) in [
            (json!({}), "say needs text (a text)"),
            (json!({"text": 3}), "say: text must be a text"),
            (
                json!({"text": "x", "count": 1.5}),
                "say: count must be an integer",
            ),
            (
                json!({"text": "x", "secret": "s3cr3t"}),
                "say has no parameter 'secret' (it takes text, count)",
            ),
        ] {
            let e = read(&SPEC, v).unwrap_err();
            assert_eq!(e, want);
            assert!(!e.contains("s3cr3t"));
        }
        assert_eq!(
            Args::read(&SPEC, None).unwrap_err(),
            "say needs text (a text)"
        );
        let a = read(&RUN, json!({"argv": ["ls", "-la", "my dir"]})).unwrap();
        assert_eq!(a.texts("argv").unwrap(), ["ls", "-la", "my dir"]);
        assert_eq!(a.lines(&RUN), [r#"argv: ["ls","-la","my dir"]"#]);
        // A caller cannot forge a line or reorder one: line breaks, bidi controls and invisible
        // format characters are shown escaped, the whole text kept.
        let forged = "fine\nCaller: the owner";
        let a = read(&SPEC, json!({"text": forged})).unwrap();
        assert_eq!(a.lines(&SPEC), [r#"text: "fine\nCaller: the owner""#]);
        let a = read(
            &SPEC,
            json!({"text": "rm \u{202E}txt.exe\u{2028}x\u{200B}"}),
        )
        .unwrap();
        assert_eq!(a.lines(&SPEC), [r#"text: "rm \u202etxt.exe\u2028x\u200b""#]);
        let a = read(&RUN, json!({"argv": ["echo", "a\rb", "\u{2066}c\"d"]})).unwrap();
        assert_eq!(a.lines(&RUN), [r#"argv: ["echo","a\rb","\u2066c\"d"]"#]);
        let long = "x".repeat(100_000);
        let a = read(&SPEC, json!({"text": long})).unwrap();
        assert_eq!(
            a.lines(&SPEC)[0].len(),
            "text: ".len() + 100_000,
            "never shortened"
        );
        assert_eq!(
            read(&RUN, json!({"argv": ["ls", 1]})).unwrap_err(),
            "run: argv must be a list of texts"
        );
        let none = ToolSpec {
            params: &[],
            ..SPEC
        };
        assert_eq!(
            read(&none, json!({"x": 1})).unwrap_err(),
            "say takes no parameters"
        );
    }

    struct Fixed(Vec<Entry>);
    impl Provider for Fixed {
        fn entries(&self) -> Vec<Entry> {
            self.0.clone()
        }
    }

    fn entry(spec: &'static ToolSpec, area: &'static str) -> Entry {
        Entry {
            area,
            spec,
            handler: Arc::new(|_| Box::pin(async { Reply::text("ok") })),
            prepare: None,
        }
    }

    #[test]
    fn the_first_provider_of_a_name_serves_it() {
        let r = Registry::new(vec![
            Box::new(Fixed(vec![entry(&SPEC, "a")])),
            Box::new(Fixed(vec![entry(&RUN, "b"), entry(&SPEC, "b")])),
        ]);
        let got: Vec<(&str, &str)> = r.entries().iter().map(|e| (e.spec.name, e.area)).collect();
        assert_eq!(got, [("say", "a"), ("run", "b")]);
    }
}
