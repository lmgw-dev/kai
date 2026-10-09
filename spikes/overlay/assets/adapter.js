// Glue: an AnalyserNode stand-in for engine.js / ring.js. The page has no audio; the Rust
// host pushes per-band levels (0..1, the engine's own 48 log bands) and a level, ~60 Hz.
// Implements exactly what engine.js touches: fftSize, smoothingTimeConstant (settable),
// frequencyBinCount, context.sampleRate, getFloatFrequencyData, getFloatTimeDomainData.
import { BANDS, SIGNAL } from "./viz/engine.js";

export class FakeAnalyser {
  constructor() {
    this.fftSize = 2048;
    this.smoothingTimeConstant = 0;
    this.context = { sampleRate: 48000 };
    this.level = 0;
    this.bands = new Float32Array(BANDS);
    this.phase = 0;
  }
  get frequencyBinCount() {
    return this.fftSize / 2;
  }
  set(level, bands) {
    this.level = level;
    for (let b = 0; b < BANDS; b++) this.bands[b] = bands[b] || 0;
  }
  // Inverse of Tap.read(): band value v -> dB such that clamp01((db+tilt-floor)/range) == v.
  getFloatFrequencyData(a) {
    const hz = this.context.sampleRate / 2 / this.frequencyBinCount;
    const { fMin, fMax, floorDb, rangeDb, tiltDbPerOct } = SIGNAL;
    const span = Math.log(fMax / fMin);
    for (let i = 0; i < a.length; i++) {
      const f = i * hz;
      const x = f >= fMin && f < fMax ? Math.floor((Math.log(f / fMin) / span) * BANDS) : -1;
      if (x < 0) {
        a[i] = -200;
        continue;
      }
      const fc = fMin * Math.pow(fMax / fMin, (x + 0.5) / BANDS);
      a[i] = this.bands[x] > 0 ? floorDb + this.bands[x] * rangeDb - tiltDbPerOct * (Math.log2(fc) - Math.log2(500)) : -200;
    }
  }
  // A sine whose RMS lands on the engine's level mapping (levelFloorDb..+levelRangeDb).
  getFloatTimeDomainData(a) {
    const rms = this.level > 0 ? Math.pow(10, (SIGNAL.levelFloorDb + this.level * SIGNAL.levelRangeDb) / 20) : 0;
    const amp = rms * Math.SQRT2;
    for (let i = 0; i < a.length; i++) a[i] = amp * Math.sin(i * 0.05);
  }
}
