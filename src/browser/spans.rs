//! Kai's input spans (kai-firefox design §6.3, the contract's `input.begin@1` and `input.end@1`):
//! input Kai sends through the desktop (the trusted press, `type_text`, `key_press`, `ui_press`)
//! is trusted input to Firefox like the user's, so Kai brackets every one with a span in every
//! linked Firefox. Inside a span the extension counts trusted input in any tab as Kai's
//! (`kai_input_at`), never the user's act the focus rule reads (`super::moves`).
//!
//! - **One span per linked profile** ([`Spans::begin`]), opened before Kai's first event and
//!   ended after its last ([`Open::end`]): `settle_ms` 0 after the trusted press's
//!   `gate.result@1` (whose window waited already), else `browser.input_settle_ms`, for keys still
//!   on their way to Firefox ([`Open::end_later`]: the call answers at once, the span ends in the
//!   background).
//! - **Best effort**: a profile whose span could not be opened (the begin failed, or its extension
//!   does not serve it) is logged, and its moment is *unbracketed* (`super::moves`): its tabs'
//!   facts in it are Kai's, until the user acts there after it. Kai's input goes on: a span is
//!   bookkeeping, not a lock.
//! - **Kai's moment** ([`super::moves::Moves::input_began`]) runs from the spans' opening to their
//!   end in every profile, which the focus rule reads for the windows' focus.
//! - **However the input ends**: an [`Open`] dropped without its end (an error, a call cancelled
//!   while it ran) ends its spans in the background.
//!
//! Nothing here waits by a clock of Kai's: a begin or an end waits for its answer, the link's end,
//! or (a begin) the call's future being dropped.

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::links::{LinkInfo, Links, OpError};
use super::moves::{Moves, now_ms};
use crate::host::policy::SharedPolicy;

pub const BEGIN: &str = "input.begin@1";
pub const END: &str = "input.end@1";

/// The spans and the focus rule's marks they feed, shared by the browser tools and the desktop's
/// input tools. Cheap to clone.
#[derive(Clone)]
pub struct Spans {
    links: Links,
    policy: SharedPolicy,
    moves: Arc<Mutex<Moves>>,
}

/// What Kai's input reached, by profile (`input.end@1`'s `tabs`), or why a span did not end.
pub type Ended = Vec<(LinkInfo, Result<Vec<i64>, OpError>)>;

impl Spans {
    pub fn new(links: Links, policy: SharedPolicy, moves: Moves) -> Self {
        Self {
            links,
            policy,
            moves: Arc::new(Mutex::new(moves)),
        }
    }

    /// The running Kai's: its marks kept in Kai's state directory when there is one
    /// (`super::moves::STORE`).
    pub fn of(deps: &crate::host::registry::Deps) -> Self {
        let moves = deps.state_dir.as_ref().map_or_else(Moves::default, |d| {
            Moves::stored(d.join(super::moves::STORE))
        });
        Self::new(deps.browser.clone(), deps.policy.clone(), moves)
    }

    pub fn moves(&self) -> MutexGuard<'_, Moves> {
        self.moves.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `browser.input_settle_ms`, now.
    pub fn settle_ms(&self) -> u64 {
        self.policy
            .read()
            .expect("never poisoned")
            .browser()
            .input_settle_ms
    }

