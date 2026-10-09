//! The lock-free structures between the PipeWire callbacks and the rest of Kai (§2): the callbacks
//! only move samples through these, with no allocation, no locks and no I/O.
//!
//! - [`SampleRing`]: a wait-free single-producer single-consumer queue of samples (rtrb) with an
//!   overflow count, for the capture (callback → session) and the playback (player → callback).
//! - [`LatestWindow`]: the newest samples of one side for the level analysers; written only while a
//!   reader wants them, never blocks the writer and never needs draining.
//! - [`SeqCell`]: a small record the playback callback publishes every cycle (the heard position),
//!   read consistently from any thread; the writer never waits.

use std::sync::Arc;
use std::sync::atomic::{AtomicI16, AtomicU64, AtomicUsize, Ordering, fence};

/// Creates a sample ring of `capacity` samples.
pub fn sample_ring(capacity: usize) -> (RingWriter, RingReader) {
    let (tx, rx) = rtrb::RingBuffer::new(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        RingWriter {
            tx,
            dropped: dropped.clone(),
        },
        RingReader { rx, dropped },
    )
}

/// The producing end. Samples that do not fit are dropped and counted, never waited for.
pub struct RingWriter {
    tx: rtrb::Producer<i16>,
    dropped: Arc<AtomicU64>,
}

impl RingWriter {
    /// Pushes as many samples as fit and returns how many; the rest count as dropped.
    pub fn push_or_drop(&mut self, samples: &[i16]) -> usize {
        let (done, rest) = self.tx.push_partial_slice(samples);
        if !rest.is_empty() {
            self.dropped.fetch_add(rest.len() as u64, Ordering::Relaxed);
        }
        done.len()
    }

    /// Pushes as many samples as fit and returns how many; the caller keeps the rest (no drop).
    pub fn push_some(&mut self, samples: &[i16]) -> usize {
        self.tx.push_partial_slice(samples).0.len()
    }

    pub fn free(&self) -> usize {
        self.tx.slots()
    }

    pub fn capacity(&self) -> usize {
        self.tx.buffer().capacity()
    }
}

/// The consuming end.
pub struct RingReader {
    rx: rtrb::Consumer<i16>,
    dropped: Arc<AtomicU64>,
}

impl RingReader {
    /// Pops up to `out.len()` samples into `out` and returns how many.
    pub fn pop_into(&mut self, out: &mut [i16]) -> usize {
        self.rx.pop_partial_slice(out).0.len()
    }

    /// Appends everything queued to `out` (not for the PipeWire thread: it may grow `out`).
    pub fn drain_into(&mut self, out: &mut Vec<i16>) -> usize {
        let n = self.rx.slots();
        if let Ok(chunk) = self.rx.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            chunk.commit_all();
        }
        n
    }

    /// Discards everything queued and returns how many samples that was.
    pub fn discard_all(&mut self) -> usize {
        let n = self.rx.slots();
        if let Ok(chunk) = self.rx.read_chunk(n) {
            chunk.commit_all();
        }
        n
    }

    /// Samples dropped so far because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// The newest samples of one side, for readers that want only "the latest window".
///
/// The writer (one PipeWire callback) appends only while [`LatestWindow::wanted`]: one relaxed load
/// per cycle otherwise. Readers copy the newest samples whenever they like; nothing is drained, and
/// the writer never waits for them.
///
/// A write publishes where it will end (`pending`) before it stores a sample, and where it ended
/// (`written`) after; a reader copies the newest samples up to `written`, then checks `pending`. A
/// write that reached into the copied slots, finished or still running, ends past the copy's start
/// plus the capacity, so the reader sees it and copies again (a sequence lock, with `pending` as its
/// sequence): no copy mixes old and new samples.
pub struct LatestWindow {
    buf: Box<[AtomicI16]>,
    mask: usize,
    written: AtomicU64,
    /// The end of the write in progress, or of the last one.
    pending: AtomicU64,
    /// Samples before this count as silence: they are older than the stream's last stop.
    floor: AtomicU64,
    readers: AtomicUsize,
}

impl LatestWindow {
    /// `capacity` is rounded up to a power of two.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        Self {
            buf: (0..cap).map(|_| AtomicI16::new(0)).collect(),
            mask: cap - 1,
            written: AtomicU64::new(0),
            pending: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            readers: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn wanted(&self) -> bool {
        self.readers.load(Ordering::Relaxed) > 0
    }

    /// The writer: appends `samples` if anyone reads. One writer at a time.
    pub fn write(&self, samples: &[i16]) {
        if !self.wanted() {
            return;
        }
        let w = self.begin(samples.len());
        self.put(w, samples);
        self.finish(w + samples.len() as u64);
    }

    /// A write of `len` samples starts: its end is published before any sample is stored.
    fn begin(&self, len: usize) -> u64 {
        let w = self.written.load(Ordering::Relaxed);
        self.pending.store(w + len as u64, Ordering::Relaxed);
        fence(Ordering::Release);
        w
    }

