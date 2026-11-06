//! Reading aloud (kai-firefox design §7.3; the contract's Reading aloud): `read_aloud`,
//! `read_aloud_from`, `read_pause`, `read_resume` and `read_skip`, listed only with the
//! hands-and-eyes switch on (D3).
//!
//! - **Who holds what.** The extension holds the page side: the paragraphs as they were when
//!   `read.open@1` ran, the mark, the scrolling. Kai holds the playback: the index, the speech of
//!   each paragraph in the voice for its `lang` (`browser.read.voices`, else Kai's own), through
//!   `crate::voice::speak`, which makes it sentence by sentence. One reading at a time: a new one
//!   ends the last once its `read.open@1` succeeded.
//! - **Each paragraph:** `read.paragraph@1` with the clean-up rules of `[browser.read]` (their
//!   defaults, superscripts kept); one whose `spoken` is empty, or whose own site or tab's site is
//!   a `none` site (an opaque origin, the top document's too, gets the strictest rule), is
//!   skipped; it is handed to the speech while the one before it plays, so the next is made
//!   meanwhile; `read.mark@1` marks it as its audio begins (a paragraph the page removed meanwhile
//!   is read all the same, unmarked). Its site rules are checked again as it plays: one that
//!   became `none` ends the reading.
//! - **Pause, resume, skip** (`read_pause`, `read_resume`, `read_skip {by}`): pausing stops the
//!   audio at once; resuming reads the paragraph it stopped in again from its start; skipping moves
//!   `by` paragraphs from the one read now (negative goes back) and reads from there, also from a
//!   pause.
//! - **The owner talking pauses it** (a barge-in, a talk press; kai §5.5), and so does the screen
//!   lock: it stays paused until `read_resume`. **Kai's own speech** (an answer, a job's result)
//!   only holds it: the paragraph it stopped in is heard again from its start once Kai is quiet,
//!   with no call (`crate::voice::speak`). What it waits for (do not disturb, PipeWire, a turn) is
//!   in its line (`browser_context`, the tools' answers).
//! - **When the speech fails:** a paragraph lmgw refuses (a 4xx: nothing to say once shaped, a
//!   text the model takes not) is skipped with one short spoken notice, and the reading goes on; a
//!   second refusal in a row pauses it. Any other failure (no answer within
//!   `browser.speech_timeout_s` for a sentence, lmgw's 5xx, a voice that cannot be read) pauses it
//!   with one spoken notice ("Reading paused: speech failed."), which becomes a desktop
//!   notification where speech itself is down (`crate::voice::speak`). Resuming tries that
//!   paragraph once more; failing again, it is skipped with a notice, so a resume never meets the
//!   same failure twice.
//! - **The end.** The last paragraph heard ends it silently. The extension's `read_ended` event
//!   (the tab navigated, closed, was discarded, or showed another page) ends it at once, events
//!   on or off: what plays stops, what waits is dropped, and Kai says once why ("Reading stopped:
//!   the page changed."), as a notice of its own speech, unless Kai's own action caused it
//!   (`by_kai`). `session_ended` on a later call, and Firefox closing, end it the same way.
//! - **Kai's words** in a notice come in English and German; the speech says them in Kai's reply
//!   language (`crate::voice::speak`).

use std::sync::{Arc, Mutex, Weak};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::{Inner, failure, page, read, read::reply, target};
use crate::browser::announce::READ_ENDED;
use crate::browser::links::{Incoming, LinkId, OpError};
use crate::browser::settings::voice_for;
use crate::browser::sites::Rule;
use crate::browser::tabs::{Tab, head};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;
use crate::voice::speak::{
    Dropped, Failure, Gate, Kind, Said, SpeechVoice, Spoken, Text, Utterance, Words,
};

pub const OPEN: &str = "read.open@1";
pub const PARAGRAPH: &str = "read.paragraph@1";
pub const MARK: &str = "read.mark@1";
pub const CLOSE: &str = "read.close@1";

/// Paragraphs handed to the speech at once: the one read and the next, made meanwhile.
const AHEAD: usize = 2;

/// Where a reading stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Reading,
    /// Paused, and why (`read_pause`, the owner spoke, a failure).
    Paused(String),
    /// Ended, and why.
    Ended(String),
}

