//! Playback and the heard position (§5.5, K7).
//!
//! Audio is pushed per response item into a [`Player`]. Every sample gets a position on one
//! timeline (the order of pushes); the player moves the queue into a ring, and the PipeWire
//! callback ([`PlayHead`]) writes the ring into the stream, silence when it runs dry.
//!
//! **What was heard.** Each cycle the callback reads the stream's time report (`pw_time`, through
//! `Stream::time()`, which calls `pw_stream_get_time_n`): the sample presented at the report's
//! `now` is the one written `queued + buffered + delay` frames before the write head. The callback
//! keeps a short log of which timeline positions it wrote where in the stream (runs; silence
//! between them), maps the presented stream position back to the timeline, and publishes it with
//! `now`. A reader extrapolates at 24 kHz from `now`, never past the end of continuous audio. Per
//! item, heard = how much of its positions lie before that point.
//!
//! A barge-in (WP4) calls [`Player::flush`]: the queue goes at once, the ring at the callback's next
//! cycle, and [`Player::flushed`] answers with every item's pushed, played and heard counts. What was
//! played but not yet heard (at most the stream's delay) still plays out.

// The voice session (WP4) plays through it; until then the tests and the live check do.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use super::RATE;
use super::ring::{LatestWindow, RingReader, RingWriter, SeqCell};

/// How many runs the callback remembers. A run ends only at an underrun or a flush; the presented
/// position is never more than the stream's delay behind the write head. Evicting the oldest run is
/// counted in [`PlayPos::evicted`], and the player logs it (the heard position may jump then).
pub const RUNS: usize = 64;

/// One cycle's time report, reduced to what the head needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Clock {
    /// CLOCK_MONOTONIC ns of the report.
    pub now_ns: i64,
    /// Stream frames between the write head and the sample presented at `now`: the sum of the
    /// three below.
    pub latency: u64,
    /// `pw_time.delay` (the graph's delay to the device) in stream frames.
    pub delay: u64,
    /// `pw_time.buffered`: frames in the resampler.
    pub buffered: u64,
    /// `pw_time.queued`: frames in buffers queued and not yet taken.
    pub queued: u64,
}

impl Clock {
    /// From `pw_time`: `delay` in graph ticks of `num/denom` s; `queued` in frames (Kai sets
    /// `pw_buffer.size` in frames); `buffered` in stream frames (the resampler).
    pub fn from_time(
        now_ns: i64,
        num: u32,
        denom: u32,
        delay: i64,
        queued: u64,
        buffered: u64,
    ) -> Option<Clock> {
        if denom == 0 || now_ns <= 0 {
            return None;
        }
        let delay = (delay.max(0) as u128 * RATE as u128 * num as u128 / denom as u128) as u64;
        Some(Clock {
            now_ns,
            latency: queued + buffered + delay,
            delay,
            buffered,
            queued,
        })
    }
}

/// What the callback publishes after every cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayPos {
    /// The timeline position presented at `now_ns`.
    pub heard: u64,
    /// In silence at `now_ns`: the frames until the next audio is presented, and its position.
    /// Playing: 0 and `heard`.
    pub hold: u64,
    pub next: u64,
    /// How far the position may be extrapolated: the end of the continuous audio from `next`.
    pub limit: u64,
    /// The report's time; 0 before the first report.
    pub now_ns: i64,
    /// Frames between the write head and the presented sample at `now_ns`.
    pub latency: u64,
    /// Timeline positions handed to PipeWire (or skipped by a flush).
    pub consumed: u64,
    /// Stream frames written, audio and silence.
    pub written: u64,
    /// Frames written in the last cycle.
    pub quantum: u64,
    pub cycles: u64,
    /// The end of the newest audio written.
    pub run_end: u64,
    /// The parts of `latency` (see [`Clock`]).
    pub delay: u64,
    pub buffered: u64,
    pub queued: u64,
    /// Runs the callback's log dropped because it holds [`RUNS`].
    pub evicted: u64,
}

