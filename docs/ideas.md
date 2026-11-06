# Ideas for later

Ideas that are not part of the spec until a design pass folds them in.

## Verbal computer use: look, point, click (owner, 2026-10-06)

**The idea.** Some vision models, such as Gemma 4 12B and some Qwen VL models, can name coordinates in an
image. Kai could act on what it sees as well as see windows (phase 3, §11.1 vision): "click Save",
"open the second result". Kai would trigger the click itself. This is computer use by voice, in any
language the model speaks, with no control scheme to learn and no prepared app. It is an
accessibility gain at no extra cost.

**What already fits.**
- `see_window` / `see_screen` through `org.kde.KWin.ScreenShot2` (§11.1).
- Clicking and typing through `org_kde_kwin_fake_input`, already listed in §11.2 ("always
  confirm") and in the `.desktop` file's `X-KDE-Wayland-Interfaces`.
- Window geometry from `org_kde_plasma_window_management`, to turn window-relative points into
  screen points.
- The overlays (phase 2) can draw a marker at the target before the click. This shows what Kai is
  about to do and is the natural place for the confirmation.

**To design.**
- **Coordinate conventions differ by model family.** Some models give absolute pixels in the image
  as the projector resized it; others normalize to 0–1000. Kai needs a per-model mapping, measured
  once per model rather than assumed. It covers image space → capture → window → logical screen
  coordinates, with the output scale.
- **Precision on a large output** (a 32:9 ultrawide, say). A full capture gets downscaled hard. Prefer a
  window capture, and possibly a two-step zoom: a coarse point on the window, then a
  native-resolution crop around it to refine.
- **Confirmation cost.** Confirming every click makes a long task tedious, and §11.2 says "always
  confirm". The options:
  - confirm per task instead of per click;
  - a marker with a short cancel window;
  - trust levels per app.

  The owner's call, at design time.
- **The check after each step.** A fresh capture shows whether the click did what was meant before
  the next step.
- **The accessibility tree (AT-SPI) as a precision aid** where an app exposes one. It gives exact
  element bounds. Vision stays the general path for canvas, games and apps without one.

**Compaction of screenshots in the thread's history (lmgw side).** A computer-use turn produces
many captures, and only some of them are worth keeping in context. One shape:
- the latest capture stays;
- older ones become a text placeholder, using the existing image-placeholder mechanism and a short
  caption written when the image was seen;
- a few that are explicitly marked "keep" (by the model or the owner) stay as images.

This is a Chat history policy in lmgw, because the thread lives there, rather than something Kai
does. It needs its own spec section in lmgw when phase 3 is designed.

## App memory (owner, 2026-10-09)

**The seed idea.** Memory that is injected into the prompt depending on the windows that are
active at the moment the prompt is sent. Talking to Kai while a spreadsheet and a mail client are
open brings what Kai learned about those apps into that turn, and nothing about the others. The
desktop already says which windows those are (`desktop_context`, the window management protocol).

**What already fits** (the spec's facts, ruling 18, §11.1, §16 Q10 as answered):
- Keyed by the app id that window management gives each window.
- Embeddings through lmgw's `/v1/embeddings`, with an `embed/` alias named in the configuration.
- A Kai-local SQLite file (`$XDG_DATA_HOME/kai/memory.db`), encrypted at rest with a key kept in
  the Secret Service, apart from the data, so a copied database file is useless without the key.

**To design.**
- **What "active" means at send time:** the focused window only, every window on the current
  desktop and output, or the focused one plus the ones the owner looked at in the last minutes.
- **Where the injection happens:** Kai owns the conversation's client side, but the prompt is
  built in lmgw (the thread, the profile). A per-turn context block from the client is an lmgw
  feature, or the memory goes in as a tool result Kai adds before the turn.
- **Budget:** the injected memory costs prefill on every turn (K22's total context cost), so how
  much per app, and how it is ranked against the turn's words.
- **Writing memory:** by a tool the model calls (`remember` for the active app), by the owner
  only, or both; how the owner sees, edits and deletes what is kept per app.
- **The tools:** K24 names none yet; they follow K22 like every tool (§16 Q20).

## Browser integration (owner, 2026-10-09)

*Designed 2026-11-03* in the `kai-firefox` repository
(`docs/design/2026-11-03-kai-firefox-design.md`): Kai's browser tools, Kai as the user's hands and
eyes in Firefox, and the sidebar as a full lmgw Chat. The open questions that stood here are
answered there.

## Spoken commands without a model (2026-11-03)

From the Firefox design (§6.5): in conversation mode, "scroll down" or "press 14" runs a whole model
turn. A path that maps simple, unambiguous commands (scroll, back, press a number, stop reading)
straight to tool calls, without a model, would make browsing by voice feel immediate. Open: which
commands, in which languages, how it falls back to the model, and how it stays out of the way of
normal speech.
