//! Kai speaks without a model (kai-firefox design §7.2, §7.3): Kai for Firefox's announcements and
//! reading aloud, spoken through lmgw's text-to-speech (`POST /v1/audio/speech`, through
//! [`VoiceLink::speech`]) and Kai's own playback, with no model turn and nothing written into a
//! thread. The browser module hands each sentence or paragraph over as an [`Utterance`]
//! ([`Mouth`]: the core's `Command::Speak`), and hears back on its `reply` when it waits, started
//! and how it ended ([`Said`]).
//!
//! - **The voice.** An utterance names its voice (reading aloud's voice for a page's language,
//!   `browser.read.voices`), or speaks in Kai's own: the text-to-speech alias, the voice, the
//!   speech style and the seed lmgw resolves for Kai's conversation thread ([`VoiceLink::kai_voice`],
//!   `GET /chat/api/threads/{id}`'s `voice_resolved`), with the language of Kai's own words. It is
//!   asked once per run of speech: the first utterance after a quiet time asks again, so a voice
//!   changed on the dashboard is heard from the next run on.
//! - **Kai's words** ([`Text::Kai`]) come in English and German; the speech says them in Kai's
//!   reply language (the thread's `reply_language`) where that is German, else in English (a draft
//!   decision, Kai's design WP24). Page text ([`Text::Page`]) is said as it is.
//! - **One request per sentence** ([`sentences`]), in the order things are played: Kai's notices
//!   and urgent announcements (an `assertive` live region), then the others, then the paragraph
//!   being read; while one plays the next is made, so a reading has no gap but the request's, and
//!   `browser.speech_timeout_s` bounds each sentence.
//! - **When it is made and plays** ([`Voice::speak_blocked`]): only at the quiet moment Kai's
//!   jobs wait for (`super::jobs`' [`Voice::not_quiet`]: not locked, not muted, the feed up, no
//!   talk key held, no approval waiting, no other client's turn in Kai's conversation (K16), the
//!   session quiet or none on its way), with PipeWire up and do not disturb off, and only after
//!   `browser.speech_quiet_ms` of all that, so lmgw's quiet points between a turn's tool rounds do
//!   not start one that the next round's reply then takes back. Nothing is made before. What waits
//!   is told why ([`Said::Waiting`]).
//! - **Checked again when it is made and when it plays** ([`Utterance::gate`], the sender's): an
//!   announcement switched off, of a site that became `none`, or older than
//!   `browser.announce_max_age_s` is dropped ([`Dropped`]), and so is one whose Firefox does not
//!   have the focused window while conversation mode is off.
//! - **Kai's own speech wins** ([`Voice::speak_yield`], at `response.created`): what plays stops
//!   (the session opens its own playback stream, which replaces Kai's) and goes back to wait,
//!   whole, so a paragraph is heard again from its start after Kai's reply, and an announcement
//!   after it. Nothing of it is made again.
//! - **The owner talking** ([`Voice::speak_cut`]): `speech_started`, or a talk press. What plays
//!   stops, and the reading's paragraphs are cut wherever they are (`Spoken::Cut`): the reading
//!   pauses until `read_resume` (design §7.3, kai §5.5). A talk press that stops Kai's speech here
//!   spends the gesture, as one over a reply does (K6).
//! - **Its echo is never the owner's turn** (a design decision, Kai's design WP24): lmgw does not
//!   know this speech, so in continuous listening the microphone may carry it back. In half duplex
//!   the session hears silence while it plays. After it stops (its end, a cut, Kai's reply) the
//!   session hears silence for `browser.speech_echo_guard_ms`, so its echo's tail cannot start a
//!   turn. A `speech_started` while it plays or within that guard is a barge-in that is spent on
//!   the cut, as a press is (K6): Kai sends `input_audio_buffer.clear`, so only what the owner says
//!   after the guard forms a turn (`super::events`).
//! - **Kai's own questions** ([`Kind::Question`], `crate::approvals::asked`, kai-firefox WP10b)
//!   are spoken while the call that asks waits, so not at the quiet moment (the session's response
//!   waits for that call): first in line, made and played once nothing else is heard (Kai's reply,
//!   the talk key held, an answer being recorded); with do not disturb on or PipeWire down they are
//!   not spoken and end at once, as their dialog shows them anyway. The rest waits meanwhile.
//! - **The screen lock** cuts what plays, and every announcement waiting or arriving while locked
//!   is dropped ([`Dropped::Locked`]); a reading is cut (paused).
//! - **Failures** (the voice, the speech, the WAV) end that utterance with `Spoken::Failed`
//!   ([`Failure`], lmgw's status when it refused), its words logged once per distinct message at
//!   `warn` and counted after (`debug`); the browser module says what follows (a reading pauses or
//!   skips a paragraph with a spoken notice). A notice that cannot be spoken (its speech failed,
//!   PipeWire is down, do not disturb is on) becomes a desktop notification instead
//!   ([`Spoken::Notified`]), so a dead text-to-speech model cannot start a loop and is never
//!   silent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::link::{Pending, Refusal};
use super::{Voice, VoiceLink};
use crate::audio::{AudioPort, Player};

