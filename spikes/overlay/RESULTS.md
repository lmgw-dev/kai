# Overlay spike: layer-shell + WebKitGTK + lmgw's ring.js (2026-10-06)

Stack: Rust, gtk 0.18 (GTK 3.24.52), gtk-layer-shell crate 0.8.2 (runtime lib 0.10.0), webkit2gtk crate 2.0.2 (WebKitGTK 2.54.0, 4.1 API),
KWin/Plasma 6 Wayland, an NVIDIA GPU, a single ultrawide output at scale 1 with HDR enabled (SDR content). NVIDIA rule applied at the top of `main()`
(`__NV_DISABLE_EXPLICIT_SYNC=1` if `/dev/nvidiactl` or `/sys/module/nvidia` exists and the variable is unset). None of the forbidden
WebKit/GDK variables was used.

## Verdicts

| Q | Verdict |
|---|---|
| Q1 works? | **Yes**, with two caveats: (a) the ring needs 9 lines of glue to be transparent, because ring.js paints an opaque background itself; (b) a real mouse click on the interactive variant was **not** injected (see Q1.3), the input region is verified at protocol level only. |
| Q1 ring.js verbatim? | **Yes.** `engine.js` and `ring.js` are byte-identical copies (`diff` clean). Glue: 45-line `adapter.js` (AnalyserNode stand-in) + 9 lines for transparency + ~20 lines of page boot that lmgw's `viz.rs` already does. |
| Q1 GPU compositing? | **Yes**: hardware acceleration policy `always`, 2D canvas `Accelerated`, renderer `DMABuf (Hardware, Shared Memory)`, buffer `DMA-BUF AR24 NVIDIA_BLOCK_LINEAR_2D`, EGL (recorded in the raw GPU probe). `nvidia-smi pmon`: overlay and WebKitWebProcess are type `G` with 11 MB and 33 MB VRAM (raw `pmon` capture). |
| Q2 RAM | One WebView ~**190 MB** PSS all-in (UI 73 + network 17 + web ~99). With the default shared `WebContext` **every view gets its own web process**: ~**66-72 MB per extra view**. Views created `related` to the first share one web process: ~**15 MB per extra view**. 4 widgets inside one view: **+6 MB**. |
| Q3 energy | Ring speaking (60 fps): **~14 % of one core** (UI 7 %, web 7 %), GPU power **a small, measurable rise** over the no-overlay baseline. Calm idle (15 fps): **~3.4 %** CPU, GPU power **within the baseline's spread**. |

## Method

* `src/main.rs`: one process, N `gtk::Window`s, each `init_layer_shell`, layer OVERLAY, namespace `kai-overlay`, anchors bottom+right, margins 24 px
  (2x2 grid for N>1), `KeyboardMode::None`, exclusive zone -1, RGBA visual + transparent GTK CSS + `set_background_color(0,0,0,0)`.
  Each hosts a `WebView`. Pages come from a custom `kai://app/` URI scheme (embedded assets), so `import` of ES modules is same-origin.
  I did not test whether `file://` would have worked; the scheme was chosen up front.
* The host feeds the page at 60 Hz with `evaluate_javascript("__kai.feed(level_out, level_in, bands_out[48], bands_in[48])")` (synthetic
  speech-like bands) and calls `__kai.setState(...)`; states cycle idle, listening, thinking, speaking, 4 s each. Feed starts when the page posts `ready`
  through a `UserContentManager` script message handler (the first version sent `setState` before the page loaded and lost it).
* Screenshots: `shots.sh` (a baseline crop of the same rect is taken before launch; the full capture is deleted at once, only crops stay).
  Transparency is judged by pixel comparison against that baseline.
* RAM/CPU/GPU: `measure.py` (PSS from `/proc/<pid>/smaps_rollup` of the UI process and all descendants after an 7-8 s warm-up; CPU from
  `/proc/<pid>/stat` utime+stime over 20 s; `nvidia-smi --query-gpu=power.draw,utilization.gpu -lms 500` over the same 20 s; a 20 s no-overlay
  GPU window right before each comparison, `q3.sh`). The raw records are kept out of the repository; the findings are summarised here.
