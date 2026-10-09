//! api §5.4 on Kai's side: a cancelled call never runs, a running one is stopped, and the owner's
//! confirmation closes on a cancel and when lmgw's time is up.

use serde_json::json;

use super::*;

/// `notifications/cancelled` while the owner is being asked: the dialog closes and the tool never
/// runs.
#[tokio::test]
async fn a_call_cancelled_before_it_runs_never_runs() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c
        .call("poke", json!({"text": "x"}), key("script"), 60_000)
        .await;
    let mut q = r.ask().await;
    assert!(!q.reply.is_closed());
    c.cancel(id, "the turn was cancelled").await;
    tokio::time::timeout(WAIT, q.reply.closed())
        .await
        .expect("the dialog's channel closes on the cancel");
    // Whatever Kai answers (lmgw no longer waits), the tool did not run.
    let a = c.answer(id, &mut Vec::new()).await;
    assert!(a.get("error").is_some(), "{a}");
    assert_eq!(r.ran.all(), Vec::<String>::new());
    // An answer that comes late changes nothing.
    let _ = q.reply.send(true);
    let id = c
        .call("echo", json!({"text": "after"}), owner(), 60_000)
        .await;
    assert_eq!(
        text_of(&c.answer(id, &mut Vec::new()).await).0,
        "echo: after"
    );
    assert_eq!(r.ran.all(), ["echo by the owner"]);
}

/// A cancel lmgw sends right behind its call, before Kai could act on it: nothing runs.
#[tokio::test]
async fn a_cancel_right_behind_the_call_wins() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    for _ in 0..5 {
        let id = c
            .call("poke", json!({"text": "x"}), key("script"), 60_000)
            .await;
        c.cancel(id, "timeout").await;
        let a = c.answer(id, &mut Vec::new()).await;
        assert!(a.get("error").is_some(), "{a}");
    }
    // Any question that reached the confirmer was closed again.
    while let Ok(mut q) = r.asks.try_recv() {
        tokio::time::timeout(WAIT, q.reply.closed()).await.unwrap();
    }
    assert_eq!(r.ran.all(), Vec::<String>::new());
}

/// `notifications/cancelled` while the tool runs: its future is dropped and its token cancelled.
#[tokio::test]
async fn a_running_call_is_stopped_by_the_cancel() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c.call("hang", json!({}), owner(), 60_000).await;
    r.ran.until("hang started").await;
    c.cancel(id, "barge-in").await;
    r.ran.until("hang dropped").await;
    r.ran.until("hang's token cancelled").await;
    let a = c.answer(id, &mut Vec::new()).await;
    assert!(a.get("error").is_some(), "{a}");
}

/// The link dropping mid-call stops the call too (lmgw reports it abandoned).
#[tokio::test]
async fn a_dropped_link_stops_its_calls() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    c.call("hang", json!({}), owner(), 60_000).await;
    r.ran.until("hang started").await;
    let id = c.call("poke", json!({"text": "x"}), owner(), 60_000).await;
    let mut q = r.ask().await;
    let _ = id;
    drop(c);
    r.ran.until("hang dropped").await;
    tokio::time::timeout(WAIT, q.reply.closed()).await.unwrap();
    assert!(!r.ran.all().iter().any(|x| x.starts_with("poke")));
}

/// `lmgw/timeout_ms` passes with no answer: the dialog closes and the caller reads why.
#[tokio::test]
async fn an_unanswered_confirmation_ends_at_lmgws_timeout() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c.call("poke", json!({"text": "x"}), owner(), 300).await;
    let mut q = r.ask().await;
    let left = q.deadline.unwrap() - std::time::Instant::now();
    assert!(left <= Duration::from_millis(300), "{left:?}");
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error, "{text}");
    assert!(
        text.contains("did not confirm the call in time") && text.contains("lmgw/timeout_ms"),
        "{text}"
    );
    tokio::time::timeout(WAIT, q.reply.closed()).await.unwrap();
    assert_eq!(r.ran.all(), Vec::<String>::new());
}

/// A confirmation that cannot be shown (the GTK thread gone) denies, saying so.
#[tokio::test]
async fn a_confirmation_that_cannot_be_shown_denies() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    let id = c.call("poke", json!({"text": "x"}), owner(), 60_000).await;
    drop(r.ask().await);
    let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
    assert!(error && text.contains("could not ask the owner"), "{text}");
    assert_eq!(r.ran.all(), Vec::<String>::new());
}

/// The screen lock (the owner's decision, 2026-10-09): every call is refused while it is locked,
/// a call waiting for the owner's confirmation and a running one are stopped at the lock, and
/// after the unlock calls run again.
#[tokio::test]
async fn the_screen_lock_refuses_and_stops_calls() {
    let mut r = rig().await;
    let mut c = r.linked().await;
    // Running: stopped at the lock, its future dropped and the caller told why.
    let running = c.call("hang", json!({}), owner(), 60_000).await;
    r.ran.until("hang started").await;
    // Waiting for the owner: the dialog closes at the lock.
    let asked = c
        .call("poke", json!({"text": "x"}), key("script"), 60_000)
        .await;
    let mut q = r.ask().await;
    assert!(r.screen.set(true));
    r.ran.until("hang dropped").await;
    r.ran.until("hang's token cancelled").await;
    tokio::time::timeout(WAIT, q.reply.closed())
        .await
        .expect("the dialog closes at the lock");
    // The two answers come in either order.
    let mut seen = Vec::new();
    let (text, error) = text_of(&c.answer(running, &mut seen).await);
    assert!(error, "{text}");
    assert_eq!(
        text,
        "the screen is locked; Kai is not active; the screen locked while it ran, so hang was \
         stopped"
    );
    let other = match seen.iter().position(|m| m.get("id") == Some(&json!(asked))) {
        Some(i) => seen.remove(i),
        None => c.answer(asked, &mut Vec::new()).await,
    };
    let (text, error) = text_of(&other);
    assert!(error, "{text}");
    assert!(text.contains("while the owner was asked"), "{text}");
    // New calls while locked: refused, whoever calls, and none asks the owner.
    for caller in [owner(), key("script")] {
        let id = c.call("echo", json!({"text": "x"}), caller, 60_000).await;
        let (text, error) = text_of(&c.answer(id, &mut Vec::new()).await);
        assert!(error);
        assert_eq!(text, "the screen is locked; Kai is not active");
    }
    assert!(r.asks.try_recv().is_err(), "nobody was asked");
    assert!(
        !r.ran
            .all()
            .iter()
            .any(|x| x.starts_with("echo") || x.starts_with("poke"))
    );
    // Unlocked: calls run again.
    assert!(r.screen.set(false));
    let id = c
        .call("echo", json!({"text": "back"}), owner(), 60_000)
        .await;
    assert_eq!(
        text_of(&c.answer(id, &mut Vec::new()).await).0,
        "echo: back"
    );
}
