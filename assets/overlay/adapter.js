// Kai's AnalyserNode stand-in for lmgw's engine.js (docs/design/2026-10-06-kai-design.md §9.2).
//
// Kai computes each tap's level and 48 bands itself, in the engine's SIGNAL units: what `Tap.read`
// would set as `tLevel` and `tBands` (src/audio/levels.rs). This analyser hands the engine data
// from which `Tap.read` computes exactly those targets again, so the engine's followers, states and
// renderer run unchanged.
//
// An exact inverse needs every band to own a bin that no other band reads: the engine's band edges
// share a bin with each neighbour, and on a 2048-point analyser the lowest bands are narrower than
// a bin. So this analyser keeps its own size (it ignores the engine's `fftSize`, as the node of a
// closed context would) at a rate that gives 4 Hz bins, where the narrowest band (90 to 98.7 Hz)
// spans three. A band's own bins carry its power, raised by the share of its bins it does not own;
// the shared bins read -200 dB, which `Tap.read` skips. The level is a constant signal whose RMS is
// the level's. `scripts/viz-adapter-check.mjs` runs `Tap.read` on it and checks the round trip.

import { BANDS, SIGNAL } from "./viz/engine.js";

const FFT_SIZE = 4096;
const RATE = 16384;
const BINS = FFT_SIZE / 2;
const HZ = RATE / 2 / BINS;

const clamp01 = (x) => (x > 0 ? (x < 1 ? x : 1) : 0);

// The engine's band layout on this analyser, computed as engine.js computes it (`Tap.attach` and
// `BAND_TILT`, a Float32Array), and which band owns which bin.
function layout() {
  const lo = new Int32Array(BANDS);
  const hi = new Int32Array(BANDS);
  const tilt = new Float32Array(BANDS);
  for (let b = 0; b < BANDS; b++) {
    const f0 = SIGNAL.fMin * Math.pow(SIGNAL.fMax / SIGNAL.fMin, b / BANDS);
    const f1 = SIGNAL.fMin * Math.pow(SIGNAL.fMax / SIGNAL.fMin, (b + 1) / BANDS);
    const l = Math.max(1, Math.min(BINS - 1, Math.floor(f0 / HZ)));
    lo[b] = l;
    hi[b] = Math.max(l + 1, Math.min(BINS, Math.ceil(f1 / HZ)));
    tilt[b] = SIGNAL.tiltDbPerOct * (Math.log2(Math.sqrt(f0 * f1)) - Math.log2(500));
  }
  const readers = new Int32Array(BINS);
  for (let b = 0; b < BANDS; b++) for (let i = lo[b]; i < hi[b]; i++) readers[i]++;
  const owner = new Int32Array(BINS).fill(-1);
  const owned = new Int32Array(BANDS);
  for (let b = 0; b < BANDS; b++) {
    for (let i = lo[b]; i < hi[b]; i++) {
      if (readers[i] === 1) {
        owner[i] = b;
        owned[b]++;
      }
    }
  }
  // The dB of a band's own bins over the band's mean, less the tilt the engine adds back.
  const offset = new Float64Array(BANDS);
  for (let b = 0; b < BANDS; b++) {
    if (owned[b] === 0) throw new Error(`adapter: band ${b} owns no bin at ${HZ} Hz per bin`);
    offset[b] = 10 * Math.log10((hi[b] - lo[b]) / owned[b]) - tilt[b];
  }
  return { first: lo[0], end: hi[BANDS - 1], owner, offset };
}

const L = layout();

export class KaiAnalyser {
  constructor() {
    this.smoothingTimeConstant = 0;
    this.context = { sampleRate: RATE };
    this.freq = new Float32Array(BINS).fill(-200);
    this.rms = 0;
  }
  get fftSize() {
    return FFT_SIZE;
  }
  set fftSize(_) {
    /* keeps its own size (module doc) */
  }
  get frequencyBinCount() {
    return BINS;
  }
  /** Kai's targets: a level and BANDS band values, each 0..1. */
  set(level, bands) {
    for (let i = L.first; i < L.end; i++) {
      const b = L.owner[i];
      this.freq[i] = b < 0 ? -200 : SIGNAL.floorDb + clamp01(bands[b] || 0) * SIGNAL.rangeDb + L.offset[b];
    }
    const l = clamp01(level || 0);
    // Tap.read adds 1e-9 before the log.
    this.rms = l > 0 ? Math.pow(10, (SIGNAL.levelFloorDb + l * SIGNAL.levelRangeDb) / 20) - 1e-9 : 0;
  }
  getFloatFrequencyData(a) {
    a.set(this.freq.subarray(0, a.length));
  }
  getFloatTimeDomainData(a) {
    a.fill(this.rms);
  }
}