* Build: `. ./.cargo-env.sh && cargo build --release` (PKG_CONFIG_PATH shim, `CARGO_BUILD_JOBS=8`).

## Q1 details

### Q1.1 Layer surface, transparency, keyboard
Wayland trace (`WAYLAND_DEBUG=client`, raw traces kept out of the repository): `get_layer_surface(..., layer 3 = overlay, "kai-overlay")`,
`set_keyboard_interactivity(0)`, `set_anchor(10)` (bottom|right), `set_margin(0,24,24,0)`, `set_size(320,320)`.
Crop `ring-states` (kept out of the repository; idle, listening, thinking, speaking, left to right) shows the ring over ordinary desktop windows with their text visible
through the transparent parts. Pixel check: corners equal the baseline exactly (`(30,32,33)` vs `(30,32,33)`). The crops show live desktop content and are
not published.

### Q1.2 Click-through
GDK3 translates an **empty** `cairo::Region` into `wl_surface.set_input_region(nil)`, and nil means "the whole surface takes input", the opposite of
click-through (trace in the first iteration). Workaround: a 1x1 px region in the corner, `wl_region.add(0,0,1,1)` (raw trace kept out of the repository). The region
must be set in the `map-event` handler: setting it in `realize` was overwritten by GDK's own nil at map.

### Q1.3 Partially interactive
Region `add(10,270,120,40)` = the blue button (raw trace and crop kept out of the repository). The page reports
`pointerenter`, `pointermove` and `click` back to the host and the button toggles blue/amber.
**Not verified:** a real click. There is no input injector installed, and injecting through `/dev/uinput` would move the real pointer of the live session,
so it was not done. `./click-test.sh` runs the variant for 30 s for a human: click the button, expect `click` events in the
output and the amber state; hover the ring elsewhere, expect no events. Compositor behaviour for input regions on layer surfaces is standard
(`wl_surface.set_input_region` honoured by KWin), so I expect it to pass, but it is unproven here.

### Q1.4 ring.js with no audio: the glue
`assets/adapter.js` (45 lines) implements what engine.js touches: `fftSize`/`smoothingTimeConstant` (settable), `frequencyBinCount`, `context.sampleRate`,
`getFloatFrequencyData` and `getFloatTimeDomainData`. It inverts `Tap.read()`: band value v becomes dB with the engine's own exported `SIGNAL` constants
(floor, range, tilt), time-domain gets a sine at the RMS that maps to the level. So it needs no copies of the tunables; if `SIGNAL` changes it follows.
Two analysers (output and input) are fed from the host. `mount()`, `setState`, `resize` are used exactly as in the contract (§10).
Measured per-frame main-thread cost reported by `handle.stats()`: 0.01 to 0.13 ms (budget 1 ms).

**Transparency is the one place ring.js is not overlay-ready.** It asks for `getContext("2d", {alpha: false})` and fills the background every frame, so
the canvas is opaque. Options, in order of preference:
1. A 2-line change in ring.js (alpha true, `clearRect` instead of the background `fillRect`), ideally as an option of `mount(canvas, inputs)`. Not done here
   because the brief said verbatim.
2. What the spike does (`transparentCanvas()` in index.html, 9 lines, `--bg alpha`): per-canvas override of `getContext` forcing `alpha: true`, and `fillRect` at (0,0)
   becomes `clearRect`. Works with the unchanged file; relies on ring.js having exactly one origin-anchored `fillRect`.