    fn put(&self, w: u64, samples: &[i16]) {
        for (i, &s) in samples.iter().enumerate() {
            self.buf[(w as usize).wrapping_add(i) & self.mask].store(s, Ordering::Relaxed);
        }
    }

    fn finish(&self, end: u64) {
        self.written.store(end, Ordering::Release);
    }

    /// Everything written so far counts as silence from now on (the stream stopped: a reader must
    /// not keep showing its last sound).
    pub fn reset(&self) {
        self.floor
            .store(self.written.load(Ordering::Acquire), Ordering::Release);
    }

    /// A reader starts; returns the position before which samples count as silence for it.
    pub fn subscribe(&self) -> u64 {
        self.readers.fetch_add(1, Ordering::Relaxed);
        self.written.load(Ordering::Acquire)
    }

    pub fn unsubscribe(&self) {
        self.readers.fetch_sub(1, Ordering::Relaxed);
    }

    /// Copies the newest `out.len()` samples, oldest first, scaled to ±1. Samples before `since`
    /// (or before the last [`LatestWindow::reset`]) read as 0.
    pub fn latest(&self, since: u64, out: &mut [f32]) {
        while !self.try_latest(since, out) {
            std::hint::spin_loop();
        }
    }

    /// One attempt of [`LatestWindow::latest`]: false when a write reached into the copied slots
    /// (the copy may mix old and new samples).
    fn try_latest(&self, since: u64, out: &mut [f32]) -> bool {
        let n = out.len() as u64;
        assert!(n as usize <= self.capacity() / 2);
        let end = self.written.load(Ordering::Acquire);
        let floor = since.max(self.floor.load(Ordering::Acquire));
        for (i, o) in out.iter_mut().enumerate() {
            // The sample at `end - n + i`, if it exists and is not older than the floor.
            *o = match (end + i as u64).checked_sub(n) {
                Some(pos) if pos >= floor => {
                    self.buf[pos as usize & self.mask].load(Ordering::Relaxed) as f32 / 32768.0
                }
                _ => 0.0,
            };
        }
        fence(Ordering::Acquire);
        // A write that overwrote a copied slot (position p + capacity or later) ends past this.
        let first = end.saturating_sub(n);
        self.pending.load(Ordering::Relaxed) <= first + self.capacity() as u64
    }
}

/// A record of `N` words, published by one writer and read consistently by many (a sequence
/// lock over atomics). The writer never waits; a reader that raced a write tries again.
pub struct SeqCell<const N: usize> {
    seq: AtomicU64,
    words: [AtomicU64; N],
}

