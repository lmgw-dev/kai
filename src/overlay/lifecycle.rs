//! The ring's lifetime and visibility as a pure state machine (K1, K11, §9.4).
//!
//! The voice session (WP4) says what is going on as a [`RingInput`]; the page and the GTK side say
//! what happened ([`Lifecycle::ready`], [`Lifecycle::settled`], [`Lifecycle::crashed`]). Each call
//! returns the [`Action`]s the GTK side carries out, in order.
//!
//! - **No view while nothing needs it.** The view (and its web process) exists while a voice
//!   session is bound or Kai is active; otherwise it is destroyed.
//! - **Created hidden** when a session binds or Kai leaves standby, so the ring shows as soon as its
//!   page is ready (WP0: a view takes about 250 ms to its first frame).
//! - **Shown while Kai is active, hidden in standby** (K11). Active is actively listening, thinking
//!   or talking: continuous listening is active the whole time, push-to-talk from a press through
//!   its turn's end. Out of a turn the page holds the listening state, fed the microphone's levels;
//!   during a turn the voice state, and a turn's end goes back to listening while Kai stays active.
//!   A turn's start places a mapped ring again (the output and layer of that moment).
//! - **Hidden after the engine drew the idle state**: when Kai enters standby, the page is sent
//!   idle, and the surface goes once the page reports that idle state drawn (`settled`). Where
//!   nothing needs the view any more either (the session ended), it is destroyed then, not before.
//! - **A crashed web process** is recreated at once while the view is wanted. A second crash before
//!   the next turn or the next time Kai leaves standby waits: for that, or for the GTK side's retry
//!   timer ([`Lifecycle::retry`]), whose wait doubles from one retry to the next. So a page that
//!   crashes at once cannot respawn in a loop, and an active Kai does not stay without its ring
//!   until someone speaks (review KAI-5 R5-3).
//! - **A surface the compositor closed** (its output went away: a monitor powered off or
//!   unplugged) shows again at once on an output resolved then, on the same view; an output added
//!   or removed places a ring that is up again, so one that comes back gets the ring when the
//!   output rule names it ([`Lifecycle::closed`], [`Lifecycle::outputs_changed`], review KAI-5
//!   R5-14).
//! - **No output at all** (every monitor powered off or unplugged): nothing is mapped while the
//!   compositor lists no output, and the ring shows once one is added ([`Lifecycle::no_output`],
//!   review KAI-6 R6-2).

use std::time::Duration;

/// How long the GTK side waits before retry `retries + 1` after a second crash: `first`, doubling
/// with each retry, at most `max`.
pub fn retry_wait(first: Duration, max: Duration, retries: u32) -> Duration {
    first
        .checked_mul(1u32.checked_shl(retries).unwrap_or(u32::MAX))
        .unwrap_or(max)
        .min(max)
}

/// The engine's five states (engine.js `STATE_NAMES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RingState {
    #[default]
    Idle,
    Listening,
    Thinking,
    Speaking,
    Interrupted,
}

impl RingState {
    pub fn key(self) -> &'static str {
        match self {
            RingState::Idle => "idle",
            RingState::Listening => "listening",
            RingState::Thinking => "thinking",
            RingState::Speaking => "speaking",
            RingState::Interrupted => "interrupted",
        }
    }
}

/// What the voice session tells the ring (§5.1, §9.3). WP4 sets it from the session's facts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RingInput {
    /// A voice session is bound (K5): the view exists, hidden while Kai is in standby (K1).
    pub bound: bool,
    /// Kai is active, not in standby (K11): actively listening, thinking or talking. The ring shows
    /// exactly while it is set (`Voice::standby` says when it is not); a turn is always active.
    pub active: bool,
    /// A turn runs (K11): from `speech_started` or a talk press to the end of the reply's playback.
    /// The page shows the voice state during one, the listening state out of one.
    pub turn: bool,
    /// The voice state (the `lmgw-client` machine's), shown during a turn.
    pub state: RingState,
    pub muted: bool,
    /// An alias that is loading (`lmgw.model.state`).
    pub loading: Option<String>,
    /// The GPU hold (K9).
    pub held: bool,
}

/// One `setState` for the page: the input's state during a turn, listening while Kai is active out
/// of one, idle in standby.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageState {
    pub state: RingState,
    pub muted: bool,
    pub loading: Option<String>,
    pub held: bool,
    /// Numbers the sends, so a late `settled` for an older idle is told apart.
    pub seq: u64,
}

impl PageState {
    fn same_as(&self, other: &PageState) -> bool {
        (self.state, self.muted, &self.loading, self.held)
            == (other.state, other.muted, &other.loading, other.held)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No view, no web process.
    Gone,
    /// The view is created, hidden, and loads its page.
    Loading,
    /// The page is ready; the surface is not mapped.
    Hidden,
    /// The surface is mapped and fed (Kai is active).
    Shown,
    /// Kai is in standby: still mapped and fed while the engine eases to idle.
    Settling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create the view hidden and load the page.
    Create,
    /// Destroy the window, the view and its context: no web process stays.
    Destroy,
    /// Map the surface on the output and layer chosen now, and feed levels. A mapped surface is
    /// placed again: it moves only where the output or the layer changed.
    Show,
    /// Stop feeding and unmap the surface.
    Hide,
    /// Tell the page.
    Send(PageState),
}

/// Why the ring is down while Kai is active: the tray says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Down {
    /// Its web process ended twice: no view until the next turn or the next retry.
    Crashed,
    /// The compositor closed its surface again, with no output added or removed and no turn
    /// started since it closed it last: down until one of those.
    Closed,
    /// The compositor lists no output: nothing to map until one is added.
    NoOutput,
}

/// The crashes since the last start of a turn or the last time Kai left standby (§9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crashes {
    None,
    /// One: its view was created again at once.
    Recreated,
    /// Another: no new view until the next turn, the next time Kai leaves standby, or the next
    /// retry ([`Lifecycle::retry`]).
    Waiting,
}

