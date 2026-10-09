// Kai's host script for an overlay's view (docs/design/2026-10-06-kai-design.md §10.1), injected
// by Kai as a user script into the isolated world "kai-widget" of the top frame only. Only that
// world has the "kai" message handler, so the caller's document (a sandboxed frame, main world)
// cannot post to Kai. The host page itself runs no script.
(() => {
  const post = (m) => window.webkit.messageHandlers.kai.postMessage(JSON.stringify(m));
  window.__kaiWidget = {
    // Kai's text card: the text as text, never as markup; its natural height at `width` CSS px.
    text(t, width) {
      const d = document.createElement("div");
      d.className = "text";
      d.style.width = width + "px";
      d.textContent = t;
      document.body.replaceChildren(d);
      post({ ev: "size", h: Math.ceil(d.getBoundingClientRect().height) });
    },
    // The card is taller than the output allows: it fills the surface and scrolls.
    fit(on) {
      document.documentElement.classList.toggle("fit", on);
      const d = document.querySelector(".text");
      if (d) d.style.width = "";
    },
    // The caller's document: served by Kai under §10.1's CSP, in a frame with scripts only.
    frame(src) {
      const f = document.createElement("iframe");
      f.setAttribute("sandbox", "allow-scripts");
      f.setAttribute("referrerpolicy", "no-referrer");
      f.addEventListener("load", () => post({ ev: "loaded" }), { once: true });
      f.src = src;
      document.body.replaceChildren(f);
    },
  };
  post({ ev: "ready" });
})();