/// Why a reading ended, in Kai's words (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    PageChanged,
    TabClosed,
    Discarded,
    Firefox,
    Restarted,
    Site,
    Other,
}

impl End {
    /// A session's end as the extension words it (`read_ended`'s and `session_ended`'s `why`).
    fn of_why(why: Option<&str>) -> Self {
        match why {
            Some("navigated" | "page_changed") => End::PageChanged,
            Some("tab_closed") => End::TabClosed,
            Some("discarded") => End::Discarded,
            _ => End::Other,
        }
    }

    fn words(self) -> Words {
        let (en, de) = match self {
            End::PageChanged => ("the page changed", "die Seite hat sich geändert"),
            End::TabClosed => ("the tab closed", "der Tab wurde geschlossen"),
            End::Discarded => ("Firefox unloaded the tab", "Firefox hat den Tab entladen"),
            End::Firefox => ("Firefox closed", "Firefox wurde geschlossen"),
            End::Restarted => (
                "Kai for Firefox restarted",
                "Kai for Firefox wurde neu gestartet",
            ),
            End::Site => (
                "its site is one Kai does not reach now",
                "Kai darf diese Seite jetzt nicht lesen",
            ),
            End::Other => ("Kai for Firefox ended it", "Kai for Firefox hat es beendet"),
        };
        Words::new(en, de)
    }
}

/// The reading now (module doc).
pub struct Reading {
    pub link: LinkId,
    /// The extension's session id.
    pub session: i64,
    pub tab: Tab,
    pub paragraphs: u64,
    st: Mutex<St>,
    wake: Notify,
}

struct St {
    state: State,
    /// Counts the pauses, skips and resumes: speech of an older one is not this reading's.
    generation: u64,
    /// The paragraphs with the speech now, in order, with their cancels.
    sent: Vec<(u64, CancellationToken)>,
    /// The next paragraph to hand over.
    next: u64,
    /// What the speech waits for now (`Said::Waiting`).
    held: Option<String>,
    /// The paragraph whose speech failed last (module doc: a second failure skips it).
    failed: Option<u64>,
    /// The last paragraph's speech was refused (a second refusal in a row pauses).
    refused: bool,
    /// A later paragraph failed while an earlier one played: it is acted on when that ends.
    failure: Option<(u64, Failure)>,
    /// The extension ended the session: nothing to close.
    gone: bool,
}

impl St {
    /// The paragraph read now, or the next to be.
    fn at(&self) -> u64 {
        self.sent.first().map_or(self.next, |(i, _)| *i)
    }

    /// Stops what the speech has, and reads again from the paragraph read now when it goes on.
    fn halt(&mut self) {
        self.next = self.at();
        self.drop_sent();
    }

    /// Takes back what the speech has; its answers belong to an older generation now.
    fn drop_sent(&mut self) {
        for (_, c) in self.sent.drain(..) {
            c.cancel();
        }
        self.failure = None;
        self.held = None;
        self.generation += 1;
    }
}

impl Reading {
    fn st(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The reading's state, the paragraph read now (from 0) and how many there are.
    pub fn now(&self) -> (State, u64) {
        let st = self.st();
        (st.state.clone(), st.at())
    }

    /// `browser_context`'s line.
    pub fn line(&self, sites: &crate::browser::sites::SiteRules) -> String {
        let (state, at, held) = {
            let st = self.st();
            (st.state.clone(), st.at(), st.held.clone())
        };
        let place = format!(
            "{}, paragraph {} of {}",
            head(&self.tab, sites),
            (at + 1).min(self.paragraphs),
            self.paragraphs
        );
        match (state, held) {
            (State::Reading, None) => format!("reading aloud: {place}"),
            (State::Reading, Some(why)) => format!("reading aloud (waiting: {why}): {place}"),
            (State::Paused(why), _) => format!("reading aloud, paused ({why}): {place}"),
            (State::Ended(why), _) => format!("reading aloud ended ({why}): {place}"),
        }
    }

    fn pause(&self, why: &str) {
        let mut st = self.st();
        if st.state == State::Reading {
            st.halt();
            st.state = State::Paused(why.to_owned());
        }
        drop(st);
        self.wake.notify_one();
    }

    /// Ends it; `false` when it had ended.
    fn end(&self, why: &str) -> bool {
        let mut st = self.st();
        if matches!(st.state, State::Ended(_)) {
            return false;
        }
        st.halt();
        st.state = State::Ended(why.to_owned());
        drop(st);
        self.wake.notify_one();
        true
    }
}

/// `read.open@1`'s answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Opened {
    session: i64,
    tab: Tab,
    paragraphs: u64,
    start: Option<u64>,
    from: Option<String>,
}

