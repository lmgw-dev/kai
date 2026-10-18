//! The delegate's model: lmgw's `/v1/chat/completions` as Kai's device (a device holds `Inference`,
//! api §1.2), over the device link, not streamed. The request is lmgw-client's `Request` (its URL
//! rules); the body is OpenAI's chat completion with the static part first.
//!
//! **The body's order** is the prompt's: `model`, `stream`, `tools`, then `messages` with the
//! system message first, so the serialized static part is a byte prefix of every request of the
//! same delegate and offer, as it is of the rendered prompt.
//!
//! **Images** are OpenAI's `image_url` parts with a data URL, in user messages (the tool message
//! carries text only, which is also all lmgw's OpenAI ingress reads of one); lmgw's egress sends
//! them to the answering alias by its capability (a model without vision gets egress §8.2's
//! placeholder).
//!
//! Nothing of the request, the key or the answer is logged; an error names lmgw's status, code
//! and message.

use std::sync::Mutex;

use kai_delegate::{CancellationToken, Message, Part, Request, ToolCall, Turn};
use lmgw_client::requests::{Header, Method, Request as LmgwRequest};
use lmgw_client::types::capabilities::ModelCapabilities;
use serde::Serialize;
use serde_json::{Value, json};

use crate::lmgw_link::Credentials;
use crate::lmgw_link::describe;

/// `POST /v1/chat/completions`.
pub fn chat_completions(body: String) -> LmgwRequest {
    LmgwRequest {
        method: Method::Post,
        path: "/v1/chat/completions".into(),
        headers: vec![Header {
            name: "Content-Type".into(),
            value: "application/json".into(),
        }],
        body: Some(body),
    }
}

/// `GET /v1/models`.
pub fn models() -> LmgwRequest {
    LmgwRequest {
        method: Method::Get,
        path: "/v1/models".into(),
        headers: Vec::new(),
        body: None,
    }
}

/// Whether lmgw's `/v1/models` says `alias` takes images (`capabilities.vision`): `None` when the
/// listing does not read, lacks the alias or states nothing ("absent means unknown").
pub fn vision_of(body: &str, alias: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(body).ok()?;
    let entry = v
        .get("data")?
        .as_array()?
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(alias))?;
    // The strict type: an entry whose capabilities do not parse states nothing (unknown).
    let caps: ModelCapabilities =
        serde_json::from_value(entry.get("capabilities")?.clone()).ok()?;
    caps.vision
}

/// The body in the prompt's order (module doc).
#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    stream: bool,
    tools: Vec<Value>,
    messages: Vec<Value>,
}

/// The request as OpenAI's chat completion body for `alias`.
pub fn body(alias: &str, request: &Request<'_>) -> String {
    let tools = request
        .prefix
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    let mut messages = vec![json!({"role": "system", "content": request.prefix.system})];
    messages.extend(request.messages.iter().map(message));
    serde_json::to_string(&Body {
        model: alias,
        stream: false,
        tools,
        messages,
    })
    .expect("JSON values serialize")
}

fn message(m: &Message) -> Value {
    match m {
        Message::User(parts) => {
            let images = parts.iter().any(|p| matches!(p, Part::Image { .. }));
            if !images {
                let text: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        Part::Image { .. } => None,
                    })
                    .collect();
                return json!({"role": "user", "content": text.join("\n")});
            }
            let content: Vec<Value> = parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => json!({"type": "text", "text": t}),
                    Part::Image { mime, base64 } => json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:{mime};base64,{base64}")}
                    }),
                })
                .collect();
            json!({"role": "user", "content": content})
        }
        Message::Assistant { text, calls } => {
            let mut v = json!({"role": "assistant", "content": text});
            if !calls.is_empty() {
                v["tool_calls"] = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": c.arguments}
                        })
                    })
                    .collect();
            }
            v
        }
        Message::Tool { call_id, text } => {
            json!({"role": "tool", "tool_call_id": call_id, "content": text})
        }
    }
}

/// lmgw's answer read as the model's turn.
pub fn read_turn(status: u16, body: &str) -> Result<Turn, String> {
    if !(200..300).contains(&status) {
        return Err(refusal(status, body));
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|e| format!("lmgw's answer does not read as a chat completion: {e}"))?;
    let choice = v
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or("lmgw's answer has no choice")?;
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(
            "the model's answer reached its output limit (finish_reason length); the alias's \
             max_tokens bounds it"
                .into(),
        );
    }
    let msg = choice
        .get("message")
        .ok_or("lmgw's answer has no message")?;
    let text = match msg.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        // Parts (a gateway that answers in parts): their texts.
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
    .filter(|t| !t.trim().is_empty());
    let calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let f = c.get("function");
                    ToolCall {
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("call_{i}")),
                        name: f
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments: match f.and_then(|f| f.get("arguments")) {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Null) | None => String::new(),
                            // A gateway that sends the object itself.
                            Some(other) => other.to_string(),
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Turn { text, calls })
}

/// An error answer in words: lmgw's status, code and message (OpenAI's `{"error": {...}}`).
pub fn refusal(status: u16, body: &str) -> String {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let err = v.as_ref().and_then(|v| v.get("error"));
    let code = err
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = err
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match (code.is_empty(), message.is_empty()) {
        (true, true) => format!("lmgw answered {status} ({} bytes)", body.len()),
        (false, true) => format!("lmgw answered {status} {code}"),
        (true, false) => format!("lmgw answered {status}: {message}"),
        (false, false) => format!("lmgw answered {status} {code}: {message}"),
    }
}

