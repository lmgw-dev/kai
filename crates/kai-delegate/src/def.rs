//! What a delegate is ([`Def`]), what one call of it brings ([`Task`]), and the loop's settings.

/// One delegate: static, declared by the client next to its tool's contract entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Def {
    /// The tool's name (`look`): the delegate's name in its prompt, its logs and the owner's
    /// confirmation.
    pub name: &'static str,
    /// What it is for and how to work, the system message's first part.
    pub instructions: &'static str,
    /// Short worked examples, after the instructions (static, so the prompt cache holds them).
    pub examples: &'static [&'static str],
    /// The client's tools it may use, in the order its tool list shows them; a tool the client
    /// does not offer now is left out ([`crate::Tools::defs`]).
    pub tools: &'static [&'static str],
    /// What the client reads for it before the model runs, into the dynamic part (the window list,
    /// the screenshot). Each step is a call of one of the client's tools under the same policy as
    /// the delegate's own calls; a step the policy does not allow is not run, and the task message
    /// says why in its place.
    pub context: &'static [ContextStep],
}

/// One read for the dynamic part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextStep {
    pub tool: &'static str,
    /// The delegate's parameters handed on, `(delegate's, tool's)`: `look`'s `window` is
    /// `see_window`'s. A parameter the call omitted is omitted here too.
    pub pass: &'static [(&'static str, &'static str)],
    /// Its heading in the task message ("Open windows").
    pub heading: &'static str,
}

/// One call of a delegate: its parameters, and what the user last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// The parameters as the caller gave them, `(name, value)`, in the tool's order; an omitted
    /// optional one is not here.
    pub params: Vec<(String, String)>,
    pub utterance: Utterance,
}

/// The user's last utterance, handed verbatim (K25), or why the client has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Utterance {
    Heard(String),
    /// Why there is none ("the call came from the dashboard"): said in its place.
    Unknown(String),
}

/// The loop's settings, the client's (visible: a stop names them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The most model turns one task takes; 0: no bound (the caller's cancellation and the model's
    /// context still end it, each saying so).
    pub max_turns: u32,
    /// The setting's name, for the answer that stops at it (`delegate.max_turns`).
    pub max_turns_key: String,
}
