// The ring page's boot (docs/design/2026-10-06-kai-design.md §9.2): mounts lmgw's ring.js on a
// transparent canvas (its `transparent` input: no background, the desktop shows through) with Kai's
// analysers, and is the page's side of the bridge to Kai.
//
// Kai calls `window.__kai`:
//   setState(state, {muted, loading, held}, seq)  the engine's state (§5.1, §9.3)
//   setTiming(timing)                             lmgw.response.timing, or null
//   feed(outLevel, inLevel, outBands, inBands)    Kai's targets, up to 30 times a second while shown
//                                                 (K11; a frame equal to the last one is not sent)
//   setReduced(bool)                              reduced motion (GTK's gtk-enable-animations)
//   report()                                      posts the engine's stats and the state set last
//   watch(level)                                  the live check's latency probe: after the first
//                                                 frame drawn with an input level at or above
//                                                 `level`, posts {ev: "heard", level}
// The page posts JSON to Kai's `kai` message handler:
//   {ev: "ready"}       mounted; Kai sends no state before it (§9.2)
//   {ev: "drawn"}       the engine drew its first frame since the page became visible
//   {ev: "settled", seq} the idle state of setState #seq has been eased into and drawn (§9.4)
//   {ev: "stats", …}, {ev: "error", msg}, {ev: "csp", …}

import { mount } from "./viz/ring.js";
import { STATE } from "./viz/engine.js";
import { KaiAnalyser } from "./adapter.js";
import { PALETTE } from "./palette.js";

const post = (o) => window.webkit.messageHandlers.kai.postMessage(JSON.stringify(o));
addEventListener("error", (e) => post({ ev: "error", msg: `${e.message} (${e.filename}:${e.lineno})` }));
addEventListener("unhandledrejection", (e) => post({ ev: "error", msg: String(e.reason) }));
addEventListener("securitypolicyviolation", (e) =>
  post({ ev: "csp", directive: e.violatedDirective, blocked: e.blockedURI }),
);

const output = new KaiAnalyser();
const input = new KaiAnalyser();
let reduced = new URLSearchParams(location.search).get("reduced") === "1";
let last = { state: "idle", info: { muted: false, loading: null, held: false } };
let handle = null;
let canvas = null;

function size() {
  if (handle) handle.resize(innerWidth, innerHeight, devicePixelRatio || 1);
}

// A fresh canvas per mount: reduced motion is read at mount (engine.js), so a change mounts again.
function mountRing() {
  if (handle) handle.destroy();
  if (canvas) canvas.remove();
  canvas = document.createElement("canvas");
  document.body.appendChild(canvas);
  handle = mount(canvas, {
    output,
    input,
    palette: PALETTE,
    reducedMotion: reduced,
    transparent: true,
  });
  size();
  handle.setState(last.state, { ...last.info, since: Date.now() });
}

// Calls `fn` once the engine has drawn a frame after now; never while the page is hidden.
function afterFrame(fn) {
  const f0 = handle.stats().frames;
  const tick = () => {
    if (document.hidden) return;
    if (handle.stats().frames > f0) fn();
    else requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

let settle = 0;
// Feeds received (the stats say so: Kai skips a feed while the last one has not completed).
let fed = 0;
// The input level `watch` waits for, or null.
let watching = null;
window.__kai = {
  setState(state, info, seq) {
    last = { state, info: info || {} };
    handle.setState(state, { ...last.info, since: Date.now() });
    clearTimeout(settle);
    if (state === "idle") {
      const ms = 1000 * (reduced ? STATE.reducedTransition : STATE.transition);
      settle = setTimeout(() => afterFrame(() => post({ ev: "settled", seq })), ms);
    }
  },
  setTiming(timing) {
    handle.setTiming(timing);
  },
  feed(outLevel, inLevel, outBands, inBands) {
    fed += 1;
    output.set(outLevel, outBands);
    input.set(inLevel, inBands);
    if (watching !== null && inLevel >= watching) {
      watching = null;
      afterFrame(() => post({ ev: "heard", level: inLevel }));
    }
  },
  watch(level) {
    watching = level;
  },
  setReduced(on) {
    if (on === reduced) return;
    reduced = on;
    mountRing();
  },
  report() {
    const r = canvas.getBoundingClientRect();
    const root = document.scrollingElement;
    post({
      ev: "stats",
      ...handle.stats(),
      state: last.state,
      fed,
      hidden: document.hidden,
      reduced,
      w: innerWidth,
      h: innerHeight,
      sheets: document.styleSheets.length,
      overflow: getComputedStyle(document.documentElement).overflow,
      canvas: [r.x, r.y, r.width, r.height],
      scroll: [root.scrollWidth, root.scrollHeight],
    });
  },
};

mountRing();
addEventListener("resize", size);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) afterFrame(() => post({ ev: "drawn", frames: handle.stats().frames }));
});
post({ ev: "ready", hidden: document.hidden, w: innerWidth, h: innerHeight });
