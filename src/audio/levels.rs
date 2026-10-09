//! Level and 48 bands for the ring (§6.1, §9.3), in lmgw's `SIGNAL` units.
//!
//! The ring's `engine.js` turns a Web Audio `AnalyserNode` into a level and [`BANDS`] bands
//! (`Tap.read`): per bin the analyser's dB, a mean power per log-spaced band, a tilt per octave, and a
//! floor and range that map to 0..1; the level is the time-domain RMS in dBFS mapped the same way.
//! Kai has no `AnalyserNode`: [`Analyser`] computes the same targets from the samples, and Kai's
//! `adapter.js` (WP5) inverts them into analyser data, so the engine's own smoothing runs unchanged.
//!
//! The analyser part follows the Web Audio spec as WebKit and Chromium implement it: a Blackman
//! window (α 0.16, denominator N), the FFT, `|X[k]|/N`, `20·log10`, and smoothing 0 (the engine sets
//! it). Where the browser keeps floats, this rounds to f32 too.
//!
//! **Windows.** The Chat's output analyser runs in its 24 kHz player context with the engine's
//! `fftSize` 2048; Kai's playback stream is 24 kHz too, so [`Tap::Output`] is 2048 points. The Chat's
//! microphone analyser runs at the device's rate (48 kHz under PipeWire), where 2048 points are
//! 42.7 ms and 23.4 Hz bins; at Kai's 24 kHz capture the same window and bins are 1024 points, so
//! [`Tap::Input`] is 1024 points. The engine derives its band edges from the analyser's own rate and
//! size, so both give it the same bins.
//!
//! The reference values in `levels_reference.json` come from `scripts/viz-reference.mjs`, which runs
//! lmgw's `Tap.read` verbatim.

use std::sync::Arc;

use super::fft::Fft;
use super::ring::LatestWindow;

pub const BANDS: usize = 48;

/// engine.js `SIGNAL` (the fields the targets use; the follower constants stay in the engine).
pub mod signal {
    pub const FFT_SIZE: usize = 2048;
    pub const F_MIN: f64 = 90.0;
    pub const F_MAX: f64 = 7600.0;
    pub const FLOOR_DB: f64 = -84.0;
    pub const RANGE_DB: f64 = 50.0;
    pub const TILT_DB_PER_OCT: f64 = 3.5;
    pub const LEVEL_FLOOR_DB: f64 = -50.0;
    pub const LEVEL_RANGE_DB: f64 = 38.0;
}

/// Which side of the audio a reader taps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// The microphone, as captured.
    Input,
    /// The playback, as handed to PipeWire.
    Output,
}

impl Tap {
    /// The analyser's size at Kai's stream rate (module doc: "Windows").
    pub fn fft_size(self, rate: u32) -> usize {
        let reference_rate = match self {
            Tap::Output => 24_000,
            Tap::Input => 48_000,
        };
        (signal::FFT_SIZE as u64 * rate as u64 / reference_rate).next_power_of_two() as usize
    }
}

/// One frame of targets: what the engine's `Tap.read` would set as `tLevel` and `tBands`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub level: f32,
    pub bands: [f32; BANDS],
}

impl Levels {
    #[cfg(test)]
    pub const SILENT: Levels = Levels {
        level: 0.0,
        bands: [0.0; BANDS],
    };
}

/// The band edges and tilt for one analyser rate and size, as engine.js computes them.
#[derive(Debug, Clone, PartialEq)]
pub struct BandLayout {
    pub lo: [usize; BANDS],
    pub hi: [usize; BANDS],
    /// engine.js `BAND_TILT`, a `Float32Array`.
    pub tilt: [f32; BANDS],
}

impl BandLayout {
    pub fn new(rate: u32, fft_size: usize) -> Self {
        use signal::*;
        let bins = fft_size / 2;
        let hz = rate as f64 / 2.0 / bins as f64;
        let edge = |b: usize| F_MIN * (F_MAX / F_MIN).powf(b as f64 / BANDS as f64);
        let mut lo = [0; BANDS];
        let mut hi = [0; BANDS];
        let mut tilt = [0.0; BANDS];
        for b in 0..BANDS {
            let (f0, f1) = (edge(b), edge(b + 1));
            let l = ((f0 / hz).floor() as usize).clamp(1, bins - 1);
            lo[b] = l;
            hi[b] = ((f1 / hz).ceil() as usize).min(bins).max(l + 1);
            tilt[b] = (TILT_DB_PER_OCT * ((f0 * f1).sqrt().log2() - 500f64.log2())) as f32;
        }
        Self { lo, hi, tilt }
    }
}

