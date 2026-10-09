// Kai's MCP Apps host script (docs/design/2026-10-06-kai-design.md §10.1, K18; WP12), injected
// after host.js into the same isolated world "kai-widget" of an overlay_app widget's top frame
// only, never into another kind's. It places the view (the server's document, served by Kai under
// §10.1's CSP into a frame with scripts only) and relays the view's JSON-RPC messages to Kai
// through the "kai" handler, which only this world has; Kai's bridge answers through post().
// Only messages whose source is the frame placed here are relayed (K18: MessageEvent.source):
// the host page's own window, another frame or a nested one the view made are ignored.
// A message larger than overlay.app_max_mb is refused here, before it is handed to Kai: a request
// gets its -32600 answer from this script, naming the setting.
(() => {
  const w = window.__kaiWidget;
  const post = (text) => window.webkit.messageHandlers.kai.postMessage(text);
  let view = null;
  // overlay.app_max_mb: { max: bytes, says: "overlay.app_max_mb = 64" } (set by w.app).
  let bound = { max: 0, says: "overlay.app_max_mb" };
  // Larger than the bound in UTF-8? A string has at least one byte per UTF-16 unit and at most
  // three, so only one in between is encoded to count.
  const over = (text) => {
    if (text.length > bound.max) return true;
    if (text.length * 3 <= bound.max) return false;
    return new TextEncoder().encode(text).length > bound.max;
  };
  window.addEventListener("message", (e) => {
    if (view === null || e.source !== view.contentWindow) return;
    let text;
    try {
      text = JSON.stringify(e.data);
    } catch (_) {
      return;
    }
    if (typeof text !== "string") return;
    if (over(text)) {
      const m = e.data;
      if (m && typeof m === "object" && m.id !== undefined && m.id !== null
          && typeof m.method === "string") {
        view.contentWindow.postMessage({ jsonrpc: "2.0", id: m.id, error: {
          code: -32600, message: "the message is larger than " + bound.says } }, "*");
      }
      return;
    }
    // The message's own JSON inside Kai's envelope, as is (sandbox::parse takes it out whole).
    post('{"ev":"rpc","m":' + text + "}");
  });
  // The view: the same sandboxed frame as overlay_html's, remembered as the one source answered.
  w.app = (src, b) => {
    bound = b;
    w.frame(src);
    view = document.querySelector("iframe");
  };
  // A message of Kai's bridge to the view (a JSON-RPC response, or Kai's ui/resource-teardown);
  // false when no view is placed.
  w.post = (m) => {
    if (view === null || !view.contentWindow) return false;
    view.contentWindow.postMessage(m, "*");
    return true;
  };
})();