#[derive(Debug)]
pub struct Lifecycle {
    input: RingInput,
    phase: Phase,
    /// What the page was last told; `None` until it is ready.
    sent: Option<PageState>,
    seq: u64,
    crashes: Crashes,
    /// The retries made while waiting after a crash; the GTK side's wait doubles with each.
    retries: u32,
    /// The compositor closed the surface and it was shown again, with no output added or removed
    /// and no turn's start since: another close leaves it down (no loop with a compositor that
    /// keeps closing it).
    reshown: bool,
    /// Shown, but the compositor closed the surface and it stays down until an output is added or
    /// removed or a turn starts.
    closed: bool,
    /// Shown, but the compositor listed no output when the ring was to be mapped: nothing is
    /// mapped until an output is added (or removed: any change is looked at again).
    no_output: bool,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl Lifecycle {
    pub fn new() -> Self {
        Self {
            input: RingInput::default(),
            phase: Phase::Gone,
            sent: None,
            seq: 0,
            crashes: Crashes::None,
            retries: 0,
            reshown: false,
            closed: false,
            no_output: false,
        }
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The view is created and its page has not said `ready` yet.
    pub fn loading(&self) -> bool {
        self.phase == Phase::Loading
    }

    /// Kai is active and the ring is up (or on its way up): not easing out.
    pub fn shown(&self) -> bool {
        self.phase == Phase::Shown
    }

    /// While settling: the send whose idle state the page must report drawn.
    pub fn settling_seq(&self) -> Option<u64> {
        match (self.phase, &self.sent) {
            (Phase::Settling, Some(s)) => Some(s.seq),
            _ => None,
        }
    }

    /// The web process crashed twice: no new view until the next turn, the next time Kai leaves
    /// standby, or the next retry.
    pub fn waiting_after_crash(&self) -> bool {
        self.crashes == Crashes::Waiting
    }

    /// The retries made since the web process crashed twice (0 until the first).
    pub fn retries(&self) -> u32 {
        self.retries
    }

    /// The ring is down while Kai is active, so meant to be up; the tray says so.
    /// - After a second crash no view exists until the next turn or the next retry. A bound session
    ///   in standby (push-to-talk between presses, muted) waits for its retry too, but its ring is
    ///   meant to be hidden, so nothing is missing and the tray says nothing (review KAI-5 R5-11).
    /// - After a second close by the compositor with no output added or removed and no turn's
    ///   start since the first, the surface stays down until one of those.
    /// - While the compositor lists no output, nothing is mapped until one is added.
    pub fn down(&self) -> Option<Down> {
        if self.crashes == Crashes::Waiting && self.phase == Phase::Gone && self.input.active {
            Some(Down::Crashed)
        } else if self.closed {
            Some(Down::Closed)
        } else if self.no_output {
            Some(Down::NoOutput)
        } else {
            None
        }
    }

    /// The view is needed: a session is bound, or Kai is active.
    fn wanted(&self) -> bool {
        self.input.bound || self.input.active
    }

    pub fn input(&mut self, input: RingInput) -> Vec<Action> {
        let turn_starts = input.turn && !self.input.turn;
        let leaves_standby = input.active && !self.input.active;
        let nothing = !(input.bound || input.active);
        if turn_starts || leaves_standby || nothing {
            self.crashes = Crashes::None;
            self.retries = 0;
        }
        if turn_starts {
            self.reshown = false;
        }
        let up = matches!(self.phase, Phase::Shown | Phase::Settling);
        self.input = input;
        let mut out = Vec::new();
        self.reconcile(&mut out);
        // A turn's start on a ring that is up places it again, as a turn that maps it does; one
        // the compositor closed shows again. With no output there is nothing to place it on: it
        // waits for one to be added.
        if turn_starts && up && self.phase == Phase::Shown && !self.no_output {
            self.closed = false;
            out.push(Action::Show);
        }
        out
    }

    /// A setting of the placement changed (`overlay.ring_over_fullscreen`, review KAI-5 R5-2): a
    /// ring that is up is placed again, as a turn's start does. One that is down stays down (it
    /// waits for its own way back, review KAI-6 R6-3); a hidden one takes the setting the next
    /// time it shows.
    pub fn replace(&mut self) -> Vec<Action> {
        if self.phase == Phase::Shown && !self.closed && !self.no_output {
            vec![Action::Show]
        } else {
            Vec::new()
        }
    }

    /// The page said `ready`.
    pub fn ready(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase == Phase::Loading {
            self.phase = Phase::Hidden;
            self.reconcile(&mut out);
        }
        out
    }

    /// The page drew the idle state of send `seq` (or the GTK side stopped waiting for it). A view
    /// nothing needs any more goes with the surface.
    pub fn settled(&mut self, seq: u64) -> Vec<Action> {
        if self.settling_seq() != Some(seq) {
            return Vec::new();
        }
        if self.wanted() {
            self.phase = Phase::Hidden;
            vec![Action::Hide]
        } else {
            self.phase = Phase::Gone;
            self.sent = None;
            vec![Action::Hide, Action::Destroy]
        }
    }

    /// The web process is gone (`web-process-terminated`, or a page that never said `ready`). The
    /// view is cleaned up and, while it is wanted, created again at once; after a second crash
    /// with no turn's start and no leaving standby in between, a new one waits for the next of
    /// those or the next retry ([`Self::retry`]).
    pub fn crashed(&mut self) -> Vec<Action> {
        if self.phase == Phase::Gone {
            return Vec::new();
        }
        self.phase = Phase::Gone;
        self.sent = None;
        self.closed = false;
        self.no_output = false;
        self.crashes = match self.crashes {
            Crashes::None => Crashes::Recreated,
            Crashes::Recreated | Crashes::Waiting => Crashes::Waiting,
        };
        let mut out = vec![Action::Destroy];
        self.reconcile(&mut out);
        out
    }

    /// The compositor closed the mapped surface (the layer surface's `closed`: its output went
    /// away, a monitor powered off or unplugged), and the window is kept (§9.1, review KAI-5
    /// R5-14).
    /// - While the ring is up it shows again at once, on the same view: unmapped, then mapped on an
    ///   output resolved now (a new layer surface). Once: another close with no output added or
    ///   removed and no turn's start in between leaves it down until one of those, so a compositor
    ///   that keeps closing it gets no loop.
    /// - While it eases out there is nothing left to ease: it settles now.
    /// - A ring that is down already (closed, or no output) stays down.
    pub fn closed(&mut self) -> Vec<Action> {
        match self.phase {
            Phase::Shown if self.closed || self.no_output => Vec::new(),
            Phase::Shown if self.reshown => {
                self.closed = true;
                vec![Action::Hide]
            }
            Phase::Shown => {
                self.reshown = true;
                vec![Action::Hide, Action::Show]
            }
            Phase::Settling => match self.settling_seq() {
                Some(seq) => self.settled(seq),
                None => Vec::new(),
            },
            Phase::Gone | Phase::Loading | Phase::Hidden => Vec::new(),
        }
    }

    /// An output was added or removed (§9.1, review KAI-5 R5-14). A ring that is up is placed
    /// again: an output that came back may be the one the output rule names (`ring.output`, else
    /// KWin's active output), and the surface moves only where the output changed. One the
    /// compositor closed shows again, and one that waited for an output shows on the one resolved
    /// now (review KAI-6 R6-2). A hidden ring takes the output the next time it shows.
    pub fn outputs_changed(&mut self) -> Vec<Action> {
        self.reshown = false;
        if self.phase != Phase::Shown {
            return Vec::new();
        }
        self.closed = false;
        self.no_output = false;
        vec![Action::Show]
    }

    /// The output resolved for a show found the compositor listing no output at all: its only
    /// monitor powered off or unplugged (§9.1, review KAI-6 R6-2). Nothing is mapped, and a
    /// surface still mapped is unmapped and no longer fed: it is on no output anyone sees. The
    /// ring stays down, and the tray says so, until an output is added
    /// ([`Self::outputs_changed`]); a turn's start does not try again, since there is still no
    /// output. Kai entering standby ends it, as it ends a close.
    pub fn no_output(&mut self) -> Vec<Action> {
        if self.phase != Phase::Shown || self.no_output {
            return Vec::new();
        }
        self.no_output = true;
        self.closed = false;
        vec![Action::Hide]
    }

    /// The GTK side's retry timer fired while waiting after a second crash (§9.4).
    /// - With no view, one is created, as a crash's first recreation would; it shows once ready
    ///   while Kai is active. A crash of that view waits for the next retry again, so a page that
    ///   keeps crashing gets one new web process per retry, never a loop.
    /// - A view the last retry created that is still there counts as recovered: the next crash is
    ///   recreated at once again, and the retries start over.
    pub fn retry(&mut self) -> Vec<Action> {
        if self.crashes != Crashes::Waiting {
            return Vec::new();
        }
        match self.phase {
            Phase::Gone if self.wanted() => {
                self.retries += 1;
                self.phase = Phase::Loading;
                vec![Action::Create]
            }
            // The retried view still loads; `READY_LIMIT` tears a hung one down as a crash.
            Phase::Loading => Vec::new(),
            // Recovered, or nothing needs the ring any more: the wait ends.
            Phase::Gone | Phase::Hidden | Phase::Shown | Phase::Settling => {
                self.crashes = Crashes::None;
                self.retries = 0;
                Vec::new()
            }
        }
    }

    fn page_state(&self) -> PageState {
        let i = &self.input;
        PageState {
            state: if !i.active {
                RingState::Idle
            } else if i.turn {
                i.state
            } else {
                RingState::Listening
            },
            muted: i.muted,
            loading: i.loading.clone(),
            held: i.held,
            seq: self.seq,
        }
    }

    fn reconcile(&mut self, out: &mut Vec<Action>) {
        let wanted = self.wanted();
        match self.phase {
            Phase::Gone => {
                if wanted && self.crashes != Crashes::Waiting {
                    out.push(Action::Create);
                    self.phase = Phase::Loading;
                }
                return;
            }
            Phase::Loading | Phase::Hidden if !wanted => {
                out.push(Action::Destroy);
                self.phase = Phase::Gone;
                self.sent = None;
                return;
            }
            Phase::Loading => return,
            // Mapped, or hidden and still wanted: the page hears the state. A mapped ring nothing
            // needs any more eases to idle first; `settled` hides and destroys it.
            Phase::Hidden | Phase::Shown | Phase::Settling => {}
        }
        let want = self.page_state();
        if !self.sent.as_ref().is_some_and(|s| s.same_as(&want)) {
            self.seq += 1;
            let send = PageState {
                seq: self.seq,
                ..want
            };
            self.sent = Some(send.clone());
            out.push(Action::Send(send));
        }
        match (self.phase, self.input.active) {
            (Phase::Hidden, true) => {
                out.push(Action::Show);
                self.phase = Phase::Shown;
            }
            (Phase::Settling, true) => self.phase = Phase::Shown,
            (Phase::Shown, false) => {
                // A surface the compositor closed, or one with no output, has nothing to ease
                // out: the GTK side settles it at once, as one that was never mapped.
                self.closed = false;
                self.no_output = false;
                self.phase = Phase::Settling;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Action::*;
    use super::*;

    /// A turn is active, and nothing else is (push-to-talk, a press session).
    fn input(bound: bool, turn: bool, state: RingState) -> RingInput {
        RingInput {
            bound,
            active: turn,
            turn,
            state,
            ..RingInput::default()
        }
    }

    /// Continuous listening between turns: a bound session, active, the voice machine idle.
    fn continuous() -> RingInput {
        RingInput {
            active: true,
            ..input(true, false, RingState::Idle)
        }
    }

    /// A turn in continuous listening.
    fn turn(state: RingState) -> RingInput {
        RingInput {
            turn: true,
            state,
            ..continuous()
        }
    }

    fn sent(a: &[Action]) -> Vec<(RingState, u64)> {
        a.iter()
            .filter_map(|a| match a {
                Send(p) => Some((p.state, p.seq)),
                _ => None,
            })
            .collect()
    }

    fn kinds(a: &[Action]) -> Vec<&'static str> {
        a.iter()
            .map(|a| match a {
                Create => "create",
                Destroy => "destroy",
                Show => "show",
                Hide => "hide",
                Send(_) => "send",
            })
            .collect()
    }

    /// A bound session in standby (push-to-talk between presses, a press session), its page ready.
    fn bound_and_ready() -> Lifecycle {
        let mut l = Lifecycle::new();
        assert_eq!(
            kinds(&l.input(input(true, false, RingState::Idle))),
            ["create"]
        );
        assert_eq!(kinds(&l.ready()), ["send"]);
        assert_eq!(l.phase(), Phase::Hidden);
        l
    }

    /// Continuous listening, its page ready and the ring up in the listening state (send 1).
    fn active_and_shown() -> Lifecycle {
        let mut l = Lifecycle::new();
        assert_eq!(l.input(continuous()), [Create]);
        let a = l.ready();
        assert_eq!(kinds(&a), ["send", "show"]);
        assert_eq!(sent(&a), [(RingState::Listening, 1)]);
        assert_eq!(l.phase(), Phase::Shown);
        l
    }

    #[test]
    fn idle_with_no_session_has_no_view() {
        let mut l = Lifecycle::new();
        assert!(l.input(RingInput::default()).is_empty());
        assert_eq!(l.phase(), Phase::Gone);
    }

    #[test]
    fn continuous_listening_shows_the_ring_in_the_listening_state_between_turns() {
        let mut l = Lifecycle::new();
        let a = l.input(continuous());
        assert_eq!(a, [Create], "created at once, shown once its page is ready");
        assert_eq!(l.phase(), Phase::Loading);
        let a = l.ready();
        assert_eq!(
            kinds(&a),
            ["send", "show"],
            "the state goes first, then the map"
        );
        assert_eq!(
            sent(&a),
            [(RingState::Listening, 1)],
            "listening, not the voice machine's idle"
        );
        assert_eq!(l.phase(), Phase::Shown);
        // The hold and a loading alias reach the page; the ring stays up.
        let held = RingInput {
            held: true,
            loading: Some("gemma4-e4b".into()),
            ..continuous()
        };
        let a = l.input(held.clone());
        let Send(p) = &a[0] else { panic!("{a:?}") };
        assert_eq!(
            (p.state, p.held, p.loading.as_deref()),
            (RingState::Listening, true, Some("gemma4-e4b"))
        );
        assert_eq!(kinds(&a), ["send"]);
        assert_eq!(l.phase(), Phase::Shown);
        // The same input again sends nothing.
        assert!(l.input(held).is_empty());
    }

    #[test]
    fn a_bound_session_in_standby_shows_nothing() {
        // Push-to-talk between presses, a press session's grace, muted: bound, in standby.
        let mut l = bound_and_ready();
        for i in [
            RingInput {
                muted: true,
                ..input(true, false, RingState::Idle)
            },
            RingInput {
                held: true,
                ..input(true, false, RingState::Idle)
            },
        ] {
            let a = l.input(i);
            assert!(!a.contains(&Show), "{a:?}");
            assert_eq!(sent(&a)[0].0, RingState::Idle);
            assert_eq!(l.phase(), Phase::Hidden);
        }
    }

    #[test]
    fn a_turn_shows_the_ring_and_its_states_reach_the_page() {
        let mut l = bound_and_ready();
        let a = l.input(input(true, true, RingState::Listening));
        assert_eq!(
            kinds(&a),
            ["send", "show"],
            "the state goes first, then the map"
        );
        assert_eq!(sent(&a), [(RingState::Listening, 2)]);
        for s in [
            RingState::Thinking,
            RingState::Speaking,
            RingState::Interrupted,
            RingState::Listening,
        ] {
            let a = l.input(input(true, true, s));
            assert_eq!(kinds(&a), ["send"]);
            assert_eq!(sent(&a)[0].0, s);
            assert_eq!(l.phase(), Phase::Shown);
        }
        // The same input again sends nothing.
        assert!(l.input(input(true, true, RingState::Listening)).is_empty());
    }

    #[test]
    fn a_turn_in_continuous_listening_keeps_the_ring_up_and_ends_in_listening() {
        let mut l = active_and_shown();
        // speech_started: the turn starts; the ring is placed again, its state is the same.
        let a = l.input(turn(RingState::Listening));
        assert_eq!(kinds(&a), ["show"]);
        assert_eq!(l.phase(), Phase::Shown);
        for s in [RingState::Thinking, RingState::Speaking] {
            let a = l.input(turn(s));
            assert_eq!(kinds(&a), ["send"]);
            assert_eq!(sent(&a)[0].0, s);
        }
        // The reply's playback ended: back to listening, still up, nothing to settle.
        let a = l.input(continuous());
        assert_eq!(sent(&a), [(RingState::Listening, 4)]);
        assert_eq!(kinds(&a), ["send"], "no hide");
        assert_eq!(l.phase(), Phase::Shown);
        assert_eq!(l.settling_seq(), None);
        // A press in continuous listening is a turn whose state is listening too.
        assert_eq!(kinds(&l.input(turn(RingState::Listening))), ["show"]);
    }

    #[test]
    fn the_turn_end_into_standby_eases_to_idle_and_hides_once_the_page_drew_it() {
        let mut l = bound_and_ready();
        l.input(input(true, true, RingState::Speaking));
        // The reply's playback ended; Kai is in standby (push-to-talk).
        let a = l.input(input(true, false, RingState::Idle));
        assert_eq!(sent(&a), [(RingState::Idle, 3)]);
        assert!(!a.contains(&Hide), "not before the idle state is drawn");
        assert_eq!(l.phase(), Phase::Settling);
        assert_eq!(l.settling_seq(), Some(3));
        assert!(l.settled(2).is_empty(), "an older idle does not count");
        assert_eq!(l.settled(3), [Hide]);
        assert_eq!(l.phase(), Phase::Hidden);
        assert!(l.settled(3).is_empty(), "once");
        // The next turn shows the same view again.
        let a = l.input(input(true, true, RingState::Listening));
        assert_eq!(kinds(&a), ["send", "show"]);
        assert!(!a.contains(&Create));
    }

    #[test]
    fn push_to_talk_shows_the_ring_from_a_press_to_its_turns_end_and_nothing_between_presses() {
        // The owner's decision, 2026-10-08: with push-to-talk Kai listens only while the talk key
        // is held, so it is in standby between presses; the bound session's view waits hidden.
        let mut l = bound_and_ready();
        for press in 0..3 {
            let a = l.input(input(true, true, RingState::Listening));
            assert_eq!(
                kinds(&a),
                ["send", "show"],
                "press {press}: shown from the press"
            );
            assert!(!a.contains(&Create), "press {press}: the same view");
            for s in [RingState::Thinking, RingState::Speaking] {
                assert_eq!(kinds(&l.input(input(true, true, s))), ["send"]);
            }
            // The reply has played: eased to idle, then unmapped once the page reports it.
            let a = l.input(input(true, false, RingState::Idle));
            assert_eq!(sent(&a)[0].0, RingState::Idle);
            assert_eq!(l.phase(), Phase::Settling);
            let seq = l.settling_seq().unwrap();
            assert_eq!(l.settled(seq), [Hide]);
            // Between presses the session's facts change; nothing shows, nothing is fed.
            for i in [
                RingInput {
                    held: true,
                    ..input(true, false, RingState::Idle)
                },
                input(true, false, RingState::Idle),
            ] {
                let a = l.input(i);
                assert!(!a.contains(&Show), "press {press}: {a:?}");
                assert_eq!(l.phase(), Phase::Hidden);
            }
        }
    }

    #[test]
    fn standby_eases_to_idle_and_hides_and_leaving_it_shows_the_ring_again() {
        // Mute (K16's silence alike) puts continuous listening in standby; the session stays bound.
        let mut l = active_and_shown();
        let a = l.input(RingInput {
            muted: true,
            ..input(true, false, RingState::Idle)
        });
        assert_eq!(sent(&a), [(RingState::Idle, 2)]);
        assert_eq!(l.phase(), Phase::Settling);
        assert_eq!(l.settled(2), [Hide], "the view stays: the session is bound");
        assert_eq!(l.phase(), Phase::Hidden);
        // Unmuted: active again, on the same view.
        let a = l.input(continuous());
        assert_eq!(kinds(&a), ["send", "show"]);
        assert_eq!(sent(&a), [(RingState::Listening, 3)]);
    }

    #[test]
    fn a_turn_during_the_ease_keeps_the_ring_up() {
        let mut l = bound_and_ready();
        l.input(input(true, true, RingState::Speaking));
        l.input(input(true, false, RingState::Idle));
        let a = l.input(input(true, true, RingState::Listening));
        assert_eq!(
            kinds(&a),
            ["send", "show"],
            "placed again, as a turn's start"
        );
        assert_eq!(l.phase(), Phase::Shown);
        assert!(l.settled(3).is_empty(), "the eased idle no longer counts");
        assert_eq!(l.phase(), Phase::Shown);
    }

    #[test]
    fn a_change_while_settling_restarts_the_wait() {
        let mut l = bound_and_ready();
        l.input(input(true, true, RingState::Speaking));
        l.input(input(true, false, RingState::Idle));
        let a = l.input(RingInput {
            held: true,
            ..input(true, false, RingState::Idle)
        });
        assert_eq!(sent(&a), [(RingState::Idle, 4)]);
        assert!(l.settled(3).is_empty());
        assert_eq!(l.settled(4), [Hide]);
    }

    #[test]
    fn the_session_end_while_shown_eases_to_idle_then_destroys_the_view() {
        let mut l = active_and_shown();
        let a = l.input(RingInput::default());
        assert_eq!(sent(&a), [(RingState::Idle, 2)], "eased, not cut");
        assert!(!a.contains(&Destroy));
        assert_eq!(l.phase(), Phase::Settling);
        assert_eq!(l.settled(2), [Hide, Destroy]);
        assert_eq!(l.phase(), Phase::Gone);
        assert!(l.input(RingInput::default()).is_empty());
    }

    #[test]
    fn a_session_back_during_the_ease_keeps_the_view() {
        // A rebind whose new session came before the old one's ease was drawn.
        let mut l = active_and_shown();
        l.input(RingInput::default());
        let a = l.input(continuous());
        assert_eq!(kinds(&a), ["send"], "no new view, no second map");
        assert_eq!(sent(&a), [(RingState::Listening, 3)]);
        assert_eq!(l.phase(), Phase::Shown);
        assert!(l.settled(2).is_empty());
    }

    #[test]
    fn the_session_end_destroys_an_unmapped_view_at_once() {
        for phase_setup in 0..3 {
            let mut l = Lifecycle::new();
            l.input(input(true, false, RingState::Idle));
            if phase_setup >= 1 {
                l.ready();
            }
            if phase_setup >= 2 {
                l.input(input(true, true, RingState::Speaking));
                l.input(input(true, false, RingState::Idle));
                let seq = l.settling_seq().unwrap();
                l.settled(seq);
            }
            let a = l.input(RingInput::default());
            assert_eq!(a, [Destroy], "setup {phase_setup}");
            assert_eq!(l.phase(), Phase::Gone);
            assert!(l.input(RingInput::default()).is_empty());
        }
    }

    #[test]
    fn a_turn_without_a_view_creates_it_and_shows_it_once_ready() {
        // Hold-to-talk with listening off: the session opens at the press (K5).
        let mut l = Lifecycle::new();
        assert_eq!(l.input(input(true, true, RingState::Listening)), [Create]);
        assert!(l.input(input(true, true, RingState::Thinking)).is_empty());
        let a = l.ready();
        assert_eq!(kinds(&a), ["send", "show"]);
        assert_eq!(sent(&a), [(RingState::Thinking, 1)], "the latest state");
    }

    #[test]
    fn activity_while_a_session_is_on_its_way_has_a_view() {
        // Continuous listening while its session is on its way (the first bind, a rebind, the bind
        // after a 4004): its audio is buffered and sent, so Kai is active; the view goes with that,
        // not with the bind.
        let mut l = Lifecycle::new();
        let on_its_way = RingInput {
            active: true,
            ..RingInput::default()
        };
        assert_eq!(l.input(on_its_way.clone()), [Create]);
        assert_eq!(kinds(&l.ready()), ["send", "show"]);
        assert!(l.input(continuous()).is_empty(), "the bind changes nothing");
        assert!(l.input(on_its_way).is_empty(), "nor a rebind's gap");
        assert_eq!(l.phase(), Phase::Shown);
    }

    #[test]
    fn a_crashed_web_process_is_recreated_at_once_while_the_ring_is_wanted() {
        let mut l = active_and_shown();
        assert!(!l.waiting_after_crash());
        assert_eq!(l.crashed(), [Destroy, Create], "between turns too");
        assert_eq!(l.phase(), Phase::Loading);
        assert!(!l.waiting_after_crash());
        let a = l.ready();
        assert_eq!(kinds(&a), ["send", "show"]);
        assert_eq!(sent(&a)[0].0, RingState::Listening);
    }

    #[test]
    fn a_second_crash_waits_for_the_next_turn_or_leaving_standby() {
        let mut l = active_and_shown();
        l.crashed();
        l.ready();
        assert_eq!(l.crashed(), [Destroy], "no loop");
        assert!(l.waiting_after_crash());
        assert!(l.crashed().is_empty());
        // Kai stays active, a change of its facts: no new view.
        assert!(
            l.input(RingInput {
                held: true,
                ..continuous()
            })
            .is_empty()
        );
        assert_eq!(l.phase(), Phase::Gone);
        // The next turn recreates it, and shows it once ready.
        assert_eq!(l.input(turn(RingState::Listening)), [Create]);
        assert!(!l.waiting_after_crash());
        assert_eq!(kinds(&l.ready()), ["send", "show"]);
        // The count starts over: the next crash is recreated at once again.
        assert_eq!(l.crashed(), [Destroy, Create]);
    }

    #[test]
    fn leaving_standby_after_two_crashes_creates_the_view() {
        let mut l = active_and_shown();
        l.crashed();
        l.ready();
        l.crashed();
        // Muted, then unmuted: Kai leaves standby.
        assert!(
            l.input(RingInput {
                muted: true,
                ..input(true, false, RingState::Idle)
            })
            .is_empty()
        );
        assert_eq!(l.input(continuous()), [Create]);
    }

    /// Two crashes while active, as in continuous listening in a quiet room (no turn starts).
    fn down_after_two_crashes() -> Lifecycle {
        let mut l = active_and_shown();
        l.crashed();
        l.ready();
        assert_eq!(l.crashed(), [Destroy]);
        assert_eq!(l.down(), Some(Down::Crashed), "wanted, and no view");
        assert_eq!(l.retries(), 0);
        l
    }

    #[test]
    fn a_retry_after_a_second_crash_brings_the_ring_back_without_a_turn() {
        // Review KAI-5 R5-3: an active Kai does not stay without its ring until someone speaks.
        let mut l = down_after_two_crashes();
        assert_eq!(l.retry(), [Create]);
        assert_eq!(l.down(), None, "a view is on its way");
        assert_eq!(l.retries(), 1);
        let a = l.ready();
        assert_eq!(kinds(&a), ["send", "show"]);
        assert_eq!(sent(&a)[0].0, RingState::Listening);
        assert!(l.shown());
        // The next retry finds the view still there: recovered, the retries start over.
        assert!(l.retry().is_empty());
        assert!(!l.waiting_after_crash());
        assert_eq!(l.retries(), 0);
        assert_eq!(l.crashed(), [Destroy, Create], "recreated at once again");
    }

    #[test]
    fn a_retried_view_that_crashes_waits_for_the_next_retry() {
        let mut l = down_after_two_crashes();
        assert_eq!(l.retry(), [Create]);
        assert!(l.retry().is_empty(), "nothing while the retried view loads");
        assert_eq!(l.crashed(), [Destroy], "no new view at once: no loop");
        assert_eq!(l.down(), Some(Down::Crashed));
        assert_eq!(l.retry(), [Create]);
        assert_eq!(l.retries(), 2, "each retry counts, so the wait doubles");
        // A page that never says ready is torn down like a crash: down again.
        assert_eq!(l.crashed(), [Destroy]);
        assert_eq!(l.down(), Some(Down::Crashed));
    }

    #[test]
    fn a_turn_or_leaving_standby_ends_the_wait_and_the_retries() {
        let mut l = down_after_two_crashes();
        l.retry();
        l.crashed();
        assert_eq!(l.retries(), 1);
        assert_eq!(l.input(turn(RingState::Listening)), [Create]);
        assert!(!l.waiting_after_crash() && l.down().is_none());
        assert_eq!(l.retries(), 0);
        assert!(l.retry().is_empty(), "a late retry does nothing");
        assert_eq!(l.phase(), Phase::Loading);
    }

    #[test]
    fn a_retry_in_standby_creates_the_view_hidden() {
        let mut l = bound_and_ready();
        l.crashed();
        l.ready();
        l.crashed();
        assert!(l.waiting_after_crash());
        assert_eq!(l.down(), None, "in standby the ring is meant to be hidden");
        assert_eq!(l.retry(), [Create]);
        assert_eq!(
            kinds(&l.ready()),
            ["send"],
            "hidden, ready for the next turn"
        );
        assert!(!l.shown());
    }

    #[test]
    fn the_retry_wait_doubles_up_to_its_maximum() {
        let (first, max) = (Duration::from_secs(30), Duration::from_secs(300));
        let waits: Vec<u64> = (0..6)
            .map(|n| retry_wait(first, max, n).as_secs())
            .collect();
        assert_eq!(waits, [30, 60, 120, 240, 300, 300]);
        assert_eq!(retry_wait(first, max, 40), max, "no overflow");
        assert_eq!(retry_wait(first, max, u32::MAX), max);
    }

    #[test]
    fn the_ring_is_down_only_while_kai_is_active() {
        // Review KAI-5 R5-11: push-to-talk with listening on, two crashes during a turn.
        let mut l = bound_and_ready();
        l.input(input(true, true, RingState::Speaking));
        l.crashed();
        l.ready();
        assert_eq!(l.crashed(), [Destroy]);
        assert_eq!(
            l.down(),
            Some(Down::Crashed),
            "the turn runs: its ring is missing"
        );
        // The turn ends: standby between presses, where the ring is meant to be hidden.
        assert!(l.input(input(true, false, RingState::Idle)).is_empty());
        assert!(l.waiting_after_crash(), "the retry still comes");
        assert_eq!(l.down(), None, "nothing the owner can see is missing");
        // Muted continuous listening is standby too.
        l.input(RingInput {
            muted: true,
            ..input(true, false, RingState::Idle)
        });
        assert_eq!(l.down(), None);
        // A retry in standby makes a hidden view; one that crashes again is still not down.
        assert_eq!(l.retry(), [Create]);
        assert_eq!(l.crashed(), [Destroy]);
        assert_eq!(l.down(), None);
        // Leaving standby creates the view at once, so the ring is never down then either.
        assert_eq!(l.input(continuous()), [Create]);
        assert_eq!(l.down(), None);
    }

    #[test]
    fn nothing_wanted_is_not_down_and_a_retry_creates_nothing() {
        let mut l = down_after_two_crashes();
        assert!(l.input(RingInput::default()).is_empty());
        assert_eq!(l.down(), None);
        assert!(l.retry().is_empty());
        assert_eq!(l.phase(), Phase::Gone);
    }

    #[test]
    fn a_crash_in_standby_is_recreated_hidden() {
        let mut l = bound_and_ready();
        assert_eq!(l.crashed(), [Destroy, Create]);
        assert_eq!(
            kinds(&l.ready()),
            ["send"],
            "hidden, ready for the next turn"
        );
        assert_eq!(l.crashed(), [Destroy]);
        assert!(l.input(input(true, false, RingState::Idle)).is_empty());
        assert_eq!(l.input(input(true, true, RingState::Listening)), [Create]);
    }

    #[test]
    fn a_crash_while_easing_out_of_a_session_creates_nothing() {
        let mut l = active_and_shown();
        l.input(RingInput::default());
        assert_eq!(l.crashed(), [Destroy]);
        assert!(!l.waiting_after_crash());
        assert_eq!(l.phase(), Phase::Gone);
    }

    #[test]
    fn a_page_that_never_says_ready_is_torn_down_like_a_crash() {
        // The GTK side's READY_LIMIT passed while loading: it reports the hang as a crash.
        let mut l = Lifecycle::new();
        l.input(continuous());
        assert!(l.loading());
        assert_eq!(l.crashed(), [Destroy, Create], "one more view at once");
        assert!(l.loading());
        assert_eq!(l.crashed(), [Destroy]);
        assert!(!l.loading());
        // Kai stays active with no view; the next turn creates a fresh one.
        assert!(l.input(continuous()).is_empty());
        assert_eq!(l.input(turn(RingState::Listening)), [Create]);
        assert!(l.loading());
    }

    #[test]
    fn a_new_session_after_a_crash_pre_creates_again() {
        let mut l = bound_and_ready();
        l.crashed();
        l.ready();
        l.crashed();
        assert!(l.input(RingInput::default()).is_empty());
        assert!(
            !l.waiting_after_crash(),
            "the session's end starts the count over"
        );
        assert_eq!(l.input(input(true, false, RingState::Idle)), [Create]);
    }

    #[test]
    fn a_surface_the_compositor_closed_shows_again_at_once_on_the_same_view() {
        // Review KAI-5 R5-14: the ring's output went away (a monitor powered off) while it was up.
        let mut l = active_and_shown();
        assert_eq!(
            l.closed(),
            [Hide, Show],
            "unmapped, then mapped on an output resolved now"
        );
        assert_eq!(l.phase(), Phase::Shown);
        assert_eq!(l.down(), None);
        // The page goes on as it was: no new view, nothing new to send.
        assert!(l.input(continuous()).is_empty());
    }

    #[test]
    fn a_second_close_with_no_change_of_the_outputs_leaves_the_ring_down_until_they_change() {
        let mut l = active_and_shown();
        assert_eq!(l.closed(), [Hide, Show]);
        assert_eq!(l.closed(), [Hide], "no loop: down");
        assert_eq!(l.down(), Some(Down::Closed), "the tray says so");
        assert!(l.closed().is_empty());
        // Kai's facts change; the ring stays down.
        assert!(
            l.input(RingInput {
                held: true,
                ..continuous()
            })
            .iter()
            .all(|a| matches!(a, Send(_)))
        );
        assert_eq!(l.down(), Some(Down::Closed));
        // An output comes (back): the ring shows again, and a close is answered again.
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
        assert_eq!(l.closed(), [Hide, Show]);
    }

    #[test]
    fn a_turns_start_shows_a_closed_ring_again() {
        let mut l = active_and_shown();
        l.closed();
        l.closed();
        assert_eq!(l.down(), Some(Down::Closed));
        let a = l.input(turn(RingState::Listening));
        assert_eq!(kinds(&a), ["show"]);
        assert_eq!(l.down(), None);
        assert_eq!(
            l.closed(),
            [Hide, Show],
            "the turn's start counts as a new try"
        );
    }

    #[test]
    fn a_change_of_the_outputs_places_only_a_ring_that_is_up() {
        let mut l = active_and_shown();
        assert_eq!(
            l.outputs_changed(),
            [Show],
            "placed again: it moves only where it changed"
        );
        assert_eq!(l.phase(), Phase::Shown);
        let mut l = bound_and_ready();
        assert!(
            l.outputs_changed().is_empty(),
            "hidden: the next show resolves"
        );
        let mut l = Lifecycle::new();
        assert!(l.outputs_changed().is_empty());
        let mut l = active_and_shown();
        l.input(input(true, false, RingState::Idle));
        assert_eq!(l.phase(), Phase::Settling);
        assert!(
            l.outputs_changed().is_empty(),
            "easing out: not placed again"
        );
    }

    #[test]
    fn a_close_while_easing_out_settles_at_once() {
        // Push-to-talk's turn end, the view kept.
        let mut l = active_and_shown();
        l.input(input(true, false, RingState::Idle));
        assert_eq!(l.closed(), [Hide]);
        assert_eq!(l.phase(), Phase::Hidden);
        // The session's end, the view goes.
        let mut l = active_and_shown();
        l.input(RingInput::default());
        assert_eq!(l.closed(), [Hide, Destroy]);
        assert_eq!(l.phase(), Phase::Gone);
        // A late close for a surface that is not mapped changes nothing.
        let mut l = bound_and_ready();
        assert!(l.closed().is_empty());
        assert_eq!(l.phase(), Phase::Hidden);
    }

    #[test]
    fn a_ring_down_after_two_closes_enters_standby_with_nothing_to_ease() {
        let mut l = active_and_shown();
        l.closed();
        l.closed();
        let a = l.input(RingInput {
            muted: true,
            ..input(true, false, RingState::Idle)
        });
        assert_eq!(sent(&a)[0].0, RingState::Idle);
        assert_eq!(l.phase(), Phase::Settling);
        assert_eq!(l.down(), None, "standby: the ring is meant to be hidden");
        let seq = l.settling_seq().unwrap();
        assert_eq!(l.settled(seq), [Hide]);
        // Unmuted: shown again on the same view.
        assert_eq!(kinds(&l.input(continuous())), ["send", "show"]);
    }

    #[test]
    fn closes_answered_between_output_changes_end_with_the_ring_up() {
        // Review KAI-6 R6-2: each output change answers a close again, whatever the order.
        let mut l = active_and_shown();
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.closed(), [Hide, Show]);
        assert_eq!(l.closed(), [Hide]);
        assert_eq!(l.down(), Some(Down::Closed));
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
        let mut l = active_and_shown();
        assert_eq!(l.closed(), [Hide, Show]);
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(
            l.closed(),
            [Hide, Show],
            "the output change counts as a new try"
        );
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
    }

    /// The only output went away under a ring that is up, the removal first and then the close
    /// (the order KWin sent them in the nested check), and the show resolved no output.
    fn no_output_after_removal_then_close() -> Lifecycle {
        let mut l = active_and_shown();
        assert_eq!(l.outputs_changed(), [Show], "placed again: resolve now");
        assert_eq!(l.closed(), [Hide, Show]);
        assert_eq!(l.no_output(), [Hide], "nothing to map on");
        assert_eq!(l.down(), Some(Down::NoOutput), "the tray says so");
        assert_eq!(l.phase(), Phase::Shown, "still meant to be up");
        l
    }

    #[test]
    fn the_only_output_going_away_leaves_the_ring_unmapped_until_one_is_added() {
        // Review KAI-6 R6-2: a single monitor powered off overnight.
        let mut l = no_output_after_removal_then_close();
        // A late close, or a second resolution that found nothing, changes nothing.
        assert!(l.closed().is_empty());
        assert!(l.no_output().is_empty());
        assert_eq!(l.down(), Some(Down::NoOutput));
        // Kai's facts change in the night: the page hears them, nothing maps.
        let a = l.input(RingInput {
            held: true,
            ..continuous()
        });
        assert_eq!(kinds(&a), ["send"]);
        // The monitor is back: the ring shows on the output resolved then.
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
        // A close is answered again at once: the output change counts as a new try.
        assert_eq!(l.closed(), [Hide, Show]);
    }

    #[test]
    fn a_close_before_the_removal_ends_the_same_way() {
        let mut l = active_and_shown();
        assert_eq!(l.closed(), [Hide, Show]);
        assert_eq!(
            l.outputs_changed(),
            [Show],
            "resolve again after the removal"
        );
        assert_eq!(l.no_output(), [Hide]);
        assert_eq!(l.down(), Some(Down::NoOutput));
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
    }

    #[test]
    fn a_removal_while_there_is_no_output_looks_again() {
        // Any change of the outputs is looked at: one that found none again stays down.
        let mut l = no_output_after_removal_then_close();
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None, "a show on its way");
        assert_eq!(l.no_output(), [Hide]);
        assert_eq!(l.down(), Some(Down::NoOutput));
    }

    #[test]
    fn a_turn_while_there_is_no_output_maps_nothing() {
        let mut l = no_output_after_removal_then_close();
        let a = l.input(turn(RingState::Listening));
        assert!(!a.contains(&Show), "nothing to place it on: {a:?}");
        let a = l.input(turn(RingState::Speaking));
        assert_eq!(kinds(&a), ["send"], "the page still hears the turn");
        assert_eq!(l.down(), Some(Down::NoOutput));
        // The turn's end, back to listening: still down.
        assert_eq!(kinds(&l.input(continuous())), ["send"]);
        assert_eq!(l.down(), Some(Down::NoOutput));
        assert_eq!(l.outputs_changed(), [Show]);
        assert_eq!(l.down(), None);
    }

    #[test]
    fn standby_while_there_is_no_output_has_nothing_to_ease_and_leaving_it_looks_again() {
        let mut l = no_output_after_removal_then_close();
        let a = l.input(RingInput {
            muted: true,
            ..input(true, false, RingState::Idle)
        });
        assert_eq!(sent(&a)[0].0, RingState::Idle);
        assert_eq!(l.phase(), Phase::Settling);
        assert_eq!(l.down(), None, "standby: the ring is meant to be hidden");
        // The GTK side settles an unmapped surface at once.
        let seq = l.settling_seq().unwrap();
        assert_eq!(l.settled(seq), [Hide]);
        // Unmuted: a show, which resolves the output again.
        assert_eq!(kinds(&l.input(continuous())), ["send", "show"]);
        assert_eq!(l.no_output(), [Hide], "still none");
        assert_eq!(l.down(), Some(Down::NoOutput));
    }

    #[test]
    fn a_crash_while_there_is_no_output_starts_over() {
        let mut l = no_output_after_removal_then_close();
        assert_eq!(l.crashed(), [Destroy, Create]);
        assert_eq!(l.down(), None, "a view is on its way");
        assert_eq!(kinds(&l.ready()), ["send", "show"]);
        assert_eq!(l.no_output(), [Hide]);
        assert_eq!(l.down(), Some(Down::NoOutput));
    }

    #[test]
    fn no_output_for_a_ring_that_is_not_up_changes_nothing() {
        let mut l = bound_and_ready();
        assert!(l.no_output().is_empty());
        assert_eq!(l.down(), None);
        let mut l = active_and_shown();
        l.input(input(true, false, RingState::Idle));
        assert_eq!(l.phase(), Phase::Settling);
        assert!(l.no_output().is_empty(), "easing out");
        assert_eq!(l.down(), None);
    }

    #[test]
    fn a_placement_setting_places_only_a_ring_that_is_up() {
        // Review KAI-6 R6-3: "ring over fullscreen" toggled.
        let mut l = active_and_shown();
        assert_eq!(l.replace(), [Show]);
        l.closed();
        assert_eq!(l.replace(), [Show], "shown again after the first close: up");
        l.closed();
        assert_eq!(l.down(), Some(Down::Closed));
        assert!(
            l.replace().is_empty(),
            "down after two closes: it stays down"
        );
        assert_eq!(l.down(), Some(Down::Closed));
        let mut l = no_output_after_removal_then_close();
        assert!(l.replace().is_empty(), "no output: nothing to place");
        let mut l = bound_and_ready();
        assert!(l.replace().is_empty(), "hidden: the next show takes it");
        let mut l = active_and_shown();
        l.input(input(true, false, RingState::Idle));
        assert!(l.replace().is_empty(), "easing out");
    }

    #[test]
    fn the_page_hears_muted_loading_and_held() {
        let mut l = bound_and_ready();
        let a = l.input(RingInput {
            muted: true,
            loading: Some("gemma4-e4b".into()),
            held: true,
            ..input(true, true, RingState::Thinking)
        });
        let Send(p) = &a[0] else { panic!("{a:?}") };
        assert_eq!(
            (p.state, p.muted, p.loading.as_deref(), p.held),
            (RingState::Thinking, true, Some("gemma4-e4b"), true)
        );
    }

    #[test]
    fn a_page_that_is_not_ready_is_told_nothing() {
        let mut l = Lifecycle::new();
        l.input(continuous());
        assert!(l.settled(0).is_empty());
        assert!(l.input(turn(RingState::Speaking)).is_empty());
        assert_eq!(l.phase(), Phase::Loading);
    }
}