/// Samples to [`Levels`]. Owns its scratch buffers: one per reader, used off the PipeWire thread.
pub struct Analyser {
    fft: Fft,
    layout: BandLayout,
    /// The Blackman window, as the f32 the browser multiplies with.
    window: Vec<f32>,
    re: Vec<f64>,
    im: Vec<f64>,
    db: Vec<f32>,
}

impl Analyser {
    pub fn new(rate: u32, fft_size: usize) -> Self {
        let n = fft_size;
        let window = (0..n)
            .map(|i| {
                let x = i as f64 / n as f64;
                let (a0, a1, a2) = (0.42, 0.5, 0.08);
                (a0 - a1 * (2.0 * std::f64::consts::PI * x).cos()
                    + a2 * (4.0 * std::f64::consts::PI * x).cos()) as f32
            })
            .collect();
        Self {
            fft: Fft::new(n),
            layout: BandLayout::new(rate, n),
            window,
            re: vec![0.0; n],
            im: vec![0.0; n],
            db: vec![0.0; n / 2],
        }
    }

    #[cfg(test)]
    pub fn fft_size(&self) -> usize {
        self.fft.len()
    }

    /// The targets for the latest `fft_size` samples (oldest first).
    pub fn analyse(&mut self, samples: &[f32]) -> Levels {
        self.frequency_db(samples);
        Levels {
            level: level_of(samples),
            bands: bands_of(&self.layout, &self.db),
        }
    }

    /// `AnalyserNode.getFloatFrequencyData` with smoothing 0: dB per bin, `-inf` for a zero bin.
    fn frequency_db(&mut self, samples: &[f32]) {
        let n = self.fft.len();
        assert_eq!(
            samples.len(),
            n,
            "the analyser takes exactly fft_size samples"
        );
        for (i, (&x, &w)) in samples.iter().zip(&self.window).enumerate() {
            self.re[i] = (x * w) as f64;
            self.im[i] = 0.0;
        }
        self.fft.forward(&mut self.re, &mut self.im);
        let scale = 1.0 / n as f64;
        for k in 0..n / 2 {
            // The packed Nyquist value is blown away from bin 0 (WebKit, Chromium).
            let im = if k == 0 { 0.0 } else { self.im[k] };
            let mag = (self.re[k].hypot(im) * scale) as f32;
            self.db[k] = (20.0 * (mag as f64).log10()) as f32;
        }
    }
}

/// engine.js `Tap.read`'s band part, from the analyser's dB per bin.
pub fn bands_of(layout: &BandLayout, db: &[f32]) -> [f32; BANDS] {
    use signal::*;
    let mut out = [0.0; BANDS];
    for (b, slot) in out.iter_mut().enumerate() {
        let (lo, hi) = (layout.lo[b], layout.hi[b]);
        let mut p = 0.0f64;
        for &d in &db[lo..hi] {
            if d > -200.0 {
                p += 10f64.powf(d as f64 * 0.1);
            }
        }
        p /= (hi - lo) as f64;
        let band_db = if p > 0.0 { 10.0 * p.log10() } else { -200.0 };
        *slot = clamp01((band_db + layout.tilt[b] as f64 - FLOOR_DB) / RANGE_DB) as f32;
    }
    out
}