/// `read.paragraph@1`'s answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Paragraph {
    tab: Tab,
    spoken: String,
    lang: Option<String>,
    origin: Option<String>,
}

/// `read_aloud`.
pub fn aloud(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(open(&inner, &call).await) })
}

/// `read_aloud_from`.
pub fn aloud_from(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    aloud(inner, call)
}

async fn open(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let v = match page::send(inner, call, &t, OPEN, Map::new()).await {
        Ok(v) => v,
        Err(e) => return Err(failure(inner, &t.link, e, t.element.as_ref(), &call.cancel).await),
    };
    let sites = inner.browser().sites;
    page::checked(call, &v, &sites)?;
    let o: Opened = read(OPEN, v)?;
    // The new session exists: the last reading ends without a word (in this tab the extension
    // ended its session already, `replaced`).
    if let Some(old) = inner.reading().take() {
        old.end("another reading began");
    }
    let at = head(&o.tab, &sites);
    let Some(start) = o.start.filter(|_| o.from.is_some() && o.paragraphs > 0) else {
        close(inner, t.link.id, o.session).await;
        return Ok(format!("{at}: nothing to read aloud"));
    };
    let from = match o.from.as_deref() {
        Some("ref") => "the element",
        Some("focus") => "the focused element",
        Some("view") => "the top of the view",
        Some("below_view") => "below the view",
        _ => "the top of the page",
    };
    let r = Arc::new(Reading {
        link: t.link.id,
        session: o.session,
        tab: o.tab.clone(),
        paragraphs: o.paragraphs,
        st: Mutex::new(St {
            state: State::Reading,
            generation: 0,
            sent: Vec::new(),
            next: start,
            held: None,
            failed: None,
            refused: false,
            failure: None,
            gone: false,
        }),
        wake: Notify::new(),
    });
    *inner.reading() = Some(r.clone());
    tokio::spawn(run(inner.clone(), r));
    Ok(format!(
        "{at}: reading aloud from paragraph {} of {} ({from}); read_pause, read_resume and \
         read_skip control it",
        start + 1,
        o.paragraphs
    ))
}

/// The reading now, or why there is none to control.
fn current(inner: &Inner) -> Result<Arc<Reading>, String> {
    inner
        .reading()
        .clone()
        .ok_or_else(|| "nothing is being read aloud; read_aloud starts".to_owned())
}

/// `read_pause`.
pub fn pause(inner: Arc<Inner>, _call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply((|| {
            let r = current(&inner)?;
            let sites = inner.browser().sites;
            match r.now().0 {
                State::Ended(why) => Err(format!("the reading ended ({why}); read_aloud starts")),
                State::Paused(_) => Ok(format!("already paused; {}", r.line(&sites))),
                State::Reading => {
                    r.pause("read_pause");
                    Ok(r.line(&sites))
                }
            }
        })())
    })
}

/// `read_resume`.
pub fn resume(inner: Arc<Inner>, _call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply((|| {
            let r = current(&inner)?;
            let sites = inner.browser().sites;
            let mut st = r.st();
            match st.state.clone() {
                State::Ended(why) => Err(format!("the reading ended ({why}); read_aloud starts")),
                State::Reading => {
                    drop(st);
                    Ok(format!("already reading; {}", r.line(&sites)))
                }
                State::Paused(why) => {
                    st.state = State::Reading;
                    st.generation += 1;
                    drop(st);
                    r.wake.notify_one();
                    let mut said = r.line(&sites);
                    if why.starts_with("its speech failed") {
                        said.push_str(&format!(
                            " (it had paused: {why}; failing again, that paragraph is skipped)"
                        ));
                    }
                    Ok(said)
                }
            }
        })())
    })
}

