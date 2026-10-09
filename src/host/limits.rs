//! The link's size limits (api §5.1): lmgw's `mcp.host_max_message_mb` and `mcp.host_max_frame_mb`,
//! as lmgw announces them in `initialize` ([`HostLimits`]), else `[host]`'s fallbacks.
//!
//! - **Frames:** Kai splits an outgoing message into continuation frames no larger than the frame
//!   limit ([`frames`]), so only the message limit binds a result.
//! - **Messages:** a tool's result that would be a larger message than the limit is not sent; the
//!   call answers with an error naming the limit instead (`server.rs`, one place for every tool), so
//!   an oversize result never closes the link (1009) and cancels every call in flight.
//! - **The room** a tool has for its text ([`LinkLimits::text_room`]): the message limit less the
//!   rest of the answer's message, so a tool that cuts its output (`run_command`) cuts it to what
//!   fits, measured as the JSON the frame carries.

use lmgw_client::mcp_host::HostLimits;
use rmcp::model::{CallToolResult, ContentBlock, RequestId, ServerJsonRpcMessage, ServerResult};
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use tokio_tungstenite::tungstenite::{Bytes, Message};

const MIB: u64 = 1024 * 1024;

/// The limits a link runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkLimits {
    /// The largest message; `None`: no bound of its own.
    pub message: Option<u64>,
    /// The largest frame; `None`: bounded by the message.
    pub frame: Option<u64>,
    /// lmgw announced them; else they are `[host]`'s fallbacks, in MiB.
    pub announced: bool,
}

impl LinkLimits {
    /// `[host]`'s fallbacks, used with an lmgw that announces no limits.
    pub fn fallback(max_message_mb: u64, max_frame_mb: u64) -> Self {
        let bytes = |mb: u64| (mb > 0).then(|| mb.saturating_mul(MIB));
        Self {
            message: bytes(max_message_mb),
            frame: bytes(max_frame_mb),
            announced: false,
        }
    }

    /// lmgw's announcement over `before`, the link's limits until it came (the fallbacks). A
    /// message `null` is no bound of its own (`mcp.host_max_message_mb` 0); lmgw always bounds
    /// frames, and should it leave the frame out (or say 0), the fallback's frame limit stays.
    pub fn announced(h: &HostLimits, before: &LinkLimits) -> Self {
        let bound = |b: Option<u64>| b.filter(|&b| b > 0);
        Self {
            message: bound(h.max_message_bytes),
            frame: bound(h.max_frame_bytes).or(before.frame),
            announced: true,
        }
    }

    /// The largest frame Kai writes.
    pub fn frame_bytes(&self) -> Option<u64> {
        match (self.frame, self.message) {
            (Some(f), Some(m)) => Some(f.min(m)),
            (f, m) => f.or(m),
        }
    }

    /// The message limit as a result names it.
    pub fn message_says(&self) -> String {
        match (self.announced, self.message) {
            (true, Some(m)) => {
                format!("{m} bytes, lmgw's mcp.host_max_message_mb as it announced it")
            }
            (true, None) => "none: lmgw's mcp.host_max_message_mb is 0".into(),
            (false, Some(m)) => format!(
                "host.fallback_max_message_mb = {} (lmgw announced no limits)",
                m / MIB
            ),
            (false, None) => {
                "none: host.fallback_max_message_mb = 0 (lmgw announced no limits)".into()
            }
        }
    }

    /// The bytes a tool's single text block may take as JSON (escaped, without its quotes) in the
    /// answer to `id`: the message limit less the rest of the answer. `None`: no bound.
    pub fn text_room(&self, id: &RequestId) -> Option<u64> {
        let m = self.message?;
        Some(m.saturating_sub(answer_overhead(id)))
    }

    /// The bytes a job's single text block may take as JSON in the answer to lmgw's `tasks/result`
    /// for task `task_id` (WP18): the message limit less the rest of that answer, its
    /// related-task `_meta` included. lmgw's request id is not known before it asks, so the widest
    /// numeric id stands in (lmgw's ids are numbers); `None`: no bound.
    pub fn task_text_room(&self, task_id: &str) -> Option<u64> {
        let m = self.message?;
        let id = RequestId::Number(i64::MIN);
        let empty = |r: CallToolResult| answer_len(&id, &related(r, task_id));
        let overhead = empty(CallToolResult::success(vec![ContentBlock::text("")]))
            .max(empty(CallToolResult::error(vec![ContentBlock::text("")])));
        Some(m.saturating_sub(overhead))
    }
}

/// `result` as `tasks/result` answers it: with MCP's related-task `_meta` naming `task_id`.
pub fn related(mut result: CallToolResult, task_id: &str) -> CallToolResult {
    let mut meta = result.meta.take().unwrap_or_default();
    meta.insert(
        rmcp::model::RelatedTaskMetadata::META_KEY.into(),
        serde_json::json!({"taskId": task_id}),
    );
    result.meta = Some(meta);
    result
}

/// Counts what serde_json writes, without keeping it.
#[derive(Default)]
struct Count(u64);

