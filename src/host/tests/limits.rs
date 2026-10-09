//! The link's limits on Kai's side: lmgw's announcement in `initialize`, frames within its frame
//! limit, a result over its message limit as the call's error (the link stays up), the room and
//! the deadline a tool sees, and what a tool resolves before the owner's confirmation.

use serde_json::json;

use super::*;

/// A message larger than the announced frame limit reaches lmgw whole, as several frames: the fake
/// host fails the link on any frame above 64 KiB, as lmgw closes it with 1009.
#[tokio::test]
async fn a_large_message_goes_out_as_frames_within_the_limit() {
    let mut r = rig().await;
    let mut c = r.fake.accept_frames_of(64 * 1024).await;
    c.initialize_with(Some(json!({
        "max_message_bytes": 4 * 1024 * 1024,
        "max_frame_bytes": 64 * 1024,
        "ping_interval_s": 30
    })))
    .await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    let text = "é and \"quotes\"\n".repeat(20_000); // about 340 KB as JSON
    let id = c
        .call("echo", json!({ "text": text }), owner(), 60_000)
        .await;
    let (got, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(!error);
    assert_eq!(got, format!("echo: {text}"));
    // The link is still up.
    let id = c
        .call("echo", json!({"text": "again"}), owner(), 60_000)
        .await;
    assert_eq!(
        text_of(&c.answer(id, &mut Vec::new()).await).0,
        "echo: again"
    );
}

/// lmgw's `null` message limit (`mcp.host_max_message_mb` 0): no message bound, not even
/// `[host]`'s fallback, while the frames stay within the frame limit lmgw said.
#[tokio::test]
async fn a_null_message_limit_bounds_nothing_but_the_frames() {
    let mut settings = settings(1);
    settings.fallback = super::super::limits::LinkLimits::fallback(1, 1);
    let mut r = rig_with(PolicyConfig::default(), settings).await;
    let mut c = r.fake.accept_frames_of(64 * 1024).await;
    c.initialize_with(Some(json!({
        "max_message_bytes": null,
        "max_frame_bytes": 64 * 1024,
        "ping_interval_s": 30,
        "call_timeout_ms": 60_000
    })))
    .await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    let id = c.call("ctx", json!({}), owner(), 60_000).await;
    let (text, _) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(text.starts_with("room None"), "{text}");
    let text = "w".repeat(1_100_000); // over the 1 MiB fallback
    let id = c
        .call("echo", json!({ "text": text }), owner(), 60_000)
        .await;
    let (got, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(!error, "{}", &got[..got.len().min(200)]);
    assert_eq!(got.len(), "echo: ".len() + text.len());
}

/// A result that would be a larger message than the announced limit is the call's error naming
/// the limit, not a close of the link; the next call works.
#[tokio::test]
async fn a_result_over_the_message_limit_is_the_calls_error() {
    let mut r = rig().await;
    let mut c = r.fake.accept().await;
    c.initialize_with(Some(
        json!({"max_message_bytes": 10_000, "max_frame_bytes": 0}),
    ))
    .await;
    r.status(|s| matches!(s, HostStatus::Linked { .. })).await;
    let id = c
        .call(
            "echo",
            json!({ "text": "x".repeat(20_000) }),
            owner(),
            60_000,
        )
        .await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error, "{text}");
    assert!(
        text.contains("more than the tool link takes")
            && text.contains("10000 bytes, lmgw's mcp.host_max_message_mb as it announced it"),
        "{text}"
    );
    // The room a tool sees: the limit less the rest of its answer; a text of exactly that fits.
    let id = c.call("ctx", json!({}), owner(), 60_000).await;
    let (text, _) = text_of(&c.answer(id, &mut Vec::new()).await);
    let room: u64 = text
        .strip_prefix("room Some(")
        .and_then(|t| t.split(')').next())
        .unwrap()
        .parse()
        .unwrap();
    assert!(room > 9_900 && room < 10_000, "{text}");
    assert!(
        text.ends_with("timeout Some(60000); deadline true"),
        "{text}"
    );
    let fits = "y".repeat(room as usize - "echo: ".len());
    let id = c
        .call("echo", json!({ "text": fits }), owner(), 60_000)
        .await;
    let a = c.answer(id, &mut Vec::new()).await;
    assert!(!text_of(&a).1, "a text of the room fits");
    assert_eq!(
        a.to_string().len(),
        10_000,
        "exactly the limit (ids of one length)"
    );
}

/// With an lmgw that announces nothing, `[host]`'s fallbacks bound the link and a result names
/// them.
#[tokio::test]
async fn without_an_announcement_the_fallbacks_apply() {
    let mut settings = settings(1);
    settings.fallback = super::super::limits::LinkLimits::fallback(1, 1);
    let mut r = rig_with(PolicyConfig::default(), settings).await;
    let mut c = r.linked().await;
    let id = c.call("ctx", json!({}), owner(), 60_000).await;
    let (text, _) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(
        text.contains("says host.fallback_max_message_mb = 1 (lmgw announced no limits)"),
        "{text}"
    );
    let id = c
        .call(
            "echo",
            json!({ "text": "z".repeat(1_100_000) }),
            owner(),
            60_000,
        )
        .await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(
        error && text.contains("host.fallback_max_message_mb = 1"),
        "{text}"
    );
}

/// A tool's resolution comes before the confirmation, which shows it, and the handler gets it; a
/// refusal of the resolution asks nobody and runs nothing.
#[tokio::test]
async fn a_tool_resolves_before_the_owner_is_asked() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c.call("prep", json!({"text": "a"}), owner(), 60_000).await;
    let q = r.ask().await;
    assert_eq!(q.lines, ["text: a", "resolved: a"]);
    q.reply.send(true).unwrap();
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(!error);
    assert_eq!(text, "prep ran Some(\"plan a\")");
    let id = c
        .call("prep", json!({"text": "refuse"}), owner(), 60_000)
        .await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error);
    assert_eq!(text, "refused before anyone was asked");
    assert!(r.asks.try_recv().is_err(), "nobody was asked");
    assert_eq!(r.ran.all(), ["prep by the owner"]);
}

/// A tool's resolution learns that lmgw approved the call: the same arguments resolve for a call
/// that was not approved there.
#[tokio::test]
async fn a_tools_resolution_knows_the_call_was_approved_in_lmgw() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c
        .call_approved("prep", json!({"text": "name-it"}), owner(), 60_000)
        .await;
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error);
    assert_eq!(text, "approved in lmgw: name it");
    let id = c
        .call("prep", json!({"text": "name-it"}), owner(), 60_000)
        .await;
    let q = r.ask().await;
    assert_eq!(q.lines, ["text: name-it", "resolved: name-it"]);
    q.reply.send(true).unwrap();
    let (_, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(!error);
}