    /// Opens a span in every linked Firefox (module doc): until each answered, `cancel` fired or
    /// its link ended. `why` names the input for the log.
    pub async fn begin(&self, why: &'static str, cancel: &CancellationToken) -> Open {
        let linked = self.links.list();
        let begins = linked.iter().map(|l| async move {
            if !l.ops.iter().any(|o| o == BEGIN) {
                return (l, Err(OpError::NotServed { op: BEGIN.into() }));
            }
            let r = self
                .links
                .op(l.id, BEGIN, json!({}), cancel)
                .await
                .and_then(|v| {
                    v.get("span").and_then(Value::as_u64).ok_or_else(|| {
                        OpError::failed(
                            "bad_answer",
                            format!("{BEGIN} answered without its span: {v}"),
                        )
                    })
                });
            (l, r)
        });
        let results = futures_util::future::join_all(begins).await;
        let mut profiles = Vec::new();
        let mut open = Vec::new();
        for (l, r) in results {
            match r {
                Ok(span) => {
                    profiles.push((l.profile.clone(), true));
                    open.push((l.clone(), span));
                }
                Err(e) => {
                    tracing::warn!(
                        link = l.id,
                        profile = l.profile,
                        input = why,
                        error = %super::tools::said(&e),
                        "browser: Kai could not open its input span in this Firefox; its tabs \
                         count as touched by Kai until the user acts in them"
                    );
                    profiles.push((l.profile.clone(), false));
                }
            }
        }
        let ticket = self.moves().input_began(&profiles);
        Open {
            spans: self.clone(),
            open,
            ticket,
            why,
            ended: false,
        }
    }
}

/// Kai's open spans (module doc).
pub struct Open {
    spans: Spans,
    open: Vec<(LinkInfo, u64)>,
    ticket: u64,
    why: &'static str,
    ended: bool,
}

impl Open {
    /// Ends the spans with `settle_ms` and waits until each ended (`settle_ms` after it was
    /// asked, or at once on an error): what Kai's input reached in each profile.
    pub async fn end(mut self, settle_ms: u64) -> Ended {
        self.ended = true;
        let open = std::mem::take(&mut self.open);
        end(self.spans.clone(), open, self.ticket, self.why, settle_ms).await
    }

    /// Ends the spans in the background with `browser.input_settle_ms` (module doc).
    pub fn end_later(mut self) {
        self.ended = true;
        let open = std::mem::take(&mut self.open);
        let settle = self.spans.settle_ms();
        spawn_end(self.spans.clone(), open, self.ticket, self.why, settle);
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let open = std::mem::take(&mut self.open);
        let settle = self.spans.settle_ms();
        spawn_end(self.spans.clone(), open, self.ticket, self.why, settle);
    }
}

/// [`end`] in the background; without a runtime (a test's teardown), Kai's moment ends at once.
fn spawn_end(
    spans: Spans,
    open: Vec<(LinkInfo, u64)>,
    ticket: u64,
    why: &'static str,
    settle: u64,
) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(async move {
                end(spans, open, ticket, why, settle).await;
            });
        }
        Err(_) => spans.moves().input_ended(ticket, now_ms() + settle as i64),
    }
}

/// Ends every span of `open` (module doc). Kai's moment covers the settle time in every profile,
/// those without a span included, whose keys may still come.
async fn end(
    spans: Spans,
    open: Vec<(LinkInfo, u64)>,
    ticket: u64,
    why: &'static str,
    settle_ms: u64,
) -> Ended {
    spans
        .moves()
        .input_ended(ticket, now_ms() + settle_ms as i64);
    // Not the call's: a span ends with its answer or its link, also after the call.
    let cancel = CancellationToken::new();
    let ends = open.iter().map(|(l, span)| {
        let links = &spans.links;
        let cancel = &cancel;
        async move {
            let r = links
                .op(
                    l.id,
                    END,
                    json!({"span": span, "settle_ms": settle_ms}),
                    cancel,
                )
                .await
                .map(|v| {
                    v.get("tabs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                        .collect::<Vec<i64>>()
                });
            match &r {
                Ok(tabs) => tracing::debug!(
                    link = l.id,
                    input = why,
                    ?tabs,
                    "browser: Kai's input span ended"
                ),
                Err(e) => tracing::info!(
                    link = l.id,
                    input = why,
                    error = %super::tools::said(e),
                    "browser: Kai's input span did not end as asked (it ends with the link)"
                ),
            }
            (l.clone(), r)
        }
    });
    let ended = futures_util::future::join_all(ends).await;
    spans.moves().input_ended(ticket, now_ms());
    ended
}
