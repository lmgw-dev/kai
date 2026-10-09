//! A scripted model, tools with a fixed policy, and an owner with a scripted answer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use kai_delegate::{
    CancellationToken, ContextStep, Def, Gate, Message, Model, Outcome, Owner, Part, Prefix,
    Request, Settings, Step, Task, ToolCall, ToolDef, Tools, Turn, Utterance,
};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;

pub const LOOK: Def = Def {
    name: "look",
    instructions: "Answer a question about what is on the screen.",
    examples: &["question: which song plays?\n-> see_window {window: player}\ndone: Blue Monday"],
    tools: &["window_list", "see_window", "see_screen", "window_close"],
    context: &[
        ContextStep {
            tool: "window_list",
            pass: &[],
            heading: "Open windows",
        },
        ContextStep {
            tool: "see_window",
            pass: &[("window", "window")],
            heading: "The window",
        },
    ],
};

pub fn settings(max_turns: u32) -> Settings {
    Settings {
        max_turns,
        max_turns_key: "delegate.max_turns".into(),
    }
}

pub fn task(question: &str, window: Option<&str>, utterance: Utterance) -> Task {
    let mut params = vec![("question".to_owned(), question.to_owned())];
    if let Some(w) = window {
        params.push(("window".into(), w.into()));
    }
    Task { params, utterance }
}

pub fn heard(s: &str) -> Utterance {
    Utterance::Heard(s.into())
}

/// A call the model makes.
pub fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}

pub fn calls(calls: Vec<ToolCall>) -> Result<Turn, String> {
    Ok(Turn { text: None, calls })
}

pub fn says(text: &str) -> Result<Turn, String> {
    Ok(Turn {
        text: Some(text.into()),
        calls: vec![],
    })
}

/// What the model was asked.
#[derive(Debug, Clone)]
pub struct Asked {
    pub prefix: Prefix,
    pub messages: Vec<Message>,
}

/// Answers each request with the next scripted turn; with the script spent, it waits forever (the
/// cancellation test), saying so through `waiting`.
#[derive(Default)]
pub struct Scripted {
    script: Mutex<Vec<Result<Turn, String>>>,
    pub asked: Mutex<Vec<Asked>>,
    pub waiting: Notify,
}

impl Scripted {
    pub fn new(mut turns: Vec<Result<Turn, String>>) -> Self {
        turns.reverse();
        Self {
            script: Mutex::new(turns),
            ..Self::default()
        }
    }

    pub fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}

impl Model for Scripted {
    async fn complete(&self, r: &Request<'_>, _cancel: &CancellationToken) -> Result<Turn, String> {
        self.asked.lock().unwrap().push(Asked {
            prefix: r.prefix.clone(),
            messages: r.messages.to_vec(),
        });
        let next = self.script.lock().unwrap().pop();
        match next {
            Some(t) => t,
            None => {
                self.waiting.notify_one();
                std::future::pending().await
            }
        }
    }
}

/// One fake tool.
#[derive(Clone)]
pub struct Fake {
    pub gate: Gate,
    pub outcome: Outcome,
    pub destructive: bool,
    /// The call waits for the cancellation, with work it spawned that records seeing it.
    pub hangs: bool,
}

impl Fake {
    pub fn reads(outcome: Outcome) -> Self {
        Self {
            gate: Gate::Allow,
            outcome,
            destructive: false,
            hangs: false,
        }
    }
}

pub fn png(tag: &str) -> Part {
    Part::Image {
        mime: "image/png".into(),
        base64: format!("iVBORw0KGgo{tag}"),
    }
}

/// The client's tools: names to fakes; the calls that ran.
#[derive(Default)]
pub struct Desk {
    pub tools: HashMap<&'static str, Fake>,
    pub ran: Mutex<Vec<(String, Map<String, Value>)>>,
    /// Spawned work of a hanging call that saw the cancellation.
    pub stopped: Arc<AtomicBool>,
    pub hanging: Arc<Notify>,
}