/// The voice a speech is made in, as `/v1/audio/speech` takes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpeechVoice {
    /// lmgw's text-to-speech alias.
    pub model: String,
    pub voice: Option<String>,
    /// The speech style (`instructions`).
    pub instructions: Option<String>,
    /// The thread's seed, sent only where lmgw's own speech would send it (a design decision,
    /// `crate::lmgw_link::realtime::read_kai_voice`).
    pub seed: Option<u32>,
}

/// The language of Kai's own words (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    De,
}

impl Lang {
    /// Kai's reply language (an ISO 639-1 code, a BCP 47 tag too): German for German, English
    /// for every other and for none.
    pub fn of(code: Option<&str>) -> Self {
        let primary = code
            .and_then(|c| c.trim().split(['-', '_']).next())
            .map(str::to_ascii_lowercase);
        match primary.as_deref() {
            Some("de") => Lang::De,
            _ => Lang::En,
        }
    }
}

/// Kai's own voice and the language of its own words ([`VoiceLink::kai_voice`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KaiVoice {
    pub voice: SpeechVoice,
    pub lang: Lang,
}

/// Kai's own words, in each language it says them in (module doc).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Words {
    pub en: String,
    pub de: String,
}

impl Words {
    pub fn new(en: impl Into<String>, de: impl Into<String>) -> Self {
        Self {
            en: en.into(),
            de: de.into(),
        }
    }

    pub fn of(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.en,
            Lang::De => &self.de,
        }
    }
}

/// What an utterance says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    /// The page's text, said as it is (a paragraph read aloud).
    Page(String),
    /// Kai's own sentence, page text inside its framing (an announcement, a notice).
    Kai(Words),
}

impl Text {
    pub fn of(&self, lang: Lang) -> &str {
        match self {
            Text::Page(t) => t,
            Text::Kai(w) => w.of(lang),
        }
    }

    /// The English words (the log's, the tests').
    pub fn en(&self) -> &str {
        self.of(Lang::En)
    }
}

/// What an utterance is, which decides its place and its gate (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Kai's own words about its speech ("Reading stopped: the page changed"): first; one that
    /// cannot be spoken becomes a desktop notification.
    Notice,
    /// An announcement; `urgent` (an assertive live region) goes before the others.
    Announcement { urgent: bool },
    /// A paragraph read aloud: after the announcements, no focus gate.
    Reading,
    /// One of Kai's own questions (module doc): first, while its call waits.
    Question,
}

impl Kind {
    fn rank(self) -> u8 {
        match self {
            Kind::Question | Kind::Notice | Kind::Announcement { urgent: true } => 0,
            Kind::Announcement { urgent: false } => 1,
            Kind::Reading => 2,
        }
    }
}

/// The sender's check, run when the utterance is made and when it plays (module doc): `Ok` with
/// whether its Firefox has the focused window now, or why it is dropped.
pub type Gate = Arc<dyn Fn() -> Result<bool, Dropped> + Send + Sync>;

/// Where an utterance's [`Said`]s go: its tag and what became of it.
pub type Replies = mpsc::UnboundedSender<(u64, Said)>;

/// One thing to say (module doc).
pub struct Utterance {
    pub kind: Kind,
    pub text: Text,
    /// `None`: Kai's own voice.
    pub voice: Option<SpeechVoice>,
    /// `None`: nothing to check again (a notice); an announcement's says Firefox's focus.
    pub gate: Option<Gate>,
    /// Cancelled: dropped where it waits, stopped where it plays (`Dropped::Cancelled`).
    pub cancel: CancellationToken,
    /// The sender's number for it, on every [`Said`].
    pub tag: u64,
    pub reply: Option<Replies>,
}

