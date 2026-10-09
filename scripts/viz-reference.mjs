#!/usr/bin/env node
// The level/band reference for src/audio/levels.rs: what lmgw's engine.js computes for fixed test
// signals. Its `Tap.read` runs verbatim (the module is imported with `Tap` and `BAND_TILT` exported);
// the AnalyserNode under it follows the Web Audio spec as WebKit and Chromium implement it (Blackman
// window with denominator N, |X[k]|/N, 20·log10, smoothing 0, floats where the browser keeps floats).
//
// usage: node scripts/viz-reference.mjs [engine.js] > src/audio/levels_reference.json
//        (default engine.js: lmgw's main checkout next to this repository, read only)
// No packages; plain Node 22.

import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const enginePath = resolve(
  process.argv[2] ?? `${here}/../../lmgw/crates/lmgw-ui/assets/voice/viz/engine.js`,
);
const src = readFileSync(enginePath, "utf8");
const { Tap, SIGNAL, BANDS, BAND_TILT } = await import(
  "data:text/javascript;base64," +
    Buffer.from(`${src}\nexport { Tap, BAND_TILT };\n`).toString("base64")
);

let lmgwCommit = null;
try {
  lmgwCommit = execFileSync(
    "git",
    ["-C", dirname(enginePath), "log", "-1", "--format=%H", "--", enginePath],
    { encoding: "utf8" },
  ).trim();
} catch {
  /* not a checkout */
}

// ------------------------------------------------------------------ the analyser stand-in
function fft(re, im) {
  const n = re.length;
  const bits = Math.log2(n);
  for (let i = 0; i < n; i++) {
    let j = 0;
    for (let b = 0; b < bits; b++) j |= ((i >> b) & 1) << (bits - 1 - b);
    if (j > i) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let size = 2; size <= n; size *= 2) {
    const half = size / 2;
    const step = n / size;
    for (let start = 0; start < n; start += size) {
      for (let k = 0; k < half; k++) {
        const a = (-2 * Math.PI * (k * step)) / n;
        const wr = Math.cos(a);
        const wi = Math.sin(a);
        const p = start + k;
        const q = p + half;
        const tr = re[q] * wr - im[q] * wi;
        const ti = re[q] * wi + im[q] * wr;
        re[q] = re[p] - tr;
        im[q] = im[p] - ti;
        re[p] += tr;
        im[p] += ti;
      }
    }
  }
}

class SpecAnalyser {
  constructor(rate, samples) {
    this.context = { sampleRate: rate };
    this.samples = samples;
    this.n = samples.length;
    this.smoothingTimeConstant = 0;
  }
  get fftSize() {
    return this.n;
  }
  set fftSize(v) {
    if (v !== this.n) throw new Error(`the engine asked for fftSize ${v}; the signal has ${this.n}`);
  }
  get frequencyBinCount() {
    return this.n / 2;
  }
  getFloatFrequencyData(out) {
    const n = this.n;
    const re = new Float64Array(n);
    const im = new Float64Array(n);
    for (let i = 0; i < n; i++) {
      const x = i / n;
      const w = Math.fround(0.42 - 0.5 * Math.cos(2 * Math.PI * x) + 0.08 * Math.cos(4 * Math.PI * x));
      re[i] = Math.fround(this.samples[i] * w);
    }
    fft(re, im);
    for (let k = 0; k < n / 2; k++) {
      const mag = Math.fround(Math.hypot(re[k], k === 0 ? 0 : im[k]) / n);
      out[k] = Math.fround(20 * Math.log10(mag));
    }
  }
  getFloatTimeDomainData(out) {
    out.set(this.samples);
  }
}

// ------------------------------------------------------------------ the test signals
// Mirrored by `Gen::render` in levels.rs, operation for operation.
function render(sig, rate, n) {
  const out = new Float32Array(n);
  const dur = n / rate;
  for (let i = 0; i < n; i++) {
    const t = i / rate;
    let v = 0;
    if (sig.kind === "tones") {
      for (const [f, a] of sig.tones) v += a * Math.sin(2 * Math.PI * f * t);
    } else if (sig.kind === "chirp") {
      const ph = 2 * Math.PI * (sig.f0 * t + ((sig.f1 - sig.f0) * t * t) / (2 * dur));
      v = sig.amp * Math.sin(ph);
    }
    out[i] = v;
  }
  if (sig.kind === "noise") {
    let x = sig.seed >>> 0;
    for (let i = 0; i < n; i++) {
      x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
      out[i] = ((x >>> 8) / 16777216 * 2 - 1) * sig.amp;
    }
  }
  return out;
}

const vowel = [];
for (let k = 1; 220 * k <= 4000; k++) vowel.push([220 * k, 0.3 / k]);

const CASES = [
  ["silence", { kind: "silence" }],
  ["A4 at half scale", { kind: "tones", tones: [[440, 0.5]] }],
  ["quiet 1 kHz", { kind: "tones", tones: [[1000, 0.05]] }],
  ["three tones", { kind: "tones", tones: [[200, 0.3], [2500, 0.1], [6000, 0.02]] }],
  ["a vowel: 220 Hz and harmonics to 4 kHz", { kind: "tones", tones: vowel }],
  ["chirp 100 Hz to 7 kHz", { kind: "chirp", f0: 100, f1: 7000, amp: 0.25 }],
  ["near the level floor", { kind: "tones", tones: [[300, 0.004]] }],
  ["7 kHz near full scale", { kind: "tones", tones: [[7000, 0.99]] }],
  ["60 Hz hum below the bands", { kind: "tones", tones: [[60, 0.5]] }],
  ["white noise", { kind: "noise", seed: 1, amp: 0.1 }],
  ["loud white noise", { kind: "noise", seed: 42, amp: 0.8 }],
];

function engine(rate, samples) {
  const tap = new Tap();
  tap.attach(new SpecAnalyser(rate, samples));
  tap.read();
  return {
    level: Math.fround(tap.tLevel),
    bands: Array.from(tap.tBands),
    lo: Array.from(tap.lo),
    hi: Array.from(tap.hi),
  };
}

const layouts = [24000, 48000].map((rate) => {
  const e = engine(rate, new Float32Array(SIGNAL.fftSize));
  return { rate, fft: SIGNAL.fftSize, lo: e.lo, hi: e.hi, tilt: Array.from(BAND_TILT) };
});

const cases = CASES.map(([name, signal]) => {
  const out = engine(24000, render(signal, 24000, SIGNAL.fftSize));
  const c = { name, signal, output: { level: out.level, bands: out.bands } };
  if (signal.kind !== "noise") {
    const inp = engine(48000, render(signal, 48000, SIGNAL.fftSize));
    c.input48k = { level: inp.level, bands: inp.bands };
  }
  return c;
});

// One line per numeric array, so a regenerated file diffs per band list.
const text = (v) =>
  JSON.stringify(v, null, 1).replace(/\[\s+(-?[\d.e+-]+(?:,\s+-?[\d.e+-]+)*)\s+\]/g, (_, inner) =>
    `[${inner.split(/,\s+/).join(", ")}]`,
  );

process.stdout.write(
  text(
    {
      generator: "scripts/viz-reference.mjs",
      engine_sha256: createHash("sha256").update(src).digest("hex"),
      lmgw_commit: lmgwCommit,
      signal: SIGNAL,
      bands: BANDS,
      layouts,
      cases,
    },
  ) + "\n",
);
