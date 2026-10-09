//! Audio (§6, WP3): PipeWire capture and playback at 24 kHz mono, device choice, the four echo
//! modes with app AEC under K20, levels and bands for the ring, and the heard position.
//!
//! [`Audio`] starts the PipeWire loop thread and is the handle to it. It reports devices, the echo
//! status and the PipeWire link as [`AudioEvent`]s. The voice session (WP4) opens a [`Player`] and
//! a [`Capture`] for as long as it needs them; the ring (WP5) reads [`LevelsReader`]s from
//! [`LevelTaps`].

mod aec;
mod capture;
mod devices;
mod echo;
mod fft;
pub mod levels;
mod playback;
mod pw;
mod ring;

#[cfg(test)]
pub(crate) mod live_tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

pub use capture::Capture;
pub use devices::{Device, DeviceList};
pub use echo::{EchoMode, EchoStatus};
pub use levels::{Levels, LevelsReader, Tap};
#[allow(unused_imports)] // the live checks read the counts
pub use playback::{ItemCount, PlayPos, Player};

use pw::Cmd;
use ring::{LatestWindow, sample_ring};

/// Both streams' rate (§6.1, the realtime API's PCM16).
pub const RATE: u32 = 24_000;

/// What the audio thread needs from the configuration and the paths.
#[derive(Debug, Clone)]
pub struct AudioSettings {
    pub echo_mode: EchoMode,
    /// The picked microphone's `node.name`; `None`: the system default.
    pub input: Option<String>,
    pub output: Option<String>,
    pub capture_ring_ms: u64,
    pub playback_ring_ms: u64,
    pub retry: Duration,
    pub aec_ready_timeout: Duration,
    pub aec_restore_timeout: Duration,
    /// `$XDG_STATE_HOME/kai/aec.json`.
    pub aec_file: PathBuf,
    /// Where the echo-cancel child's configuration is written (`$XDG_RUNTIME_DIR/kai`).
    pub runtime_dir: PathBuf,
}

/// What the audio thread reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    /// The devices or the defaults changed.
    Devices(DeviceList),
    /// The echo status changed.
    Echo(EchoStatus),
    /// The PipeWire connection: `None` connected, else why not and when it retries.
    Link(Option<String>),
    /// A line to log and show once (a stream error, an AEC restore).
    Notice(String),
}

/// The tap rings hold this many samples: twice the largest analyser window.
const TAP_SAMPLES: usize = 2 * levels::signal::FFT_SIZE;

fn samples_for_ms(ms: u64) -> usize {
    (ms * RATE as u64 / 1000) as usize
}

pub struct Audio {
    tx: ::pipewire::channel::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
    input_tap: Arc<LatestWindow>,
    output_tap: Arc<LatestWindow>,
    capture_ring_ms: u64,
    playback_ring_ms: u64,
    /// Ids for opened streams, so a late close never closes a newer stream.
    next_id: AtomicU64,
}