impl PlayPos {
    fn words(&self) -> [u64; 15] {
        [
            self.heard,
            self.hold,
            self.next,
            self.limit,
            self.now_ns as u64,
            self.latency,
            self.consumed,
            self.written,
            self.quantum,
            self.cycles,
            self.run_end,
            self.delay,
            self.buffered,
            self.queued,
            self.evicted,
        ]
    }

    fn from_words(w: [u64; 15]) -> Self {
        Self {
            heard: w[0],
            hold: w[1],
            next: w[2],
            limit: w[3],
            now_ns: w[4] as i64,
            latency: w[5],
            consumed: w[6],
            written: w[7],
            quantum: w[8],
            cycles: w[9],
            run_end: w[10],
            delay: w[11],
            buffered: w[12],
            queued: w[13],
            evicted: w[14],
        }
    }

    /// The timeline position presented at `t_ns` (CLOCK_MONOTONIC): extrapolated from the report
    /// at 24 kHz, through any silence before the next audio, never past the end of what is written.
    pub fn heard_at(&self, t_ns: i64) -> u64 {
        if self.now_ns <= 0 || t_ns <= self.now_ns {
            return self.heard;
        }
        let elapsed = ((t_ns - self.now_ns) as u128 * RATE as u128 / 1_000_000_000) as u64;
        if elapsed < self.hold {
            self.heard
        } else {
            (self.next + (elapsed - self.hold)).min(self.limit.max(self.next))
        }
    }
}

/// CLOCK_MONOTONIC in ns, the clock of `pw_time.now`.
pub fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid out pointer; CLOCK_MONOTONIC always exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Shared between the player, the callback and any reader.
pub struct PlaybackShared {
    pos: SeqCell<15>,
    flush_to: AtomicU64,
    flush_gen: AtomicU64,
    flush_ack: AtomicU64,
    flushed_at: AtomicU64,
    tap: Arc<LatestWindow>,
    closed: AtomicBool,
}

impl PlaybackShared {
    pub fn position(&self) -> PlayPos {
        PlayPos::from_words(self.pos.read())
    }

