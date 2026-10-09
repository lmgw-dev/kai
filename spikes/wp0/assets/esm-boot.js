// Boot module for the ES-module/CSP matrix: imports ring.js (which imports ./engine.js) and mounts it.
import { mount } from "./viz/ring.js";
import { FakeAnalyser } from "./adapter.js";   // adapter.js itself imports ./viz/engine.js
const post = (o) => { try { webkit.messageHandlers.kai.postMessage(JSON.stringify(o)); } catch (e) {} };
addEventListener("securitypolicyviolation", (e) => post({ ev: "csp", directive: e.violatedDirective, blocked: e.blockedURI }));
try {
  const c = document.querySelector("canvas");
  const h = mount(c, { output: new FakeAnalyser(), input: new FakeAnalyser(), palette: {}, reducedMotion: false });
  h.resize(100, 100, 1);
  h.setState("speaking", { muted: false, since: performance.now(), loading: null, held: false });
  post({ ev: "esm-ok", origin: location.origin, inline: !!window.__inline_ran });
  let n = 0; (function f() { if (++n < 3) requestAnimationFrame(f); else post({ ev: "raf-ok", stats: h.stats() }); })();
} catch (e) { post({ ev: "esm-throw", e: String(e) }); }
