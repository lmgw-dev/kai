// The approval card's page (WP19). Kai calls window.__kai.show(model, armed, width, typed) and
// window.__kai.arm(armed, typed); the page posts to the "kai" message handler: ready, its content's
// height (size), whether the end of the arguments is in view (seen), and a click (answer). Every
// text goes in as text, never as markup. Keys never reach the page: Kai's window takes Enter and
// Esc itself, after the arming delay.
"use strict";
(function () {
  const $ = (id) => document.getElementById(id);
  const post = (msg) => window.webkit.messageHandlers.kai.postMessage(JSON.stringify(msg));
  let call = null;
  let busy = false;
  let typedText = "";
  let lastWidth = 0;

  function text(id, value) {
    const el = $(id);
    el.textContent = value || "";
    el.hidden = !value;
  }

  // The card's natural height at `width` CSS px (Kai's surface width), whatever the surface is now.
  function size(width) {
    const card = $("card");
    card.style.width = width + "px";
    card.style.height = "auto";
    const h = card.getBoundingClientRect().height;
    card.style.width = "";
    card.style.height = "";
    post({ ev: "size", h: Math.ceil(h) });
  }

  // The line keeps its place either way, so the card does not change its height when it arms.
  // `typed`: a key reached the card before it armed for this call (Enter alone does not approve).
  // The typed line shows only once it applies, and the card measures itself again then.
  function arm(armed, typed) {
    document.body.classList.toggle("unarmed", !armed);
    $("arming").style.visibility = armed ? "hidden" : "visible";
    const t = $("typed");
    const was = !t.hidden;
    t.textContent = typedText;
    t.hidden = !typed;
    if (was !== !!typed && lastWidth) size(lastWidth);
  }

  // Whether the end of the arguments is in view: Enter alone waits for it; the cue says more is
  // below until then.
  function seen() {
    const a = $("args");
    const all = a.scrollTop + a.clientHeight >= a.scrollHeight - 2;
    // Only shown while the arguments overflow, when the card is at its tallest already: the
    // arguments' box gives it the room, the card's height stays.
    $("more").hidden = all;
    if (call) post({ ev: "seen", call, all });
  }

  window.__kai = {
    show(model, armed, width, typed) {
      call = model.call;
      lastWidth = width;
      busy = !!model.busy;
      typedText = model.typed || "";
      text("heading", model.heading);
      text("asked", model.asked);
      text("count", model.count);
      text("note", model.note);
      text("busy", model.busy);
      text("hint", model.hint);
      const args = $("args");
      args.replaceChildren();
      for (const line of model.args) {
        const d = document.createElement("div");
        d.textContent = line;
        args.appendChild(d);
      }
      $("approve").disabled = busy;
      $("decline").disabled = busy;
      args.scrollTop = 0;
      arm(armed, typed);
      size(width);
      requestAnimationFrame(seen);
    },
    arm,
  };

  $("args").addEventListener("scroll", seen, { passive: true });
  window.addEventListener("resize", () => requestAnimationFrame(seen));
  $("approve").addEventListener("click", () => call && !busy && post({ ev: "answer", call, approve: true }));
  $("decline").addEventListener("click", () => call && !busy && post({ ev: "answer", call, approve: false }));
  window.addEventListener("error", (e) => post({ ev: "error", msg: String(e.message) }));
  post({ ev: "ready" });
})();
