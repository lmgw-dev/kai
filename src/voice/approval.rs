//! The voice's part of approvals (WP19, K17; `crate::approvals` holds the calls and the card):
//! the talk key records the spoken answer while the card asks for it (a call of Kai's own, the
//! card armed for it), and the verdicts go out through the bound session (`mcp_approval_response`
//! each, then `response.create`) or the approvals route, the spoken answer through `…/transcribe`,
//! a thread's waiting calls read again through `GET /chat/api/threads/{id}`.

use tokio::time::Instant;

use super::link::ClientEvent;
use super::session::Cut;
use super::{Key, Voice, VoiceLink};
use crate::approvals::{Act, Done, Op, Wake};
use crate::audio::AudioPort;
use lmgw_client::requests::approvals::ApprovalsRequest;

impl<L: VoiceLink> Voice<L> {
    /// The press while the card asks for the voice: the spoken answer is recorded from the
    /// pre-roll on. A reply
    /// still playing stops (K6: the press is the owner's turn to speak). Muted, the hold records
    /// nothing (§5.4).
    pub(super) async fn approval_press(&mut self, audio: &impl AudioPort, now: Instant) {
        // The key is held for the answer: nothing else of K6 applies to this press.
        self.key = Some(Key {
            at: now,
            consumed: true,
            muted: self.muted,
            gated: false,
        });
        if self.muted {
            tracing::info!("talk: muted, so the spoken answer records nothing (§5.4)");
            return self.settle();
        }
        let speaking = self
            .session
            .as_ref()
            .and_then(|s| s.machine.playing().map(str::to_owned));
        if let Some(rid) = speaking {
            tracing::info!("talk: the press stops the voice to hear the approval's answer");
            let wait = self.s.flush_wait;
            if let Some(s) = self.session.as_mut() {
                s.cut(&rid, Cut::Stop, wait, now).await;
            }
        }
        let preroll: Vec<i16> = self.preroll.drain(..).collect();
        if self.approvals.press(preroll) {
            self.open_capture(audio);
            self.ensure_tick();
        }
        self.settle();
    }

    /// The release of an answer's press: what the microphone heard up to now goes with it, then to
    /// be transcribed.
    pub(super) fn approval_release(&mut self) {
        self.drain_capture();
        let acts = self.approvals.release();
        self.approval_acts(acts);
        self.settle();
    }

    /// An event of the card, or an approvals request that came back.
    pub(super) async fn on_approval(&mut self, w: Wake) {
        let session = self
            .session
            .as_ref()
            .filter(|s| s.created)
            .map(|s| (s.thread.id, self.session_no));
        let acts = self.approvals.on_wake(w, session);
        self.approval_acts(acts);
    }

    /// Carries out what the approvals ask.
    pub(super) fn approval_acts(&mut self, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Session {
                    thread_id,
                    decisions,
                    create,
                } => match self
                    .session
                    .as_mut()
                    .filter(|s| s.created && s.thread.id == thread_id)
                {
                    Some(s) => {
                        for d in decisions {
                            s.send(ClientEvent::ApprovalResponse {
                                approval_request_id: d.approval_request_id,
                                approve: d.approve,
                                reason: d.reason,
                            });
                        }
                        // Its id comes back on an `error` refusing it (M1: any code).
                        s.send_tagged(ClientEvent::ResponseCreate, create);
                    }
                    // The session went in between: the approvals see it gone at the next settle
                    // (`Approvals::observe`) and send the verdicts by the route.
                    None => tracing::info!(
                        thread = thread_id,
                        calls = decisions.len(),
                        "approvals: the session went before the verdicts; the route takes them"
                    ),
                },
                Act::Route { thread_id, body } => self.decide_by_route(thread_id, body),
                Act::Reread { thread_id } => {
                    let fut = self.link.pending_approvals(thread_id);
                    self.approvals.push(
                        Op::Reread { thread_id },
                        Box::pin(async move { Done::Pending(fut.await) }),
                    );
                }
                Act::Transcribe {
                    thread_id,
                    call,
                    wav,
                } => {
                    let fut = self.link.transcribe(thread_id, wav);
                    self.approvals.push(
                        Op::Hear { call },
                        Box::pin(async move { Done::Heard(fut.await) }),
                    );
                }
            }
        }
    }

    fn decide_by_route(&mut self, thread_id: i64, body: ApprovalsRequest) {
        let ids = body
            .decisions
            .iter()
            .map(|d| d.approval_request_id.clone())
            .collect();
        let fut = self.link.approvals(thread_id, &body);
        self.approvals.push(
            Op::Decide { thread_id, ids },
            Box::pin(async move { Done::Decided(fut.await) }),
        );
    }
}