3. Opaque disc (`--bg disc`): CSS `border-radius: 50%` puck in the engine's own `#121518`. Zero glue, but not an overlay look.
4. **Does not work**: CSS/SVG filter `feColorMatrix` mapping luminance to alpha on a black background (`--bg lum`). It works on canvases up to ~100 px and
   silently yields an opaque black rectangle from ~150 px up (both `alpha:true` and `false`, animated or not; `--mode test` reproduces it). Probably the accelerated-canvas
   compositing path ignores the SVG filter on WebKitGTK 2.54. Not investigated further.

## Q2: RAM (PSS, MB)

Ring speaking, 240 px surfaces, 7-8 s after start. Medians of stable runs; the first run of each series came out higher (n=2 shared 316, n=4 shared 498, widgets
249) and is excluded as a cold-start artefact (raw rows kept out of the repository, labels without `-r2/-r3`). PSS divides shared library pages between all processes
mapping them, including other WebKit processes running at the time, so absolute numbers are somewhat lower than a lone app would show.

| Setup | UI | WebKitNetwork | WebKitWeb (count) | Total | Marginal per extra view |
|---|---|---|---|---|---|
| 1 view | 73 | 17 | ~99 (1) | **189** | - |
| 2 views, shared default `WebContext` | 72 | 16 | ~172 (2) | **260** | +71 |
| 4 views, shared default `WebContext` | 73 | 16 | ~299 (4) | **388** | +66 |
| 2 views, separate (ephemeral) `WebContext` each | 71 | 2 x 13 | 2 x 85 | 269 | +80 |
| 4 views, separate `WebContext` each | 73 | 4 x 12 | 4 x 74 | **420** | +77 |
| 2 views, `related` to the first (same context) | 75 | 17 | 111 (1) | 204 | +15 |
| 4 views, `related` to the first | 78 | 17 | ~137 (1) | **234** | +15 |
| 1 view with 4 widgets (ring, 2 text cards, image card) | 72 | 17 | ~106 (1) | **195** | +6 vs 1 view |

Process model (observed): the default `WebContext` in WebKitGTK 2.54 starts **one WebKitWebProcess per unrelated view**, one shared NetworkProcess.
`WebView::builder().related_view(&first)` puts views into the same web process (one extra ~15 MB each: their own JS heap, canvas, layer). Separate contexts
also duplicate the network process. I did not call `set_process_model` (deprecated). Related views share a process, so one slow or crashing page
affects all of them, and they share cookies/storage/cache of the context. GPU memory per view (`pmon`): ~11 MB UI + ~33 MB web process.

## Q3: Energy

20 s windows, ring 320 px, Canvas2D. CPU is percent of one core, summed over the UI process, WebKitWebProcess and WebKitNetworkProcess. GPU values are medians of
the 500 ms samples (means agreed closely); the baseline is a 20 s no-overlay window directly before each run. Frames counted by `handle.stats()` over ~29 s.

| Run | CPU total | UI / Web / Net | GPU power against base | GPU util % base -> run | Frames |
|---|---|---|---|---|---|
| speaking #2 | 13.9 | 7.1 / 6.8 / 0 | a small rise | 2 -> 9 | 1728 (60 fps) |
| speaking #3 | 13.7 | 6.8 / 6.8 / 0 | a small rise | 0 -> 7 | 1728 (60 fps) |
| idle #2 | 3.3 | 1.4 / 1.9 / 0 | within the baseline's spread | 1 -> 2 | 432 (15 fps) |
| idle #3 | 3.4 | 1.5 / 1.9 / 0 | within the baseline's spread | 0 -> 2 | 431 (15 fps) |
| no overlay | 0 | - | the baseline | 0 to 2 | - |

* Speaking run #1 is discarded: other GPU load during the window disturbed the measurement. Idle #1 (CPU 3.1 %)
  is consistent with #2/#3. The raw rows are kept out of the repository.
* Cost split: speaking costs a small rise in GPU power and 14 % CPU, i.e. the animation plus the host feeding it; the UI process's 7 % is presumably mostly the 60 Hz
  `evaluate_javascript` of ~700 bytes per frame (not isolated by a separate run). A real Kai would get the levels from its own audio path, so that share is spike overhead and could be
  replaced by a user-script message or lower feed rate (the ring smooths anyway).