impl std::fmt::Debug for Utterance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Utterance")
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("voice", &self.voice)
            .field("gate", &self.gate.is_some())
            .field("tag", &self.tag)
            .finish_non_exhaustive()
    }
}

/// What became of an utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// It cannot be made or played now, and why (said again when the reason changes).
    Waiting(String),
    /// Its audio began to play (again, after Kai's own speech took the playback meanwhile).
    Started,
    Done(Spoken),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spoken {
    /// Played to its end.
    Heard,
    /// The owner talked over it, or the screen locked: why.
    Cut(String),
    /// Not spoken, and why.
    Dropped(Dropped),
    /// Its speech could not be made or played.
    Failed(Failure),
    /// A notice that could not be spoken went out as a desktop notification instead.
    Notified,
}

/// Why an utterance was not spoken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// Its sender cancelled it (a newer one replaced it, a pause, a skip).
    Cancelled,
    /// The screen was locked (K32).
    Locked,
    /// Firefox did not have the focused window and conversation mode was off.
    NotFocused,
    /// Older than `browser.announce_max_age_s` (this many seconds) when its turn came.
    Stale(u64),
    /// Announcements were switched off.
    Off,
    /// Its site is one Kai does not reach now.
    Site,
    /// Kai quit.
    Quit,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dropped::Cancelled => f.write_str("cancelled"),
            Dropped::Locked => f.write_str("the screen was locked"),
            Dropped::NotFocused => {
                f.write_str("Firefox did not have the focused window and conversation mode is off")
            }
            Dropped::Stale(s) => write!(
                f,
                "older than {s} s when its turn came (browser.announce_max_age_s)"
            ),
            Dropped::Off => f.write_str("announcements were switched off"),
            Dropped::Site => f.write_str("its site is one Kai does not reach now"),
            Dropped::Quit => f.write_str("Kai quit"),
        }
    }
}

/// A speech that could not be made or played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// lmgw's HTTP status when it refused the speech; `None`: no answer, a timeout, a voice that
    /// could not be read, audio that did not play.
    pub status: Option<u16>,
    pub why: String,
}

impl Failure {
    /// lmgw refused this one request (a 4xx): another text may still be spoken.
    pub fn refused(&self) -> bool {
        self.status.is_some_and(|s| (400..500).contains(&s))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

/// Where the browser module hands its utterances (the core's `Command::Speak`; the tests' fakes).
pub trait Mouth: Send + Sync {
    /// Says `u` when its time comes. A mouth that is gone drops it (its `reply` ends).
    fn say(&self, u: Utterance);
}

impl Mouth for mpsc::UnboundedSender<crate::core::Command> {
    fn say(&self, u: Utterance) {
        let _ = self.send(crate::core::Command::Speak(Box::new(u)));
    }
}

/// `text` as the sentences it is spoken in, one request each (module doc): a sentence ends after
/// `.`, `!`, `?`, `…` or their full-width forms followed by white space, and at a line break. A
/// sentence without such an end stays whole, however long.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            push_sentence(&mut out, &mut cur);
            continue;
        }
        cur.push(c);
        let ends = matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？');
        if ends && chars.peek().is_none_or(|n| n.is_whitespace()) {
            // Closing quotes and brackets stay with their sentence.
            push_sentence(&mut out, &mut cur);
        }
    }
    push_sentence(&mut out, &mut cur);
    out
}

fn push_sentence(out: &mut Vec<String>, cur: &mut String) {
    let s = cur.trim();
    if !s.is_empty() {
        out.push(s.to_owned());
    }
    cur.clear();
}

/// What the speaker woke for.
pub enum SpeakWake {
    /// A sentence of the item with this id was made, or not.
    Made(u64, Result<Vec<i16>, Refusal>),
    /// Kai's voice for this thread, or why not.
    Voice(i64, Result<KaiVoice, Refusal>),
}

impl std::fmt::Debug for SpeakWake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpeakWake::Made(id, r) => write!(f, "Made({id}, {:?})", r.as_ref().map(Vec::len)),
            SpeakWake::Voice(t, r) => write!(f, "Voice({t}, {r:?})"),
        }
    }
}

struct Item {
    id: u64,
    u: Utterance,
    /// Its sentences in the language picked when its first was made; empty until then.
    sentences: Vec<String>,
    /// Their speech, in order, as made.
    made: Vec<Vec<i16>>,
    /// The sentences in the player now (while it plays).
    pushed: usize,
    /// Times it began to play (the player's item name).
    plays: u64,
    /// The reason it was last told it waits.
    told: Option<String>,
}

