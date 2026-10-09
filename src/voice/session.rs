//! One bound realtime session (§5.2–§5.5): its socket and events, the derived state, the replies
//! and their audio in the player, and the truncate-first cut.
//!
//! The rules are `lmgw-client`'s (K3): the voice state is its [`Machine`], each response's audio
//! is its [`Replies`] (which item it is, ended, complete, cut), the truncate is built from the
//! player's heard position as its [`PlaybackCursor`], and a stop cancels only what its truncate
//! did not stop (`Replies::cancel_after_truncate`). The player is Kai's own, as the crate leaves
//! it to the client: which replies were heard to their end is kept beside the crate's `Replies`,
//! whose `items` are walked to see the others heard or cut. A reply whose `response.done` came
//! (kept beside them too: the machine's open set also drops a reply once it has played out) and
//! whose audio was heard to its end or cut, or that has none, is forgotten (`Replies::forget`):
//! nothing of it can come or be cut any more, and a long session's walks stay short.
//!
//! **Audio out.** A response's audio goes into the [`Player`] as one item per assistant item
//! (`item_id`), opened at `response.created` and closed once nothing is open and everything was
//! heard (so quiet listening keeps no playback stream). `response.output_audio.done` (or
//! `response.done` for a cancelled response) ends the item; `speaking` is left when its audio has
//! been heard to the end.
//!
//! **Cuts** (§5.5, chat-voice §9.2). A barge-in (`speech_started` while audio plays) and a stop (a
//! talk press over the voice, listening off, quit) flush the player; the flush answers what was
//! *heard*, and `conversation.item.truncate {audio_end_ms}` carries exactly that. A barge-in's
//! response the server cancels itself. A stop sends the truncate first and `response.cancel` only
//! when the truncate did not end the response. Audio of a cut response that is still on its way is
//! dropped. Unlike the dashboard, which flushes only the cut reply's item, Kai's cut flushes every
//! item in the player (§5.5), so each flushed item gets its own truncate.
//!
//! **A tool turn** (chat-voice §8.5): a `tool` chat frame's `start` marks the tool running and its
//! `result` (or the turn's `done`) ends it; once the listener has heard everything of the playing
//! reply that came, while the reply goes on, the reply is starved. The machine then reads
//! `thinking` while the tool runs after its spoken preamble.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use lmgw_client::realtime::Updates;
use lmgw_client::truncate::{Incoming, PlaybackCursor, Replies};
use lmgw_client::voice::{Machine, VoiceState};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::link::{
    ClientEvent, ErrorFacts, SessionEvent, ThreadRef, TurnDetection, VoiceSocket, append_samples,
    event_kind,
};
use crate::audio::{ItemCount, Player, RATE};

/// What was heard of an item, as the crate's truncate takes it.
fn heard(c: &ItemCount) -> PlaybackCursor {
    PlaybackCursor {
        heard_samples: c.heard,
        sample_rate: RATE,
    }
}

/// Why a reply's audio is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// The server heard the user start talking over it: it cancels the response itself.
    BargeIn,
    /// A talk press over the voice, listening off, a rebind or quit.
    Stop,
}

