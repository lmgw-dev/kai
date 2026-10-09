#!/usr/bin/env node
// Checks the ring page's glue against the vendored engine (part of `just viz-check`):
// - assets/overlay/adapter.js inverts Kai's targets: lmgw's own `Tap.read` (run verbatim on the
//   vendored engine.js, with `Tap` exported as scripts/viz-reference.mjs does) computes the level
//   and bands Kai set, for random targets, the edges 0 and 1, and the cases of
//   src/audio/levels_reference.json;
// - with a voice.css given, assets/overlay/palette.js holds lmgw's `--rt-*` tokens.
//
// usage: node scripts/viz-adapter-check.mjs [lmgw voice.css]
// No packages; plain Node 22.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const overlay = `${root}/assets/overlay`;
const src = readFileSync(`${overlay}/viz/engine.js`, "utf8");
const { Tap, BANDS } = await import(
  "data:text/javascript;base64," + Buffer.from(`${src}\nexport { Tap };\n`).toString("base64")
);
const { KaiAnalyser } = await import(pathToFileURL(`${overlay}/adapter.js`).href);
const { PALETTE } = await import(pathToFileURL(`${overlay}/palette.js`).href);

let failed = 0;
const fail = (msg) => {
  failed++;
  console.error(`FAIL ${msg}`);
};

// ------------------------------------------------------------------ the round trip
const cases = [];
let x = 12345;
const rnd = () => ((x = (Math.imul(x, 1664525) + 1013904223) >>> 0) / 4294967296);
cases.push(["silence", 0, new Array(BANDS).fill(0)]);
cases.push(["full", 1, new Array(BANDS).fill(1)]);
cases.push(["alternating 0 and 1", 0.5, Array.from({ length: BANDS }, (_, b) => b % 2)]);
cases.push(["one loud band among quiet ones", 0.3, Array.from({ length: BANDS }, (_, b) => (b === 7 ? 1 : 0.01))]);
for (let n = 0; n < 200; n++) {
  cases.push([`random #${n}`, rnd(), Array.from({ length: BANDS }, () => rnd())]);
}
const reference = JSON.parse(readFileSync(`${root}/src/audio/levels_reference.json`, "utf8"));
for (const c of reference.cases) {
  cases.push([`reference: ${c.name} (output)`, c.output.level, c.output.bands]);
  if (c.input48k) cases.push([`reference: ${c.name} (input)`, c.input48k.level, c.input48k.bands]);
}

let worst = { d: 0, name: "", at: "" };
for (const [name, level, bands] of cases) {
  const a = new KaiAnalyser();
  a.set(level, bands);
  const tap = new Tap();
  tap.attach(a);
  if (tap.freq.length !== a.frequencyBinCount) fail(`${name}: the engine read ${tap.freq.length} bins`);
  tap.read();
  const check = (got, want, at) => {
    const d = Math.abs(got - Math.fround(want));
    if (d > worst.d) worst = { d, name, at };
    if (!(d < 1e-5)) fail(`${name}: ${at} is ${got}, Kai set ${want}`);
  };
  check(tap.tLevel, level, "the level");
  for (let b = 0; b < BANDS; b++) check(tap.tBands[b], bands[b], `band ${b}`);
}
console.log(
  `adapter: Tap.read gives back Kai's targets in ${cases.length} cases; worst difference ${worst.d.toExponential(2)}` +
    (worst.d > 0 ? ` (${worst.name}, ${worst.at})` : ""),
);

// ------------------------------------------------------------------ the palette
const css = process.argv[2];
if (css) {
  const text = readFileSync(css, "utf8");
  const token = (name) => {
    const m = text.match(new RegExp(`--rt-${name}:\\s*(#[0-9A-Fa-f]{3,8})`));
    if (!m) fail(`${css} has no --rt-${name}`);
    return m ? m[1].toUpperCase() : null;
  };
  const role = (name) => ({ deep: token(`${name}-deep`), base: token(name), hi: token(`${name}-hi`) });
  const want = { bg: token("bg"), accent: role("accent"), accent2: role("think"), muted: role("idle"), warn: role("user") };
  const norm = (v) => JSON.stringify(v, (_, s) => (typeof s === "string" ? s.toUpperCase() : s));
  for (const k of Object.keys(want)) {
    if (norm(PALETTE[k]) !== norm(want[k])) fail(`palette.${k} is ${norm(PALETTE[k])}, lmgw's voice.css has ${norm(want[k])}`);
  }
  if (!failed) console.log(`palette: the --rt-* tokens of ${css}`);
}

process.exit(failed ? 1 : 0);