    /// The stream is gone (closed, or PipeWire went away): the tap reads silence from now on.
    pub fn close(&self) {
        self.tap.reset();
        self.closed.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Run {
    stream: u64,
    ring: u64,
    len: u64,
}

/// The callback's side: owns the ring's reading end. No allocation, no locks, no I/O.
pub struct PlayHead {
    reader: RingReader,
    shared: Arc<PlaybackShared>,
    written: u64,
    consumed: u64,
    flush_seen: u64,
    runs: [Run; RUNS],
    n_runs: usize,
    first: usize,
    evicted: u64,
    last: PlayPos,
}

impl PlayHead {
    /// What the audio thread marks closed when the stream goes.
    pub fn shared(&self) -> Arc<PlaybackShared> {
        self.shared.clone()
    }

    /// One cycle: fills `out` from the ring (silence when it is dry) and publishes the position.
    /// `clock` is this cycle's time report, read before writing.
    pub fn fill(&mut self, out: &mut [i16], clock: Option<Clock>) {
        let generation = self.shared.flush_gen.load(Ordering::Acquire);
        if generation != self.flush_seen {
            let to = self.shared.flush_to.load(Ordering::Relaxed);
            self.reader.discard_all();
            self.shared
                .flushed_at
                .store(self.consumed, Ordering::Relaxed);
            self.consumed = self.consumed.max(to);
            self.flush_seen = generation;
            self.shared.flush_ack.store(generation, Ordering::Release);
        }
        let w0 = self.written;
        let k = self.reader.pop_into(out);
        out[k..].fill(0);
        if k > 0 {
            self.add_run(w0, self.consumed, k as u64);
        }
        self.consumed += k as u64;
        self.written += out.len() as u64;
        self.shared.tap.write(out);

        let mut pos = self.last;
        if let Some(c) = clock {
            let p = self.locate(w0 as i64 - c.latency as i64);
            pos.heard = p.heard;
            pos.hold = p.hold;
            pos.next = p.next;
            pos.limit = p.limit;
            pos.now_ns = c.now_ns;
            pos.latency = c.latency;
            pos.delay = c.delay;
            pos.buffered = c.buffered;
            pos.queued = c.queued;
        }
        pos.consumed = self.consumed;
        pos.written = self.written;
        pos.evicted = self.evicted;
        pos.quantum = out.len() as u64;
        pos.cycles += 1;
        pos.run_end = if self.n_runs > 0 {
            let r = self.runs[(self.first + self.n_runs - 1) % RUNS];
            r.ring + r.len
        } else {
            self.consumed
        };
        self.last = pos;
        self.shared.pos.write(&pos.words());
    }

    fn add_run(&mut self, stream: u64, ring: u64, len: u64) {
        if self.n_runs > 0 {
            let last = &mut self.runs[(self.first + self.n_runs - 1) % RUNS];
            if last.stream + last.len == stream && last.ring + last.len == ring {
                last.len += len;
                return;
            }
        }
        if self.n_runs == RUNS {
            self.first = (self.first + 1) % RUNS;
            self.n_runs -= 1;
            self.evicted += 1;
        }
        self.runs[(self.first + self.n_runs) % RUNS] = Run { stream, ring, len };
        self.n_runs += 1;
    }

    /// What is presented at stream position `h` (negative: before the stream's first frame), and
    /// how it goes on from there.
    fn locate(&self, h: i64) -> Presented {
        let run = |i: usize| self.runs[(self.first + i) % RUNS];
        // The newest run that started at or before h.
        let at = (0..self.n_runs).rev().find(|&i| run(i).stream as i64 <= h);
        if let Some(i) = at {
            let r = run(i);
            if h < (r.stream + r.len) as i64 {
                let p = r.ring + (h - r.stream as i64) as u64;
                return Presented {
                    heard: p,
                    hold: 0,
                    next: p,
                    limit: r.ring + r.len,
                };
            }
        }
        // In silence: after the run before (if any), until the run that comes next (if any).
        let heard = match at {
            Some(i) => run(i).ring + run(i).len,
            None if self.n_runs > 0 => run(0).ring,
            None => self.consumed,
        };
        let next = match at {
            Some(i) if i + 1 < self.n_runs => Some(run(i + 1)),
            Some(_) => None,
            None if self.n_runs > 0 => Some(run(0)),
            None => None,
        };
        match next {
            Some(n) => Presented {
                heard,
                hold: (n.stream as i64 - h).max(0) as u64,
                next: n.ring,
                limit: n.ring + n.len,
            },
            None => Presented {
                heard,
                hold: 0,
                next: heard,
                limit: heard,
            },
        }
    }
}

/// What is presented at a report's time, and how it continues: `hold` frames of silence, then
/// timeline positions from `next` on, continuous up to `limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Presented {
    heard: u64,
    hold: u64,
    next: u64,
    limit: u64,
}

/// An item's counts, in samples at 24 kHz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCount {
    pub item: String,
    /// Samples pushed (a flushed item takes no more).
    pub pushed: u64,
    /// Samples handed to PipeWire.
    pub played: u64,
    /// Samples presented by the device: what the truncate keeps (`audio_end_ms`).
    pub heard: u64,
}

impl ItemCount {
    pub fn heard_ms(&self) -> u64 {
        self.heard * 1000 / RATE as u64
    }
    pub fn played_ms(&self) -> u64 {
        self.played * 1000 / RATE as u64
    }
}

struct Item {
    id: String,
    /// (timeline start, length) of each contiguous part.
    segs: Vec<(u64, u64)>,
    ended: bool,
    /// A flush took it: it takes no more audio.
    flushed: bool,
    /// Where the callback cut it at the flush: nothing from there on plays.
    cut: Option<u64>,
}