/// `read_skip {by}`.
pub fn skip(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move {
        reply((|| {
            let by = call
                .args
                .integer("by")
                .ok_or("read_skip needs by, a number of paragraphs")?;
            let r = current(&inner)?;
            let sites = inner.browser().sites;
            let mut st = r.st();
            if let State::Ended(why) = &st.state {
                return Err(format!("the reading ended ({why}); read_aloud starts"));
            }
            let at = st.at() as i64;
            let to = at.saturating_add(by);
            if to < 0 || to >= r.paragraphs as i64 {
                return Err(format!(
                    "the page has paragraphs 1 to {} and the reading is at {}; by {by} goes \
                     past them",
                    r.paragraphs,
                    at + 1
                ));
            }
            st.drop_sent();
            st.next = to as u64;
            st.state = State::Reading;
            drop(st);
            r.wake.notify_one();
            Ok(r.line(&sites))
        })())
    })
}

/// The tag of a paragraph's speech: its generation and index.
fn tag(generation: u64, index: u64) -> u64 {
    (generation << 32) | (index & 0xffff_ffff)
}

/// The reading's task (module doc).
async fn run(inner: Arc<Inner>, r: Arc<Reading>) {
    let (said_tx, mut said) = mpsc::unbounded_channel();
    let mut events = inner.links.subscribe();
    loop {
        fill(&inner, &r, &said_tx).await;
        if matches!(r.now().0, State::Ended(_)) {
            break;
        }
        tokio::select! {
            Some((t, s)) = said.recv() => heard(&inner, &r, t, s).await,
            () = r.wake.notified() => {}
            item = events.recv() => match item {
                Some(Incoming::Event { link, kind, fields }) if link == r.link && kind == READ_ENDED => {
                    read_ended(&inner, &r, &fields);
                }
                Some(_) => {}
                None => {
                    r.end("Kai's link to Firefox ended");
                }
            },
        }
    }
    // It stays the reading now, ended, so the reading tools can say why.
    if !r.st().gone {
        close(&inner, r.link, r.session).await;
    }
}

/// The extension's `read_ended` (module doc): of this session, it stops at once and says why
/// once, unless Kai's own action ended it.
fn read_ended(inner: &Inner, r: &Reading, f: &Map<String, Value>) {
    if f.get("session").and_then(Value::as_i64) != Some(r.session) {
        return;
    }
    let end = End::of_why(f.get("why").and_then(Value::as_str));
    let by_kai = f.get("by_kai").and_then(Value::as_bool) == Some(true);
    r.st().gone = true;
    if r.end(&end.words().en) && !by_kai {
        stopped(inner, r, end);
    } else if by_kai {
        tracing::info!(
            session = r.session,
            "browser: Kai's own action ended the reading; nothing is said"
        );
    }
}

/// Hands the next paragraphs to the speech while the reading goes on and fewer than [`AHEAD`]
/// are with it; the end of the page ends it.
async fn fill(inner: &Arc<Inner>, r: &Reading, said: &mpsc::UnboundedSender<(u64, Said)>) {
    loop {
        let (generation, index) = {
            let mut st = r.st();
            if st.state != State::Reading || st.sent.len() >= AHEAD || st.failure.is_some() {
                return;
            }
            if st.next >= r.paragraphs {
                if st.sent.is_empty() {
                    st.state = State::Ended("the end of the page".into());
                }
                return;
            }
            (st.generation, st.next)
        };
        let policy = inner.browser();
        let args = json!({"session": r.session, "index": index, "rules": policy.read.rules()});
        let answer = inner
            .links
            .op(r.link, PARAGRAPH, args, &CancellationToken::new())
            .await;
        let p = match answer {
            Ok(v) => match read::<Paragraph>(PARAGRAPH, v) {
                Ok(p) => p,
                Err(e) => {
                    r.pause(&format!("Kai for Firefox's paragraph did not read: {e}"));
                    return;
                }
            },
            Err(e) => {
                ended_by(inner, r, &e);
                return;
            }
        };
        // The paragraph's own site and its tab's (an opaque origin, the top document's too: the
        // strictest rule).
        let skip = p.spoken.trim().is_empty() || !reached(&policy.sites, &p.tab.url, &p.origin);
        let mut st = r.st();
        if st.generation != generation || st.state != State::Reading || st.next != index {
            // Paused, skipped or ended while the paragraph was asked for.
            continue;
        }
        st.next = index + 1;
        if skip {
            continue;
        }
        let voice = voice_for(&policy.voices, p.lang.as_deref()).map(|v| SpeechVoice {
            model: v.model.clone(),
            voice: v.voice.clone(),
            ..Default::default()
        });
        let cancel = CancellationToken::new();
        st.sent.push((index, cancel.clone()));
        drop(st);
        inner.mouth.say(Utterance {
            kind: Kind::Reading,
            text: Text::Page(p.spoken),
            voice,
            gate: Some(site_gate(inner, p.tab.url, p.origin)),
            cancel,
            tag: tag(generation, index),
            reply: Some(said.clone()),
        });
    }
}