pub struct Session {
    pub socket: Box<dyn VoiceSocket>,
    pub events: mpsc::UnboundedReceiver<SessionEvent>,
    /// The thread it is bound to.
    pub thread: ThreadRef,
    /// `session.created` came: audio may flow.
    pub created: bool,
    /// The machine's clock starts here.
    pub epoch: Instant,
    pub machine: Machine,
    /// Each response's audio (`lmgw-client`'s bookkeeping): `Replies::items` lists the replies
    /// whose audio started, oldest first.
    pub replies: Replies,
    /// The replies heard to their end (the player is Kai's), until they are forgotten.
    pub played: HashSet<String>,
    /// The responses whose `response.done` came, until they are forgotten (review KAI-3 K3-1: the
    /// machine's `is_open` is no gate, as `played_out` closes a response too).
    pub done: HashSet<String>,
    pub player: Option<Player>,
    /// The thread's resolved turn detection from `session.created` (`None`: push-to-talk).
    pub resolved: Option<String>,
    /// The turn detection last sent in `session.update` (outer `None`: none sent yet): an
    /// unchanged choice is not sent again.
    pub sent_detection: Option<Option<TurnDetection>>,
    pub sent_half_duplex: bool,
    /// `session.update`s not answered yet: only the answer to the last one is the session's word
    /// (`lmgw-client`'s `Updates`).
    pub updates: Updates,
    /// Stages whose model is loading, with the alias (`lmgw.model.state`).
    pub loading: BTreeMap<String, String>,
    /// Stages the GPU hold holds, with the alias.
    pub held: BTreeMap<String, String>,
    /// The last `error` (a takeover's names the binder before the close comes).
    pub last_error: Option<ErrorFacts>,
    /// A push-to-talk release came before `session.created`: commit once the buffer went.
    pub commit_pending: bool,
    /// The machine holds a push-to-talk press (`ptt_down` without its `ptt_up` yet).
    pub ptt: bool,
    /// The session's reply is over and it waits to close (K5's grace), until then.
    pub grace_at: Option<Instant>,
    /// PipeWire is reachable: with it down no playback stream is opened (it could only fail), and a
    /// reply's audio counts as unheard.
    pub audio_up: bool,
    /// The owner bound it (a press, the tray): it took the thread over. Kai's own binds do not
    /// (K5, review KAI-2 K2-13).
    pub by_owner: bool,
    /// A note of the socket's (`SessionEvent::Note`) was notified: the next ones only go to the
    /// tooltip.
    pub noted: bool,
}

impl Session {
    pub fn new(
        socket: Box<dyn VoiceSocket>,
        events: mpsc::UnboundedReceiver<SessionEvent>,
        thread: ThreadRef,
        now: Instant,
    ) -> Self {
        Self {
            socket,
            events,
            thread,
            created: false,
            epoch: now,
            machine: Machine::default(),
            replies: Replies::default(),
            played: HashSet::new(),
            done: HashSet::new(),
            player: None,
            resolved: None,
            sent_detection: None,
            sent_half_duplex: false,
            updates: Updates::default(),
            loading: BTreeMap::new(),
            held: BTreeMap::new(),
            last_error: None,
            commit_pending: false,
            ptt: false,
            grace_at: None,
            audio_up: true,
            by_owner: true,
            noted: false,
        }
    }

    /// A push-to-talk press: the owner speaks (the machine's `ptt_down`).
    pub fn ptt_press(&mut self) {
        self.ptt = true;
        self.machine.ptt_down();
    }

    /// The press ends; `asked`: a commit and `response.create` went. Every release ends what a
    /// press began, whatever comes after it (a tap, a commit).
    pub fn ptt_release(&mut self, asked: bool) {
        if std::mem::take(&mut self.ptt) {
            self.machine.ptt_up(asked);
        } else if asked {
            self.machine.ptt_up(true);
        }
    }