impl Item {
    fn count(&self, pos: &PlayPos, heard_at: u64) -> ItemCount {
        let stop = self.cut.unwrap_or(u64::MAX).min(pos.consumed);
        let mut c = ItemCount {
            item: self.id.clone(),
            pushed: 0,
            played: 0,
            heard: 0,
        };
        for &(start, len) in &self.segs {
            let end = (start + len).min(stop);
            c.pushed += len;
            c.played += end.saturating_sub(start);
            c.heard += end.min(heard_at).saturating_sub(start);
        }
        c
    }
}

/// The player's side. Not for the PipeWire thread: it allocates.
pub struct Player {
    writer: RingWriter,
    shared: Arc<PlaybackShared>,
    pending: VecDeque<i16>,
    /// Timeline positions assigned so far.
    pushed: u64,
    items: Vec<Item>,
    /// The flush waiting for the callback's acknowledgement.
    flush: Option<u64>,
    generation: u64,
    /// Audio was left waiting at the last pump (the ring was full then).
    backlog: bool,
    ring_ms: u64,
    /// The callback's run log dropped a run, and this was logged.
    eviction_logged: bool,
    /// The clock of the time reports (`CLOCK_MONOTONIC`; the tests give their fake PipeWire's).
    clock: fn() -> i64,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

/// A player and the head its stream's callback runs.
pub fn player(
    ring: (RingWriter, RingReader),
    ring_ms: u64,
    tap: Arc<LatestWindow>,
) -> (Player, PlayHead) {
    let (writer, reader) = ring;
    let shared = Arc::new(PlaybackShared {
        pos: SeqCell::default(),
        flush_to: AtomicU64::new(0),
        flush_gen: AtomicU64::new(0),
        flush_ack: AtomicU64::new(0),
        flushed_at: AtomicU64::new(0),
        tap,
        closed: AtomicBool::new(false),
    });
    (
        Player {
            writer,
            shared: shared.clone(),
            pending: VecDeque::new(),
            pushed: 0,
            items: Vec::new(),
            flush: None,
            generation: 0,
            backlog: false,
            ring_ms,
            eviction_logged: false,
            clock: monotonic_ns,
            on_drop: None,
        },
        PlayHead {
            reader,
            shared,
            written: 0,
            consumed: 0,
            flush_seen: 0,
            runs: [Run::default(); RUNS],
            n_runs: 0,
            first: 0,
            evicted: 0,
            last: PlayPos::default(),
        },
    )
}

impl Player {
    /// Runs `f` when the player is dropped (the audio thread closes the stream).
    pub fn on_drop(&mut self, f: impl FnOnce() + Send + 'static) {
        self.on_drop = Some(Box::new(f));
    }

    /// The clock the heard position is read at: the one its head's time reports use (tests).
    #[cfg(test)]
    pub fn set_clock(&mut self, clock: fn() -> i64) {
        self.clock = clock;
    }

    /// Queues `pcm` (24 kHz mono) for `item`. A flushed item takes no more audio (false).
    pub fn push(&mut self, item: &str, pcm: &[i16]) -> bool {
        let at = self.pushed;
        let len = pcm.len() as u64;
        match self.items.iter_mut().find(|i| i.id == item) {
            Some(i) if i.flushed || i.ended => return false,
            Some(i) => match i.segs.last_mut() {
                Some((s, l)) if *s + *l == at => *l += len,
                _ => i.segs.push((at, len)),
            },
            None => self.items.push(Item {
                id: item.to_owned(),
                segs: vec![(at, len)],
                ended: false,
                flushed: false,
                cut: None,
            }),
        }
        self.pending.extend(pcm);
        self.pushed += len;
        self.pump();
        true
    }

    /// No more audio for `item`.
    pub fn end(&mut self, item: &str) {
        if let Some(i) = self.items.iter_mut().find(|i| i.id == item) {
            i.ended = true;
        }
    }