impl std::io::Write for Count {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn json_len<T: serde::Serialize>(v: &T) -> u64 {
    let mut c = Count::default();
    serde_json::to_writer(&mut c, v)
        .map(|()| c.0)
        .unwrap_or(u64::MAX)
}

/// The answer's message to `id` around its result: `{"jsonrpc":"2.0","id":…,"result":…}`.
fn envelope(id: &RequestId) -> u64 {
    let probe = CallToolResult::success(Vec::new());
    let whole = json_len(&ServerJsonRpcMessage::response(
        ServerResult::CallToolResult(probe.clone()),
        id.clone(),
    ));
    whole - json_len(&probe)
}

/// The answer to `id` with one empty text block, an error or not (the larger): what a tool's
/// text adds to.
fn answer_overhead(id: &RequestId) -> u64 {
    let empty = |r: CallToolResult| json_len(&r);
    envelope(id)
        + empty(CallToolResult::success(vec![ContentBlock::text("")]))
            .max(empty(CallToolResult::error(vec![ContentBlock::text("")])))
}

/// The size of the message answering `id` with `result`, as the link writes it.
pub fn answer_len(id: &RequestId, result: &CallToolResult) -> u64 {
    envelope(id) + json_len(result)
}

/// The bytes `text` takes inside a JSON string (serde_json's escaping: `"`, `\` and the control
/// characters; everything else as its UTF-8).
pub fn escaped_len(text: &str) -> u64 {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0C}' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf8() as u64,
        })
        .sum()
}

/// `text` as the frames Kai writes: one text frame when it fits `max` (or there is no bound), else
/// a text frame and continuation frames of at most `max` bytes each, cut at characters' edges.
pub fn frames(text: String, max: Option<u64>) -> Vec<Message> {
    let Some(max) = max.map(|m| m.max(4) as usize) else {
        return vec![Message::Text(text.into())];
    };
    if text.len() <= max {
        return vec![Message::Text(text.into())];
    }
    let mut cuts = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + max).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        cuts.push((at, end));
        at = end;
    }
    let bytes = Bytes::from(text.into_bytes());
    let last = cuts.len() - 1;
    cuts.into_iter()
        .enumerate()
        .map(|(i, (a, b))| {
            let op = if i == 0 { Data::Text } else { Data::Continue };
            Message::Frame(Frame::message(
                bytes.slice(a..b),
                OpCode::Data(op),
                i == last,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_name_where_they_come_from() {
        let f = LinkLimits::fallback(64, 16);
        assert_eq!(
            (f.message, f.frame_bytes()),
            (Some(64 * MIB), Some(16 * MIB))
        );
        assert_eq!(
            f.message_says(),
            "host.fallback_max_message_mb = 64 (lmgw announced no limits)"
        );
        let a = LinkLimits::announced(
            &HostLimits {
                max_message_bytes: Some(1000),
                max_frame_bytes: None,
                ping_interval_s: None,
                call_timeout_ms: None,
            },
            &LinkLimits::fallback(0, 0),
        );
        assert_eq!(
            a.frame_bytes(),
            Some(1000),
            "no frame limit said or kept: bounded by the message"
        );
        // lmgw's `null` message (`mcp.host_max_message_mb` 0): no message bound, the frames still
        // bounded by what lmgw said, or else by the fallback's.
        let unbounded = |frame| HostLimits {
            max_message_bytes: None,
            max_frame_bytes: frame,
            ..HostLimits::default()
        };
        let u = LinkLimits::announced(&unbounded(Some(4096)), &f);
        assert_eq!((u.message, u.frame_bytes()), (None, Some(4096)));
        assert_eq!(u.text_room(&RequestId::Number(1)), None);
        assert_eq!(
            u.message_says(),
            "none: lmgw's mcp.host_max_message_mb is 0"
        );
        let u = LinkLimits::announced(&unbounded(None), &f);
        assert_eq!((u.message, u.frame_bytes()), (None, Some(16 * MIB)));
        assert_eq!(
            a.message_says(),
            "1000 bytes, lmgw's mcp.host_max_message_mb as it announced it"
        );
        let id = RequestId::Number(7);
        let room = a.text_room(&id).unwrap();
        // A text of exactly the room makes a message of exactly the limit.
        let text = "x".repeat(room as usize);
        let r = CallToolResult::success(vec![ContentBlock::text(text.clone())]);
        assert_eq!(answer_len(&id, &r), 1000);
        let e = CallToolResult::error(vec![ContentBlock::text(text)]);
        assert!(answer_len(&id, &e) <= 1000, "an error's answer fits too");
        let real = serde_json::to_string(&ServerJsonRpcMessage::response(
            ServerResult::CallToolResult(r.clone()),
            id.clone(),
        ))
        .unwrap();
        assert_eq!(real.len() as u64, answer_len(&id, &r));
        assert_eq!(LinkLimits::fallback(0, 0).frame_bytes(), None);
    }

    #[test]
    fn escaping_is_measured_as_serde_json_writes_it() {
        for t in [
            "plain",
            "a\"b\\c\nd\te\u{1}f\u{7f}",
            "é€😀",
            "\u{FFFD}\r\u{0C}",
        ] {
            assert_eq!(
                escaped_len(t) + 2,
                serde_json::to_string(t).unwrap().len() as u64,
                "{t:?}"
            );
        }
    }

    #[test]
    fn a_message_over_the_frame_limit_is_cut_at_characters() {
        let one = frames("small".into(), Some(10));
        assert!(matches!(&one[..], [Message::Text(t)] if t.as_str() == "small"));
        let text = "aé".repeat(10); // 30 bytes
        let parts = frames(text.clone(), Some(8));
        assert!(parts.len() >= 4);
        let mut joined = Vec::new();
        for (i, m) in parts.iter().enumerate() {
            let Message::Frame(f) = m else { panic!() };
            assert!(f.payload().len() <= 8);
            assert!(
                std::str::from_utf8(f.payload()).is_ok(),
                "cut at a character"
            );
            assert_eq!(f.header().is_final, i == parts.len() - 1);
            let want = if i == 0 { Data::Text } else { Data::Continue };
            assert_eq!(f.header().opcode, OpCode::Data(want));
            joined.extend_from_slice(f.payload());
        }
        assert_eq!(joined, text.as_bytes());
        assert_eq!(frames(text, None).len(), 1);
    }
}