impl Desk {
    /// `window_list` and `see_window` read, `see_screen` reads an image, `window_close` needs a
    /// confirmation.
    pub fn standard() -> Self {
        let mut tools = HashMap::new();
        tools.insert(
            "window_list",
            Fake::reads(Outcome::text("w1 firefox \"News\"\nw2 konsole \"~\"")),
        );
        tools.insert(
            "see_window",
            Fake::reads(Outcome {
                parts: vec![Part::Text("window w1, 800×600".into()), png("WINDOW")],
                error: false,
            }),
        );
        tools.insert(
            "see_screen",
            Fake::reads(Outcome {
                parts: vec![Part::Text("output DP-1, 1920×1080".into()), png("SCREEN")],
                error: false,
            }),
        );
        tools.insert(
            "window_close",
            Fake {
                gate: Gate::Confirm,
                outcome: Outcome::text("closed"),
                destructive: true,
                hangs: false,
            },
        );
        Self {
            tools,
            ..Self::default()
        }
    }

    pub fn ran(&self) -> Vec<String> {
        self.ran
            .lock()
            .unwrap()
            .iter()
            .map(|(t, a)| format!("{t} {}", Value::Object(a.clone())))
            .collect()
    }
}

impl Tools for Desk {
    fn defs(&self, names: &[&str]) -> Vec<ToolDef> {
        names
            .iter()
            .filter(|n| self.tools.contains_key(*n))
            .map(|n| ToolDef {
                name: (*n).into(),
                description: format!("The tool {n}."),
                parameters: json!({"type": "object", "properties": {"window": {"type": "string"}}}),
            })
            .collect()
    }

    fn check(&self, tool: &str, arguments: &Value) -> Result<Step, String> {
        let fake = self
            .tools
            .get(tool)
            .ok_or_else(|| format!("no tool {tool} for this delegate"))?;
        let args = match arguments {
            Value::Null => Map::new(),
            Value::Object(m) => m
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            _ => return Err(format!("{tool}: the arguments are no object")),
        };
        // A test names its call's target in `label`.
        let target = args
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(Step {
            tool: tool.into(),
            shown: format!("{tool} {}", Value::Object(args.clone())),
            args,
            gate: fake.gate.clone(),
            destructive: fake.destructive,
            target,
        })
    }

    async fn call(&self, step: &Step, cancel: &CancellationToken) -> Outcome {
        self.ran
            .lock()
            .unwrap()
            .push((step.tool.clone(), step.args.clone()));
        let fake = self.tools[step.tool.as_str()].clone();
        if fake.hangs {
            let (token, stopped) = (cancel.clone(), self.stopped.clone());
            tokio::spawn(async move {
                token.cancelled().await;
                stopped.store(true, Ordering::SeqCst);
            });
            self.hanging.notify_one();
            std::future::pending::<()>().await;
        }
        fake.outcome
    }
}

/// The owner: answers each plan with `yes`, or never (`hangs`).
#[derive(Default)]
pub struct Owner1 {
    pub yes: bool,
    /// The dialog cannot be shown: the owner is not asked.
    pub unavailable: bool,
    pub hangs: bool,
    pub asked: AtomicUsize,
    pub shown: Mutex<Vec<Vec<String>>>,
    pub asking: Notify,
}

impl Owner1 {
    pub fn answers(yes: bool) -> Self {
        Self {
            yes,
            ..Self::default()
        }
    }
}

impl Owner for Owner1 {
    async fn confirm_plan(&self, delegate: &str, steps: &[Step]) -> Option<bool> {
        assert_eq!(delegate, "look");
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.shown
            .lock()
            .unwrap()
            .push(steps.iter().map(|s| s.shown.clone()).collect());
        if self.hangs {
            self.asking.notify_one();
            std::future::pending::<()>().await;
        }
        (!self.unavailable).then_some(self.yes)
    }
}

/// The texts of a message's parts.
pub fn texts(m: &Message) -> String {
    match m {
        Message::User(parts) => parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.clone(),
                Part::Image { base64, .. } => format!("[image {base64}]"),
            })
            .collect::<Vec<_>>()
            .join("|"),
        Message::Assistant { text, calls } => format!("{text:?} {calls:?}"),
        Message::Tool { call_id, text } => format!("{call_id}: {text}"),
    }
}

pub fn images(m: &Message) -> usize {
    match m {
        Message::User(parts) => parts
            .iter()
            .filter(|p| matches!(p, Part::Image { .. }))
            .count(),
        _ => 0,
    }
}