/// One alias at the paired lmgw.
pub struct LmgwModel {
    pub creds: Credentials,
    pub alias: String,
    /// The aliases that answered in its place (`x-lmgw-fallback`), in order, once each.
    answered_by: Mutex<Vec<String>>,
}

impl LmgwModel {
    pub fn new(creds: Credentials, alias: String) -> Self {
        Self {
            creds,
            alias,
            answered_by: Mutex::default(),
        }
    }

    /// The aliases that answered in place of the asked one.
    pub fn answered_by(&self) -> Vec<String> {
        self.answered_by.lock().expect("never poisoned").clone()
    }

    /// `(model: <alias>, answered by <fallback>)` when a fallback answered; the asked alias alone
    /// answered otherwise, and nothing is said.
    pub fn note(&self) -> Option<String> {
        let by = self.answered_by();
        (!by.is_empty()).then(|| format!("(model: {}, answered by {})", self.alias, by.join(", ")))
    }
}

impl kai_delegate::Model for LmgwModel {
    async fn complete(
        &self,
        request: &Request<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Turn, String> {
        let req = chat_completions(body(&self.alias, request));
        let sent = send_full(&self.creds.http(), &self.creds.url(), req)
            .await
            .map_err(|e| format!("lmgw did not answer: {e}"))?;
        if let Some(fallback) = sent.fallback {
            tracing::info!(
                alias = %self.alias,
                answered_by = %fallback,
                "delegate: a fallback answered in place of the alias"
            );
            let mut by = self.answered_by.lock().expect("never poisoned");
            if !by.contains(&fallback) {
                by.push(fallback);
            }
        }
        read_turn(sent.status, &sent.body)
    }
}

/// lmgw's answer: the status, the body as text, and the alias that answered when it was not the
/// one asked (`x-lmgw-fallback`).
pub struct Sent {
    pub status: u16,
    pub body: String,
    pub fallback: Option<String>,
}

/// Sends an lmgw-client request; the status and the body as text. Dropping the future aborts it.
pub async fn send(
    client: &reqwest::Client,
    base: &str,
    req: LmgwRequest,
) -> Result<(u16, String), String> {
    let s = send_full(client, base, req).await?;
    Ok((s.status, s.body))
}

/// [`send`], with the headers the delegate reads too.
pub async fn send_full(
    client: &reqwest::Client,
    base: &str,
    req: LmgwRequest,
) -> Result<Sent, String> {
    let method = match req.method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
    };
    let mut b = client.request(method, req.url(base));
    for h in &req.headers {
        b = b.header(h.name.as_str(), h.value.as_str());
    }
    if let Some(body) = req.body {
        b = b.body(body);
    }
    let resp = b.send().await.map_err(|e| crate::lmgw_link::tls::why(&e))?;
    let status = resp.status().as_u16();
    let fallback = resp
        .headers()
        .get("x-lmgw-fallback")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    let body = resp.text().await.map_err(|e| describe(&e))?;
    Ok(Sent {
        status,
        body,
        fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_says_whether_an_alias_sees_images() {
        let list = r#"{"data":[
            {"id":"eyes","capabilities":{"task":"chat","endpoints":[],"source":"config","vision":true}},
            {"id":"blind","capabilities":{"task":"chat","endpoints":[],"source":"config","vision":false}},
            {"id":"unsure","capabilities":{"task":"chat","endpoints":[],"source":"config"}},
            {"id":"odd","capabilities":{"vision":true,"shape":"unknown to this build"}},
            {"id":"bare"}]}"#;
        assert_eq!(vision_of(list, "eyes"), Some(true));
        assert_eq!(vision_of(list, "blind"), Some(false));
        for unknown in ["unsure", "odd", "bare", "missing"] {
            assert_eq!(vision_of(list, unknown), None, "{unknown}");
        }
        assert_eq!(vision_of("<html>", "eyes"), None);
    }

    #[test]
    fn lmgws_answers_read_as_turns_and_errors_name_status_code_and_message() {
        let t = read_turn(
            200,
            r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"see_screen","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap();
        assert_eq!(t.text, None);
        assert_eq!(
            t.calls,
            [ToolCall {
                id: "a".into(),
                name: "see_screen".into(),
                arguments: "{}".into()
            }]
        );
        let t = read_turn(
            200,
            r#"{"choices":[{"message":{"content":"done: 14:02"},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert_eq!(t.text.as_deref(), Some("done: 14:02"));
        assert!(t.calls.is_empty());
        assert!(
            read_turn(
                200,
                r#"{"choices":[{"message":{"content":"done: 14"},"finish_reason":"length"}]}"#
            )
            .unwrap_err()
            .contains("finish_reason length")
        );
        assert_eq!(
            read_turn(
                503,
                r#"{"error":{"code":"gpu_hold","message":"the GPU is held","type":"x"}}"#
            )
            .unwrap_err(),
            "lmgw answered 503 gpu_hold: the GPU is held"
        );
        assert_eq!(
            read_turn(502, "<html>bad gateway</html>").unwrap_err(),
            "lmgw answered 502 (24 bytes)",
            "a body that is no error object is not quoted"
        );
    }
}