    /// Moves queued audio into the ring and takes a flush's acknowledgement. The owner calls it
    /// regularly while audio waits (the session's 20 ms tick); the ring holds
    /// `audio.playback_ring_ms` of audio. Returns a warning when the ring ran dry with audio
    /// waiting, which means a gap was heard.
    pub fn pump(&mut self) -> Option<String> {
        if let Some(generation) = self.flush {
            if self.shared.flush_ack.load(Ordering::Acquire) != generation {
                return None;
            }
            let at = self.shared.flushed_at.load(Ordering::Relaxed);
            for i in &mut self.items {
                if i.flushed && i.cut.is_none() {
                    i.cut = Some(at);
                }
            }
            self.flush = None;
        }
        if !self.eviction_logged {
            let evicted = self.position().evicted;
            if evicted > 0 {
                self.eviction_logged = true;
                tracing::warn!(
                    evicted,
                    "the playback stream's position log dropped its oldest run (it keeps {RUNS} \
                     runs, one per gap of silence); what was heard may jump from here on"
                );
            }
        }
        let mut warning = None;
        if self.backlog && self.writer.free() == self.writer.capacity() {
            warning = Some(format!(
                "the playback ring ran dry while audio waited: a gap was heard (the ring holds \
                 {} ms, audio.playback_ring_ms; the player was not pumped in time)",
                self.ring_ms
            ));
        }
        while !self.pending.is_empty() {
            let (a, _) = self.pending.as_slices();
            let n = self.writer.push_some(a);
            self.pending.drain(..n);
            if n == 0 {
                break;
            }
        }
        self.backlog = !self.pending.is_empty();
        warning
    }

    pub fn position(&self) -> PlayPos {
        self.shared.position()
    }

    /// The stream is gone: nothing more plays (PipeWire went away, or the audio thread ended).
    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// The counts of `item` now.
    pub fn count(&self, item: &str) -> Option<ItemCount> {
        let pos = self.position();
        let heard = pos.heard_at((self.clock)());
        self.items
            .iter()
            .find(|i| i.id == item)
            .map(|i| i.count(&pos, heard))
    }

    /// Every item's counts at the last report, not extrapolated (for a stream that is gone).
    pub fn counts_reported(&self) -> Vec<ItemCount> {
        let pos = self.position();
        self.items
            .iter()
            .map(|i| i.count(&pos, pos.heard))
            .collect()
    }

    pub fn counts(&self) -> Vec<ItemCount> {
        let pos = self.position();
        let heard = pos.heard_at((self.clock)());
        self.items.iter().map(|i| i.count(&pos, heard)).collect()
    }

    /// Everything pushed has been presented (or flushed): the reply's playback is over.
    pub fn idle(&self) -> bool {
        self.idle_at((self.clock)())
    }

    /// [`Player::idle`] at `t_ns` on the reports' clock.
    pub fn idle_at(&self, t_ns: i64) -> bool {
        let pos = self.position();
        self.pending.is_empty()
            && self.flush.is_none()
            && pos.consumed >= self.pushed
            && pos.heard_at(t_ns) >= pos.run_end
    }

    /// Stops playback: the queue goes now, the ring at the callback's next cycle. Every item so
    /// far is flushed and takes no more audio.
    pub fn flush(&mut self) {
        for i in &mut self.items {
            i.flushed = true;
        }
        self.pending.clear();
        self.backlog = false;
        self.generation += 1;
        self.shared.flush_to.store(self.pushed, Ordering::Relaxed);
        self.shared
            .flush_gen
            .store(self.generation, Ordering::Release);
        self.flush = Some(self.generation);
    }