/// Neither the tab's URL nor the paragraph's origin is a `none` site.
fn reached(sites: &crate::browser::sites::SiteRules, url: &str, origin: &Option<String>) -> bool {
    sites.rule(url) != Rule::None && sites.frame_rule(origin.as_deref()) != Rule::None
}

/// The paragraph's site rules, checked again when it is made and when it plays (module doc).
fn site_gate(inner: &Arc<Inner>, url: String, origin: Option<String>) -> Gate {
    let w: Weak<Inner> = Arc::downgrade(inner);
    Arc::new(move || {
        let i = w.upgrade().ok_or(Dropped::Quit)?;
        if reached(&i.browser().sites, &url, &origin) {
            Ok(true)
        } else {
            Err(Dropped::Site)
        }
    })
}

/// What a failure of a paragraph's speech leads to (module doc).
enum After {
    Skip(Words),
    Pause(String),
}

/// What the speech said of a paragraph.
async fn heard(inner: &Inner, r: &Reading, t: u64, s: Said) {
    let (generation, index) = (t >> 32, t & 0xffff_ffff);
    if r.st().generation != generation {
        return;
    }
    match s {
        Said::Waiting(why) => r.st().held = Some(why),
        Said::Started => {
            r.st().held = None;
            let args = json!({"session": r.session, "index": index});
            match inner
                .links
                .op(r.link, MARK, args, &CancellationToken::new())
                .await
            {
                Ok(v) if v.get("marked") == Some(&Value::Bool(false)) => {
                    // The page took the paragraph away meanwhile: read all the same (module doc).
                    let why = v.get("why").and_then(Value::as_str).unwrap_or_default();
                    tracing::debug!(
                        session = r.session,
                        index,
                        why,
                        "browser: the paragraph could not be marked"
                    );
                }
                Ok(_) => {}
                Err(e) => ended_by(inner, r, &e),
            }
        }
        Said::Done(Spoken::Heard) => {
            let pending = {
                let mut st = r.st();
                st.sent.retain(|(i, _)| *i != index);
                st.refused = false;
                if st.failed == Some(index) {
                    st.failed = None;
                }
                if st.sent.is_empty() && st.next >= r.paragraphs && st.state == State::Reading {
                    st.state = State::Ended("the end of the page".into());
                }
                st.sent.is_empty().then(|| st.failure.take()).flatten()
            };
            if let Some((i, f)) = pending {
                failed(inner, r, i, f);
            }
        }
        Said::Done(Spoken::Cut(why)) => r.pause(&why),
        Said::Done(Spoken::Failed(f)) => {
            let earlier = {
                let mut st = r.st();
                st.sent.retain(|(i, _)| *i != index);
                let earlier = st.sent.iter().any(|(i, _)| *i < index);
                if earlier {
                    // An earlier paragraph plays: it ends first, nothing after this one goes.
                    for (_, c) in st.sent.iter().filter(|(i, _)| *i > index) {
                        c.cancel();
                    }
                    st.sent.retain(|(i, _)| *i < index);
                    st.failure = Some((index, f.clone()));
                }
                earlier
            };
            if !earlier {
                failed(inner, r, index, f);
            }
        }
        Said::Done(Spoken::Dropped(Dropped::Site)) => {
            if r.end(&End::Site.words().en) {
                stopped(inner, r, End::Site);
            }
        }
        // Kai's own cancel (a pause, a skip) changed the generation first.
        Said::Done(Spoken::Dropped(Dropped::Cancelled)) => {}
        Said::Done(Spoken::Dropped(d)) => r.pause(&d.to_string()),
        Said::Done(Spoken::Notified) => {}
    }
}

