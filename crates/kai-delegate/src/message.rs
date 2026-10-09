//! The delegate's request, in the shape of OpenAI's chat messages (which every client renders
//! as its gateway speaks): a system message and tools as the static part, then user, assistant and
//! tool messages.

use serde_json::Value;

/// One part of a user message or of a tool's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    /// An image as base64 with its mime type (a screenshot). In a request it goes in a user
    /// message (OpenAI's `image_url` part with a data URL); a tool message carries text only.
    Image {
        mime: String,
        base64: String,
    },
}

/// A tool the model may call, as OpenAI's `tools` array lists one.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// The JSON schema of the arguments.
    pub parameters: Value,
}

/// A tool call the model made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model sent them: a JSON object's text (OpenAI's wire), kept as is.
    pub arguments: String,
}

/// One message after the static part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    User(Vec<Part>),
    Assistant {
        text: Option<String>,
        calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        text: String,
    },
}

/// The static part: the system message and the tools. Built once per delegate and offer
/// ([`crate::prefix`]); equal input gives byte-identical output.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefix {
    pub system: String,
    pub tools: Vec<ToolDef>,
}

/// One model request: the static part, then the dynamic messages in order.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub prefix: &'a Prefix,
    pub messages: &'a [Message],
}

/// The model's turn: its text and its tool calls (none: it answered).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Turn {
    pub text: Option<String>,
    pub calls: Vec<ToolCall>,
}