impl Audio {
    /// Starts the PipeWire thread. It connects (retrying every `audio.retry_s`), restores defaults a
    /// crashed run left flipped, and brings up app AEC when that is the mode.
    pub fn start(
        settings: AudioSettings,
        events: UnboundedSender<AudioEvent>,
    ) -> std::io::Result<Audio> {
        let (tx, rx) = ::pipewire::channel::channel();
        let capture_ring_ms = settings.capture_ring_ms;
        let playback_ring_ms = settings.playback_ring_ms;
        // The echo-cancel child is spawned from this thread, and dies with it (PR_SET_PDEATHSIG):
        // it lives until Kai quits.
        let thread = std::thread::Builder::new()
            .name("kai-pipewire".into())
            .spawn(move || pw::run(settings, events, rx))?;
        Ok(Audio {
            tx,
            thread: Some(thread),
            input_tap: Arc::new(LatestWindow::new(TAP_SAMPLES)),
            output_tap: Arc::new(LatestWindow::new(TAP_SAMPLES)),
            capture_ring_ms,
            playback_ring_ms,
            next_id: AtomicU64::new(1),
        })
    }

    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            tracing::warn!("the audio thread is gone");
        }
    }

    pub fn set_echo_mode(&self, mode: EchoMode) {
        self.send(Cmd::SetEcho(mode));
    }

    pub fn set_input(&self, node: Option<String>) {
        self.send(Cmd::SetInput(node));
    }

    pub fn set_output(&self, node: Option<String>) {
        self.send(Cmd::SetOutput(node));
    }

    /// Opens the playback stream; dropping the player closes it. A second open replaces the first
    /// (whose player then reads `closed()`). If PipeWire goes away, the player reads `closed()` too
    /// and the owner opens a new one once the link is back.
    pub fn open_playback(&self) -> Player {
        let ring = sample_ring(samples_for_ms(self.playback_ring_ms));
        let (mut player, head) =
            playback::player(ring, self.playback_ring_ms, self.output_tap.clone());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let tx = self.tx.clone();
        player.on_drop(move || {
            let _ = tx.send(Cmd::ClosePlayback(id));
        });
        self.send(Cmd::OpenPlayback(Box::new(head), id));
        player
    }

    /// Opens the capture stream (the microphone is in use from now on); dropping it closes it.
    pub fn open_capture(&self) -> Capture {
        let ring = sample_ring(samples_for_ms(self.capture_ring_ms));
        let (mut capture, head) =
            capture::capture(ring, self.capture_ring_ms, self.input_tap.clone());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let tx = self.tx.clone();
        capture.on_drop(move || {
            let _ = tx.send(Cmd::CloseCapture(id));
        });
        self.send(Cmd::OpenCapture(Box::new(head), id));
        capture
    }

    /// Levels for one side, computed only while the reader lives and only when it reads.
    #[cfg(test)]
    pub fn levels(&self, tap: Tap) -> LevelsReader {
        self.taps().reader(tap)
    }

    /// Both taps, for a consumer on another thread (the ring, on the GTK thread).
    pub fn taps(&self) -> LevelTaps {
        LevelTaps {
            input: self.input_tap.clone(),
            output: self.output_tap.clone(),
        }
    }

    /// §2's quit step 3: app AEC restores the default devices, the streams close, the thread ends.
    /// Waits at most `limit` (`aec.restore_timeout_s`; past it aec.json stays and the next start
    /// restores).
    pub async fn shutdown(&mut self, limit: Duration) {
        let (ack, done) = std::sync::mpsc::channel();
        self.send(Cmd::Quit(ack));
        let thread = self.thread.take();
        let joined = tokio::task::spawn_blocking(move || {
            let acked = done.recv_timeout(limit).is_ok();
            if acked && let Some(t) = thread {
                let _ = t.join();
            }
            acked
        })
        .await
        .unwrap_or(false);
        if !joined {
            tracing::warn!(
                "the audio thread did not finish within {} s (aec.restore_timeout_s)",
                limit.as_secs()
            );
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let (ack, _) = std::sync::mpsc::channel();
            self.send(Cmd::Quit(ack));
        }
    }
}

/// The microphone's and the playback's level taps. A reader taps its side while it lives; with no
/// reader the PipeWire callbacks copy nothing and nothing is computed (§6.1).
#[derive(Clone)]
pub struct LevelTaps {
    input: Arc<LatestWindow>,
    output: Arc<LatestWindow>,
}

impl LevelTaps {
    pub fn reader(&self, tap: Tap) -> LevelsReader {
        let window = match tap {
            Tap::Input => self.input.clone(),
            Tap::Output => self.output.clone(),
        };
        LevelsReader::new(window, tap, RATE)
    }

    /// Whether a reader taps the input and the output (the callbacks copy samples only then).
    #[cfg(test)]
    pub fn wanted(&self) -> (bool, bool) {
        (self.input.wanted(), self.output.wanted())
    }

    /// Writes `samples` to the playback's tap as its PipeWire callback would (the README's ring,
    /// `widgets::live_tests::readme`); nothing is kept while no reader taps it.
    #[cfg(test)]
    pub fn feed_output(&self, samples: &[i16]) {
        self.output.write(samples);
    }

    /// Taps with no stream behind them: silence, for the ring's checks that need no audio.
    #[cfg(test)]
    pub fn silent() -> Self {
        Self {
            input: Arc::new(LatestWindow::new(TAP_SAMPLES)),
            output: Arc::new(LatestWindow::new(TAP_SAMPLES)),
        }
    }
}