/// Paragraph `index`'s speech failed: skipped or paused, with one spoken notice (module doc).
fn failed(inner: &Inner, r: &Reading, index: u64, f: Failure) {
    let mut st = r.st();
    if st.state != State::Reading {
        return;
    }
    let after = if f.refused() && !st.refused {
        st.refused = true;
        After::Skip(Words::new(
            "Skipped a paragraph: lmgw would not speak it.",
            "Einen Absatz übersprungen: lmgw wollte ihn nicht sprechen.",
        ))
    } else if st.failed == Some(index) {
        st.failed = None;
        After::Skip(Words::new(
            "Skipped a paragraph: its speech failed twice.",
            "Einen Absatz übersprungen: die Sprachausgabe ist zweimal fehlgeschlagen.",
        ))
    } else {
        st.failed = Some(index);
        After::Pause(format!("its speech failed: {f}"))
    };
    st.drop_sent();
    let words = match after {
        After::Skip(w) => {
            tracing::info!(
                session = r.session,
                index,
                "browser: a paragraph is skipped: {f}"
            );
            st.next = index + 1;
            w
        }
        After::Pause(why) => {
            tracing::info!(
                session = r.session,
                index,
                "browser: the reading pauses: {f}"
            );
            st.next = index;
            st.state = State::Paused(why);
            Words::new(
                "Reading paused: speech failed.",
                "Vorlesen angehalten: die Sprachausgabe ist fehlgeschlagen.",
            )
        }
    };
    drop(st);
    r.wake.notify_one();
    notice(inner, words);
}

/// An operation of the reading failed: the session's end (`session_ended`, `unknown_session`,
/// the link gone) ends the reading and says why once; anything else pauses it.
fn ended_by(inner: &Inner, r: &Reading, e: &OpError) {
    let end = match e {
        OpError::Failed { code, about, .. } if code == "session_ended" => {
            match about.as_ref().and_then(|a| a.why.as_deref()) {
                // Kai's own doing: another `read.open@1` in its tab, or `read.close@1`.
                Some("replaced" | "closed") => {
                    r.end("another reading began in its tab");
                    return;
                }
                why => End::of_why(why),
            }
        }
        OpError::Failed { code, .. } if code == "unknown_session" => End::Restarted,
        OpError::NoLink(_) | OpError::Closed => End::Firefox,
        other => {
            r.pause(&format!("Kai for Firefox failed: {}", super::said(other)));
            return;
        }
    };
    r.st().gone = true;
    if r.end(&end.words().en) {
        stopped(inner, r, end);
    }
}

/// Kai says once why the reading stopped (module doc).
fn stopped(inner: &Inner, r: &Reading, end: End) {
    let w = end.words();
    tracing::info!(session = r.session, why = %w.en, "browser: the reading stopped");
    notice(
        inner,
        Words::new(
            format!("Reading stopped: {}.", w.en),
            format!("Vorlesen beendet: {}.", w.de),
        ),
    );
}

/// One notice of Kai's own speech about the reading (module doc).
fn notice(inner: &Inner, words: Words) {
    inner.mouth.say(Utterance {
        kind: Kind::Notice,
        text: Text::Kai(words),
        voice: None,
        gate: None,
        cancel: CancellationToken::new(),
        tag: 0,
        reply: None,
    });
}

/// `read.close@1`, its answer only logged (a session that ended answers `closed: false`).
async fn close(inner: &Inner, link: LinkId, session: i64) {
    match inner
        .links
        .op(
            link,
            CLOSE,
            json!({"session": session}),
            &CancellationToken::new(),
        )
        .await
    {
        Ok(v) => tracing::debug!(session, answer = %v, "browser: read.close"),
        Err(e) => tracing::debug!(session, "browser: read.close failed: {e}"),
    }
}