    pub fn ms(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.epoch).as_secs_f64() * 1000.0
    }

    pub fn state(&self, now: Instant) -> VoiceState {
        self.machine.state(self.ms(now))
    }

    /// [`Self::send`] with an `event_id` an `error` about it echoes.
    pub fn send_tagged(&mut self, event: ClientEvent, event_id: String) {
        tracing::trace!(kind = event_kind(&event), %event_id, "voice: send");
        self.socket.send_tagged(event, event_id);
    }

    pub fn send(&mut self, event: ClientEvent) {
        // The kind and an append's sample count: never the audio (review KAI-2 K2-1).
        tracing::trace!(
            kind = event_kind(&event),
            samples = append_samples(&event),
            "voice: send"
        );
        self.socket.send(event);
    }

    /// Sends `session.update` when what the client owns differs from what was sent.
    pub fn update(&mut self, detection: Option<TurnDetection>, half_duplex: bool) {
        if !self.created
            || (self.sent_detection == Some(detection) && self.sent_half_duplex == half_duplex)
        {
            return;
        }
        self.sent_detection = Some(detection);
        self.sent_half_duplex = half_duplex;
        self.updates.sent();
        tracing::info!(
            turn_detection = detection.map_or("null", TurnDetection::key),
            half_duplex,
            "voice: session.update"
        );
        self.send(ClientEvent::SessionUpdate {
            turn_detection: detection,
            half_duplex,
        });
    }

    /// A quiet point (K5): nothing open, waited for or playing, nothing left to commit.
    pub fn quiet(&self) -> bool {
        self.machine.quiet()
            && !self.commit_pending
            && self.player.as_ref().is_none_or(Player::idle)
    }

    // ---------------------------------------------------------------- audio out

    /// A player is needed: a new one when there is none (or PipeWire closed it), unless PipeWire
    /// is down, where opening could only fail.
    fn ensure_player(&mut self, open_player: impl FnOnce() -> Player) -> bool {
        if self.player.as_ref().is_some_and(|p| !p.closed()) {
            return true;
        }
        if !self.audio_up {
            return false;
        }
        self.player = Some(open_player());
        true
    }

    /// A response's first event: its reply, and a player when there is none.
    pub fn response_created(
        &mut self,
        rid: &str,
        now: Instant,
        open_player: impl FnOnce() -> Player,
    ) {
        self.machine.response_created(rid);
        self.replies.created(rid);
        self.reap_closed(now);
        self.ensure_player(open_player);
    }

    pub fn audio(
        &mut self,
        rid: &str,
        item_id: &str,
        pcm: &[i16],
        now: Instant,
        open_player: impl FnOnce() -> Player,
    ) {
        match self.replies.audio(rid, item_id) {
            Incoming::Drop => return,
            Incoming::Start => self.machine.audio_started(rid),
            Incoming::Continue => {}
        }
        // A stream PipeWire closed is dealt with first, whichever comes first, this delta or the
        // tick: the replies it played are cut at what was heard, this one included.
        self.reap_closed(now);
        if self.replies.get(rid).is_some_and(|r| r.cut) {
            return;
        }
        if !self.ensure_player(open_player) {
            // PipeWire is down: none of it can be heard, so the thread keeps none of it.
            tracing::warn!(
                rid,
                "voice: PipeWire is down; the reply is cut at 0 ms heard"
            );
            self.end_unheard(rid, item_id, 0, now);
            return;
        }
        let player = self.player.as_mut().expect("opened above");
        if !player.push(item_id, pcm) {
            tracing::debug!(rid, item_id, "voice: audio of a flushed item dropped");
        }
    }

    /// `response.done`: the response is over on the gateway's side, and no more of its audio comes.
    pub fn response_done(&mut self, rid: &str) {
        self.machine.response_done(rid);
        self.end_audio(rid);
        self.done.insert(rid.to_owned());
    }

    /// No more audio comes for `rid`; `speaking` ends once it has been heard.
    pub fn end_audio(&mut self, rid: &str) {
        if !self.replies.end(rid) {
            return;
        }
        let item = self.replies.get(rid).and_then(|r| r.item_id.clone());
        match (item, self.player.as_mut()) {
            (Some(item), Some(p)) => p.end(&item),
            // No audio came at all: nothing to wait for.
            _ => {
                self.played.insert(rid.to_owned());
                self.machine.played_out(rid);
            }
        }
    }

    /// Reply `rid` ends with `heard_ms` of item `item` heard and no more to come (its stream is
    /// gone, or never opened): the truncate keeps what was heard (§5.5), and `response.cancel`
    /// follows only when the truncate did not end the response.
    fn end_unheard(&mut self, rid: &str, item: &str, heard_ms: u64, now: Instant) {
        let now_ms = self.ms(now);
        if self.replies.get(rid).is_none() {
            return;
        }
        self.replies.cut(rid);
        self.machine.cut(rid, now_ms, false);
        tracing::info!(
            response_id = %rid,
            item_id = %item,
            audio_end_ms = heard_ms,
            "voice: truncate at what was heard (the playback stream is gone)"
        );
        self.send(ClientEvent::Truncate {
            item_id: item.to_owned(),
            content_index: 0,
            audio_end_ms: heard_ms,
        });
        if self
            .replies
            .cancel_after_truncate(rid, self.machine.is_open(rid), true)
        {
            self.send(ClientEvent::ResponseCancel {
                response_id: Some(rid.to_owned()),
            });
        }
    }

    /// PipeWire closed the playback stream: what it held cannot play. Each reply still playing is
    /// cut where the last report put the heard position (no extrapolation: the stream is gone);
    /// the next response opens a new player once PipeWire is back.
    fn reap_closed(&mut self, now: Instant) {
        let Some(p) = self.player.as_ref().filter(|p| p.closed()) else {
            return;
        };
        tracing::warn!("voice: the playback stream closed; its replies end at what was heard");
        let counts = p.counts_reported();
        let cut: Vec<(String, String, u64)> = self
            .replies
            .items()
            .into_iter()
            .filter(|i| !i.cut && !self.played.contains(&i.response_id))
            .map(|i| {
                let heard = counts
                    .iter()
                    .find(|c| c.item == i.item_id)
                    .map_or(0, |c| c.heard_ms());
                (i.response_id, i.item_id, heard)
            })
            .collect();
        self.player = None;
        for (rid, item, heard) in cut {
            self.end_unheard(&rid, &item, heard, now);
        }
    }

    /// The tick's part: pumps the player, marks replies heard to their end, closes the player
    /// when nothing needs it. Returns a line to log (a gap was heard).
    pub fn pump(&mut self, now: Instant) -> Option<String> {
        self.reap_closed(now);
        let warning = self.pump_player();
        self.forget_done();
        warning
    }

    fn pump_player(&mut self) -> Option<String> {
        let p = self.player.as_mut()?;
        let warning = p.pump();
        for i in self.replies.items() {
            if i.cut || self.played.contains(&i.response_id) {
                continue;
            }
            let rid = i.response_id;
            let count = p.count(&i.item_id);
            if !i.ended {
                // Starved: everything of it that came was heard, and more is to come (§8.5).
                if self.machine.playing() == Some(rid.as_str()) {
                    let dry = count.as_ref().is_some_and(|c| c.heard >= c.pushed);
                    self.machine.starved(&rid, dry);
                }
                continue;
            }
            if count.is_none_or(|c| c.heard >= c.pushed) {
                p.forget(&i.item_id);
                self.machine.played_out(&rid);
                self.played.insert(rid);
            }
        }
        if self.machine.current().is_none() && p.idle() {
            tracing::debug!("voice: playback idle, its stream closes");
            self.player = None;
        }
        warning
    }

    /// Forgets the replies whose `response.done` came and whose audio was heard to its end, was
    /// cut, or never came (a cut before the first audio included): nothing of them can come or be
    /// cut any more (`Replies::forget`), so a long session's walks stay short (review KAI-2 K2-18).
    /// A reply heard to its end before its `response.done` is kept until then, ended, so a late
    /// delta of it is dropped rather than read as a new reply's (review KAI-3 K3-1).
    fn forget_done(&mut self) {
        let over: Vec<String> = self
            .done
            .iter()
            .filter(|rid| {
                self.replies.get(rid).is_none_or(|r| {
                    r.cut || r.item_id.is_none() || self.played.contains(rid.as_str())
                })
            })
            .cloned()
            .collect();
        for rid in over {
            self.replies.forget(&rid);
            self.played.remove(&rid);
            self.done.remove(&rid);
        }
    }

    /// Cuts response `rid`'s audio (module doc).
    pub async fn cut(&mut self, rid: &str, why: Cut, flush_wait: Duration, now: Instant) {
        let now_ms = self.ms(now);
        if !self.replies.cut(rid) {
            return;
        }
        let stop = why == Cut::Stop;
        let item = self.replies.get(rid).and_then(|r| r.item_id.clone());
        let (Some(player), Some(item)) = (self.player.as_mut(), item) else {
            // Nothing of it reached the player: nothing to truncate, only the cancel.
            if stop && self.machine.is_open(rid) {
                self.send(ClientEvent::ResponseCancel {
                    response_id: Some(rid.to_owned()),
                });
            }
            return;
        };
        let heard_any = player.count(&item).is_some_and(|c| c.played > 0);
        self.machine.cut(rid, now_ms, heard_any);
        let (counts, note) = player.flushed(flush_wait).await;
        if let Some(n) = note {
            tracing::warn!("voice: {n}");
        }
        // Every item in the player is flushed: this reply's, and any queued behind it (§5.5).
        for c in counts {
            let Some(other) = self.replies.response_of(&c.item).map(str::to_owned) else {
                continue;
            };
            if other != rid {
                if self.played.contains(&other) || !self.replies.cut(&other) {
                    continue;
                }
                self.machine.cut(&other, now_ms, c.played > 0);
            }
            let Some(truncate) = self.replies.truncate(&other, Some(heard(&c))) else {
                continue;
            };
            tracing::info!(
                response_id = %other,
                item_id = %c.item,
                audio_end_ms = c.heard_ms(),
                played_ms = c.played_ms(),
                ?why,
                "voice: truncate at what was heard"
            );
            self.send(truncate);
            if let Some(p) = self.player.as_mut() {
                p.forget(&c.item);
            }
            if stop
                && self
                    .replies
                    .cancel_after_truncate(&other, self.machine.is_open(&other), true)
            {
                self.send(ClientEvent::ResponseCancel {
                    response_id: Some(other.clone()),
                });
            }
        }
    }

    /// Leaving (listening off, a rebind, quit, §2): the reply still playing is cut at what was
    /// heard before the socket closes; a response still open is cancelled.
    pub async fn leave(&mut self, flush_wait: Duration, now: Instant) {
        if let Some(rid) = self.machine.playing().map(str::to_owned) {
            self.cut(&rid, Cut::Stop, flush_wait, now).await;
        } else if let Some(rid) = self.machine.current().map(str::to_owned) {
            self.send(ClientEvent::ResponseCancel {
                response_id: Some(rid),
            });
        }
        self.player = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A socket that only counts what is sent.
    struct Sent(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);

    impl VoiceSocket for Sent {
        fn send(&mut self, event: ClientEvent) {
            self.0.lock().unwrap().push(event_kind(&event));
        }
        fn close(&mut self) {}
    }

    fn no_player() -> Player {
        unreachable!("PipeWire is down in this test: no player is opened")
    }

    type SentKinds = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

    fn session(now: Instant) -> (Session, SentKinds) {
        let sent = SentKinds::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let thread = ThreadRef {
            id: 1,
            title: "Kai".into(),
        };
        (
            Session::new(Box::new(Sent(sent.clone())), rx, thread, now),
            sent,
        )
    }

    /// Nothing of any reply is kept: not in `Replies`, not heard, not done.
    fn nothing_kept(s: &Session) -> bool {
        s.replies == Replies::default() && s.played.is_empty() && s.done.is_empty()
    }

    #[tokio::test(start_paused = true)]
    async fn done_replies_are_forgotten_and_open_ones_kept() {
        let now = Instant::now();
        let (mut s, sent) = session(now);
        // PipeWire is down: every reply's audio is cut at 0 ms heard (the truncate keeps none).
        s.audio_up = false;
        for n in 0..50 {
            let rid = format!("resp_{n}");
            s.response_created(&rid, now, no_player);
            s.audio(&rid, &format!("item_{n}"), &[0; 480], now, no_player);
            // A response without audio is heard at its end.
            let quiet = format!("quiet_{n}");
            s.response_created(&quiet, now, no_player);
            // A talk press while Kai thinks: cut before its first audio (review KAI-3 K3-2).
            let early = format!("early_{n}");
            s.response_created(&early, now, no_player);
            s.cut(&early, Cut::Stop, Duration::ZERO, now).await;
            for r in [&rid, &quiet, &early] {
                s.response_done(r);
            }
            s.pump(now);
        }
        assert!(
            nothing_kept(&s),
            "50 turns leave nothing behind: {:?}, {:?}, {:?}",
            s.replies,
            s.played,
            s.done
        );
        let sent = sent.lock().unwrap().clone();
        assert!(sent.contains(&"conversation.item.truncate"));
        assert_eq!(
            sent.iter().filter(|k| **k == "response.cancel").count(),
            50,
            "each early cut cancelled its open response"
        );
        // A cut reply whose response is still open stays: its response.done is still to come.
        s.response_created("open", now, no_player);
        s.audio("open", "item_open", &[0; 480], now, no_player);
        s.pump(now);
        let items = s.replies.items();
        assert_eq!(items.len(), 1);
        assert!(items[0].cut && items[0].response_id == "open");
        // So is a reply cut before its first audio.
        s.response_created("early", now, no_player);
        s.cut("early", Cut::Stop, Duration::ZERO, now).await;
        s.pump(now);
        assert!(s.replies.get("early").is_some_and(|r| r.cut));
        s.response_done("open");
        s.response_done("early");
        s.pump(now);
        assert!(nothing_kept(&s));
    }

    /// Review KAI-3 K3-1: a reply heard to its end before its `response.done` is kept, ended, so
    /// a late delta of it is dropped and the voice does not read speaking again; its
    /// `response.done` then forgets it.
    #[tokio::test(start_paused = true)]
    async fn a_reply_played_out_before_its_done_drops_a_late_delta() {
        let now = Instant::now();
        let (mut s, _) = session(now);
        let (player, mut head) = crate::audio::test_support::playback(2000);
        s.response_created("resp_1", now, move || player);
        s.audio("resp_1", "item_1", &[900; 480], now, no_player);
        assert_eq!(s.state(now), VoiceState::Speaking);
        s.replies.audio_done("resp_1");
        s.end_audio("resp_1");
        // PipeWire plays it out: 40 ms in 10 ms cycles, no device latency.
        for _ in 0..4 {
            head.cycle(240, 0);
            tokio::time::advance(Duration::from_millis(10)).await;
        }
        let now = Instant::now();
        s.pump(now);
        assert!(s.played.contains("resp_1"), "heard to its end");
        assert!(!s.machine.is_open("resp_1"), "played_out closes it");
        assert!(
            s.replies.get("resp_1").is_some_and(|r| r.ended),
            "kept until its response.done"
        );
        assert_ne!(s.state(now), VoiceState::Speaking);
        // A late delta of it (a duplicate frame, a later item): dropped.
        s.audio("resp_1", "item_1", &[900; 480], now, no_player);
        s.audio("resp_1", "item_2", &[900; 480], now, no_player);
        s.pump(now);
        assert_eq!(s.machine.playing(), None);
        assert_ne!(s.state(now), VoiceState::Speaking);
        assert_eq!(
            s.player.as_ref().and_then(|p| p.count("item_2")),
            None,
            "nothing of it reached the player"
        );
        s.response_done("resp_1");
        s.pump(now);
        assert!(nothing_kept(&s));
    }
}