* Idle: engine.js did drop to IDLE_FPS 15 (432 frames), CPU 1.4-1.5 % in the UI is the spike's 60 Hz no-op host timer plus one eval per second, not the
  page. The GPU power rise is within the baseline's spread; `nvidia-smi` util +1 to 2 points includes KWin compositing the surface.
* A visible transparent layer surface costs KWin a composite per page frame; no finer split attempted.

## Problems and workarounds

1. `gtk-layer-shell-devel` missing: the `gtk-layer-shell-sys` crate only needs a pkg-config probe and an unversioned `.so` to link (it is pure FFI). Spike-local
   `shim/pkgconfig/gtk-layer-shell-0.pc` + `shim/lib/libgtk-layer-shell.so -> /usr/lib64/libgtk-layer-shell.so.0`, enabled by `. ./.cargo-env.sh`.
   **Proper fix: `sudo dnf install gtk-layer-shell-devel`.**
2. Crate features: `gtk-layer-shell` needs `v0_6` for `set_keyboard_mode`/`is_supported`; webkit2gtk needs `v2_40` for `evaluate_javascript`; `javascriptcore-rs` must be a direct
   dependency to read results (`ValueExt`).
3. Opaque canvas in ring.js, SVG-filter route broken on large canvases: see Q1.4.
4. Empty input region becomes "everything": see Q1.2. Set regions in `map-event`.
5. Events sent before the page is ready are dropped: the host waits for the page's `ready` message.
6. `WebView::with_related_view` has no user-content-manager parameter: related views are built with `WebView::builder().related_view().user_content_manager()`.
7. Transparency needs the whole set: application CSS rule `window { background: transparent }`, `set_app_paintable`, RGBA visual and
   `WebView::set_background_color(0,0,0,0)` (I did not test which of them can be dropped).
8. `Gtk-Message: Failed to load module "appmenu-gtk-module"` on start: harmless.
9. No crash or protocol error with `__NV_DISABLE_EXPLICIT_SYNC=1` and DMABuf rendering on NVIDIA.

## Open

* Real click on the interactive variant (`./click-test.sh`), including that the rest of the surface lets clicks through to the window below.
* Behaviour over fullscreen windows, on multiple monitors, with fractional scaling/HDR content, and surface-size changes (input regions need re-applying on resize): untested.
* Whether `related` views fit Kai (shared crash domain) or the 66-72 MB per unrelated view is acceptable: a design call.
* ring.js: ask for the 2-line transparent option upstream in lmgw instead of the 9-line shim.

## Files

`src/main.rs` host; `assets/{index.html,adapter.js,viz/engine.js,viz/ring.js}` page; `shots.sh`, `measure.py`, `q3.sh`, `click-test.sh`. The raw records (logs, measurements, Wayland traces, GPU probes, crops) are kept out of the repository; the findings are summarised here.

## WP0 (2026-10-06): verify the untested points

Code in `../wp0/` (one probe binary `wp0 --mode ...`, `owner-checks.sh`, `shortcuts-probe.py`, `cpu.py`, `fs-test.sh`). Its raw logs and crops are kept out of the repository; the findings are summarised here.
On the spike's test system: Plasma 6 Wayland, a single ultrawide output at scale 1 with HDR enabled, an NVIDIA GPU, WebKitGTK 2.54, xdg-desktop-portal 1.22.1 + portal-kde 6.7.5.
`gtk-layer-shell-devel` is installed, no shim used. Built with `CARGO_TARGET_DIR=../overlay/target` to reuse the compiled dependencies.