impl Item {
    fn reply(&self, said: Said) {
        if let Some(r) = &self.u.reply {
            let _ = r.send((self.u.tag, said));
        }
    }

    fn name(&self) -> String {
        format!("kai_speech_{}_{}", self.id, self.plays)
    }

    fn all_made(&self) -> bool {
        !self.sentences.is_empty() && self.made.len() == self.sentences.len()
    }
}

/// A sentence on its way: 24 kHz mono PCM16, or lmgw's refusal.
type Made = Pending<Result<Vec<i16>, Refusal>>;

/// The voice's part (module doc).
pub(super) struct Speaker {
    /// In the order they play.
    items: Vec<Item>,
    next_id: u64,
    /// The sentence on its way, by its item's id.
    making: Option<(u64, Made)>,
    /// Kai's voice on its way, by its thread.
    asking: Option<(i64, Pending<Result<KaiVoice, Refusal>>)>,
    /// Kai's voice for this run of speech, by its thread (module doc).
    kai_voice: Option<(i64, KaiVoice)>,
    /// The item whose audio is in the player.
    playing: Option<u64>,
    player: Option<Player>,
    /// Failures by their words: the first is logged at `warn`, the rest counted.
    failures: HashMap<String, u64>,
    /// Since when the quiet moment holds (`browser.speech_quiet_ms`).
    quiet_since: Option<Instant>,
    /// The session hears silence until then: the echo's tail (`browser.speech_echo_guard_ms`).
    echo_until: Option<Instant>,
    quiet: Duration,
    echo_guard: Duration,
}

impl Speaker {
    /// `quiet`: `browser.speech_quiet_ms`; `echo_guard`: `browser.speech_echo_guard_ms`.
    pub(super) fn new(quiet: Duration, echo_guard: Duration) -> Self {
        Self {
            items: Vec::new(),
            next_id: 0,
            making: None,
            asking: None,
            kai_voice: None,
            playing: None,
            player: None,
            failures: HashMap::new(),
            quiet_since: None,
            echo_until: None,
            quiet,
            echo_guard,
        }
    }

    /// Anything to make, to play or playing: the voice's tick runs.
    pub(super) fn busy(&self) -> bool {
        !self.items.is_empty() || self.player.is_some() || self.asking.is_some()
    }

    /// Its audio plays now.
    pub(super) fn sounding(&self) -> bool {
        self.playing.is_some()
    }

    /// The session hears silence now (module doc): this speech plays and `half_duplex`, or its
    /// echo's guard runs.
    pub(super) fn mutes_mic(&self, half_duplex: bool, now: Instant) -> bool {
        (half_duplex && self.sounding()) || self.echo_until.is_some_and(|t| now < t)
    }

    /// Speech heard now may be this speech's echo (module doc): it plays, or its guard runs.
    pub(super) fn echo_risk(&self, now: Instant) -> bool {
        self.sounding() || self.echo_until.is_some_and(|t| now < t)
    }

    /// The guard starts again from `now` (module doc).
    pub(super) fn guard_echo(&mut self, now: Instant) {
        self.echo_until = Some(now + self.echo_guard);
    }