/// engine.js `Tap.read`'s level: the time-domain RMS in dBFS, mapped to 0..1.
pub fn level_of(time: &[f32]) -> f32 {
    use signal::*;
    let s: f64 = time.iter().map(|&x| x as f64 * x as f64).sum();
    let rms = (s / time.len() as f64).sqrt();
    clamp01((20.0 * (rms + 1e-9).log10() - LEVEL_FLOOR_DB) / LEVEL_RANGE_DB) as f32
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// A consumer of one tap's levels. While at least one reader exists, the PipeWire callbacks copy
/// that side's samples into the tap; with none they copy nothing, and nothing is computed: the
/// analysis runs only in [`LevelsReader::read`]. Drop it to stop.
pub struct LevelsReader {
    tap: Arc<LatestWindow>,
    since: u64,
    analyser: Analyser,
    buf: Vec<f32>,
}

impl LevelsReader {
    pub fn new(tap: Arc<LatestWindow>, side: Tap, rate: u32) -> Self {
        let n = side.fft_size(rate);
        assert!(
            n <= tap.capacity() / 2,
            "the tap holds {} samples, the analyser needs {n}",
            tap.capacity()
        );
        let since = tap.subscribe();
        Self {
            tap,
            since,
            analyser: Analyser::new(rate, n),
            buf: vec![0.0; n],
        }
    }

    /// The targets for the latest window. Samples from before this reader existed, or from before
    /// the stream last stopped, count as silence.
    pub fn read(&mut self) -> Levels {
        self.tap.latest(self.since, &mut self.buf);
        self.analyser.analyse(&self.buf)
    }
}

impl Drop for LevelsReader {
    fn drop(&mut self) {
        self.tap.unsubscribe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// `levels_reference.json`, written by `scripts/viz-reference.mjs` from lmgw's engine.js.
    #[derive(Deserialize)]
    struct Reference {
        signal: RefSignal,
        bands: usize,
        layouts: Vec<RefLayout>,
        cases: Vec<RefCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RefSignal {
        fft_size: usize,
        f_min: f64,
        f_max: f64,
        floor_db: f64,
        range_db: f64,
        tilt_db_per_oct: f64,
        level_floor_db: f64,
        level_range_db: f64,
    }

    #[derive(Deserialize)]
    struct RefLayout {
        rate: u32,
        fft: usize,
        lo: Vec<usize>,
        hi: Vec<usize>,
        tilt: Vec<f32>,
    }

    #[derive(Deserialize)]
    struct RefCase {
        name: String,
        signal: Gen,
        /// The engine on a 24 kHz analyser of 2048 points: Kai's output tap, exactly.
        output: RefLevels,
        /// The engine on a 48 kHz analyser of 2048 points (the Chat's microphone), on the same
        /// sound sampled at 48 kHz; absent for noise, which differs between the rates.
        input48k: Option<RefLevels>,
    }

    #[derive(Deserialize)]
    struct RefLevels {
        level: f32,
        bands: Vec<f32>,
    }

    /// The test signals, generated identically by the script.
    #[derive(Deserialize, Debug)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum Gen {
        Silence,
        Tones { tones: Vec<(f64, f64)> },
        Chirp { f0: f64, f1: f64, amp: f64 },
        Noise { seed: u32, amp: f64 },
    }

    impl Gen {
        fn render(&self, rate: u32, n: usize) -> Vec<f32> {
            let t = |i: usize| i as f64 / rate as f64;
            let dur = n as f64 / rate as f64;
            match self {
                Gen::Silence => vec![0.0; n],
                Gen::Tones { tones } => (0..n)
                    .map(|i| {
                        tones
                            .iter()
                            .map(|&(f, a)| a * (2.0 * std::f64::consts::PI * f * t(i)).sin())
                            .sum::<f64>() as f32
                    })
                    .collect(),
                Gen::Chirp { f0, f1, amp } => (0..n)
                    .map(|i| {
                        let ph = 2.0
                            * std::f64::consts::PI
                            * (f0 * t(i) + (f1 - f0) * t(i) * t(i) / (2.0 * dur));
                        (amp * ph.sin()) as f32
                    })
                    .collect(),
                Gen::Noise { seed, amp } => {
                    let mut x = *seed;
                    (0..n)
                        .map(|_| {
                            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            ((((x >> 8) as f64 / 16_777_216.0) * 2.0 - 1.0) * amp) as f32
                        })
                        .collect()
                }
            }
        }
    }

    fn reference() -> Reference {
        serde_json::from_str(include_str!("levels_reference.json")).expect("the reference parses")
    }

    #[test]
    fn the_constants_are_the_engines() {
        let r = reference();
        let s = &r.signal;
        assert_eq!(r.bands, BANDS);
        assert_eq!(s.fft_size, signal::FFT_SIZE);
        assert_eq!(
            (s.f_min, s.f_max, s.floor_db, s.range_db, s.tilt_db_per_oct),
            (
                signal::F_MIN,
                signal::F_MAX,
                signal::FLOOR_DB,
                signal::RANGE_DB,
                signal::TILT_DB_PER_OCT
            )
        );
        assert_eq!(
            (s.level_floor_db, s.level_range_db),
            (signal::LEVEL_FLOOR_DB, signal::LEVEL_RANGE_DB)
        );
    }

    #[test]
    fn band_edges_and_tilt_are_the_engines() {
        for l in reference().layouts {
            let got = BandLayout::new(l.rate, l.fft);
            assert_eq!(got.lo.to_vec(), l.lo, "lo at {} Hz / {}", l.rate, l.fft);
            assert_eq!(got.hi.to_vec(), l.hi, "hi at {} Hz / {}", l.rate, l.fft);
            assert_eq!(got.tilt.to_vec(), l.tilt, "tilt");
        }
    }

    #[test]
    fn the_tap_sizes_follow_the_chats_analysers() {
        assert_eq!(Tap::Output.fft_size(24_000), 2048);
        assert_eq!(Tap::Input.fft_size(24_000), 1024);
        assert_eq!(Tap::Input.fft_size(48_000), 2048);
        // Same bin width as the Chat's microphone analyser at 48 kHz.
        let a = BandLayout::new(24_000, 1024);
        let b = BandLayout::new(48_000, 2048);
        assert_eq!((a.lo, a.hi), (b.lo, b.hi));
    }

    fn worst(got: &Levels, want: &RefLevels) -> (f32, usize) {
        let mut worst = ((got.level - want.level).abs(), usize::MAX);
        for b in 0..BANDS {
            let d = (got.bands[b] - want.bands[b]).abs();
            if d > worst.0 {
                worst = (d, b);
            }
        }
        worst
    }

    /// The output tap sees the exact samples the reference saw: equal to float rounding.
    #[test]
    fn output_levels_match_the_engine() {
        let r = reference();
        let mut a = Analyser::new(24_000, Tap::Output.fft_size(24_000));
        for c in &r.cases {
            let x = c.signal.render(24_000, a.fft_size());
            let got = a.analyse(&x);
            let (d, at) = worst(&got, &c.output);
            assert!(
                d < 1e-4,
                "{}: off by {d} at band {at} (MAX = level)\n got {:?}\nwant {:?}",
                c.name,
                got.bands,
                c.output.bands
            );
        }
    }

    /// The input tap's 1024 points at 24 kHz against the Chat's 2048 at 48 kHz on the same sound:
    /// the same bins and window, different sampling. Measured within 4e-5 of the range (2026-10-06);
    /// held to a thousandth (0.05 dB).
    #[test]
    fn input_levels_match_the_chats_microphone_analyser() {
        let r = reference();
        let mut a = Analyser::new(24_000, Tap::Input.fft_size(24_000));
        let mut n = 0;
        for c in &r.cases {
            let Some(want) = &c.input48k else { continue };
            let x = c.signal.render(24_000, a.fft_size());
            let (d, at) = worst(&a.analyse(&x), want);
            assert!(
                d < 1e-3,
                "{}: off by {d} at band {at} (MAX = level)",
                c.name
            );
            n += 1;
        }
        assert!(n >= 5, "only {n} input cases");
    }

    #[test]
    fn a_reader_counts_samples_from_before_it_as_silence_and_stops_the_copy_when_dropped() {
        let tap = Arc::new(LatestWindow::new(4096));
        tap.write(&[i16::MAX; 512]); // nobody listens: not copied
        assert!(!tap.wanted());
        let mut r = LevelsReader::new(tap.clone(), Tap::Input, 24_000);
        assert!(tap.wanted());
        assert_eq!(r.read(), Levels::SILENT);
        let tone: Vec<i16> = (0..1024)
            .map(|i| ((i as f64 * 0.2).sin() * 16000.0) as i16)
            .collect();
        tap.write(&tone);
        let l = r.read();
        assert!(l.level > 0.5, "{l:?}");
        drop(r);
        assert!(!tap.wanted());
    }
}