| Pt | Verdict | Result |
|---|---|---|
| a | **owner** | `owner-checks.sh a` (button click, click-through of the rest). The probe's page now logs click coordinates, so the script decides automatically whether any pointer event arrived outside the button rect. |
| b | **verified** (first half), **owner** (fullscreen application) | A normal fullscreen window does not hide the OVERLAY layer surface; the TOP layer is hidden by it. See below. |
| c | **N/A** (one output); checked in a nested KWin | `output = nil` is accepted; explicit `wl_output` works. See below. |
| d | **verified** | GDK does NOT re-send the input region after a resize; re-applying it in `size-allocate` works. K-text in §9.1 is right and needed. |
| e | **verified** | `kai://` works, also under a strict CSP. `file://` fails by default and works only with `allow_file_access_from_file_urls`. K10 confirmed. |
| f | **owner** | `owner-checks.sh f` (portal through `shortcuts-probe.py`, KGlobalAccel fallback built in). |
| g | **partly verified** | Portal app ids derived as K12/§8.3 say, but only if a matching `.desktop` exists; `gtk-launch` does not make its own scope. Two spec-relevant findings below. |
| h | **verified** | The Secret Service is answered, by `ksecretd` (not kwalletd). |
| i | **verified** | View creation to the ring's first frame: ~250 ms, cold or warm. |
| j | **verified** | At 30 Hz the transport is not measurable against the ring's own cost; keep `evaluate_javascript`. |
| l | **owner** (optional) | `owner-checks.sh l` (simulates the lock with `Secret.Service.Lock`, or run it after a real locked login). |
| m | **owner** (pre-tested) | `owner-checks.sh m` installs and removes a temporary handler. Pre-test: WebKitGTK alone never launches the handler. |
| n | **verified** (HDR), **N/A** (fractional scale) | Colours exact, no black box. |
| o | **verified** | UI process 16 MB without WebKit; the default `WebContext` keeps the network process alive for good; a dropped ephemeral one does not. |
| p | **verified** | pipewire-rs 0.10.1 has `Stream::time()` over `pw_stream_get_time_n` (feature `v0_3_50`). |

### (b) Ring over a fullscreen window
`fs-test.sh` opens the overlay surface (bottom right, 320 px), 1.5 s later a normal GTK window goes `fullscreen()` (solid 20,60,30) and one capture is cropped.
* Layer OVERLAY: ring and 8 colour swatches fully visible over the fullscreen window (crop kept out of the repository), the Plasma panel strip is covered by the window, so it was really fullscreen. The ring was already up when the window went fullscreen, so this is the "game starts later" order.
* Layer TOP: the whole surface is hidden, the crop is pure window colour (crop kept out of the repository). Thus `overlay.ring_over_fullscreen = false` can be implemented by creating the surface on TOP instead of OVERLAY (no polling of window state needed). It has to be chosen at creation, which fits K1 (destroyed when hidden).
* The observed sequence (first the green window with the swatches, then the overlay alone on the desktop, then the green window without them) matches two automated runs back to back and is expected behaviour: run 1 (OVERLAY): green + ring; the fullscreen window closes at 8 s while the overlay lives until 9 s (the ring alone on the desktop); run 2 (TOP): ring on the desktop for 1.5 s, then green WITHOUT the ring because TOP sits below fullscreen. Not a failure of (b).
* Not measured: frame cost in a game (owner, `owner-checks.sh b`). A real game may use direct scanout; KWin then composites the overlay or drops the scanout, which only the game run shows.

### (c) Two outputs
The test session had a single output, so this was checked in a nested KWin: a nested `kwin_wayland --virtual --output-count 2` (private D-Bus, XDG dirs in /tmp, no GPU, no window shown; `kwinoutputconfig.json` unchanged) with a WebKit-free layer surface (`wp0 --mode plain`):
* `output = nil` (`get_layer_surface(..., nil, ...)`): surface entered `wl_output` 0 (the first output); GDK's monitor-for-window agrees.
* explicit monitor (`set_monitor`, a `wl_output` argument): surface entered output 1.
The code must therefore pick `ring.output` by connector name (match GDK monitor to its connector) and pass it, else it takes whichever KWin picks. Whether KWin picks the active/focused output for `nil` on a real two-monitor session could not be tested (the nested session has no focus); my reading of KWin is that it uses the active output at creation. A layer surface does not move afterwards, but K1 re-creates it for every turn, so it re-picks each time. Not tested: output hot-unplug while shown.