    /// The next made sentence or voice (cancel-safe: the futures stay stored).
    pub(super) async fn next(&mut self) -> SpeakWake {
        let Self { making, asking, .. } = self;
        enum Got {
            Made(Result<Vec<i16>, Refusal>),
            Voice(Result<KaiVoice, Refusal>),
        }
        let got = tokio::select! {
            r = async {
                match making.as_mut() {
                    Some((_, f)) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            } => Got::Made(r),
            r = async {
                match asking.as_mut() {
                    Some((_, f)) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            } => Got::Voice(r),
        };
        match got {
            Got::Made(r) => SpeakWake::Made(self.making.take().map_or(0, |(id, _)| id), r),
            Got::Voice(r) => SpeakWake::Voice(self.asking.take().map_or(0, |(t, _)| t), r),
        }
    }

    /// Ends item `id` with `spoken` (its audio stopped first where it plays). Whether it was
    /// there.
    fn end(&mut self, id: u64, spoken: Spoken, now: Instant) -> Option<Item> {
        if self.playing == Some(id) {
            self.playing = None;
            self.guard_echo(now);
            if let Some(p) = self.player.as_mut() {
                p.flush();
            }
        }
        if self.making.as_ref().is_some_and(|(m, _)| *m == id) {
            self.making = None;
        }
        let i = self.items.iter().position(|i| i.id == id)?;
        let item = self.items.remove(i);
        if let Some(p) = self.player.as_mut() {
            p.forget(&item.name());
        }
        item.reply(Said::Done(spoken));
        Some(item)
    }

    /// A failure's words, logged once per distinct message (module doc).
    fn log_failure(&mut self, why: &str) {
        let n = self.failures.entry(why.to_owned()).or_default();
        *n += 1;
        if *n == 1 {
            tracing::warn!("speech: {why} (logged once; repeats are counted at debug)");
        } else {
            tracing::debug!(count = *n, "speech: {why}");
        }
    }
}

impl<L: VoiceLink> Voice<L> {
    /// An utterance of the browser module's (`Command::Speak`): in its place, made and played
    /// when its time comes (module doc). While the screen is locked it is dropped at once.
    pub fn speak(&mut self, u: Utterance) {
        let now = Instant::now();
        let sp = &mut self.speak;
        sp.next_id += 1;
        let item = Item {
            id: sp.next_id,
            u,
            sentences: Vec::new(),
            made: Vec::new(),
            pushed: 0,
            plays: 0,
            told: None,
        };
        if self.locked {
            tracing::debug!("speech: the screen is locked; dropped");
            item.reply(Said::Done(Spoken::Dropped(Dropped::Locked)));
            return;
        }
        let rank = item.u.kind.rank();
        let at = sp
            .items
            .iter()
            .position(|i| i.u.kind.rank() > rank)
            .unwrap_or(sp.items.len());
        sp.items.insert(at, item);
        self.speak_sweep(now);
        self.ensure_tick();
    }

    /// Why nothing of Kai's own speech may be made or played now (module doc), or `None`. The
    /// quiet moment's hold-off counts from the first call that found it.
    fn speak_blocked(&mut self, now: Instant) -> Option<String> {
        if !self.audio_up {
            self.speak.quiet_since = None;
            return Some("PipeWire is not reachable".into());
        }
        if self.results_dnd() {
            self.speak.quiet_since = None;
            return Some("do not disturb is on".into());
        }
        let thread = self
            .session
            .as_ref()
            .map(|s| s.thread.id)
            .or(self.thread_id());
        if let Some(why) = self.not_quiet(thread) {
            self.speak.quiet_since = None;
            return Some(why.into());
        }
        let since = *self.speak.quiet_since.get_or_insert(now);
        (now.saturating_duration_since(since) < self.speak.quiet).then(|| QUIET_HOLD.to_owned())
    }

    /// Why one of Kai's own questions may not be made or played now (module doc), or `None`.
    fn question_blocked(&self) -> Option<String> {
        if !self.audio_up {
            return Some("PipeWire is not reachable".into());
        }
        if self.results_dnd() {
            return Some("do not disturb is on".into());
        }
        if self.locked {
            return Some("the screen is locked".into());
        }
        if self.key.is_some() {
            return Some("the talk key is held".into());
        }
        if self.approvals.recording() {
            return Some("an answer is being recorded".into());
        }
        self.session
            .as_ref()
            .and_then(|s| s.machine.playing())
            .map(|_| "Kai's reply is playing".into())
    }

    /// Drops what was cancelled, and what its sender's gate drops now (switched off, a `none`
    /// site, too old), wherever it waits.
    fn speak_sweep(&mut self, now: Instant) {
        let mut gone: Vec<(u64, Dropped)> = Vec::new();
        for i in &self.speak.items {
            if i.u.cancel.is_cancelled() {
                gone.push((i.id, Dropped::Cancelled));
            } else if Some(i.id) != self.speak.playing
                && let Some(Err(d)) = i.u.gate.as_ref().map(|g| g())
            {
                gone.push((i.id, d));
            }
        }
        for (id, d) in gone {
            self.speak.end(id, Spoken::Dropped(d), now);
        }
    }

    /// The gate of an item about to be made or to play (module doc): `Some` drops it.
    fn speak_gate(&self, i: &Item) -> Option<Dropped> {
        match i.u.gate.as_ref().map(|g| g()) {
            Some(Err(d)) => Some(d),
            Some(Ok(focused)) => (matches!(i.u.kind, Kind::Announcement { .. })
                && !focused
                && !self.conversation_on())
            .then_some(Dropped::NotFocused),
            None => None,
        }
    }

    /// The tick's part: what was cancelled or gated goes, what played to its end is heard, the
    /// next made one plays when it may, and the next sentence is made.
    pub(super) fn speak_tick(&mut self, audio: &impl AudioPort) {
        let now = Instant::now();
        self.speak_sweep(now);
        if let Some(id) = self.speak.playing {
            let p = self.speak.player.as_mut().expect("a player while playing");
            if let Some(w) = p.pump() {
                tracing::warn!("speech: {w}");
            }
            let closed = p.closed();
            let idle = p.idle();
            if closed {
                self.speak.player = None;
                let why = "the playback stream closed while Kai spoke (PipeWire went away)";
                self.speak.log_failure(why);
                let f = Failure {
                    status: None,
                    why: why.into(),
                };
                if let Some(item) = self.speak.end(id, Spoken::Failed(f), now) {
                    self.speak_notice_failed(&item, why);
                }
            } else if idle
                && self
                    .speak
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .is_some_and(|i| i.all_made() && i.pushed == i.sentences.len())
            {
                // A natural end: the echo's guard, and nothing to flush.
                self.speak.playing = None;
                self.speak.guard_echo(now);
                self.speak.end(id, Spoken::Heard, now);
            }
        }
        // Kai's own question first in line takes its own rule (module doc).
        let question = self
            .speak
            .items
            .first()
            .is_some_and(|i| i.u.kind == Kind::Question);
        let blocked = if question {
            self.question_blocked()
        } else {
            self.speak_blocked(now)
        };
        // A notice that cannot be spoken soon is notified instead, and a question ends: its dialog
        // shows it (module doc).
        if blocked.is_some() && (!self.audio_up || self.results_dnd()) {
            let unsaid: Vec<u64> = self
                .speak
                .items
                .iter()
                .filter(|i| i.u.kind == Kind::Question && Some(i.id) != self.speak.playing)
                .map(|i| i.id)
                .collect();
            for id in unsaid {
                let why = blocked.clone().unwrap_or_default();
                self.speak
                    .end(id, Spoken::Failed(Failure { status: None, why }), now);
            }
            let notices: Vec<u64> = self
                .speak
                .items
                .iter()
                .filter(|i| i.u.kind == Kind::Notice && Some(i.id) != self.speak.playing)
                .map(|i| i.id)
                .collect();
            for id in notices {
                if let Some(item) = self.speak.end(id, Spoken::Notified, now) {
                    let why = blocked.clone().unwrap_or_default();
                    self.speak_notify(&item, &why);
                }
            }
        }
        match &blocked {
            None => {
                if self.speak.playing.is_none() {
                    self.speak_play(audio, now);
                }
                self.speak_make(now, question);
            }
            Some(why) if why != QUIET_HOLD => {
                // One that plays and has sentences left to make, which may not be made now,
                // goes back to wait whole once what was made has played (as for Kai's reply).
                if let Some(id) = self.speak.playing
                    && self.speak.player.as_ref().is_some_and(Player::idle)
                    && let Some(i) = self.speak.items.iter_mut().find(|i| i.id == id)
                    && !i.all_made()
                {
                    i.pushed = 0;
                    self.speak.playing = None;
                    self.speak.guard_echo(now);
                }
                for i in &mut self.speak.items {
                    if i.told.as_deref() != Some(why.as_str()) {
                        i.told = Some(why.clone());
                        i.reply(Said::Waiting(why.clone()));
                    }
                }
            }
            Some(_) => {}
        }
        if self.speak.items.is_empty() && self.speak.playing.is_none() {
            // Quiet again: the stream closes (its sink may suspend), and the next run asks for
            // Kai's voice again.
            if self.speak.player.take().is_some() {
                tracing::debug!("speech: done; the playback stream closes");
            }
            self.speak.kai_voice = None;
        }
    }

    /// The first item plays when its first sentence is made and its gate lets it.
    fn speak_play(&mut self, audio: &impl AudioPort, now: Instant) {
        loop {
            let Some(item) = self.speak.items.first() else {
                return;
            };
            if item.made.is_empty() {
                return;
            }
            let id = item.id;
            if let Some(d) = self.speak_gate(item) {
                self.speak.end(id, Spoken::Dropped(d), now);
                continue;
            }
            break;
        }
        let sp = &mut self.speak;
        if sp.player.as_ref().is_none_or(Player::closed) {
            sp.player = Some(audio.open_playback());
        }
        let p = sp.player.as_mut().expect("opened above");
        let item = sp.items.first_mut().expect("looked at above");
        item.plays += 1;
        let name = item.name();
        for pcm in &item.made {
            p.push(&name, pcm);
        }
        item.pushed = item.made.len();
        if item.all_made() {
            p.end(&name);
        }
        item.told = None;
        sp.playing = Some(item.id);
        item.reply(Said::Started);
    }

    /// The next sentence in play order is made, one at a time; Kai's voice is asked first where
    /// it needs it. `questions`: only Kai's own questions may be made now (module doc).
    fn speak_make(&mut self, now: Instant, questions: bool) {
        if self.speak.making.is_some() {
            return;
        }
        loop {
            let Some(item) = self
                .speak
                .items
                .iter()
                .find(|i| !i.all_made() && (!questions || i.u.kind == Kind::Question))
            else {
                return;
            };
            let id = item.id;
            if let Some(d) = self.speak_gate(item) {
                self.speak.end(id, Spoken::Dropped(d), now);
                continue;
            }
            let (voice, lang) = match item.u.voice.clone() {
                Some(v) => (v, Lang::En),
                None => {
                    let Some(thread) = self
                        .session
                        .as_ref()
                        .map(|s| s.thread.id)
                        .or(self.thread_id())
                    else {
                        self.speak_failed(
                            id,
                            Failure {
                                status: None,
                                why: "Kai has no conversation thread yet, so it has no voice to \
                                      speak with"
                                    .into(),
                            },
                            now,
                        );
                        continue;
                    };
                    match &self.speak.kai_voice {
                        Some((t, v)) if *t == thread => (v.voice.clone(), v.lang),
                        _ => {
                            if self.speak.asking.is_none() {
                                let fut = self.link.kai_voice(thread);
                                self.speak.asking = Some((thread, fut));
                            }
                            return;
                        }
                    }
                }
            };
            let item = self
                .speak
                .items
                .iter_mut()
                .find(|i| i.id == id)
                .expect("found above");
            if item.sentences.is_empty() {
                item.sentences = sentences(item.u.text.of(lang));
                if item.sentences.is_empty() {
                    self.speak.end(id, Spoken::Heard, now);
                    continue;
                }
            }
            let fut = self.link.speech(&voice, &item.sentences[item.made.len()]);
            self.speak.making = Some((id, fut));
            return;
        }
    }

    /// Item `id` failed (module doc).
    fn speak_failed(&mut self, id: u64, f: Failure, now: Instant) {
        self.speak.log_failure(&f.why);
        let why = f.why.clone();
        if let Some(item) = self.speak.end(id, Spoken::Failed(f), now) {
            self.speak_notice_failed(&item, &why);
        }
    }

    /// A notice whose speech failed is notified instead (module doc).
    fn speak_notice_failed(&mut self, item: &Item, why: &str) {
        if item.u.kind == Kind::Notice {
            self.speak_notify(item, &format!("Kai could not speak it: {why}"));
        }
    }

    fn speak_notify(&mut self, item: &Item, why: &str) {
        self.notify("Kai", format!("{} ({why})", item.u.text.en()));
    }

    /// What [`Speaker::next`] woke for.
    pub(super) fn speak_wake(&mut self, w: SpeakWake, audio: &impl AudioPort) {
        let now = Instant::now();
        match w {
            SpeakWake::Made(id, Ok(pcm)) => {
                let playing = self.speak.playing;
                let sp = &mut self.speak;
                if let Some(item) = sp.items.iter_mut().find(|i| i.id == id) {
                    item.made.push(pcm);
                    if playing == Some(id)
                        && let Some(p) = sp.player.as_mut()
                    {
                        let name = item.name();
                        for pcm in &item.made[item.pushed..] {
                            p.push(&name, pcm);
                        }
                        item.pushed = item.made.len();
                        if item.all_made() {
                            p.end(&name);
                        }
                    }
                }
            }
            SpeakWake::Made(id, Err(e)) => {
                let f = Failure {
                    status: (e.status != 0).then_some(e.status),
                    why: format!("lmgw did not make the speech: {e}"),
                };
                self.speak_failed(id, f, now);
            }
            SpeakWake::Voice(thread, Ok(v)) => self.speak.kai_voice = Some((thread, v)),
            SpeakWake::Voice(_, Err(e)) => {
                // Every item that waits for Kai's voice fails with it.
                let waiting: Vec<u64> = self
                    .speak
                    .items
                    .iter()
                    .filter(|i| !i.all_made() && i.u.voice.is_none())
                    .map(|i| i.id)
                    .collect();
                let why = format!("Kai's voice could not be read from lmgw: {e}");
                for id in waiting {
                    let f = Failure {
                        status: None,
                        why: why.clone(),
                    };
                    self.speak_failed(id, f, now);
                }
            }
        }
        self.speak_tick(audio);
    }

    /// Kai's own speech begins (`response.created`): what plays goes back to wait, whole (module
    /// doc).
    pub(super) fn speak_yield(&mut self) {
        let Some(id) = self.speak.playing.take() else {
            return;
        };
        tracing::info!("speech: Kai's reply begins; what Kai said by itself waits for its end");
        let now = Instant::now();
        self.speak.guard_echo(now);
        self.speak.quiet_since = None;
        if let Some(i) = self.speak.items.iter_mut().find(|i| i.id == id) {
            i.pushed = 0;
        }
        // The session's stream replaces this one; it closes now.
        if let Some(mut p) = self.speak.player.take() {
            p.flush();
        }
    }

    /// The owner talks (`speech_started`, a talk press) or the screen locked (`why`): what plays
    /// stops, the reading is cut (module doc). Whether anything played.
    pub(super) fn speak_cut(&mut self, why: &str) -> bool {
        let now = Instant::now();
        let sounding = self.speak.playing;
        let cut: Vec<u64> = self
            .speak
            .items
            .iter()
            .filter(|i| Some(i.id) == sounding || i.u.kind == Kind::Reading)
            .map(|i| i.id)
            .collect();
        if !cut.is_empty() {
            tracing::info!(why, "speech: cut");
        }
        for id in cut {
            self.speak.end(id, Spoken::Cut(why.to_owned()), now);
        }
        self.speak.quiet_since = None;
        sounding.is_some()
    }

    /// The screen locked (K32): what plays stops, the reading is cut, every announcement dropped.
    pub(super) fn speak_locked(&mut self) {
        self.speak_cut("the screen locked");
        let now = Instant::now();
        let rest: Vec<u64> = self.speak.items.iter().map(|i| i.id).collect();
        for id in rest {
            self.speak.end(id, Spoken::Dropped(Dropped::Locked), now);
        }
        self.speak.player = None;
        self.speak.making = None;
    }

    /// Kai quits: nothing more is said.
    pub(super) fn speak_quit(&mut self) {
        let now = Instant::now();
        let all: Vec<u64> = self.speak.items.iter().map(|i| i.id).collect();
        for id in all {
            self.speak.end(id, Spoken::Dropped(Dropped::Quit), now);
        }
        self.speak.player = None;
        self.speak.asking = None;
        self.speak.making = None;
    }
}

/// The quiet moment's hold-off: not told as a reason to wait (module doc).
const QUIET_HOLD: &str = "waiting for a quiet moment (browser.speech_quiet_ms)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_into_sentences_and_never_shortened() {
        assert_eq!(
            sentences("We left at dawn. Then north! Did we? Yes…\nA line\n\n"),
            [
                "We left at dawn.",
                "Then north!",
                "Did we?",
                "Yes…",
                "A line"
            ]
        );
        // No end inside a number or an abbreviation without a space after it; a long run without
        // an end stays whole.
        assert_eq!(sentences("It costs 3.50 now"), ["It costs 3.50 now"]);
        let long = "word ".repeat(2000);
        assert_eq!(sentences(&long), [long.trim()]);
        assert!(sentences("  \n ").is_empty());
    }

    #[test]
    fn kais_words_follow_its_reply_language_for_german_and_english_only() {
        assert_eq!(Lang::of(Some("de")), Lang::De);
        assert_eq!(Lang::of(Some("de-AT")), Lang::De);
        assert_eq!(Lang::of(Some("en")), Lang::En);
        assert_eq!(
            Lang::of(Some("fr")),
            Lang::En,
            "others fall back to English"
        );
        assert_eq!(Lang::of(None), Lang::En);
        let w = Text::Kai(Words::new("Reading stopped.", "Vorlesen beendet."));
        assert_eq!(w.of(Lang::De), "Vorlesen beendet.");
        assert_eq!(Text::Page("Bonjour.".into()).of(Lang::De), "Bonjour.");
    }

    #[test]
    fn a_refusal_is_a_4xx_only() {
        let f = |status| Failure {
            status,
            why: String::new(),
        };
        assert!(f(Some(400)).refused() && f(Some(404)).refused());
        assert!(!f(Some(503)).refused() && !f(None).refused());
    }
}
