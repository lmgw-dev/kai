# Kai: a Plasma client of lmgw

**Draft v2, 2026-10-06.** This is v1 plus an adversarial review
(`reviews/2026-10-06-kai-specs-review.md`, checked against lmgw main at 2baa764). Every finding's
disposition is in the appendix "Review dispositions".

The owner approved the proposal on 2026-10-06. Everything under "Decided" is the owner's. "Taken in this
draft" is this document's own and stands until the owner changes it.

Companion: [lmgw docs/design/2026-10-06-client-apps-design.md](../../../lmgw/docs/design/2026-10-06-client-apps-design.md) ("api
§n"), the lmgw-side changes. It stands alone and moves into lmgw's `docs/design/` later; it calls
Kai "a desktop client" and tags its phases Desktop 1–3.

lmgw's design records are cited by short name, all in lmgw's `docs/design/`:
- "chat-voice §n" (2026-10-03);
- "realtime §n" (2026-10-01);
- "principals §n" (2026-09-22);
- "server-tools §n" (2026-10-05);
- "egress §n" (2026-10-06).

lmgw paths are relative to its repository, at main 2baa764.

Kai is a desktop client of lmgw on Plasma 6 Wayland:
- voice at any time from a global shortcut;
- its state in the tray and in a ring drawn over the desktop;
- one ongoing conversation;
- later, overlays the agent controls and desktop tools the agent can call.

Kai holds no models. Every model call goes through lmgw.

## Decided by the owner (2026-10-06)

Rulings:
1. **A separate process** that talks to lmgw only through its network API, the same API a future
   Android client (chat + realtime) uses. An "extension" of lmgw is a client with a restricted key.
2. **XDG autostart** (a `.desktop` file), no systemd unit. The tray process is the long-running
   process; Plasma runs autostart entries as `app-…@autostart.service` units.
3. **One ongoing conversation "Kai".** A new thread starts after a configurable idle time, or on
   demand.
4. **A clean rewrite.** An earlier private prototype is a parts bin: the ksni tray, the
   app-managed PipeWire echo cancellation (`src/aec.rs`), pause as state, the KRunner runner.
5. **The desktop MCP server lives inside Kai:** run programs, see apps via vision, app-scoped
   memory, browser integration.
6. **Tool-controlled overlays:** the agent shows widgets (weather, an emote, an image) borderless
   over everything, like the ring.
7. **Configured fallbacks are always used**, with no content-based exceptions. Privacy is chosen
   by picking an alias without a fallback. Only capability (a route that cannot decode images or
   audio) keeps content from a route.
8. **OpenAI-shaped routes copy OpenAI's protocol**; lmgw-only behaviour is opt-in.
9. **No hidden limits.** A limit surfaces as a visible setting, a logged note or an explicit error.

The approved proposal:

10. **One Rust process.** A tokio core holds the lmgw client: `/v1/realtime?chat_thread=<id>`
    (chat-voice §8) and the chat change feed. PipeWire capture and playback run with echo modes
    that mirror Chat voice: device AEC, app AEC (the prototype's module-echo-cancel child),
    headphones, half duplex. Capture is buffered from the key press until `session.created`, so a
    cold model warm loses no words.
11. **ksni tray.**
    - The icon shows idle, listening, thinking, speaking, muted, held (amber, GPU hold) or lmgw
      unreachable.
    - The tooltip shows the thread and the last turn's timing line.
    - The menu has: start/stop, mute, automatic vs push-to-talk, "continue <thread>" / "new
      conversation", open in lmgw, echo mode and devices.
    - Middle-click mutes.
12. **Global shortcuts** come from the XDG GlobalShortcuts portal (ashpd) or KGlobalAccel. Press
    and release give a toggle plus hold-to-talk. Release delivery is unverified. Keys are set in
    System Settings. Right Alt is avoided: it is AltGr on many layouts.
13. **Credentials** are kept in KWallet through the Secret Service API.
14. **A sans-IO `lmgw-client` crate** holds the realtime event handling, the barge-in truncate
    bookkeeping from the playback cursor and the Chat API client, apart from desktop I/O.
    Android may reuse it through UniFFI or port it.
15. **Overlays are HTML in WebKitGTK views on gtk-layer-shell surfaces** (GTK3, webkit2gtk-4.1).
    - lmgw's `ring.js` is reused verbatim.
    - Kai hosts MCP Apps: `ui://` resources with mime `text/html;profile=mcp-app`, shown in a
      sandboxed iframe that speaks JSON-RPC over postMessage.
    - The overlay tools are:
      - `overlay_show {kind: built-in | html | ui_resource, anchor, size, timeout, interactive}`,
        which returns an id;
      - `overlay_update`, `overlay_hide` and `overlay_list`.

      *Changed, 2026-10-09 (ruling 24):* one show tool per kind, no `overlay_update` (§10.1).
    - Widget content has no network access; images arrive through Kai. Overlays are
      click-through by default.

    The ruling named "MCP 2026-07-28, SEP-1865". What could be verified on 2026-10-06 is SEP-1865
    as the MCP Apps extension: `modelcontextprotocol/ext-apps`,
    `specification/2026-01-26/apps.mdx`, capability `io.modelcontextprotocol/ui`. §10 and api §7
    use that document.
16. **KWin's restricted interfaces** are granted per installed `.desktop` file:
    `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2` and
    `X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,zkde_screencast_unstable_v1,org_kde_kwin_fake_input`.
    The desktop tools therefore belong to an installed Kai.
17. **Device-hosted MCP.**
    - Kai connects out to lmgw with its device key and offers its MCP server over that
      connection.
    - lmgw lists it like a registered MCP server while it is connected.
    - Its tools then work from:
      - Kai voice;
      - the dashboard chat;
      - Claude Code through `/mcp`;
      - agents;
      - the phone.

      Who among them reaches it is api §5.6: the owner, Kai itself, and keys whose tool scope names
      the label.
    - Android later uses the same path for its phone tools.
18. **App-scoped memory lives in Kai**, keyed by the app id from window management, with
    embeddings from lmgw's `/v1/embeddings`. **Browser integration** is a native-messaging helper
    that forwards to a Kai socket.
19. **Android later.**
    - Audio over WebSocket first; OpenAI's WebRTC transport (`/v1/realtime/calls`) only if mobile
      jitter hurts.
    - Reachability through WireGuard or Tailscale.
    - Push through UnifiedPush/ntfy.
    - A native app can register as Android's assist app.

Rulings of 2026-10-08 (phase 2 and 3 tools, a design discussion with the owner):

20. **Tools are designed for their total context cost, not for a low count.** Many tools are not the
    problem: the owner tested more than 300 tools with every Gemma 4 variant, and selection was
    always correct. What fails is a tool whose use is complicated (many, nested or mode-dependent
    parameters): the model gets lazy. Kai's tools are optimized for the context cost of the whole
    desktop integration (prefill is latency, and not every model has prompt caching). Rules: K22.
21. **Delegate tools run their loop in Kai** (K25).
22. **Kai may speak an async tool result on its own,** opening a voice session without a key press.
    Each job has an `announce` flag: on by default for jobs started by voice, off otherwise (then a
    notification, and the result waits in the thread). K26.
23. **Conversation mode is a tool** (the owner's idea). With push-to-talk now and the hotword later,
    the hotword serves single tasks; saying "enter conversation mode" (any phrasing, any language:
    the model recognizes the intent) switches Kai to listening for a conversation or multi-step
    work, such as directing the browser or general PC use by voice. K27.

Rulings of 2026-10-09 (the configuration UI and routines, a design discussion with the owner):

24. **The `overlay_show` split is approved** (§16 Q11): one show tool per kind, each with an
    optional flat `position` and returning `{id}`; `overlay_hide {id?}` and `overlay_list`; no
    `overlay_update`. §10.1.
25. **A routine can also be a script.** Scripts are the flexible form: the owner can build them
    outside Kai, and a capable model can be tasked with writing one. Even non-developers are
    expected to prefer an agent-written script over building one by hand in a simplified system.
    Routines come in three kinds: fixed steps, a goal for the delegate loop, and a script. K28.
26. **Kai's configuration UI is a WebKitGTK window with a Leptos frontend,** sharing its
    construction with lmgw's UI through shared interface crates: Kai is an extension of lmgw, and
    more extensions may follow. K29.
27. **Kai's configuration is also offered as owner-only tools,** behind a setting "Kai configuration
    via tools" that hides them when off. The growth of lmgw's tool list is answered on lmgw's side
    (better tool handling in its chat interface, key scoping), not by leaving these tools out. K30.
28. **Every tool can be set to `allow`.** Kai does not patronize the user: the machine may be
    dedicated to agent use, or the user may be overloaded by confirmations (a user who prefers
    fewer interruptions, say). K24's "always confirmed" class becomes "confirmed by default", and K27's caller level is a
    default the owner can widen. K15, K24, K27.
29. **Personality profiles are an lmgw Chat feature, not Kai's.** A voice turn's prompt is the
    thread prompt (by default lmgw's built-in chat prompt, an explainer that answers in Markdown)
    plus a voice block that only changes form (lmgw chat-voice design §8.5), so Kai's spoken
    replies come out as walls of text. The personality must be the same from Kai, the phone and
    the dashboard, and the thread owns the prompt. Voice chat profits through voice profiles. Kai
    picks the profile its conversation uses from the tray, and a tool `personality_set {name}`
    switches it by voice. Until lmgw has profiles the owner keeps the current prompt (no interim
    prompt). K31.
30. **`overlay_weather` takes its data from DWD through Bright Sky** (2026-10-09). Bright Sky is the
    free public JSON API for DWD (Deutscher Wetterdienst) open data, `https://api.brightsky.dev`,
    with no key. It is the one fetch besides images that Kai makes. K19, §16 Q19.

Rulings of 2026-10-09 (the lock, conversation mode's timeout, accessibility):

31. **Kai is not active on a locked screen.** When the session locks: listening stops (the capture
    closes, a press session ends), conversation mode ends, Kai's speech stops, and every hosted tool
    call is refused with "the screen is locked; Kai is not active". The approval card does not show
    while locked (its calls stay waiting for other clients and show again after the unlock if they
    still wait); the talk key does nothing; nothing of Kai's is visible or interactive on the lock
    screen. After the unlock Kai is idle: conversation mode does not resume. The tray shows the
    state. K32.
32. **Muted time counts toward `voice.conversation_timeout_s`.** The silence timeout runs on while
    muted (it used to pause). K27.
33. **Kai may switch accessibility, `IsEnabled` only.** Measured (WP13 (a)): with
    `org.a11y.Status` `IsEnabled` alone, Qt apps expose their trees at a small memory cost in
    plasmashell; `ScreenReaderEnabled` adds nothing and costs noticeably more memory. The rule: on while
    conversation mode is active; in push-to-talk and other modes only around a read (switched on,
    the apps waited for, read, the earlier value put back). Never `ScreenReaderEnabled`. Restore
    puts back the value it had before Kai switched it (on already: Kai leaves it on and never
    switches it off), and a change of the user's in between wins. Restored at conversation mode's
    end, at the lock and at Kai's exit. The wait for the apps is no guessed sleep but the
    registry's events, bounded by a visible setting. K33.

## Taken in this draft

**K1. The RAM call: the ring has its own web process. Agent widgets share a second one as related
views. Every view is destroyed when hidden** (§9.4, §10.1; the ring's view changed 2026-10-08, see
below). *Why:*
- **The ring is Kai's state display while it hears and speaks.** Agent HTML and third-party MCP
  Apps must not take it down with a crash, a hung script or a runaway canvas.
- **Widgets share one crash domain.** That is acceptable: they are all the agent's output, and the
  agent can show them again.
- **RAM, from the spike's PSS figures:**
  - The UI process is about 73 MB with one view. Idle, measured in WP0 (o): 16 MB while the
    process never used WebKit, 41 MB once a `WebContext` exists, and 60 to 63 MB after the last view
    on the default context (it does not go back).
  - The ring's web process adds about 99 MB while it shows (one view, warm run; a cold first run read higher).
    With a session bound this is the ring's RAM.
  - The first widget adds 66 to 72 MB, and each further widget 15 MB. Unrelated views would cost
    66 to 72 MB each.
- **Destroyed when hidden.** Kai runs all day, and the ring shows only during turns (K11).
  Respawning a web process costs one spawn and one page load: the ring's first frame comes about
  250 ms after the view is created (cold 234 to 267 ms, warm 248 to 305 ms; no warm gain, because the
  web process respawns every time). So the ring's view is created hidden when a voice session binds,
  ready for the first `speech_started`, and destroyed when the session ends. Kai at idle, with no
  session, has no view. *(WP0, 2026-10-06)*
- *Measured in the build, 2026-10-07* (WP5's live check, `just live-check-ring`; PSS):
  - a bound session's hidden view: web process 71 to 75 MB, network process 14 to 15 MB; shown and
    speaking, the web process reads 147 to 151 MB;
  - no WebKit process 0.5 s after the session ends;
  - the first frame comes 78 to 129 ms after a turn starts on a pre-created view's first show, 34 to
    41 ms on later shows, and 231 to 258 ms after the press for a view created at the press;
  - the UI process (the live check's, with its audio threads) reads 31 MB before WebKit, 64 MB
    with the hidden view, and about 120 MB once a view was shown, also after the session: heap
    and the NVIDIA driver's mappings (`nvidiactl`, `libnvidia-gpucomp`, `libnvidia-eglcore`)
    stay. Kai at idle, never having had a session, reads 36 MB with no WebKit process.
- *Changed during the build, 2026-10-08 (the owner's decision, K11):* the ring now shows whenever
  Kai is active, and continuous listening is active all day. The ring's view already lived hidden
  while a session was bound and went at the session's end; it now also lives while Kai is active
  with no session (one on its way), and goes once neither holds and the ring has eased to idle
  (§9.4). With continuous listening on all day, the shown figure is the ring's RAM all day.
  *Measured in the build, 2026-10-08* (`just live-check-ring`, quiet continuous listening with the
  ring shown): web process 148 to 149 MB, network process 16 MB.
- **Every web view uses its own non-default, ephemeral `WebContext`**, created per use and dropped
  with its view. With the default context the network process (about 14 MB) stays alive after the
  last view; a dropped ephemeral context leaves no child process 0.5 s later. *(WP0, 2026-10-06)*
- *Decided by the owner, 2026-10-09:* the memory budget covers only the persistent background
  service at rest, not ephemeral windows or overlays. The figures above stay the
  measure of Kai at idle (no view) and of the ring, which lives while Kai is active; a window
  opened on demand (the configuration window, K29) and the overlays an agent shows are reported
  with their figures, not held against the budget. Destroying them when hidden or closed stays the
  rule: nothing of them lingers at rest.
- **Rejected:**
  - one shared web process for everything: a widget's crash would take the ring with it;
  - one process per widget: +66 to 72 MB each, for isolation between widgets nobody needs;
  - all widgets in one page (+6 MB each): one layout and one script heap for unrelated content
    from different servers.

**K2. The device-hosted MCP link (api §5) lands in phase 2.** Tool-controlled overlays are tools
the agent calls. In a bound session the thread owns the tools (chat-voice §8.1), so they must reach
the agent as Kai's hosted server.

**K3. `lmgw-client` lives in lmgw's workspace** (api §4.1). It takes over the dashboard's pure
realtime rules (`crates/lmgw-ui/src/pages/chat_voice/realtime/machine.rs` and `protocol.rs`), so
the dashboard and Kai run one set of rules for state derivation and truncate-first barge-in. Kai
depends on it by path until lmgw tags a release with it.
*WP4, 2026-10-07:* until the crate exists, Kai carries a port of `machine.rs` (lmgw 116051e) in
`src/voice/machine.rs` and reaches lmgw only through the seam `src/voice/link.rs` (`VoiceLink`:
`current`, `open` a bound session, `send` the client events, `close`; the server events as typed
OpenAI realtime GA and `lmgw.*` events, then one `Closed`). The `lmgw-client` adapter implements
the seam and replaces the port, rule for rule.
*Changed during the build, 2026-10-07 (WP1):* Kai depends on `lmgw-client` by path (lmgw's
`feat/client-api` worktree until that branch merges, then `../lmgw/crates/lmgw-client`), and the
port is gone. The voice state is the crate's `voice::Machine`, each response's audio its
`truncate::Replies`, the truncate's position its `PlaybackCursor`, and `VoiceLink`'s events are its
`realtime::ClientEvent` and `ServerEvent`, so the adapter only moves text frames (`to_json`,
`realtime::parse`). Where the port differed from the crate:
- the per-response bookkeeping (item, ended, complete, cut) is the crate's; the crate leaves the
  player to the client, so Kai keeps beside it which replies were heard to their end and which
  reached the player;
- a cut still flushes and truncates every item in the player (§5.5), where the dashboard flushes
  only the cut reply's;
- `session.updated` is read through the crate's `Updates`: only the answer to the last update moves
  the pre-roll. Kai still sends no update that repeats the last one;
- a tool turn (chat-voice §8.5) now reads `thinking` once its spoken preamble was heard while the
  tool runs: a `tool` frame's `start` or `ready` and its `result` (or the turn's `done`) set the
  machine's tool, and a reply whose audio that came was all heard while more is to come is
  starved;
- `next_change`, `key` and `label` are not used: the 20 ms tick runs while a session exists, and the
  tray and the ring have names of their own.
*Built, 2026-10-07 (WP4's real half):* the adapter is `lmgw_link::RealtimeLink`.
- `current` goes over the feed's reqwest client, bounded by `lmgw.timeout_s` for the answer and for
  its body; a refusal is its message and code.
- The bound session is a WebSocket (tokio-tungstenite) with the device key as the bearer. A `wss:`
  gateway gets the feed's TLS (rustls on aws-lc-rs, the system's trust store through the platform
  verifier).
- A refusal before the 101 is `read_refusal`'s code and message; an unreachable gateway is a refusal
  that says why. Events sent before the 101 wait in order; a close asked for before it gives the
  handshake up.
- The session ends at lmgw's close frame, with its code and reason, without waiting for the
  connection's end; an end without one is 1006 with what broke it.
- tungstenite's caps on a message and a frame (64 and 16 MiB) are lifted: the socket has no limit of
  its own (ruling 9).
*Changed during the build, 2026-10-07 (review KAI-2):* Kai uses three additions of `lmgw-client`
(lmgw `feat/client-api` da034d8, 22532e3, 096406d): `requests::realtime_unless_bound` for its own
binds (K5), the thread's `last_message_at` for the idle rollover (§4), and
`requests::folder_settings_page` for Open in lmgw while the folder waits for the owner's steps (§4).
The socket now has two bounds of its own, both settings: a ping keep-alive and a bounded outgoing
queue (§5.2).
*Changed with lmgw main, 2026-10-07 (lmgw 66bb7fb, after the client-apps merge):* the path is
`../lmgw/crates/lmgw-client`, lmgw's checkout next to Kai's, which resolves from Kai's main
checkout and from a worktree beside it alike; `packaging/build-rpm.sh` reads it from `Cargo.toml`
as before. Two of Kai's own stand-ins went with it: the replies are walked from the crate's
`Replies::items` instead of a list of ids kept beside it, and a reply whose response is done and
whose audio was heard to its end or cut is forgotten (`Replies::forget`), so a long session's
bookkeeping stays small (K2-18's bound, now the crate's own).
*Changed during the build, 2026-10-07 (review KAI-3 K3-1, K3-2):* "whose response is done" is now
`response.done` itself, kept in a set of the session's own: the machine's open set also drops a
response once its audio has played out, so it forgot every played reply before its
`response.done`, and a late delta of it read as a new reply's. A reply is forgotten once its
`response.done` came and its audio was heard to its end, was cut (a cut before its first audio
included), or never came; until then a played reply stays ended, and a late delta of it is
dropped.
*Changed during the build, 2026-10-07 (review KAI-3 K3-5):* Kai is pinned to an lmgw commit,
`LMGW_PIN` (66bb7fb, lmgw main's HEAD then). `just check` and the live checks against a dev lmgw
still build with whatever `../lmgw` holds, and warn (never fail) when its HEAD is another commit
or its tree has changes; `just rpm` stages the two crates and the workspace root's
`[workspace.package]` from the pin with `git archive`, so the RPM does not depend on what is
checked out there, and its description names the commit. CI clones lmgw at the pin. A worktree of
Kai is a sibling of lmgw's checkout (`../kai-wt-<name>`), never nested in another repository. The
docs/development.md says how to bump the pin.

**K4. Voice starts off at login.** The first press or toggle opens the first session, and the
thread's models load on that press (chat-voice §4.1, §16). An autostarted Kai never evicts models
at login. `voice.on_at_start` (default false) changes it.

**K5. The session lifecycle** (§5.2):
- "Listening on" is one session bound to the folder's current thread.
- With listening off, a hold-to-talk press opens a session that closes `voice.grace_s` (default
  30) after its reply has played.
- Kai rebinds at a quiet point after a rollover. It never binds a thread another client's voice
  session holds (feed `voice.bound` by someone else): it closes its own session, turns listening
  off and waits for a press.
- A session that drops for any reason but a takeover (lmgw restarting, the network) turns listening
  off, with a notification. Re-opening would warm models in an lmgw that was just started, so Kai
  does not reconnect by itself (chat-voice §8.6).
- *Changed during the build, 2026-10-07:* "never binds a thread another client's voice session
  holds" applies to the binds Kai makes by itself: a rebind after a rollover and
  `voice.on_at_start`. A talk press or the tray is the owner's own choice and binds anyway, taking
  the thread over, as §5.6's "the next press takes the thread back" says. A press session (listening
  off) whose thread rolls over does not rebind: it closes at its grace end, and the next press binds
  the new thread.
- *Changed during the build, 2026-10-07 (review KAI-1 K-17):* a press session whose thread rolls
  over closes at its next quiet point, not at its grace end; it still does not rebind, and the next
  press binds the new thread. Kept, and the note above amended, because it serves the owner
  better: until the grace end a press would still talk into the old thread after the conversation
  moved, and "New conversation" from Kai's own tray would be ignored by every press for up to
  `voice.grace_s`. The reconnect a press then needs loses no words (§5.3's buffer).
- *Changed during the build, 2026-10-07 (review KAI-2 K2-13):* the feed's view of who holds a
  thread races another client following the same rollover. So Kai's own binds (a rebind,
  `voice.on_at_start`) also ask lmgw never to take over (`/v1/realtime?chat_thread=<id>&takeover=
  never`): while another session holds the thread lmgw refuses with 409 `chat_thread_bound`, naming
  the holder, and Kai stays off with a notification until a press. A press or the tray binds as
  before and takes the thread over; so does the rebind of the owner's own New conversation.
- *Changed during the build, 2026-10-07 (review KAI-2 K2-7):* one bind is in flight at a time. A
  bind that waits for the last close (one slot per session, §5.2) binds the conversation's thread
  as known when the close is answered; a press, listening on or New conversation meanwhile joins
  that wait instead of starting a bind of its own, so a second session never opens beside the first,
  and New conversation binds the new thread. A bind never replaces a session.

**K6. One talk key** (`shortcuts.hold_ms`, default 300):
- a short press toggles listening; a hold talks;
- a press that stopped Kai speaking consumes the gesture, so a tap meant to interrupt never also
  turns listening off;
- repeated press events while the key is held are ignored.

*WP4, 2026-10-07:* in automatic mode with listening on, a hold needs no commit (the server's detector
ends the turn); it shows the ring from the press and sends the microphone even while K16 holds
back. A press while Kai only thinks (a response open, no audio yet) stops nothing in automatic mode;
in push-to-talk it cancels that response before the new turn, as the page's press over the voice
does.

*Changed during the build, 2026-10-07 (review KAI-1 K-2):* `shortcuts.max_hold_s` (default 300) is
a stuck-key guard, not a limit on speech: a hold that no release ended in that long ends as a release
would, logged, and the tooltip says so until the next press. Press and release reach the voice in
their true order (§8.2).
*Changed during the build, 2026-10-07 (review KAI-2 K2-3, the owner's decision):* the guard no
longer ends the hold as a release would, which committed up to `shortcuts.max_hold_s` of the room as
a turn. A hold no release ended is no deliberate turn: in push-to-talk (and a press session) its
audio is discarded, the server's buffer cleared with no commit and no `response.create`; in
automatic listening the hold only ends. One notification and the tooltip say "talk key released by
the guard after N s; nothing was sent", and the log says it too.

Two more actions: new conversation and mute.

**K7. Audio goes through pipewire-rs, not cpal.** Kai needs:
- stream properties (node name, `target.object`, app name);
- the default-device and echo-cancel nodes `aec.rs` manages;
- the stream's delay from `pw_stream_get_time_n`, which turns "samples handed over" into "samples
  heard" for the truncate.

pipewire-rs 0.10.1's `Stream::time()` (cargo feature `v0_3_50`) calls `pw_stream_get_time_n`, so no
`pipewire-sys` is needed (probe in `spikes/wp0/pwprobe`). *(WP0, 2026-10-06)*

*Changed during the build, 2026-10-06:* the stream's delay is `queued + buffered + delay` of
`pw_time` (`buffered` is the resampler, which a 24 kHz stream on a 48 kHz graph always has; Kai sets
`pw_buffer.size` in frames, so `queued` is in frames). Only the playback callback sets
`pw_buffer.size`, so it goes through pipewire-sys's raw buffer calls (`pipewire::sys`, re-exported).

**K8. The default echo mode is `device`**, as in the Chat (chat-voice ruling 3): the owner's input
device cancels echo against the default output's monitor.
*Note, 2026-10-07:* `device` suits any setup whose input source is already echo-cancelled
(hardware AEC, or a driver that processes a microphone array's raw channels). `app` stays a
first-class mode for setups without such a source (§6.2).

**K9. Kai ships its own tray icon set** (hicolor SVG, graphite and blue, never pastel). The hold is
an amber badge on every state, not a state of its own, so it shows while listening too. Unreachable
is grey. *Changed during the build, 2026-10-06:* the tray sends each icon by name and as ARGB
pixmaps rendered from the same SVGs. In the live check, plasmashell resolved neither names
installed into a hicolor directory created after it started nor names under `IconThemePath`, and
showed its unknown icon. Plasma's tray falls back to `IconPixmap` when a name does not resolve. The
status icons are named `kai-tray-*`, not `dev.rootshell.kai-*`, so that a missing one cannot fall
back to the app icon by the naming spec's dash rule.

**K10. Overlay assets are served from a custom `kai://` URI scheme**, embedded in the binary, as in
the spike. `file://` works only with `allow_file_access_from_file_urls`, which lets every page read
any local file URL; the scheme keeps ES-module imports same-origin without exposing the filesystem.
`kai://` is verified, including under a strict CSP set by `<meta>` or by response header. The page
boot must be an external module (`ring.html` loads a `boot.js`), since inline scripts are blocked;
setting response headers needs `soup3` as a direct dependency. *(WP0, 2026-10-06)*
*Changed during the build, 2026-10-07:* the response headers replace the response's content type,
so `Content-Type` goes into them as well; without it `ring.css` was applied as an empty sheet under
`nosniff`, and the ring sat 8 px off-centre. The page's CSP needs no `'unsafe-inline'` at all:
`default-src 'none'; script-src 'self'; style-src 'self'; base-uri 'none'; form-action 'none'`,
sent as a response header. Verified: an inline script and an inline style are refused.

**K11. The ring shows only during a turn** (changed 2026-10-08, see below: it shows while Kai is
active): from `speech_started` or a talk press to the end of the reply's playback. Quiet
listening shows nothing on screen, only the tray icon. While shown, the ring is fed 30 times a
second through `evaluate_javascript`. At 30 Hz the transports are within noise (11 to 13 % of one
core in total), so `evaluate_javascript` stays and there is no
WebProcessExtension (WP5). *(WP0, 2026-10-06)* All day listening thus costs no rendering. The ring's view exists
hidden while a voice session is bound (K1); a hidden page's rAF is paused, so it draws nothing. *(WP0, 2026-10-06)*
*Measured in the build, 2026-10-07:* during quiet listening the web process used 0.0 % of a core
over 2 s, the engine drew no frame, and no level reader existed. While speaking: the UI process 8
to 10 %, the web process 6 to 8 %.

*Changed during the build, 2026-10-08 (the owner's decision):* the ring shows while Kai is
**active** (actively listening, thinking or talking), and is hidden while Kai is in **standby**.
While active and out of a turn it is mapped on the active output in the engine's listening state
(amber), fed the microphone's levels; a turn shows its states as before. Entering standby eases it
to idle and hides it once the page reports that drawn, as a turn's end did before. `Voice::standby`
decides, by mode:
- **Continuous listening** (automatic mode with listening on) is active the whole time it is not
  muted, so the ring stays drawn throughout, and a turn's end goes back to the listening state.
  It stays active through a rebind and the bind after a 4004: the audio in between is buffered and
  sent (§5.3).
- **Push-to-talk** (the owner's decision, 2026-10-08): Kai listens only while the talk key is held,
  so it is in standby between presses and the ring is hidden then. It is active from the press
  through the end of that turn, as before. A press session (hold-to-talk with listening off) is
  the same. *Changed during the build, 2026-10-08 (review KAI-5 R5-1):* a release that comes
  before the press's session exists (its bind waits for the last close, or the thread is still
  asked for) is still that turn, since the session commits it once created: the ring stays up
  without a gap, on the same view. It counts only while that session is really on its way, so a
  bind that gave up cannot hold the ring up.
- **Standby** also covers: muted, no session and none on its way, listening off, a drop or a known
  4003, K16's silence during another principal's turn, and PipeWire down. The GPU hold (K9)
  changes nothing. An unknown 4003's one rebind stays active, like a 4004: the audio in between is
  buffered and sent.
- **A hotword mode**, when it comes, is in standby while it waits for its word, so the ring is
  hidden; it is active from the word through the end of that turn. `Voice::standby` is a function
  of the current state, so the mode adds one fact that it reads: the word fired, set at the word
  and cleared at its turn's start, or by a timeout when no speech follows the word (review KAI-5
  R5-7).

*Measured in the build, 2026-10-08* (`just live-check-ring` on a 60 Hz
display, quiet continuous listening with the ring shown, 10 s each; the UI process is the live
check's, with its audio threads):
- in silence, before the change below: the UI process used 7.0 % of a core and the web process
  7.4 %; the engine drew 60 frames a second and Kai fed 30 a second;
- Kai now sends no feed equal to the last one: the page's analysers hold it, so the engine draws
  the same. In silence that is no feed at all. In one run, in silence, as Kai feeds now: the UI
  process 6.8 %, the web process 6.3 %; with every frame sent again (a test switch): 7.8 % and
  6.9 %. Other runs as Kai feeds now: the UI process 6.7 to 7.0 %, the web process 6.2 to 7.8 %
  (one run under another build's load);
- in a quiet room (white noise at -60 dBFS on the microphone, below the level's floor) every
  feed differs, so nothing is saved: the UI process 7.9 to 8.7 %, the web process 6.9 to 8.0 %;
- speaking: the UI process 7.0 to 9.0 %, the web process 6.0 to 8.0 %;
- in standby (muted, or push-to-talk between presses): 0.0 to 0.5 % each, no frame drawn;
- the web process's PSS while shown: 148 to 149 MB.

*Measured in the build, 2026-10-08 (review KAI-5 R5-4):* the compositor and the GPU, read only
(`kwin_wayland`'s CPU from `/proc`; `nvidia-smi` every 500 ms for the P-state, power draw, graphics
clock and utilisation), in two `just live-check-ring` runs: 10 s each with no view of Kai's at all,
quiet continuous listening with the ring shown (silence, as Kai feeds; every frame sent; a quiet
room's noise), muted (the view hidden), and shown again, on an NVIDIA test system with a 60 Hz
display. Each window starts once the check's screen captures are written and the GPU had 3 s: in an
earlier run a capture just before a window held the GPU in a higher P-state for a third of it.
- `kwin_wayland`: no view 0.5 %, hidden 0.4 to 0.7 %, shown 3.1 to 4.1 % of a core. The shown
  ring makes KWin composite at the display's rate: about 3 % of a core more.
- On an NVIDIA test system, the shown ring keeps the GPU in its lowest P-state (but for a single
  sample in one shown window) and adds a small, measurable amount of GPU power (no higher P-state); the graphics clock rises a little and the
  utilisation by a few percent. The shown ring costs the GPU a small, measurable amount of power.
- All told, the ring shown all day costs about 18 % of one core (the UI process 7.2 to 8.3 %, the
  web process 6.2 to 8.6 %, KWin 3.1 to 4.1 %) and a small, constant GPU cost in
  continuous listening. Muted or between push-to-talk presses it costs nothing measurable. Review
  R9 is open again on this.

*Measured in the build, 2026-10-08 (a report that the ring showed speech seconds late):* how late the ring shows a sound. Five tone bursts were played into the private instance's
test microphone while the ring was up in quiet listening, in three `just live-check-ring` runs:
- the samples reached Kai's capture 3 to 18 ms after the push, the feed read them 8 to 46 ms after
  it, and the page drew its first frame with them 15 to 57 ms after it;
- Kai's part, from the capture to the drawn frame: medians 19 to 35 ms, worst 36 to 44 ms.
- The check holds Kai's part to two feed intervals and two of the display's frames, 100 ms at
  60 Hz: the wait for the next feed, one feed skipped while the previous call completes, the frame
  the call just missed, and the one that draws it. The rig's own injection is held to three
  capture quanta (64 ms at its 512 samples): the player's cycle, the test sink's monitor one period
  later, and Kai's capture cycle.
- Kai's part is tens of milliseconds, so a delay of seconds lies outside Kai's path, before the
  samples reach Kai's capture.
  *Changed, 2026-10-08 (review KAI-6 R6-7):* the frame interval is the display's, from GDK's
  refresh rate for the ring's monitor, no longer the page's own rate, which a slow page would
  loosen. The page's rate is checked against the display's instead: within a frame a second.

What remains is the engine drawing at the display's rate, and KWin compositing it. It drops to 15
frames a second (`IDLE_FPS`) only in idle, never in listening, although in silence every listening
frame is the same. A calm rate for quiet listening is an `engine.js` change in lmgw (review R9's
second fix), the mitigation for R9 now; it is not made here, since the vendored files stay
byte-identical. It would change the look in two places: a room's faint noise on the top bands
would move at 15 frames a second, and the first reaction to speech could come up to 67 ms later.

**K12. The app id is `dev.rootshell.kai`.** Dev builds run in a scope named
`app-dev.rootshell.kai-dev<n>.scope`, with no dash in the random part, so the portal parses the id
as `dev.rootshell.kai` (§8.3). The no-dash rule is confirmed, and the portal also needs a matching
`dev.rootshell.kai.desktop` in the XDG data dirs: without one it refuses even a correct scope name
("An app id is required"). Dev builds therefore need that file installed, and the dev recipe installs
it. *(WP0, 2026-10-06)*

**K13. Pairing uses a `lmgw-pair:` link** (api §1.4), handled by Kai's `.desktop` scheme handler or
pasted (§3.1).
- The handler never logs its argv, which carries the key.
- It hands the link to a running Kai over D-Bus only.
- When no Kai runs, it stores the key and URL itself, starts Kai through its desktop file
  (through `systemd-run --user --scope -u app-dev.rootshell.kai-<rand>.scope`, with no dash in
  `<rand>`, so Kai gets its own app scope, not the browser's), and exits. `gtk-launch` makes no scope:
  the child inherits its caller's, and the portal then sees the caller's app id. *(WP0, 2026-10-06)*
- *Changed during the build, 2026-10-06:*
  - The hand-off is the method `Pair` of `dev.rootshell.kai.Pairing1` at `/dev/rootshell/kai`,
    bounded by `pairing.handoff_timeout_s` (default 15), which is named in its error.
  - The handler starts its own executable, the binary the desktop file's `Exec` names.
  - With the wallet locked, the handler cannot store. It stores nothing, notifies "unlock KWallet,
    then use the pairing link again", and starts Kai anyway. A running Kai, by contrast, keeps a
    handed-over link until the wallet opens.
- *Changed during the build, 2026-10-07 (review KAI-1 K-1, K-4, K-5, K-21, K-22; api §1.4: "a
  client must confirm before it replaces an existing pairing, showing the new url"):* **nothing
  pairs without the owner's confirmation in Kai's own dialog.** This replaces "stores the key and URL
  itself" and "keeps a handed-over link until the wallet opens" above.
  - Every arrival is unauthenticated: a browser click (its external-protocol prompt names the app,
    never the url) and any process of the session calling `Pair` (the session bus lets every peer
    of the same user through). So a link waits in memory, never on disk, and Kai's GTK confirmation
    dialog shows the gateway URL, the device name (control and bidirectional-formatting characters
    replaced, cut past 64 characters), the pairing it replaces with that pairing's URL, and a
    warning for `http://` to a host that is not this computer (the key and the voice would travel
    unencrypted). Cancel is the default; closing the window declines; declining changes nothing.
  - Nothing is stored, stopped or switched before the answer, "Start with Plasma" included: only a
    confirmed first pairing switches it on. The pairing Kai has runs on meanwhile.
  - One confirmation at a time: a newer link replaces the waiting one and its dialog says so; an
    answer for the replaced one is ignored.
    *Changed during the build, 2026-10-07 (review KAI-2 K2-4, the owner's decision):* this rule is
    replaced. While a link waits (for the wallet or the owner's answer), every further link is
    refused: the D-Bus `Pair` answers "a pairing is already waiting for confirmation" (a gate it
    claims and the core releases once the link no longer waits), and a link on stdin is said the
    same way. So nothing swaps under the owner's pointer, a timed link cannot take the click meant
    for another, and arrivals cannot flood. `Pair` therefore answers alike only while nothing
    waits.
  - With the wallet locked the link waits in memory: the tray says so, one notification asks to
    unlock KWallet, and the working pairing is untouched. Once the wallet opens (checked every
    `wallet.retry_s`) the owner is asked, then Kai stores.
  - The paste dialog is its own confirmation: it parses the link as it changes and shows the same
    facts before its Pair button can be pressed. A locked wallet is said in that dialog; nothing
    waits.
  - `Pair` answers "awaiting the owner's confirmation in Kai" at once, the same for every link that
    parses (a link with `fp` is refused there), so the hand-off never waits on the owner, and the
    answer says nothing about the wallet. `pairing.handoff_timeout_s` now defaults to 5; its old
    relation to `wallet.timeout_s` is gone.
  - **The handler exits as fast as it can,** since its argv carries the key. It parses the link and
    hands it to the running Kai; with no Kai running it writes the link into a pipe, starts `kai
    --pair-stdin` with that pipe as stdin through `systemd-run --scope` (which execs Kai in place)
    and exits without waiting. The started Kai reads the link (within `pairing.handoff_timeout_s`);
    if another Kai won the race, it hands the link on. The handler stores nothing and waits for no
    confirmation. Every bus call it makes is bounded: the hand-off by `pairing.handoff_timeout_s`,
    a notification and reading the session's environment by the new `dbus.timeout_s` (default 5).
  - A Kai the handler starts gets the systemd user manager's environment (what the Plasma session
    put there) plus Kai's own variables from the handler (`KAI_LOG` and the XDG base directories),
    not the caller's: a browser's `GDK_BACKEND`, `LD_PRELOAD` or tokens stay behind.
  - The rest of Kai's D-Bus surface keeps the usual same-user trust: any peer of the session can
    also drive the tray (SNI `Activate`, dbusmenu): listening, mute, the echo mode and devices,
    Quit. Only pairing asks the owner, because it hands the microphone and, from phase 3, the desktop
    tools to another gateway.
- *Changed during the build, 2026-10-07 (review KAI-2):*
  - **The threat model** (K2-15): the confirmation guards against web content and links followed by
    mistake. Code running as the owner is trusted: it could press "Pair with this gateway" over the
    accessibility bus, read the unlocked wallet or write `config.toml`. Later work must not lean on
    the dialog for more.
  - **The paste dialog** (K2-5) checks what the link replaces at the commit: it sends the pairing
    it showed, and when `lmgw.url` changed meanwhile (a link confirmed while it was open) Kai refuses
    the Pair and the dialog shows its facts again against the pairing Kai has, so no key is deleted
    that the owner was not shown. Once paired, a clipboard that still holds exactly the link gets a
    note in its place (K2-20; not emptied, since Klipper's "prevent empty clipboard" would put the
    link back), and the dialog says Klipper's history may keep a copy.
  - **The device name** (K2-14) also has format characters (category Cf: zero-width, the soft
    hyphen, tags) and line and paragraph separators replaced, so it cannot add a line under itself.
  - **A handler-started Kai** (K2-23) runs as `systemd-cat -t kai kai --pair-stdin` in its scope,
    so its log goes to the journal, not the browser's stderr. The session-bus connect of every
    start is bounded by `dbus.timeout_s` too, so "every bus call it makes is bounded" now holds.
  - **The live-test build** (K2-2) confirms only the links the live script minted
    (`KAI_LIVE_LINK_ACCEPT`: each link's URL and the SHA-256 of its key); any other gets the
    owner's dialog. It is built into `target/live-test`, never `target/debug` (the dev handler and
    `just run`), and refuses a configuration outside a `target` directory.

**K14. Programs Kai launches run in their own `app-*.scope`** (phase 3), never as Kai's children.
They survive Kai and never inherit its restricted interfaces.

**K15. Kai, as an MCP server, keeps its own per-tool policy**, `allow | confirm | deny` (phase 2
and 3):
- **Trust by caller** (`_meta["lmgw/caller"]`, api §5.5). Read-only tools are allowed for owner and
  gateway callers and for Kai's own key. Every other caller gets `confirm`, unless it is in
  `policy.trusted_callers`. State-changing tools are always `confirm`.
- **Trust by approver.** A call carrying `_meta["lmgw/approval"]` is not confirmed again only if
  its approver (`by`) is the owner, the gateway, Kai's own key or in `policy.trusted_approvers`.
- **Cancelled calls never run** (api §5.4). A confirmation closes on `notifications/cancelled`, and
  shows the call's remaining time from `lmgw/timeout_ms`.
- **F (approvals in the thread) is the path for thread calls.** K15's own confirmation covers
  callers that have no F, such as `/mcp` clients.
- *Changed, 2026-10-08 (K23, K24, K27):*
  - read-only, state-changing and destructive are read from each tool's MCP annotations in the
    `kai-desktop` contract (`readOnlyHint`, `destructiveHint`), not from a hand list;
  - K24's confirmed-by-default tools (`run_command`, `ui_press`, `type_text`, `key_press`) are
    confirmed for every caller unless the per-tool policy sets them to `allow`;
  - a third caller level, **Kai's own session only**: by default `conversation_start` and
    `conversation_end` are denied outright to every other caller, not confirmed.
- *Changed, 2026-10-09 (ruling 28, K30):*
  - every tool can be set to `allow`, the confirmed-by-default ones included; a caller level is a
    default the owner can widen;
  - a fourth per-tool state, `off`: the tool is not listed (K22's `list_changed` when it changes).
    The configuration shows the states as on (`allow`), ask (`confirm`), deny and off.
  - *Taken in this draft, 2026-10-09:* a fifth, `allow_trusted` ("allow
    if trusted"): unasked for trusted callers, confirmed for the others (unless a trusted approver
    approved), the read-only default made settable for any tool. It is how the owner grants a
    routine what its `routine.toml` asks (K28).
- *As built in WP8 per PD4, 2026-10-09 (taken in this draft):* a trusted
  approval in the thread (F, `lmgw/approval`) is the confirmation for confirmed-by-default tools
  too, so there is no second dialog; this resolves the older wording above, "asked for every caller
  unless set to `allow`". *Changed by WP19's review, 2026-10-09:* not for K24's
  confirmed-by-default tools: their dialog names the window or element the call acts on, which no
  approver in the thread saw, so it asks after any approval unless the tool is set to `allow`.
  *Restored after WP19's second review, 2026-10-09 (taken in this draft):* a trusted approval decides K24's confirmed-by-default tools again, as WP8 built it: a
  remote approval with nobody at the desk would otherwise time out on Kai's dialog, and since
  ff1b9e8 a call approved in lmgw must name its window (`type_text`, `key_press`, `ui_press` and
  every window action refuse an omitted or `active` one), so the approver saw the target in the
  arguments. Only `routine_enable` switching a routine on still asks after F
  (`policy::approval_applies`).
- *WP19, 2026-10-09 (taken in this draft):* an approval whose approver
  (`by`) is the call's own caller is no approval, unless that caller is the owner, the gateway or
  Kai's own key; the call is then decided as if nobody had approved it. It compares principals: an
  approver holding a second principal's key can still approve its own call through it (WP19's
  notes).

**K16. Automatic listening holds back while another principal's turn runs in Kai's thread** (feed
`turn.started` with a `by` that is not Kai, until `turn.done`): Kai sends silence. Otherwise the
room (a TV, a visitor) would cancel the owner's text turn from the phone. A talk press still wins:
it is deliberate. *Note, 2026-10-08:* this holds in conversation mode too (K27).

**K17. An approval answered by voice never enters the thread** (§11.1). While the approval overlay
is open:
- Kai sends the session no audio;
- the answer is recorded locally while the talk key is held, and transcribed through the dictation
  route (`POST /chat/api/threads/{id}/transcribe`);
- Kai then sends `mcp_approval_response` and `response.create`.

Otherwise the spoken "yes" would become a user message and decline the call it approves.
*Changed by WP19's review, 2026-10-09:* this holds for **Kai's own calls** (its bound session's,
its conversation thread's) only, and the talk key answers only once the overlay said it armed for
the call it shows; a recording with no speech in it is not sent. Calls of other threads are
answered on the overlay and hold back nothing of Kai's voice.

**K18. The MCP Apps bridge answers only views it placed** (§10.1):
- only `overlay_app` frames, identified by `MessageEvent.source`;
- a view may call only tools of the server that served it;
- every call not marked `readOnlyHint` goes through K15's confirmation;
- `overlay_html` gets no bridge.

*Changed, 2026-10-09 (ruling 24):* the tool names; was `kind: "ui_resource"` and `kind: "html"`.

*Changed by the owner, 2026-10-09:* views of apps the owner trusts may skip the confirmation: a
view's calls run as `app:<server>`, a `policy.trusted_callers` entry like any caller's, and
`policy.tools` applies to the server's tools by their exposed name (WP12's note).

*Changed after review, 2026-10-09 (taken in this draft):* the skip covers
only tools the server marks `destructiveHint: false`. A destructive one (MCP's default for a tool
that is not read-only) still asks a trusted app unless `policy.tools.<server>__<tool> = "allow"`.
While any Kai confirmation is open, every MCP Apps view is click-through and gives way to Kai's
note, so it cannot catch a click meant for the dialog or pass for it (§10.1).

**K19. Kai's image fetch is bounded** (§10.1). *Changed, 2026-10-09 (ruling 30):* Kai fetches
nothing but images and, for `overlay_weather`, weather data from Bright Sky; `api.brightsky.dev` is
the only allowed weather origin (see the weather details below):
- the body streams against `overlay.image_max_mb` (default 32, named in the tool's error) and is
  decoded before WebKit sees it;
- loopback, link-local and private (LAN) addresses are fetched like any other; the owner
  approved this on 2026-10-06 (§16 answers). `overlay.fetch_private_addresses` (on by default)
  can switch it off, and a refusal then names the setting and is logged;
- errors name status and size, never the body.

**K20. App AEC survives a crash** (§6.2):
- the real default devices are written to `$XDG_STATE_HOME/kai/aec.json` before Kai flips them, and
  restored from there on the next start;
- the `pipewire -c` child dies with Kai (`PR_SET_PDEATHSIG`);
- the wait for the module's nodes is `aec.ready_timeout_s` (default 4, from `aec.rs`), named in its
  error.

*Changed during the build, 2026-10-06:*
- **Kai's nodes are `kai_aec_sink`/`kai_aec_source`**, not `aec_sink`/`aec_source`: an earlier
  prototype's PipeWire drop-in uses those names, so Kai's must differ. There is no reuse path; an
  existing pair of Kai's names is reported as someone else's.
- **`aec.json` holds the configured defaults** (WirePlumber's `default.configured.audio.*`; an
  absent key is restored as absent, not pinned) and the effective ones the module was pinned to. A
  key is restored only while it still names Kai's node, so a default the owner chose meanwhile
  stays. The file goes only after a PipeWire roundtrip confirms the restore; that wait is
  `aec.restore_timeout_s` (default 4), named in its note, and past it the next start restores.
- **Defaults are set in PipeWire's `default` metadata** (what `wpctl set-default` does), not with
  `pactl`, so pipewire-pulse is not needed.
- **Kai moves no stream.** WirePlumber's `linking.follow-default-target` (on by default) moves the
  streams that follow the default; a stream the owner routed elsewhere keeps its route, so nothing
  per stream needs restoring. Verified live: a stream following the default moved to
  `kai_aec_sink`, and back after the restore.
- **`PR_SET_PDEATHSIG` is SIGKILL**, set by the PipeWire thread that spawns the child (the signal
  follows the spawning thread, and that thread lives as long as Kai). Live: after `kill -9` of Kai
  the child was gone within 5 ms and its nodes within 21 ms.

*Changed during the build, 2026-10-07 (review KAI-1 K-7, K-8, K-9):*
- **Kai's own node is never saved as a real default.** A configured default that still names
  `kai_aec_*` (a leftover whose `aec.json` was unreadable) is saved as unset and named in the note,
  so the stop removes the key instead of writing the dead node back. An unreadable `aec.json` is set
  aside as `aec.json.bad`, never overwritten. Pinning the canceller to Kai's own node is refused.
- **On a switch away the canceller runs until PipeWire confirms the restore.** An unconfirmed round,
  or a failed write, is sent again every `aec.restore_timeout_s` while Kai runs (said once in the
  note) and ends as soon as no key names Kai's nodes, so the defaults never name a node that is
  gone. A loss after a failed flip is restored the same way. At quit the restore is still given up
  after one round, and the next start restores from `aec.json`.
- **A microphone or speaker picked while app AEC runs restarts the canceller** on the new device:
  restore, confirm, start. The tooltip shows it starting meanwhile.

**K21. A fallback is made visible.** The first time in a hold period that a stage is answered by a
fallback (`answered_by` in `lmgw.response.timing`), Kai notifies once, naming the stage and the
alias that answered. This is visibility only: nothing is routed differently (ruling 7).

**K22. Tool design rules** (ruling 20; 2026-10-08):
- one purpose per tool; at most two plain parameters; no nested objects; no parameter that applies
  only in some mode (`window_close {window?}`, not `window_action {action, …}`);
- a parameter may be omitted where that helps: `window` defaults to the active window and takes a
  uuid, an app name or a title fragment, which Kai resolves. An ambiguous match is an error that
  lists the candidates in a few lines. *Decided by the owner 2026-10-09:* `active` names the active
  window explicitly (the shorthand; the default is unchanged);
- terse names and one-line descriptions; a parameter is described only where its name is not
  enough;
- compact results: lines rather than pretty JSON, no null fields, no echo of the input. Never
  truncated (ruling 9): the scope is chosen by which tool is called (`read_selection` vs
  `read_window`);
- the tool list is stable within a thread: built from the probed capabilities (K23) at startup,
  changed only when they change;
- the cost is measured: a check sends Kai's full tool list through lmgw's `/v1/count_tokens` for the
  aliases the Kai thread uses and reports the number. A figure to watch, not a cap.

*Why:*
- the parameter rules remove what makes a model lazy (ruling 20);
- the defaulted `window` saves a `window_list` round trip, whose result would stay in the history;
- llama-server reuses its cached prompt only up to the first differing token, and the tools render
  near the start: a changed list costs a full prefill;
- results stay in the history and are prefilled on every later turn.

**K23. Desktop providers are modules behind capability traits** (2026-10-08):
- the `kai-desktop` crate holds the tool contract (names, schemas, MCP annotations) with no desktop
  dependency. Tool names never name the desktop;
- capabilities are traits: Windows, Capture, Accessibility, Launch, Search, Clipboard, Media, Audio,
  Session, Notify, Input;
- providers:
  - `fdo`, the freedesktop base: MPRIS, notifications, UPower, NetworkManager, logind, systemd app
    scopes (K14), desktop entries, mime and recently-used, PipeWire, AT-SPI, the OpenURI and
    RemoteDesktop portals;
  - `plasma`: the window management protocol, ScreenShot2, KWin scripting through
    `org.kde.kwin.Scripting` (geometry, tiling), Baloo, Klipper, Night Light;
  - `gnome` and `wlroots` later, only on demand;
- per capability the highest-priority provider whose probe passes wins (`XDG_CURRENT_DESKTOP` plus a
  D-Bus name or Wayland global probe); the configuration can override it per capability;
- a tool this desktop cannot support is not listed; the capability table shows in the log and the
  tray. A tool that only lacks a grant (a dev build) stays listed and answers §11.1's explicit
  error;
- K15's policy derives from the MCP annotations (`readOnlyHint`, `destructiveHint`), not a hand
  list;
- Kai's shell has desktop seams too: layer-shell (the ring, overlays) and the SNI tray work on KWin
  and wlroots, not GNOME. A GNOME module would be Kai plus a small GNOME Shell extension. The
  overlay surface and the tray sit behind the same seam.

*Why:* one contract on every desktop, so a thread's tool list does not depend on where Kai runs;
most of the work is freedesktop and portable; the probe keeps the list honest (no listed tool that
cannot work), and the annotations keep the policy from drifting from the tools.

**K24. The desktop tool set** (2026-10-08), about 35 tools. `window?` is optional and defaults to
the active window (K22).
- **Read-only:**
  - `desktop_context`: the active window's app id and title, outputs, desktop and activity, locked
    or idle, DND, what is playing, battery;
  - `window_list`, `see_window {window?}`, `see_screen {output?}`;
  - `read_window {window?}` and `read_selection` (AT-SPI);
  - `find_files {query}`, `recent_files`;
  - `clipboard_get`, `clipboard_history`.
- **State-changing** (K15: confirmed unless a trusted approver approved; the per-tool policy may
  allow one):
  - `open {target}`, `app_launch {app, urls?}`;
  - `window_activate`, `window_close`, `window_minimize`, `window_maximize`, `window_fullscreen`
    `{window?}`; `window_to_desktop {window?, desktop}`, `window_to_output {window?, output}`,
    `window_tile {window?, side}`;
  - `media_play_pause`, `media_next`, `media_previous` `{player?}`; `volume_set {percent,
    device?}`, `mute {on}`;
  - `notify {title, body}`; `ask {question, choices}`, apart from `notify` because it waits for an
    answer where `notify` only shows;
  - `clipboard_set {text}`;
  - `lock_screen`, `dnd {on}`, `night_light {on}`, `brightness_set {percent}`; `suspend`
    (destructive).
- **Confirmed by default** (K15; *changed, 2026-10-09, ruling 28:* was "always confirmed"; the
  per-tool policy can set any of them to `allow`):
  - `run_command {argv, cwd?}`: an argv list, not a shell string; output captured; its own scope
    (K14). *Answered, 2026-10-09:* its own tool, confirmed by default, settable to `allow` (§16 Q12);
  - `ui_press {window?, element}` (AT-SPI, preferred over pixel input);
  - `type_text {text}`, `key_press {keys}`.
- **Not in the desktop module:** timers and reminders (Kai's core: they must speak); KDE Connect
  tools (redundant once the phone hosts its own tools).

*Why:* each is one plain action the model gets right on the first call (ruling 20);
`desktop_context` answers in one call what would take several; the confirmed-by-default tools act
as the owner on anything.

**K25. Delegate tools** (ruling 21; 2026-10-08): domain tools with one plain-language parameter:
`look {question, window?}`, `find {what}`, `arrange_windows {goal}`, `operate_app {app, goal}`.
- Kai runs the delegate's loop, and hands it the user's last utterance verbatim besides the
  parameter.
- Its context: a static part first (instructions, examples, K24's fine-grained tools; cacheable in
  its slot), a dynamic part last (window list, outputs, the screenshot). It never sees the
  conversation.
- It returns a short answer for the main agent: done, partial or failed, and what it did. The
  transcript stays in lmgw's request log.
- A multi-step delegate proposes a plan; the owner confirms the plan once, not each step.
- One-call, small-result jobs stay direct tools: a delegate costs a second prefill and generation.
- Each delegate's alias is a setting: the same row's second slot (no extra VRAM; without kv-unified
  llama-server divides the context across slots, with kv-unified a large delegate context competes
  with the conversation and a full shared pool aborts running slots), or a smaller model. Open: §16
  Q13.
- Its model calls go to lmgw under Kai's key. Delegates are offered through Kai's hosted MCP server
  like any tool. The loop lives in a client-neutral crate, so it can move into lmgw if a second
  client needs it. *Decided by the owner, 2026-10-09:* under Kai's key for every caller (WP17
  (d)).
- *§16 Q13 answered by the owner, 2026-10-09:* all three setups are allowed (one row with more
  than one slot, one slot and no delegate model, a delegate model); the warnings and what lmgw
  must provide are WP17's requirements for the async delegates.

*Why:* screenshots never enter the main context, so the conversation model needs no vision (the
biggest win); the main thread prefills a short answer, not a transcript.

*Decided by the owner 2026-10-09:* who looks at screenshots is a setting: the main agent (as
built, `see_window` and `see_screen` listed to callers) or a delegate (they are not listed to
callers; `look` is the way to see). It lets a main model without vision or with a short context
use the desktop. A delegate may read any window, not only the one its call named (a review
suggested limiting it; the owner rejected limiting). Built in WP17's as-built.

**K26. Sync or async is decided per tool class, when the tool is designed, not per call**
(2026-10-08):
- instant (D-Bus queries, window and media actions): synchronous;
- a few seconds (`look`, `find`, short delegates): synchronous, with the thinking state. This makes
  realtime §19's spoken filler (Later) more pressing;
- open-ended (multi-step delegates, `run_command`, anything waiting on the owner or an app, "tell me
  when …" watchers): asynchronous, returning "started, <id>" at once.
- **Protocol:** MCP's Tasks extension in the 2026-07-28 spec (introduced as experimental in
  2025-11-25; the Python SDK docs call the experimental API removed or deprecated), so one spec
  version is pinned before building.
- **lmgw needs:** Tasks support as the MCP client of hosted servers (useful for every registered
  server); and its realtime route keeps the conversation going while a call is pending, as OpenAI's
  gpt-realtime does (ruling 8; server-tools already renders a pending call as a synthetic "(no
  result yet)").
- **Delivery:** the late result enters the thread like any tool result. Kai picks the moment: never
  while the owner talks or the model speaks. With no session bound, ruling 22's `announce` decides.
- **Control:** `jobs` and `job_cancel {id}` tools, and a tray submenu of running jobs: nothing runs
  unseen, as with overlays (§10.1).

*Why:* a tool's latency follows from what it does, so the class is known in advance; a voice
conversation must not stall on an open-ended call.

**K27. Conversation mode** (ruling 23; 2026-10-08): `conversation_start` and `conversation_end`, no
parameters.
- A temporary override for the rest of the session, not a third mode: Kai listens as in continuous
  listening (K11); when the session ends the configured mode returns. The ring stays up (the ring
  means Kai is active).
- It ends by `conversation_end`, the talk key, the tray, or a silence timeout. The timeout is a
  visible setting, and the tool result names it so the model can say it. Mute does not end it;
  *changed by the owner 2026-10-09 (ruling 32):* muted time counts toward the timeout. The screen
  lock ends it (K32). It takes effect after the current reply.
- By default only Kai's own voice session may call it. Every other caller (the phone, Claude Code,
  agents, the dashboard) is denied outright, not confirmed: a remote caller switching on the room
  microphone is the case a dialog is not enough for. K15's third caller level, "Kai's own session
  only". *Changed, 2026-10-09 (ruling 28):* that level is a default; the owner can widen it.
- A hotword activation ends after one task; conversation mode ends only as above. A short follow-up
  window after a hotword task is a separate decision (§16 Q14).
- It works with push-to-talk before the hotword exists. Continuous listening relies on working echo
  cancellation (§6.2); K16 still holds back while another principal's turn runs.
- **Sequencing:** in a bound session the thread owns the tools (K2), so this tool too comes through
  the hosted MCP link (`/mcp/host`, phase 2). It is the smallest first tool to prove that path end
  to end (§10.1).

*Why:* the model recognizes the intent in any phrasing and language, which no fixed phrase does;
an override for one session adds no mode to the configuration or the tray.

*As built, WP9, 2026-10-09 (taken in this draft; `src/voice/conversation_mode.rs`):*
- The tool call only asks: the mode takes effect at the first quiet point after the current reply
  (nothing open, nothing playing, no held key), and only then the server's turn detection is
  switched on.
- Ends by: `conversation_end`; a **press** of the talk key (it ends the mode and spends the gesture,
  as a press that stops the voice does, K6; a press that stops a playing reply only stops it, but
  ends a request that waits for that very reply); the tray's listening toggle (it ends the mode and
  changes nothing else); the silence timeout; the session's end (listening off, a drop, a rebind,
  quit).
- **`voice.conversation_timeout_s`** (default 300, 0: never by silence). Silence is time with no
  turn, no reply, no speech and no other principal's turn (K16). *Changed by the owner, 2026-10-09
  (ruling 32):* muted time counts as silence; the timeout runs on while muted (it used to pause).
- Timeout end: a notification and a tooltip line name the setting. The tooltip says while the mode
  is on; the tray shows the quiet listening icon, the ring stays up.
- Both tools are `Ask::Trusted` at level `own`: Kai's own session runs them without a dialog.
  `conversation_start` with no open session is an error the model reads.
- *Review follow-up, 2026-10-09:* a drop ends the mode **every** way,
  through one end: any close of the session (a 4004 that Kai binds again included, which used to
  keep the mode across the re-bind), a refused bind, listening off, the dead-link verdict, a connect
  timeout, and a K5 rebind to another thread (a new conversation, the folder's rollover), which used
  to clear it silently right after the tool had said "on from the end of this reply". The mode is
  never carried to the next session: the configured mode is back, the tooltip says "conversation
  mode ended", and the rebind, a 4004 and a revocation (which notify nothing of their own) also send
  the notification the timeout sends. PipeWire being down is silence for the timeout (pinned by a
  test); mute counts as silence too (ruling 32, 2026-10-09; `muted_time_counts_toward_the_timeout`).
- *2026-10-09:* the screen lock ends it (K32), and it does not resume after the unlock; while it is
  active Kai holds accessibility on (K33).

**K28. Routines** (ruling 25; 2026-10-09):
- A routine is a tool of Kai's hosted server, named and described like any tool (K22: a one-line
  description, at most two plain parameters).
- **Kinds:**
  - `steps`: fixed tool calls, no model; fast and deterministic;
  - `goal`: a delegate-loop prompt with an allowed tool list (K25);
  - `script`: any executable.
- **Storage:** one directory per routine, `$XDG_DATA_HOME/kai/routines/<name>/`, with
  `routine.toml` (name, description, parameters, kind, `async`, `announce`, the policy default;
  the steps or the goal) and, for a script, the executable (any language, a shebang).
  - It is editable outside Kai and can be its own git repository.
  - Kai watches the directory; a change sends MCP `notifications/tools/list_changed`.
- **Scripts reach Kai through the `kai` binary as a CLI:** `kai tool <name> --<param> <value>`,
  and `kai model <alias>` (the prompt on stdin, the answer on stdout) through Kai's lmgw key.
  - The CLI talks to Kai over `$XDG_RUNTIME_DIR/kai/cli.sock` (mode 0600, peer credentials
    checked, like the browser socket of §11.1). No secret ever enters the script's environment.
  - Tool calls pass K15 with the routine as the caller: the peer's cgroup names the routine's
    scope. A call from outside any routine's scope (the owner's terminal) is the caller `cli`,
    confirmed like any other untrusted caller unless in `policy.trusted_callers`.
- **A script runs** in its own app scope (K14) with its parameters as arguments. Stdout is the
  result returned to the model (compact, K22), stderr goes to the log, the exit code decides done
  or failed. `async` puts it in K26's open-ended class; ruling 22's `announce` applies.
- **Creating one:**
  - a delegate tool `routine_create {goal}` (K25), on an alias the configuration names (a capable
    coding model), writes a draft routine, disabled;
  - the owner reviews it (a script is shown in full) and enables it. Enabling a script is confirmed
    by default; ruling 28 lets the owner allow it;
  - by hand, or by any agent writing the directory, outside Kai.
- **Triggers:** as a tool (voice, chat, phone, any principal api §5.6 allows), a key binding, the
  tray. Schedules are open (§16 Q16).

*Why:* scripts are the most flexible form and the one capable models and agents produce best; the
socket keeps the tool contract and K15 the single path to the desktop; the directory format lets
routines live outside Kai.

*Note, 2026-10-09:* the owner means bash scripts: `kai tool` and `kai model` are commands for a
shell script, and `routine_create` writes bash by default. Any executable still runs.

*Decided by the owner 2026-10-09:* no pinning; a switched-on routine always runs the latest
version of its folder. *Consequence, stated plainly:* a switched-on routine runs whatever its
folder holds now, so it is exactly as trustworthy as everything that can write its folder (any
program of the user, another routine's script, `run_command`, an agent). `routine:<name>`
identifies which routine called, not its content.

*Taken in this draft, 2026-10-09:* because the folder can
change after the switch-on, `routine.toml` is no source of permissions beyond Kai's default for a
routine. It can declare defaults up to confirm and the owner's level (`policy = "always"`, `level =
"own"` or `"owner"` are honoured); anything wider (unasked runs for trusted callers through `policy =
"trusted"` or `read_only = true`, a level `trusted` or `any`, and an absent `level`, which means
`any`) is only asked, and honoured only from the owner's `policy.tools.<name>` (`allow_trusted`,
`allow`) and `policy.levels.<name>`, which K30's guard covers. Not to patronize: the switch-on
dialog (the owner's) says what the routine asks and has one box per grant, each unticked ("Also
grant: policy.tools.<name> = ..."), and writes the ticked ones as the owner together with the
switch (undone if the switch is not written). The owner's own tray or key run of a routine that
asked to run unasked is not confirmed again (the owner's act); one that changes state still is.

**K29. The configuration window** (ruling 26; 2026-10-09):
- A normal GTK toplevel (not layer-shell) with a WebKitGTK view on the `kai://` scheme (K10), with
  its own ephemeral `WebContext` (K1). Opened from the tray, created on demand, destroyed on close:
  K1's RAM rule. While open it costs about what an unrelated view does (66 to 72 MB, K1).
- The frontend is Leptos (CSR, built with trunk like lmgw-ui), embedded in the binary. Its CSP is
  K10's, widened only by what the WASM build needs.
- It reaches Kai's core only through the scheme handler: Kai still opens no network listener.
- Every setting is an operation in Kai's core. The window and the owner tools (K30) are two clients
  of the same operations, as lmgw's dashboard and its self-admin tools are. Writes go through §2's
  configuration write.
- **Shared crates:** lmgw extracts its reusable UI parts (theme tokens and CSS, widgets) from
  lmgw-ui into a shared crate that Kai consumes through `LMGW_PIN`, as with `lmgw-client` (K3).
  Until it exists Kai uses lmgw-ui's CSS tokens at the pin. The extraction is lmgw-side work (Later;
  §16 Q18).
- It follows lmgw's native-feel rules: the KWin frame, desktop fonts, the KDE header palette, the
  interface scale.
- A System Settings entry (KCM) may come later as a thin launcher.

*Why:* no second UI stack; extensions share construction.

**K30. Owner configuration tools** (ruling 27; 2026-10-09):
- Kai's configuration operations (K29) are offered as tools, owner-only: the owner and Kai's own
  key (api §5.6); other callers only if the owner grants them (K15's caller level, ruling 28).
- The setting `config_tools` ("Kai configuration via tools") hides them when off. Default off: the
  draft's choice (§16 Q17). Switching it changes the tool list (K22: `list_changed`).
- Per-tool state in the policy (K15): on (`allow`), ask (`confirm`), deny, off (not listed).
- *Decided by the owner 2026-10-09:* the config tools cannot widen a permission by default (a tool
  state toward `allow`, a wider level, an added trusted caller or approver, `config_tools` itself
  switched on); the owner permits it per tool with `policy.widen_by_tools.<tool>` (trust:
  `policy.widen_trust_by_tools`), which only the settings window or the file can set. Narrowing
  always passes. The risk is judged by the user, tool by tool, not by Kai (WP21a's as-built).
  *Taken in this draft, 2026-10-09 (review fixes):* an unset entry counts as the default Kai
  really applies (a routine's from its loaded spec, K28's capped defaults); a name Kai offers no
  tool for is judged conservatively (any entry but `deny` or `off`, and taking an entry away, is a
  widening); a key whose value Kai runs as a command, `launch.terminal`, is `run_command`'s power:
  the tools change it only with `policy.widen_by_tools.run_command`. *Third review (2026-10-09):*
  a permission-like boolean counts the same when it goes on, or is unset while off with its default
  on: `overlay.fetch_private_addresses` needs `policy.widen_trust_by_tools`
  (`widen::PERMISSION_FLAGS`). Considered and left out as not permissions: `overlay.<kind>.interactive`
  and `auto_show`, `voice.on_at_start`, `shortcuts.bind`, `update.background_check`,
  `overlay.ring_over_fullscreen` (all visible, narrow, and the owner's own surface).
- This is Kai's side, distinct from lmgw's key scopes: lmgw decides who may reach Kai's label, Kai
  decides what it offers and how it confirms.

*Why:* the owner can configure Kai by voice or from any client; one set of operations keeps the
window and the tools from drifting apart.

**K31. Personality profiles: the proposed content** (ruling 29; 2026-10-09; draft). The feature is lmgw's; this is what a profile should hold for voice:
- a persona;
- a concrete length rule instead of "short": one to three sentences, details on request, no
  closing recap;
- three to five short example exchanges. They are static, so the prompt cache holds them; their
  token cost is measured like K22's tool list;
- the speech style and the voice for TTS (already per thread in lmgw);
- reasoning on or off: thinking is the costliest voice delay;
- its own voice block, replacing the generic one.

The editor is one Leptos component in the shared UI crate (ruling 26, K29), used by lmgw's
dashboard and Kai's window, with lmgw's Test button plus "speak a sample". `personality_set` is a
state-changing tool under K15/K24 defaults like any other.

*Why:* the length and the form of a spoken reply are decided by the prompt, and only lmgw's thread
owns it.

**K32. The screen lock** (ruling 31; as built 2026-10-09, `src/screen_lock.rs`; where the ruling
was silent, the builder's decisions, marked *draft*):
- **The signal.** Plasma 6.7's locker (KScreenLocker, inside KWin) emits the screen saver service's
  `ActiveChanged` (`org.freedesktop.ScreenSaver`, on `/ScreenSaver` and
  `/org/freedesktop/ScreenSaver`) at the lock and the unlock, and sets logind's `LockedHint` on the
  session (`SetLockedHint`; `libKScreenLocker` 6.7.5 carries the call, and `LockedHint` is
  `emits-change`). Kai follows both facts: `LockedHint` on Kai's own logind session (the system
  bus; `XDG_SESSION_ID`'s session, else `GetSessionByPID` of Kai's PID; *changed 2026-10-09 after
  the review:* not `user/self`'s `Display`, which can be another of the user's sessions) through
  `PropertiesChanged`, and `ActiveChanged` with `GetActive` at start. *Draft:* the session counts
  as locked while **either** says so (the safe side), unlocked once both say so. logind's `Lock`
  and `Unlock` signals are **not** used as the state: they are requests to the locker
  (`loginctl lock-session` sends them whether or not anything locks), which KScreenLocker answers
  by locking, and both facts then follow; a Kai that took `Lock` for the state would stay locked
  forever where no locker answers. A source that cannot be followed is logged; with neither, Kai
  sees no lock (logged). Kai's own `session_lock` tool (WP16b) is unaffected: it asks the locker.
- **Following the services** (*added 2026-10-09 after the review*: a match rule pinned to the
  start's owners missed the unlock after a KWin or logind restart, and Kai refused everything
  until its own restart). Each service is followed by name through the bus's `NameOwnerChanged`:
  one that leaves the bus takes its fact with it (it says nothing until it is back), a new owner
  is followed and asked again (`GetActive`, `LockedHint`), and a screen saver service that appears
  after Kai's start is picked up. When the two facts disagree after one of them changed, the other
  is read again before the lock is published, so a lost signal holds neither the lock nor the
  unlock. The watch is the shared flag's only writer; the core reads the flag at each
  `Command::ScreenLock` and acts only on a change of it (a burst of locks and unlocks cannot leave
  the host's flag cleared while the screen is locked).
- **At start** the lock counts as locked until the watch's first look at both sources (bounded by
  `dbus.timeout_s` after the system bus answered; the host refuses with the lock's words until
  then), and that first look is published whatever it found: a source that cannot be followed
  counts as unlocked, logged. *Draft:* reading the state before the host starts would hold Kai's
  start on two buses; not known counts as locked instead, which nothing has to wait for.
- **What the lock does** (`Command::ScreenLock`): conversation mode ends; a held talk key's hold
  ends with its audio discarded (no commit, no `response.create`, as a stuck key's, K6); listening
  goes off and the session ends as at quit (a playing reply stops with its truncate, a response on
  its way goes with the session); the capture closes. The talk key, the tray's listening, a new
  conversation and `conversation_start` do nothing while locked (the tool says the lock's words).
  The tray's status reads "screen locked: Kai is not active" with the idle icon, and a tooltip line
  says Kai stays idle after the unlock.
- **Tool calls.** The host's one path (`host::server::Shared::dispatch`, lmgw's calls, the
  routines' CLI, the tray and keys) refuses every call while locked with "the screen is locked;
  Kai is not active". *Draft:* a call on its way at the lock is stopped too: preparing, waiting for
  the owner's confirmation (its dialog closes) or running (its future dropped and its token
  cancelled, as lmgw's cancel does), answered "the screen is locked; Kai is not active; the screen
  locked while it ran, so <tool> was stopped". A typing call cannot reach the lock screen's
  password field. *Decided by the owner, 2026-10-09 (review):* Kai is not active on a locked
  screen, so this includes the routines' timers: a scripted `kai tool` run (a routine's script, a
  systemd timer, cron) while the screen is locked fails with "the screen is locked; Kai is not
  active" (exit code 1, the words on stderr), and nothing queues it for after the unlock.
- **The approval card** hides at the lock (no answer is taken while locked: a spoken answer being
  recorded is dropped, one being transcribed is ignored when it comes back); the calls stay
  waiting, other clients can answer them, and the card shows again (and arms afresh) after the
  unlock for what still waits. The tooltip says where the call waits and that the card comes back.
- **Overlays.** *Draft:* the widgets' layer-shell surfaces are unmapped while locked and mapped
  again at the same place after the unlock (`WidgetsCmd::Locked`), rather than hidden for good: a
  timer or a note the owner had up survives a lock. A show on its way fails. The ring goes because
  the session does. KWin draws only the greeter (and its own lock-screen overlays) while locked,
  so layer-shell surfaces should not show there anyway; Kai does not rely on that. Kai's normal
  windows (the settings window, the pairing dialogs) are left to KWin, which does not show normal
  windows on the lock screen. *Decided by the owner, 2026-10-09 (review):* a widget unmapped while
  locked also pauses its refresh timers and fetches (the weather's, any other) until the unlock,
  then refreshes once. As built no widget of Kai's has a timer or fetch of its own: `overlay_weather`
  and `overlay_image` fetch once, at their show, and a show while locked fails, so there is nothing
  to pause yet; a widget that gets one follows this rule. *Built 2026-10-09:* an `overlay_app`
  view's own script still runs on in its unmapped page (WebKit is not suspended), but the app
  bridge (`src/widgets/app_bridge.rs`, `Bridge::with_screen`, the shared `ScreenLock`) answers
  every request that would reach lmgw (`tools/call`, `resources/read`, `resources/list`,
  `resources/templates/list`) with a JSON-RPC error (-32000) carrying the host's words, "the screen is locked;
  Kai is not active", and forwards nothing, while the flag says locked (not known at start counts
  as locked). `ping`, `ui/initialize` and the like are the bridge's own and stay. *Draft:* the
  unasked deliveries of a call's view (`tool-input`, `tool-result` or `tool-cancelled`) wait for
  the unlock and are sent then if the view still exists (a call that ended while locked shows its
  result after the unlock; the view being closed drops it); a view's request already in flight at
  the lock is not cut (its answer is lmgw's), the next one is refused.
- **After the unlock** Kai is idle: listening stays off (also when it was on before the lock,
  *draft*: ruling 31 says idle), conversation mode is gone, nothing reconnects by itself; the next
  press or listening on starts as usual.
- Accessibility goes back at the lock (K33).
- *Tests:* `src/screen_lock/tests.rs` (both sources over private buses, either locks, both unlock,
  a start on a locked screen; Kai's own session by `XDG_SESSION_ID` or its PID, never the
  `Display` session; a screen saver service and logind that restart with the unlock's signal lost,
  a screen saver service that appears after the start, a missed signal read again when the two
  disagree, not known counting as locked, the first look published with no source);
  `src/voice/tests/screen_lock.rs` (speech, a press, conversation mode, the card, the talk key
  while locked, idle after, a lock/unlock/lock burst that leaves the flag to its watch);
  `host::tests::calls` (refused, and stopped while asked and while running). *Live:* `just live-check-approvals` step 5b (a stand-in
  screen saver service on the nested session's private bus, Kai's own watch: the card goes and
  comes back, an approval given in lmgw while locked reaches Kai's tool, which refuses); `just
  live-check-widgets` section `lock` (KWin lists none of Kai's surfaces while locked, both back at
  their place after). *Unverified:* the owner's real lock screen (Meta+L, the idle lock, a lock at
  suspend): that `ActiveChanged` and `LockedHint` come as read here, and that nothing of Kai's
  shows on the greeter.

**K33. The accessibility switch** (ruling 33; as built 2026-10-09,
`src/desktop/fdo/atspi/switch.rs`; *draft* marks the builder's decisions):
- **Settings** (`[accessibility]`, beside `concurrency`): `switch = "conversation"` (the owner's
  rule: on while conversation mode is active, around each read otherwise), `"read"` (around each
  read only) or `"never"` (Kai does not switch it); `wait_ms = 5000`, how long a read waits after
  Kai switched it on for the window's app to join the bus (long enough for the apps seen in testing
  to join). Both are read at start, as the other keys (`Effect::Restart`). *Draft:* the owner's
  configuration tools count switching it wider (`never` to `read` to `conversation`) as widening a
  permission (`policy.widen_trust_by_tools`), as `overlay.fetch_private_addresses` (K30).
- **The rule.** Kai wants `IsEnabled` on while the setting allows it and conversation mode is on or
  a read holds it, never while the screen is locked or after Kai's quit began. Each stretch of
  wanting is one period: at its start Kai reads the switch; off, Kai switches it on and owns that
  change; on already, it is not Kai's and stays on after. At the period's end Kai switches off only
  what it owns, and only if it still reads on. Kai follows `PropertiesChanged` of
  `org.a11y.Status`: the switch going off while Kai owns it ends Kai's ownership for the period
  (Kai does not switch it on again then, nor off at its end), so an owner who switches it off and
  on again in between keeps it on. Kai's only write is `IsEnabled` through
  `org.freedesktop.DBus.Properties.Set` on `org.a11y.Bus`; `ScreenReaderEnabled` is never written
  (tests assert it). *Added 2026-10-09 after the review:* the launcher's signal of a write can come
  late (it announces its GSettings key's change, after the write), so the "off" of one read's
  restore could arrive after the next read switched it on and end that read's ownership, leaving
  the switch on for good. Each of Kai's writes absorbs the first signal of its value within
  `dbus.timeout_s`; only a signal none of Kai's writes accounts for is someone else's change.
- **Cancellation** (*added 2026-10-09 after the review*). A read's call can be dropped while the
  switch settles (lmgw's cancel, the lock stopping the call, a deadline). Every settle runs in a
  task of its own that a dropped waiter does not stop, so a switch-on that went out is always
  owned (and marked) and a restore always finishes; a read's hold exists before the read counts,
  so a dropped read still ends (before, it could stay counted and hold the switch on until Kai
  quit).
- **The reads:** `read_window`, `read_selection`, and `ui_press` (*draft:* held from its find
  through the owner's confirmation to its press, so the app stays on the bus for the press). Reads
  share the switch: the last one's end puts it back. *Decided by the owner, 2026-10-09 (review):*
  the switch stays on while Kai's dialog for a `ui_press` is open, also for a local caller with no
  deadline of its own; the dialog is the bound, and the owner controls it (answering or closing it
  ends the press and puts the switch back). No further deadline.
- **The cost of "only at the moment of read"** (*the owner's rule, kept 2026-10-09 after the
  review*): outside conversation mode every read switches it on and back off. Each time, every
  running app with an accessibility bridge joins the bus and leaves it again, the launcher writes
  its GSettings key twice (dconf, on disk), and the read waits for its app to join (9 to 12 ms for
  a Qt dialog in the nested check, at most `accessibility.wait_ms`); back-to-back reads (a
  `read_window`, then a `ui_press`) pay it each. Conversation mode avoids it (the switch stays on
  through it), and so does a switch the owner left on (not Kai's to toggle).
- **Waiting for the apps.** Qt's bridge joins the bus when it sees the switch go on, and
  at-spi2-registryd announces each app that joins with `ChildrenChanged` on its root. A read whose
  window's app is not on the bus right after Kai switched it on follows those announcements and
  goes on at its app's, at most `accessibility.wait_ms` from the switch-on (a registry whose
  announcements cannot be followed: one more look at the bound's end, logged). Measured in the
  nested check: a Qt dialog off the bus with the switch off was read 9 to 12 ms after the switch
  went on.
- **Restore** at conversation mode's end, at a read's end, at the lock, and at Kai's quit (the
  core's quit sequence, which SIGTERM and SIGINT reach; `main` restores once more after the core's
  task ended, which covers a core that panicked). A crash or SIGKILL cannot restore, and the switch
  is persistent (the launcher keeps it in GSettings `toolkit-accessibility`): while Kai owns a
  switch it keeps a marker (`$XDG_STATE_HOME/kai/a11y-switched-on`), and the next start switches a
  switch that still reads on off. *Draft:* what happened between the crash and that start cannot
  be told apart from Kai's change (an owner who switched it on meanwhile finds it off again).
- **The read's error** names the switch's state and Kai's part: with it off and `never`, "which
  Kai does not switch (accessibility.switch = "never")"; with it off otherwise, why Kai did not
  switch it (it failed, or the owner switched it off meanwhile); with it on after Kai's wait, "Kai
  switched it on and the app did not join within N ms (accessibility.wait_ms)".
- *Tests:* `src/desktop/tests/read/switch.rs` against a fake `org.a11y.Status` with writable
  properties (conversation on and off, around a read with restore and shared reads, a switch on
  already left alone, the owner's change not clobbered, `never`, the lock and the exit, the crash
  marker, the read waiting for its app's announcement, an app that never joins; a read dropped
  while the switch is read and while it is written, a restore and a quit dropped half way, a late
  signal of Kai's own "off" after the next read's switch-on, with an owner's late "off" still
  winning). *Live:* `just
  live-check-read` (a Qt dialog that follows the switch, on the private launcher whose switches
  live in memory: off the bus with `never`, read with `read` and put back, twice, only
  `IsEnabled` written). *Unverified:* a real session (typical Qt/KDE and LibreOffice apps
  joining at the switch-on, the registry's announcements there, plasmashell's cost).

## What exists today (relied on, not re-derived)

**lmgw** (main 2baa764)
- **The binding.** `GET /v1/realtime?chat_thread=<id>` binds a session to a Chat thread before
  the 101 (chat-voice §8.1).
  - The thread owns the model, prompt, tools, ASR, TTS, voice, languages and the conversation.
  - The client owns turn detection (`null` = push-to-talk), `create_response`, `half_duplex`,
    `echo_tail_ms`, the barge-in knobs, the output modality and the truncate, cancel, create and
    buffer events.
  - The bind needs `Cap::Admin` today (`crates/lmgw-core/src/realtime/thread/bind.rs:86-96`); api
    §1 opens it to device keys.
- **One bound session per thread.** A second bind takes over: close 4000 and
  `chat_thread_taken_over` (`realtime/thread.rs:67-73`, `realtime/thread/hooks.rs:57-66`,
  `web/chat_live/voice.rs:53-76`).
- **Bound-session events.** The events table at the end of chat-voice §8.6 lists
  `lmgw.chat.frame`, `lmgw.chat.user`, `lmgw.chat.reply`, `lmgw.chat.thread`, `lmgw.model.state`
  and `lmgw.response.timing`. The last carries the stage times, `cold`, and per stage `alias` and
  `answered_by`.
- **`session.created` comes before the connect warm**, so audio sent after it waits on the server
  while models load.
- **The page's rules** (chat-voice §9.2, §9.3), which Kai implements the same way:
  - the voice state is derived from facts, not stepped;
  - a stop sends the truncate first, with what was heard, and `response.cancel` only when the
    truncate did not end the response;
  - a push-to-talk press over the voice stops it, then listens.
- **Echo modes** `device | browser | not_needed | none` (chat-voice §12.1). In `device` mode,
  playback must go to the system default output (§12.2).
- **The voice settings Kai inherits** come through its folder's defaults and Settings → Chat → Voice
  (chat-voice §2.1, §2.2): ASR and TTS alias, voice, speech style, the spoken and the reply
  language, turn detection, read-aloud, and `chat_voice_audio_input`.
  `config/settings.rs:302-311` has `off | local` today. The fix/voice-audio-fallback branch is
  changing it to `off | on`, decided by capability only: with `on`, a voice turn goes as audio to
  whichever model answers it, fallback included, when that model takes audio input, and as its
  transcript otherwise. Kai sets none of these itself.
- **lmgw has no server-side default chat model.** A thread's model comes from the request or the
  folder's defaults (`web/chat.rs:103-129`, `web/chat_folders.rs:281-301`). Folder defaults are a
  copy at creation (`store/chat_folders.rs:8-11`), and folder names are not unique.
- **The Chat sweep** archives a thread after 14 idle days and deletes it 30 days later. Pinned
  threads are exempt (`config/settings.rs:238-250`).
- **A text send cancels the thread's live turn**, whichever window or device sent it
  (`web/chat_live.rs:17-27`, `:189-215`).
- **The visualisation contract** (chat-voice §10) and `crates/lmgw-ui/assets/voice/viz/engine.js`
  and `ring.js`.
  - The ring paints an opaque background (`ring.js:28`, `:72`).
  - The engine renders calm (15 fps) only when idle with both taps quiet (`engine.js:426-430`).
- **The GPU hold** answers 503 with `gpu_hold` (`error.rs:427`). A fallback that answered is named
  by `x-lmgw-fallback` and `answered_by`.
- **Tool-result images** go to the model only where the route can see them; elsewhere they become a
  placeholder with a reason (egress §8.2, `gate/tool_images.rs`).
- **CORS** mirrors request headers since 2baa764 (`server.rs:120-135`). The listener speaks
  `http://` only (`principal.rs:403-406`).
- *Note, 2026-10-09 (at the pin f57aa71e):* the bind checks `Chat` since api WP3, not `Admin`. Of
  what phases 2 and 3 need, lmgw has only the stored hosting grant: no `/mcp/host`, no approvals in
  the Chat, no resources on `/mcp`, no MCP Tasks. §15.1.1 lists it.

**An earlier private prototype.** Its DESIGN.md was read for this draft.
- **`src/aec.rs`** (384 lines):
  - `libpipewire-module-echo-cancel` hosted in a `pipewire -c <conf>` child, spawned plainly
    (`:85-92`) and pinned to the real default devices;
  - defaults flipped to `aec_sink`/`aec_source`, every open stream moved;
  - a 4 s wait for the nodes (`:102`);
  - an existing pair reused (`:55-80`). On the reuse path the defaults to restore are `None` when
    the current default is already `aec_*` (`:66-75`), and `Drop` restores only `Some` values
    (`:132-145`). K20 fixes both.
- **`src/tray.rs`** (241 lines): ksni 0.3.5 on async-io, phase icons, middle-click mutes. Taken as
  the pattern; Kai runs ksni on tokio.
- **`src/state.rs`:** pause is mic-mute, a state of the running process (DESIGN.md §12, Stage 6.2).
  Taken.
- **`src/krunner.rs`** (166 lines): an `org.kde.krunner1` runner for `kai <question>`. Later.
- **Not taken:** local ASR, VAD and TTS; the wake word; the SQLCipher memory store and curator; the
  D-Bus cockpit; the systemd unit.

**The overlay spike** (`spikes/overlay/RESULTS.md`; host `spikes/overlay/src/main.rs`):
- **It works** on GTK 3.24.52, gtk-layer-shell 0.10.0 (crate 0.8.2), WebKitGTK 2.54.0 (4.1 API), KWin 6
  and an NVIDIA GPU. The NVIDIA rule is set at the top of `main()` (`main.rs:90-96`). Rendering is
  DMABuf hardware.
- **Click-through** needs a 1×1 px input region set in `map-event` (`main.rs:167-185`).
- **ring.js** runs byte-identical, with a 45-line adapter, about 20 lines of boot glue and a 9-line
  transparency shim.
- **RAM (PSS):**

  | Setup | Total |
  |---|---|
  | one view | 189 MB |
  | each further unrelated view | +66 to 72 MB |
  | each further `related_view` | +15 MB |
  | each further widget in one view | +6 MB |

- **Energy:** speaking ~14 % of a core and a small rise in GPU power; idle at 15 fps ~3.4 % and a
  GPU cost within the baseline's spread.
- **Untested at the spike (WP0 has since verified most):** a real click, fullscreen windows,
  multi-monitor, fractional scale and HDR content, the input region after a resize, `file://` against
  the custom scheme, shortcut release.
- **History:** `gtk-layer-shell-devel` is now packaged; the spike's pkg-config shim is obsolete.

## 1. Overview

```
                 ┌──────────────────────── Kai (one process) ────────────────────────┐
 talk key ──▶ portal / KGlobalAccel ─┐                                              │
 tray    ◀──▶ ksni ──────────────────┤ tokio core (lmgw-client rules)               │
                                     ├─ session task ── WS  /v1/realtime?chat_thread ┼─▶ lmgw
 mic     ──▶ PipeWire ─▶ rings ─────▶├─ feed task ───── SSE /chat/api/feed           ┼─▶ lmgw
 speaker ◀── PipeWire ◀─ rings ◀─────┤─ host task ───── WS  /mcp/host (phase 2)      ┼─▶ lmgw
                                     └─ GTK main thread: layer-shell windows, WebKit │
                 └───────────────────────────────────────────────────────────────────┘
```

| Phase | Scope | lmgw needs (api §) |
|---|---|---|
| 1 | voice, tray, shortcuts, ring, the Kai conversation | A (device keys, binding), B for Desktop 1, C, D's crate and the types it uses; H optional |
| 2 | tool-controlled overlays, the MCP Apps host | E with A's hosting grant (K2), G |
| 3 | desktop tools, approvals, vision; app memory and browser parked in `docs/ideas.md` (§16 Q20, *2026-10-09*) | E, F; MCP Tasks (§15.1.2 M4, *2026-10-09*) |

## 2. Process and threads

- **One binary, `kai`.** Three kinds of threads:
  - **The GTK main thread** runs the glib loop: layer-shell windows and WebKit views, nothing else.
  - **A tokio runtime:**
    - the lmgw connections;
    - the voice session;
    - ksni (tokio backend);
    - ashpd and zbus for the shortcuts;
    - Secret Service.
  - **The PipeWire loop thread** (pipewire-rs). Its callbacks only move samples through lock-free
    rings: no allocation, no locks, no I/O. *Changed during the build, 2026-10-06:* the streams run
    with `RT_PROCESS`, so their process callbacks run in PipeWire's realtime data thread; the loop
    thread runs everything else (registry, default metadata, stream setup, the AEC child). The
    other callbacks only queue an event that the loop handles between iterations.
- **Bridges.** tokio reaches GTK through an async channel drained by
  `glib::MainContext::spawn_local`. GTK reaches tokio through a tokio mpsc.
- **The NVIDIA rule is the first statement of `main()`,** before GTK and before any thread:
  - it sets `__NV_DISABLE_EXPLICIT_SYNC=1` when that is unset and `/dev/nvidiactl` or
    `/sys/module/nvidia` exists;
  - it never sets `WEBKIT_DISABLE_DMABUF_RENDERER`, `WEBKIT_DMABUF_RENDERER_FORCE_SHM` or
    `GDK_BACKEND=x11`.
- **Configuration** lives in `$XDG_CONFIG_HOME/kai/config.toml`: the lmgw base URL, the folder id, the
  modes, the devices, the ring placement and every threshold in this document. Each limit is named
  in the error it causes. *2026-10-09:* the configuration window (K29) and the owner tools (K30)
  edit it; routines live apart, under `$XDG_DATA_HOME/kai/routines/` (K28).
- **The secret.** The device key is a Secret Service item (served by `ksecretd`, not kwalletd; store,
  read and delete took 13 to 52 ms with no dialog, WP0 (h)) with attributes
  `{application: dev.rootshell.kai, lmgw: <base url>}`. A locked wallet at login shows "wallet
  locked: unlock KWallet", not "not paired", and Kai retries when the wallet opens.
  *Changed during the build, 2026-10-06:* Kai reads the default collection's `Locked` first. On a
  locked wallet it neither searches, writes nor unlocks, because the Secret Service client would
  raise an unlock prompt. "Retries when the wallet opens" is a check every `wallet.retry_s`
  (default 5), since KWallet's change signals are unverified. Each call is bounded by
  `wallet.timeout_s` (default 10). Both are named in the tooltip or the error.
- **Single instance:** the D-Bus name `dev.rootshell.kai` (K13).
- **Logs** go to stderr through tracing, into the autostart unit's journal. Argv is never logged.
  *Changed during the build, 2026-10-07 (review KAI-2 K2-1, K2-10, the owner's decision):* the
  device key and audio never reach a log at any level. The HTTP and WebSocket stack (tungstenite,
  tokio-tungstenite, hyper, h2, reqwest, rustls) is capped at `info` and D-Bus (zbus, ksni, the
  Secret Service crate) at `warn` whatever `KAI_LOG` says: a second filter every record passes, so
  no directive lifts it (tungstenite traced the handshake's `Authorization` header). The bearer is a
  sensitive header. An append is logged as its kind and sample count, never its audio; a feed event
  Kai cannot read as its name only, at `debug` (its data may be the owner's messages). A test runs
  the realtime adapter at `trace` and finds neither; the live checks assert their logs hold no key
  and no audio, the link check at `trace`.
  *Changed during the build, 2026-10-07 (review KAI-3 K3-8):* a realtime event of a type
  `lmgw-client` reads, whose fields did not read (so it came as `ServerEvent::Unknown`), is logged
  at `warn` by its name and size; every other event Kai does not act on (the OpenAI-shaped events
  it skips, a newer gateway's) stays at `trace`.
- **Quit is a clean exit:**
  1. the playing reply gets its truncate (what was heard);
  2. the sessions close;
  3. app AEC restores the default devices;
  4. the overlays go.

  *WP4, 2026-10-07:* step 2 waits for lmgw's answer to the close at most `voice.close_wait_s`
  (default 3, named in the log line when it passes); lmgw answers only after its journal drained.

  *Changed during the build, 2026-10-07 (review KAI-1 K-24):* the tray item's removal and the D-Bus
  name's release each wait at most `dbus.timeout_s`, logged when it passes, and notifications still
  on their way (the voice's last ones included) are sent before Kai exits, each bounded by it.

  *Changed during the build, 2026-10-07 (review KAI-2 K2-24):* the runtime shuts down with
  `dbus.timeout_s` too, so blocking work still running (a DNS lookup) cannot hold the exit.

  *Changed during the build, 2026-10-07 (review KAI-2 K2-9, the owner's decision):* a write of the
  configuration changes only its value: the comment lines above a key, its spacing and a comment
  after it stay, and a removed key's comments move to the next line. It goes through a symlink to
  its target with the file's permissions, and it is atomic and durable (a temp file in the same
  directory, synced, renamed, the directory synced).

## 3. The connection to lmgw

### 3.1 Pairing

1. On the dashboard's Devices card (api §1.4), the owner names the device, sets its scopes and
   budget, and grants it a hosting label (`kai` suggested). The card shows the
   `lmgw-pair:?v=1&url=…&name=…&key=…` link once.
2. Kai gets the link one of two ways:
   - Clicked: the scheme handler, by K13's path.
   - Pasted into the dialog behind the tray's "Pair with lmgw…".
3. Kai stores the key (§2) and the URL, then opens the feed. `hello` names the device, which the
   tooltip shows, and gives the hosting label Kai uses (§10.1).
   *Changed during the build, 2026-10-07 (review KAI-1 K-1, K-3):* only after the owner confirmed
   the link (K13). A re-pair first ends the voice session bound on the gateway it replaces, as at
   quit (a press session waiting out its grace too), and forgets that gateway's thread and its other
   clients' sessions and turns. The old gateway's wallet item stays.
   *Changed during the build, 2026-10-07 (the owner's decision):* a confirmed re-pair to another
   URL deletes Kai's own wallet item for the old one (attributes `application` and `lmgw` both
   match; nothing else), once the new key and `lmgw.url` are stored; the dialogs' line on the
   pairing it replaces says so. A re-pair to the same URL replaces its item. A deletion that fails
   (the wallet locked meanwhile, an error) leaves the item, logged and notified with what to delete
   in KWallet. The feed then opens, and `hello`'s device name is the tooltip's device line.
   *Changed during the build, 2026-10-07 (review KAI-2 K2-16):* "both match; nothing else" was not
   true: the Secret Service's search matches every item with at least the given attributes, in
   every collection. Kai's item now carries `kind: device-key` too, and load and delete look only in
   the default collection and take only an item that is exactly Kai's key (an earlier build's item
   with only `application` and `lmgw` included, which the next store of its URL removes).
4. A link with `fp=` is verified against the server's certificate (none today: lmgw serves no TLS).
   *Changed during the build, 2026-10-06:* until the lmgw link brings a TLS client that can read the
   certificate, every link that carries `fp` is refused with the reason (fail closed). `fp` with an
   `http://` url is always refused.

An unpaired Kai shows the grey icon with "not paired" and offers only "Pair with lmgw…" and Quit.

### 3.2 The feed

Kai keeps one `GET /chat/api/feed` open (api §2). It resumes with `Last-Event-ID`, an
`"<epoch>:<seq>"` cursor.

**Reachability is the feed.** While it is connected, lmgw is reachable.
- **Reconnects** back off from 1 s, doubling, to `lmgw.retry_max_s` (default 30), shown in the
  tooltip.
- **lmgw down at login is normal**, because lmgw may be started by hand, later. It shows the grey
  icon and raises no notification.
- **A talk press while the feed is down** says "lmgw is not running" at once, as a notification and
  a tooltip line. Kai buffers nothing.
- **The keep-alive interval** comes from `hello.keepalive_s`. Kai declares the link dead after two
  intervals without a byte.

| Event | Kai |
|---|---|
| `hello`, `state` | hold badge, voice and turn state of its thread, hosting label |
| `hold` | the amber badge |
| `folder.current` (its folder) | rebinds at the next quiet point (K5) |
| `folder.updated` (its folder) | retries `current` if it was waiting for a model (§4) |
| `folder.deleted` (its folder) | looks the folder up again (§4) |
| `thread.updated` (current thread) | title; if it was archived, asks `current` again |
| `thread.deleted` (current thread) | asks `current` again |
| `voice.bound`, `voice.ended` by another client | tooltip ("voice in use on device 'phone'"); K5's no-rebind rule |
| `turn.started`, `turn.done` by another client | K16 |
| `resync` | asks `current` again |
| `revoked` | §3.3 |
| `approval.requested`, `approval.decided` (phase 3) | approval overlay opens, or closes (§11.1) |

*Built in WP2, 2026-10-07* (`src/lmgw_link.rs`, `src/lmgw_link/feed.rs`):
- **The client.** reqwest (rustls with the system's trust store, for a gateway behind https; no
  proxy), the request from `lmgw-client`'s `requests::feed` with the device key as a bearer, the
  body through its `FeedReader`. The cursor lives in memory for one run of the link: a reconnect
  resumes with it, a new pairing or a restart of Kai starts from now (`hello` brings the live
  state). `hello`'s principal gives Kai's own `by` (`FeedPrincipal::by`).
- **Bounds.** `lmgw.timeout_s` (default 10, new) for the request's answer and for the `hello`; two
  `hello.keepalive_s` intervals without a byte after it; each named in the tooltip's note. The
  backoff (`lmgw.retry_max_s`) starts over at 1 s after a connection that got its `hello`. Any
  answer but 200 and 401 (a 404 from an older lmgw, a 5xx) is a retry with the status, code and
  message in the note.
  *Changed during the build, 2026-10-07 (review KAI-2 K2-24):* `lmgw.timeout_s` bounds each chunk
  until the `hello`, not the `hello` as a whole: a server that dribbles bytes does not trip it.
  *Changed during the build, 2026-10-07 (review KAI-2 K2-6):* the dead-link verdict (two keep-alive
  intervals without a byte) also ends a bound voice session, as a drop with K5's notification: the
  socket shares the dead path.
- **Not followed yet.** `folder.current`, `folder.*` and `thread.*` wait for WP6, which gives Kai
  its folder (§4). *Followed since WP6, 2026-10-07:* the feed passes them on, and the core follows
  its own folder's (§4's notes). `resync` reaches the voice as a fact: with a session or a known thread it asks
  `current` again and rebinds at the next quiet point if the conversation moved. `hosts_label` is
  kept with the link's state for §10.1 and not shown. An event Kai cannot read, or one a newer
  gateway sends, is logged and skipped, never a link failure.

*Changed with lmgw main, 2026-10-07 (lmgw 66bb7fb):* `lmgw-client` tells an event of a type it does
not know (`FeedItem::Unknown`: logged by its name and skipped) from a known one whose data does not
read (`FeedItem::Unreadable`). The cursor has moved past the latter, so what it was about may be
shown stale: once the feed is up, Kai reloads as for `resync`. `hello.self_admin` and
`state.self_admin` say the device's level of lmgw's admin tools (§7); lmgw sends a `state` after
every move of it.
*Changed during the build, 2026-10-07 (review KAI-3 K3-3, K3-4):* the feed is up from its `hello`
on, not from the end of the chunk that carried it, so an unreadable event of the catch-up lmgw
sends right behind `hello` reloads too, however the transport cuts the stream. A `hello` that does
not read ends the try at once with "lmgw's hello does not read (this Kai may be older than lmgw)"
rather than as a missing hello after `lmgw.timeout_s`; the backoff stays.
*Changed with lmgw a50141d, 2026-10-08 (api §2.1's note of that day):* a device's feed opened
while a level change goes live begins with a keep-alive comment, and `hello` follows within one
keep-alive interval (15 s by default). `lmgw.timeout_s` (10 s) bounded each wait before `hello`,
so Kai dropped such a feed, tried again, and met the same wait each time. Every byte is progress
now, a comment included, and what may pass without one follows how far the stream got
(`feed::Wait`):
- **The request's answer and the body's first bytes:** `lmgw.timeout_s`, as before.
- **A body that began without `hello`:** two keep-alive intervals between bytes, as once it is up.
  The interval is the one lmgw stated in its last `hello` on the link's run. Before lmgw stated
  one, `lmgw.timeout_s` stands in for it (20 s by default, more than lmgw's default wait).
- **Up:** two `hello.keepalive_s` intervals, as before.

A stream that sends nothing is still dropped, and the tooltip's note and the log name the wait
that ran out: "lmgw sent no hello, nor any byte, within 10 s (lmgw.timeout_s)", or "lmgw began the
feed but sent no hello and nothing else for 20 s (two keep-alive intervals of 10 s: lmgw.timeout_s,
until lmgw states its keep-alive in a hello)". The bounds are the setting and lmgw's stated
interval, no constant of Kai's own. One case is left: a gateway whose keep-alive is more than
twice `lmgw.timeout_s` can cut a run's first feed short while a level change goes live, until a
`hello` states the interval. Raising `lmgw.timeout_s` covers it, and the config's doc says so.

*WP4, 2026-10-07:* the feed adapter hands the voice its rows through the `lmgw_link` seam as
`LinkEvent::Fact` (`Live` for `hello`'s and `state`'s `voice` and `turns`, `VoiceBound`,
`VoiceEnded`, `TurnStarted`, `TurnDone`, `FolderCurrent` for Kai's folder), each `by` marked `own`
when it is Kai's own principal.

### 3.3 Revocation and expiry

Kai shows "this device's key was revoked (<reason>); pair again", stops retrying, and keeps only
"Pair with lmgw…" on any of these:
- the feed's `revoked` event;
- a 4003 close of a WebSocket link;
- a 401 on the feed (`session_required` for a rotated or deleted key, `device_disabled`,
  `key_expired`).

The server's message is shown verbatim.

*Changed during the build, 2026-10-07:* a rotated or deleted key matches no row, so lmgw answers
it as a caller with no credential: `session_required` with its dashboard's login hint ("open the
login link printed in the process log, …"). Kai shows that as "lmgw no longer knows this device's
key: it was rotated or deleted (session_required: <the server's message>)". The `revoked` event's
message is shown as it comes ("revoked: device 'kai' was rotated — pair it again"). The feed and a
voice session may both say it; it is said once.
*Changed during the build, 2026-10-07 (WP6, lmgw 37da693 and 5c59874):* lmgw now answers a rotated
or deleted device key with 401 `device_key_unknown` and its own "pair again" message, and sends the
`revoked` message without its "revoked: " prefix. Both are shown verbatim. `session_required` keeps
its wording for an older gateway. A 401 on `current`, the folder routes or the bind is the same
revocation.
*Note, 2026-10-07 (review KAI-2 K2-24):* only the feed rewrites `session_required`; a 401 on the
folder routes, `current` or the bind shows the server's message as it comes, which for lmgw since
37da693 is its own "pair again" (`device_key_unknown`).
*Changed with lmgw main, 2026-10-07 (lmgw 66bb7fb):* the feed no longer rewrites
`session_required`, which lmgw main never sends a device key: every 401 shows lmgw's message
verbatim on every route, `device_key_unknown` (rotated or deleted: "this device key is no longer
valid (rotated or deleted) — pair the device again"), `device_disabled` and `key_expired` alike,
and is logged with `lmgw-client`'s `requests::key_refused`.
*Changed during the build, 2026-10-07 (review KAI-3 K3-6):* a disabled device is not "pair
again": lmgw keeps its key, which works again once the owner enables the device. On a 401
`device_disabled`, the feed's `revoked` with reason `disabled`, or a 4003 close whose reason is
lmgw's "<who> was disabled", Kai shows "this device was disabled on lmgw: enable it on Usage →
Keys, or pair again" with lmgw's message as a note, stops retrying, keeps the stored key, and
offers **Try again**, Pair with lmgw… and Quit. Try again loads the stored key and opens the feed
once more: connected once the device is enabled, the same state again (a 401 `device_disabled`)
while it is not. Of the two ways to resume, this one keeps §3.3's "stops retrying": a key that
opens nothing is not tried again until the owner says so, and the owner, who just enabled the
device on lmgw's dashboard, is the one who knows when; a slow retry would knock on lmgw every
`lmgw.retry_max_s` for as long as the device stays disabled. Rotated, deleted and expired keys
stay "pair again" with only Pair and Quit.
*Changed with lmgw main, 2026-10-07 (lmgw 7d6d18c, api §1.6 and §1.8 as the desktop client's note
changed them):* lmgw says what ended a key or a session by kind, and Kai reads only that, never
lmgw's words. A 4003 close is a revocation only, and its reason starts with a token, read with
`lmgw-client`'s `realtime::close_kind`; the feed's `revoked` carries the same token as `kind`; a 401
says it by its code (`requests::key_refused`). lmgw's sentence is shown without the token.
- `device_disabled` (401 `device_disabled`): enable it, as K3-6's note above.
- `key_expired` (401 `key_expired`): no longer "pair again", since lmgw keeps the key and its expiry
  can be moved. Kai shows "this device's key expired on lmgw: move its expiry on Usage → Keys, or
  pair again" with lmgw's message as a note, stops retrying, keeps the key, and offers Try again,
  Pair with lmgw… and Quit; Try again works as for a disabled device.
- `key_unknown` (401 `device_key_unknown`), and `revoked` (another kind of key's revocation): pair
  again, as before.
- A kind this build does not know, or none (an older gateway's): lmgw's guidance is to tell the
  user the sentence, and it gives no step beyond that. Kai says it in a notification and asks lmgw
  once with the key. The voice asks `current` and, where listening was on, binds the answer; the
  feed's event asks `current` for Kai's folder (the folder list while Kai knows none). A 401
  decides by its code. Any other answer, or none, starts the link again with the stored key, and
  its own answer (`hello`, or a 401) says the rest.
- A 4004 close is no revocation: the bound thread left this device's reach (the owner attached
  lmgw's admin tools to it, or what the device may see narrowed). Kai forgets the thread, says so in
  the tooltip ("the last thread left this device's reach (lmgw: …); Kai follows the folder's current
  thread"), asks `current`, and where listening was on binds the answer with its own bind (never a
  takeover, K5); a press session's thread is only remembered for the next press. A refusal of that
  `current` decides as any does, a 401 by its code. A 4004 never says "pair again".

Kai writes no thread settings, messages or folder defaults: it creates its folder with its name
only and asks `current`. So lmgw's 403 `chat_toolset_needs_full` cannot meet it, and no mapping is
built for it.

*Changed during the build, 2026-10-07 (found by the live check against lmgw 25ef1cc):* lmgw closes
the session with 4004 before its `current` and its bind see the change. The `current` Kai asked 2
to 13 ms after the close still named the thread, and the bind to it was either accepted and closed
with 4004 again, or refused 404, which left listening off. Kai now binds no answer naming the thread
that just left reach unless it was asked after the feed spoke of that thread (its removal or
return, the folder's current thread moving, a `resync`). It asks again once the feed has spoken, at
once if the feed spoke while the first answer was on its way, and takes that answer. The connect
timeout (`voice.connect_timeout_s`) bounds the wait. The same run found the bind's refusals unread:
lmgw sends them chunked, and tungstenite hands the body over as it came, so a 404 read as
`http_404`, and a 409 `chat_thread_bound` (K2-13) would not have been told apart. Kai joins the
chunks before it reads the code.

*Changed during the build, 2026-10-07 (review KAI-4):*
- **The owner's intent survives Kai's re-ask** (K4-1, K4-2). Every ask for the thread carries
  what it is for: whether its answer is bound, and whether the owner asked. A re-ask (after a
  4004, a bind's 404, a 401 without its code, an answer held back) keeps the intent of the
  request it replaces. A press while Kai asks makes the answer the owner's bind (a takeover, the
  hold's audio and release kept). Listening on looks at what is really on its way, not at a wait
  that only looks like opening: a request on its way is bound, else Kai asks now, and either way
  the microphone opens and the connect timeout runs.
- **The connect timeout counts from the first ask** (K4-6), never from a re-ask. When it runs out
  while Kai waits for lmgw's feed after a 4004, it says so: "Kai waited 15 s
  (voice.connect_timeout_s) for lmgw to confirm that thread 3 left this device's reach", not that
  lmgw failed to start the session.
- **A bind refused before the 101 is read by its status where its body did not come** (K4-5).
  tungstenite hands over only the body bytes that came with the head, so a body written later is
  missing, and Kai never guesses its code. A 404 is a 4004's twin (K4-8): the thread is forgotten,
  Kai asks for the folder's current thread and binds it, and listening, the press's audio and its
  key stay. A 409 on Kai's own bind is another client's voice (K2-13). A 401 asks `current`, whose
  401 is read whole, and its code decides; a second 401 without its code before a session is
  created is "pair again", as any 401 without a known code. The chunk join stays for the body that
  did come.
- **A revocation of a kind Kai does not know is said once, and met ever more slowly** (K4-7). Its
  sentence is notified once per reason, whether the feed's `revoked` or a 4003 said it, until a
  revocation decides. The feed no longer restarts at once after its question: it tries again after
  its own backoff (1 s doubling to `lmgw.retry_max_s`, in the tooltip), which a `hello` before such
  an end does not start over. A 4003 of such a kind binds again once; another in a row turns
  listening off, as any end that is not Kai's own does (K5), and the owner's press starts again.
- **A 4004 while the talk key is held** drops the held turn, since it talked into the session that
  ended, and the tooltip says so: "the held turn went with it: press the talk key again".

*Changed with lmgw a50141d, 2026-10-08 (api §1.6's note of 2026-10-07 and 2026-10-08):* the wait
for lmgw's feed after a 4004 is gone.
- **Why it could go.** lmgw closes with 4004 only once the change that took the thread out of reach
  is committed and live. So the `current` Kai asks on the close, and the bind after it, see the new
  state. A bind of a thread deleted in between is a 404, a 4004's twin (K4-8).
- **What Kai does now.** It binds `current`'s answer at once, as it comes, whatever thread it names
  (one that came back into reach included). It awaits no word from the feed.
- **What went with it** (review KAI-4 K4-3): the held-back answer and its re-ask, the watch's state,
  the `opening()` term for its wait, and the connect timeout's words for Kai's own wait. When the
  connect timeout runs out it says "lmgw did not start the voice session within N s" again, which
  is true now that nothing of Kai's own waits.
- **What stays of review KAI-4:** the intent kept across a re-ask, refusals read by their status,
  the quiet `chat_thread_not_found`, the feed's backoff after a revocation of an unknown kind, and
  the connect timeout counted from the first ask.
- **A deleted thread** ends a device's session as a hidden one does: the `error
  chat_thread_not_found` (no notification, §5.6) and the 4004. The device's feed hears
  `thread.deleted` and a fresh `state`, over its own connection and in no fixed order with the
  close. Each asks `current`, and Kai binds once either way.
- **`voice.ended {reason: thread_gone}`** now reaches the owner's stream only. Kai never read a
  `voice.ended`'s reason, only who ended it, so nothing changes.

## 4. The Kai conversation

- **The folder.** A Chat folder named "Kai" (`conversation.folder_name`), marked ongoing with an
  idle rollover (api §3).
  - At its first connected start, Kai looks it up by name. When names repeat, it takes the ongoing
    folder of that name with the lowest id and logs the duplicates. It then remembers the id.
  - When no such folder exists, Kai creates a plain folder of that name. Marking a folder ongoing
    requires a model, and Kai does not choose the conversation's model. The tooltip and one
    notification then say "set the Kai folder's model and mark it as an ongoing conversation in
    lmgw", with Open in lmgw.
  - Kai retries when `folder.updated` arrives. A `current` answered 409 `folder_no_model` gets the
    same message.
- **The thread.** Kai asks `POST /chat/api/folders/{id}/current`:
  - when a session opens;
  - when the folder's idle time passes with no turn;
  - when its thread is deleted or archived;
  - on `resync`.

  "New conversation" sends `{new: true}`.
- **What the thread runs with** is the folder's defaults: model, prompt, tools (including Kai's own
  label from phase 2, with `require_approval` from phase 3), voice settings, knowledge bases.
  - They are copied into each new thread, and api §3.4 also applies a changed default to the current
    thread.
  - Kai sets only what the client owns: turn detection, and `half_duplex` and `echo_tail_ms` as its
    echo mode implies.
- **Privacy is the folder's choice of aliases** (ruling 7).
  - In automatic mode Kai streams the microphone to lmgw for as long as listening is on. It goes to
    the ASR alias, and to its fallback when that alias has one.
  - The tooltip names `answered_by`, and K21 notifies once.
- **The phone** continues the same folder's current thread. If it binds the thread, Kai's session is
  taken over (§5.6).
  - The phone's turns run under the phone's key (api L4). Kai's tools therefore run in them only if
    the phone's tool scope names Kai's label (`kai__*`, api §5.6).
  - The owner's dashboard turns reach them as the owner's.
- **From phase 2 on**, the Kai thread still answers from the phone or the dashboard while Kai is off.
  lmgw skips an offline device's label instead of refusing the turn (api §5.3).
- **"Open in lmgw"** opens the thread's dashboard URL with `xdg-open`.
- **Text turns from Kai** are Later (the KRunner runner).

*Changed during the build, 2026-10-07 (WP4's real half):* until WP6 looks the folder up, its id is
read from `conversation.folder_id` in the configuration, the id this section has Kai remember.
Without it no session starts, and the note names the key.

*Changed during the build, 2026-10-07 (WP6; `src/conversation.rs`, `src/voice/thread.rs`):*
- **No hand-set id is needed any more.** `conversation.folder_name` (default "Kai") names the
  folder. `conversation.folder_id` is the id Kai remembers: written once Kai found an ongoing
  folder, removed when lmgw no longer lists it or on a confirmed re-pair to another gateway. Set by
  hand it picks the folder while that folder is ongoing (the first rule below).
- **The lookup runs at every connected start** (each `hello`) and after a `resync`, not only at the
  first one. Its order, where this section named only the second rule:
  1. the remembered folder, while it is ongoing, whatever its name now;
  2. else the ongoing folder of that name with the lowest id, the namesakes logged;
  3. else the remembered folder, not ongoing;
  4. else the plain folder of that name with the lowest id;
  5. else a new plain folder.
- **Kai's folder carries only its name.** A device that writes a folder's `archive_days` or
  `purge_days` is refused (403 `forbidden`, api L5's W5-1 note), so the year of history (§16
  answer 1) is the owner's to set. Marking the folder ongoing is the owner's too: it needs the
  model, which Kai never chooses, and the idle minutes (§16 answer 9) are set with it.
- **The owner's steps** are said in the dashboard's own words: "set the folder 'Kai''s model and mark
  it as an ongoing conversation in lmgw: in Chat, the folder's menu → Folder settings: pick its
  Model, tick Ongoing conversation (a new thread after 120 idle minutes), and under Retention set
  Delete after (days archived) to 365 or more, so its history stays a year". The retention part is
  left out when the folder keeps a year already; a folder with a model that is only not ongoing is
  asked only to be marked. The tooltip shows the steps while the folder is not ready. One
  notification says them per folder and state, and a press that `current` refuses says them again.
  `not_ongoing` is handled like `folder_no_model`. A 404 means the folder is gone: Kai looks it up
  again.
- **A folder that keeps its past threads less than a year** gets a tooltip line naming its
  Retention fields. This means less than a year by the folder's own days: the global setting is not
  a device's to read.
- **`current` at a connected start.** Kai also asks `current` once the folder is ready at a
  connected start, and when the folder becomes ready (§3.2's "retries `current`"). The tray's
  Continue item and the idle timer need the thread before the first press. After the idle time
  this rolls over at login, which the first press would do anyway, and it loads no model.
- **The idle rollover** is timed from the last turn Kai knows of in the thread: the feed's
  `turn.started` and `turn.done` of every principal, Kai's own included, and a reply of its own
  session. A thread that `current` just started has no message, so it is never idle, and no timer
  runs until its first turn. When the time passes during a turn or while the talk key is held, it
  starts over. `current` decides either way.
- **"Deleted or archived"** is read from `thread.deleted` and `thread.updated` (`archived_at`) of
  the conversation's thread, and from `folder.current` to none with reason `gone` (a device may read
  `first` and no previous thread). `not_ongoing` asks nothing: `folder.updated` says it.
- **`folder.deleted`** of Kai's folder ends the session as at quit, turns listening off with a
  notification, and Kai looks up again (which creates a plain folder and says the steps).
- **A `current` request on its way** (Kai's own) answers a press or listening on: its thread is
  bound, as a press already did for a new conversation.
- **A 401** on `current`, the folder routes or the bind is §3.3's revocation.
- **Open in lmgw** opens `<lmgw.url>/chat?t=<thread id>` (the dashboard's own deep link), or
  `<lmgw.url>/chat` while no thread is known. It is handed to `xdg-open` and not waited for.
- **An untitled thread** is "New chat" in the tray, the dashboard's name for a new one.
- **`voice.on_at_start`** on a first run waits until the folder is ready.

*Changed during the build, 2026-10-07 (review KAI-2):*
- **The idle rollover runs on the wall clock** (K2-8, the owner's decision), as lmgw's does: tokio's
  timers do not count a suspend, so after a night's sleep the first turns went into a thread lmgw
  had left. It counts from the thread's newest message as lmgw says it (`last_message_at`, in
  `current`'s answer and `thread.updated`) or a later turn Kai saw; a thread with no message has
  no timer (now true of an existing empty thread too, which the note above got wrong). The
  deadline is looked at every 30 s at the latest and at once when logind's `PrepareForSleep(false)`
  says the machine resumed. An idle `current` that kept the thread starts the timer over from the
  answer.
- **A 404 on `current`** (K2-11) no longer means "the folder is gone" by itself: after a backoff (1 s,
  doubling up to `lmgw.retry_max_s`) the folder list decides. Gone from it, the folder is forgotten
  as at `folder.deleted`; still in it (a gateway without the route, a proxy, the list and `current`
  disagreeing), Kai stops there with a tooltip note instead of asking again.
  `conversation.folder_id` is written only on a change.
- **A create settles first** (K2-22): a reconnect or `resync` while Kai creates its folder no longer
  replaces the create, and after a create Kai lists again and takes the namesake with the lowest
  id, so two creates that raced converge (the other folder is left, plain).
- **Open in lmgw while the folder waits for the owner's steps** opens that folder's settings form,
  `<lmgw.url>/chat?folder=<id>&settings=1` (lmgw's deep link), and the steps say so.

## 5. Voice

### 5.1 State

The voice state is the `lmgw-client` machine's: idle, listening, thinking, speaking or interrupted
(chat-voice §9.3). Kai adds:
- off;
- connecting;
- muted;
- unreachable;
- not paired;
- locked wallet;
- held, a badge (K9).

The tray icon is the first of these that applies:

1. not paired or locked
2. unreachable
3. muted
4. listening
5. thinking
6. speaking
7. idle

The hold is an amber badge on whichever icon shows. The ring gets `setState(state, {muted,
loading, held})`; `loading` names an alias from `lmgw.model.state`.

*WP4, 2026-10-07:* K11 leaves quiet listening to the tray alone, so the listening icon (a solid blue
disc) also shows while listening is on and nothing happens, and while the talk key talks;
interrupted shows as listening (the owner took over). Thinking, speaking and muted have icons of
their own (`kai-tray-thinking`, `-speaking`, `-muted`; muted is struck through in Breeze red).
Connecting and off show the idle icon, with "connecting the voice" or "idle (voice off)" as the
state line. Muted shows while voice is off too: it says a press will not be heard.
*Changed during the build, 2026-10-08 (the owner's decision, K11):* quiet continuous listening
shows the ring too, in the listening state; the tray's icons are unchanged. Push-to-talk with
listening on still shows the listening icon between presses, but no ring: it is in standby then.

### 5.2 Session lifecycle (K5, K16)

- **Listening on** (the toggle or the menu) opens one session bound to the current thread.
  - **In automatic mode** the microphone streams continuously, with the thread's resolved turn
    detection (`semantic_vad` or `server_vad`; `push_to_talk` becomes `semantic_vad`, as on the
    page). K16 sends silence while another principal's turn runs in the thread.
  - **In push-to-talk mode** the session stays open and audio is sent only while the talk key is
    held.
- **Hold-to-talk with listening off** opens a session at the press. It closes `voice.grace_s` after
  its reply has played.
- **Rebinding** after `folder.current`, or after the folder's idle time with no turn (Kai asks
  `current`, which rolls over), happens at the next quiet point: no open response, nothing playing,
  no held key.
  - Kai closes the old session as at quit and opens one bound to the new thread. Audio captured in
    between is buffered (§5.3).
  - If another client's voice session holds the new thread, Kai turns listening off instead and
    waits for a press (K5).
- **A drop that is not a takeover** turns listening off with a notification ("voice stopped: lmgw
  went away"). The next press reconnects.
  *Changed with lmgw main, 2026-10-07 (lmgw 66bb7fb, api §1.6):* lmgw closes every session with
  1001 and "lmgw is stopping or restarting" when it stops or restarts (a quit, an update's restart,
  SIGTERM or Ctrl-C), so that close is not a lost connection: the tooltip says "voice stopped: lmgw
  is stopping or restarting" and the notification "Kai: lmgw is stopping" with the same words and
  "press the talk key to start again once it is back". Listening is off as after a drop (K5); Kai
  does not rebind by itself. Kai's own close of a session (listening off, the grace, a rebind,
  quit) is 1000 with the reason "Kai closed the voice session", where it was a close frame without
  a code.
- **One open session holds one slot** of the device key's `concurrency_limit`; a refusal for it
  shows the server's message.
  *WP4, 2026-10-07:* the slot is held until lmgw answers the close, after its journal drained
  (api §1.3), so a rebind, or listening turned off and on again, binds only once the last close
  was answered, at most `voice.close_wait_s` later; the audio meanwhile is buffered (§5.3).
- *Changed during the build, 2026-10-07 (review KAI-2 K2-6, the owner's decision):* the session's
  socket finds a dead link and holds no unbounded queue:
  - a WebSocket ping every `voice.keepalive_s` (default 15); nothing heard from lmgw for two
    intervals, or a write it does not take in that long, ends the session as a drop (K5) that
    says so;
  - the outgoing queue holds `voice.send_queue_s` of audio (default 30, at least
    `voice.connect_timeout_s`, whose buffered audio goes into it at `session.created`); appends
    beyond it are dropped and said once (a notification and the tooltip, naming the key), and
    control events always go;
  - the feed's dead-link verdict ends the session too (§3.2).

### 5.3 Capture before the session exists

1. At the press, capture starts into a buffer, and Kai opens the WebSocket and binds the thread.
   This only happens while the feed is up (§3.2).
2. On `session.created`, Kai sends its `session.update` (turn detection, `half_duplex`), then the
   buffered audio as `input_audio_buffer.append` chunks in order, then live 20 ms chunks (PCM16 LE,
   mono, 24 kHz).
3. The buffer is bounded only in time, by `voice.connect_timeout_s` (default 15). Past it the
   buffer is dropped with a notification naming the setting and the seconds not sent.
   *Changed during the build, 2026-10-07:* the connect is given up with it (the socket closed,
   listening off): a session that did not start within the bound would only start the buffering
   over. The bound counts from the press, the `current` request included.

### 5.4 Push-to-talk, automatic, mute

- **Push-to-talk** sends `turn_detection: null`.
  - Press: `input_audio_buffer.clear`, then the pre-roll (the last `prefix_padding_ms` of the
    microphone, from the session echo, kept while listening is on) and the live chunks.
  - Release: `commit` and `response.create`.
  - A press while Kai speaks stops the reply first (§5.5) and consumes the gesture (K6).
- **Automatic** leaves turns to the server's detector.
- **Mute** sends silence, so an open turn ends naturally. It is a state of the running process
  (the prototype's pause), shown by the icon. While muted, push-to-talk presses are ignored and the
  tooltip says so.

### 5.5 Playback and barge-in

- **Playback.** Audio deltas (PCM16, 24 kHz) go into a playback ring per response item.
- **What was heard.** Per item, Kai counts the samples handed to PipeWire minus the stream's current
  delay (K7). *Changed during the build, 2026-10-06:* the playback callback logs where audio and
  silence went in the stream, so silence (an underrun, the gap between items) is never counted as
  heard, and a reader extrapolates from the cycle's `pw_time.now` through any silence into the next
  audio. Live, on a null sink that runs in the same cycle as its monitor: within 1 frame of the sink's
  own timeline (worst 1 frame, mean 0.5 frames over 625 probes). A flush answers with heard and
  played; what was played but not yet heard (at most the delay) still plays out. The flush waits
  for the callback at most `audio.flush_wait_ms` (default 1000), named in its note.
- **A barge-in** is `speech_started` while audio plays, or a talk press. Kai flushes playback, sends
  `conversation.item.truncate {item_id, content_index: 0, audio_end_ms: heard}`, and sends
  `response.cancel` only when the truncate did not end the response. The thread keeps exactly what
  was heard (chat-voice §8.4).
- **Half duplex** (§6.2): only the key interrupts.
- *WP4, 2026-10-07:* the playback stream opens at `response.created` and closes once nothing is
  open and everything was heard, so all-day quiet listening keeps no playback stream (and its sink
  may suspend). Every item in the player is flushed by a cut, so audio of a newer response queued
  behind the playing one is truncated too. A stream that PipeWire closed ends its replies; the
  capture opens again when PipeWire is back.
- *Changed during the build, 2026-10-07 (review KAI-1 K-11, K-12, K-19):* a stream PipeWire closed
  mid-reply ends each reply it played with a truncate at the heard position of its last report (no
  extrapolation: the stream is gone), and `response.cancel` only when the truncate did not end the
  response, so the thread keeps what was heard. While PipeWire is down no playback stream is opened
  (it could only fail, once per delta); a reply's audio then counts as unheard, truncated at 0 ms.
  An identical audio notice is notified once until PipeWire's connection changes, the repeats
  counted in the log. The playback callback's run log keeps 64 runs; dropping one is counted and
  logged once per stream (the heard position may jump then).

### 5.6 Takeover, hold, errors

- **`chat_thread_taken_over`** (close 4000), with the binder named (api §1.7): Kai goes idle,
  notifies with the name, and does not reconnect by itself. The next press takes the thread back.
- **`gpu_hold`** on a turn shows the amber badge, and one notification per hold period names the
  stage whose alias has no fallback. A fallback that answered is K21's notification.
- **Any other `error`** is shown verbatim (code and message) and logged. Repeats are counted in the
  log, never dropped.
- *WP4, 2026-10-07:* the stage comes from `lmgw.model.state {state: held}` (stage and alias); an
  `error gpu_hold` with no such frame notifies once with its message. A hold period runs from one
  change of the feed's `hold` to the next, so K21 also says a fallback outside a hold (one that
  answered because the model did not fit) once per such period. "Shown" is a tooltip line and,
  for the first of each code and message in a session, a notification.
- *Changed with lmgw main, 2026-10-07 (lmgw 7d6d18c):* a close is read with `lmgw-client`'s
  `realtime::close_kind`: 4000 a takeover, 1001 lmgw stopping or restarting, 4003 a revocation of a
  kind and 4004 the thread out of the device's reach (both §3.3's note); any other code is a drop.
  lmgw's `error chat_thread_not_found` that comes before a 4004 is still shown as any `error` is:
  Kai cannot tell it from a deleted thread's without reading its words.
- *Changed during the build, 2026-10-07 (review KAI-4 K4-4):* `error chat_thread_not_found` is no
  notification any more, by its code: it is logged, counted, and the tooltip's note until the 4004's
  own note replaces it. Every such error on a bound session means the thread is gone for this key,
  and a 4004 follows (on an older gateway, for a deletion, the feed's `thread.deleted`); Kai acts on
  either and says it there. Holding errors back on a timer to see whether a 4004 follows was
  rejected: it would need a guessed delay on every real error, and would still leak when the close
  came later than the hold.
- *Changed with lmgw a50141d, 2026-10-08:* a deletion of the bound thread now brings the same error
  and 4004 as a hide, on every gateway Kai is pinned to (§3.3's note of that day).

## 6. Audio

### 6.1 Streams

- **Two pipewire-rs streams**, `kai.capture` and `kai.playback`: S16LE, mono, 24 000 Hz, application
  name "Kai", icon `dev.rootshell.kai`.
- **Devices.** "Default" follows the system default; a node picked in the tray is stored by node name
  and targeted with `target.object`.
- **Levels for the ring.** Level and 48 bands for input and output, computed in Rust from the same
  samples, in the engine's own `SIGNAL` units (§9.3). They are computed only while the ring shows.
  *Changed during the build, 2026-10-06:*
  - The output tap analyses 2048 points at 24 kHz, as the Chat's player context does. The input tap
    analyses 1024 points at 24 kHz: the same 42.7 ms window and 23.4 Hz bins as the Chat's
    microphone analyser at the device's 48 kHz. Against lmgw's `Tap.read` (run verbatim by
    `scripts/viz-reference.mjs`), the output tap matches to float rounding and the input tap within
    4e-5 of the range.
  - "Only while the ring shows" is a reader: while one exists, the callbacks copy that side's
    samples into a small window; the analysis runs only when the reader reads.
    *Changed during the build, 2026-10-08 (K11):* the ring shows while Kai is active, so with
    continuous listening on the levels are computed all day.
  - Ring sizes are `audio.capture_ring_ms` and `audio.playback_ring_ms` (2000 each), named in the
    overflow and underrun notes; the player's own queue is unbounded, like the Chat's worklet.
  - *Changed during the build, 2026-10-07 (review KAI-1 K-6):* a level reader never gets a window
    of mixed old and new samples: a write publishes where it ends before it stores a sample, and a
    reader whose slots a write reached into, finished or not, copies again (a sequence lock).

### 6.2 Echo modes

| Mode | What Kai does | Session | Barge-in by voice |
|---|---|---|---|
| `device` (default, K8) | Nothing: the input device cancels echo against the default output's monitor. Kai must play on the default output. | full duplex | yes |
| `app` | Brings up the prototype's module-echo-cancel child with K20's changes: real defaults saved to `aec.json`, the defaults flipped to Kai's canceller, restored on exit, on a switch away or on the next start. *Changed during the build (K20's notes):* the nodes are `kai_aec_sink`/`kai_aec_source`, and Kai moves no stream itself. | full duplex | yes |
| `headphones` | Nothing. | full duplex | yes |
| `half_duplex` | Nothing. | `half_duplex: true`, `echo_tail_ms` from lmgw's settings | no; the key still stops |

- **`device` with an output other than the default** gets a tray warning, as in chat-voice §12.2. The
  warning offers half duplex.
- **`app` runs from Kai's start (or the switch to `app`) to its exit or a switch away.**
  - Without `libspa-aec-webrtc`, or past `aec.ready_timeout_s`, Kai shows the error (naming the
    setting), falls back to `half_duplex` and says so in the tooltip.
- *Changed during the build, 2026-10-06:* the device warning is a tooltip line and a menu item
  "Use half duplex (Kai's speaker is not the default output)". The echo note shows while unpaired
  too, since app AEC switches the defaults either way.
- **Against the Chat's names:** `device` = `device`, `app` takes the place of `browser` (a canceller
  in the client), `headphones` = `not_needed`, `half_duplex` = `none`.
- **A raw-channel microphone array** (client-side AEC and beamforming) is covered by `device` mode
  when a driver outside Kai does the processing and presents an echo-cancelled source. A fifth mode
  in Kai stays a possible extension for arrays without such a driver.
- *Note, 2026-10-07:* `app` stays a first-class mode, not a fallback, for everyone who runs Kai
  without an echo-cancelled input source.

## 7. Tray

- **ksni on tokio**, with the icon set of K9.
- **Tooltip:**
  - Title: "Kai: <thread title>".
  - Body: the state; the last turn's timing line from `lmgw.response.timing` (e.g. "first audio
    800 ms (turn end 400, ASR 30, first token 200, clause 100, audio 40), chat gemma4-e4b, cold:
    tts"), naming `answered_by` when a fallback answered; then any held, unreachable, echo,
    shortcut or feed-retry note.
- **Left click** toggles listening. **Middle click** mutes.
- **Menu:**
  1. Start listening / Stop listening
  2. Mute (check)
  3. Automatic / Push-to-talk (radio)
  4. Continue "<title>" (informational) and New conversation
  5. Open in lmgw
  6. Echo mode (radio: Input device cancels echo, Kai cancels echo, Headphones, Half duplex)
  7. Microphone, Speaker (Default plus PipeWire nodes)
  8. Overlays (phase 2: the shown overlays, Hide all). *WP5, 2026-10-07:* until phase 2 it holds
     only the check item "Ring over fullscreen windows" (`overlay.ring_over_fullscreen`, §16).
  9. Pair with lmgw…
  10. Start with Plasma (check)
  11. Quit

  *2026-10-08 (§13.1):* "Check for Updates…" sits between Start with Plasma and Quit, in every menu.

*WP4, 2026-10-07:* items 1 to 4 are built (Continue shows once a thread is known); item 5, Open in
lmgw, comes with WP6. *Built in WP6, 2026-10-07:* Open in lmgw (§4's note), and the folder's notes
(the lookup, the owner's steps, a short retention) after the voice's in the tooltip. The tooltip body is the state line, the timing line, the device, the hold,
the voice's notes (K16, "voice in use on …", muted, a pending rebind, connecting, the last drop,
takeover or error), then the audio, pairing and shortcut notes.
*Changed with lmgw main, 2026-10-07 (lmgw 66bb7fb, api L3 as the pre-merge review's P-3 changed
it):* the owner can give a device lmgw's admin tools at a level, `off`, `read_only` or `full`
(capped by the gateway's own self-admin level), said by name in `hello.self_admin` and
`state.self_admin`. After the device line the tooltip says "admin tools: read only" or "admin
tools: full" while the level is not off, and nothing at off; a level a newer lmgw adds is shown as
it is sent. A change reaches the tooltip live (the feed's `state`).
*Changed during the build, 2026-10-07 (review KAI-3 K3-7):* the line reads "admin tools: full (up
to lmgw's own level)", since the gateway's level caps the device's and the device is not told it,
and a level a newer lmgw adds reads in words ("ask first", not "ask_first").
*Changed during the build, 2026-10-07 (review KAI-3 K3-6):* while the device is disabled on lmgw
(§3.3) the menu is Try again, Pair with lmgw… and Quit, and a click on the icon says what to do
instead of "not paired".
*Changed with lmgw main, 2026-10-07 (lmgw 7d6d18c):* `hello.self_admin` and `state.self_admin` are
lmgw's typed `AdminLevel`, and since lmgw 3ac1155 they say what the device's admin tools may do now:
its own level already capped by the gateway's, with a `state` whenever either moves it. The line is
"admin tools: read only" or "admin tools: full" again; K3-7's "(up to lmgw's own level)" would now
be untrue. An expired key (§3.3's note) gets the disabled device's menu and click, saying to move
its expiry.

## 8. Global shortcuts

### 8.1 Actions

| Action | Press | Release |
|---|---|---|
| `talk` | starts a hold; or, while Kai speaks, stops it and consumes the gesture (K6) | under `shortcuts.hold_ms`: toggles listening; held longer: commits the push-to-talk turn |
| `new-conversation` | `current {new: true}`, rebind | — |
| `mute` | toggles mute | — |
| `hide-overlays` (phase 2; built in WP10) | hides every overlay, as the tray's Hide all | — |

- **Repeated press events while held are ignored.** The portal sends none: WP0 (f) saw `Activated`/`Deactivated` strictly alternating while the key was held, the first press-to-release at 111 ms. *(WP0, 2026-10-06)*
- **Kai binds no keys by default.** The portal's bind dialog asks at the first start, and System
  Settings → Shortcuts changes them later. Right Alt is AltGr on many layouts and must not be offered.
  *Decided by the owner, 2026-10-09:* `hide-overlays` stays unbound too; Kai suggests no keys.

### 8.2 Delivery

- **The XDG GlobalShortcuts portal** through ashpd: `CreateSession`, `BindShortcuts`, then
  `Activated` for a press and `Deactivated` for a release.
- **KGlobalAccel over D-Bus** (component `dev.rootshell.kai`) is the fallback if WP0 finds no *WP0 (f), 2026-10-06: the portal delivers `Deactivated` on KWin 6, so the portal is the path and KGlobalAccel stays only for a session without the portal.*
  `Deactivated` on KWin 6.
- **If neither delivers releases,** hold-to-talk degrades to press-to-start, press-to-commit, and
  the tooltip says so.
  *Not built (2026-10-07, review KAI-1 K-16):* Plasma 6's portal delivers releases (the owner's
  check below) and so does KGlobalAccel (`globalShortcutReleased`); Kai targets Plasma, so no
  deliverer without releases is left to degrade for.
- *WP4, 2026-10-07:* the portal path is built through ashpd 0.13 (`src/shortcuts.rs`), binding
  `talk`, `new-conversation` and `mute` at every start with no preferred trigger; the tooltip
  names the actions with no key. Verified (`just live-check-shortcuts`): in
  `app-dev.rootshell.kai-dev<n>.scope` the portal (version 2) answers `CreateSession` and
  `ListShortcuts`, its KDE backend sees the app id `dev.rootshell.kai`, and nothing is
  registered. `BindShortcuts` opens KDE's "Global Shortcuts Requested" dialog, which lists only
  shortcuts it has not seen (its QML has `newShortcuts` and `returningShortcuts`), so the bind,
  real presses and whether a second start stays silent are the owner's check
  (`just owner-check-shortcuts`). The KGlobalAccel fallback is built from its D-Bus API and not
  exercised live: registering would leave a component in the session it runs in.
- *The owner's check, 2026-10-07:* (A) the dialog listed the
  three actions with no preset key; presses arrived as strictly alternating `Activated`/`Deactivated`,
  short taps and long holds alike. (B) A second start bound silently, with no dialog. (C)
  Unregistering left no `dev.rootshell.kai` group; kglobalaccel's groups match the state before.
- *The owner's re-check after the KAI-1 fix (one ordered event stream), 2026-10-07:* (A) the dialog
  again listed the three actions with no preset key; presses arrived strictly alternating; (B) a
  second start bound silently (a short tap arrived); (C) no group left, kglobalaccel's
  groups identical to before. An earlier run that day was inconclusive: automated live checks that start
  Kai had left an empty `dev.rootshell.kai` group, so no dialog appeared and no key was assigned.
- *Changed during the build, 2026-10-07 (review KAI-1 K-2):* presses and releases are handled in
  their true order. Each deliverer's signals come from one message stream (one match rule on its
  interface and object, from its unique bus name; for the portal on the connection that owns the
  session, and only Kai's `session_handle` counts), not from two streams a `select!` could swap: a
  release handled before its press was dropped, and the press then held the talk key down with the
  microphone streaming. On top, a press that arrives after a release with a newer timestamp is that
  release's own press, late, and is dropped. `shortcuts.max_hold_s` (K6) is the last net.
- *Changed during the build, 2026-10-07 (the owner's shortcut check):* `shortcuts.bind` (default
  true) set to false registers no global shortcuts with Plasma at all, neither the portal nor
  KGlobalAccel, and the tooltip says the shortcuts are off. Every live check's configuration sets it
  (only `scripts/live-check-shortcuts.sh` and `scripts/owner-check-shortcuts.sh` bind, and they
  clean up), and every script that starts Kai checks afterwards that the owner's
  `kglobalshortcutsrc` gained no `dev.rootshell.kai` group (`scripts/shortcuts-guard.sh`).

### 8.3 App id

The portal names a host app by its systemd unit.
- The autostart entry runs as `app-dev.rootshell.kai@autostart.service`.
- Dev builds run as `app-dev.rootshell.kai-dev<n>.scope` (K12), and need `dev.rootshell.kai.desktop`
  installed, or the portal answers "An app id is required"; the recipe installs it. *(WP0, 2026-10-06)*
- A scheme-handler start goes through `systemd-run --user --scope -u app-dev.rootshell.kai-<rand>.scope`
  (K13), because `gtk-launch` creates no scope and the child keeps its caller's. *(WP0, 2026-10-06)*

WP0 (g) confirmed the first two forms and the dash rule; the real browser click is WP0 (m), the
owner's.

## 9. The ring overlay

### 9.1 Surface

- **One gtk-layer-shell window:**
  - layer OVERLAY (TOP when `overlay.ring_over_fullscreen` is off, §16), namespace `kai-ring`;
  - `KeyboardMode::None`, exclusive zone −1;
  - anchor, margins and size from configuration (default bottom right, 24 px, 240 px);
  - the output: Kai names it explicitly, `ring.output` if set, else the output with the focused
    window (from window management), falling back to nil (the compositor's choice). In a nested
    2-output KWin, nil went to output 0 and an explicit `wl_output` worked; whether KWin picks the
    active output for nil is unverified (only a single-output setup was available).
    *(WP0, 2026-10-06)*
  - *Changed during the build, 2026-10-07:* the focused window's output would need
    `org_kde_plasma_window_management`, a restricted interface §13 keeps out of the `.desktop`
    file until a phase-3 tool uses it. Kai asks KWin over D-Bus instead (`org.kde.KWin`
    `activeOutputName`, no grant needed): the output KWin itself treats as active, which is the
    focused window's, or the pointer's under "active screen follows mouse". GDK3 does not expose
    connector names, so Kai reads them (wl_output version 4 `name`) on a short Wayland connection of
    its own and matches the GDK monitor by position. A `ring.output` that is not connected, and an
    unknown active output, fall back with a logged note. Verified on a single-output setup. *Accepted by the owner, 2026-10-07:* the active output in place of the focused
    window's.
  - *Changed during the build, 2026-10-07 (review KAI-1 K-23):* the outputs' Wayland query waits at
    most 1000 ms (`OUTPUTS_TIMEOUT`, named in the note), and only one runs at a time; past it the
    compositor chooses, with a logged note.
  - *WP5, 2026-10-07:* gtk-layer-shell creates the layer surface at every map, so the layer and the
    output are chosen each time the ring shows; a change of `overlay.ring_over_fullscreen` (the
    tray's Overlays menu or the configuration) applies from the next turn.
    *Changed during the build, 2026-10-08 (K11):* the ring stays up through continuous listening,
    so a turn's start on a shown ring places it again: the output and layer of that moment, and
    gtk-layer-shell remaps only for one that changed.
    *Changed during the build, 2026-10-08 (review KAI-5 R5-2, R5-5):* a change of
    `overlay.ring_over_fullscreen` places a ring that is up again at once, the same way; a hidden
    one takes it the next time it shows. Live: the layer changed 5 to 6 ms after the toggle, on the
    same view. A ring that is up keeps its output when the output cannot be resolved at a
    re-placement (a stalled or failed query), so it never jumps to the compositor's choice; only a
    resolved output moves it.
    *Changed during the build, 2026-10-08 (review KAI-5 R5-12):* the log line of an unresolved
    output gives the cause and then what follows: the compositor's choice for a new map, the output
    it is on for a ring that is up.
    *Changed during the build, 2026-10-08 (review KAI-5 R5-14):* when the ring's output goes away
    (a monitor that powers off and unplugs), KWin closes the layer surface. gtk-layer-shell 0.10
    (Fedora 44's) ignores that close by default; older versions close the window. Verified in a
    nested KWin with two virtual outputs before the fix: the ring vanished with GTK still taking the
    window for mapped, the page went on being fed and drawn, nothing showed it again when the output
    came back, and only the next turn's start placed it anew; continuous listening in a quiet room
    could go hours without one. Kai now asks for the close (`gtk_layer_set_respect_close`, looked up
    at run time because the bindings lack it), keeps the window, and shows the ring again at once
    on the same view, on an output resolved then. A second close with no change of the outputs and
    no turn's start in between leaves it down, so a compositor that keeps closing it gets no loop,
    and the tray says so. An output added or removed places a ring that is up again, so an output
    that comes back gets the ring when `ring.output` or KWin's active output names it; a hidden
    ring takes the output the next time it shows. One output resolution runs at a time, and a show
    requested meanwhile gets its own once that ends: the removal and the close come together.
    Live (`just live-check-ring`, the nested part): Kai heard the removal and the close in the
    same millisecond and mapped the ring on the other output 8 ms later; KWin showed it there at
    its next look (within 120 ms of the switch), on the same view, its page drawing; with the
    output back the ring went back to `ring.output`, and without `ring.output` it stayed on KWin's
    active output. Not verified: the last output going away (KWin refuses to disable it), as when
    a single monitor powers off.
    *Changed during the build, 2026-10-08 (review KAI-6 R6-2, R6-3):* **no output at all.** When the
    output resolution for a show finds the compositor listing no output, nothing is mapped: a map
    with no output named would leave the choice to a compositor that has none to give, a state
    nothing here tests for KWin, GTK or WebKit, and whatever KWin did with it nobody would see. The
    ring is down and the tray says "no output is connected, so the ring is down; it shows again when
    one is". Any output added or removed resolves again, so an output that comes back gets the ring.
    A turn's start does not try again while there is none, and standby ends the state as it ends a
    close. The second close's log and tray line now say what Kai knows ("no output added or removed
    and no turn started since it closed it last"), not that the outputs did not change.
    `overlay.ring_over_fullscreen` places only a ring that is up, so a ring that is down stays down
    rather than mapping behind the lifecycle. Live (`just live-check-ring`, the nested part): KWin
    produces no zero-output state (it refuses to disable the last output, turns a disabled one back
    on when the last enabled one goes, tried with a screencast's virtual output as that one, and its
    virtual backend has one at least), so the check stands in for the empty list at Kai's resolution
    while a real output goes away under the ring with KWin's own removal and close. Kai mapped
    nothing, KWin had no window of Kai's, the tray had its line, a turn's start mapped nothing, and
    with the output back the ring mapped on KWin's active output on the same view, its page drawing,
    and the line went. Not verified: what KWin and GDK send when the last real output goes (the
    order of the removal and the close, and that the wl_output list is then empty), which is WP5's
    owner check.
  - Configuration: `ring.anchor` (`top-left`, `top`, `top-right`, `left`, `center`, `right`,
    `bottom-left`, `bottom`, `bottom-right`), `ring.margin`, `ring.size`, `ring.output` (a connector
    name), `overlay.ring_over_fullscreen`.
- **Transparency** uses the spike's full set: RGBA visual, app-paintable, transparent window CSS,
  and the WebView background (0, 0, 0, 0). WP5 drops whatever proves unneeded.
  *WP5, 2026-10-07:* all four are kept (none was tested away); the window CSS is set on the ring's
  window only, since a screen-wide rule would make the pairing dialog transparent. Verified: the
  crops' corners equal the desktop beneath, pixel for pixel, in every state.
- **Click-through:** a 1×1 px input region applied in `map-event` and after every size change. GDK
  never re-sends the input region after a resize, so re-applying both the 1×1 click-through region
  and the interactive rect in `size-allocate` is REQUIRED. *(WP0, 2026-10-06)*

### 9.2 Page

- **`kai://app/ring.html`** (K10), booted by an external module `boot.js` (inline scripts are
  blocked under the strict CSP). *(WP0, 2026-10-06)*
- **lmgw's `engine.js` and `ring.js`, byte-identical,** vendored from a pinned lmgw commit and
  checked by `just viz-check`.
  *WP5, 2026-10-07:* vendored at lmgw 116051e (`assets/overlay/viz/PIN`). `just viz-check`, part
  of `just check`, compares the copies with that commit and lmgw main with the pin (drift), checks
  that `src/audio/levels_reference.json` came from the same `engine.js`, runs the adapter through
  lmgw's own `Tap.read`, and compares the palette with lmgw's `voice.css`. `just viz-vendor`
  copies them again.
- **Kai's `adapter.js`** implements the `AnalyserNode` subset.
  *Changed during the build, 2026-10-07:* it is an exact inverse of `Tap.read`. The engine's bands
  share their edge bins, and on a 2048-point analyser the lowest bands are narrower than a bin, so
  no spectrum on the engine's own size gives back every target. The adapter is a 4096-point
  analyser at 16384 Hz that keeps its own size: every band owns a bin, which carries its power,
  and the shared bins read -200 dB. Round trip through `Tap.read`: within 1.2e-7. The spike's
  adapter was off by up to 0.9 of the range on a band (mean 0.023).
- **Transparency:** `inputs.transparent: true` once api §8 is in lmgw; until then the spike's 9-line
  shim. *WP5:* the shim lives only in `assets/overlay/transparent.js`; the swap is one edit there.
  *Done with lmgw main, 2026-10-07:* `engine.js` and `ring.js` vendored again at lmgw 0884d28, whose
  ring takes the flag (a context with alpha, cleared each frame where it filled the background);
  `boot.js` mounts with `transparent: true` and the shim is gone.
- **Palette** from lmgw's voice tokens, graphite and blue.
- **Reduced motion** follows GTK's `gtk-enable-animations`. *WP5:* read when the view is created
  and followed live (the page mounts the ring again).
- The page says `ready` before Kai sends any state.

### 9.3 Feeding

- **`setState`** from §5.1, **`setTiming`** from `lmgw.response.timing`.
- **Levels** at 30 Hz while shown (K11).
  *Changed during the build, 2026-10-08:* a feed equal to the last one sent is not sent; the
  page's analysers hold it, so the engine draws the same. Quiet listening in silence sends none
  (K11's numbers). The feed's log line at its stop counts the feeds sent and the ones not sent.
  A call that failed does not count as sent, so the next frame goes whatever it is (review KAI-5
  R5-9).
  *Measured in the build, 2026-10-08:* a sound reaches the drawn ring 19 to 35 ms (medians) after
  Kai's capture, 44 ms at worst; the live check holds it to two feed intervals and two of the
  display's frames, and the page's rate to the display's (K11's note).

### 9.4 Lifetime

- **Created hidden** when a voice session binds, so it is ready for the first `speech_started`
  (K1). The ring's first frame comes about 250 ms after view creation. *(WP0, 2026-10-06)*
- **Shown** from `speech_started` or a talk press to the end of the reply's playback (K11).
  *Changed during the build, 2026-10-08 (the owner's decision, K11):* shown while Kai is active,
  hidden in standby. Out of a turn an active ring holds the listening state, so in continuous
  listening a turn's end goes back to it; push-to-talk shows from the press through the turn's
  end, as before. The view is created when a session binds or Kai leaves standby, and lives while
  either holds. Live: the first frame 360 to 373 ms after continuous listening started on a new
  view; the ring stays up from a turn's start to its end and after.
- **Hidden** after the engine drew the idle state. A hidden page's rAF is paused.
  *WP5, 2026-10-07:* at the turn's end the page is sent idle and reports `settled` once the
  engine's transition has run (`STATE.transition`, 0.32 s; 0.5 s in reduced motion) and a frame
  was drawn; Kai then unmaps. Live: 327 to 337 ms from the turn's end. A page that never reports
  is unmapped after 2 s, with a logged note. Level readers exist only while mapped.
  *Changed during the build, 2026-10-08 (K11):* hidden when Kai enters standby. Live: 326 to
  337 ms from mute, 322 to 336 ms from a push-to-talk turn's end, and nothing mapped or drawn
  between presses; muted and unmuted, the same view shows again (first frame 35 to 41 ms after the
  show).
  *Changed during the build, 2026-10-08 (review KAI-5 R5-10):* a ring that enters standby before
  its surface was mapped (the output still being resolved) has nothing to ease out: it is hidden
  at once, and the late output maps nothing.
- *WP5, the seam for WP4:* the session sets `RingInput {bound, turn, state, muted, loading, held}`
  through `Core::set_ring`: `bound` from the bind to the session's end (the view is created hidden
  then), `turn` from `speech_started` or a talk press to the end of the reply's playback. While
  not in a turn, the page holds idle, whatever the voice state. `Core::set_ring_timing` carries
  `lmgw.response.timing`.
  *Changed during the build, 2026-10-08 (K11):* `RingInput` gains `active`, Kai not in standby
  (`Voice::standby`); a turn is always active. The ring shows exactly while it is set; out of a
  turn the page holds listening then, idle in standby.
- **Destroyed** when the session ends (K1), with its own ephemeral `WebContext` (non-default, per use,
  dropped with the view). With the default context the network process (about 14 MB) stays after the
  last view; the UI process idles at 60 to 63 MB once WebKit was used, 16 MB if never. *(WP0, 2026-10-06)*
  *Changed during the build, 2026-10-08 (K11):* a ring shown when the session ends (listening off,
  a drop) eases to idle first, and the view goes once the page reports that drawn; a session back
  during the ease (a quick rebind) keeps it. Live: hidden 329 to 340 ms after the end, then
  destroyed, no web process 0.5 s later.
- **A crashed web process** (`web-process-terminated`) is logged and recreated at the next turn.
  Voice is unaffected.
  *Changed during the build, 2026-10-08 (K11):* recreated at once while the view is wanted, so
  the ring comes back during continuous listening too. A second crash before the next turn or
  the next time Kai leaves standby waits for that, so a page that crashes at once never respawns
  in a loop; the log line says which. A page that does not say `ready` within `READY_LIMIT` counts
  the same. Live (the check kills its own web process): the ring back 323 to 336 ms after the
  kill; after a second kill no view until the next turn, whose view mapped 209 to 215 ms after its
  start.
  *Changed during the build, 2026-10-08 (review KAI-5 R5-3):* an active Kai with no ring until
  someone speaks is the failure that matters, so a second crash also starts a retry timer:
  `RETRY_FIRST` (30 s), doubling with each retry up to `RETRY_MAX` (300 s). A retry creates the
  view, which shows once ready while Kai is active (hidden in standby). A crash of a retried view
  waits for the next retry, so a page that keeps crashing gets one web process per wait, never a
  loop. A retried view still up when the next retry is due counts as recovered: the next crash is
  recreated at once again. The next turn's start or leaving standby ends the wait, as before. Each
  retry is logged with the next wait. While the ring is down, the tray's tooltip says so and when
  Kai tries again (`ring: its web process ended twice, so the ring is down; Kai starts it again at
  <time> (retry <n>; each wait doubles, up to 300 s)`). There is no cap on the number of retries:
  at the 300 s wait a page that keeps crashing costs one spawn every five minutes, and the tooltip
  says the ring is down the whole time. Live, with the first wait at 3 s: retry 1 created a view
  3.0 s after the second kill, mapped 3.3 s after it, in the listening state, and the tooltip's
  line went; the retried view, killed, came back with retry 2, 6 s after retry 1.
  *Changed during the build, 2026-10-08 (review KAI-5 R5-11):* the tooltip's line shows only
  while Kai is active. A bound session in standby (push-to-talk between presses, muted) still gets
  its retries, which create the view hidden, but its ring is meant to be hidden, so nothing the
  owner can see is missing and the tray says nothing.
- *Changed during the build, 2026-10-07 (review KAI-1 K-15):* a feed is skipped while the last
  `evaluate_javascript` has not completed, so a hung web process gets no queue of calls; the skips
  are counted in the log when the feed stops, and the page's stats count the feeds it received. A
  new view whose page does not say `ready` within 10 s (`READY_LIMIT`, logged with its value) is
  torn down like a crashed one and recreated at the next turn. *Changed, 2026-10-08:* it counts
  as a crash, so the crash bullet above says when a new view comes.

## 10. Phase 2: tool-controlled overlays and the MCP Apps host (decision level)

### 10.1 Decided now, so the API holds

- **The tools are Kai's hosted server's,** under the label `hello.hosts_label` names (e.g. `kai`):
  `<label>__overlay_image` and so on.
  - They reach exactly the principals api §5.6 allows: the owner, Kai, and keys and agents that name
    the label.
  - K15 applies by caller: drawing over the desktop is state-changing, so untrusted callers are
    confirmed.
- **One show tool per kind** (ruling 24, K22), each returning `{id}`:
  - `overlay_image {src}`, `src` as `http(s)` or `data:`;
  - `overlay_text {text}`, `overlay_emote {name}`, `overlay_weather {place}`;
  - `overlay_html {html}`, shown with no bridge (K18);
  - `overlay_app {server, uri}`, the MCP Apps `ui_resource` view of the server labelled `server`,
    with K18's bridge. It passes no tool input or result (K22's two parameters); the view loads its
    data through the bridge.

  Each takes an optional flat `position` (e.g. "bottom-right"; `overlay_app` none, its two
  parameters name the view: WP12's note). Output, margin, size, timeout and
  interactive come from the configuration's defaults, not from parameters; no timeout shows the
  overlay until it is hidden.
- **`overlay_hide {id?}`** hides one, or all without an id. **`overlay_list`** (no parameters)
  returns one line per overlay: id, kind, position, shown and expiry times, interactive, notes.
- **No update.** Showing never replaces an overlay; to change one, hide it and show a new one.
  *Changed, 2026-10-09 (ruling 24):* replaces `overlay_show {kind, data, anchor, size, timeout_ms,
  interactive}` and `overlay_update {id, …}`, whose `kind` decided what went in `data` (§16 Q11).
- **Processes** follow K1.
- **The sandbox** follows the extension's model, adapted to a native host:
  - views are iframes with `sandbox="allow-scripts"` (no same-origin), inside a widget host page on
    a `kai-sandbox://` origin separate from the ring's `kai://`;
  - the CSP is `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src
    data: blob:; font-src data:`;
  - the widget views refuse every load that is not `kai-sandbox:`, `data:` or `blob:`
    (`decide-policy` and resource-load hooks).
- **The MCP Apps bridge** (K18) speaks the extension's host side (`ui/initialize`,
  `ui/notifications/tool-input`, `…/tool-result`, `…/tool-cancelled`, `…/size-changed`, and proxied
  `tools/call` and `resources/read`). A call's view gets its `tool-result` as the server sent it
  (the content blocks, `structuredContent`, `isError`, from lmgw's `result` frame; WP12).
  - It answers only `overlay_app` views, by `MessageEvent.source`.
  - Their `tools/call` goes to lmgw's `/mcp` under Kai's key, for the serving server's tools only.
    Every call not marked `readOnlyHint` passes K15's confirmation. *Changed by the owner,
    2026-10-09:* unless the app is trusted: a view's calls run as the caller `app:<server>`, which
    `policy.trusted_callers` can name, and `policy.tools.<server>__<tool>` applies to them.
    *Changed after review, 2026-10-09 (draft):* trust skips the dialog only for
    tools marked `destructiveHint: false`; a destructive one still asks unless `policy.tools`
    allows it.
  - Their `resources/read` goes to `/mcp` (api §7).
  - Requested CSP domains (`connectDomains`, `resourceDomains`, `frameDomains`, `baseUriDomains`)
    and permissions (camera, microphone, geolocation, clipboard) are refused, named in
    `overlay_list`'s `notes` and logged.
- **Images arrive through Kai** (K19). `overlay_image` takes `src` as `http(s)` or `data:`.
- **Input.** Overlays are click-through unless the configuration makes the kind interactive
  (*decided by the owner, 2026-10-09:* MCP Apps views, `[overlay.app]`, are interactive by
  default). Then
  the input region is the widget's rectangle, or the rectangles a built-in declares, re-applied on
  every resize. No keyboard in phase 2.
  - *Known gap, 2026-10-09:* a widget's layer surface has `KeyboardMode::None`, so an interactive
    MCP Apps view takes clicks but no keys: a text field in a view cannot be typed into.
  - *Added after review, 2026-10-09 (draft):* an MCP Apps view sits on the OVERLAY
    layer, above Kai's own confirmation dialogs (normal toplevels). While any is open (K15/PD4's
    tool dialog, which a view's own calls use too, and K13's pairing dialog;
    `crate::widgets::DialogOpen`), every MCP Apps view takes no click, and its content gives way to
    Kai's note "<server>'s view waits while Kai asks you to confirm a call"; after, it is as
    configured. An
    interactive view carries Kai's strip "<server> — app view" above its content (24 px of its
    configured height; `hostContext.containerDimensions` is the rest), drawn by Kai outside the
    WebView, so the view's content cannot paint over it. Graphite and blue, Kai's palette.
  - *Decided by the owner, 2026-10-09 (built):* the transparent surface around the note still
    covered the dialog (a centred view over the centred dialog's title bar). While a Kai dialog is
    open the view now shrinks to Kai's strip alone, 24 px tall at the view's width, with the note
    in it, and moves to its output's top edge (the bottom edge for a view anchored at the bottom),
    keeping its horizontal place (`widgets::view::held_place`); no area of it can cover a dialog
    the compositor centres. After, its place, size and content are restored. Every MCP Apps view
    gets the strip this way, an interactive one shows it above its content as before. The tool
    dialog keeps the strip and `[overlay.app] margin` free above and below itself
    (`host::confirm::args_max_height`, `widgets::held_reserve`): its arguments scroll within what
    the smallest monitor leaves beside that, so centred it misses the strip on either edge. Known
    limits: GTK 3 under Wayland sees the monitor's height, neither Plasma's panels nor the title
    bar KWin adds, so a tool dialog within their height of its largest can still meet the strip,
    by at most half of it (what it meets is Kai's own strip, never the view's content); and a
    dialog whose parts other than the arguments alone exceed what the reserve leaves.
    `just live-check-widgets` asserts through KWin's geometry that the held view misses the
    dialog and is back at its size after (`shots/full-app-call-dialog-Virtual-0.png`).
- **Nothing lingers unseen:** the tray's Overlays menu, "Hide all", and the `hide-overlays` shortcut.
- **The first tool through the link** *(2026-10-08, K27)*: `conversation_start` and
  `conversation_end`, the smallest tools that prove `/mcp/host` end to end, ahead of the overlays.
- *As built in WP10, 2026-10-09 (taken in this draft):*
  - **The sandbox in layers** (`src/widgets/sandbox.rs`): the caller's document is served by
    Kai's `kai-sandbox:` handler with the CSP above as a response header (not `srcdoc`, which
    would inherit the host page's policy) into the `allow-scripts` frame; the host page has no
    script of its own (its CSP has no `script-src`); Kai's host script and its message handler
    live in an isolated world (`kai-widget`, top frame only), so the caller's document cannot
    post to Kai; a WebKit content filter (block everything, then `kai-sandbox:`, `data:`,
    `blob:`) refuses every other load in the web process whatever the CSP says, navigations
    included; `decide-policy` lets a view go only to its host page and its own document, and
    refuses every new window; the scheme handler serves a document only to its own view; every
    permission request is denied; the widgets' context knows no other scheme (`kai:` is not
    registered there). A refused navigation is logged with its scheme only (the URL is the
    caller's content).
  - **Nothing a widget does not need** *(review fixes, 2026-10-09)*: no sound or media (Web
    Audio, media elements, playback only on a user gesture; `SpeechSynthesisAPI` off, which in
    2.54 speaks through flite in the web process: a widget could have said "Kai, …" into Kai's
    own microphone), no WebGL, no fullscreen, `LinkPreconnect` off. Features without a setter are
    turned off by identifier from WebKit's own feature list at run time
    (`src/widgets/features.rs`); one this WebKit does not list, or that stays on, stops the show
    with an error naming it (fail closed). Script dialogs (alert, confirm, prompt) and print are
    swallowed. The live check runs inside a private PipeWire instance and sees no PipeWire client
    of Kai or its WebKit processes while a document tries every sound API (a GStreamer control
    shows up there; without these settings the web process did). The loopback server saw nothing
    of preconnect, dns-prefetch, prefetch, preload, modulepreload, speculation rules, a
    stylesheet, `@import`, `@font-face`, a CSS background, EventSource, `sendBeacon`, a meta
    refresh or a form; preconnect was stopped even with `LinkPreconnect` on (the content filter),
    so that one is depth. The DNS-prefetch setter does nothing in 2.54 (deprecated); a lookup
    cannot be seen by the loopback server.
  - **Logs are bounded per widget:** the first refused navigation and the first denied permission
    request of a widget are logged; the rest are counted, and `overlay_list` shows the counts
    (`; refused: 3 navigations, 1 permission request`).
  - **Placement:** one layer-shell surface per widget on the ring's window code, layer OVERLAY
    (over everything, like the ring, ruling 6), namespace `kai-widget`; the output is
    `overlay.output`, else KWin's active output at each show (the ring's resolution). A size
    beyond the output less the margins is fitted to it and the answer says so; text follows its
    lines up to the output's height and scrolls past it, said in the answer and in
    `overlay_list`'s notes. Widgets at the same position overlap (placement beyond `position`
    can wait, §10.2).
  - **Answers:** a show answers `id: <n>` once the surface is mapped (or an error naming why:
    no layer-shell, no output, `overlay.show_timeout_s` (default 10 s, at least 3: the output's
    resolution alone may take 2 s), its web process ended or stopped responding), plus a line per
    note; ids count up for Kai's run and are never reused; `overlay_hide` answers `hidden: <ids>`
    or `no overlay was shown`, and an unknown id is an error listing the shown ones.
  - **Defaults** (`[overlay.text]`, `[overlay.html]`, `src/widgets/settings.rs`): no timeout
    (shown until hidden) and click-through for both; text at `top-right`, 360 px wide, as tall as
    its lines; HTML at `top-left`, 480×320. `[overlay]` has `show_timeout_s = 10` and
    `unresponsive_s = 3`.
  - **Click-through is no input at all:** a click-through widget's surface gets an empty
    `wl_region` set by Kai through GDK's own Wayland objects (GDK3 turns an empty region into
    `nil`, the whole surface, which is why the ring keeps its 1×1 px stand-in). GDK sends its own
    `nil` in the first frame after the map, so Kai sets its region again in that frame's paint
    phase, before the commit. The live check reads the region in effect at every commit from a
    `WAYLAND_DEBUG` log: empty at each.
  - **Gone without a call:** a widget whose web process ends (all of them: one shared process)
    or whose output goes away (the compositor closes its surface) is hidden and logged; the next
    `overlay_list` says it once (`<id> <kind> <position> ended <time>: <why>`; with nothing else
    left that replaces "no overlays"); a show still on its way gets the reason as its error. The
    agent shows it again if it wants.
  - **A script that never yields** stalls the shared web process, so every widget. WebKit's own
    responsiveness check (`is-web-process-responsive`, watched) runs only when a page is used or
    closed: in 2.54 a stalled widget nobody touched went 120 s without a verdict. So Kai pings
    every widget view in its own world every `overlay.unresponsive_s` (3 s, WebKit's own
    timeout); a ping still unanswered at the next one ends the process (`terminate_web_process`)
    with every widget in it, one log line, the reason in each pending show's error and in
    `overlay_list`. Live: a loop before the document's load failed its show after 5.8 s, a loop
    after it ended the widget 5.8 s after its show, and a new widget showed in a new process. The tray's Overlays menu lists the widgets with a Hide each and
    Hide all, also while Kai is not paired.

### 10.2 Can wait

- the built-in widgets' designs;
- placement beyond `position` and the configured output;
- the extension's display modes;
- keyboard focus;
- overlays surviving a restart (they do not).

### 10.3 Open

Whether a tool's UI shows by itself when the tool runs in the Kai thread, or only on `overlay_app`
(§16 Q4; was `overlay_show {kind: "ui_resource"}` until 2026-10-09). Either way the `tool` frame
must carry the UI resource and the structured result (api §7.3). *Answered by the owner,
2026-10-09:* it shows by itself (WP12's note, "Owner decisions"); `overlay_app` stays.

## 11. Phase 3: desktop tools (decision level)

### 11.1 Decided now, so the API holds

- **Every tool is the hosted server's** (api §5). Kai opens no network listener; the browser socket
  is local, and so is the routines' CLI socket (K28, *2026-10-09*).
- **The contract and the tools** *(2026-10-08)*: K22's design rules, K23's `kai-desktop` crate and
  providers, K24's tool set, K25's delegates, K26's sync and async classes, K27's conversation mode.
  *2026-10-09:* K28's routines and K30's configuration tools are hosted tools too.
- **An installed Kai only** (ruling 16). A dev build answers `tool_execution_error` "needs the
  installed Kai: KWin grants <interface> per .desktop file". *(2026-10-08, K23:)* such a tool stays
  listed; a tool the desktop cannot support is not listed. *(WP13 (d), 2026-10-09:)* the ruling
  stands; KWin would also grant a dev build whose absolute path a `.desktop` in the user's
  applications directory names, so "installed only" is a choice (§16 Q21). *Answered by the
  owner, 2026-10-09:* a dev build holds the grant only by opting in, `just grant-dev` (§16 Q21).
- **Approvals** (api §6).
  - The Kai folder's defaults attach Kai's label with `require_approval` naming the state-changing
    tools.
  - A pending approval (`approval.requested`, or the bound session's `mcp_approval_request`) shows
    as an interactive overlay with keyboard on demand (Enter approves, Esc declines). It shows the
    call's remaining time. *(WP13 (c), 2026-10-09, draft decision:)* the layer-shell keyboard mode
    is `OnDemand`; the keyboard moves to the overlay the moment it appears, so Enter and Esc count
    only after a short arming delay (WP19). *(WP19, as built:)* a waiting thread call has no deadline in lmgw,
    so the card shows no time; the arming is `approvals.arm_ms` (WP19's notes).
  - The owner answers by click, or by voice under K17: hold the talk key and say yes or no. The
    recording goes through the dictation route and is matched against `approvals.yes` and
    `approvals.no` (configurable phrase lists, German and English defaults). Then Kai sends
    `mcp_approval_response` and `response.create`. *(WP19's review, 2026-10-09:)* by voice only a
    call of Kai's own conversation, once the overlay armed for it; a key before the arming leaves
    Ctrl+Enter, not Enter, to approve. *(Second review:)* the `response.create` carries an
    `event_id`; an `error` echoing it, whatever its code, means the verdicts were not taken.
  - An answer that matches neither list keeps the overlay and says "say yes or no, or click".
  - A decision made elsewhere (`approval.decided`, `lmgw.approval.decided`) closes the overlay.
- **K15 trust** by caller and approver. Cancelled calls never run.
- **Vision.**
  - `see_window` and `see_screen` use the Capture capability; on Plasma `org.kde.KWin.ScreenShot2`
    (`CaptureWindow`, `CaptureActiveWindow`, `CaptureScreen`). They return MCP image blocks.
  - lmgw sends them to the model by capability (egress §8.2). Fallbacks apply: a screenshot reaches
    whatever alias answers the thread.
  - *2026-10-08 (K25):* `look` takes the screenshot into the delegate's context, so it reaches the
    delegate's alias, not the conversation's.
  - A full capture of a large ultrawide desktop is the largest expected message (api §5.1's
    size settings).
- **Window facts** come from the Windows capability; on Plasma `org_kde_plasma_window_management`.
  The app id keys app memory. *Changed, 2026-10-08 (K23):* both Plasma interfaces are the `plasma`
  provider's, behind capability traits.
- **Programs run in their own scope** (K14): `open`, `app_launch` and `run_command` (K24).
- **App memory** lives in a Kai-local SQLite file (`$XDG_DATA_HOME/kai/memory.db`), keyed by app id.
  Vectors come from lmgw's `/v1/embeddings` with an `embed/` alias named in the configuration.
- **The browser.** `kai-browser-host` (native messaging, Firefox and Chromium) forwards to
  `$XDG_RUNTIME_DIR/kai/browser.sock` (mode 0600, peer credentials checked).
- *Decided by the owner, 2026-10-09 (§16 Q20):* app memory and browser integration go to
  `docs/ideas.md` for brainstorming first; the two bullets above are the facts that stand, not a
  design. WP23 and WP24 are parked until the ideas are designed.

### 11.2 Can wait

- the tool list beyond K24's (*changed, 2026-10-08:* K24 names about 35);
- live watching through `zkde_screencast_unstable_v1`;
- pixel clicking through `org_kde_kwin_fake_input` or the RemoteDesktop portal (confirmed by
  default).
  *Changed, 2026-10-08:* typing and keys are K24's `type_text` and `key_press`, and clicking an
  element is `ui_press` through AT-SPI, preferred over pixel input; all confirmed by default
  (*changed, 2026-10-09,* ruling 28);
- the memory schema and its encryption (§16);
- the browser extension;
- answering an approval by voice without the talk key.

**To verify before building on them** *(2026-10-08)*:
- AT-SPI on Plasma 6 Wayland: Qt apps expose their tree only while the accessibility bus reports
  enabled, and enabling it costs some apps performance; Chromium needs a flag.
- KRunner as a general search tool: most built-in runners are C++ plugins inside KRunner
  (KF6::Runner), not on D-Bus; reaching them would need a C++ helper, so apps and files are covered
  directly instead (`app_launch`, `find_files`).
- Klipper's D-Bus history calls in Plasma 6 (`clipboard_history`).

*Verdicts, 2026-10-09 (WP13, `spikes/p3/RESULTS.md`):* (a) and (b) pending the owner
(`spikes/p3/owner-checks.sh`); (c) verified: `OnDemand` works, the keyboard moves at map;
(d) verified: a `.desktop` grants a dev build the restricted interfaces; (e) verified:
`CaptureWindow` takes the window management uuid; (f) verified: KWin scripting needs no grant;
(g) verified: `baloosearch6` works, the baloo runner's `Match` is capped at 10; (h) verified: both
`org_kde_kwin_fake_input` v6 and the RemoteDesktop portal type Unicode; (i) comes from
`just tool-cost`.

## 12. Android later: what phase 1 must not foreclose

- **`lmgw-client` stays sans-IO:**
  - no tokio, GTK or PipeWire types and no clock reads;
  - owned FFI-friendly types;
  - wasm-compatible (K3).
- **Everything Kai uses is api A to H.** Nothing in lmgw is Kai's own.
- **The phone's path:** audio over WebSocket; WebRTC only if jitter hurts; reachability over
  WireGuard or Tailscale; push driven by the feed; the assist app. The `/mcp/host` binding needs a
  small custom transport in an Android MCP SDK (api §5.1).

## 13. Packaging

- **An RPM `kai`:**
  - `/usr/bin/kai`;
  - `/usr/share/applications/dev.rootshell.kai.desktop`, with `MimeType=x-scheme-handler/lmgw-pair;`
    and, from phase 3, the restricted-interface keys of ruling 16. They stay out until a tool
    uses them, so no build holds rights it does not use (WP2 briefly carried them; removed
    2026-10-06);
  - the icons;
  - `kai-browser-host` and its manifests (phase 3).
- **"Start with Plasma"** writes or removes `~/.config/autostart/dev.rootshell.kai.desktop`. It is
  switched on after the first pairing. *Changed during the build, 2026-10-07:* after the first
  pairing the owner confirmed (K13).
- **Build dependencies:** `gtk3-devel`, `webkit2gtk4.1-devel`, `gtk-layer-shell-devel` and
  `pipewire-devel`.
- **CI** tags every job (the runners are picked by tag). Publishing is the owner's call.
- *Changed during the build, 2026-10-07 (WP7, without the owner's acceptance):* `packaging/kai.spec`,
  `just rpm` (`packaging/build-rpm.sh` in a Fedora podman container of the host's release, only
  `target/rpm` mounted with `:Z`) and `.gitlab-ci.yml` (`default: tags: [docker]`, version
  `<Cargo version>+<pipeline IID>` as in lmgw, not run). Decisions: **(1) lmgw-client:** the build
  script stages the two crates the path in Cargo.toml names (`lmgw-client`, `lmgw-api-types`) and a
  minimal workspace root (for `version.workspace`) into the source tarball next to `kai/`, and
  rewrites the staged path to `../lmgw/crates/lmgw-client`; Cargo.toml itself is untouched, so the
  later one-line change needs nothing here. Not vendored (a path crate never is). **(2)** No
  `%post`/`%postun`: Fedora's file triggers refresh the desktop database and icon cache (lmgw's
  RPM has no scriptlets either). **(3)** Requires: `xdg-desktop-portal-kde`, `pipewire` (the
  `app` echo mode's `libspa-aec-webrtc.so` is in `pipewire-libs`, which it pulls; no package of its
  own), `xdg-utils` (Open in lmgw); Recommends `kf6-kwallet`. **(4)** The `%build` unsets rpm's
  `CFLAGS`: with them `aws-lc-sys` fails (jitterentropy must be compiled `-O0`). **(5)** The binary
  reports the Cargo version, not the CI's `+IID` (stamping Cargo.toml would break `--locked`).
- *Changed during the build, 2026-10-07 (review KAI-3 K3-5):* decision (1) stages the two crates
  and the workspace root from the commit `LMGW_PIN` names (`git archive` in that checkout), not
  from its working tree, and the spec's description names the commit; CI clones lmgw's project at
  the pin with the job token, which lmgw's job token permissions must allow (K3's note).

### 13.1 The updater

*Decided by the owner, 2026-10-08:* **Kai updates itself**, with its own updater on lmgw's shared
`lmgw-update` crate (lmgw 9fed9ae6; pinned at f57aa71e, where the crate sends the feed's auth only
to the feed's own origin): the manifest, the version order `0.1.0 < 0.1.0+12 < 0.1.1`, the
sha256-checked download, `pkexec dnf install`. Kai has **its own feed**: CI publishes every build of
`main` to the GitLab project's generic package registry, and the build carries that feed. **No built-in
URL:** a build without a feed (every local `just rpm`) has none.

- **The feed** (`src/update.rs`): `KAI_UPDATE_ENDPOINT` at run time, else `KAI_UPDATE_MANIFEST_URL`
  compiled in. Auth, each token bound to its own feed: `KAI_REGISTRY_TOKEN` (a user's token,
  `PRIVATE-TOKEN`) only together with `KAI_UPDATE_ENDPOINT`; `KAI_UPDATE_DEPLOY_TOKEN` compiled in
  (a read-only deploy token from a masked CI variable, `Deploy-Token`) only with the compiled-in
  feed; never a token to the public feed (a build marked `KAI_UPDATE_PUBLIC_FEED` by
  `packaging/gh-build.sh`, or any feed on GitHub's hosts). A token that is set but not sent is
  named in the log. Without a feed Kai logs one line and never polls, and the tray's check says
  "this build has no update feed". A feed with a token follows redirects only within its origin:
  reqwest strips only the standard auth headers when a redirect leaves the host, so the token's
  headers would go along; a redirect to another origin is an error that says so. No redirect is
  followed from https down to http. A manifest whose RPM has no 64-hex sha256 or no https URL is
  refused before the download.
- **Configuration** (`[update]`): `background_check` (default on), `first_check_s` (20),
  `interval_s` (21600, 6 h), `timeout_s` (30: the connect and each read, never a download as a
  whole; named in the error).
- **Tray:** "Check for Updates…" above Quit, in every menu (updating needs no pairing). The
  background check logs "up to date" and errors, and notifies only an update it can install; the
  tray's check always answers in a notification: up to date, the error, or the update.
- **The offer** is a notification "Kai X is available (you have Y)" with the manifest's notes and
  the actions Install and Later:
  - Install says "Installing Kai X", downloads to `kai-update.rpm` in Kai's runtime directory
    (`$XDG_RUNTIME_DIR/kai`, made 0700; a fixed path, never the manifest's file name, and not the
    shared temp directory; root's `pkexec dnf` reads it), runs `pkexec dnf install -y` on a
    blocking thread and deletes the file whatever the outcome; then "Updated to Kai X" with
    Restart, or "Kai's update failed" with the crate's error text. A file an install cut short
    left (Kai quit or was killed mid-download) is removed at the updater's start and before each
    download.
  - Later ends the offer; the next check offers it again.
  - Once installed, a check that finds the installed version says "Updated to Kai X" with Restart
    again rather than offering it; a newer one is offered.
  - Each outcome takes the place of the updater's last notification, so they do not pile up,
    except the installed update's "Updated to Kai X": later outcomes (an error, a newer offer) are
    shown beside it, so its Restart stays.
  - A button counts only on the updater's own notification and from the server that showed it:
    every listener gets every `ActionInvoked`, and any peer of the session can send one.
- **A host without `pkexec` and `dnf`:** the background check only logs a newer version; the tray's
  check says how to update instead of offering an Install that could only fail (lmgw's rule).
- **Restart** runs Kai's quit sequence (§2), exactly the tray's Quit: the voice's truncate and
  close, app AEC's devices restored, the tray item and the D-Bus name released, the overlays gone.
  Then `main` execs the installed binary in place, so the new Kai keeps the PID and with it the app
  scope and cgroup. The path is resolved at start (after an RPM upgrade `/proc/self/exe` names a
  deleted file); the new Kai starts plain, with no argument, and without the NVIDIA variable when
  Kai set it itself. A Restart that comes once a quit began is logged, and Kai only quits. A Quit
  during a Restart's quit sequence wins (Kai ends, nothing is started), and so does a SIGTERM or
  SIGINT any time until `main` would exec: the signal listener runs on a thread of its own that
  outlives Kai's runtime, and the flag it sets is checked right before the exec.
- **Version:** `KAI_VERSION` (the spec's `%{kai_version}`, `<X.Y.Z>+<pipeline IID>` in CI) is
  compiled in and is what `kai --version`, the log and the updater say and compare; Cargo.toml keeps
  `X.Y.Z`, so `--locked` holds. This replaces decision (5) of WP7 above.
- **CI** (`.gitlab-ci.yml`): `packaging/ci-build.sh` derives the feed from the CI variables (this
  project's registry by the project's id, so a moved or renamed project keeps installed builds'
  feed; `latest/latest.json`; no host in the tree), bakes it and the masked
  `KAI_UPDATE_DEPLOY_TOKEN` in (missing: a warning, and a build that cannot read its own feed), and
  writes `latest.json` (version, notes, date, the RPM's file, URL and sha256);
  `packaging/ci-publish.sh` uploads the RPM under its version and `latest.json` under `latest/`,
  one pipeline at a time (`resource_group`), and never moves `latest` backwards: when the
  published manifest names a newer version, only the RPM is uploaded. The job token is sent as a
  header from a pipe, never in an argv. lmgw's project path is `lmgw-dev/lmgw`, and lmgw's job
  token permissions allow Kai's project.
- **Tests** (`src/update/tests.rs`): the feed's resolution, and the flow on a fake feed and a fake
  desktop in tokio's paused time; the real feed's check and download against a loopback HTTP
  server. Nothing runs `pkexec` or `dnf`, and no notification reaches the desktop.

## 14. Testing and live checks

- **`lmgw-client`:** native unit tests.
- **Kai runs only against a dev lmgw** (`scripts/dev-instance.sh`), never the real data directory and
  never `127.0.0.1:8001`.
- **Speech:** only TTS-generated clips or locally kept recordings, never committed; checks with a
  real voice are manual.
- **Overlay checks:** crops only; nothing of the owner's screen leaves the machine or enters the
  repository.
- **No input is injected into the owner's session.** Clicks are verified by the owner.
- **Audio checks run in a private PipeWire instance** (`scripts/pw-private.sh`: own runtime, config
  and state directories, a D-Bus session that activates nothing, WirePlumber without hardware
  monitors, null sinks and a loopback microphone); `scripts/live-check-audio.sh` runs them. Audible
  checks and checks on the session's own defaults are the owner's (`scripts/owner-check-audio.sh`).
  *(WP3, 2026-10-06)*
- **A driving sink's monitor lags its consumption by one period:** the sink runs at the end of its
  cycle, so a stream reading its monitor gets that cycle's data in the next one. Ground truth for
  timing comes from a sink that does not drive the graph (`kai-test-sink-follower`).
  *(WP3, 2026-10-06)*
- *Changed during the build, 2026-10-07 (review KAI-1 K-13, K-14):* the voice tests' fake PipeWire
  reports 300 ms of latency, far more than one cycle, and stamps its time reports with tokio's
  paused clock, which the players read too. The truncate tests assert what the sink got less that
  latency, so a truncate at what was played fails them. `KAI_LIVE_NO_SHOTS=1 just live-check-ring`
  runs the ring's check with no screen capture.
- *Changed during the build, 2026-10-08 (review KAI-5 R5-14):* **output changes run in a nested
  KWin.** `scripts/nested-kwin.sh` starts `kwin_wayland --virtual` with two virtual outputs, its
  own D-Bus session and XDG directories, and a kscreen backend on that bus; its output switch
  refuses any other display or bus. Nothing shows on the owner's screen, and the owner's outputs
  and display settings are never touched. KWin itself says where a window is, through a KWin
  script that prints to KWin's log. Anything still running in the nested environment afterwards
  fails the run.
  *Changed, 2026-10-08 (review KAI-6 R6-1, R6-6):* the script takes only a new or empty directory,
  or one it used before (its marker file), and clears only what it made there, never the
  directory itself. Its bus is `pw-private.sh`'s kind: it listens in the nested runtime directory
  and has no service directories, so it activates nothing. The bus address is written to the
  directory, and the output switch and the nested test compare against it exactly, with the
  display's socket in the same directory.
- *WP4's real half, 2026-10-07:* `scripts/live-check-voice-lmgw.sh` (`just live-check-voice-lmgw`)
  runs Kai's core with the real feed and `RealtimeLink` and real PipeWire streams in the private
  instance, against a dev lmgw on 127.0.0.1:8897.
  - The test starts and stops the dev lmgw itself, since a restart is one of the checks.
  - Its chat is lmgw's `scripts/mock-openai.py` (`ECHO=1`), its ASR and TTS
    `scripts/mock-voice-audio.py`: no model, no GPU. lmgw has no standalone ASR mock; its
    chat-voice fakes live in its Rust suite.
  - The owner's actions use the dev lmgw's owner bearer from its log line, scrubbed from the kept
    log at the end. The device key lives in the test's memory: no wallet item.
  - lmgw trims a clause's leading silence (`longest_pause_ms`), so what the sink played is placed by
    the item's own audio, not by the TTS clip.
- *Changed during the build, 2026-10-07 (WP2):* the lmgw link's live check
  (`scripts/live-check-link.sh`, `just live-check-link`) runs Kai unattended against the dev lmgw
  on 127.0.0.1:8897. It needs the cargo feature `live-test`, which no default build has and a
  release build refuses to compile: with `KAI_LIVE_LINK=1` a pairing link is confirmed without
  Kai's dialog, notifications go to the log, and no shortcuts are bound (the portal would name the
  caller's app scope). Kai's config and state live under `target/`, it cannot reach PipeWire, and
  its wallet items are only for the dev lmgw's URLs, deleted at the end. The dev lmgw runs on its
  own data dir and container prefix.
- *Changed during the build, 2026-10-07 (review KAI-2 K2-2):* the live-test build accepts only the
  links the script minted and lives in `target/live-test` (K13's note). The voice and conversation
  checks assert that `kai.log` holds no device key and no audio; run them with `KAI_LOG=trace` to
  check the log cap under the worst level.
  *Verified after the KAI-2 fixes, 2026-10-07* (lmgw `feat/client-api` 3f75cee, its
  headless example built twice; all three at `KAI_LOG=trace`): `just live-check-link`
  PASS (no key or link in Kai's 182-line trace log; the live-test build from `target/live-test`);
  `just live-check-voice-lmgw` PASS (3473 lines, no key, no audio; appends logged as
  `samples=480`; lmgw's restart now closes the session with 1001 "lmgw is shutting down" 42 ms after
  its SIGINT, and lmgw quits 102 ms after it); `just live-check-conversation` PASS (1646 lines; the
  settings link `/chat?folder=1&settings=1` served 200; the idle rollover asked 59 983 ms after the
  reply was stored, timed from `last_message_at`). No check left a `dev.rootshell.kai` group in the
  owner's `kglobalshortcutsrc`. The first link run found a start that panicked (a timer made outside
  the runtime), fixed in 8518420.
- *Verified on lmgw main, 2026-10-07* (lmgw 66bb7fb, its headless example built from
  that checkout with no changes; Kai on `chore/lmgw-main`; the live checks' lmgw default is
  `../lmgw` now). Every live check passed:
  - `just live-check-link`: pairing, the hold badge, the backoff (1, 2, 4, 4 s), disable, rotate
    (401 `device_key_unknown`, lmgw's "this device key is no longer valid (rotated or deleted) —
    pair the device again" verbatim) and expiry each "pair again", the re-pair deleting the old item.
    New: the admin-tools level set with `key_set` reached the tooltip live ("admin tools: read
    only", then "full"), came back from the `hello` after the dev lmgw's restart, and left no line
    at off.
  - `just live-check-voice-lmgw`: round trips, the barge-in (truncate at 2176 ms heard, the sink
    got 2176 ms), K16, the takeover; lmgw's stop closed the session with 1001 "lmgw is stopping or
    restarting" at once, Kai notified "Kai: lmgw is stopping" with those words, listening off, no
    session in the 5 s after; the dev lmgw quit 20 ms after SIGINT.
  - `just live-check-conversation`: the first run, the recovery on `folder.updated`, Open in lmgw,
    the idle rollover (asked 60 005 ms after the reply), a new conversation, K5 against a second
    device key.
  - `scripts/live-check-pairing.sh`, `just live-check-audio`, `just live-check-ring` (run in full
    for the first time since the shortcut bind guard; the ring mounted with `transparent: true`, its
    crops' corners the desktop's own pixels), `just live-check-shortcuts` (dialog-free).

  No check left a wallet item, a process, a container, a `dev.rootshell.kai` group in the owner's
  `kglobalshortcutsrc` or a change in the owner's `~/.config/kai`.
- *Changed during the build, 2026-10-07 (review KAI-3):* `scripts/live-check-pairing.sh` crops the
  panel's strip, which `scripts/tray-crop.py` derives from the real geometry (kscreen-doctor,
  plasmashell's layout dump, plasmashellrc's thickness; one screen and one top or bottom panel,
  else no crop and the reason): the fixed bottom-right crop came out 1x1 with a panel at the
  top. A crop smaller than 100x16 fails the run. `just live-check-link`'s
  disable follows §3.3's new note: "enable it, or pair again" with Try again, the key kept, Try
  again while still disabled meeting the 401 again, and Try again after enabling connecting with
  the stored key.
  *Verified after the KAI-3 fixes, 2026-10-07* (lmgw 66bb7fb, the pin, its checkout
  clean and its headless example built from it): `just check` green (265 tests, no pin warning);
  `just live-check-link` PASS: a disable said "enable it on Usage → Keys, or pair again" from the
  `revoked` event, from Try again while still disabled (the 401 `device_disabled` again) and from
  the 401 at a restart, with Try again, Pair and Quit, no retry in 6 s and the key kept; enabled,
  Try again connected with the stored key, no re-pair; rotate and expiry still "pair again" (the
  first run failed on the check's own wait, a count read before the click, fixed in 628528c);
  `just live-check-voice-lmgw` PASS (the barge-in truncated at 2176 ms heard, the sink got 2176 ms);
  `just live-check-conversation` PASS; `scripts/live-check-pairing.sh` PASS, its crop a 60 px strip
  across the capture's full width, holding the panel and its tray; `just rpm` built
  kai-0.1.0-1.fc44 with both crates staged from the pin, its description naming the commit (not
  installed). No wallet item, process, container, `dev.rootshell.kai` shortcut group or change in
  the owner's `~/.config/kai` was left.
- *Verified on lmgw 7d6d18c, 2026-10-07* (LMGW_PIN 7d6d18c; lmgw main moved to 25ef1cc
  and then 0d75d42 during the run, neither changing `lmgw-client`, `lmgw-api-types`, nor the
  server's realtime, web, devices or migrations; the dev lmgw is the headless example built from
  25ef1cc):
  - `just check` green (281 tests); its only warning the pin's, since `../lmgw` was ahead of it.
  - `just live-check-link` PASS on lmgw 0d75d42. The admin-tools line follows the capped level:
    "read only" for the device at full under the gateway's read only, "full" once the gateway's
    level was raised. The feed's `revoked` is read by its kind: `device_disabled` waits as before;
    `key_unknown` says "pair again"; `key_expired` says "move its expiry on Usage → Keys, or pair
    again" from the event, from Try again while still expired and from the 401 at a restart, with
    Try again, Pair and Quit, no retry and the key kept, and once the expiry was moved Try again
    connected with the stored key.
  - `just live-check-conversation` PASS on lmgw 0d75d42, with a new step: the owner attached lmgw's
    admin tools to Kai's thread 3, and lmgw closed the session 3 ms later with 4004 "chat thread 3
    is out of reach for this key". Kai's first `current`, 3 ms after the close, still answered
    thread 3 and was not bound; the next, 15 ms after, answered thread 4 (rolled over). Kai bound
    thread 4 with its own bind 15 ms after the close, `session.created` at 25 ms: one bind,
    listening on, the pairing connected, no revocation notification, and thread 3 answers Kai's key
    with 404. The two runs before the guard (lmgw 25ef1cc) bound thread 3 again and met a second
    4004, or met a 404 that left listening off.
  - `just live-check-voice-lmgw` PASS on lmgw 0d75d42 (the barge-in truncated at 2112 ms heard, the
    sink got 2112 ms; the takeover's 4000 and lmgw's stop's 1001 as before).
  - `just live-check-voice` PASS (truncated at 1494 ms heard, the sink got 1495 ms).
  - `just live-check-ring` PASS: nine 240x240 crops of the ring, no full capture left.
  - `just rpm` built kai-0.1.0-1.fc44 with both crates staged from the pin, its description naming
    7d6d18c (not installed).

  No wallet item, process, container, `dev.rootshell.kai` shortcut group or change in the owner's
  `~/.config/kai` was left.
- *Verified on lmgw a50141d, 2026-10-08* (LMGW_PIN a50141d, lmgw main's HEAD
  throughout, its checkout clean; the dev lmgw is the headless example built from it):
  - `just check` green (295 tests), with no pin warning.
  - `just live-check-link` PASS: pairing, the hold badge, the admin-tools level live and after the
    restart, the backoff (1, 2, 4, 4 s), disable, rotate and expiry each by its kind, and the
    re-pair.
  - `just live-check-conversation` PASS. Its first run failed at step 2 on the check's own read. The
    feed's `folder.current` (reason `first`) came 11 ms after Kai asked `current` and before the
    answer. So the tray knew the thread first, and the check took the press's earlier refusal for
    the answer. Steps 2 and 5 now wait for Kai's own answer (3ee25d5).
  - Step 5b, the attach: lmgw closed the session 8 ms after the owner attached the admin tools,
    with one 4004. Kai's one `current` after the close answered thread 4 (rolled over) 10 ms after
    it, and Kai bound thread 4 at once with its own bind: `session.created` 18 ms after the close.
    One 4004, one bind, no notification, and thread 3 answers Kai's key with 404.
  - `just live-check-voice-lmgw` PASS (the barge-in truncated at 2197 ms heard, the sink got
    2197 ms; the takeover's 4000 and lmgw's stop's 1001 as before).
  - `just live-check-voice` PASS (truncated at 1494 ms heard, the sink got 1495 ms).
  - `just live-check-ring` PASS: nine 240x240 crops of the ring, no full capture left.
  - `just rpm` built kai-0.1.0-1.fc44 with both crates staged from the pin, its description naming
    a50141d (not installed).
  - Not met live: the feed's keep-alive comment before `hello` (no feed opened while a level change
    went live). The feed's unit tests cover it.
  - lmgw's usage accounting at this pin (a realtime speech row's `requests` counts answered
    clauses; speech and transcription rows record measured quantities) touches no check: none
    reads usage rows or counts lmgw's requests.

  No wallet item, process, container, `dev.rootshell.kai` shortcut group or change in the owner's
  `~/.config/kai` was left.

## 15. Work packages (phase 1, build order)

| WP | Scope | Done when |
|---|---|---|
| 0 | **Verified, 2026-10-06** (`spikes/overlay/RESULTS.md`, WP0 section): (b) first half, (c) in a nested KWin, (d), (e), (g) partly, (h), (i), (j), (n) HDR, (o), (p). **The owner's points, 2026-10-06** (the raw verdict record is kept out of the repository; the findings are summarised in `spikes/overlay/RESULTS.md`): (a) verified, 3 clicks on the button and 0 events outside it; (b) the ring shows over a borderless fullscreen application, its frame cost was not measured (the setting `overlay.ring_over_fullscreen` covers it either way); (f) verified, portal `Activated`/`Deactivated` alternate with no repeat while held; (m) a browser click (after its external-protocol prompt) starts the handler once; the lmgw-window half is untested, because lmgw shows no pairing link yet. The automated pre-test in a plain WebKitGTK view never launched the handler, so lmgw's window is expected to need to hand `lmgw-pair:` links to the system (see "Later"); verify it once api's pairing card exists; (l) not run (optional). **Verify the untested points**, one verdict each. **Overlay:** (a) a real click on an interactive region, the rest clicking through (owner, `click-test.sh`); (b) the ring over a fullscreen window and a fullscreen game; (c) two outputs; (d) the input region after a resize; (e) `file://` against `kai://` for ES modules; (n) fractional scale and HDR content. **Shortcuts:** (f) portal `Activated` and `Deactivated` on KWin 6, and whether a held key repeats `Activated`; KGlobalAccel's released signal as the fallback; (g) the portal app id for an autostarted run, a `app-dev.rootshell.kai-dev<n>.scope` run and a `gtk-launch` start. **Platform:** (h) KWallet answering the Secret Service API; (l) a locked wallet at login; (m) a `lmgw-pair:` click from lmgw's own window (WebKitGTK navigation policy) and from a browser, with and without a running Kai. **Costs:** (i) time from creating a view to the ring's first frame; (j) 30 Hz feeding through `evaluate_javascript` against one cheaper transport; (o) PSS of the UI process without a view, and of the network process after the last view is destroyed; (p) whether pipewire-rs exposes `pw_stream_get_time_n`. Phase 3's WP0 adds layer-shell on-demand keyboard for the approval overlay. *Phase 3's WP13, 2026-10-09* (`spikes/p3/RESULTS.md`): (c) to (h) verified, (a) and (b) pending the owner, (i) from `just tool-cost`; §11.2 has a line each. | `spikes/overlay/RESULTS.md` gains a WP0 section with a verdict per point; any failed point changes this spec first |
| 1 | `lmgw-client` in lmgw's workspace (api §4.1) | lmgw-ui's voice panel runs on it unchanged; native tests green |
| 2 | Kai skeleton: NVIDIA rule, configuration, single instance, Secret Service with the locked state, pairing (K13's handler path and the paste dialog), the feed task with cursor, reconnects and `resync`, revocation and expiry handling (§3.3), the tray with not paired, locked, unreachable, held badge and idle | paired against a dev lmgw; icon follows the dev hold and the dev instance stopping and starting; disable, rotate and expiry each show "pair again". *Verified, 2026-10-07* (`just live-check-link`, lmgw `feat/client-api` 18b6fb9): a device key minted with `key_create`, handed over by the scheme handler's path and confirmed by the live-test build, stored, the `hello` naming the device in the tooltip; the amber badge on and off with the dev hold; the dev lmgw stopped: grey within the poll, the waits 1, 2, 4, 4 s (`lmgw.retry_max_s = 4`) in the tooltip, a tray click said "lmgw is not running" 6 ms later; started again: idle; disable, rotate and expiry each ended the feed with its `revoked` event and, at a restart of Kai, with the 401 (`device_disabled`, `session_required`, `key_expired`): "pair again" with the server's words, only Pair and Quit, no retry in 6 s; a re-pair to `http://localhost:8897` deleted the item for `http://127.0.0.1:8897`; no key or link in Kai's logs; a clean quit. |
| 3 | Audio: pipewire-rs capture and playback, device choice, the four echo modes with `aec.rs` ported under K20, levels and bands, the heard position | a TTS clip's heard position within one period of PipeWire's reported delay; app AEC restored after a `kill -9` of Kai. *Verified, 2026-10-06, in a private PipeWire instance (`scripts/live-check-audio.sh`): heard vs played minus the reported delay within 511 of 512 frames (one period); against the sink's own timeline within 1 frame; after `kill -9` the defaults stayed on Kai's canceller and the next start restored them (the absent source key removed, not pinned). On real hardware: the owner's `scripts/owner-check-audio.sh`.* |
| 4 | The voice session: shortcuts with K6's rules, buffered capture, bind to the current thread, push-to-talk and automatic, mute, truncate-first barge-in, grace close, rebind with K5's rule, K16, takeover, drop handling, hold, K21, tray states and timing | TTS-driven round trips in both modes; a barge-in leaves the heard text in the thread; a dashboard bind takes over with its name shown; a dashboard text turn silences automatic listening until it is done; a dev lmgw restart turns listening off with a notification. *The local half verified, 2026-10-07*, against a scripted lmgw behind the `VoiceLink` seam (`src/voice/mock.rs`, after lmgw's `scripts/realtime-mock.js`) through Kai's core (`src/voice/tests.rs`): push-to-talk and automatic round trips; a barge-in and a stopping press leave the heard text (the reply cut by share of its audio at the truncate's `audio_end_ms`); a takeover names who took it and Kai does not reconnect; another principal's turn silences automatic listening until `turn.done`, a press still talks; a drop turns listening off with a notification. Live in a private PipeWire instance (`just live-check-voice`): a clip spoken 150 ms after the press reached lmgw whole across a 500 ms cold bind (correlation 0.995, the capture running 21 ms after the press); a barge-in 1.5 s into the reply truncated at 1494 to 1515 ms heard, the sink getting 1 ms more each time (three runs). *The real-lmgw half verified, 2026-10-07* (`just live-check-voice-lmgw`; lmgw `feat/client-api` 5c59874 with lmgw-client's changes in progress; two runs; chat, ASR and TTS mocked, every reply clause the 4.15 s TTS clip): a press with listening off bound in 20 to 23 ms (101) and the whole hold went up (4585 to 4606 ms of audio for a 4605 ms hold); release to the first reply sound on the sink 511 to 532 ms in that press session and 566 to 567 ms in push-to-talk mode; automatic (`semantic_vad`) from the end of the speech to the first reply sound 902 to 945 ms; the tooltip's timing line came from `lmgw.response.timing` ("first audio 482 ms (ASR 49, first token 65, clause 351, audio 15), chat kai-chat"); a barge-in 1.5 s into Kai's speech: `speech_started` 639 to 681 ms after the clip began (its own 338 ms of leading silence included), the truncate 7 to 22 ms after it at 2154 ms heard, the sink getting 1 ms more, no `response.cancel`, and the thread kept the heard prefix of the echoed text; a dashboard text turn of 7.6 s: Kai's note 9 ms after the send began, 380 and 381 appends meanwhile, all silence, with the clip playing into the microphone and no `speech_started`, the microphone heard again at the next append after `turn.done`; a dashboard bind closed Kai's session 7 to 8 ms after its handshake (4000 "voice mode moved to the dashboard"), the notification and the tooltip naming the dashboard, no session in the 5 s after; a dev lmgw restart: sent SIGINT with Kai's feed and session open, lmgw closed neither and did not quit within 15 s, so the drop came at its kill (1006, "voice stopped: lmgw went away (…)", listening off); started again, the feed was up 0.8 s later, and no session opened in the next 5 s. With Kai quit, the same lmgw quit 19 ms after SIGINT. |
| 5 | The ring overlay: layer-shell window, its own web process, `kai://`, the vendored files with `just viz-check`, the adapter, click-through, shown only during turns, destroyed when hidden, feeding, reduced motion | ring states visible over the owner's desktop (crops); quiet listening renders nothing; no web process left after hide. *Verified, 2026-10-07* (`just live-check-ring`: the overlay as Kai runs it on the owner's desktop, levels from a private PipeWire instance): crops of every state (listening and interrupted amber, thinking and speaking blue, the corners equal to the desktop beneath); quiet listening with sound on the microphone: the surface never mapped, no frame drawn in 2 s, no level reader, the web process at 0.0 % of a core; no WebKit process 0.5 s after the session ends; the first frame 78 to 129 ms after a turn starts on a pre-created view, 34 to 41 ms later, 231 to 258 ms for a view created at the press; reduced motion re-mounts calm; `ring_over_fullscreen` off maps on TOP; the tray's check item writes the setting; Kai at idle has no web process and quits cleanly. *Changed, 2026-10-08 (the owner's decision, K11):* the ring shows while Kai is active and hides in standby. `just live-check-ring` now checks: continuous listening starts with the ring in the listening state; its cost over 10 s of silence (as Kai feeds and with every frame sent) and 10 s of a quiet room's noise (K11's numbers); sound on the microphone between turns moves it; a turn on it keeps it up and ends in listening; mute eases it out and keeps the view, unmute shows it again; its own web process killed once brings a new view at once, twice waits for the next turn; the session's end eases it out, then the view goes; push-to-talk shows from the press and nothing between presses. *Changed, 2026-10-08 (review KAI-5):* it also toggles `ring_over_fullscreen` while the ring is up (the layer changes at once, same view); after the second kill it checks the tray's line, no view at once, the retry's view with no turn, and a retried view's crash waiting for the next retry; and it measures `kwin_wayland`'s CPU and the GPU's P-state and power against no view and a hidden view (K11's numbers). *Changed, 2026-10-08 (review KAI-5 R5-14):* it also switches the ring's output off and on in a nested KWin, with and without `ring.output`, and with the ring up and hidden, and asks KWin where the ring went (§9.1's note). *Changed, 2026-10-08 (the owner's latency report):* it measures how late the ring shows tone bursts on the microphone and holds Kai's part to two feed intervals and two frames (K11's note). *Changed, 2026-10-08 (review KAI-6):* the nested part also takes the output list away from Kai's resolution while an output goes under the ring (nothing maps, the tray says so, a turn maps nothing, the output back shows it), and the latency's frames are the display's. **Owner's check, open:** with Kai running and the ring up, the last output switched off and on again (a monitor powered off, then on), and Kai's log lines read back: the ring not mapped while there is no output, the tray's line, the ring back on the output (§9.1's KAI-6 note). |
| 6 | The Kai conversation: folder lookup and creation, the no-model path, `current` at session start, after idle and after its thread was deleted or archived; new conversation; Open in lmgw; K5's no-rebind onto another client's thread | a rollover after a shortened idle time on the dev instance; the first run with a model-less folder shows the message and recovers on `folder.updated`; a second device key binding the rolled-over thread first makes Kai stay off. *Verified, 2026-10-07* (`just live-check-conversation`; lmgw `feat/client-api` 0993fea, its headless example built from the worktree with the fix agent's changes in progress; chat, ASR and TTS mocked; Kai's audio in the private PipeWire instance; one run): **the first run**: no folder "Kai" on the gateway, so Kai created folder 1 with its name only (model, ongoing and retention null as the owner reads it), notified the owner's steps 8 ms after the feed was up and showed them in the tooltip; a device's write of the folder's `purge_days` was refused (403 `forbidden`); a press was refused `not_ongoing` and said the steps again, with no session. The owner's patch (model `kai-chat`, ongoing with 1 idle minute, `purge_days` 365) arrived as `folder.updated`: 19 ms later Kai had asked `current` (thread 1, a new one), remembered `conversation.folder_id = 1`, dropped the steps and showed "Kai: New chat", Continue "New chat" and Open in lmgw. Open in lmgw named `http://127.0.0.1:8897/chat?t=1`, which the dev lmgw serves (200 `text/html`); no browser was opened. **The rollover**: a push-to-talk turn in thread 1, then nothing. Kai asked `current` 60 005 ms after the reply was stored; lmgw rolled over (reason `idle`, thread 2), and Kai rebound at the quiet point: the open came 60 035 ms after the reply and `session.created` 60 044 ms after it. A turn in thread 2, then New conversation: `current {new: true}` answered thread 3 in 9 ms, and the rebind's open came 48 ms after the item. **K5**: a turn in thread 3. 50 s later Kai's talk key went down and stayed down. At 64 s a second device key's `current` rolled the conversation over (`idle`, thread 4), and it bound thread 4 13 ms later. Kai's tooltip then said "voice in use on device 'kai-live-2'" and "the conversation moved: Kai rebinds at the next quiet point". The release committed the held turn in thread 3, whose reply played. 8.7 s after the release Kai notified "voice in use on device 'kai-live-2'; Kai stays off until you press the talk key", with listening off, no socket opened to thread 4 and the other device's session untouched. The dev lmgw quit 19 ms after SIGINT; no key or owner login is left in the logs. |
| 7 | Packaging: RPM, `.desktop`, the scheme handler, "Start with Plasma", icons; the owner's live acceptance | Kai autostarts in the owner's session, pairs, talks; the owner signs off |

lmgw-side order: api §10. WP1 needs api WP6; WP2 needs api WP2 and WP4; WP4 needs api WP3; WP6
needs api WP5. WP5 works with the shim until api WP1 lands.

## 15.1 Work packages, phases 2 and 3 (build order)

*Planned 2026-10-09, against lmgw f57aa71e (`LMGW_PIN`).* The decisions stay where they are (rulings
5 to 29, K1, K2, K14 to K31, §10, §11); this section orders the build, names what lmgw must build
first, and cuts Kai's work by file ownership so that WPs in different groups can run in separate
worktrees (K3's rule for where a Kai worktree lives still holds).

### 15.1.1 lmgw at the pin (read 2026-10-09)

Built and relied on:
- **The hosting grant is stored, not served.** `api_keys.hosts_label` is validated (unique and
  case-insensitive, not a server's prefix or name; `mcp_server_set` and agents refuse a device's
  label, `ops/mcp_server.rs`) and shown on the Devices card. `devices::links::LinkKind::Tools`
  counts a host link for "online (… tools)"; nothing opens one yet.
- **A device key holds `Inference`** (api §1.2), so a delegate's `/v1/chat/completions`, the
  embeddings of app memory and K22's `/v1/count_tokens` need nothing new.
- **`/v1/responses`** has `require_approval` and `mcp_approval_response` (server-tools); an unbound
  realtime session renders a call with no output yet as "(no result yet)" (`realtime/render.rs`).
- **rmcp 2.0** is lmgw's southbound MCP client (client role only). rmcp 2.0's model already has
  `tasks/*`, `CreateTaskResult` and protocol `2026-07-28`; `/mcp` speaks 2025-11-25 at most.

Missing (each a prerequisite below):
- **E (api WP7):** no `GET /mcp/host`; `McpTransport` (`config/mcp.rs`) is stdio, http and sse,
  with no `Device`; no device row from the grant, no `mcp.host_*` settings, no `_meta` stamping, no
  `notifications/cancelled` to a device; L16's device-row reach is not in `ToolScope`
  (`mcp/scope.rs`), and W3-9's `narrows()` shortcut is still open.
- **G (api WP8):** `/mcp` advertises `tools` only and answers `resources/*` with -32601
  (`mcp/ingress.rs`); no URI namespacing, no `_meta.ui.visibility` in model runs; the Chat `tool`
  frame has no `server_label`, `ui_resource` or `structured_content`.
- **F (api WP9):** `ThreadMcp` (`store/chat.rs`) has no `require_approval`, and its comment says
  why; no approvals route, no `pending_approvals`, no `approval.*` feed records, no bound-session
  `mcp_approval_request` or `lmgw.approval.decided`.
- **MCP Tasks (K26):** in no lmgw design and no code. Nothing puts a late tool result into a
  thread, and nothing answers a bound session's `response.create` that follows no user item.
- **`lmgw-client` and `lmgw-api-types`:** no host-link types (the `_meta` caller, approval and
  timeout; `host_not_granted`; the 4000 and 4003 closes), no approvals or `…/transcribe` request
  builder, no approval or task events.
- **The shared UI crate (K29, §16 Q18):** `lmgw-ui` is a binary crate (`src/main.rs`, trunk). Its
  `widgets/` import `crate::catalog`, `fmt`, `charts`, `scope` and `pages`, so Kai can consume none
  of it before the extraction. The tokens and CSS are `crates/lmgw-ui/assets/app.css`.
- **Personality profiles (ruling 29):** being specified in lmgw's
  `docs/design/2026-10-09-personality-profiles-design.md`. Kai relies only on "list the profiles;
  set the thread's profile".
- **A tool-calling mock:** `scripts/mock-openai.py` emits no tool calls, so no live check can drive a
  hosted tool through a Chat or bound voice turn without a real model.

### 15.1.2 lmgw-side prerequisites

Built in lmgw's repository under lmgw's own designs and reviews; Kai bumps `LMGW_PIN` after each
lands. The paths are lmgw's.

**M1 = api WP7 (E): the host link.** Needed by WP8's live check and every hosted tool.
- Goal: api §5.1 to §5.6 and §9's host-link and reach tests, as specified there.
- Owns: a new `crates/lmgw-core/src/mcp/host.rs` with children (the WebSocket as an rmcp transport,
  the link table, takeover); the manager's `Device` arm in `mcp/mod.rs` and `mcp/connect.rs` (wait
  for an inbound link, lazy list, reap, offline resolution that never refuses a turn);
  `config/mcp.rs` (`Device`); `mcp/scope.rs` (L16, W3-9); the migration for
  `mcp_servers.device_key_id`; `ops/keys.rs` (the grant creates, renames and deletes the row); the
  route and its `CAPABILITY_TABLE` row; the `mcp.host_*` settings; lmgw-ui's Devices card reach list
  and the MCP page's device chip. In `lmgw-api-types` and `lmgw-client`: the `_meta` types, the
  host-link close and refusal codes, `requests::mcp_host`.
- Tests: api §9's host-link list with a fake device, and its reach list.

**M2 = api WP8 (G): resources and MCP Apps metadata.** Needed by WP12 only.
- Goal: api §7.2 to §7.4, starting with §7.4's re-read of the MCP Apps revision.
- Owns: `mcp/ingress.rs` (capabilities, `resources/*`), a new `mcp/resources.rs` (aggregate,
  namespacing, routing), the `visibility` filter where model runs list tools, the `tool` frame's
  fields in `web/agentchat.rs` and their relay in the bound session's `lmgw.chat.frame`.
- Tests: api §9's resources list.
- After M1: both touch `mcp/mod.rs`.

**M3 = api WP9 (F): approvals.** Needed by WP19.
- Goal: api §6 whole.
- Owns: `store/chat.rs` (`ThreadMcp.require_approval`), the `pending_approvals` migration, the
  gated turn in `web/agentchat.rs`, a new approvals route module, `realtime/thread/owned.rs` and
  `realtime/conversation/mcp.rs`, the `approval.*` feed records; in `lmgw-client`:
  `requests::approvals`, `requests::transcribe`, the `mcp_approval_request` items and
  `lmgw.approval.decided` as typed events, the feed's `approval.*` items.
- Tests: api §9's approvals list.
- After M2: both change the Chat's `tool` frames in `web/agentchat.rs`.

**M4 (new; needs an lmgw design first): MCP Tasks for hosted servers, and late results.** Needed by
WP18. The contract Kai builds against (K26), for lmgw's design to confirm or change:
1. One MCP revision pinned (K26: the 2026-07-28 spec's Tasks, if that revision is stable); the host
   link's `initialize` offers it, and lmgw's client role sends a task-augmented `tools/call` to a
   tool whose `execution.taskSupport` asks for one.
2. The turn's tool result is at once the text `started, job <task id>`; the turn goes on.
3. lmgw follows the task (status notifications, else `tasks/get` at an interval that is a visible
   setting) across turns and with no session bound. A link that drops ends the task with the
   abandoned wording (api §5.4).
4. The finished task's result enters the thread as a tool result the next turn renders (valid for
   strict templates), recorded in the feed as `job.done {thread_id, label, task_id, status}` and
   sent to a bound session as `lmgw.job.done`.
5. lmgw starts no response for it. A bound session's `response.create` with no new user item runs a
   continuation turn that answers it; the dashboard's next turn sees it. A task cancelled in lmgw
   (the thread deleted, the owner's cancel) sends `tasks/cancel` to the device.
- Owns: a new `mcp/tasks.rs`, its hooks in `mcp/host.rs` and `web/agentchat.rs`, a table of open
  tasks, the feed record; in `lmgw-client`: the feed and realtime items.
- Tests: a fake device that answers with a task, completed before and after its turn ended, with no
  session bound, a link drop, a cancel from each side, the bound continuation.

**M5: the shared UI crate (K29, §16 Q18).** Needed by WP21's second half only; until it lands, Kai
uses `app.css`'s tokens at the pin (K29). It is WP0 of lmgw's personality-profiles design (M6):
`crates/lmgw-ui-kit`.
- Goal: the tokens, the CSS and the generic widgets (confirm, form, modal, popover, section,
  split, sub-navigation, table, page frame, dirty guard, filter bar, clamp) move into a library
  crate that lmgw-ui consumes unchanged; the pickers that read lmgw's catalog stay. Its extent
  beyond that is Q18.
- Owns: the new crate, lmgw-ui's `widgets/` and the split of `app.css`.
- Tests: trunk builds lmgw-ui; `scripts/ui-matrix.py`'s screenshots unchanged.
- Disjoint from M1 to M4 (lmgw-ui only): any time.

**M6: personality profiles (ruling 29).** Needed by WP22. Designed in lmgw as
`docs/design/2026-10-09-personality-profiles-design.md` (approved by the owner, 2026-10-09). Kai
needs the profile list (`GET /chat/api/profiles`), a write of its profile that its device key may
make, and the `profile.*` feed events. `personality_set` needs no new lmgw route (D14, §3.5): the
client resolves the name against the list and writes the Kai folder's defaults with the existing
`POST /chat/api/folders/{id}` and `{defaults_patch: {profile_id}}`; `apply_to_current` (default
true) switches the current thread, which matters because Kai's thread rolls over.

**M7: a tool-calling mock.** Needed by the live checks of WP8 onward.
- Goal: `scripts/mock-openai.py` answers with scripted tool calls (one list per turn, in order, set
  by environment), then echoes the tool's result as its text, so a live check drives a hosted tool
  through a Chat turn and a bound voice turn with no model and no GPU.
- Owns: `scripts/mock-openai.py`.
- Tests: its own self-check of one scripted call through a dev lmgw's Chat turn.
- Small and disjoint: any time.

lmgw order: M1 and M7 first; M2, then M3; M4's design once M1 is merged; M5 and M6 alongside.

### 15.1.3 Plan decisions (PD1 to PD6 approved by the owner, 2026-10-09)

*Decided by the owner, 2026-10-09:* **PD1 to PD6 stand as written.**

- **PD1. Kai becomes a workspace.** The root package `kai` (the binary) plus `crates/kai-desktop`
  (K23: the contract, the capability traits, the tools' sync or async class; no desktop, I/O or
  tokio dependency), `crates/kai-delegate` (K25's client-neutral loop, I/O behind traits) and
  `crates/kai-browser-host` (phase 3). The configuration window's Leptos frontend is `ui/`, a wasm
  crate built by trunk and kept out of the native workspace build.
- **PD2. The hosted server is `src/host/`,** a task of its own like the feed. It holds the link to
  `/mcp/host` (the device key, the feed's TLS and backoff, K5's drop rules do not apply: it
  reconnects by itself, since it warms no model), rmcp's server role at lmgw's rmcp version over a
  WebSocket transport, the registry, K15's policy and the dispatch. The core starts and stops it
  exactly where it starts and stops `LmgwLink` (pairing, load, revocation, re-pairing, quit); a
  4003 goes the feed's revocation path.
- **PD3. Tools that act on the core** (conversation mode, a job's announce, the configuration
  operations, the personality) reach it as one `Command::Tool(CoreCall)` with a oneshot answer.
  Every other tool runs in its own module, off the core.
- **PD4. K15's confirmation, until F (WP19), is a GTK dialog like K13's:** the caller, the tool,
  the arguments in K22's compact lines, the time left from `lmgw/timeout_ms`; Deny is the
  default; `notifications/cancelled` closes it, and so does the timeout ("not confirmed in time").
  The overlay tools default to `allow` for the owner, the gateway and Kai's own key, as §10.1 reads
  ("untrusted callers are confirmed"); every other K15 default stands. From WP19 a thread call
  carrying `lmgw/approval` is decided by F, and the dialog stays for callers without F.
  *(WP19's second review, 2026-10-09, a draft:)* K24's
  confirmed-by-default tools included, as WP8 read it (K15's notes); `routine_enable` switching a
  routine on is the one call F does not decide.
- **PD5. The Kai folder's tool defaults are the owner's.** Kai never writes its label into them.
  While its link is Ready and the folder's defaults lack the label, the tooltip and one
  notification name the step, and Open in lmgw opens the folder's settings (§4's no-model pattern).
- **PD6. Every hosted tool's contract entry is checked by a test:** at most two plain parameters,
  no nested object, a one-line description, MCP annotations present, no desktop in the name (K22,
  K23). `just tool-cost` sends the full list through `/v1/count_tokens` for the Kai thread's
  aliases and prints the figure (K22: a figure, not a cap).

### 15.1.4 Kai's WPs

Numbered on from §15. "Without the owner" is what automated checks verify (dev lmgw, mocks, private
PipeWire, nested KWin, crops); "the owner" is what only the owner can check (§14's rules).

**Phase 2**

**WP8. The hosted MCP server and K15's policy.**
- Implements: ruling 17, K2, K15 with its notes, K22's list rules, K23's contract half, §10.1's
  first bullet, the device side of api §5.1 to §5.5; PD1 to PD6.
- Owns: `crates/kai-desktop/` (new: `contract/mod.rs` with `ToolSpec` and the list of areas);
  `src/host/` (new: link, transport, registry, policy, confirmation, settings, tests);
  `Cargo.toml` (the workspace, rmcp); `src/core/calls.rs` (new, PD3). Hook lines: `src/core.rs`
  (start and stop beside `LmgwLink`, `Command::Tool`, `mod calls`), `src/main.rs` (the task),
  `src/ui.rs` (`UiCmd::Confirm`), `src/tray/model.rs` (the tooltip's tools line: linked as
  `<label>`, not granted, offline), `src/config.rs` (the `[host]` and `[policy]` fields; their
  types live in `src/host/settings.rs`).
- Depends on: nothing to build; M1 and M7 for the live check. Until M1 it is tested against a fake
  host in its tests that plays lmgw's side of api §5.1 to §5.5.
- Tests: the fake host's script (`initialize`, listing, a call with `_meta`, `notifications/
  cancelled` before and during a call, a 4000 takeover, a 4003, an oversize close); the policy
  table (caller kind × annotations × per-tool state × trusted callers and approvers; `off` unlists
  and sends `list_changed`); a cancelled call never runs; the dialog closes on cancel and on
  timeout; the list is identical across reconnects; PD6's contract test; no key and no tool
  arguments in the log at any level.
- Live: `just live-check-host` against a dev lmgw with M1 and M7: the grant, the row `Ready` on the
  MCP page, `/mcp` listing `<label>__…` for the owner and not for a `deny` key, a mock turn calling
  a tool of the `live-test` feature only, the dev lmgw's restart and Kai's reconnect.
- The owner: the grant on the owner's lmgw at acceptance (WP25).
- *As built, 2026-10-09 (with WP16a's review fixes):* the link's size limits are
  lmgw's, announced in its `initialize` (`_meta["lmgw/host_limits"]`, or
  `capabilities.experimental["lmgw/host_limits"]`: `max_message_bytes`, `max_frame_bytes`,
  `ping_interval_s`, optional `call_timeout_ms`; 0 is "no bound of its own", as lmgw's settings
  say), asked of M1 and read in `src/host/wire.rs` until M1's types replace it. With an lmgw that
  announces none, `[host]`'s `fallback_max_message_mb` (64) and `fallback_max_frame_mb` (16),
  lmgw's defaults, apply, and a result that names the limit says it is the fallback. Kai writes a
  message larger than the frame limit as a text frame and continuation frames within it (cut at
  characters), so the message limit is the one that binds; a tool's result that would be a larger
  message is the call's error naming the limit, checked in one place on the host's result path for
  every tool, so it never closes the link (1009) and cancels every call in flight. A call carries
  its deadline (`lmgw/timeout_ms` from its arrival) and the room its result has (the message limit
  less the rest of its answer) to its handler; and a tool may resolve before the confirmation (a
  `Preparer`: the dialog shows what it resolved, a refusal of it asks nobody, the handler runs what
  was shown).
- *On M1 (LMGW_PIN at M1, 2026-10-09):* `src/host/wire.rs` is gone; the route, the refusals, the
  closes, `CallMeta` (`from_meta`), `CallerKind` and `HostLimits` are `lmgw_client::mcp_host`'s.
  lmgw announces the limits in `initialize`'s `_meta["lmgw/host_limits"]` only, every part
  optional: `max_message_bytes` `null` is no message bound (`mcp.host_max_message_mb` 0), the
  frames still within `max_frame_bytes`; a frame limit left out (or 0) keeps `[host]`'s
  fallback. `CallerKind::Anonymous`, an unknown kind and one Kai's match does not name yet stay
  K15's least trusted. `just live-check-host` checks the limits read from `initialize`, also the
  `null` after a restart with `mcp.host_max_message_mb` 0; it and `live-check-link` run on a
  private bus with their own Secret Service (`scripts/private-bus.sh`), so the owner's Kai may run.

**WP9. Conversation mode** (ruling 23, K27; §10.1's first tool through the link).
- Owns: `src/voice/conversation_mode.rs` (new), `crates/kai-desktop/src/contract/conversation.rs`,
  `src/voice/tests/conversation_mode.rs`. Hook lines: `src/voice/mod.rs` (`Voice::standby` reads the
  override; a session's end clears it), `src/voice/control.rs` (the talk key and the tray end it),
  `src/core/calls.rs` (one arm), `src/config.rs` (`voice.conversation_timeout_s`).
- Depends on: WP8.
- Tests (paused time): it takes effect after the current reply; continuous listening until the
  timeout, whose setting the result names; muted time counts toward it (ruling 32, was: mute pauses
  it); the talk key, the tray and
  `conversation_end` end it; K16 still holds back; the session's end restores the configured mode;
  every other caller denied, and allowed once the owner widens the level (ruling 28).
- Live: `just live-check-voice-lmgw` gains a step: in push-to-talk, M7's model calls
  `<label>__conversation_start`; after the reply Kai listens continuously and the ring stays up.
- The owner: saying it to a real model in German and English.

**WP10. Widget overlays: the surface, the sandbox, text and HTML** (ruling 6, ruling 24, K1's
widget process, K18's no-bridge half, §10.1).
- Owns: `src/widgets/` (new: the GTK-side host with one shared web process of related views, one
  layer-shell surface per widget, the flat `position` and the configured defaults, interactive
  input regions re-applied on resize, timeouts; `kai-sandbox://` with §10.1's CSP and load
  refusals; the tools `overlay_text`, `overlay_html`, `overlay_hide`, `overlay_list`),
  `assets/widgets/`, `crates/kai-desktop/src/contract/overlay.rs`. Hook lines: `src/ui.rs`
  (`UiCmd::Widgets`), `src/tray/model.rs` and `src/tray/mod.rs` (the Overlays menu's list and Hide
  all), `src/shortcuts.rs` (`hide-overlays`), `src/config.rs` (`[overlay]`'s per-kind defaults).
- Depends on: WP8.
- Tests: the pure lifecycle (show, hide by id and all, expiry, `overlay_list`'s lines, no update);
  the sandbox refuses `http:`, `file:` and `kai:` loads and runs an inline script; a widget's
  web-process crash leaves the ring's; no web process once the last widget is hidden.
- Live: `just live-check-widgets`: crops of each kind, both outputs and every `position` in a
  nested KWin, PSS against K1's figures (first widget, a second one).
- The owner: a real click on an interactive widget, the rest clicking through.
- *Landed 2026-10-09 (`feat/widgets`):* as §10.1's WP10 note says. `just live-check-widgets`
  (nested KWin, two outputs) passes: the sandbox (an inline script and inline style run, `data:`
  and `blob:` images load; `http:`, `file:` and `kai:` images, fetch, a WebSocket, a popup and a
  navigation to the loopback server or to another widget's document are refused, the loopback
  server saw no connection; the frame's origin is opaque, the host page out of its reach, no
  message handler in it or in the host page's main world), every position on both outputs where
  KWin shows them, a fitted size, expiry, an output going away, no WebKit process 0.5 s after
  the last widget, the ring's processes untouched by the widgets' crash, no widget content in a
  trace log. PSS in the nested, software-rendered KWin: the first widget +148 MB in all (web
  process 146 MB, network process 16 MB, the UI process's heap), a second one +51 MB in all
  (web process +9 MB); the UI process's figure moves by tens of MB between samples, so the web
  process's is the one to compare with K1's.
- *Decided by the owner, 2026-10-09:* the defaults are kept as built (the positions and sizes
  per kind, shown until hidden, only MCP Apps views interactive). The view over Kai's dialogs is
  fixed as §10.1's note says (the view shrinks to its strip, away from the dialog).
- *For WP11 and WP12:* a kind is a `Kind` variant (`src/widgets/kinds.rs`) with an async
  `body` (WP11's fetch goes there), its `[overlay.<kind>]` table, its contract entry before
  `overlay_hide`; the provider and the PD6 test pick it up. WP12's bridge: a `Body` variant whose
  frame the host script (Kai's isolated world) answers by `MessageEvent.source`, relaying through
  the handler only that world has; that an isolated world's `message` listener sees the frame's
  `postMessage` is not verified yet (WP12's first check).

**WP11. Images and emotes** (K19, §10.1's `overlay_image` and `overlay_emote`).
- Owns: `src/widgets/fetch.rs`, `src/widgets/image.rs`, `assets/widgets/emote/`; the two entries in
  `contract/overlay.rs` (after WP10 merged).
- Depends on: WP10.
- Tests, against a loopback HTTP server: `overlay.image_max_mb` named in the error, status and size
  errors with no body, private addresses refused with the setting off and the refusal logged,
  `data:` sources, a body that does not decode as an image refused before WebKit sees it.
- The owner: the emote set's look.
- `overlay_weather {place}` is built here (ruling 30, §16 Q19), in `src/widgets/weather.rs`.
  *Taken in this draft:*
  - `place` stays one plain parameter: a saved place name from Kai's configuration
    (`[weather] places`, with a default `home`) or `latitude,longitude` (models know city
    coordinates well). Bright Sky takes coordinates and has no place-name search; a geocoding
    service would be a new external service, the owner's call (Later).
  - Data: `/current_weather` for now, `/weather` for the day's forecast. The widget shows the
    source attribution Bright Sky asks for. No caching rule beyond what the HTTP response says.
  - `api.brightsky.dev` is the only allowed weather origin (K19).
  - Tests: a loopback server standing in for the API; a bad `place` named in the error.

- *Landed 2026-10-09 (`feat/overlay-kinds`), taken where the spec was silent:*
  - **One fetch** (`src/widgets/fetch.rs`) for images and Bright Sky: http and https only, 2xx only,
    the body streamed against `overlay.image_max_mb` (default 32; a `Content-Length` over it is
    refused before a byte is read), a deadline `overlay.fetch_timeout_s` (default 15), errors with
    the status or size and never the body or the URL's path and query.
  - **Redirects**: followed within one origin only (same host and port; `http` to `https` of the
    default ports counts), at most 5; any other target is refused and named by its origin so the
    caller can use that address itself.
  - **Private addresses** (`overlay.fetch_private_addresses`, on): with it off a name is resolved
    first, every address checked, and the connection pinned to what was checked; the refusal names
    the key and is logged (host only). The check covers loopback, link-local, private, unspecified,
    `0.0.0.0/8`, shared `100.64.0.0/10`, `198.18.0.0/15`, `240.0.0.0/4` (with broadcast), multicast,
    IPv6 unique-local, site-local `fec0::/10`, and the forms that carry an IPv4 address (mapped,
    NAT64 `64:ff9b::/96`, 6to4 `2002::/16`), judged by the address they carry. The client ignores
    `HTTP(S)_PROXY` and `ALL_PROXY` (`no_proxy()`), which would send the request elsewhere than the
    checked address.
  - **One deadline**: `overlay.fetch_timeout_s` bounds the whole fetch (lookups, every hop, the body),
    not each hop; the error names the setting.
  - **Pictures** (`src/widgets/image.rs`): the `Content-Type` must be `image/*` (not SVG); the bytes
    are decoded as png, jpeg, gif or webp and written out as a fresh PNG, so metadata and a
    polyglot's second format are gone; a gif shows its first frame. `overlay.image_max_mb` also
    bounds the decode's pixel memory (RGBA), so one number covers download and unpack and the error
    names it. `data:` sources must be base64 and take the same path. The picture reaches the frame
    as a `data:` image inside a document (`Body::Document`), contained in the widget, so the
    sandbox needed no change.
  - **Emotes** (`assets/widgets/emote/`): smile, laugh, wink, sad, surprised, angry, heart, check,
    question; plain SVG in graphite `#31363b`, Breeze blue `#3daee9` and text `#eff0f1` (a test
    checks the palette and that no SVG holds script, link or style). The names are in the tool's
    description and a test keeps the two lists equal. The look is the owner's to judge.
  - **Weather** (`src/widgets/weather.rs`): `place` is optional (default `home`), a saved name
    (any case) or `latitude,longitude`; `[weather] places = { home = "52.52,13.405" }`, each value
    checked at load. `/current_weather` first, then `/weather?date=` with the date of the
    observation's own timestamp (so Bright Sky's time zone decides the day); the card shows
    temperature, sky, the day's low and high, the day's rain, wind and humidity, and the
    attribution "Source: Deutscher Wetterdienst (DWD), via Bright Sky." with the station's name.
    A failure of either request fails the call. The origin is the constant `BRIGHT_SKY`; only
    tests pass another (a loopback stand-in).
  - **Configuration**: `[overlay.image]` (bottom-right, 480x320), `[overlay.emote]` (bottom-right,
    160x160), `[overlay.weather]` (top-right, 300x230), each with the same keys as `[overlay.text]`
    and a required height; `openWorldHint` is true for `overlay_image` and `overlay_weather` only.
  - Tests: loopback stand-ins only (the fetch rules, every refusal above, a body that is not a
    picture, a small file that unpacks huge, `data:` sources, the weather's data, escaping and a bad
    `place`); `just live-check-widgets` section `kinds` shows the three in the nested KWin, the
    image's crop blue throughout.

**WP12. The MCP Apps host: `overlay_app`** (K18, §10.1's bridge, §10.3 and Q4 as answered, api §7).
- Owns: `src/widgets/app_bridge.rs`, `assets/widgets/app-host.js`, `src/lmgw_link/mcp.rs` (new: the
  bridge's `tools/call` and `resources/read` to lmgw's `/mcp` under Kai's key); its contract entry.
- Depends on: WP10, M2 (and M2's pinned MCP Apps revision).
- Tests: the bridge answers only frames it placed (`MessageEvent.source`); a view calls only its
  server's tools; a call not marked `readOnlyHint` goes through K15; requested CSP domains and
  permissions are refused and named in `overlay_list`'s notes.
- Live: a small MCP Apps fixture server (in Kai's `scripts/`) registered on the dev lmgw; its view
  shown, its data loaded through the bridge.
- The owner: an interactive view's click.
- *As built, 2026-10-09 (`feat/overlay-app`; taken where the spec was silent):*
  - **The postMessage path, verified first** (WP10's open point): in the nested KWin a view's
    `parent.postMessage` reaches the `message` listener of Kai's host script in its isolated
    world; `MessageEvent.source` there is identical to the placed frame's `contentWindow` read in
    the same world, `MessageEvent.data` reads there, and `contentWindow.postMessage` from that
    world reaches the view (whose `MessageEvent.source` is its parent). A message posted by the
    host page's main world, and an `overlay_html` document's message to its parent, reach no
    bridge (`just live-check-widgets`, section `apps`).
  - **Parameters:** `overlay_app {server, uri}` has no `position` (K22's two parameters, PD6's
    test); its place is `[overlay.app]`'s (`center`, 480×360, click-through like every kind by
    §10.1: `interactive = true` for clicks; *changed below: interactive by default*). `server` is the label lmgw lists the server's tools
    under (its tool prefix; the `tool` frame's `server_label`); `uri` the namespaced
    `ui://<server>__…` URI as `/mcp` serves it (a tool's `_meta.ui.resourceUri` there, the frame's
    `ui_resource`). A server without a tool prefix is refused, naming why: `/mcp` lists its tools
    bare, so nothing tells them from another server's.
  - **The document:** `resources/read` on `/mcp` under Kai's device key (`src/lmgw_link/mcp.rs`),
    in a session of the view's own whose `initialize` declares `io.modelcontextprotocol/ui` (else
    `/mcp` lists no app-only tool), ended with `DELETE /mcp` when the view goes; a session lmgw
    forgot (its restart) is opened again once. The read comes before `tools/list` (a read connects
    a server lmgw had not, api §7.5). The content must be `text/html` (any parameters; `text` or a
    base64 `blob`) and is shown exactly as `overlay_html`'s document, same sandbox. Its
    `_meta.ui.csp` lists and `permissions` are refused, logged and named one line each in the
    show's answer and `overlay_list` (`refused: csp connectDomains https://…`); `domain` and
    `prefersBorder` are ignored.
  - **What a view may do** (`src/widgets/app_bridge.rs`): `ui/initialize` (revision `2026-01-26`;
    host capabilities `serverTools`, `serverResources`, `logging`, no `openLinks`, no `sandbox`
    grants; context: dark theme with Breeze's colours, `inline` only, the configured size,
    desktop, hover without touch), `ping`, `ui/request-display-mode` (answered `inline`), and
    proxied to `/mcp`: `tools/call` by the server's own tool name, sent as `<server>__<name>`,
    only for a tool `/mcp` listed to Kai at the show whose `_meta.ui.visibility` includes `"app"`
    (absent: both; a tool added later needs a new show; a renamed tool's view name does not
    match); `resources/read` with the URI put into the server's namespace (lmgw's rule, so no other
    server's URI can be named; a URI without an authority is refused), its `contents[].uri`
    answered as the view asked. A call not marked `readOnlyHint` opens K15's dialog first ("Let
    the view of app '<server>' run <server>__<name>?", MCP's `destructiveHint` default for the
    warning), with no deadline of its own (lmgw's row `timeout_ms` bounds the call); hiding the
    view closes it as a Deny. No setting skips it (K18 as written; a `policy.trusted_callers`
    entry `app:<server>` would be the owner's call). *Changed below (owner decisions,
    2026-10-09).* Every other request is `-32601`; Kai's
    refusals are `-32602`, the owner's no `-32000`; lmgw's errors pass on with their code.
    Notifications change nothing (the size is the configuration's, §10.1). Kai sends the view
    nothing unasked (`overlay_app` carries no tool input or result) but `ui/resource-teardown`
    (*changed by the review below*; was: none, a deliberate departure).
  - **Bounds:** `overlay.app_max_mb` (default 64, lmgw's own `mcp.host_max_message_mb` default)
    bounds every answer from `/mcp` (the document included) and every message of a view, named in
    the error; connecting is bounded by `lmgw.timeout_s`; no other deadline of Kai's (the show's
    own call deadline bounds the open).
  - **Answered only where placed:** `assets/widgets/app-host.js` is injected only into an
    `overlay_app` widget's page, after `host.js`, in Kai's world; it relays a message only when
    `MessageEvent.source` is the frame it placed, and the GTK side hands a widget's messages only
    to that widget's bridge. The log names the server, the tool and the decision, never
    arguments, results or a view's message (`PageMsg::Rpc`'s `Debug` is its size).
  - **Live** (`just live-check-widgets`, section `apps`, about 10 s): the proof above against a
    scripted `/mcp`; then a dev lmgw on 127.0.0.1:8897 (the checkout's headless example) with
    `scripts/mcp-apps-fixture.py` registered under `fx` and a device key: the view read, its
    data loaded through an app-only read-only tool and shown (crop), its state-changing call
    asked of a scripted owner who says no, a model-only tool, another server's tool, a URI without
    an authority and `ui/open-link` refused, its own resource read, the refused origin and
    permission named in the answer and `overlay_list`, nothing of it in the trace log.
  - *Review fixes, 2026-10-09 (an Opus review of `feat/overlay-app`; taken in this draft):*
    - **One dialog per view, a bound on requests in flight.** A view could open any number of
      K15 dialogs and calls. Now a call not marked `readOnlyHint` while that view's dialog is
      open is answered `-32000` ("a confirmation is already open …"), nobody asked; and a view
      has at most `overlay.app_max_calls` requests in flight (new key in `[overlay]` beside
      `app_max_mb`, not in `[overlay.app]`, whose table is the kinds' common placement keys;
      default 16, at least 1), each counted from its message until its answer is handed to the
      GTK side, so answers waiting for a view are bounded too (16 × `app_max_mb`). One more is
      answered `-32000` naming the key; a notification over it is dropped.
    - **A view's message is bounded before it is read:** Kai's app script gets
      `overlay.app_max_mb` with the view and answers a larger request `-32600` itself, naming
      the key, without posting it; it posts a message as `{"ev":"rpc","m":<its JSON>}` (no second
      escaping), so the GTK side refuses a larger envelope before parsing anything, and the
      bridge reads none either (a backstop: logged, unanswered).
    - **Overlapping prefixes** (lmgw allows `fx`, `fx_` and `fx__x` side by side): `fx` +
      `__` + `x__y` is also `fx__x`'s `y`, and `fx` + `__` + `_y` is `fx_`'s `_y`. So a tool's own
      name, and a URI's own authority (up to `/`, `?`, `#`), that starts with `_` or holds `__`
      is refused to the view and left out of its rules and lists, and `overlay_app`'s `uri` too.
      Kai's interim rule: a server whose own tool is so named is out of its view's reach. The
      real fix is lmgw's (refusing overlapping prefixes, or a separator a prefix cannot hold),
      **the owner's call**.
    - **lmgw's URI rules exactly:** a scheme starts with a letter (lmgw's `uri.rs` split); a URI
      with an empty authority is namespaced as lmgw does (`file:///x` → `file://fx__/x`), no
      longer refused; `lmgw`, `docs` and `kb` (lmgw's own labels, and any `<label>__…`) are
      refused as `server`.
    - **`resources/list` and `resources/templates/list`** (the spec's `serverResources` was
      advertised while both answered `-32601`): proxied, the spec-faithful choice (the
      extension's own host bridge proxies both under that capability). lmgw's list is cut to the
      view's server's entries by the rules above, each URI (template) in the server's own terms;
      a cursor passes on (lmgw answers in one page and refuses one).
    - **`ui/resource-teardown`**, replacing the departure above: the revision says the host MUST
      send it before tearing a view down, and SHOULD wait for the answer. Kai sends it (id
      `kai-teardown`, `reason`: `hidden`, `its time is up`, `Kai quits`, or the failure's words)
      and destroys the surface two turns of the page's event loop later (the view takes it in
      the first; an answer it gives at once is relayed in the second, and the live check sees
      it), or after `overlay.unresponsive_s` for a page that runs no script; it does not wait
      for the answer (a hide stays immediate, nothing lingers unseen). The bridge and its `/mcp`
      session end at once, as before; the answer is relayed to nobody.
    - **`/mcp` sessions:** requests that meet lmgw's 404 together open one new session (the
      first re-opens, under a lock, only if the session is still the forgotten one; the others
      take the new id); a session dropped without its close (an `overlay_app` cancelled by its
      deadline after lmgw gave it a session) sends its `DELETE` from its drop.
    - **`tool-input` and `tool-result` stay unsent** (the owner's decision, §10.1 and K22:
      `overlay_app` has no tool call to relay; the view loads its data through the bridge). A view
      written for a host that sends them waits for them in vain; that is the documented cost.
      *Still so for `overlay_app`; a call in Kai's conversation now sends both (below).*
    - `just live-check-link` also asserts that the host link's own 401 count stays the same
      through each 6 s wait (a host link that kept trying a refused key went unseen).
    - Tests: the prefix overlap (target, tools, reads, lists), the scheme's first character, one
      dialog and the in-flight bound (a flood), the oversized message (the app script in the
      live check; the GTK envelope and the bridge in unit tests), concurrent re-opens, a
      cancelled open, the teardown (its answer seen in the live check, for the proof and the
      fixture view), the lists in the fixture.
  - *Owner decisions, 2026-10-09 (`feat/apps-owner-decisions`):*
    - **A trusted app's view skips the dialog** (*decided by the owner, 2026-10-09*). A view's
      calls run as the caller `app:<server>` (`app_bridge::caller`; the dialog still reads "the
      view of app '<server>'"), a new kind of `policy.trusted_callers` entry (`app:fx`; the
      Settings list edits the key, and a bad entry's error names the kinds). The decision
      (`app_bridge::decide`) is K15's for any caller, applied to the server's tool by its exposed
      name: `policy.tools.<server>__<tool>` first (`deny` and `off` refuse with `-32000` naming
      the key, `allow` runs, `confirm` asks even a trusted app), then a read-only tool runs, a
      state-changing one runs for a trusted app and asks for every other. Default: no app is
      trusted, so the dialog is as before. The Tools page lists Kai's own tools only, so a
      server's tool is set in the file or the Settings list.
      - *Changed after review, 2026-10-09 (taken in this draft):* trust no longer
        covers destructive tools. A trusted app's state-changing call skips the dialog only when
        the tool says `destructiveHint: false`; an absent `destructiveHint` on a tool that is not
        read-only counts as destructive (MCP's default) and asks, unless
        `policy.tools.<server>__<tool> = "allow"`. No new key: `app:` stays in
        `policy.trusted_callers`, and `policy.trusted_approvers` refuses it (an app's views
        approve nothing). Trust entries are trimmed (`" app: fx "` is `app:fx`).
      - *Changed after review:* only `policy.tools` names holding `__` are exempt from the
        host's unknown-name note; they are listed at the start as "an MCP app's tool, checked when
        its server's view opens", and when a view of `<server>` opens, each
        `policy.tools.<server>__…` its `/mcp` list lacks is named in the log and the view's
        `overlay_list` notes ("the setting does nothing"). A `policy.levels.<server>__…` name is
        reported at the start like any unknown tool: no level applies to an app's tools.
      - *Changed after review:* `overlay_app` and a call's view refuse Kai's own label
        (`hello.hosts_label`) as they refuse lmgw's.
    - **Interactive by default** (*decided by the owner, 2026-10-09*): `[overlay.app]
      interactive = true`. MCP Apps carries no flag for it (SEP-1865 `2026-01-26`: a resource's
      `_meta.ui` has `csp`, `permissions`, `domain`, `prefersBorder`; a tool's `_meta.ui` has
      `resourceUri`, `visibility`; display modes `inline`, `fullscreen`, `pip` are placement, and
      `appCapabilities.availableDisplayModes` lists those), so there is nothing to honour over
      the default; the configuration's `interactive = false` still makes views click-through.
    - **A call's view shows by itself, with its input and result** (*decided by the owner,
      2026-10-09*; `src/widgets/thread_apps.rs`). Kai's turns run in its bound session, whose
      `lmgw.chat.frame` relays the Chat turn's frames (`crate::voice` hands each to
      `thread_apps::frame`, and its end to `thread_apps::session_ended`: one line each). *Taken
      in this draft:*
      - `[overlay.app] auto_show = true` (a key of `[overlay.app]` only; false: views come only
        from `overlay_app`).
      - At the call's `ready` frame (name and arguments) Kai looks the tool up in `/mcp`'s tool
        list under its key (a session of the view's own, kept for the view) and, when its
        `_meta.ui.resourceUri` (or the deprecated flat key) names a view, shows it at once,
        while the tool runs: the spec's lifecycle renders the view beside the call, which is
        what makes `tool-cancelled` mean something. A tool of Kai's own contract is not looked
        up; lmgw's own labels and unlabelled tools are never followed. When the list names no
        view but the `result` frame names one (`server_label`, `ui_resource`), it is shown then,
        input and result at once. Same rules as `overlay_app` (a prefixed server only, the
        namespace check, K18, K15 as `app:<server>`), same place (`[overlay.app]`).
      - *Changed after review, 2026-10-09:* **a call that may wait for an approval gets no input
        before it runs.** lmgw sends `ready` for every call, gated or not, then for a turn that
        stops on gated calls an `approval` frame per gated call and `done` with
        `pending_approvals`; the ungated calls beside them wait too. So the view's `tool-input`
        goes only once Kai knows the call runs: at a `ready` that says `needs_approval: false`
        (lmgw's follow-up, tolerated and used when present), else at its `result` frame (input
        and result together); a `ready` that says `needs_approval: true` shows nothing yet. An
        `approval` frame is matched to its call by response, tool (`<server_label>__<name>`) and
        arguments (a JSON string there); `done` with `pending_approvals` holds the turn's open
        calls, uncancelled. `lmgw.approval.decided` (another client decided; its request runs
        the resumed turn, which Kai's session does not see) ends a held call with
        `tool-cancelled` alone: "the call was declined by <by>", "… never runs (its reply was
        edited or deleted)", or "the call was approved by <by>; it runs in that client's turn,
        whose result Kai does not see"; the turn's other held calls end once its last gated
        call is decided. A resumed turn that does reach Kai's session (a later Kai that answers
        approvals itself) finds the held call by tool and arguments at its `ready`, or by tool
        at a `result` whose `ready` it did not see when one held call has it: the view shown
        stays the one, no second, empty view.
      - *Changed with lmgw's richer frames (2026-10-09, pin `3fe2aff`; client-apps §7.6, typed
        as `lmgw_client::mcp_apps::{ToolReadyFrame, ToolResultFrame}`):* the `ready` frame now
        carries `call_id`, `server_label`, `needs_approval` and `ui_resource`, the `result` frame
        `call_id`, the MCP `content` blocks as the server sent them and `structured_content`.
        - **The view at `ready`, without the list.** A `ready` that names its view
          (`server_label`, `ui_resource`) and says `needs_approval: false` has it read and shown
          at once, with its input at once; `/mcp` is asked only for the view's own bridge rules
          (its server's tools), no longer to learn whether the tool has a view. A `ready` that
          says `needs_approval` and names no view shows nothing and asks no list (lmgw names
          every view there is); the result frame can still name one (step 3). The view's server
          is the frame's `server_label`, so a tool a name collision moved (`<server name>__…`)
          shows its server's view. A `ready` without `needs_approval` (an older lmgw) still looks
          the tool up in the list as before; a Kai contract tool is not shown at `ready` either
          way.
        - **Calls by `call_id`.** A call is its `(response, index)` within a turn; across
          responses (a held call met again in a resumed turn's `ready`, a `result` whose `ready`
          was not seen) it is found by its `call_id`, the tool and arguments only when a frame has
          none (an older lmgw). `approval` frames and `pending_approvals` carry the id too since
          lmgw d3c7fe14 (below). The id is kept
          and sent whole (Gemini's signed ids run to hundreds of characters; the tests use one of
          over 600): it is `hostContext.toolInfo.id` in the view's `ui/initialize` answer. Kai
          stores no tool call id anywhere else but its delegate's own conversation, which keeps
          lmgw's ids verbatim too (`src/delegate/model.rs`).
        - **The real result.** `tool-result` is MCP's `CallToolResult` from the frame:
          `content` the blocks as sent (images, resource links, annotations), `structuredContent`,
          `isError` (always said now); from an older lmgw `output` as one text block as before.
          No bound of Kai's own: the realtime WebSocket has no message limit, and a view's
          messages keep `overlay.app_max_mb`.
      - *Changed with lmgw d3c7fe14 (2026-10-09; pin `d3c7fe14`; client-apps §6.6 and §7.6):*
        approvals carry the call's id, and `/mcp`'s tool list says whose each tool is. This closes
        the two gaps the section above noted.
        - **Approvals by `call_id`.** `ApprovalRequest::call_id` (additive, absent from an older
          lmgw) is on the `tool {event: "approval"}` frame, on `done`'s and a message's
          `pending_approvals`, and on the feed's `approval.requested` and `approval.decided`. A
          bound session's `mcp_approval_request` item stays OpenAI's shape and has none; the
          `lmgw.chat.frame` relaying the approval frame right after it carries `call_id` beside
          the same `approval_request_id`. The flow marks a held call with its approval id by
          `call_id` when the request has one and a call of the response has it (the same tool
          with the same arguments twice is told apart; the frame and `done` mark the same call,
          not the next open one of the tool), else by tool and arguments as before. A decision
          names only the approval id; when the frame was not matched, the card's call id finds
          the held call.
        - **The approval card** (`src/approvals/`) keeps `approval_request_id` as the key of
          everything (the answer, the dedup, the verdict in flight). Its call also keeps the
          `call_id`: a request without one learns it from the relayed frame
          (`Approvals::call_id_seen`) or from the feed's announcement, the first id stays, and
          `Approvals::call_id_of` hands it to the voice, which passes it to the MCP App views on a
          decision (`thread_apps::approval_decided`). The card shows no id.
        - **A view's tools from the stamp.** `/mcp`'s `tools/list` carries
          `_meta["lmgw/server"] = {label, name, tool}` on every registered server's tool
          (`lmgw_api_types::mcp_apps::ToolServer`). When any tool of the list has it, the bridge's
          rules (`Rules::of_list`) are the stamped tools whose `label` is the view's server (the
          label `overlay_app` and the frames name; when no stamp has that label, whose `name`
          is): a view calls its own name for the tool (`tool`), and the bridge sends the name
          `/mcp` lists it under, so a tool a collision moved to `<server name>__x` (or the owner
          renamed) is the view's own and another server's `<label>__x` is not. An own name the
          overlapping-prefix rule would refuse (`z__w`) is fine here: the stamp is exact. A name
          two stamped tools of the label share is refused as ambiguous. Servers that share a tool
          prefix share the label, so a view opened for a call is narrowed to the server of that
          call's tool (`Rules::narrow_to_call`, by the stamp's `name`); `overlay_app` has no call
          and an ambiguous name stays refused. `hostContext.toolInfo.tool` is built from the
          stamp's own name (a moved tool has one now), and a view looked up by the list (an older
          lmgw's `ready`) takes its server from the stamp's label. A list without stamps (an
          older lmgw) is read as before: the tools named `<server>__…`, none moved.
        - **`policy.tools` for a moved tool** keys on the exposed name lmgw lists it under
          (`policy.tools.beta__x`). lmgw itself carries a tool's switches and deny lists across its
          names; Kai's `policy.tools` does not carry anything on its own, so the bridge also reads
          the `<label>__<tool>` spelling (`fx__x`, the name the tool answered to before) and the
          **strictest of the two wins** (`allow` < `allow_trusted` < `confirm` < `deny` < `off`),
          with one limit: only the tightening states (`confirm`, `deny`, `off`) carry over from the
          old spelling. After a collision `fx__x` names another server's tool (the one that kept
          it), so an `allow` written for it must never loosen this tool; it is ignored, and the
          view's notes and the log say so ("write policy.tools.beta__x"). The unmatched-name note
          does not flag an entry that is a moved tool's old spelling.
      - *Changed after review:* each view looked for gets a sequence number; a view whose call
        is older than the one shown is hidden at once (shows finish out of order), and one looked
        for in a voice session that has ended is not shown. `/mcp`'s tool list is kept as last
        fetched (per pairing: each tool's name and whether it names a UI resource), so a call
        whose tool has no view opens no `/mcp` session; a tool it lacks is looked up afresh, any
        fetch replaces it, and the voice session's end forgets it (lmgw's `list_changed` never
        reaches Kai: its `/mcp` sessions only POST). (A `result` frame whose `server_label` was not
        its tool's label was logged at debug; since lmgw's collision prefixes that is a moved
        tool, and the frame's label is the view's server.)
      - Once the view says `ui/notifications/initialized` (the spec's "View-ready signal"), it
        gets `ui/notifications/tool-input {arguments}`, then `…/tool-result` with MCP's
        `CallToolResult` from the frame (`content` the blocks as sent, `structuredContent`,
        `isError`; above) or `…/tool-cancelled {reason}`: "the turn was stopped" (a `stop` frame
        before `done`), "the turn was aborted", "the turn ended without the call's result",
        "Kai's voice session ended". Its `ui/initialize` answer names the tool in
        `hostContext.toolInfo.tool` in the server's own terms (own name, own resource URI) and
        the call in `toolInfo.id` (the frames' `call_id`, when lmgw sent one).
      - One such view at a time: a new one replaces the one shown this way before it (both sit
        at `[overlay.app]`'s place), with `ui/resource-teardown` first; `overlay_app`'s views are
        never replaced. `overlay_list` says "shown for <tool> in Kai's conversation". Nothing of
        the arguments or result is logged.
      - `overlay_app {server, uri}` stays for explicit opening and sends no `tool-input`,
        `tool-result` or `tool-cancelled` (it has no call; module docs of `app_bridge` and
        `thread_apps`).
      - **What lmgw does not give Kai** (not worked around): turns of other principals in Kai's
        thread (the phone, the dashboard) are not relayed to Kai's session (lmgw's "live
        mirroring of another client's turn is Later", client-apps §2.5), so their calls show
        nothing on the desktop. *The frames' fields asked of lmgw (`needs_approval`, `server_label`,
        the UI resource and the call id on `ready`; the content blocks as sent and the call id on
        `result`) were built by lmgw on 2026-10-09 and are used above.*
    - **Full-output shots** (`just live-check-widgets`): besides the crops, `shots/full-<name>.png`
      (both nested outputs) and `shots/full-<name>-<output>.png`, so
      `full-kinds-Virtual-0.png` shows all nine positions and `full-app-*.png`,
      `full-app-call-*.png` the app views where they sit.
    - Tests: `decide` (default, trusted, another kind or app of the same name, each
      `policy.tools` state, no unknown-name note), a trusted app's call without a dialog and a
      denied one never reaching lmgw, `tool-input` only after `initialized` then the result or
      the cancel (also when nobody follows the call), nothing unasked for `overlay_app`,
      `open_for_call` against a loopback `/mcp` (the list before the read, the flat key, a tool
      without a view closing its session, the result's names read first, the namespace), the
      flow's bookkeeping by response and index (stop, abort, session end, lmgw's labels, Kai's
      own tools, auto_show off, a result without its ready), `auto_show` only in `[overlay.app]`;
      live (section `apps`, 5f): the fixture's SDK-style view (`ui://fixture/live`, shows nothing
      until the host sends the call) shown at the `ready` frame, `tool-input` then
      `tool-result`, `toolInfo` in its own terms, its `card_bump` run without a dialog under
      `app:fx`, a second call replacing it (teardown answered) and, stopped, getting
      `tool-cancelled`, a tool with no view showing nothing.
    - Tests after review: `decide` asks a trusted app for a destructive tool and `allow` lets it
      run; the bridge's input only once the call runs, the cancel alone for a call that never ran;
      the flow's held calls (an approval matched by arguments, `done` with `pending_approvals`, a
      decision elsewhere declining or approving, a resumed turn finding its call, a result
      without its `ready`, `needs_approval` either way); the newest view and the session
      generation; Kai's own label refused; the kept tool list (no session for a tool without a
      view, a tool it lacks, another pairing, forgotten); unknown policy names only outside
      `policy.tools`'s `__` names, unmatched ones named at the view's open; `app:` refused in
      `trusted_approvers`, trimmed entries. Live (5f): the fixture's destructive `card_reset`
      asks the owner through the real K15 dialog while `card_bump` (`destructiveHint: false`)
      runs under `app:fx`; the view is click-through and gives way to Kai's note while the dialog
      is open (`shots/full-app-call-dialog-*.png`) and takes clicks after; no input before the
      result; `needs_approval: false` sends it at once; a gated call's view gets no input while
      it waits and, declined by another client, `tool-cancelled` alone; a resumed turn's frames
      open no second view and bring the input with the result; `needs_approval: true` shows
      nothing. The `WAYLAND_DEBUG` run adds an interactive app view whose input region is its
      rectangle, none while a K15 dialog is open, its rectangle again after.
    - Tests for the richer frames: a `ready` naming its view opens it without the list and
      sends the input at once, one naming none asks no list, Kai's own tool not shown at
      `ready`, a moved tool's view under the frame's label; held calls found by a 600-character
      call id (the same tool twice, arguments re-serialised, a result without its `ready`);
      `CallToolResult` from the blocks as sent (an image, a resource link, annotations, only
      structured content) and from an older lmgw's text; the bridge's `toolInfo.id` whole and the
      blocks passed unchanged. Live (5f): an older lmgw's frames still go through the list
      (the first call); a `ready` naming no view shows nothing; the second call's view is opened
      from its `ready`, input at once; a last call with a long id and a result with a text and
      an image block: the view saw `["text", "image"]`, its `structuredContent`, `isError`, and
      `toolInfo.id` whole.
    - Tests for lmgw d3c7fe14 (`app_bridge/stamped_tests.rs`, `thread_apps/call_id_tests.rs`, the
      card's in `approvals/tests.rs`): a stamped list gives the label's tools by their own names
      (a renamed tool, an own name with `__`, a name by the server's name), two servers sharing a
      label are told apart by the call (`narrow_to_call`) and refused as ambiguous without it, a
      list without stamps reads as before; through the bridge a view's `x` reaches `beta__x` and
      never alpha's `fx__x`; `decide` reads both spellings, the strictest wins, an old `allow`
      does not loosen; an approval finds its call by id when the tool and arguments fit two
      calls, the frame and `done` mark the same call, an older gateway's request (and one whose
      id no call has) falls back to tool and arguments, a decision finds a held call by the
      card's id; the card learns an id from the relayed frame and the feed, keyed as before; the
      feed passes `call_id` on. *Not live:* the dev lmgw rig can register a second server under
      the same prefix, but that would make the label ambiguous for every other section of
      `just live-check-widgets` (5a to 5e call `fx` tools by their own names), so the collision is
      unit-tested; the live sections run the stamped path against the pinned lmgw (its list is
      stamped).

**Phase 3**

**WP13. Verify before building** (phase 3's WP0; §11.2's list, §15 WP0's note).
- Owns: `spikes/p3/` and its `RESULTS.md`, one verdict per point.
- Points: (a) AT-SPI trees of Qt and Chromium apps on Plasma 6 Wayland and the cost of enabling the
  bus; (b) Klipper's D-Bus history; (c) layer-shell keyboard on demand for the approval overlay;
  (d) whether KWin grants the restricted interfaces (ruling 16) to a dev build through a `.desktop`
  in a nested KWin's data directories; (e) ScreenShot2's `CaptureWindow` by the window management
  protocol's uuid; (f) `org.kde.kwin.Scripting` for geometry and tiling from Kai; (g) Baloo's
  query path without KRunner; (h) the RemoteDesktop portal against `org_kde_kwin_fake_input` for
  `type_text` and `key_press`; (i) K22's token figure for K24's list on the Kai thread's alias.
- Depends on: nothing. Runs beside everything.
- Without the owner: (c) to (i) in a nested KWin and private sessions. The owner: (a) and (b) on the
  owner's session, since they read its windows and clipboard.
- *Landed 2026-10-09:* (c) to (h) verified, (a) and (b) pending (`spikes/p3/owner-checks.sh`), (i)
  from `just tool-cost`; verdicts in `spikes/p3/RESULTS.md`, folded into WP14 to WP16c and WP19.
  (a) and (b) the owner ran the same day (`spikes/p3/results/owner-a.txt`, `owner-b.txt`, not
  tracked); folded into WP15's as-built.

**WP14. The desktop contract, providers and windows** (K22, K23, K24's `desktop_context`,
`window_list` and window actions, ruling 16).
- Owns: `crates/kai-desktop/src/caps/` (the eleven capability traits),
  `crates/kai-desktop/src/contract/windows.rs`; `src/desktop/` (new: the probe, the choice per
  capability with its configuration override, the capability table in the log and the tray,
  K22's window resolution with its ambiguity error, `fdo/mod.rs`, `plasma/mod.rs`,
  `plasma/windows.rs`, `plasma/kwin_script.rs`). Hook lines: `src/tray/model.rs` (the capability
  line), `src/config.rs` (`[desktop]`), `packaging/` (the `.desktop`'s `X-KDE-Wayland-Interfaces`
  with `org_kde_plasma_window_management`, §13).
- Depends on: WP8; WP13 (d, f).
- *WP13 (f), (d):* KWin scripting works without any grant. Kai exports a D-Bus object on its own
  bus name, and the script returns its result with `callDBus` to it; reading a geometry back in the
  same script is stale (Wayland configures asynchronously), so verify with the next query.
  `getWindowInfo` and `queryWindowInfo` on `org.kde.KWin` give a grant-free window list without
  live events: the fallback for the windows capability when the window management protocol is not
  granted (a dev build, say).
- Tests: the probe's choice with fake providers; a tool its desktop cannot support is not listed; a
  dev build's missing grant answers §11.1's error; the resolution table; `just tool-cost`.
- Live: a nested KWin with test windows: list, activate, move between outputs, tile.
- The owner: the installed RPM on a real desktop (WP25).
- *Built 2026-10-09 (`feat/desktop-providers`), decisions where the spec was silent:*
  - the facts of the Windows capability come from the window management protocol where KWin grants
    it (live, its own Wayland connection; the outputs' names and logical geometry from `wl_output`
    v4 and xdg-output, the desktops' names, the current desktop and the active output from KWin's
    D-Bus), else from KWin scripting; every action goes through scripting, granted or not (one
    path; the protocol has no tiling). Both list the windows a task bar shows, topmost first;
  - `window_maximize` and `window_fullscreen` toggle, as the title bar's button and KWin's
    shortcuts do (the result says `maximized` or `restored`); `window_minimize` only minimizes
    (`window_activate` brings a window back); `window_tile`'s sides are KWin's eight quick-tile
    positions and `none`, which untiles and unmaximizes; quick tiling activates the window;
  - `desktop_context` is listed where any of its parts (Windows, Session, Notify, Media) is served
    and leaves out what is not; do not disturb is Plasma's `Inhibited` (fdo has none); the lock
    and idleness are the screen saver service on Plasma, logind's hints elsewhere;
  - the capability traits other than Windows, Session, Media and Notify are declared without
    methods; the WP that builds their tools adds them;
  - the probe runs once, at the host's first list: outside a runtime (`main`, `kai --tool-list`)
    there and then, so the first list is complete; `[desktop]` takes `plasma`, `fdo` or `none`
    per capability; the tray's line reaches the tray beside the core's model (`src/tray/mod.rs`),
    so `src/core.rs` has no line of WP14;
  - `just install-dev` drops the restricted-interface keys from its `.desktop` (§16 Q21 stays the
    owner's); the packaged one carries `org_kde_plasma_window_management`.
  - *The `active` shorthand, decided by the owner 2026-10-09:* `window` takes `active` (any case)
    for the active window, in every tool that has the parameter (the window tools, `see_window`,
    `read_window`, `ui_press`, `look`), resolved first, as an omitted one is. *Taken in this draft:* the word wins over a title that contains it (that window is named by its id or
    app); the parameter is one shared definition (`contract::WINDOW`) whose description reads
    "active (the default), a window id, an app name or a title fragment"; `see_window` without the
    window list takes `active` as the active window, not as an id.
- *Review follow-up, 2026-10-09:*
  - **A KWin script's answer needs the run's token.** Any session-bus client can have KWin load a
    script of its own, and KWin is then the sender of what that script says; run ids that counted
    from 1 under a predictable object path let it forge a "done" or a snapshot. Each run now has a
    128-bit random token in the script's `ARGS` (`ARGS.reply.id`); the answer is looked up by it,
    and one without it is ignored (and logged at debug). What remains: a process of the same user
    that is not sandboxed can read the script file (0600) or ptrace Kai and learn the token, and
    such a process can do all that Kai can do to the session anyway; a sandboxed client (Flatpak)
    cannot reach the runtime directory.
  - **Script files:** created new (`O_EXCL`, `O_NOFOLLOW`, 0600) under a random name
    (`kai-<pid>-<16 hex>.js`; the plugin name is the file's), in a directory created 0700 that must
    be a real directory of the user's with no group or other bits (else the run fails naming it;
    Kai changes no permissions through a path). At an instance's first run, the leftovers of Kai
    processes that are gone (a file of that name pattern, a pid that is not alive) are unloaded from
    KWin and removed; nothing else in the directory is touched. A plugin whose file was already
    removed when Kai died cannot be found (KWin lists none by name pattern) and stays until KWin
    restarts.
  - `window_close` is destructive (closing can lose unsaved work); K15 still confirms it by default
    and the owner can set it to `allow`.
  - **A call approved in lmgw names its window** (*decided by the owner 2026-10-09, from the
    review of `active`*): lmgw's thread approval comes before the call arrives, so `active` (or an
    omitted `window`) resolved at arrival means whatever has the focus after the owner's click,
    possibly lmgw's dashboard, a WebKitGTK app that is not Kai's own (`key_press enter` or
    `ui_press button "Approve"` would press its next Approve; `window_close` would close it). A
    `Preparer` now receives `Origin` (`registry::Origin::approved_in_lmgw`: `lmgw/approval` present
    and counted by `approval_applies`). For such a call `type_text`, `key_press`, `ui_press` and
    every window action that is not read-only (`tools::is_window_action`: activate, close,
    minimize, maximize, fullscreen, to_desktop, to_output, tile) refuse an omitted, blank or
    `active` window, naming the fix (the window's id or app, `window_list`). The read tools
    (`see_window`, `read_window`, `look`) keep `active`. No list of lmgw's app ids (that would
    miss any other app; naming the window covers all).
  - **Window actions are prepared** (like `ui_press`): the `Preparer` resolves `window` and the
    dialog adds `acts on: <id> <app> "<title>"` instead of leaving "window: active"; the action
    runs only on that id, and a window that is gone is a refusal (`the window this call was
    confirmed for is gone; nothing was done`). Kai's own windows are refused at the preparation.
  - **`type_text` and `key_press` take an optional `window`** (`WINDOW`, two parameters, PD6
    holds): it names the window that must have the keyboard focus; one that does not is a refusal
    (`window_activate` first), nothing is sent. Omitted still means the focused window, except for
    an approved call (above). The focus guard is unchanged.

**WP15. Seeing and reading** (K24's read-only tools, §11.1's vision).
- Owns: `src/desktop/plasma/capture.rs`, `src/desktop/fdo/atspi.rs`, `src/desktop/fdo/recent.rs`,
  `src/desktop/plasma/{baloo,klipper}.rs`, `crates/kai-desktop/src/contract/read.rs`;
  `packaging/`'s `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`.
- Tools: `see_window`, `see_screen`, `read_window`, `read_selection`, `find_files`,
  `recent_files`, `clipboard_get`, `clipboard_history`.
- Depends on: WP14; WP13 (a, b, e, g); M1's message-size settings.
- *WP13 (e), (g):* `see_window` passes the window management uuid to ScreenShot2 `CaptureWindow`
  as it is; no mapping. `find_files` runs `baloosearch6` (the runner's D-Bus `Match` silently caps
  at 10, so it is not used) and reports the limit it applied in the result, no hidden cap; it
  reads Baloo's index status itself, since `baloosearch6` answers from a stale or disabled index
  without saying so.
- Tests: MCP image blocks; a large ultrawide capture within the default `mcp.host_max_message_mb`;
  compact text results.
- Live: nested KWin captures (crops); a non-vision alias gets egress §8.2's placeholder.
- The owner: captures of a real screen (crops), Chromium's accessibility flag.
- *Built 2026-10-09 (`feat/see-read`), decisions where the spec was silent:*
  - providers: Capture is `plasma`'s (ScreenShot2), Search `plasma`'s (Baloo plus the recent
    list) or `fdo`'s (the recent list only, so `find_files` is not listed there), Clipboard
    `plasma`'s (Klipper), Accessibility `fdo`'s (AT-SPI). One provider serves a capability, so
    on Plasma WP16b's `clipboard_set` goes through Klipper's `setClipboardContents`; `wl-copy`
    (`fdo`) only sets, and `clipboard_get` and `clipboard_history` are listed only where the
    provider reads (`Clipboard::reads`);
  - `see_window` is the window's content (no title bar, shadow or pointer); `see_screen`'s
    `output` defaults to KWin's active output and draws the pointer in. Both at the native
    resolution and never scaled: Kai does not know which model reads the image (lmgw sends it by
    the answering alias, egress §8.2, fallbacks included). PNG (lossless, llama.cpp decodes it),
    RGB where every pixel is opaque, else RGBA. The result is one line naming the window or
    output and the pixel size, then MCP's image content;
  - sizes: a synthetic 5760×1440 ultrawide desktop of six text windows is 1.5 MiB as a message;
    incompressible noise of that size is 32 MiB, within the default `mcp.host_max_message_mb`
    (64) but over `mcp.host_max_frame_mb` (16), and Kai sends a message as one frame. No cap in
    Kai: an overrun is lmgw's close naming the setting (api §5.1);
  - a capture, the call and the pipe's read, is bounded by `dbus.timeout_s`; the probe finds
    ScreenShot2 by introspection, and a build without the grant answers §11.1's error at its
    first capture. The packaged `.desktop` carries
    `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`; `just install-dev` drops it with
    the other keys (§16 Q21);
  - `read_window` and `read_selection` are listed where Accessibility and Windows are served: the
    AT-SPI app is the one registered from the window's process, the window among its top-level
    elements the one titled like it (exactly, else the app's only visible one, else by a part of
    the title); two of one title are an error, never a guess. Only `VISIBLE` elements; a line per
    element that has a name, a text or a value, or is a control, indented under the shown one
    above it, with its states; elements that did not answer are counted in the result.
    *Changed by the owner, 2026-10-09 (ruling 33, K33):* Kai switches `org.a11y.Status`
    `IsEnabled` (never `ScreenReaderEnabled`) on during conversation mode and around each read,
    waits for the window's app to join, and puts it back; an app still off the bus is an error
    naming the switch, Kai's part and Chromium's flag (a Flatpak app, whose connection goes
    through xdg-dbus-proxy and shows the proxy's process, gets an error that says so). `read_selection` is the selected ranges of the active
    window's visible text elements, as plain text;
  - the walk's guards (review of 2026-10-09): nothing inside a `password text` element is read,
    not its own text and not the text, selection or value of anything below it; an element with
    the `MANAGES_DESCENDANTS` state (a big table or list that builds its cells on demand) is
    not asked for its children, and its line says "(N children, not walked: ...)" with its
    `ChildCount` (in `read_selection`'s answer too); a reference to an element of another
    application is not followed, one line says "(element of another application, not read)"; at
    most `[accessibility] concurrency` (default 16, 0: no bound) AT-SPI calls are in flight at
    once in one walk, a throughput bound that makes a big tree slower, never shorter; the
    accessibility bus address from `org.a11y.Bus.GetAddress` is used only as a `unix:` address
    with a path or abstract socket (`unixexec:`, `tcp:` are errors);
  - `find_files`: `[search] limit` (default 100, 0: every match) is passed as `-l limit+1`, and
    the result says "the first N matches; the index has more (search.limit)"; the query is one
    argument after `--` (a limit too big to add one to is no limit). `baloosearch6` has no
    NUL-separated output: a line that does not start with `/` is joined to the path before it
    (a path with a newline), and a newline followed by `/` inside a path stays ambiguous; a path
    that is not UTF-8 is shown with `\xNN` escapes and the result says how many. The result always names the indexed (and excluded) folders and says when
    `baloofilerc` has indexing off or file names only, and when the indexer (`org.kde.baloo`) is
    not running. The scheduler's state enum is not read (undocumented on D-Bus);
  - `recent_files`: every entry of `recently-used.xbel`, newest first, to the minute in UTC, with
    the apps that used it; entries their writer marked `<bookmark:private/>` are not listed, and
    the result says how many were skipped;
  - captures are converted to RGBA on a blocking thread, not the async runtime; the 8-bit RGB and
    RGBA formats and the 16-bit RGBA64 ones are read, the 30-bit and floating-point formats an
    HDR output may send are a visible error naming the QImage format;
  - `clipboard_get`'s empty answer reads "Klipper's clipboard entry holds no text" (what
    `getClipboardContents` returns is WP13 (b)'s open check; *answered by a run on a real Plasma
    session, 2026-10-09:* it equals history item 0, and the menu and per-item reads agree);
  - `clipboard_history` numbers the entries, each JSON-quoted on one line, an empty one "(no
    text)";
  - live: `just live-check-read` (nested KWin: the grant's error, then captures about 0.6 s
    after the `.desktop`, the window's own colour sampled, both outputs at 1280×720; AT-SPI over a
    private bus with a GTK window and a Qt dialog, the selection; a private Baloo index, running
    and stopped; the recent list). Klipper on a real session and real apps' trees (WP13 (a), (b))
    stay the owner's.
  - *WP13 (a) on a real Plasma session* (both switches off before, restored after): most Qt apps
    (Dolphin, plasmashell and LibreOffice among them) expose their trees only once `IsEnabled` is
    on; Firefox exposed only its application node; Electron and Chromium apps were absent without
    their flag. `ScreenReaderEnabled` on top added no tree and cost plasmashell noticeably more
    memory; the CPU cost was small. So `IsEnabled` is enough (the probe's docstring guessed Qt 6
    follows only `ScreenReaderEnabled`; the run refutes it). *Answered by the owner, 2026-10-09 (ruling 33):*
    Kai may switch `IsEnabled`, on during conversation mode and around each read, never
    `ScreenReaderEnabled` (K33).
  - *Taken in this draft (2026-10-09):* `read_window`'s error for an app off the bus
    names the switch precisely: with it off, "Qt apps join it once org.a11y.Status IsEnabled is
    true (org.a11y.Bus at /org/a11y/bus on the session bus, kept as the GSettings key
    org.gnome.desktop.interface toolkit-accessibility; ScreenReaderEnabled is not needed)", then
    Kai's part (*changed 2026-10-09, K33:* "which Kai does not switch (accessibility.switch =
    "never")", or why Kai's switch-on did not happen), then that Electron and Chromium apps join
    only when started with `--force-renderer-accessibility`; with it on, only the latter (and,
    when Kai switched it on, that the app did not join within `accessibility.wait_ms`). An app on the bus without its
    windows (Firefox) says so. With it on the words name the switch that is on (`IsEnabled`, or
    `ScreenReaderEnabled` when only that one is). The live checks' stand-in pids
    (`resolve::STAND_INS`) are removed when their test windows are reaped.

**WP16. Acting** (K24's state-changing and confirmed-by-default tools, K14). Three parts with
disjoint files, so they can run side by side:
- **16a, launch:** `open`, `app_launch`, `run_command` (§16 Q12, answered); a
  transient `app-*.scope` per program on the user manager, output captured. Owns
  `src/desktop/fdo/launch.rs`, `contract/launch.rs`.
- **16b, session and media:** `media_play_pause`, `media_next`, `media_previous`, `volume_set`,
  `mute`, `notify`, `ask`, `clipboard_set`, `lock_screen`, `dnd`, `night_light`, `brightness_set`,
  `suspend`. Owns `src/desktop/fdo/{media,session,notify,clipboard}.rs`,
  `src/desktop/plasma/night_light.rs`, `contract/session.rs`.
- **16c, input:** `ui_press` (AT-SPI), `type_text`, `key_press`. Owns `src/desktop/fdo/input.rs`,
  `contract/input.rs`; after WP15 merged (`atspi.rs`).
- Depends on: WP14; 16c also on WP13 (a, h) and WP15.
- *WP13 (h):* `type_text` and `key_press` use `org_kde_kwin_fake_input` (v6 sends characters
  directly) in the `plasma` provider, and the RemoteDesktop portal with a stored restore token in
  `fdo` (its first session shows portal-kde's dialog once).
- Tests: K15's defaults from the annotations (`suspend` destructive; the four confirmed by
  default; each settable to `allow`); a launched program survives `kill -9` of Kai and inherits
  none of its interfaces.
- Live: nested KWin and a private PipeWire instance.
- The owner: everything that types, presses or clicks in a real session (§14: no input is injected).
- *WP16b built 2026-10-09 (`feat/session-media`), decisions where the spec was silent:*
  - the thirteen tools are one contract file (`contract/session.rs`), all state-changing with
    `Ask::Hints` (K15 confirms every one by default, each can be set to `allow`), `suspend` alone
    destructive; each is listed only where its capability is served, and `dnd`, `brightness_set`
    and `night_light` only where the provider can do them (`Notify::can_dnd`,
    `Session::features`);
  - the capability of each: Media (`media_*`), Audio (`volume_set`, `mute`), Notify (`notify`,
    `ask`, `dnd`), Clipboard (`clipboard_set`), Session (`lock_screen`, `suspend`,
    `brightness_set`, `night_light`);
  - `player?` resolves like `window?`: the player that is playing, else the paused one, else the
    only one; a name is matched whole, then by the bus name's last part, then as a fragment; more
    than one match is an error listing them. `volume_set`'s `device?` and `mute` use the default
    output; `mute {on}` has no device (two plain parameters, K22 left it at that);
  - `percent` is 0 to 100 and anything else is an error before the desktop is asked (a visible
    bound, not a hidden clamp: 100 is full scale). `brightness_set` never goes fully dark: 0 is the
    lowest step above off, and the answer says the percent it really is;
  - audio is `pactl` (PipeWire's pulse server) in `fdo/audio.rs`; the sink names come from its
    listing, never from the caller. The clipboard is `wl-copy` in `fdo/clipboard.rs` (the text over
    its standard input); Plasma has no extra clipboard provider yet, so `plasma/klipper.rs` of
    WP15 can take `Clipboard::set` over with Klipper's `setClipboardContents` in one method;
  - `ask` is a notification with one button per choice (the action keys are the positions), no
    expiry, and it waits with no bound of its own: the owner answers whenever they do, lmgw's cancel
    drops the wait and the drop closes the notification. The answer is `answer: "<choice>"` or
    `no answer: ...` when it was dismissed; only signals from the server that showed it count;
  - do not disturb on Plasma is the server's `Inhibit` with Kai's cookie (released with `UnInhibit`,
    or by the server when Kai's connection goes); the owner's own do not disturb is not Kai's to
    switch off, and the answer says "still on" then. The freedesktop server has none, so `dnd` is
    not listed there and `desktop_context` leaves its line out;
  - Night Light is paused and resumed (`org.kde.KWin.NightLight` `inhibit` / `uninhibit` with Kai's
    cookie), never configured: the schedule and temperature stay the owner's. Switched off in the
    settings it cannot be resumed, and the tool says so;
  - brightness: PowerDevil's `org.kde.ScreenBrightness` on Plasma (every display to the same share
    of its own range), logind's `SetBrightness` on the first sysfs backlight elsewhere (firmware
    before platform before raw). Locking is the screen saver service's `Lock` on Plasma and
    logind's `Lock` elsewhere; suspend is logind's `Suspend` (not interactive) on both;
  - the idle and presence reads stay in `desktop_context` (WP14); no new read tool.
  - *Review follow-up, 2026-10-09:*
    - **The inhibitions' cookies cannot get lost.** `Inhibit` (do not disturb) and Night Light's
      `inhibit` run in a task of their own that stores the cookie whatever happens to the caller
      (lmgw's `notifications/cancelled`, or `dbus.timeout_s` passing after the server acted); the
      caller waits at most `dbus.timeout_s` and says so when it passes. The cookie is cleared only
      after a release that succeeded; a failed `UnInhibit` / `uninhibit` keeps it, so a later `off`
      retries and does not misreport. The next call waits for the task, so it sees its outcome. A
      service that never answers `Inhibit` keeps the task (and the next calls' wait, which times
      out) until the bus drops it.
    - **`ask` cannot wait unseen.** The wait ends with an error when the server that showed the
      question leaves the bus (Plasma's shell restarting takes its notifications with it; the
      error says the question is gone unanswered). Under do not disturb, Kai's own or the owner's
      (`Inhibited`), the question goes out with urgency 2 (critical), which Plasma shows through
      it, and the result says so ("do not disturb was on: ..."). `notify` stays as quiet as the
      owner set it. `Notify::ask` returns `Asked { choice, under_dnd }`.
- *WP16a built 2026-10-09 (`feat/launch`), decisions where the spec was silent:*
  - three tools in `contract/launch.rs`, served by the `fdo` provider's Launch (`fdo/launch.rs`
    with `launch/{entries,mime,spawn,settings}.rs`), listed where the user's service manager
    (`org.freedesktop.systemd1`) is on the session bus. All state-changing with `openWorldHint`
    (a URL, whatever the program does): `open` and `app_launch` `Ask::Hints`, `run_command`
    `Ask::Always` (K24) and `destructiveHint` (any command may delete what the owner can; the
    dialog shows its warning). `run_command` is K26's `Class::Async`, but until M4 and WP18 it
    answers when the command ends; every one may be set to `allow` (ruling 28);
  - **the scope, without a race:** Kai forks the program; before `exec` the child calls `setsid`,
    marks every descriptor above 2 close-on-exec, sends its pid through a pipe and waits at a
    gate. Kai asks the user manager for `app-<app id>-<random>.scope` (`StartTransientUnit`,
    `PIDs`, `Slice=app.slice`, `CollectMode=inactive-or-failed`, the description never an
    argument, since the journal logs it), waits for the job's `JobRemoved` within
    `dbus.timeout_s`, then opens the gate; a scope that failed answers "give up" and the program
    never runs. Not `systemd-run --scope`: it would leave a failed scope and the command's own
    exit and stderr indistinguishable. Commands' scopes are `app-dev.rootshell.kai.run-…` (not
    Kai's app id, so a portal grants them nothing of Kai's);
  - **the environment** is the user manager's (`Manager.Environment`, as the scheme handler's
    start of Kai), with PATH, HOME, the runtime directory, the bus and the display taken from
    Kai's where it lacks them; unreadable, Kai's own without `KAI_*`, said in the log. Desktop
    entries and the defaults are looked up in that environment's XDG directories;
  - **`app` resolves** like `window`: the id (with or without `.desktop`), the id's last dotted
    part, the name (as written or in the session's language), the command's file name, a fragment
    of the name or generic name; `NoDisplay` entries only by id; ambiguity lists `<id> "<name>"`.
    `urls` are URLs or paths (absolute, `~/`, else from home); a scheme counts only with `://`, as
    a common one (`https`, `mailto`, …) or as one an application here handles
    (`x-scheme-handler/…`), so `notes:x.txt` is a file. They are passed by the field codes (`%f`/`%u`
    start one process per target, as the spec says; an app with no code gets none and the result
    says so; a URL never reaches `%f`). **The field codes expand as GLib and KIO expand them:** into
    the raw `Exec`, each value quoted, before the line is split (`g_shell_parse_argv`'s rules:
    double and single quotes, a backslash). Outside quotes the value is one shell-quoted word, so
    the argument is the value itself; inside `Exec`'s quotes (`sh -c "mpv %f"`) the word is a
    shell's command, so the value goes in as that shell reads it literally where it stands (its own
    single or double quotes followed), and a file named `x;curl …|sh;.mp4` stays one word. Not
    covered, as in GLib: a shell command built without quotes (`sh -c mpv\ %f`). An entry with
    `Terminal=true` starts in `launch.terminal`. *Decided by the owner, 2026-10-09 (built,
    `src/desktop/fdo/launch/terminal.rs`):* an explicit `launch.terminal` always wins (an empty
    list is refused, naming it); unset (the default now), the first that is installed of: Plasma's
    configured terminal (`kdeglobals`' `[General] TerminalApplication`, read as KConfig reads it:
    the cascade from the least important of `XDG_CONFIG_DIRS` to the user's `XDG_CONFIG_HOME`,
    `[$i]` immutable keys, groups and files, `[$d]` and KConfig's escapes, then expanded as KIO's
    `readPathEntry` expands it with or without `[$e]`: KConfig 6's `$VAR`, `${VAR}` and `$$`, no
    `~`, nothing run; its words then `-e`, as KIO starts a terminal application), then
    `xdg-terminal-exec`, then `konsole -e` when the session is Plasma; only when none is there is
    it an error, naming `launch.terminal` and what was tried (a configured value with an unclosed
    quote is said as that). Installed means found on the
    session's `PATH` or at its absolute path, so a configured but uninstalled terminal falls
    through. Tests with fake `kdeglobals` files and `PATH` (`tests/terminal.rs`, an `app_launch`
    in the terminal the session's `kdeglobals` names). Entries without `Exec` (D-Bus activation only) are not
    installed for Kai yet;
  - **`open`'s type:** `x-scheme-handler/<scheme>` for a URL, `inode/directory`, else the MIME
    database's globs (weight, then length; `cs`); no glob: a program by its first bytes (ELF, `#!`,
    `MZ`), else `text/plain` for UTF-8 without a NUL in the first 4 KiB, else
    `application/octet-stream` (the magic rules are not read). The default follows the MIME
    Applications Associations spec (`kde-mimeapps.list` first, defaults before associations,
    removed ones left out, the entries' `MimeType`, then the parent types and aliases). A missing
    file is an error, not a guess at a URL;
  - **`open` resolves before the confirmation** (the host's `Preparer`): the dialog shows
    `opens with: <name> (<id>) as <type>`, and the call starts exactly that. **It refuses programs
    and launchers:** a desktop entry, a binary, a shell, Python, Perl or Ruby script, a Windows
    program or installer, an AppImage, a Java archive (the type or a type it is a subclass of),
    with an error that points at `run_command`, which the owner confirms;
  - **applications** run in their entry's `Path` (one that is not a folder is an error, not the
    home directory) or the home directory, stdin empty, stdout and stderr on a journal stream named
    by their id (control and format characters left out of the stream's identifier and the scope's
    description); the call answers once they `exec`ed;
  - **commands:** `argv` with no shell (the description says to name one), `cwd` from home
    (`~/` too); stdin empty; stdout and stderr each on a pipe Kai reads into memory, keeping a
    stream's first bytes and its last half as many within the result's room (below), so a runaway
    command fills neither RAM nor a disk. The call answers when the command's leader ends: what
    waits in the pipes then is read and the pipes are closed, so a background child neither holds
    the answer nor writes on (it gets EPIPE); one that ended by itself keeps what it left otherwise
    running. **The time limit is the call's:** the command stops at its deadline (`lmgw/timeout_ms`
    from the call's arrival, the confirmation included) less 0.5 s for the answer (measured on
    loopback: about 14 ms from the limit to the provider's answer, about 240 ms for a 60 MiB result
    to reach lmgw's side), and the result names `lmgw/timeout_ms`; `launch.command_timeout_s`
    (default 0: none) is only the owner's own shorter limit. At the limit and when lmgw's
    `notifications/cancelled` drops the call, the whole scope is killed (`KillUnit`, SIGKILL, so a
    child that left the process group with `setsid` too; then the process group). **Output is
    bounded by the link:** the result's text fits the room the call has within lmgw's message
    limit, measured as the JSON the frame carries (a byte that is not UTF-8 is a U+FFFD, a control
    character an escape); `launch.output_max_mb` (default 0: none) is only the owner's own smaller
    bound. The result's first line is how it ended, the time and each stream's real size; the
    streams share the room by need, a stream that does not fit shows its start and end with the
    cut's size in the first line and a marker line, and the first line names the limit that cut
    it;
  - **two spawns at one instant:** std's spawn learns of an `exec` failure through a pipe a
    sibling forked at that moment holds until its own `exec`, so the earlier answer can wait for
    the sibling's gate (at most `dbus.timeout_s`, or its cancel). Documented, not fixed: closing
    the copies in the child would need std's pipe, which it cannot tell from the others. A launch
    held at its gate holds no launch forked after it (tested);
  - **live:** `just live-check-launch`: K14 on the session's real user manager with `sleep` only
    (it survives SIGKILL of a stand-in for Kai, in its own scope under `app.slice`, descriptors 0
    to 2, no `KAI_*`; a command stopped at its limit has its whole scope killed, a `setsid` sleep
    too), and kdialog windows from `app_launch` and `open` only in a nested KWin with
    a fake manager on its bus.
- *WP16c built 2026-10-09 (`feat/input`), decisions where the spec was silent:*
  - the three tools are one contract file (`contract/input.rs`), all state-changing, none
    destructive, `Ask::Always` (K15 confirms every one for every caller, each can be set to
    `allow`). Neither returns what it typed or pressed, and Kai logs none of it. `ui_press` is
    listed where Accessibility and Windows are served (like `read_window`); `type_text` and
    `key_press` where Input and Windows are (*review, 2026-10-09:* the window with the focus is
    what the owner confirms and what the typing checks, so without Windows they are not listed).
    The `Input` trait takes parsed values (`type_keysyms`, `press_chords`) and a `Guard`; the
    parsing is `kai_desktop::caps::input`, the same for both providers;
  - *review, 2026-10-09:* **the confirmation names the target.** All three tools have a
    `Preparer` (WP16a's hook): the arguments are checked and the target resolved before the
    dialog, which shows `receives it: <id> <app> "<title>"` (the window with the keyboard focus;
    none is a refusal that asks nobody) or, for `ui_press`, `presses: <role> "<name>" in <id>
    <app> "<title>"`: the element is found before the dialog, so none, several or a disabled one
    is refused without asking. The handler acts on what was prepared: `ui_press` presses exactly
    that element (`Accessibility::find`, then `press(&Found)`, which re-reads its role, name and
    state first; gone or changed is a refusal and nothing is pressed);
  - *review:* **the focus guard.** Before the first event and before every further character and
    chord, the window with the focus (`Windows::active`: the protocol's table where granted, no
    call; else one KWin script run, any window, a panel's or KRunner's too) must be the confirmed
    one. At the start Kai waits for it up to `dbus.timeout_s` (the confirmation's or the portal's
    dialog hands the focus back as it closes); after the first event a change stops the call at
    once: `focus moved to <app>; N of M characters typed` (`chords pressed`), never the text. The
    check and the character are two steps, so one character may still land in the new window
    (live, three runs: stopped at 91, 107 and 116 of 3000, none slipped). The KWin-scripting path
    costs about 2.5 to 3 ms a character (1000 characters 5.2 to 5.8 s, 2.8 s without the check);
    an installed Kai reads the table. **Kai's KWin script runs now take turns** (one process-wide
    lock from `loadScript` to the unload): KWin numbers a script by its list's length, so a load
    while another run unloaded could get a number in use (`No such object path
    '/Scripting/Script1'`, seen live when `window_activate` ran during typing);
  - *review:* **one input call at a time** (all three tools, any provider): an async mutex in the
    services; a waiting call gives up at its deadline less `ANSWER_MARGIN` (it says so, nothing
    typed) and stops waiting when lmgw cancels it;
  - **`type_text`** types the text as keysyms (one press and one release per character; a line
    break is Return, a tab is Tab, a lone CR is Return, CR LF one Return; any other control
    character is refused before anything is typed, by its code point). It does not depend on the
    keyboard layout. The answer is `typed N characters`;
  - **`key_press {keys}`**: chords separated by spaces, keys joined by `+`, any case:
    `ctrl+a`, `alt+tab enter`, `super+d`, `f5`. Names: letters, digits, `f1` to `f24`, `ctrl`,
    `shift`, `alt`, `altgr`, `super`, `enter`, `esc`, `tab`, `space`, `backspace`, `delete`,
    `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `capslock`,
    `print`, `menu`, `pause`, `plus` (keypad plus), `minus`, `equal` and the US punctuation keys.
    The names are keys of a US (ANSI) keyboard sent as evdev codes; on another layout the key at
    that place gives its own letter. A chord
    is all down in order, all up in reverse; a bad name is an error that names the chord's number,
    not its text. The answer is `pressed N chords`;
  - **`ui_press {window?, element}`** runs one action of one visible element through AT-SPI's
    Action interface (`fdo/atspi/press.rs`, a child of WP15's module: one `mod` line and the trait
    method in `atspi.rs`). `element` is a name as `read_window` shows it (exact, then a fragment,
    any case), or `button "Save"`: the role, then the name in JSON quotes, the role matching
    whole. More than one match is an error that lists them as `role "name"` lines; none says what
    the walk could not see (elements that did not answer, ones that manage their own
    descendants, which are not descended into). A control that is not enabled is not pressed.
    The action is the first of `click`, `press`, `activate`, `toggle`, `check`, `select`, `open`,
    `jump`, else the first the element has. Its `window` takes `active` like every `window`
    (WP14's as-built, 2026-10-09). A handler that never returns (a dialog it waits in)
    counts as a press made after `dbus.timeout_s`. Unnamed elements cannot be pressed this way;
    the answer is `pressed <role> "<name>" in <window>`;
  - **plasma provider** (`plasma/input.rs`): `org_kde_kwin_fake_input` v6, `keyboard_keysym` for
    text and `keyboard_key` for chords. One short Wayland connection per call on a blocking
    thread (bind, authenticate, send, close), so a `.desktop` written while Kai runs is honoured
    by the next call. Every event is followed by a round trip to KWin and every release by 2 ms
    (1000 characters took 2.8 s, 5.2 to 5.8 s with the focus check through KWin scripting, and
    arrived exactly in GTK and Qt). Lmgw's cancel drops the call;
    the thread stops before the next event and releases the keys it holds. The probe asks only
    whether KWin lists the interface to this process (nothing is bound or sent); without the
    grant the offer is absent with the grant named, so `fdo`'s portal is tried and, with neither,
    the tools are not listed and the capability table says why. (K23 keeps a tool listed with a
    `NotGranted` error where the same desktop could serve it after an install; here the portal is
    the other way to the same end, so absent-and-fall-through was the smaller decision. The error
    stays for a grant lost after the probe.) The packaged `.desktop` carries
    `X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,org_kde_kwin_fake_input`;
    `just install-dev` still strips the keys; a dev build gets them only by opting in with
    `just grant-dev` (§16 Q21 as answered, 2026-10-09);
  - **fdo provider** (`fdo/input.rs`; *review:* paced like KWin's, every event awaits the
    portal's reply and every release then `RELEASE_PAUSE`, 2 ms: 1000 characters exact in GTK in
    6.4 to 7.3 s with the focus check through KWin scripting; a call dropped mid-chord or mid-character releases the
    keys still down, a press counted from before it is sent, before the session closes): the
    RemoteDesktop portal (`ashpd`), keyboard only,
    `persist_mode` 2. A session per call (CreateSession, SelectDevices, Start, the key events,
    Close), so the desktop's remote-control indicator shows only while Kai types. The probe takes
    a running portal whose `AvailableDeviceTypes` has the keyboard bit, or one that is only
    activatable. The first Start shows the desktop's "Remote Control" dialog and waits with no
    bound of Kai's (the owner answers whenever they do; lmgw's cancel drops the call, and the drop
    closes the session, which ends the dialog). The restore token the last Start returned is
    stored in `$XDG_STATE_HOME/kai/remote-desktop-token` (`Paths::state_dir`; 0600, written through
    a temporary file, replaced only when it changes; *review:* the directory is made 0700 also when
    it existed, only if it is a real directory of this user's, never through a symlink, else
    nothing is stored), offered at the next SelectDevices, and never logged; with it later
    sessions need no dialog, even after a pre-authorisation was revoked. **Revoking Kai in System
    Settings does not invalidate a stored token**: `kai --forget-remote-control` removes it (a CLI
    flag, the smallest way until WP21a's settings). The owner's "no" is a refusal that says so;
  - *Decided by the owner, 2026-10-09 (built):* the stored token is kept. The configuration
    window's Desktop page has a *Remote control* section: a status line saying whether a token is
    stored, and "Forget remote-control permission", which asks first ("Forget the remote-control
    permission?") and then does what `kai --forget-remote-control` does (`GET
    /api/remote-control`, `POST /api/remote-control/forget`, `Command::WindowRemoteControl` on the
    core's state directory, `src/settings/window/remote.rs`). The confirmation dialog of
    `type_text` and `key_press` (PD4), when the portal types, has a line saying the remote-control
    permission lasts until it is forgotten in Kai's settings and that revoking it in System
    Settings does not end it (worded for a stored token and for the desktop's first ask;
    `Prepared::notes`, `Services::input_token`); KWin's fake input needs no permission and has no
    such line. Tests: the routes and the page's wire type, the core forgetting the file, the
    note's words with and without a token and its absence with KWin; `just live-check-window`
    forgets a stand-in token through the page after its confirmation;
  - tests: the parsers, the query matching, the tools over recording fakes, K15's defaults on the
    real specs, `ui_press` against a fake accessibility bus (picks, preferred action, disabled,
    hidden, a hanging handler, a found element gone or changed), the portal provider against a
    fake RemoteDesktop portal on a private bus (token stored 0600 and offered, refusal, a dropped
    call closes its session, the guard's stop, a drop mid-chord releasing its keys, the pacing),
    the probes' offers; *review:* the dialog's lines, focus moved between the confirmation and the
    start (waited for, then nothing typed; back in time, typed), focus moved mid-typing and
    between chords, the fragment found before the dialog, input calls taking turns (deadline,
    cancel), the token directory tightened and a symlink refused, forgetting the token;
  - live: `just live-check-input` (nested KWin, private buses; nothing is typed into the owner's
    session): no grant and no portal lists neither tool, then the `.desktop` grants it 1.4 s
    later; text exact in a GTK entry and a Qt entry (kdialog, 1029 characters), chords, a call
    dropped half way stops, the focus moved to another window mid-typing stops it (the count
    matches the entries), `ui_press` on GTK and Qt buttons and a check box, the portal's dialog
    wait and its closing on a drop, pre-authorised use with 1000 characters timed, the stored
    token alone after the pre-authorisation was revoked;
  - the owner's: everything that types or presses in a real session (§14), including the focus
    coming back to the confirmed window after K15's dialog closes (not driven live: the nested
    check calls the handlers without a dialog), the portal's dialog on a real desktop once (its "remember" box is what makes the token), and the installed Kai's grant
    of `org_kde_kwin_fake_input`.

**WP17. Delegates** (ruling 21, K25).
- Owns: `crates/kai-delegate/` (the loop: static part first, dynamic part last, the user's last
  utterance verbatim, a plan confirmed once, a short done, partial or failed answer; model and tools
  behind traits); `src/delegate/` (Kai's adapter: lmgw's `/v1/chat/completions` under Kai's key,
  the tools of WP14 to WP16); `contract/delegate.rs`; `src/config.rs`'s `[delegate]` (an alias per
  delegate, §16 Q13).
- Tools: `look` and `find` (synchronous, K26); `arrange_windows` and `operate_app` are listed only
  once WP18's asynchronous class exists.
- Depends on: WP15; WP16c for `operate_app`; M7 (scripted tool calls) for its live check.
- Tests, against a scripted model: the static prefix byte-identical across calls; no conversation
  in the context; a screenshot only in the delegate's request; the plan's single confirmation.
- The owner: real-model quality of `look` on the owner's aliases, and Q13.
- *Built 2026-10-09 (`feat/delegates`), decisions where the spec was silent:*
  - **The loop** (`kai_delegate::run`): the static part is the system message (the delegate's
    instructions and examples, then one frame shared by every delegate: the task's scope, the
    answer's form, the plan rule) and the tools (the delegate's own list as the desktop offers it,
    then `propose_plan`); it depends on nothing but the delegate and that offer, and lmgw's body is
    serialized `model`, `stream`, `tools`, `messages` so it is also a byte prefix of every request.
    The dynamic part is one user message: the context reads (`look`: `window_list` and
    `see_window` of the call's `window`; `find`: `window_list`), the parameters, then the user's
    words verbatim at the very end. Then the loop's own turns. A tool's image goes as a user
    message after the tool messages (OpenAI's tool message is text only, and lmgw's OpenAI ingress
    reads only that), as an `image_url` data URL; the main agent gets text only.
  - **The answer** is `done|partial|failed: <the delegate's words>` and a line `(used: see_screen,
    read_window (Konsole) ×2)` (each call's tool with its arguments' values, so the main agent sees
    what was looked at). When a fallback answered the delegate's request (lmgw's `x-lmgw-fallback`),
    a last line says `(model: <alias>, answered by <fallback>)`, and the log names it too. A final text without the word counts as done; an empty one, a model error
    (lmgw's status, code and message), a declined plan and the turn bound are failed or partial
    answers that say why. A failed answer is the tool's error (`isError`).
  - **The bound:** `delegate.max_turns` (default 20 model turns, 0: none); the stop is a partial
    answer naming the key. lmgw's timeout for the call (its cancel) and the model's context end a
    task otherwise. No bound of Kai's own on a model request.
  - **K15:** each of the delegate's calls (the context reads included) is decided by the policy
    as a call by the delegate's caller, with no approver (F approved the delegate, not its
    steps). Allowed: it runs. Denied: the model reads the reason. Confirm: it runs only as a step
    of the plan. A context read that is not allowed is left out, its place saying why. The policy
    is the host's own, shared live (`Deps::policy`, the `Arc<RwLock<Policy>>` the host's server
    reads): a change (WP21a's `set_policy`) applies to the delegates' next decision and to their
    listing at once, a running task's next step included.
  - **The plan** is the tool `propose_plan {steps: [{tool, arguments}]}` (a delegate-internal tool,
    not a hosted one, so K22's flat rule does not bind it): each step checked as a call; a
    malformed or refused step, or a plan with nothing to confirm, goes back unasked and is not the
    plan. Otherwise PD4's dialog shows the steps that need a confirmation, once ("look (a plan)",
    the caller, the delegate's task line (`question`, `what`) and alias in its description,
    numbered steps, the time left of the delegate's call as the host's dialog counts it; lmgw's
    cancel closes it). A dialog that cannot be shown is not a refusal: the task fails with "could
    not ask the owner" (`Owner::confirm_plan` is `Option<bool>`). Confirmed, each such step
    runs once when called with the same arguments; any other call that needs a confirmation is
    refused; a second plan is refused unasked. Declined, the task ends failed and nothing of it
    runs. Each step's call carries the delegate's call's deadline and `lmgw/timeout_ms`, no
    message room (its result goes to the delegate's model, not over the link). A tool with a
    Preparer (WP16a's `open`) resolves at the call, so a plan shows its arguments only; that
    matters once a delegate uses such a tool (`operate_app`, with WP18), and a test fails while
    any delegate tool has one.
  - **The user's words:** lmgw stamps no utterance on a call (api §5.5), so Kai hands the user
    message of its own voice session's current turn (`lmgw.chat.user`, `src/voice/events.rs`
    records it) to calls whose caller is Kai itself. A heard turn's message races its reply, so
    it counts only if it came after `response.created` or names that response
    (`lmgw.chat.user.response_id`); a response's start and the session's end drop what was kept,
    and with nothing for the turn the prompt says "not transcribed yet"; another caller's delegate gets the parameter alone and
    its prompt says the words are not known and why. An `lmgw/utterance` stamp would close that
    (lmgw-side, Later).
  - **The alias** (§16 Q13): `delegate.look` and `delegate.find`; absent, the Kai conversation's
    folder's model (`conversation::pick` over `GET /chat/api/folders`, asked at each call,
    bounded by `lmgw.timeout_s`). No alias is a failed answer naming the key to set. `look` (it works from
    screenshots) reads `capabilities.vision` of the alias in lmgw's `/v1/models` before its first
    request, and fails with "set delegate.look to a vision alias" when lmgw says `false`; absent
    or unreadable means unknown, and the task goes on. The default alias is unchanged.
  - **Listing:** `look` needs `see_window`, `find` needs `find_files` or `recent_files`, as the
    desktop serves them and the policy lists them (a tool set `off` is no delegate's either).
    Only `Class::Sync` delegates are listed; `arrange_windows` and `operate_app` join
    `contract/delegate.rs` as `Class::Async` with a `Def` each in `src/delegate/defs.rs`, and their
    handler starts a WP18 job that runs the same loop; nothing in `kai-delegate` changes.
  - **Hook lines:** `registry::providers()` shares the desktop provider with the delegates' (the
    desktop line became `let desktop = …; push(desktop.clone()); push(delegate::provider(deps,
    desktop))`, `DesktopTools` is `Clone`), so the probe runs once; `areas!` gains `delegate`;
    `src/config.rs` the `[delegate]` field and its check; `src/main.rs` `mod delegate`;
    `src/voice/events.rs` and `src/voice/control.rs` the lines recording the user message, a
    response's start and a session's end; `Deps` and `HostLink::new` carry the shared policy (the
    one new `Deps` field); the host's contract test one line
    for the delegates' entries.
  - Tests: `kai-delegate`'s own against a scripted model (13: the static part byte-identical and
    free of the task, the dynamic order, no conversation, the screenshot in the delegate's request
    only, the plan's single confirmation, its refusals and the declined plan, the answers and the
    turn bound, cancellation during the model request, a tool call (its spawned work sees it) and
    the owner's question); Kai's adapter against a loopback lmgw (the key, the alias from the
    folder or the setting, the body's byte prefix across calls, OpenAI's image parts, K15 by an
    untrusted caller with one dialog, listing, a 503's words, cancellation, a trace log without the
    key, the words, the question, the arguments, the screenshots or the prompt). lmgw's
    `scripts/mock-openai.py` was not used: the loopback server scripts the same turns in process.
    The live check (M7 driving `look` through a Chat turn on a dev lmgw) and real-model quality
    stay open.
  - **Who looks at screenshots** (*decided by the owner 2026-10-09*, K25's note): a setting,
    `delegate.screenshots = "main" | "delegate"`, default `main` (as built before). With
    `delegate`, `see_window` and `see_screen` are not listed to Kai's callers (lmgw's chats, Kai's
    own thread, `kai tool`, the routines); `look` is listed as before and is the way to see: its
    delegate gets the screenshots, the caller a text answer, so the caller's model needs no vision
    and little context. The delegate's alias must have vision (the check above). *Taken in this
    draft:*
    - the shape: the host's list loses the two image tools, nothing is added, renamed or
      duplicated (K22: no second seeing tool beside `look` to confuse the model, and the list
      gets shorter); a call of one of them is refused as an unlisted tool. The delegates' own tools
      stay the desktop module's, unfiltered (`src/delegate/screenshots.rs`'s `HostView` is what
      the host lists in the desktop provider's place: one changed line in
      `registry::providers()`);
    - the key lives in `[delegate]`, beside `delegate.look`, the alias it needs set to a vision
      model; `[desktop]` is K23's per-capability provider table, which the settings window shows
      as provider rows. The settings window's generic list shows it with the other `delegate.*`
      keys;
    - it applies at once (`Effect::Now`, like `policy.*`): the core hands a change to the host
      (`ToolHost::set_screenshots`), which lists again and sends `list_changed` only when the list
      changed (K22: one full prefill, only when the owner changes it);
    - with `delegate` and `look` switched off by the policy (or its desktop lacking Capture), the
      callers have no way to see; that is the owner's combination, not refused.
  - **The image tools are a contract flag** (review, 2026-10-09): `ToolSpec::returns_images`
    (set for `see_window` and `see_screen`) replaces the hand-kept name list; `HostView` filters by
    it, and a test runs every read tool over fakes and asserts that exactly the flagged ones answer
    with an image block.
  - **Relisting is serialized by the list's lock** (review, 2026-10-09): `Shared::relist` builds
    the list inside the `listed` write lock (order: listed, then policy), so a relister that was
    late cannot put a list built before another's change over it; a test races two setting
    changes and compares the list with the final setting.
  - **No limit on what a delegate reads** (*decided by the owner 2026-10-09*): `look` may see or
    read any window and output, not only the one its call named (the review suggested limiting
    it; the owner rejected limiting). Its `window` takes `active` (WP14's as-built).
  - *Review fixes, 2026-10-09:* the user's words by turn, the live policy, the answering alias and
    the vision check, the used line's targets, the plan dialog's context, the unshowable dialog,
    and the Preparer test (`kai-delegate`'s plan tests, `src/delegate/heard.rs`'s unit tests and
    `src/delegate/tests/review.rs`). Not tested through the voice events: the words are one
    process-wide value, so their logic is tested on `Heard` itself.
- *Decided by the owner, 2026-10-09 (the walkthrough of open calls):*
  - **(a) The alias when `delegate.look` or `delegate.find` is unset** is the Kai conversation's
    model, with its fallback: a fallback configured on that alias answers the delegate as it would
    answer the conversation (a fallback the owner configured applies here too; ruling 7). As built: the
    request goes to the folder's alias and lmgw applies its fallback; the answer's last line names
    the alias that answered.
  - **(c) One plan dialog for an untrusted caller's `look`,** kept as built: the plan's steps that
    need a confirmation are shown once in PD4's dialog.
  - **(d) The delegates' inference runs under Kai's key for every caller,** as built: Kai's
    sessions live in lmgw, so whatever interface starts the work, lmgw counts it as Kai's usage. A
    delegate called from lmgw's dashboard Chat,
    a routine or `kai tool` is Kai's usage in lmgw's ledger, not the caller's.
- **Requirements for the async delegates** (`arrange_windows`, `operate_app`; §16 Q13 as answered
  by the owner, 2026-10-09). The design of the async delegates starts from these:
  - **Three setups, all allowed.**
    1. The Kai conversation's model on a row with more than one slot: the async delegates share
       it, each in a slot of its own; nothing to warn about.
    2. One slot and no delegate model: allowed, with a warning that the conversation waits while
       an async task runs (its requests queue behind the task's on the one slot). The warning is
       on Kai's delegates settings page, and in the tray once, the first time it happens.
    3. A delegate model (`delegate.<name>` naming another alias): the slot count does not matter.
       A warning only when lmgw's VRAM fit verdict says the conversation's model and the
       delegate's cannot be resident together (a kv-unified pool counted as lmgw counts it).
  - **Warnings state their limits:** slots are shared with lmgw's other clients (a free slot now
    may be another client's in a moment), and the VRAM figure is lmgw's estimate, not a promise.
  - **From lmgw, built together with the async delegates, not before:** a per-local-alias slot
    count that a device key can read, and a co-residency fit verdict for two aliases (the VRAM fit
    lmgw already computes, including a kv-unified pool). Cloud aliases have neither, and get no
    warning.
  - **A request priority value** (a planned lmgw feature, request priority): an lmgw feature for
    every client, so the conversation's requests can go before the async delegates' while both
    wait for the same model. Kai sends the lower priority on a delegate's requests once lmgw has it.

**WP18. Asynchronous jobs** (ruling 22, K26).
- Owns: `src/jobs/` (the job table, `jobs`, `job_cancel`, `announce`, the delivery moment),
  `contract/jobs.rs`. Hook lines: `src/voice/control.rs` (a `response.create` at a quiet point for
  a late result; the announce bind, with K5's `takeover=never`), `src/tray/model.rs` and
  `src/tray/mod.rs` (the Jobs submenu), `src/core/calls.rs`, `src/config.rs` (`[jobs]`).
- Depends on: M4, WP8, WP9 (the voice files, in that order).
- Tests (paused time): never delivered while the owner talks or Kai speaks; `announce` on for a
  voice-started job and off otherwise (then a notification); `job_cancel` cancels the task; the
  tray lists every running job.
- Live: M7 and a slow `live-test` job against the dev lmgw: the result enters the thread, a bound
  session speaks it, and with no session an announce binds by itself.
- The owner: an announce with real speech.
- *Built 2026-10-09 (`feat/async-jobs`, lmgw at 0eef8ee: M4's WP1 to WP3 with their review fixes),
  decisions where the spec was silent, drafts:*
  - **M4's contract as lmgw built it** (its design's table): MCP **2025-11-25**'s Tasks, not
    2026-07-28's (T1); Kai declares `capabilities.tasks {cancel, requests.tools.call}` and every
    `Class::Async` tool `execution.taskSupport: "required"` (T3; `src/host/server.rs`'s `render`);
    a link that drops ends nothing (T7: the job table lives with the host's `Shared`, lmgw polls
    again on the next link, a restarted Kai answers `-32602` and lmgw reads the job abandoned);
    wire names `task.*` and `lmgw.task.done` (T14); the result enters the thread as an
    `lmgw__job_result` pair placed before the next reply (T11; since lmgw `3fe2aff` where it was
    stored, below); lmgw starts no turn (T12): a bare
    `response.create` continues.
  - **Admission before the task.** A task-augmented call passes the one path every call takes
    (`Shared::dispatch`: K15, the preparation, PD4's dialog, the screen lock) within lmgw's
    `lmgw/timeout_ms`, which on such a call bounds the wait for the task. Refused or cancelled
    there, it is the request's JSON-RPC error (`-32001` with K15's or the owner's words, `-32800`
    cancelled) and no job exists; admitted, the handler runs as a job with no deadline and its own
    cancellation, and `tools/call` answers `CreateTaskResult` with the model's line
    (`io.modelcontextprotocol/model-immediate-response`: the result comes into the chat, spoken or
    notified). `src/host/tasks.rs`; `dispatch` itself is unchanged (a gate handler hands the
    admitted call over).
  - **The job table** (`src/jobs/`): ids four hex digits, unique among the jobs Kai knows, short
    to say (the task id on the wire carries more, see the review fixes); no ttl (null) and no poll
    interval of Kai's: Kai sends `notifications/tasks/status` at the job's end,
    `mcp.task_poll_interval_s` covers a lost one. An error reply is `failed`; `tasks/cancel`,
    `job_cancel` and the tray's Cancel end it `cancelled` at once and cancel the handler's token;
    Kai's quit cancels what runs. When an ended job is forgotten: the review fixes below.
  - **`announce`** (ruling 22): on when the caller is Kai's own key (its voice session), off for
    every other caller; an async routine's `announce` sets it from inside its run
    (`jobs::set_announce`, a task-local of the job); `job_announce {id, on}` and the tray's switch
    change it. A result of another server's job counts as announced when it is in Kai's
    conversation thread (a job the voice started, whoever ran it), and is not Kai's to say in any
    other thread.
  - **Tools** (`contract/jobs.rs`, served by the core, PD3): `jobs` (read-only, `Ask::Hints`),
    `job_cancel {id}` and `job_announce {id, on}` (`Ask::Trusted`: trusted callers without a
    dialog, others confirmed; a cancel stops work and starts none). Level `any`.
  - **The quiet moment** (`src/voice/jobs.rs`): no talk key held, the session's machine quiet (no
    speech heard, no reply open or playing, nothing to commit), no call waiting for an approval,
    no other client's turn in the thread (K16), the screen unlocked, not muted, no session on its
    way; then `jobs.quiet_ms` (a visible setting, default 1500 ms, 0: at once) of all that. One
    rule for push-to-talk and continuous listening (conversation mode, automatic): the owner's
    speech is the held key there and the server's detector here.
  - **Spoken in a bound session**: one tagged `response.create` (`kai_job_<n>`, no commit, no
    item) at the quiet moment after every result, also one that came while a response ran (lmgw
    says that response answers it, but `lmgw.task.done` and `response.done` are not ordered, so Kai
    asks once more and lmgw's `empty_turn` for it speaks nothing). The refusal is read from the
    `error` that echoes the create's id: `empty_turn` ends it silently (another window answered);
    `turn_running` waits for the thread's `turn.done` on the feed, then asks at the next quiet
    moment (a turn the feed never showed: one more ask, then the wait); any other code is a
    notification. Kai's grace close waits while a result is to be spoken in its session.
  - **No session**: Kai binds its conversation's thread itself (K5's `takeover=never`, a press
    session closing `voice.grace_s` after the reply). A result in another thread (an old
    conversation after a rollover), a thread another client's voice holds, a session bound
    elsewhere or a bind that did not come is notified instead: Kai never moves its conversation
    for a result.
  - **The screen lock** (WP18, a draft of 2026-10-09): results that arrive while locked wait,
    their notifications too; after the unlock they are spoken (or notified) at the first quiet
    moment, which binds again since the lock ended the session. A job admitted before the lock
    runs on (a long command the owner left running); every call it makes through Kai is refused
    while locked as any call is, and a new task call is refused (`crate::screen_lock::REFUSAL`).
    No job delivers while locked: the review fixes below.
  - **A cough** never speaks a result: lmgw continues only on the client's own bare create, and
    Kai never answers a cough's `empty_turn` with a create; the cough restarts the quiet wait.
  - **Routines**: an `async = true` routine is offered (no longer held back) and runs as a job; from
    the tray or `kai tool` it runs synchronously as before. `arrange_windows` and `operate_app`
    are not built (WP17's as-built: they join `contract/delegate.rs` as `Class::Async`; nothing
    of WP18 changes for them).
  - **The tray**: a Jobs submenu beside Routines while any job is known, one submenu per job (tool,
    id, start, caller) with "Speak the result" and Cancel; the tooltip says how many run.
  - **Tests**: the table (`src/jobs/tests.rs`), the host's tasks against a fake lmgw
    (`src/host/tests/tasks.rs`: what Kai declares, the life of a task, `failed`, `tasks/cancel`,
    `-32602`, a refusal and a cancel before admission, a link that drops, the lock, the quit, the
    announce default), the voice in paused time (`src/voice/tests/jobs.rs`: the quiet wait, never
    while the key is held or Kai speaks, speech and a cough restart it, `turn_running` seen and
    unseen, `empty_turn`, Kai's own bind, another thread and a held one notified, announce off,
    the lock during the wait and results arriving locked, the job tools). Live:
    `just live-check-jobs` (the bound session, Kai's own bind, an owner's job notified, lmgw's
    cancel reaching Kai).
  - *Seen live:* a send in a thread whose newest row is an unanswered result was rendered by lmgw
    as `U [call R] R`, so the model answered the result first (T11's table then); the live check
    used a fresh thread for each owner job for that reason. *Since lmgw `3fe2aff`* (T11 decided
    by the owner, chronological order) the send follows the result, and the check's owner jobs
    share one thread: the cancel step's send comes after the announce-off job's unanswered
    result, and the mock calls the tool again (it would echo the result under the old placement).
- *Review fixes, 2026-10-09 (`feat/async-jobs`; taken in this draft):*
  - **A job's output is bounded.** The admitted call gets the room its text has in the answer to
    `tasks/result` under the link's limits at its admission (`LinkLimits::task_text_room`: the
    message limit less that answer's envelope and its related-task `_meta`, the widest numeric
    request id standing in for lmgw's, which is not known yet), as a synchronous call gets its
    answer's room: `run_command` keeps its output's start and end within it and says what it cut
    and which limit cut it, a script routine keeps no more than it (a larger stdout is its error
    naming the room, K22's "never cut"), so no job grows Kai's memory past the message limit
    (`launch.output_max_mb` 0 and no deadline included). Only a later link whose limit shrank
    still answers an oversized result with the limit's error.
  - **A wait that cannot end, ended.** A `turn_running` wait on a turn the feed showed ends when
    the turn leaves the feed's live state without its `turn.done` (a resync's `state`, the feed
    dropping), and Kai asks again at the next quiet moment. A turn the feed never showed: one more
    ask, then `jobs.quiet_ms` for the feed to show it, then Kai gives up with a notification ("lmgw
    says a turn runs there that Kai does not see"; the result waits in its chat). Nothing holds
    the session or the microphone for a wait that nothing ends.
  - **Muted and do not disturb** (a draft of 2026-10-09): muted, a result waits until the
    unmute and stays owed (no session held for it meanwhile); do not disturb, it is not spoken but
    notified. Kai did not follow do not disturb (the desktop tools read it when they run); it now
    follows the notification server's `Inhibited` (`src/dnd.rs`, `PropertiesChanged`, read at
    start and again when a new server takes the name; a server without it counts as off).
  - **The lock and revocation** (a draft of 2026-10-09): admitted jobs run on while locked, but
    no job delivers while locked: `tasks/result` and the status notifications are held until the
    unlock, for every caller (`ScreenLock::unlocked`); `tasks/get` answers as it is, so lmgw may
    see `completed` and still wait for the result. A routine switched off or whose folder is gone
    ends its running jobs (the host's relist compares what the providers offer: K15's `off` and
    any other policy change unlist a tool but end no admitted job). Kai's own key revoked (as
    before) or replaced by a new pairing ends every running job: no link of the new key follows
    them. A caller device's key deleted on lmgw (the phone's, not Kai's) is said by the feed's
    `device.revoked` (`{device: {kind, name}, by}`, lmgw's client-apps design §2.2, MCP Tasks
    §4.1), which Kai hears as the host of an open task the device started: `LinkEvent::DeviceRevoked`,
    and `Jobs::device_revoked` cancels every running job whose starter is `device:<name>` (the
    `lmgw/caller` the job recorded) by the same path as a routine switched off (status notification,
    the tray's list shows it cancelled, "the device '<name>' that started it was deleted" in its
    result and the log); other callers' jobs, ended ones and a device with no jobs are left alone,
    and it works whatever the link's or the lock's state. A disabled or expired key, a rotation and a
    cleared hosting grant say nothing: such a job waits or runs on.
  - **No result lost on a half-dead link.** A fetch alone confirms nothing: an ended job stays
    until lmgw fetched it and `task.done` said it entered its thread. A result no `task.done` will
    confirm (lmgw's bridge, Admin Chat's thread, a delivery Kai does not know) stays fetchable
    after its fetch, so lmgw's next poll on a new link still gets it.
  - **The table cannot grow for good, nor hang.** An ended job whose delivery was never confirmed
    is dropped `jobs.keep_ended_s` after its end or its latest fetch (a visible setting, default
    86400, 0: until Kai quits); the tray's entry and the `jobs` line say until when ("kept until
    HH:MM (jobs.keep_ended_s)"). The task id on the wire is the short id, six random hex digits of
    this run of Kai and a counter (`ab12-3f9c1e-7`), so a task id never repeats within a run and,
    but by chance, not across runs (an earlier Kai's row gets `-32602`, never a new job's
    status); the short id stays the one to say, and `job_cancel`, `job_announce` and lmgw take
    either. With all 65536 short ids taken a new task call is refused with words naming
    `jobs.keep_ended_s` (one pass over the table, never a search that does not end). A job called
    before the feed's `hello` named the label is matched at its `task.done` with the label known
    then, so its announce holds.
  - **The continuation** (low): one cut off (`cancelled`, `incomplete`: a barge-in, a stop) is
    notified ("Kai's reply to it was cut off; it is in its chat"). A result that comes while a
    response of its thread runs is never asked for while that response runs (lmgw says the
    response answers it): Kai waits for its `response.done`, then asks at the next quiet moment
    with `empty_turn` the fallback, since `lmgw.task.done` and `response.done` are not ordered (the
    live check met it: a result that entered after the turn was saved came before the turn's
    `response.done`, and skipping the ask there left it unspoken). Every other result still gets
    its bare create at the next quiet moment.
  - **The quit** (low): `Core::quit` cancels the running jobs first and waits for their status
    notifications (all within `lmgw.timeout_s`, none while locked), so lmgw sees them
    `cancelled` before the link closes; the test drives the core's quit.
  - **Who acts on a job** (low): the owner, lmgw on the owner's behalf and Kai itself act on any;
    another caller only on the jobs it started, one lmgw did not name on none; refused before
    anyone is asked. `job_cancel`'s and `job_announce`'s dialog names the job's tool, its state and
    who started it.
  - **No microphone without the owner** (low): Kai's own bind for a result opens no capture (the
    tests count the captures); the owner's press or listening on in such a session opens it (it
    did not before: a press there talked into a session without a microphone).
  - **Smaller** (low): the message ids said are kept as long as the table keeps an ended job
    (`jobs.keep_ended_s`) and then pruned; the model's line is written after one poll of the
    handler, so an async routine's own `announce` is the one it states.
  - *T11, decided by the owner and built in lmgw (2026-10-09, `3fe2aff`; pinned since):* a late
    result is rendered in chronological order, its pair where its row is stored among the
    thread's messages, so a send after it follows it and the model answers the user with the
    result as context (it was `U [call R] R`, answering the result first). Nothing changes in
    Kai: it keeps speaking a result at the first quiet moment; a result that was notified
    (announce off, do not disturb, the voice elsewhere) or waited through a mute is now context
    for the next words, not answered before them. `just live-check-jobs` runs its owner jobs in
    one thread to show it.

**WP19. Approvals** (§11.1, K17, api §6; PD4's hand-over).
- Owns: `src/approvals/` (the interactive overlay on WP10's surface code, a Kai page on `kai://`,
  Enter and Esc, the time left; the voice answer through `…/transcribe` and `approvals.yes` and
  `approvals.no`), `assets/approvals/`. Hook lines: `src/voice/mod.rs` and `src/voice/control.rs`
  (no audio to the session while the overlay is open; the talk key records locally),
  `src/host/policy.rs` (an approved thread call skips PD4's dialog), `src/ui.rs`, `src/config.rs`.
- Depends on: M3, WP10, WP13 (c), WP18 (the voice files, in that order).
- *WP13 (c), draft decision:* layer-shell keyboard mode `OnDemand`. Focus moves to the overlay the
  moment it appears, so a key meant for another app would answer the approval: Enter and Esc count
  only after an arming delay, a visible setting with a default, or WP19 measures it; the overlay
  shows that it is not armed yet.
- Tests: no append and no commit while the overlay is open; an answer matching neither list keeps
  it; a decision made elsewhere and a cancel close it; `mcp_approval_response` then
  `response.create`.
- The owner: the spoken yes and no in German and English; the keyboard in a real session.
- *Built 2026-10-09 (`feat/approvals`, lmgw at 17d3b49), before WP18 (blocked on lmgw), so WP19 took
  the voice files' hook lines first; decisions where the spec was silent:*
  - **Where calls come from.** The feed's `approval.requested` (every thread the device sees) and
    the bound session's `mcp_approval_request`, one call per `approval_request_id`. The feed's
    `approval.decided` (an approved call that never ran counts as not approved), the session's
    `lmgw.approval.decided`, a deleted thread and leaving the gateway take calls away. Calls that
    waited before Kai's feed started are not shown (the feed starts without a cursor). *(Review,
    2026-10-09:)* a `resync` reads the waiting calls of every thread Kai knows calls of, and of
    its own conversation, again (`GET /chat/api/threads/{id}`, the messages' `pending_approvals`;
    lmgw-client has no request for it, so Kai builds it) and rebuilds them: a call decided
    meanwhile goes, one Kai missed comes, verdicts given for calls still listed stay; the card
    stays as it is while they are read.
  - **The card** (`src/approvals/overlay.rs`, `assets/approvals/`) shows one call at a time, oldest
    first: `Approve <label>__<name>?`, who asked (the starter of the thread's last `turn.started`;
    "Kai's voice session" for its own; *review:* by kind, never a bare name: "the owner (the
    dashboard)", `device 'phone'`, anything else "a principal lmgw names "…"", and a name or a
    title holding a quote is JSON-quoted, so a device named "Kai's voice session" or "the owner"
    reads as a device and no name can close its quotes and go on as the card's words) and in which
    thread (by title), the arguments in K22's
    compact lines (escaped like PD4's dialog, never shortened, scrolled when the output is too
    short), and "call 1 of 2 this reply waits on". It sits on its own layer-shell surface, OVERLAY
    layer, `[approvals]` `anchor = "top"`, `margin = 48`, `width = 560`, the height its content's up
    to the output's less the margins, on `overlay.output` (else KWin's active output). Its page is
    Kai's (`kai://approvals/`, script and style from its own files only) in an ephemeral web context
    that goes with the card. The tooltip says "an approval waits for …". *(Review:)* arguments
    that do not fit show a "more below" line; plain Enter waits until their end was in view
    (Ctrl+Enter does not). A card that cannot show (no layer-shell; its page ended a second time
    for the same call) says so back: the call is left to lmgw, the tooltip and a notification say
    "answer it in lmgw", the voice is not held back for it, and the next call that can show gets a
    new card (it was hidden for good before).
  - **No time left on the card.** lmgw sets no deadline on a waiting thread call: it waits until
    someone decides it or a new message declines it. PD4's dialog keeps `lmgw/timeout_ms` for calls
    without F.
  - **Arming** (WP13 (c)): `approvals.arm_ms`, default 600 (a visible setting; not yet tuned
    against real typing). Counted from the map, and again from every key that reaches the card
    before it armed, so a burst of typing meant for another window keeps it unarmed; the card says
    "Not armed yet" (the line keeps its place). Clicks before then are ignored too. A key's repeats
    count once, a key held from before the arming never counts, and a new call on the card arms
    again. The window takes the keys itself; its page gets none. *(Review, 2026-10-09:)* the wait
    also starts again at every `focus-in` (the keyboard arriving, whenever it does, not only the
    map) and at a button press before it armed (seen in the capture phase, before the page). **A
    key that reached the card before it armed for a call takes Enter's approval of that call
    away:** then Ctrl+Enter (the chord chosen; the card says so in its own line), a click or, for
    Kai's own call, the voice approves; Esc still declines. One timer per card, set again while
    keys move the time (was one per key). The card tells the approvals when it armed and when it
    waits again, per call (`CardEvent::Armed`/`Unarmed`): the voice answers only while armed.
  - **The keyboard flag** *(review)*: an in-process "the approval card holds the keyboard"
    (`src/approvals/keyboard.rs`), set at the map (`OnDemand` moves the keyboard then) and at
    `focus-in`, cleared at `focus-out` and when the card goes. Kai's input tools' focus guard
    refuses while it is set ("Kai's approval card holds the keyboard", at the start after waiting
    `dbus.timeout_s` for it to let go, mid-typing at once): KWin's window list never names
    layer-shell surfaces, so the window it calls active could be the confirmed one while
    `key_press`'s Enter would land on the card. Set from the map on, so a compositor that never
    sends the card `focus-in` keeps Kai's input tools waiting while the card shows: the safe side.
    *(Second review, 2026-10-09:)* set at `focus-in` only, no longer at the map: a card KWin never
    gives the keyboard left the flag set while it showed, and `type_text`/`key_press` refused
    meanwhile. Keys that reach the card between its map and its `focus-in` come before it armed
    and answer nothing. **The keyboard that arrives while the pointer is on the card** (the
    toplevel's enter and leave; a move onto the page's own window is no leave) came with a click
    on it (KWin gives an `OnDemand` layer surface the keyboard at its map and when it is clicked,
    before the click reaches it), so it no longer starts the arming again: the first click on an
    armed card the keyboard had left counts (it never did). A click before the arming still
    starts the wait again, and the card says it is not armed (`Arming::focus_in`).
  - **One reply's verdicts together.** Every waiting call of a thread belongs to its last reply,
    and lmgw takes them in one request (api §6.3): the card asks each, then sends all. They go
    through the bound session when it is the session that showed every one of them
    (`mcp_approval_response` each, then `response.create`; the resumed reply is spoken), otherwise
    through `POST …/approvals` with Kai's device key, whose resumed-turn stream Kai reads to its end
    (no bound of Kai's own: a request that stops reading stops the turn). A decline's reason is
    "declined on Kai". A refusal beyond the device's reach (`approval_out_of_scope`) keeps the call
    on the card: "Decline here, or approve in lmgw.". *(Review, 2026-10-09:)* sent verdicts keep
    their calls ("sent", off the card) until lmgw took them: the route's answer; for the session,
    the feed's `approval.decided`, the session's `lmgw.approval.decided`, or the end of the first
    response created after them (`response.done` not failed; the answering session gets no
    `lmgw.approval.decided` for its own answers). Every other refusal (`approval_decided`,
    `approval_moved_on`, `approval_not_found`, `approval_missing`, `approval_starter_unavailable`,
    `key_rate`, no answer in time, which lmgw may have taken), a session `error` with an
    `approval_*` code, and a failed response that carried them read the thread's waiting calls
    again before anything is asked: what still waits is asked again ("Answer again"), what went is
    said in a notification when lmgw said why, and a batch is never dropped whole for one stale
    call. A session that ended before lmgw took them sends the same verdicts by the route. A new
    call of the same thread while a batch is on its way is a batch of its own. *(Second review,
    2026-10-09:)* the session's `response.create` carries an `event_id` (`kai_approvals_<n>`),
    and an `error` echoing it (`error.event_id`, `SessionEvent::ErrorFor`) is lmgw's refusal of
    it whatever its code (`conversation_already_has_active_response`, `gpu_hold`, a model's
    error): read again like an `approval_*` code, so the verdicts no longer wait for a response
    that never comes (the card hidden, no `lmgw.approval.decided`, the grace close held, and the
    owner's next turn's `response.created` taken for theirs, whose words lmgw lets decline what
    waits). Only a `response.done` with status `completed` counts as taken; `failed`, `cancelled`
    and `incomplete` read the thread again (nothing waiting then is said nowhere). After a
    session ended, the route's `approval_decided` for the verdicts it resends means lmgw took
    them in the session (the resumed reply had started): taken, no "not taken" notice.
  - **The spoken answer (K17).** "Spoken briefly or shown": shown. Kai has no voice of its own; the
    session's voice is the thread's reply. *(Review, 2026-10-09: Kai's own calls only.)* While a
    call **of Kai's own** waits (one its bound session showed, or one in the conversation's
    thread), no audio goes to the session (no append, no commit, no pre-roll; a hold that began
    before is cleared, not committed; audio buffered for a session is dropped). *(Second
    review:)* nor while its verdict went through the session and lmgw has not said
    `response.created` for it (a turn committed then would be taken for it); a talk-key hold that
    began while the gate was up stays silent and commits nothing to its release, also when the
    gate lifts while it is held (it would commit a clipped turn: `Key::gated`); and a call an
    ended session showed counts as Kai's own only through the conversation's thread. A press of the talk
    key records the answer only when the card shows such a call **and said it armed for that call**;
    a press that began before is the talk key's own press (its audio still held back). Calls of
    other threads (a phone's turn, the dashboard's) show on the card and are answered there; they
    hold back no audio, leave the talk key and conversation mode as they are, and their card says
    no word of the voice. So the owner talking to Kai while a phone's turn parks on a call in
    another thread cannot approve that call by saying "ok". An answering press starts with the
    pre-roll (a reply still playing stops; muted, nothing records; a press while the last answer is
    still transcribed waits). **The recording must hold speech** *(review)*: at least
    `approvals.speech_ms` (150) of 20 ms frames at or above `approvals.speech_dbfs` (-40 dBFS RMS),
    both visible settings, not yet tuned against real microphones; else the card says "Kai
    heard no speech" and nothing is sent (a recogniser makes "Okay." or "Ja." of near silence). The
    release sends the WAV to `…/transcribe` of the call's thread. **The match is the whole utterance:** lower-cased, apostrophes dropped, every other
    character that is no letter or digit a space, then compared with each phrase of
    `approvals.yes` and `approvals.no` normalised the same way (`src/approvals/words.rs`; German
    and English defaults; a phrase in both lists is a configuration error). "Ja." approves; "mach
    es nicht" is in neither list, so a dropped word cannot turn a no into a yes. *(Review:)* the
    default yes list holds only phrases that say yes on their own ("yes", "yes please", "approve",
    "approved", "go ahead", "do it", "ja", "ja bitte", "jawohl", "genehmigen", "genehmigt",
    "freigeben", "mach das", "mach es"); the filler words "ok", "okay", "sure", "yeah", "yep",
    "genau", "okay mach" and "mach" went, and "nicht" left the no list. Neither: the card
    stays and says `Heard “…”: say yes or no, or click.` A press session's grace close waits while a
    call it showed waits.
  - **K15 (PD4's hand-over) and self-approval.** A trusted approver's approval decides the call
    (as WP8 built it), except **an approval whose approver is the call's own caller, unless that
    caller is the owner, the gateway or Kai's own key** (`Policy::approval_counts`): lmgw stamps
    every real decision and leaves this to the device (client-apps §6.6). Kai's own card approves as
    Kai's device key, a trusted approver. `routine_enable {on: true}` still ignores F (WP20).
    *(Review, 2026-10-09:)* **K24's confirmed-by-default tools (`type_text`, `key_press`,
    `ui_press`, `run_command`) keep PD4's dialog after any thread approval**, Kai's card included
    (`policy::approval_applies`): their dialog names what the call acts on, resolved just before it
    runs (the focused window, the element; WP16c's preparation), and the card cannot show that line
    (the call reaches Kai only after the approval, and the keyboard is the card's while it shows),
    so the dialog stays; `allow` in `policy.tools` still skips it. *(Second review, 2026-10-09:
    reverted, a draft of 2026-10-09:)* a trusted approval, Kai's card
    included, decides K24's tools again as WP8 built it (K15's notes: a remote approval would time
    out with nobody at the desk, and a call approved in lmgw names its window since ff1b9e8, so the
    card's arguments show the target); `routine_enable {on: true}` still asks. **The limit of the self-approval
    rule:** it compares the approver with the call's caller by principal. A trusted approver that
    holds a second principal's key (another device key, a client key it was given) can start a turn
    as one and approve as the other, and Kai cannot tell; that trust is the owner's to give, by
    keeping `policy.trusted_approvers` to principals that hold no other key.
  - *Verified, 2026-10-09* (`just live-check-approvals`: a nested KWin, a private
    PipeWire, a dev lmgw with `TOOL_SCRIPT` calling the live-test build's state-changing
    `live_note`, the speech-to-text mock answering "Ja."): the card 560×220 at the top of the
    output (a crop in `target/live-approvals/shots/`); an Enter 6 ms after KWin listed it answered
    nothing; Enter after the arming delay (1500 ms in the check) approved through the route, and the
    resumed turn ran `kailive__live_note` 114 ms after the key with no dialog; Esc declined (the
    tool did not run); a second device in `policy.trusted_approvers` approving its own call met
    Kai's dialog (denied, the tool did not run); in a push-to-talk press session the
    `mcp_approval_request` showed the card, no append or commit went for 1.8 s of waiting, the
    spoken answer went to `…/transcribe` and `mcp_approval_response` followed 12 ms after the
    release, `response.create` right after it, the resumed reply ran the tool with no dialog; no
    device key and no tool argument in Kai's trace log; no web process left.
  - *Re-verified after the review, 2026-10-09* (`just live-check-approvals`, kai at the review's
    fixes, lmgw main ab36ef1, one commit past the pin 17d3b49): KWin's nested session sends the
    card `focus-in` 2 ms after the map (the arming restarts from it); an Enter 6 ms after the map
    answered nothing, a plain Enter after the arming delay answered nothing either (a key came
    first; the card's orange line said so), Ctrl+Enter approved through the route (the tool ran
    115 ms after the chord, no dialog); Esc declined; the self-approval met the dialog; in the
    bound session a talk press right as the card showed was talk (nothing transcribed, nothing
    sent), the press after the arming answered by voice ("Ja.", `mcp_approval_response` 11 ms
    after the release); with an owner's thread's call on the card the keyboard flag was set and
    Kai's `key_press` refused ("Kai's approval card holds the keyboard …; 0 of 1 chords
    pressed"), and a push-to-talk turn in Kai's conversation streamed and committed meanwhile;
    Esc then cleared the flag. `just live-check-voice-lmgw` and `just live-check-conversation`
    pass on the same build. Not verified: KWin on the owner's own session (focus at the map beside
    other `OnDemand` surfaces or a fullscreen window), the speech threshold on a real microphone, a
    card that cannot show (no layer-shell, a second crash: unit tests only), and a session
    `error` with an `approval_*` code from a real lmgw (unit tests only).
  - *Second review's fixes, 2026-10-09* (besides the notes above): **PD4's dialog text**
    (`host::confirm`) shows the invisible parts of an emoji as the emoji (UTS #51): a variation
    selector after an Extended_Pictographic base (❤️, ⚠️, ✔️) or in a keycap, a ZWJ between two
    emoji (👨‍👩‍👧), a subdivision flag's whole tag sequence (🏴 + tag digits and small letters + the
    cancel tag); a lone or stray one is still escaped, in the dialog and in the text the model
    gets. **A dropped command's scope** (`run_command` cancelled, WP16a's `Scope`) dies by its
    cgroup v2 `cgroup.kill` (the user manager delegates the user's cgroups): read from the
    leader's `/proc/<pid>/cgroup` once the scope holds it, and only a path whose last part is the
    unit, never Kai's own; one write kills every process in it, `setsid` and reparented ones
    included, with no fork past it. Without one, the process group and then `KillUnit` (the walk
    of `/proc` below the leader is gone). The fake manager of the tests gives each scope a real
    child cgroup of the test's own where it may.

**WP20. Routines** (ruling 25, K28).
- Owns: `src/routines/` (the directory watch, `routine.toml`, the kinds steps, goal and script, a
  script in its own scope with stdout as the result), `src/cli/` (`kai tool`, `kai model`; the
  `cli.sock` server with peer credentials and the cgroup naming the routine). Hook lines:
  `src/main.rs` (the subcommands, USAGE, the socket), `src/host/registry.rs` (routines as a
  provider of entries), `src/shortcuts.rs` (a routine's key binding), `src/tray/` (Routines),
  `src/config.rs`.
- Tools: each enabled routine; `routine_create` (a delegate, K25).
- Depends on: WP8, WP16a (scopes); WP17 for the goal kind and `routine_create`. Schedules wait for
  §16 Q16.
- Tests: a change in the directory sends `list_changed`; a script's call reaches K15 as the routine;
  a terminal's call is the caller `cli`; the script's environment holds no secret; the exit code
  decides done or failed; a draft is disabled until enabled.
- The owner: enabling a script (confirmed by default).
- *Built 2026-10-09 (`feat/routines`), decisions where the spec was silent:*
  - **The folder:** `$XDG_DATA_HOME/kai/routines/<name>/routine.toml`; the folder's name is the
    routine's and its tool's (K22's name rule, not one of Kai's own tools; a `name` key must match).
    Keys: `description` (one line), `kind` (`script`, `steps`, `goal`), at most two `[[params]]`
    (`name`, `kind` text, integer, number or boolean, `optional`, `description`), the annotations
    `read_only`, `destructive`, `open_world` (default false), `policy` (K15's default: `hints`,
    `trusted` or `always`; a routine cannot set itself to `allow`, that is the owner's
    `policy.tools`), `level` (default `any`), `async`, `announce`; a script's `script` (default
    `run.sh`), a steps routine's `[[steps]]` (`tool`, `args`), a goal's `goal`, `tools` and
    optional `alias`. Each spec passes the contract's `check` like every tool; one that does not
    load is named in the log and in the tray with its reason. Specs are `&'static` like the
    contract's: interned, so a reload of the same text takes nothing, and only a changed text adds
    its new version for Kai's life.
  - **On and off:** the switches are `[routines.enabled]` in Kai's configuration, not in the
    folder (the folder is written by agents, so nothing in it can switch itself on). Every routine
    starts off, a draft included. `routine_enable {name, on}` (contract `routines`) is
    owner-level, confirmed by default for every caller (K24's class, settable to `allow`), and a
    configuration tool (K30: listed only with `policy.config_tools`); its dialog shows
    `routine.toml` and a script's file in full, and a routine that does not load cannot be
    switched on. The tray's switch calls it as the owner whether it is listed or not; switching
    off from the tray asks nobody. WP21a's operations: `Request::Routine {name, enabled}` writes
    `true` (or removes the key), `Set` refuses `routines.enabled.*` (pointing at `routine_enable`;
    `false` passes since the second review fixes),
    `Unset` is allowed, the effect is `now`; `known_tool` knows the routines. F's approval does not
    switch one on (the review fixes below). *Decided by the owner 2026-10-09: no pinning; the
    latest version always runs* (the pin of the review fixes below is withdrawn; see there).
  - **The list:** the routines' provider lists `routine_create`, `routine_enable` and every routine
    that is on, loads and is synchronous. `async = true` waits for WP18's jobs, as the async
    delegates do (the hook is in `Provider::entries`; `announce` is read and kept for it). inotify
    watches the routines' folder, each routine's folder (not deeper, so a routine that is its own
    git repository does not rescan on every git operation) and the configuration's folder; a burst
    is read once after 150 ms of quiet, and `list_changed` goes out only when the list changed.
  - **Script:** its own app scope `app-dev.rootshell.kai.routine.<name>-<random>.scope` (WP16a's
    spawn), the parameters as positional arguments in declared order (an omitted optional one is
    an empty argument), the routine's folder as its working directory, stdin empty, the session's
    environment (the user manager's) plus `KAI_CLI_SOCKET`, `KAI_ROUTINE` and Kai's own binary's
    folder first on `PATH` when the `kai` found there is another; no key or other secret. stdout is
    the result (empty: `done`), never cut: larger than the call's room in lmgw's message limit is
    the call's error naming the limit; stderr goes to the journal as `kai-routine-<name>`. Exit 0
    is done; anything else is the tool's error ending `failed: <name> exited with <n>`. Its time is
    lmgw's wait less `run_command`'s answer margin; a call with no wait (the CLI, the tray, a key)
    runs until it ends or its caller goes, and a cancel kills the whole scope. (The review fixes
    below bound what is kept, stream it to the CLI and give the tray Stop and a deadline.)
  - **Steps:** in order, each a call of Kai's own by the routine through the host's one path (K15,
    the confirmation, the handler); `{param}` in a text is the parameter's value, a text that is
    only `{param}` the typed value (left out when omitted). The result is one line per step
    (`<tool>: <its answer>`); the first failing step ends the run and is named.
  - **Goal:** K25's loop with the routine's `goal` as its instructions, its `tools` (those Kai
    offers now), its parameters as the task's, on its `alias`, else the Kai conversation's;
    `delegate.max_turns` bounds it; a plan is confirmed once (PD4).
  - **Callers:** a routine's own calls (steps, goal, its script's `kai tool`) are `routine:<name>`;
    a `kai tool` from outside a routine's scope is `cli`. Both are untrusted by default; the trust
    lists take `routine:<name>` and `cli`. A routine does not start routines: a routine tool called
    by a routine is refused, so none can recurse. The tray's Run and a routine's key are the
    owner's own calls.
  - **The CLI:** `$XDG_RUNTIME_DIR/kai/cli.sock` (`KAI_CLI_SOCKET` overrides it, for the tests and
    the live check), 0600, its folder 0700 when Kai makes it, the peer's uid must be Kai's. One
    JSON line each way: `{"op":"list","name":…}`, `{"op":"tool","name":…,"args":[[param,value],…]}`
    (each value read by the parameter's type; a list parameter takes its flag once per item),
    `{"op":"model","alias":…,"prompt":…}`; the answer `{status: done|failed|usage, text, notes}`.
    The client keeps its side open, and its end cancels the call (a script it started is killed).
    The caller is read from `/proc/<pid>/cgroup`: a path part `app-dev.rootshell.kai.routine.<name>-
    <random>.scope` is that routine, anything else `cli`. The cgroup identifies, it does not
    authenticate: a process of the same user could join it, as it could edit the routine. `kai
    tool` alone lists the offered tools, `kai tool <name> --help` one tool's parameters. Exit codes:
    0 done, 1 the tool or model failed (the reason on stderr), 2 a usage error, 3 no Kai answers.
  - **`kai model <alias>`:** the prompt from stdin as one user message to `/v1/chat/completions`
    under Kai's device key, not streamed, the answer's text on stdout; a fallback that answered is
    said on stderr. Kai's tool policy does not apply (not a tool); lmgw's scope for the key does.
  - **The host:** `Shared::dispatch` is the one path for lmgw's calls and Kai's own
    (`host::local::Local`: no approver, no deadline, the caller's cancellation). The routines'
    runtime (`routines::serve`: the socket, the watch, the tray's and keys' triggers) starts in
    `main.rs` and finds its provider through `Provider::as_any`, so `Deps` has no new field.
  - **Triggers:** the tray's Routines submenu (before Check for Updates while any routine folder
    exists): Run for each routine that is on, synchronous and needs no parameter, and a check item
    per routine (its load error or why it cannot run from the tray in its label). The global
    shortcuts bind one action `routine-<name>` per such routine when Kai starts, with no key
    (§8.1); a routine switched on later gets its action at the next start.
  - **`routine_create`:** a delegate with one tool of its own, `draft_write {path, content}`, into
    a folder the task made (never an existing routine; every file but `routine.toml` executable);
    its task message carries the goal, Kai's tools as offered now (as `kai tool` lists them) and
    the existing names; the alias is `routines.create_alias`, else the Kai conversation's. The
    answer names each draft and that it is off.
  - **Tests:** the folder's change sends `list_changed`; a script's call reaches K15 as the routine;
    a terminal's is `cli`; the script's environment holds no secret; the exit code decides; a
    draft is off until enabled, and enabling shows the script in full. `just live-check-routines`:
    a script in a real scope of the session's user manager calls a test tool through the real
    `kai` binary and the test's own socket, the real cgroup names it, the terminal is `cli`, and no
    routine scope is left.
- *Review fixes (2026-10-09, `feat/routines` after the rebase onto WP21b; taken in this draft). Each changes behaviour that the notes above describe:*
  - **The switch is a plain on/off; no pinning.** *Decided by the owner 2026-10-09: no pinning; the
    latest version always runs.* This withdraws the first version of this fix (the switch held the
    SHA-256 of the folder and any edit left the routine off as "changed since switched on"). Now
    `routines.enabled.<name>` is `true` or `false`; an existing `"sha256:<hex>"` value from that
    version reads as on (the digest is no longer checked) and is never written again. An edit after
    switching on, to the script, `routine.toml` or a new file, takes effect without a second look: the
    watch reads the folder, a run uses the folder as it is. There is no folder hash, no rescan
    mismatch, no "changed since switched on" or "needs review" tray state, and a run no longer reads
    the folder before it starts. Kept: `routine_enable`'s preparation reads the folder once, parses
    `routine.toml` from those bytes (it must load) and shows them in full: `routine.toml` first,
    then the entry script, then the rest by name; a file in full when it is text (UTF-8 without NUL),
    otherwise as `not text, <size> bytes, sha256 <hex> (not shown)`; a subfolder as not shown; a pipe,
    socket or dangling link as not read. Choice: helpers in full rather than by hash, since a
    sourced helper is code the owner must read. A file is opened without blocking and looked at
    before it is read, so a pipe never hangs the scan. The switch-on dialog is built from the bytes
    it read, and the routine must load from them.
  - **Orphan switches:** a switch whose folder is gone is listed in the tray (`Forget the switch of <name> (its
    folder is gone)`), which removes the key. `routine_create`'s `draft_write` refuses a name the
    configuration has any switch for (`false` included), and its task message lists those names as
    taken.
  - **F does not switch a routine on.** `host::policy::approval_applies`: for `routine_enable` with
    `on: true` the thread's approval is dropped before the policy decides, so Kai's dialog (which
    shows the routine) is asked; only `policy.tools.routine_enable = "allow"` (ruling
    28) skips it. `on: false` and every other tool go by F as before.
  - **`routine:<name>` only in a scope Kai runs** (*decided by the owner, 2026-10-09:* kept).
    Kai marks each script's unit name in a set of
    live scopes before the spawn and removes it when the run ends however it ends
    (`routines::LiveScopes`); the CLI grants `routine:<name>` only for a peer in one of them, any
    other routine-named scope (`systemd-run --user --scope --unit=app-dev.rootshell.kai.routine.x-1
    kai tool …`, or Kai's after its run) is `cli`, logged. The peer is read through its pidfd
    (`SO_PEERPIDFD`, Linux 6.5 and later): the pid from its fdinfo, the cgroup file, then the pidfd
    must still be alive, so a pid reused in between is not read; on an older kernel `SO_PEERCRED`'s
    pid with that race, said in `crate::cli`'s doc. Still identification, not a boundary: a process
    of the user can move itself into a running routine's scope (the user manager delegates those
    cgroups to the user). `[policy]`'s doc and the tooltip (while `cli` is in
    `policy.trusted_callers` or `policy.trusted_approvers`) say that trusting `cli` or a routine, or
    `allow`ing a tool, hands it to every program of the user. "A routine does not start routines"
    reads the caller and is said to be identification: `systemd-run --user kai tool <routine>` from
    a script is `cli`.
  - **Bounded memory.** A script's stdout is kept only within the call's room: lmgw's message limit
    for a tool call (as before), a notification's room for the tray and a key
    (`notify::BODY_ROOM`: the D-Bus specification's 2^27-byte message less 64 KiB for the call's
    other fields, divided by five for escaping, about 25.6 MiB), the run's error naming the size
    when more came (`<name> wrote N bytes to stdout, more than its result may take (<the room>)`,
    the lmgw wording changed to this form). For the CLI the stdout streams: `Call::out` (a channel
    of four pieces of at most 64 KiB, a pace, not a bound), Kai writes `{"stdout": <base64>}` lines
    before the answer and keeps none of it; the client writes them to its stdout as they come;
    the answer of a streamed script is empty on success and `failed: <name> exited with <n>` (stdout
    already printed). A client that cannot take more is drained, and its going cancels the call. A
    call with neither room nor stream (only a delegate's loop, which cannot reach a routine: a
    routine's caller is refused) would still keep everything. The CLI's request line is bounded by
    the host link's message limit as Kai knows it (`Shared::limits`: `[host]`'s fallbacks, then
    lmgw's announcement), a larger one refused with the usage exit code naming the limit; with the
    owner's 0 (no message limit) it is unbounded.
  - **Tray and key runs:** each is tracked while it runs; the tray offers `Stop <name>` for it,
    which cancels it (a script's scope is killed) and notifies `stopped from the tray's Routines
    menu` (*decided by the owner, 2026-10-09:* the default 0 with the tray's Stop is kept).
    `routines.run_timeout_s` (default 0: no deadline, stated in the section's doc; read live
    with the switches, the operations' effect `now`) stops a tray or key run after that long,
    notifying `it ran longer than routines.run_timeout_s = N s`. A state-changing routine run from
    the tray is still confirmed like any owner call of it (K15).
  - **The configuration window** (WP21b) refuses `{"op": "routine", "enabled": true}` with a 403
    until the window shows routines; switching one off passes. `settings/mod.rs`'s doc says so.
  - **Kai's own windows are never acted on** (cross-cutting, `desktop::resolve::not_kais`): a
    window whose app id is `dev.rootshell.kai`, or whose process is Kai or descends from it (its
    parent chain in `/proc`, so the WebKit processes at any depth), is refused by every window
    action, by `type_text` and `key_press` when it has the focus, and by `ui_press`, before the
    confirmation and at the call; `window_list` and `see_window` still read it.
  - **Tests:** an edit after switching on (the script, `routine.toml`, a new file) stays on and the
    next call runs the new version, with no note in the tray; the switch is written as `true`; the
    dialog's lines (entry first, a helper in full, a binary by hash, a subfolder, a pipe); an old
    pin and a plain `true` read as on; an orphan switch is listed and forgotten from the tray; F's approval still asks; `draft_write`
    refuses a switched name; the window's route; `approval_applies` and the `cli` note; the live
    scopes and the pidfd (a gone process); the tray's Stop and deadline; a result over the room;
    the CLI's stream and request bound; Kai's own windows over fakes. `just live-check-routines`
    adds a real `systemd-run` scope of the routine's name, which is `cli`, and `kai tool
    live_probe` from the terminal streaming its script's stdout.
- *Second review fixes (2026-10-09, `feat/routines-unpin-widen`; taken in this draft):*
  - **A routine is its folder, now.** With no pin (the owner's decision), a switched-on routine
    runs whatever its folder holds now, so it is exactly as trustworthy as everything that can
    write its folder; `routine:<name>` identifies which routine called, not its content. Said in
    `routines`' and `[policy]`'s docs.
  - **`routine.toml` gets at most confirm and the owner's level** (K28's draft): `def::parse`
    checks the spec as declared, then gives the routine `Ask::Always` where it asks for unasked
    runs of trusted callers (`policy = "trusted"`, or `read_only = true` under `hints`) and caps its
    level at `owner`; `Routine::asks` keeps the rest (`def::Asks`: `allow_trusted`, a level
    `trusted` or `any`, an absent level being `any`). The `readOnlyHint` stays as declared; the
    contract's rule now allows read-only with `Ask::Always` (only read-only with `Ask::Trusted` is
    refused). Consequence: `kai tool <routine>` from a terminal (the caller `cli`), another device
    or a key is refused by the level until the owner grants a wider one, and a read-only routine is
    confirmed even for the owner until granted `allow_trusted`.
  - **The grant box.** `routine_enable`'s dialog starts with what the routine asks ("routine.toml
    asks for more than Kai gives a routine …") and adds a checkbox, unticked: "Also grant what it
    asks: policy.tools.<name> = "allow_trusted", policy.levels.<name> = "any"" (less the keys the
    owner set already; none when nothing is left). Ticked with Allow, the handler writes those
    entries as the owner, then the switch, and its answer names them. The box is
    `host::confirm::DialogOption` (a `Prepared` field passed to `ConfirmRequest`, a GTK
    `CheckButton` in the tool dialog, cleared on a denial); without a dialog (`routine_enable` set to
    `allow`) nothing is granted. K15's fifth state `allow_trusted` exists for it.
  - **Writes through the core.** The routines write the configuration (the switches, the grants,
    an orphan's `false`) with `Command::Setting` (`routines::Settings::Core`), the core's one
    writer, so a switch never loses a concurrent owner's narrowing to a read-modify-write and a
    grant reaches the host's policy at once. The tests write the file (`Settings::File`).
  - **Orphans become `false`.** A rescan that finds a switch on whose folder is gone writes it
    `false` (`Set` takes `false` for `routines.enabled.*`; never `true`): the name stays taken and
    listed to forget, and a new folder of that name, anyone's, is off until the dialog.
  - **`routine_create` and the switch.** `draft_write` reads the switches at every write (not once at
    the task's start), so a draft switched on meanwhile takes no more writes; `routine_enable`
    refuses a name a running task writes (`routine_create is still writing <name>`) until the task
    ends (`create::Drafting`, released when its tools are dropped, a cancel included).
  - **Third review fixes (2026-10-09, same branch):**
    - **One box per grant.** `Prepared.options` is a list; the GTK dialog shows one unticked
      `CheckButton` per grant, `Shown` pairs grants and boxes, and only ticked grants are written
      (a read-only routine without `level` has two: `allow_trusted` and the level `any`).
    - **The owner's own run is not asked.** A tray or key run calls `Local::call_approved` with the
      owner as approver when the routine asked `allow_trusted` (read-only, or `policy = "trusted"`)
      and the owner has no `policy.tools.<name>` entry; a routine that changes state keeps its
      tray confirmation; model, gateway, device and cli calls are unchanged. The dialog reason for
      a routine reads "routine.toml asks at most for confirmation (K28)".
    - **A failed scan switches nothing off.** `routines::scan` returns a `Result`; on an error
      other than NotFound the routines and switches stay as they were and no orphan is written,
      and a name becomes `false` only when `symlink_metadata` of its folder says NotFound.
    - **Grants and switch together.** The settings layer has no batch request, so the handler
      writes the ticked grants then the switch and, when any write fails, unsets the grants it
      wrote (they are keys the owner had not set) and says so.
    - **`allow_trusted` on `routine_enable`** no longer skips the dialog for a switch-on: only
      `allow` does (`Policy::decide_call`); switching off is unchanged.
    - **Tests:** `switches.rs` (each grant its own box, a failed switch takes the grants back,
      an unreadable folder, `allow_trusted` vs `allow`, the reason text), `tray.rs` (owner runs),
      `confirm.rs` (several boxes), `widen_tests.rs` (the boolean).
  - **Tests:** `switches.rs` (a new folder under an orphan's name is off; a draft is not switched
    on while its task writes it, and a switch written meanwhile stops its writes; the grant only by
    the box, the owner's entries then decide, cli refused until granted), `def.rs`'s caps,
    `confirm.rs`'s checkbox model, `policy/tests.rs`'s `allow_trusted`, and `widen_tests.rs`
    (below, WP21a). `just live-check-routines` grants its probe `policy.levels.live_probe = "any"`
    for the terminal's call.

**WP21. The configuration window and the owner tools** (rulings 26 and 27, K29, K30).
- **21a, the operations:** every setting as an operation in the core, written through §2's
  configuration write; the policy table, the capability overrides, the routines' switches; K30's
  tools from the same operations behind `config_tools` (off by default, §16 Q17). Owns
  `src/settings/` and `contract/config.rs`; one arm in `src/core/calls.rs`.
- **21b, the window:** `ui/` (Leptos, trunk, embedded), a GTK toplevel with its own ephemeral
  `WebContext` reaching the operations through the scheme handler only; the tray's Settings item;
  trunk and the wasm target in `packaging/build-rpm.sh` and CI. Owns `ui/`,
  `src/settings/window.rs`, its `src/ui.rs` and tray hook lines.
- Depends on: WP8 and the WPs whose settings it shows (21a grows with them); 21b on M5 for the
  shared crate, or `app.css`'s tokens at the pin until then.
- Tests: an operation round trip keeps the file's comments (§2); the scheme refuses other origins;
  switching `config_tools` changes the list once.
- The owner: the window's look (lmgw's native-feel rules, at the default interface scale, 125 %).
- *As built, 21a (2026-10-09; taken in this draft):*
  - `src/settings/` is one function, `apply(file, Request) -> Result<Outcome, Refusal>`, serde in and
    out, no core or GTK in it: list, get, set, unset, plus tool state, tool level and provider
    (capability override) as typed shortcuts for `policy.tools.<tool>`, `policy.levels.<tool>` and
    `desktop.<capability>`. The window (21b) calls the same function through the scheme handler.
  - It is generic: the effective values are `Config`'s serialization (so every config struct derives
    `Serialize`), a write is the edit of §2's write on the file's text, accepted only if the whole
    result loads again (`Config::parse`), and the refusal is the loader's message. A section a later
    WP adds needs nothing in `src/settings/`.
  - A value is TOML text (`30`, `true`, `["a"]`); text that is none is a string, and so is a text
    that a string-typed key needs (`5` for `audio.input`). A table value and a multi-line value are
    refused.
  - Effects: `policy.*` applies at once (the core builds the policy again, the host lists again,
    `list_changed` only when the list changed); every other key is read at Kai's next start. The tray's
    own live switches keep their path.
  - `config_tools` is `policy.config_tools` (default false). Off: the seven tools are not listed
    (`Policy::listed`); on, they are owner-level and confirmed unless set to `allow` (ruling 28). Owner level for these
    tools means the owner and Kai's own key (K30) and not `Caller::Gateway` (dashboard turns,
    in-process agent runs, internal keys), unlike K15's reading for other owner-level tools: a
    per-tool check in `Policy::decide` (`is_config_tool`), no new level word. Widening
    `policy.levels.<tool>` (ruling 28) admits the gateway again.
    The tools are `config_list {section?}`, `config_get {key}`, `config_set {key, value}`,
    `config_unset {key}`, `tool_state_set {tool, state}`, `tool_level_set {tool, level}`,
    `provider_set {capability, provider}`; `default` as state, level or provider removes the key.
  - Secrets: none is in the file (the device key is in the Secret Service). A key whose name has
    the word key, token, secret, password, passphrase, bearer or credential is neither listed, read
    nor written, matched per word in any case and split at `_`, `-` and `aB` (`API_KEY`, `apiKey`;
    the words also include apikey, passwd, pwd, auth, authorization) (a tool's name under
    `policy.tools` and `policy.levels` excepted); a test fails if a future key looks
    like one. `lmgw.url` is read-only here: pairing writes it.
  - **Config tools do not widen permissions by default** (*decided by the owner 2026-10-09:* config
    tools may not widen permissions by default, but the user can allow it per tool, since that risk
    is the user's judgement; K30).
    `settings::apply_from(file, request, Source)`: `Source::Owner` (the settings window, the tray's
    routine switch, `settings::apply`) writes as before; `Source::Tool` (the seven config tools,
    `Core::config_tool`) checks the result of the write against the file before it
    (`src/settings/widen.rs`, on the loaded `PolicyConfig`s, so every route to a key, the typed tools
    and `config_set` alike, is covered). A write WIDENS when: a tool's state rises (off and deny, then
    confirm, then "allow if trusted", then allow; an unset counts as the tool's default, so taking
    away a deny can widen), a tool's level rises (own, owner, trusted, any; an unset counts as the
    contract's level), an entry is added to `policy.trusted_callers` or `policy.trusted_approvers`,
    or `policy.config_tools` goes on. Narrowing always passes, and so does every key that is not a
    permission. A refused widening writes nothing and its message names the opt-in. The opt-ins:
    **`policy.widen_by_tools.<tool> = true`** lets the tools widen that tool's state and level;
    **`policy.widen_trust_by_tools = true`** lets them add trusted callers and approvers and switch
    `config_tools` on. Both default off (absent) and are the owner's alone: a write of either to
    true from the tools is refused (`only the owner permits widening`), setting them back to false
    or removing them is narrowing and passes. The opt-in is judged on the file before the write, so
    one call cannot grant itself. The window sets them (a "Config tools: may widen this tool"
    checkbox in each Tools row; the trust opt-in is a flag in the generic list) and so does the
    file. Tests: `src/settings/widen_tests.rs` (refused by default with the message, permitted per
    tool, narrowing always ok, a returning default counts, the opt-in unwritable by the tools,
    the owner path widens and the opt-in then works), the core's tool round trip and the window's
    path, and `ui/`'s `flag_on`.
    *Review fixes (2026-10-09; taken in this draft):* an unset counts as the default Kai applies,
    the contract's or a loaded routine's spec (`widen::offered`, through `routines::spec`): a
    routine's is at most confirm and owner (WP20), so `config_unset policy.levels.backup` is judged
    against `owner`, not the `any` its `routine.toml` asks. A name Kai offers no tool for counts as
    the narrowest before a write and the widest after, so setting anything but `deny` or `off` on
    it, or unsetting it, widens. `launch.terminal` (argv put before a `Terminal=true` application's
    command, `desktop::fdo::launch`) is `run_command`'s power: any change by a tool needs
    `policy.widen_by_tools.run_command` (`widen::COMMANDS`; no other key holds a command, URL or
    path Kai runs). The state ladder gains `allow_trusted` between confirm and allow. Tests: routine
    shapes (level unset, `policy = "trusted"`, `read_only`, `own`), unknown names, the terminal, and
    a real level widening permitted per tool (`config_set` to `any`).
  - The configuration write's temporary file is created per write (process id, a counter, the clock)
    with `O_EXCL` in the target's directory, so concurrent writers (the core, `pairing.rs`) never
    share one and a planted link is never followed; then renamed.
  - The routines' switches (WP20) are `routines.enabled.<name>`, written by the `Request::Routine`
    arm (`true` after the caller showed the routine in full; no pin, the owner's decision of
    2026-10-09, see WP20), and `Set` writes only `false` there (an orphan's, WP20); `known_tool`
    knows the routines; the window's
    route refuses switching one on until the window shows routines (WP20's as-built notes).
- *As built, 21b (2026-10-09; taken in this draft):*
  - **The window.** `src/settings/window.rs` (children `assets`, `route`, `tools`): a plain
    `gtk::Window` toplevel with a `WebView` in its own ephemeral `WebContext`, opened by the tray's
    `Settings…` (`Command::OpenSettings`, `UiCmd::ShowSettings`; raised when open), dropped on close
    (the web processes go with the context). No header bar is set, so GTK asks KWin for server-side
    decoration (`org_kde_kwin_server_decoration`, mode 2, seen with `WAYLAND_DEBUG=client`) and the
    frame is KWin's. The NVIDIA rule is the process-wide `nvidia::apply` of `main()`; nothing of
    its own. 1100×760 logical, less on a small screen (94 % of the primary work area).
  - **Scheme.** The context serves `kai://settings/` and nothing else: the page's files, `GET
    /api/tools` (the contract's tools with area, description, default state and level) and `POST
    /api/apply` (a `settings::Request` as JSON in, the `Response` out, or 422 `{"refused": text}`
    with the loader's sentence). The operation runs in the core (`Command::Setting`, the same
    `apply` as the owner tools, so the core's configuration and the host's policy follow). `route`
    is a function of the request alone: another authority is 403 and runs nothing, an `Origin` or
    `Sec-Fetch-Site` that is not this origin is 403, a body that is no `Request` is 400, any other
    path or method is 404 or 405. The view refuses every navigation, window, permission and WebKit
    context menu. CSP: K10's, widened by `'wasm-unsafe-eval'`, `connect-src 'self'` and the SHA-256
    of trunk's one inline module script (computed from the served `index.html`, so another inline
    script cannot ride along); a CSP violation on the console fails the live check.
  - **The page** (`ui/`, crate `kai-ui`, its own workspace): Leptos 0.8 (the kit's minor), trunk,
    wasm. A rail (Tools, Desktop, Settings) and one page: *Tools*, the policy table by area with a
    select for who may call it (`policy.levels`) and one for what happens (`policy.tools`), each
    with `Default (…)` as the removal of the key, and the switch for `policy.config_tools`;
    *Desktop*, a select per capability (`desktop.<cap>`: Automatic, Plasma, freedesktop, None);
    *Settings*, every key `list` returns in sections, edited in place (a flag is a checkbox, any
    other value TOML text as `set` takes it, an empty text removes the key, Reset removes it too,
    `lmgw.url` is shown read-only), the loader's refusal inline under the key, filter by key. Every
    control sends its operation at once; the answer says `applied` or `saved; applies when Kai
    restarts`, and a footer says so for the rest of the visit. The page's wire types are
    `ui/src/wire.rs`, which Kai's tests compile and read its real answers with; the model logic
    (`ui/src/model.rs`) runs natively (`just ui-check`).
  - **Fixed-word settings** (2026-10-09, after the owner found no option for the weather overlay's
    position: the key was listed, as a free-text box among two dozen `overlay.*` keys). An `Entry`
    carries `choices`, the words the key's type takes, read off the deserializer's refusal of a probe
    word (`src/settings/choices.rs`; nothing lists them twice, a new enum key gets them unasked), and
    *Settings* shows such a key as a select of them: every `Anchor` key (`overlay.<kind>.position`,
    `ring.anchor`, `approvals.anchor`), `audio.echo_mode`, `voice.mode`, `accessibility.switch`,
    `delegate.screenshots`, `desktop.<capability>`, a set `policy.tools`/`policy.levels` entry. A
    section's keys in a table under it sit under that table's name (`[overlay.weather]`).
  - **The kit.** `ui/Cargo.toml` takes `lmgw-ui-kit` by path (`../../lmgw/crates/lmgw-ui-kit`, lmgw's
    checkout at `LMGW_PIN`, as `lmgw-client`): `kit.css` (tokens, density, section and form rules,
    linked first in `ui/index.html`) and the widgets `Section` and `Field`. The kit has no buttons,
    inputs, tables or page frame (those stayed in lmgw-ui's `app.css`), so `ui/assets/window.css`
    has the window's own, in the kit's tokens, plus lmgw-ui's `user-select: none` rule over the kit's
    and the window's classes. The desktop's fonts (`system-ui`, `monospace`) replace the tokens'
    bundled Plex. `just rpm` stages the kit (and `lmgw-api-types`, its dependency) from the pin with
    `git archive` into the tarball's `lmgw/`, as it does `lmgw-client`.
  - **Interface scale.** The view's zoom opens at 125 % (`UI_SCALE`); Ctrl+plus,
    Ctrl+minus and Ctrl+0 change it for the visit. Not stored; a setting for it is a later key.
  - **Embedding and build.** The page is `ui/dist/` (trunk), read from disk by a debug build and
    embedded (rust-embed) by a release build; a build without it opens the window on a sentence
    saying `just ui`. `packaging/build-rpm.sh` builds the page in the staged tree (in the Fedora
    image, which now has the wasm32 standard library and a pinned, hash-checked trunk from
    `packaging/install-trunk.sh`; in CI the job) and the spec refuses to build without
    `ui/dist/index.html`; `--page-only` stops after the page. CI installs the same and caches
    `target/rpm/ui-target` and trunk's tool cache. `just check` adds clippy with `live-test`, the
    doc tests, `just ui-check` and `just ui`. `wasm-opt` needs `--enable-bulk-memory` and
    `--enable-nontrapping-float-to-int` for current rustc output (`ui/index.html`).
  - **Memory (K1's rule).** Closing the window leaves no WebKit process (live check). Open, in the
    nested KWin (software GL, a release page, the Settings page's 99 fields mounted): WebKitWebProcess
    209 MB and the network process 17 MB PSS, against K1's 66 to 72 MB, which was a one-element
    overlay. The page is far from that figure; whether it is the page (a 450 kB wasm module, about 150
    mounted rows) or llvmpipe is not separated yet, and a real desktop (GPU) is the
    measurement that counts (`just live-check-window` prints the figure). *Decided by the owner,
    2026-10-09:* K1's budget covers only the persistent background service at rest, not short-lived
    windows or overlays, so this window's about 226 MB while open is reported, not held
    against K1; closing it still leaves nothing.
  - **Tests.** `src/settings/window/tests.rs` (origins, the reachable operations, hidden paths,
    wire types, the CSP, the zoom, the size), the core's `the_windows_operations_run_in_the_core`,
    the tray's menus; `just live-check-window` (nested KWin, the page driven through its DOM
    against the real operations, screenshots at 125 %).
  - **Profiles page** (2026-10-09; taken in this draft; ruling 29, lmgw's
    personality-profiles design §4.2, §5 and decision 7):
    - *The page.* A fourth rail item, *Personality*: a header that says which profile Kai's
      conversation talks with (the tray's checked Personality item, WP22's state: "Default
      (global: Concise)" and the like) with the folder, the thread and the folder's model, then the
      kit's `ProfilesPanel`, the same component lmgw's dashboard shows. Picking stays the tray's and
      `personality_set`'s; the page edits lmgw's profiles. The panel gets the conversation's thread
      id (Preview, Test and Speak assemble as Kai's thread would; none before a session or lookup
      named one: an empty thread) and the *folder's* `defaults.model_alias` as `test_model`: Kai
      does not track a thread's own model, and a Kai thread takes the folder's. The panel is
      mounted once, when Kai says it is paired; the header is asked again after each change in the
      panel (`on_changed`, and 1.5 s later, after lmgw's feed event reached the core). The kit's
      `ModelCatalog` is provided on the page, locked (no fetch) until Kai is paired. Leaving the
      page with unsaved edits asks first (the kit's `Modal`, Discard and leave / Stay); closing the
      window does not ask.
    - *The facts.* `GET /api/conversation` (`Command::WindowConversation`,
      `src/core/window.rs`): `{paired, why, gateway, folder, thread_id, model, profile}`. Not
      paired (also disabled, expired, revoked or the wallet still being read), `why` says what to do
      and the page shows that sentence instead of the panel; nothing asks lmgw.
    - *The proxy.* The kit's base is `/lmgw` (`kit::http::configure`, no `on_unauthorized` hook:
      lmgw's `session_required` is a dashboard session's, and a refused device key is said by the
      refusal inline). `route` hands everything under `/lmgw/` to `profiles::forward` *after* the
      origin checks of `/api/apply`, which admits exactly the kit's requests: `GET` and `POST
      /chat/api/profiles`, `POST /chat/api/profiles/{id}` and `…/{id}/reset|delete` (decimal ids),
      `POST /chat/api/profiles/preview|test|speak`, `GET /v1/models` and `GET
      /v1/audio/voices?model=<alias>` (that one parameter, written again before it goes). lmgw's
      `GET /chat/api/profiles/{id}` is not admitted (the kit does not call it); every other method,
      path or query is a 404 and a POST body that is not JSON a 400, in lmgw's `{code, message}`
      shape so the kit shows it. The core (`Command::WindowProxy`) refuses while unpaired (503
      `not_paired`) and sends the rest through the voice's link (`VoiceLink::proxy`,
      `crate::lmgw_link::proxy`): the device key as the bearer, the page's body, none of the page's
      headers; back go lmgw's status and body with its content type reduced to JSON, `audio/wav`
      (Speak), plain text or opaque bytes, and none of lmgw's other headers, so lmgw's refusals
      (R1's `403 profile_in_admin_use`, R5's `key_scope`, Speak's `tts_not_configured`) read inline
      in lmgw's words. Each request runs in a spawned task.
    - *Bounds* (a new section `[window]`): `proxy_max_mb` (default 64, lmgw's own `max_body_mb`
      default; 0: none) for the page's request (413 `body_limit`, before it goes) and for lmgw's
      answer (502 `answer_too_large`), and `proxy_timeout_s` (default 0, no bound of Kai's own:
      lmgw's own timeouts apply, and a guessed number would cut a Test on a cold local model; 0: none) for the whole exchange (504). Each refusal names its key. lmgw's
      own 413 passes through.
    - *CSP.* `media-src blob:` only, for Speak's `<audio>` on an object URL of the WAV; the proxy is
      same-origin, so `connect-src 'self'` stays and no network origin is named.
    - *Look.* `ui/index.html` links the kit's `profiles.css` after `kit.css`; `window.css` adds the
      controls the editor uses that the kit leaves to its host, as lmgw-ui's `app.css` draws them
      (primary and danger buttons, the textarea, the segmented control, facets, the field grid and
      hints, the field labels), and scopes its own `.select` to native `<select>`s (the kit's picker
      is a `div.select`). `just rpm` already stages the kit's whole `assets/`.
    - *Tests.* `src/settings/window/profiles/tests.rs` (the admitted table, every refusal under the
      prefix, the origin checks first, the facts read by the page's wire type),
      `src/lmgw_link/proxy/tests.rs` (against a scripted gateway: the bearer goes, lmgw's headers do
      not come back, the WAV and a refusal pass, both bounds and the timeout name their keys), the
      core's `the_windows_lmgw_requests_need_a_pairing_and_stay_bounded`, `ui/src/conversation.rs`;
      `just live-check-window` starts a dev lmgw on 127.0.0.1:8897, mints a device key into a mode
      600 file for the stand-in core, and drives the page unpaired (the sentence, no panel, a 503),
      then paired: lmgw's list, a create, a save and a delete, each read back from lmgw with the
      key; other routes under `/lmgw/` are 404, the key is not in the page; the captures at 125 %
      are copied to `target/window-shots/`.
  - **Next, not built.** The KDE header palette (lmgw's `desktop_palette`); a stored interface scale; the probe's
    choice (and its note) beside each capability's select, which needs the desktop module's table
    exposed to the core; the routines' switches on the Tools page (WP20); a System Settings
    entry (K29).

**WP22. `personality_set` and the tray's profile picker** (ruling 29, K31).
- Owns: `src/personality.rs`, its contract entry; hook lines in `src/core/calls.rs` and the tray.
- Depends on: M6, WP8.
- Mechanism (lmgw D14, §3.5): the name is matched case-insensitively against `GET /chat/api/profiles`
  (`default` means null), then written with `POST /chat/api/folders/{id}` and
  `{defaults_patch: {profile_id}}`, so the rolled-over thread keeps it. No new route.
- Tests: an unknown name lists the candidates in one line; the tray follows the feed's `profile.*`
  events.
- The owner: the profiles' content (K31's draft).
- **As built** (decisions where the text above was silent):
  - The tool is `personality_set {name}` in the contract area `personality`: caller level `owner`
    (the owner and Kai's own key; ruling 28 widens it), `Ask::Hints`, so it is confirmed unless
    the owner sets it to `allow`; annotations: not read-only, not destructive, idempotent. The
    core's side is `src/core/personality.rs`, the pure part `src/personality.rs`.
  - The pure logic lives in `src/personality.rs`; the core keeps the profile list (read at the
    feed's `hello` and after a `resync`; `profile.*` events upsert or remove by id and name) and
    each thread's profile (`thread.*` events carry `profile_id`, so a thread the dashboard moved
    shows so). `VoiceLink` gained `profiles()` and `set_folder_profile()`; the folder is Kai's
    conversation folder (§4).
  - **"Default"** is lmgw's null, and a folder's null means "the Chat setting `chat_profile`" for
    new threads, not "no profile" (lmgw rewrites the current thread to it). Kai cannot change that
    setting (a device key does not reach it), so `default` is that choice, and the tool's answer
    says what it came to ("... the profile Concise ..." or "... talks with none").
  - **The tray** has a Personality radio (Default, then the names in name order), shown while Kai
    knows at least one profile (an older lmgw: no submenu). The checked item is the profile the
    conversation's thread talks with, as the feed last said (else the folder's choice). The
    Default item reads "Default (global: <name>)" when the Chat setting names a profile, unless
    the thread is known to talk with none. A pick is the same operation as the tool; lmgw's
    refusal is a notification.
  - The set reads the list fresh first (it is the match's source and refreshes the tray); the two
    requests are each bounded by `lmgw.timeout_s` and run in spawned tasks, so the core's loop (voice,
    barge-in, feed, tray) is never held by them. Each task posts its answer back on the core's own
    channel; the tool's oneshot is answered, or a tray refusal becomes a notification, from there.
    One set runs at a time and a later one waits its turn, so the last pick is applied last and
    stands; a pairing that moves to another gateway answers the running and waiting sets with an
    error.
  - Not built: a live check against a dev lmgw (it needs a ready folder with a model and the
    wallet; the scripted lmgw and the real request shapes of lmgw-client are tested).

**WP23. App memory** (rulings 5 and 18, §11.1, §16 Q10 as answered: encrypted, the key in the
Secret Service). `src/memory/`, embeddings through an `embed/` alias. Depends on WP14 (the app id)
and §16 Q20 (its tools). *Parked, 2026-10-09 (§16 Q20 as answered by the owner):* app memory is
brainstormed in `docs/ideas.md` ("App memory") first; this WP waits for that design.

**WP24. Browser integration** (rulings 5 and 18, §11.1). `crates/kai-browser-host`, its manifests in
`packaging/`, `src/browser/` (the socket). Depends on §16 Q20 (its tools); the extension can wait
(§11.2). *Parked, 2026-10-09 (§16 Q20 as answered by the owner):* brainstormed in
`docs/ideas.md` ("Browser integration") first; this WP waits for that design.

**WP25. The owner's acceptance,** twice: after WP12 (phase 2) and at the end of phase 3. The RPM
with the restricted-interface keys, installed; the grant on the owner's lmgw; PD5's step; real voice,
desktop and clicks.

### 15.1.5 What runs side by side

Hot files and who owns their hook lines (a later WP adds its one line there, nothing more):

| File | Created or shaped by | One line each from |
|---|---|---|
| `src/core.rs` | WP8 (host start and stop, `Command::Tool`, `mod calls`) | nobody else |
| `src/core/calls.rs` | WP8 | WP9, WP18, WP21a, WP22 |
| `src/main.rs` | WP8 (the host task) | WP20 (subcommands, socket), WP24; none from WP10 or WP14 (their providers take what they need from `Deps`) |
| `src/ui.rs` | WP8 (`Confirm`) | WP10 (`Widgets`), WP19, WP21b |
| `src/tray/model.rs`, `src/tray/mod.rs` | WP8 (tools line) | WP10, WP14, WP18, WP20, WP21b, WP22 |
| `src/config.rs` | WP8 (sections as one field each, types in their own modules) | every WP with settings |
| `src/voice/mod.rs`, `src/voice/control.rs` | WP9 | WP18, then WP19: in that order, never side by side |
| `src/shortcuts.rs` | WP10 (`hide-overlays`) | WP20 |
| `src/host/registry.rs` | WP8 (`Deps`: the core's commands, the GTK channel, the session bus, the configuration, the current credentials, the change signal) | WP10, WP14, WP17, WP20: one `providers` line each, no new `Deps` field |
| `src/host/policy.rs` | WP8 | WP19 |
| `crates/kai-desktop/src/contract/mod.rs` | WP8 | one `mod` line per contract file |
| `contract/overlay.rs` | WP10 | WP11, WP12, after WP10 merged |
| `src/desktop/mod.rs`, `src/desktop/fdo/mod.rs`, `src/desktop/plasma/mod.rs` | WP14 | one `mod` line per provider file |
| `src/desktop/fdo/atspi.rs` | WP15 | WP16c, after WP15 merged |
| `Cargo.toml`, `Cargo.lock` | WP8 (workspace, rmcp) | WP14, WP17, WP21b, WP23, WP24: a rebase takes main's lock and lets cargo add the WP's entries |
| `packaging/` (spec, `.desktop`, `build-rpm.sh`, CI) | — | WP14, WP15 (keys), WP21b (trunk), WP24 (browser host) |
| `justfile` | — | append-only: one recipe per WP |

Groups (each row's WPs own disjoint files; a row starts when what it waits for is merged):

| Group | Side by side | Waits for |
|---|---|---|
| A | lmgw M1, M7; Kai WP8 (against the fake host), WP13 | nothing |
| B | WP9, WP10, WP14; lmgw M2 | WP8 (M1 for their live checks) |
| C | WP11, WP15, WP16a, WP16b; lmgw M3, M4's design, M5 | WP10 (WP11), WP14 (WP15, WP16) |
| D | WP12, WP16c, WP17, WP21a | M2 (WP12), WP15 (WP16c, WP17) |
| E | WP18, then WP19 (voice files); WP20 beside both | M4 (WP18), M3 (WP19), WP17 (WP20's goal kind) |
| F | WP21b, WP22; WP23, WP24 parked (§16 Q20 as answered, 2026-10-09: `docs/ideas.md` first) | M5 or the tokens (WP21b), M6 (WP22), the ideas' design (WP23, WP24) |

### 15.1.6 The first WP

**lmgw's M1 and Kai's WP8, side by side, with WP13 and M7 alongside.** Every phase 2 and 3 tool
reaches the agent only through the host link (K2, ruling 17), so M1 and WP8 are the critical path
of both phases; api §5 fixes the wire, so WP8 is built against a fake host while M1 is built, and
the two meet in `just live-check-host`. WP9's conversation tools are then the first real tools
through it (§10.1). WP13 and M7 own nothing anyone else touches and remove the two unknowns that
would stall phase 3's start and every live check.

## 16. Open questions, in priority order

1. **Kai's past threads** are deleted 30 days after their 14-day archive, unless pinned.
   *Recommendation:* a per-folder retention override (api §11 Q2), and keep the Kai folder's
   history for at least a year.
2. **The Kai device's scope and budget** at pairing (api §11 Q1). *Recommendation:* allow the aliases
   the Kai folder uses plus the embedder, set a budget if any of them has a cloud fallback, and
   give the hosting label `kai`.
3. **Voice at login** is off (K4). *Recommendation:* keep it off; models load on the first press.
4. **Phase 2: should a tool's UI show by itself?** *Recommendation:* only on `overlay_show`
   (*2026-10-09:* `overlay_app`, ruling 24). A tool running in the Kai thread does not draw over the
   desktop unasked.
5. **The ring over fullscreen games.** *Recommendation:* show it (the owner pressed the key), unless
   WP0 (b) finds it costs frames in the game.
6. **Private-address image fetches** are refused by default (K19). *Recommendation:* keep that; allow
   them per setting if the owner wants images from LAN services.
7. **After a non-takeover drop** listening stays off (K5). *Recommendation:* keep it off; resuming
   would warm models in an lmgw that was just started.
8. **`device` as the default echo mode** (K8). *Recommendation:* keep it, matching the Chat's default
   setup.
9. **The idle rollover** for the Kai folder. *Recommendation:* 120 minutes, set on the folder when
   the owner marks it ongoing.
10. **Phase 3: app memory at rest.** *Recommendation:* encrypt it with a key in the Secret Service;
    the key lives apart from the data, so a copied database file is useless without the key.

Added 2026-10-08 (phases 2 and 3):

11. **Split §10.1's `overlay_show`** under K22. `kind` decides what goes in `data`, and `anchor` and
    `size` are nested objects: the complicated kind (ruling 20). *Proposal:* `overlay_image {src}`,
    `overlay_text {text}`, `overlay_emote {name}`, `overlay_weather {place}`, `overlay_html {html}`,
    `overlay_app {server, uri}`, each with an optional flat `position` (e.g. "bottom-right";
    otherwise placement from the configuration), and `overlay_hide {id?}`. *Answered,
    2026-10-09:* approved (ruling 24), with `overlay_list` kept and no `overlay_update`; §10.1.
12. **`run_command`:** its own confirmed-by-default tool (K24; "always confirmed" until
    2026-10-09), or "run programs" (ruling 5) means `app_launch` only, with shell work in lmgw's
    planned Podman sandbox. *Recommendation (the draft):* its own tool. *Answered, 2026-10-09:*
    its own tool, confirmed by default (K24), and like every tool it may be set to allow (ruling 28).
13. **The delegate's slot per alias** (K25): the same row with two slots or a smaller model, and
    the interplay with kv-unified.
    *Interim, 2026-10-09 (WP17):* one alias per delegate in `[delegate]` (`look`, `find`),
    defaulting to the Kai conversation's alias (its folder's model), so the delegates work before
    the owner decides; the setting's documentation names both choices.
    *Answered by the owner, 2026-10-09:* all three setups are allowed: a main model with more than
    one slot (the async delegates share it); one slot and no delegate model, with a warning that
    the conversation waits for async tasks (on Kai's delegates settings page, and once in the tray
    the first time it happens); a delegate model, where the slot count does not matter and a
    warning comes only when lmgw's VRAM fit verdict (a kv-unified pool included) says the two
    cannot be resident together. The warnings state their limits (slots are shared with other
    clients; the VRAM figure is an estimate). lmgw provides a per-local-alias slot count readable
    by devices and a co-residency fit verdict, built together with the async delegates
    (`arrange_windows`, `operate_app`), not before; and a request priority value, an lmgw feature
    for all clients (a planned lmgw feature, request priority), so the conversation goes before
    async delegates. WP17's "Requirements for the async delegates" is the requirement section of
    that design.
14. **A short follow-up listening window after a hotword task** (K27).
15. **Kai's configuration UI** is the next discussion: routines that appear as tools, enabling and
    disabling single tools as Kai-side policy (apart from lmgw's key scopes), and more. *Answered in
    part, 2026-10-09:* rulings 25 to 28, K28 to K30. Open: the window's structure, which the owner's
    further ideas that need configuration will shape. *Note, 2026-10-09:* those further ideas are
    future features; the window's structure is designed when it is built, from what exists then.

Added 2026-10-09:

16. **Routine schedules** (K28): whether routines run on a timer or calendar, and how a scheduled
    run reports (ruling 22's `announce`, a notification).
17. **The `config_tools` default** (K30). The draft's choice: off, so the tool list grows only when
    the owner asks for it.
18. **The shared UI crate's extent** (K29, lmgw side): which of lmgw-ui's parts move into it beyond
    the theme tokens, CSS and widgets.

Added 2026-10-09 (the build plan, §15.1):

19. **`overlay_weather`'s data source.** Kai fetches nothing but images (K19), and lmgw serves no
    weather. A public weather service is a new external call from the owner's machine, so it is the
    owner's choice; WP11 builds the other kinds meanwhile. *Answered, 2026-10-09:* DWD data through
    Bright Sky (ruling 30); WP11 builds `overlay_weather`. A geocoding service stays under Later.
20. **The tools of app memory and the browser** (rulings 5 and 18). K24 names none, and WP23 and WP24
    wait for them. They follow K22 like every tool. *Answered by the owner, 2026-10-09:* both go
    to `docs/ideas.md` for brainstorming first ("App memory", with the owner's seed idea: memory
    injected into the prompt depending on the windows active when the prompt is sent; "Browser
    integration", with a first tool sketch as open questions). WP23 and WP24 are parked until then
    (§15.1).

Added 2026-10-09 (WP13's findings):

21. **Dev builds and the restricted interfaces.** Ruling 16 stays: the desktop tools belong to an
    installed Kai. WP13 (d) showed KWin grants the interfaces to any binary whose absolute path a
    `.desktop` in `~/.local/share/applications` names (symlinks resolve, a PATH lookup does not; new
    connections get the grant about 1.4 s after the file appears, and lose it on removal). Should
    `just install-dev` and worktree builds write a `.desktop` carrying the two keys? Each
    worktree's binary has its own path, so each needs its own file. *Answered by the owner,
    2026-10-09:* opt-in, built: `just grant-dev` writes this build's own
    `dev.rootshell.kai-devgrant-<checkout>-<hash>.desktop` in the applications directory, naming
    the build's canonical binary path with the packaged `.desktop`'s two keys (`NoDisplay`, no
    `MimeType`: no menu entry, no scheme handler); `just revoke-dev` removes that build's file;
    `just grants-dev` lists every dev grant file and whether its binary still exists, so stale ones
    from removed worktrees show (it removes nothing). `just install-dev` keeps stripping the keys.
    One entry per binary: KWin 6.7's `fetchProcessServiceField` (`serviceutils.h`) takes the
    first service `KApplicationTrader::query` lists whose `Exec`'s first word
    (`QProcess::splitCommand`) resolves to the binary, in the service cache's order, so beside
    install-dev's stripped entry for the same build a grant would hold by chance: `grant-dev`
    refuses then, naming the entry (`just uninstall-dev` resolves it), `install-dev` refuses while
    a grant names its binary, and `grants-dev` marks a grant another entry shadows. The file's
    name comes from the canonical checkout path (a symlinked checkout names the same file);
    `revoke-dev` finds the binary's files by their `X-Kai-Dev-Grant` marker. `Exec` is quoted; a
    path with `"`, `` ` ``, `$`, `\` or `%` is refused, since the spec's escapes for them are
    undone neither by KService nor by `splitCommand`, so KWin could not match it.
    `scripts/dev-grant.sh` does the work; `scripts/dev-grant-check.sh` (in `just check`) tests the
    files in a scratch HOME.

### Answers (the owner, 2026-10-06)

- **1–4 and 7–10:** approved as recommended.
- **5, the ring over fullscreen:** must be configurable whatever WP0 (b) finds. A setting
  `overlay.ring_over_fullscreen` (on by default, shown in the tray's overlay menu and the settings),
  and WP0 (b) measures the frame cost so the default can be revisited. WP0 found the OVERLAY layer
  stays above a fullscreen window and the TOP layer is hidden by one, so the setting is implemented by
  choosing the layer at creation: on = OVERLAY, off = TOP. The frame cost in a game is the owner's
  check. *(WP0, 2026-10-06)*
- **4, a tool's UI by itself:** *changed by the owner, 2026-10-09:* yes. A call in Kai's own
  conversation whose tool names a UI resource shows that view by itself, with the call's input and
  result (`[overlay.app] auto_show`, on by default: a draft default); `overlay_app` stays for
  opening a view explicitly. WP12's note says how it is built.
- **6, private-address fetches:** allowed by default (the owner, 2026-10-06). The requirement
  behind it: a request like "show me a picture of X" must fetch and show an image from the
  internet, unblocked; Kai fetches every `http(s)` image itself (K19), and LAN and loopback
  addresses are no exception. `overlay.fetch_private_addresses` can switch the latter off. Finding
  an image URL needs a search tool the model can call; Kai only fetches and shows.

### Answers (the owner, 2026-10-09: the walkthrough of open calls)

Each is recorded where it applies; this list points there.

- **WP16c, the remote-control token:** kept, and forgotten from the configuration window's
  Desktop page after a confirmation; the typing dialog says how long the permission lasts. Built
  (WP16c's note).
- **§16 Q21, dev builds:** opt-in, `just grant-dev`, `just revoke-dev`, `just grants-dev`. Built.
- **WP16a, the terminal:** with `launch.terminal` unset, Plasma's configured terminal, else
  `xdg-terminal-exec`, else Konsole on Plasma; the setting always wins. Built (WP16a's note).
- **WP10, the view over Kai's dialogs:** the view shrinks to its strip, away from the dialog,
  while one is open. Built (§10.1's note). **WP10's defaults** are kept as built.
- **WP17:** (a) an unset delegate alias is the conversation's model with its fallback; (c) one
  plan dialog for an untrusted caller's `look`, kept; (d) delegates' inference under Kai's key for
  every caller.
- **§16 Q13:** answered; WP17's requirements for the async delegates.
- **§16 Q20:** app memory and browser integration to `docs/ideas.md` first; WP23 and WP24 parked.
- **K1:** the memory budget covers only the persistent background service at rest, not
  short-lived windows or overlays (K1, WP21's memory note).
- **WP20:** `routine:<name>` only in Kai's live scopes, and `routines.run_timeout_s` default 0 with
  the tray's Stop: both kept.
- **`hide-overlays`:** stays unbound (§8.1).

## Later

- **A geocoding service** for `overlay_weather` place names: a new external service, the owner's
  call (ruling 30).
- **The KRunner runner** from the prototype (`kai <question>` as a text turn in the Kai thread,
  read aloud with `speak: true`).
- **A wake phrase:** client-side matching on transcripts, or a lmgw feature. *Note, 2026-10-08:*
  conversation mode (K27) does not wait for it; it works with push-to-talk.
- **lmgw-side, from WP0 (m):** lmgw's window must hand `lmgw-pair:` links to the system (a
  `decide-policy` handler calling xdg-open), since WebKitGTK alone never launches a scheme handler.
  Not done here; the lmgw repository is untouched.
- **Opening lmgw's own window at a thread** needs a URL scheme handled by lmgw's shell.
- **A fifth echo mode** for a raw-channel microphone array. *Changed, 2026-10-07 (the owner):* not
  needed where a driver on the PC already cancels echo from the array's raw channels and presents a
  processed source, which `device` mode records (§6.2, K8). Kept only as a possible extension for
  arrays without such a driver.
- **lmgw-side, from K29:** a shared UI crate extracted from lmgw-ui (theme tokens and CSS,
  widgets) for Kai's configuration window and later extensions (§16 Q18). Not done here.
- **lmgw-side, from ruling 29:** personality profiles as an lmgw Chat feature (persona, length
  rule, examples, speech style and voice, reasoning, voice block; K31), with the profile editor in
  the shared UI crate. Kai depends on it; until then the owner keeps the current prompt.
- **A port to GTK4 and webkitgtk-6.0,** if the 4.1 API is dropped.
- **Spoken filler while a tool runs** (realtime §19), and the phone's push and assist app.
  *Note, 2026-10-08:* K26's few-second synchronous class (`look`, `find`, short delegates) makes
  the filler more pressing.

## Appendix: Review dispositions (2026-10-06, against lmgw 2baa764)

| # | Finding | Disposition |
|---|---|---|
| R1 | Anonymous and default-scope callers reach device tools | **Changed:** api L16 / §5.6 limit reach to the owner, Kai, and keys and agents naming the label. K15 trusts by caller. Ruling 17 notes the rule. |
| R2 | An approval does not say who approved | **Changed:** K15 skips its confirmation only for trusted approvers named in `_meta` (api §5.5). |
| R5 | `current` has no model | **Changed:** §4's no-model path; Kai never chooses the conversation's model. |
| R6 | An offline device refuses every turn | **Changed** in api §5.3; §4 states the effect. |
| R7 | Approving by voice declines the call | **Changed:** K17, §11.1. The answer goes through dictation with the talk key and never enters the thread. |
| R8 | Multi-device races | **Changed:** K5 (no rebind onto another client's thread), K16 (silence during another principal's turn; a press still wins), WP4 and WP6 tests. |
| R9 | Listening all day costs energy and hides the hold | **Changed:** K11 (ring only during turns), K9 (held as a badge), K21 (fallback notification, visibility only). *Changed, 2026-10-08 (the owner's decision):* the ring shows while Kai is active, all day in continuous listening; K11 records the cost. **Open again, 2026-10-08 (review KAI-5 R5-4):** the ring shown all day costs a noticeable share of a CPU core and a small, measurable amount of GPU power. The mitigation is the `engine.js` calm rate for quiet listening in lmgw (this finding's second fix). The GPU part is small; the CPU part is what the calm rate would cut. |
| R10 | MCP Apps widgets get the whole device scope | **Changed:** K18, §10.1. |
| R11 | Hidden limits | **Changed:** `aec.ready_timeout_s` (K20); the host link's sizes and the feed keep-alive are in the API spec, and Kai reads `hello.keepalive_s` (§3.2). |
| R12 | Image fetch of any size from any address | **Changed:** K19. |
| R13 | Unsourced MCP revision | **Changed:** ruling 15 notes what was verified (the ext-apps document, `io.modelcontextprotocol/ui`); §10.1 uses its identifiers. |
| R14 | Confirmation outlives the call's timeout | **Changed:** K15 (cancelled calls never run, remaining time from `lmgw/timeout_ms`, F for thread calls). |
| R18 | Revocation has no signal | **Changed:** §3.3. |
| R19 | Folder and current edges | **Changed:** §4 (lowest-id lookup, re-ask on delete or archive); chat-only current threads are in the API spec. |
| R22 | aec.rs does not restore after a crash | **Changed:** K20. |
| R23 | Dev app id and pairing launch | **Changed:** K12, K13, WP0 (g) and (m). |
| R24 | WP0 gaps | **Changed:** WP0 (f) key repeat, (l) locked wallet, (m) pairing clicks, (n) scale and HDR, (o) idle and network-process PSS, (p) the PipeWire delay; on-demand keyboard listed for phase 3. |
| R25 | Phase 1 carries Android-sized work | **Changed** in the API spec's phase split; §1's table follows it. |
| R27 | Kai hardcodes its label | **Changed:** §3.1, §10.1 use `hello.hosts_label`. |
| R28 | Public-docs hygiene | **Changed:** the API spec names neither this spec nor anyone; this spec writes "the owner" throughout. The repository README and `RESULTS.md` lie outside `docs/design/`, which this revision may not write; they are left for the owner. |
| R29 | Tap, lmgw down, listening after a drop | **Changed:** K6 (a stopping press consumes the gesture), §3.2 (immediate "lmgw is not running"), K5 (off after a drop, with a notification). |
| R31 | Claim accuracy | **Changed:** the events table is cited at the end of chat-voice §8.6; other corrections are in the API spec. |
| R35 | The pairing key on argv | **Changed:** K13. |
| R3, R4, R15, R16, R17, R20, R21, R26, R30, R32, R33, R34, R36 | lmgw-side findings | Dispositions are in the API spec's appendix. Kai's effects: `key_expired` handling (§3.3), epoch cursor and `resync` (§3.2), `state` frames (§3.2), `lmgw.approval.decided` (§11.1). |

## Appendix: Review KAI-3 dispositions (2026-10-07, Kai on lmgw 66bb7fb)

| # | Finding | Disposition |
|---|---|---|
| K3-1 | Played replies forgotten before their `response.done` | **Fixed:** `response.done` kept in a set of its own; forget once done and played out, cut or without audio (K3's note). |
| K3-2 | A reply cut before its first audio is never forgotten | **Fixed** by K3-1's gate; the 50-turn test includes such a reply. |
| K3-3 | An unreadable event in `hello`'s chunk does not reload | **Fixed:** up from `hello` on (§3.2's note). |
| K3-4 | An unreadable `hello` reads as no hello | **Fixed:** said at once as version skew (§3.2's note). |
| K3-5 | `../lmgw` unpinned; CI cannot resolve it | **Fixed:** `LMGW_PIN`, warnings in `just check` and the live checks, the RPM from the pin, CI clones at the pin (K3's note, §13). |
| K3-6 | `device_disabled` says "pair again" | **Changed:** §3.3's note: enable it or pair again, the key kept, Try again; no slow retry. |
| K3-7 | The admin-tools line: raw names, "full" though capped | **Fixed:** "(up to lmgw's own level)", unknown levels in words (§7's note). |
| K3-8 | A known realtime event that does not read is logged at trace | **Fixed**, narrowed to the kinds `lmgw-client` reads by type: the review's prefixes would warn on every event Kai skips by design (§2's logging note). |
| K3-9 | The feed's 401 test leaves out `device_disabled` | **Fixed.** |

*Changed with lmgw main, 2026-10-07 (lmgw 7d6d18c):* K3-6's "rotated, deleted and expired keys stay
pair again" no longer holds for an expired key, which now waits as a disabled device does, and
none of it is read from lmgw's words any more (§3.3's note). K3-7's "(up to lmgw's own level)" is
gone, since the feed now says the capped level (§7's note).

## Appendix: Review KAI-4 dispositions (2026-10-07, Kai on lmgw 7d6d18c)

| # | Finding | Disposition |
|---|---|---|
| K4-1 | A press during the 4004 wait is turned into a follow and lost | **Fixed:** every ask carries its intent, kept across a re-ask; a press makes the answer the owner's bind (§3.3's note). |
| K4-2 | Listening on during the wait asks nothing, never times out, binds without a microphone | **Fixed:** listening on looks at what is really on its way (§3.3's note). |
| K4-3 | The wait outlives its question and reads one global flag | **Not fixed, by decision:** the wait for lmgw's feed goes when lmgw's fix of the race (its `current` and bind see the change before the 4004) is pinned. The fixes for K4-1 and K4-2 do not depend on it. |
| K4-4 | Every 4004 pops "Kai: voice error" | **Fixed** by its code (§5.6's note); the mock sends the error before the 4004, as lmgw does. |
| K4-5 | A refusal's body is only what came with the head | **Fixed:** the status decides where the code did not come (§3.3's note). |
| K4-6 | The connect timeout speaks for Kai's own wait, and the re-ask resets it | **Fixed** (§3.3's note). |
| K4-7 | An unknown-kind revocation has no bound if it repeats, and is said twice | **Fixed:** the feed's backoff, said once, the voice binds again once (§3.3's note). |
| K4-8 | A 404 on Kai's own bind turns listening off | **Fixed:** a 4004's twin (§3.3's note). |
| K4-9 | The tests cover the mapping, not the failure paths | **Fixed:** `src/voice/tests/reask.rs` (a 4004 while speaking, while the key is held, before `session.created`, twice in a row; a 404 on Kai's and on a press's bind; a 401 and a 409 without their code; a press and listening on during a re-ask and during the wait; an unknown 4003 twice), the adapter's body after the head, the feed's backoff after an unknown `revoked`. |

*Changed with lmgw a50141d, 2026-10-08:* K4-3 is settled by removal, as its alternative proposed.
The wait it found holes in went with the pin that brought lmgw's fix (§3.3's note of that day).
The tests that ran a press and listening on during that wait, and K4-6's words for it, went with
it. K4-1, K4-2 and the rest of K4-6 stand, for the re-asks that remain.

## Appendix: Review KAI-5 dispositions (2026-10-08, the ring while Kai is active)

| # | Finding | Disposition |
|---|---|---|
| R5-1 | A press released before its session exists drops the ring to standby mid-turn and respawns its web process | **Fixed:** a commit waiting for its session counts as the turn while that session is on its way (K11's push-to-talk note). Voice tests (`src/voice/tests/ring.rs`) assert one rise and one fall of `active` per press: the release during the wait for the last close, while the thread is asked for, and in push-to-talk mode before the first session. |
| R5-2 | "Ring over fullscreen" toggled while the ring is up does nothing until the next turn | **Fixed:** a ring that is up is placed again at once (§9.1's note); the live check toggles it both ways while the ring is up. |
| R5-3 | After a second crash an active Kai has no ring until speech, and only the log says so | **Fixed:** a retry timer, 30 s doubling to 300 s, each retry logged, and the tray's line while the ring is down (§9.4's crash note). No cap on the number of retries: the doubling bounds the rate at one spawn per 300 s, and the tooltip shows the state throughout. Live with a 3 s first wait. |
| R5-4 | The all-day cost leaves out the compositor and the GPU | **Measured:** K11's note of the same day. KWin uses a little more CPU while the ring shows; the GPU stays in a low power state. R9 is open again. |
| R5-5 | A re-placement on a failed output query moves a ring that is up | **Fixed:** a ring that is up keeps its output unless one is resolved (§9.1's note). |
| R5-6 | Stale or slightly wrong statements | **Fixed:** K1's and K11's heads point to their change notes; K1's note says what changed for the view; the K-15 bullet points to the crash note; standby names the unknown 4003's rebind; `Core::set_ring`'s and `Voice::standby`'s docs; the live test's wrap and the README line. |
| R5-7 | The hotword hook needs more than `standby` | **Fixed** in the words (K11's hotword bullet, `Voice::standby`); nothing is built. |
| R5-8 | Tests miss the rule's continuity and several transitions | **Fixed:** the one-rise-one-fall tests (R5-1); automatic to push-to-talk at runtime, mute mid-turn, the unknown 4003's rebind, the connect timeout while active, a dead link and a confirmed re-pair (`src/voice/tests/ring.rs`); the feed's dedup as `FeedDedup` with unit tests (`src/overlay/feed.rs`). |
| R5-9 | A failed feed call still counts as sent | **Fixed:** a failed call clears what was sent (§9.3's note). |
| R5-10 | A late output maps a ring that is already settling | **Fixed:** an output maps only a ring that is to be up, and a ring that settles before it was mapped hides at once (§9.4's note). `boot.js` is unchanged. |
| R5-11 | With a session bound in standby, the tooltip says the ring is down | **Fixed, 2026-10-08:** the line shows only while Kai is active (§9.4's crash note). The lifecycle test walks a push-to-talk turn's two crashes into standby and mute, a retry in standby, and leaving standby. |
| R5-12 | The log line of a kept output contradicts its own note | **Fixed, 2026-10-08:** a resolution's note gives only the cause, and the show says what follows (§9.1's note); a placement test pins both lines. |
| R5-13 | The R5-1 condition is not pinned | **Fixed, 2026-10-08:** a voice test flags the press's commit, ends the bind with no thread, and asserts standby. Without the condition it fails. |
| R5-14 | A removed output may close the ring's window behind Kai's back | **Verified, then fixed, 2026-10-08:** on gtk-layer-shell 0.10 the window is not closed; the surface is, and the ring stayed invisible until the next turn's start while its page went on drawing. Older gtk-layer-shell closes the window as described. Kai now asks for the close, keeps the window and shows the ring again; an output that comes back gets it when the output rule names it (§9.1's note). The live check covers it in a nested KWin (§14's note). The last output going away: R6-2. |

## Appendix: Review KAI-6 dispositions (2026-10-08, the ring's follow-up)

| # | Finding | Disposition |
|---|---|---|
| R6-1 | `nested-kwin.sh` deletes whatever directory it is given | **Fixed:** it refuses a directory that is neither new, empty nor marked as its own, and clears only its own subdirectories and files (§14's note). |
| R6-2 | The last output going away maps into an untested zero-output state, and the log and tray say the outputs did not change | **Fixed:** no map while the compositor lists no output, a "no output" state that an output change ends, and the second close's lines say what Kai knows (§9.1's KAI-6 note). Lifecycle tests walk the removal before and after the close, the output back, a turn's start, standby and a crash while there is no output, and both close-and-change orders. The nested check stands in for the empty list (KWin keeps one output at least); what KWin and GDK send for a last real output is WP5's owner check. |
| R6-3 | "Ring over fullscreen" re-places a ring that is down | **Fixed:** the toggle goes through the lifecycle, which places only a ring that is up (`Lifecycle::replace`, with a test). |
| R6-4 | A missing `ring.output` still names two outcomes | **Fixed:** the resolution's note is the cause only; the show adds the one outcome, "the ring goes to the active output 'X'" when the active output stands in. The placement test covers the configured output missing with and without an active output. |
| R6-5 | The respect-close fallback assumes an old gtk-layer-shell | **Fixed:** with 0.10 or later and no symbol found, Kai warns once that the compositor's close goes unseen. |
| R6-6 | The nested bus can activate services, and its guard is negative | **Fixed:** a bus with no service directories in the nested runtime directory, its address written there and compared exactly (§14's note). |
| R6-7 | The latency bound loosens with a slow page | **Fixed:** the display's refresh rate gives the frame interval, and the page's rate must be within a frame a second of it (K11's note). |
| R6-8 | `Lifecycle::feeding()` is test-only | **Fixed:** removed with its asserts; the action sequences and phases say the same. |
| R6-9 | Private session detail in the K11 note | **Fixed:** the note keeps the finding (Kai's part in milliseconds, the delay outside Kai's path) and drops the microphone chain's details and its log. |