### (d) Input region after a resize
`wp0 --mode resize --interactive [--reapply]`, `WAYLAND_DEBUG=client` filtered (raw traces kept out of the repository). Steps: `set_size_request` 240, 320, 200x160, 320, view `set_size_request(280)`, back to 320.
* Every size change reaches the compositor as `zwlr_layer_surface_v1.set_size` (no `configure` came back; KWin accepted the client's size).
* Without handler: `set_input_region` is sent once (map) and never again, so the interactive rect `(10,270,120,40)` stays stale: after shrinking to 240 px it lies partly outside the surface, after 160 px high entirely outside.
* With a `size-allocate` handler that recomputes the region from the allocation: a fresh `wl_region` is sent after every resize with the rect following the surface (`(10,190,..)` at 240, `(10,110,..)` at 160, `(10,230,..)` at 280); the 1x1 click-through region works the same way. `gtk_window_resize` below the size request is a no-op (no change on the wire).
Rule for the code: compute both regions in `map-event` AND in `size-allocate`, from the allocated height.

### (e) `file://` against `kai://` for ES modules
`wp0 --mode esm`: a page with `<script type=module src=./esm-boot.js>` importing `ring.js` (which imports `engine.js`) and the adapter, mounting the ring and waiting for 3 animation frames. Per case a fresh ephemeral `WebContext` (raw log kept out of the repository).

| Case | Result |
|---|---|
| `file://` default settings | **fails**: module script `error` event, nothing runs (origin `file://` is opaque, module fetch is CORS-checked) |
| `file://` + `allow_file_access_from_file_urls` | works, but that setting lets every page read any local file URL |
| `file://` + universal access | works (even worse) |
| `kai://app/` no CSP | works, origin `kai://app` |
| `kai://app/` strict CSP by `<meta>` | works; inline script blocked, modules run |
| `kai://app/` strict CSP by response header (`URISchemeResponse`) | works; same |
The strict CSP used: `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'`. `'self'` matches the `kai://app` origin. ring.js and engine.js set styles through the CSSOM, so no `unsafe-inline` for scripts is needed, but the page boot must be an external module (the spike's inline module would be blocked): `ring.html` should load a `boot.js`. `style-src 'unsafe-inline'` was included but not proven necessary. Needs `soup3` as a direct dependency to set response headers. K10 stands.

### (g) Portal app id (partial; BindShortcuts never called)
Method: `CreateSession` (no dialog) from `busctl` under various units, `busctl monitor` on `org.freedesktop.impl.portal.desktop.kde` to read the `app_id` the portal hands the backend, sessions die with the caller. Temporary `dev.rootshell.kaiwp0test.desktop`, removed. `app-app\x2d…slice` units my tests left were stopped.
* `systemd-run --user --scope -u app-<id>-dev1.scope` -> app id `<id>` (K12 pattern works, also in the form `app-gnome-<id>-4242.scope`).
* Transient service `app-<id>@autostart.service` -> app id `<id>` (the autostart form works).
* A dash in the random part (`app-<id>-dev-1.scope`) -> **"An app id is required"** (confirms K12's no-dash rule).
* A plain transient service (`run-…`/`wp0-plain.service`) -> "An app id is required".
* **Without a matching `<id>.desktop` in the XDG data dirs the portal refuses even a correct scope name: "An app id is required"** (tested with `dev.rootshell.kai` and no file). Dev builds therefore need a `dev.rootshell.kai.desktop` in `~/.local/share/applications` (or equivalent). **Affects K12 / §8.3 / §13.**
* Bare from a shell: the process inherits the launching app's scope; from an IDE's integrated terminal the portal saw that IDE's app id (from a Konsole it would be `org.kde.konsole`).
* `gtk-launch <id>`: **does not create a scope of its own**; two seconds after launch the child was still in the caller's scope (`app-<caller-id>-<n>.scope`) and the portal saw that app id. **Affects K13 / §8.3** ("a scheme-handler start goes through `gtk-launch`"): a Kai started that way gets its caller's id, not `dev.rootshell.kai`, unless the launcher is KDE's (KIO/Plasma create `app-<desktop-id>@<uuid>.service`) or Kai moves itself/starts with `systemd-run --scope`. Not tested: a real browser click launching the handler (that is (m)), and the autostart entry itself.

### (h) KWallet through the Secret Service
`org.freedesktop.secrets` is served by **`ksecretd`** (pid of the Plasma session; `org.kde.ksecretd`, `org.kde.secretservicecompat`), default alias -> collection `kdewallet`, unlocked. With `secret-tool` (default collection, attribute `kai-wp0-test=<unique>`, label "kai-wp0-test (safe to delete)"): store 52 ms, lookup 13 ms (value round-trips), `SearchItems` over D-Bus sees the item, clear 14 ms, lookup afterwards fails, `SearchItems` finds 0. No dialog appeared. No dedicated collection was created (creating one prompts for a password). `Unlock` on an unlocked collection returns `(collection, "/")` = no prompt. The Rust side can use any Secret Service client (e.g. `oo7`/`secret-service` crates) against this.

### (i) View creation to first ring frame
`wp0 --mode life`, ring page (kai:// scheme, shared default WebContext), time from before `WebView` creation (including the layer window) to the engine's own frame counter plus one rAF; 3 fresh processes x 3 cycles.

| | create -> page ready | create -> first frame |
|---|---|---|
| cold (cycle 1 of a new process) | 215, 231, 250 ms | **234, 250, 267 ms** |
| warm (cycles 2, 3, same process, network process alive) | 224-277 ms | **248-305 ms** (median ~280) |
There is no warm gain: the web process is gone 0.5 s after the view (no process cache) and every cycle spawns a new one. With an ephemeral per-use context: 248 / 268 ms. Consequence for K1/K11: a ring created at `speech_started` shows about a quarter second later; if that is visible, pre-create the view hidden when listening starts, or accept it.

### (j) Feeding at 30 Hz
`cpu.py`: ring speaking at 60 fps rendering, host feed at 30 Hz, 7 s warm-up, 20 s window, CPU in percent of one core (UI / web / network), two runs for three of the transports (raw rows kept out of the repository).

| Transport | UI | Web | Total |
|---|---|---|---|
| none (ring animates, no feed) | 5.3, 6.4 | 5.6, 6.6 | 10.9, 12.9 |
| `evaluate_javascript` (text, ~700 B) | 5.1, 5.0 | 7.0, 6.4 | 12.0, 11.4 |
| `evaluate_javascript` (base64 of 98 f32) | 6.4 | 6.4 | 12.8 |
| `call_async_javascript_function` (base64 argument) | 5.6 | 7.2 | 12.8 |
| page pulls `fetch("kai://app/feed")` at 30 Hz (binary) | 5.3, 5.4 | 6.8, 7.1 | 12.1, 12.4 |
All differences are inside the run-to-run spread (the two "none" runs differ by 2 points). The spike's note that the UI's 7 % was mostly the 60 Hz eval was wrong: the UI process spends ~5 % with NO feed (frame callbacks and buffer commits at 60 fps). **Keep `evaluate_javascript` at 30 Hz; a cheaper transport is not worth building** (K11 / WP5). Not tested: WebKit user messages (host to page needs a WebProcessExtension shared object, not worth shipping for this).

### (n) HDR, fractional scale
With HDR enabled, the overlay (swatches #ff0000, #00ff00, #0000ff, #ffffff, #3daee9, #808080, #000000, #e8a33d plus the ring) over a fullscreen solid window: swatch pixels in the spectacle capture equal their sRGB values exactly, the transparent areas equal the window colour (20,60,30), no black box, the ring blends normally (crop kept out of the repository). Fractional scaling was not tested: N/A (the code must still scale the regions by the surface's logical size, which (d) does).

### (o) Memory without a view
`wp0 --mode life` (raw logs kept out of the repository), PSS:
* UI process: 11 MB before `gtk::init`, 16 MB after it, **41-42 MB as soon as a `WebContext` exists** (libwebkit is mapped, still no child process), 60-64 MB with a ring up, **60-63 MB after the last view is destroyed** (does not go back).
* Web process (~77 MB) is gone within 0.5 s of the last view.
* Network process (14 MB): with the **default** `WebContext` it stays after the last view for as long as watched (20 s, and across the next cycles). With a **per-use ephemeral `WebContext` dropped together with the view** there are no children left 0.5 s later (view up: web 77 + network 12 MB).
Consequence for §9.4: the ring should use its own non-default `WebContext::new_ephemeral()` (Kai's pages need no persistent storage) created per show and dropped after the view; the UI process then idles at ~60 MB (WebKit libraries stay mapped), 16 MB only if the process never used WebKit.

### (p) pipewire-rs and `pw_stream_get_time_n`
pipewire 0.10.1 (crates.io): `Stream::time()` returns a `Time` with `now`, `rate`, `ticks`, `delay`, `queued` and, with the cargo feature **`v0_3_50`**, `buffered`, `queued_buffers`, `avail_buffers`; it calls `pw_stream_get_time_n` under that feature and `pw_stream_get_time` otherwise. `pw_stream_get_nsec` is not wrapped (a TODO in the crate); `now` is CLOCK_MONOTONIC ns, so `clock_gettime` gives the interpolation base. Probe `pwprobe/main.rs` (30 lines, built against the installed libpipewire 1.6.9) compiles and links; `time()` on an unconnected stream returns EIO, as expected. `pipewire-sys` is not needed.

### Spec items touched by findings (the spec is not edited here)
* **K12 / §8.3 / §13**: the portal needs `dev.rootshell.kai.desktop` installed (dev builds too) or it answers "An app id is required"; the name rule (no dash in the random part) is confirmed.
* **K13 / §8.3**: `gtk-launch` gives the launched process the caller's scope, not its own app id.
* **§9.4 / K1**: use a dropped ephemeral `WebContext`, else the network process stays; a ring shows ~250-300 ms after creation.
* **K11 / WP5**: no cheaper feed transport needed.
* **§9.1**: the "after every size change" input-region rule is required, not optional.
* **§16 answer 5**: `ring_over_fullscreen = off` is implementable as layer TOP at creation.
No point FAILED outright.

### WP0, the owner's checks (2026-10-06)

From the owner's verdict record (kept out of the repository; the findings are summarised here):
- **(a) verified.** 3 clicks reached the interactive button and 0 events reached anything outside it, so click-through was confirmed by hand.
- **(b) fullscreen half.** The ring is visible over a borderless fullscreen application (a game). Its frame cost was not measured within the probe's window. Not blocking, since `overlay.ring_over_fullscreen` is a setting.
- **(f) verified.** Through the portal, `Activated` and `Deactivated` each arrived twice, in strict alternation, with no repeat while held. The first press to release took 111 ms. The KGlobalAccel fallback is not needed.
- **(m) half.** A browser click (the browser asked first) ran the handler once. The lmgw-window half is untested: lmgw shows no pairing link yet, so the owner found none to click. The automated pre-test in a plain WebKitGTK view suggests lmgw's window will need to hand the link to the system. Verify it when the pairing card exists (an lmgw-side item).
- **(l) not run** (optional).

The test shortcut was unregistered from KGlobalAccel afterwards. The temporary `.desktop` files and the `mimeapps.list` entry are gone.