impl<const N: usize> Default for SeqCell<N> {
    fn default() -> Self {
        Self {
            seq: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl<const N: usize> SeqCell<N> {
    /// One writer at a time.
    pub fn write(&self, words: &[u64; N]) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (a, &w) in self.words.iter().zip(words) {
            a.store(w, Ordering::Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    pub fn read(&self) -> [u64; N] {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let words = std::array::from_fn(|i| self.words[i].load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return words;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_ring_drops_and_counts_and_keeps_order() {
        let (mut w, mut r) = sample_ring(8);
        assert_eq!(w.capacity(), 8);
        assert_eq!(w.push_or_drop(&[1, 2, 3, 4, 5]), 5);
        assert_eq!(w.push_or_drop(&[6, 7, 8, 9, 10]), 3);
        assert_eq!(r.dropped(), 2);
        let mut out = [0; 3];
        assert_eq!(r.pop_into(&mut out), 3);
        assert_eq!(out, [1, 2, 3]);
        let mut all = Vec::new();
        assert_eq!(r.drain_into(&mut all), 5);
        assert_eq!(all, [4, 5, 6, 7, 8]);
        assert_eq!(r.pop_into(&mut out), 0);
    }

    #[test]
    fn push_some_never_drops() {
        let (mut w, mut r) = sample_ring(4);
        assert_eq!(w.push_some(&[1, 2, 3, 4, 5, 6]), 4);
        assert_eq!(w.free(), 0);
        assert_eq!(r.dropped(), 0);
        assert_eq!(r.discard_all(), 4);
        assert_eq!(w.free(), 4);
    }

    #[test]
    fn the_ring_carries_every_sample_across_threads_in_order() {
        let (mut w, mut r) = sample_ring(64);
        const N: i32 = 200_000;
        let producer = std::thread::spawn(move || {
            let mut next = 0i32;
            while next < N {
                let chunk: Vec<i16> = (next..(next + 37).min(N)).map(|v| v as i16).collect();
                let mut sent = 0;
                while sent < chunk.len() {
                    sent += w.push_some(&chunk[sent..]);
                    std::hint::spin_loop();
                }
                next += chunk.len() as i32;
            }
        });
        let mut got = 0i32;
        let mut buf = [0i16; 29];
        while got < N {
            let n = r.pop_into(&mut buf);
            for &s in &buf[..n] {
                assert_eq!(s, got as i16);
                got += 1;
            }
        }
        producer.join().unwrap();
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn the_window_is_written_only_while_wanted() {
        let w = LatestWindow::new(16);
        let mut out = [9.0f32; 4];
        w.write(&[100; 4]);
        let since = w.subscribe();
        w.latest(since, &mut out);
        assert_eq!(
            out, [0.0; 4],
            "nothing written for nobody, nothing before the reader"
        );
        w.write(&[16384, -16384]);
        w.latest(since, &mut out);
        assert_eq!(out, [0.0, 0.0, 0.5, -0.5]);
        w.write(&[1, 2, 3, 4, 5, 6, 7]);
        w.latest(since, &mut out);
        let s = 1.0 / 32768.0;
        assert_eq!(out, [4.0 * s, 5.0 * s, 6.0 * s, 7.0 * s]);
        w.reset();
        w.latest(since, &mut out);
        assert_eq!(out, [0.0; 4], "a stopped stream reads as silence");
        w.write(&[32767]);
        w.latest(since, &mut out);
        assert_eq!(out[3], 32767.0 / 32768.0);
        w.unsubscribe();
        assert!(!w.wanted());
    }

    #[test]
    fn the_window_wraps() {
        let w = LatestWindow::new(8);
        let since = w.subscribe();
        for k in 0..5i16 {
            w.write(&[k * 3, k * 3 + 1, k * 3 + 2]);
        }
        let mut out = [0f32; 4];
        w.latest(since, &mut out);
        let got: Vec<i16> = out.iter().map(|v| (v * 32768.0) as i16).collect();
        assert_eq!(got, [11, 12, 13, 14]);
    }

    fn as_samples(out: &[f32]) -> Vec<i16> {
        out.iter().map(|v| (v * 32768.0).round() as i16).collect()
    }

    #[test]
    fn a_copy_that_a_running_write_reached_into_is_taken_again() {
        // Deterministic: the write is stopped half way, as a descheduled writer would be.
        let w = LatestWindow::new(8);
        let since = w.subscribe();
        w.write(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut out = [0f32; 4];
        assert!(w.try_latest(since, &mut out));
        assert_eq!(as_samples(&out), [5, 6, 7, 8]);
        // A write of positions 8..13 begins; its samples land in the slots of 0..5, which hold the
        // copied position 4. Half of it is stored, `written` still says 8.
        let start = w.begin(5);
        w.put(start, &[100, 101, 102]);
        w.put(start + 3, &[103, 104]);
        assert!(
            !w.try_latest(since, &mut out),
            "the copy of 4..8 may hold position 12's sample: taken again"
        );
        w.finish(start + 5);
        assert!(w.try_latest(since, &mut out));
        assert_eq!(as_samples(&out), [101, 102, 103, 104]);
        // A running write that stays clear of the copied slots does not disturb the copy.
        let start = w.begin(2);
        w.put(start, &[200]);
        assert!(w.try_latest(since, &mut out));
        assert_eq!(as_samples(&out), [101, 102, 103, 104]);
        w.put(start + 1, &[201]);
        w.finish(start + 2);
        assert!(w.try_latest(since, &mut out));
        assert_eq!(as_samples(&out), [103, 104, 200, 201]);
    }

    #[test]
    fn a_window_reader_never_sees_a_torn_copy() {
        // A stress run across threads: every copy must be a piece of one ramp. (With the check
        // above this holds for any interleaving; the deterministic test pins the mechanism.)
        // The writer writes one long ramp; every copy must be a piece of it.
        let w = Arc::new(LatestWindow::new(64));
        let since = w.subscribe();
        let ramp = |v: i16| -> [i16; 5] { std::array::from_fn(|i| v.wrapping_add(i as i16)) };
        let mut v = 0i16;
        for _ in 0..10 {
            w.write(&ramp(v));
            v = v.wrapping_add(5);
        }
        let writer = {
            let w = w.clone();
            std::thread::spawn(move || {
                for _ in 0..200_000 {
                    w.write(&ramp(v));
                    v = v.wrapping_add(5);
                }
            })
        };
        let mut out = [0f32; 32];
        let mut copies = 0;
        while !writer.is_finished() || copies == 0 {
            w.latest(since, &mut out);
            let s: Vec<i16> = out.iter().map(|v| (v * 32768.0) as i16).collect();
            for k in 1..32 {
                assert_eq!(s[k], s[k - 1].wrapping_add(1), "{s:?}");
            }
            copies += 1;
        }
        writer.join().unwrap();
    }

    #[test]
    fn a_seqcell_reader_sees_whole_records_only() {
        let cell = Arc::new(SeqCell::<4>::default());
        cell.write(&[0, 0, 0, 0xdead]);
        let writer = {
            let cell = cell.clone();
            std::thread::spawn(move || {
                for k in 1..300_000u64 {
                    cell.write(&[k, k * 2, k * 3, k ^ 0xdead]);
                }
            })
        };
        let mut seen = 0;
        while !writer.is_finished() {
            let [a, b, c, d] = cell.read();
            assert_eq!((b, c, d), (a * 2, a * 3, a ^ 0xdead));
            seen += 1;
        }
        writer.join().unwrap();
        assert!(seen > 0);
        assert_eq!(cell.read()[0], 299_999);
    }
}
