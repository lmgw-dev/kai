//! Capture (§6.1): the microphone at 24 kHz mono, from the PipeWire callback into a ring the
//! session (WP4) drains into its 20 ms chunks and its buffer before `session.created` (§5.3).

// The voice session (WP4) opens the capture; until then the tests and the live check do.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::RATE;
use super::ring::{LatestWindow, RingReader, RingWriter};

/// The callback's side. No allocation, no locks, no I/O.
pub struct CaptureHead {
    writer: RingWriter,
    shared: Arc<CaptureShared>,
}

/// Shared between the session's side and the audio thread.
pub struct CaptureShared {
    tap: Arc<LatestWindow>,
    closed: AtomicBool,
    /// Samples in the last callback: the stream's quantum at Kai's rate.
    cycle: AtomicUsize,
}

impl CaptureShared {
    /// The stream is gone: the input tap reads silence from now on.
    pub fn close(&self) {
        self.tap.reset();
        self.closed.store(true, Ordering::Release);
    }
}

impl CaptureHead {
    /// One cycle's samples: into the ring (what does not fit is dropped and counted) and, while a
    /// level reader wants them, into the input tap.
    pub fn take(&mut self, samples: &[i16]) {
        self.writer.push_or_drop(samples);
        self.shared.tap.write(samples);
        self.shared.cycle.store(samples.len(), Ordering::Relaxed);
    }

    pub fn shared(&self) -> Arc<CaptureShared> {
        self.shared.clone()
    }
}

/// The session's side. Dropping it closes the capture stream (the microphone is free then).
pub struct Capture {
    reader: RingReader,
    reported: u64,
    ring_ms: u64,
    shared: Arc<CaptureShared>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

pub fn capture(
    ring: (RingWriter, RingReader),
    ring_ms: u64,
    tap: Arc<LatestWindow>,
) -> (Capture, CaptureHead) {
    let (writer, reader) = ring;
    let shared = Arc::new(CaptureShared {
        tap,
        closed: AtomicBool::new(false),
        cycle: AtomicUsize::new(0),
    });
    (
        Capture {
            reader,
            reported: 0,
            ring_ms,
            shared: shared.clone(),
            on_drop: None,
        },
        CaptureHead { writer, shared },
    )
}

impl Capture {
    pub fn on_drop(&mut self, f: impl FnOnce() + Send + 'static) {
        self.on_drop = Some(Box::new(f));
    }

    /// The stream is gone (PipeWire went away, or the audio thread ended): nothing more comes.
    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Samples per PipeWire cycle, as the last callback brought them (0 before the first): the
    /// capture's quantum at 24 kHz.
    pub fn cycle(&self) -> usize {
        self.shared.cycle.load(Ordering::Relaxed)
    }

    /// Appends everything captured since the last call (PCM16, 24 kHz mono) to `out`.
    pub fn read(&mut self, out: &mut Vec<i16>) -> usize {
        self.reader.drain_into(out)
    }

    /// Audio dropped since the last call because the session did not drain the ring in time, as
    /// the error line that names the setting.
    pub fn overflow(&mut self) -> Option<String> {
        let dropped = self.reader.dropped();
        let new = dropped - self.reported;
        self.reported = dropped;
        (new > 0).then(|| {
            format!(
                "the capture ring overflowed: {} ms of microphone audio was dropped (the ring \
                 holds {} ms, audio.capture_ring_ms)",
                new * 1000 / RATE as u64,
                self.ring_ms
            )
        })
    }
}

impl Drop for Capture {
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

    #[test]
    fn captured_audio_reaches_the_session_and_an_overflow_names_the_setting() {
        let tap = Arc::new(LatestWindow::new(4096));
        let (mut c, mut h) = capture(sample_ring(480), 20, tap.clone());
        h.take(&[1; 240]);
        let mut out = Vec::new();
        assert_eq!(c.read(&mut out), 240);
        assert_eq!(c.overflow(), None);
        h.take(&[2; 480]);
        h.take(&[3; 240]); // 240 more than the ring holds
        let e = c.overflow().unwrap();
        assert!(
            e.contains("10 ms") && e.contains("audio.capture_ring_ms"),
            "{e}"
        );
        assert_eq!(c.overflow(), None, "reported once");
        out.clear();
        c.read(&mut out);
        assert_eq!(out, vec![2; 480]);
    }

    #[test]
    fn the_input_tap_gets_samples_only_while_wanted() {
        let tap = Arc::new(LatestWindow::new(64));
        let (_c, mut h) = capture(sample_ring(100), 4, tap.clone());
        let since = tap.subscribe();
        h.take(&[16384; 4]);
        let mut got = [0f32; 4];
        tap.latest(since, &mut got);
        assert_eq!(got, [0.5; 4]);
    }
}