    /// [`Player::flush`], then the counts once the callback took it, or after `wait`
    /// (`audio.flush_wait_ms`) with what the last cycle reported; the second element then says so.
    pub async fn flushed(&mut self, wait: Duration) -> (Vec<ItemCount>, Option<String>) {
        self.flush();
        // tokio's clock: the bound and the 2 ms polls run on the same clock (a paused one in the
        // tests).
        let t0 = tokio::time::Instant::now();
        loop {
            self.pump();
            if self.flush.is_none() {
                return (self.counts(), None);
            }
            if t0.elapsed() >= wait {
                let note = format!(
                    "the playback stream did not take the flush within {} ms \
                     (audio.flush_wait_ms); the counts are from its last cycle",
                    wait.as_millis()
                );
                return (self.counts(), Some(note));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// Forgets `item` (its counts are settled).
    pub fn forget(&mut self, item: &str) {
        self.items.retain(|i| i.id != item);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::sample_ring;

    fn rig(ring: usize) -> (Player, PlayHead, Arc<LatestWindow>) {
        let tap = Arc::new(LatestWindow::new(4096));
        let (p, h) = player(sample_ring(ring), 1000, tap.clone());
        (p, h, tap)
    }

    fn clock(now_ns: i64, latency: u64) -> Option<Clock> {
        Some(Clock {
            now_ns,
            latency,
            ..Clock::default()
        })
    }

    const MS: i64 = 1_000_000;

    #[test]
    fn the_time_report_converts_to_stream_frames() {
        // 48 kHz graph, a delay of 1024 ticks = 512 frames at 24 kHz, plus queue and resampler.
        let c = Clock::from_time(5, 1, 48_000, 1024, 100, 7).unwrap();
        assert_eq!(c.latency, 512 + 100 + 7);
        assert_eq!((c.delay, c.queued, c.buffered), (512, 100, 7));
        assert_eq!(Clock::from_time(5, 1, 48_000, -3, 0, 0).unwrap().latency, 0);
        assert_eq!(Clock::from_time(5, 1, 0, 1, 0, 0), None);
        assert_eq!(Clock::from_time(0, 1, 48_000, 1, 0, 0), None);
    }

    #[test]
    fn a_reply_after_silence_is_heard_from_the_moment_it_is_presented() {
        let (mut p, mut h, _) = rig(4800);
        let mut out = [0i16; 100];
        h.fill(&mut out, clock(MS, 30)); // silence
        p.push("a", &[1; 300]);
        // Written at stream 100..200; with 30 of latency stream 70 is presented: 30 to go.
        h.fill(&mut out, clock(2 * MS, 30));
        let pos = p.position();
        assert_eq!((pos.heard, pos.hold, pos.next, pos.limit), (0, 30, 0, 100));
        assert_eq!(pos.heard_at(2 * MS + 1_000_000), 0, "24 frames on: not yet");
        assert_eq!(
            pos.heard_at(2 * MS + 2_000_000),
            18,
            "48 frames on: 18 into a"
        );
        assert_eq!(pos.heard_at(9 * MS), 100, "never past what is written");
    }

    #[test]
    fn heard_is_written_minus_the_delay_and_extrapolates_to_the_end_of_the_audio() {
        let (mut p, mut h, _) = rig(4800);
        p.push("a", &[1; 1000]);
        let mut out = [0i16; 240];
        // Cycle 1 at t=10 ms: nothing written before, latency 300: nothing heard.
        h.fill(&mut out, clock(10 * MS, 300));
        assert_eq!(out, [1; 240]);
        assert_eq!(p.position().heard, 0);
        // Cycle 2 at t=20 ms: 240 written before; with 300 of latency still nothing presented.
        h.fill(&mut out, clock(20 * MS, 300));
        assert_eq!(p.position().heard, 0);
        // Cycle 3 at t=30 ms: 480 written; the sample at 480-300 = 180 is presented.
        h.fill(&mut out, clock(30 * MS, 300));
        let pos = p.position();
        assert_eq!((pos.heard, pos.limit, pos.consumed), (180, 720, 720));
        // 5 ms later: +120 samples at 24 kHz.
        assert_eq!(pos.heard_at(35 * MS), 300);
        // Never past the end of the written audio.
        assert_eq!(pos.heard_at(1_000 * MS), 720);
        // The item's counts as the reader sees them.
        let c = p.items[0].count(&pos, pos.heard_at(35 * MS));
        assert_eq!((c.pushed, c.played, c.heard), (1000, 720, 300));
        assert_eq!(c.heard_ms(), 12);
    }

    #[test]
    fn silence_between_items_is_not_heard_as_audio() {
        let (mut p, mut h, _) = rig(4800);
        let mut out = [0i16; 100];
        p.push("a", &[5; 150]);
        h.fill(&mut out, clock(MS, 0)); // a[0..100]
        h.fill(&mut out, clock(2 * MS, 0)); // a[100..150] + 50 silence
        assert_eq!(&out[..50], &[5; 50]);
        assert_eq!(&out[50..], &[0; 50]);
        h.fill(&mut out, clock(3 * MS, 0)); // silence
        p.push("b", &[7; 100]);
        h.fill(&mut out, clock(4 * MS, 0)); // b[0..100] at stream 300..400
        // Latency 120 at stream position 400: presented is stream 280, in the silence after a;
        // b starts at stream 300, 20 frames later.
        h.fill(&mut out, clock(5 * MS, 120));
        let pos = p.position();
        assert_eq!(
            (pos.heard, pos.hold, pos.next, pos.limit),
            (150, 20, 150, 250),
            "a is fully heard, b comes after 20 frames of silence"
        );
        let at = |t| {
            (
                p.items[0].count(&pos, pos.heard_at(t)),
                p.items[1].count(&pos, pos.heard_at(t)),
            )
        };
        let (a, b) = at(5 * MS + 800_000); // 19 frames on: still silence
        assert_eq!((a.played, a.heard, b.played, b.heard), (150, 150, 100, 0));
        let (_, b) = at(6 * MS); // 24 frames on: 4 frames into b
        assert_eq!(b.heard, 4);
        // Next cycle: stream 500 - 120 = 380, 80 samples into b.
        h.fill(&mut out, clock(6 * MS, 120));
        assert_eq!(p.position().heard, 230);
        // Read now (the real clock, long after these reports): extrapolated to the end of b.
        assert_eq!(p.count("b").unwrap().heard, 100);
    }

    #[test]
    fn a_flush_cuts_the_items_and_keeps_the_timeline_aligned() {
        let (mut p, mut h, _) = rig(300);
        let mut out = [0i16; 100];
        p.push("a", &[1; 1000]); // 300 into the ring, 700 waiting
        h.fill(&mut out, clock(MS, 50));
        assert_eq!(p.position().consumed, 100);
        p.flush();
        // Pushes for the flushed item are refused once the flush is taken; a new item waits.
        assert!(p.push("b", &[2; 200]));
        assert_eq!(p.pump(), None);
        assert!(p.flush.is_some(), "not taken yet");
        h.fill(&mut out, clock(2 * MS, 50));
        assert_eq!(
            out, [0; 100],
            "the ring was dropped; b is still in the queue"
        );
        p.pump();
        assert!(p.flush.is_none());
        assert!(!p.push("a", &[1; 10]), "a flushed item takes no more");
        let a = p.count("a").unwrap();
        assert_eq!((a.pushed, a.played), (1000, 100));
        // b starts right after the flush point on the timeline: position 1000.
        h.fill(&mut out, clock(3 * MS, 0));
        assert_eq!(out, [2; 100]);
        let pos = p.position();
        assert_eq!(pos.consumed, 1100);
        // Latency 0 at stream 200: the presented sample is b's first.
        assert_eq!(pos.heard, 1000);
        let b = p.count("b").unwrap();
        assert_eq!((b.pushed, b.played), (200, 100));
        // a never counts as more heard than played, however far the timeline goes.
        assert_eq!(p.items[0].count(&pos, u64::MAX).heard, 100);
    }

    #[tokio::test]
    async fn flushed_waits_for_the_callback_and_says_so_when_it_does_not_come() {
        let (mut p, mut h, _) = rig(1000);
        let mut out = [0i16; 100];
        p.push("a", &[1; 500]);
        h.fill(&mut out, clock(MS, 0));
        let (counts, note) = p.flushed(Duration::from_millis(20)).await;
        assert!(note.unwrap().contains("audio.flush_wait_ms"));
        assert_eq!(counts[0].played, 100);
        // The callback takes it later; the cut lands where it consumed up to.
        h.fill(&mut out, clock(2 * MS, 0));
        p.pump();
        assert_eq!(p.items[0].cut, Some(100));
        // With a running callback it answers without the note.
        let (mut p, mut h, _) = rig(1000);
        p.push("a", &[1; 500]);
        let task = tokio::spawn(async move {
            let r = p.flushed(Duration::from_secs(5)).await;
            (r, p)
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        h.fill(&mut out, clock(MS, 0));
        let ((counts, note), _p) = task.await.unwrap();
        assert_eq!(note, None);
        assert_eq!((counts[0].pushed, counts[0].played), (500, 0));
    }

    #[test]
    fn the_queue_is_unbounded_and_a_dry_ring_with_audio_waiting_is_reported() {
        let (mut p, mut h, _) = rig(200);
        p.push("a", &[3; 1000]);
        assert_eq!(p.pending.len(), 800);
        let mut out = [0i16; 200];
        h.fill(&mut out, clock(MS, 0));
        let w = p.pump().expect("the ring ran dry with 800 waiting");
        assert!(w.contains("audio.playback_ring_ms"), "{w}");
        assert_eq!(p.pending.len(), 600);
        h.fill(&mut out[..150], clock(2 * MS, 0)); // 50 left in the ring
        assert_eq!(p.pump(), None);
        assert_eq!(p.pending.len(), 450);
    }

    #[test]
    fn idle_once_everything_is_presented() {
        let (mut p, mut h, _) = rig(1000);
        assert!(p.idle_at(MS), "nothing pushed");
        p.push("a", &[1; 150]);
        assert!(!p.idle_at(MS));
        // Reports at 1, 2 and 3 ms, read at the third report's time: no extrapolation.
        let mut out = [0i16; 100];
        h.fill(&mut out, clock(MS, 100));
        h.fill(&mut out, clock(2 * MS, 100));
        h.fill(&mut out, clock(3 * MS, 100));
        assert!(
            !p.idle_at(3 * MS),
            "written; the presented sample is at 100 of 150"
        );
        // 2 ms on (48 frames) it is still within the audio; 3 ms on it is past it.
        assert!(!p.idle_at(5 * MS));
        assert!(p.idle_at(6 * MS));
        // Stream position 300 - 100 presents 200: past the 150 of audio.
        h.fill(&mut out, clock(4 * MS, 100));
        assert!(p.idle_at(4 * MS));
    }

    #[test]
    fn a_full_run_log_counts_what_it_drops() {
        let (mut p, mut h, _) = rig(4800);
        let mut out = [0i16; 2];
        // Each push is followed by a cycle that runs dry: every item is a run of its own.
        for k in 0..(RUNS + 3) {
            p.push(&format!("i{k}"), &[1]);
            h.fill(&mut out, None);
        }
        assert_eq!(p.position().evicted, 3);
        assert!(p.eviction_logged, "the player's pump logged it, once");
    }

    #[test]
    fn the_output_tap_sees_what_is_written() {
        let (mut p, mut h, tap) = rig(1000);
        let since = tap.subscribe();
        p.push("a", &[16384; 10]);
        let mut out = [0i16; 20];
        h.fill(&mut out, None);
        let mut got = [9.0f32; 20];
        tap.latest(since, &mut got);
        assert_eq!(&got[..10], &[0.5; 10]);
        assert_eq!(&got[10..], &[0.0; 10]);
        assert_eq!(p.position().cycles, 1);
        assert_eq!(p.position().now_ns, 0, "no report, no heard");
    }

    #[test]
    fn dropping_the_player_runs_its_close() {
        let (mut p, _h, _) = rig(10);
        let (tx, rx) = std::sync::mpsc::channel();
        p.on_drop(move || tx.send(()).unwrap());
        drop(p);
        assert!(rx.try_recv().is_ok());
    }
}