/// What the core and the voice session need of the audio thread; the tests use a fake.
pub trait AudioPort: Send + Sync + 'static {
    fn set_echo_mode(&self, mode: EchoMode);
    fn set_input(&self, node: Option<String>);
    fn set_output(&self, node: Option<String>);
    /// See [`Audio::open_playback`].
    fn open_playback(&self) -> Player;
    /// See [`Audio::open_capture`].
    fn open_capture(&self) -> Capture;
    fn shutdown(&mut self, limit: Duration) -> impl std::future::Future<Output = ()> + Send;
}

impl AudioPort for Audio {
    fn set_echo_mode(&self, mode: EchoMode) {
        Audio::set_echo_mode(self, mode);
    }
    fn set_input(&self, node: Option<String>) {
        Audio::set_input(self, node);
    }
    fn set_output(&self, node: Option<String>) {
        Audio::set_output(self, node);
    }
    fn open_playback(&self) -> Player {
        Audio::open_playback(self)
    }
    fn open_capture(&self) -> Capture {
        Audio::open_capture(self)
    }
    async fn shutdown(&mut self, limit: Duration) {
        Audio::shutdown(self, limit).await;
    }
}

pub use pw::runtime_dir_for;

/// A player and a capture with the heads their PipeWire callbacks would run, for the voice
/// session's tests: the test drives the heads in place of PipeWire.
#[cfg(test)]
pub mod test_support {
    use std::sync::Arc;

    use std::sync::OnceLock;

    use super::capture::{self, CaptureHead};
    use super::playback::{self, Clock, PlayHead};
    use super::ring::{LatestWindow, sample_ring};
    use super::{Capture, Player, RATE};

    pub use super::playback::monotonic_ns;

    /// tokio's clock in ns (paused in the tests that pause it), from a fixed origin and kept
    /// positive: the fake PipeWire stamps its time reports with it and the players read it, so
    /// the heard position never mixes the paused clock with the real one.
    pub fn tokio_ns() -> i64 {
        static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
        let origin = *ORIGIN.get_or_init(std::time::Instant::now);
        let now = tokio::time::Instant::now().into_std();
        let ns = match now.checked_duration_since(origin) {
            Some(d) => d.as_nanos() as i64,
            None => -(origin.duration_since(now).as_nanos() as i64),
        };
        1_000_000_000_000_000 + ns
    }

    /// A player whose ring holds `ring_ms`, and its head; both on [`tokio_ns`].
    pub fn playback(ring_ms: u64) -> (Player, FakeHead) {
        let tap = Arc::new(LatestWindow::new(4096));
        let (mut player, head) = playback::player(
            sample_ring((ring_ms * RATE as u64 / 1000) as usize),
            ring_ms,
            tap,
        );
        player.set_clock(tokio_ns);
        (player, FakeHead(head))
    }

    /// A capture whose ring holds `ring_ms`, and its head.
    pub fn capture(ring_ms: u64) -> (Capture, FakeMic) {
        let tap = Arc::new(LatestWindow::new(4096));
        let (capture, head) = capture::capture(
            sample_ring((ring_ms * RATE as u64 / 1000) as usize),
            ring_ms,
            tap,
        );
        (capture, FakeMic(head))
    }

    /// The playback callback's part.
    pub struct FakeHead(PlayHead);

    impl FakeHead {
        /// One cycle of `frames`, reported at [`tokio_ns`] with `latency` frames between the
        /// write head and what the device presents. A closed stream runs no more cycles (silence).
        pub fn cycle(&mut self, frames: usize, latency: u64) -> Vec<i16> {
            let mut out = vec![0i16; frames];
            if self.0.shared().is_closed() {
                return out;
            }
            let clock = Clock {
                now_ns: tokio_ns(),
                latency,
                delay: latency,
                ..Clock::default()
            };
            self.0.fill(&mut out, Some(clock));
            out
        }

        /// The stream went away (PipeWire restarted).
        pub fn close(&self) {
            self.0.shared().close();
        }
    }

    /// The capture callback's part.
    pub struct FakeMic(CaptureHead);

    impl FakeMic {
        pub fn take(&mut self, samples: &[i16]) {
            self.0.take(samples);
        }

        pub fn close(&self) {
            self.0.shared().close();
        }
    }
}
