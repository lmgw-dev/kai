# WP13: verify before building (phase 3's WP0, 2026-10-09)

Points from §15.1.4 WP13 (§11.2's list, §15 WP0's note). Everything below ran in a **nested, invisible KWin**
(`nested.sh`: `kwin_wayland --virtual`, one 1280x720 output, a private D-Bus session that activates nothing,
private XDG directories under `.run/<name>`, a clean environment). Nothing was captured from, typed into, scripted
in or read from the session's own KWin, clipboard or index; no window was shown on the session's screen.

Stack: KWin 6.7.5, plasma-workspace 6.7.5, Qt 6.11.2, KF6 6.30 (KService, Baloo), xdg-desktop-portal 1.22.1,
xdg-desktop-portal-kde 6.7.5, GTK 3.24.52 with gtk-layer-shell 0.10.0 (crate 0.8.2), at-spi2-core 2.60.7,
wayland-client 0.31 with bindings generated from plasma-wayland-protocols' XML (`protocols/`), zbus 5.

## Verdicts

| Pt | Verdict | In one line | Consequence |
|---|---|---|---|
| a | **pending: needs the owner** | Nested pre-test only: Qt 6 apps join the AT-SPI bus at runtime when `IsEnabled` turns on. | WP15/WP16c: `owner-checks.sh a` decides Chromium's flag and the real cost. |
| b | **pending: needs the owner** | The probe ran against a mock `org.kde.klipper` only. | WP15's `clipboard_history`: `owner-checks.sh b`. |
| c | **verified** | `OnDemand` gets the keyboard at map, gives it back to the previous window when unmapped, loses it to a click elsewhere and regains it by a click. `Exclusive` behaves the same on KWin. | WP19: use `OnDemand`; arm Enter/Esc only after a short delay, since the focus moves at map. |
| d | **verified: yes** | A `.desktop` in the nested `XDG_DATA_HOME/applications` naming the dev binary's absolute path grants all three Wayland interfaces and ScreenShot2. A file written while KWin runs takes effect about 1.4 s later. | WP14: a dev build can hold the grants. Ruling 16's "installed Kai only" is a choice now, not a limit (the owner decides). |
| e | **verified** | `CaptureWindow` takes the window management protocol's uuid as given (`{…}`) or without braces. It captures a covered window's own content, and a minimized one. | WP15: the `see_window` handle is the uuid. No mapping is needed. |
| f | **verified** | `org.kde.kwin.Scripting` needs **no grant**. Load, run and the reply over `callDBus` take about 5 ms. Geometry, quick tiling, maximize and custom tiles all work. Ids equal the uuids. | WP14: `kwin_script.rs` works for any build. Kai needs a D-Bus object to receive results. |
| g | **verified** | `baloo_file` has no query method on D-Bus. Two paths work without KRunner: the `baloosearch6` CLI (reads the index directly, about 15 ms) and the baloo runner's `org.kde.krunner1.Match` (about 5 ms, but **capped at 10 results**). | WP15 `find_files`: use `baloosearch6`. The runner's hidden cap rules it out as the main path. |
| h | **verified: both work** | fake_input v6 `keyboard_keysym` typed `Hallo Welt: äöü ß € ñ ç 日本 😀` exactly into GTK 3 and Qt 6. The portal does the same, but `Start` shows a "Remote Control" dialog unless the app is pre-authorised or holds a restore token. | WP16c: fake_input in the `plasma` provider, the portal in `fdo`. |
| i | **skipped** | WP8's `just tool-cost` produces the token figure. | |

## (c) Layer-shell keyboard on demand for the approval overlay
`run-c.sh`. A focused GTK test window (an entry) at the back. The overlay is a 400x160 gtk-layer-shell surface,
namespace `kai-approval`, unanchored (centred), with Enter = APPROVE and Esc = DECLINE. Keys and clicks come from
fake_input. Each keyboard mode was tried on OVERLAY and on TOP, in two orders each. Focus-in/out is GTK's
`wl_keyboard` enter/leave. Raw log: `.run/c.log`.

| Mode (both layers) | At map | Enter with no click | Click on the window behind, type `c` | Click on the overlay, Esc | After unmap |
|---|---|---|---|---|---|
| `OnDemand` | overlay gets the keyboard at once (≤2 ms); the window gets leave | overlay: APPROVE | window gets the keyboard and the `c`; overlay leave | overlay gets it back: DECLINE | keyboard back on the window: the next key lands there |
| `Exclusive` | same | same | **same as OnDemand**: KWin 6.7.5 does not hold the keyboard on the surface | same | same |
| `None` | nothing | goes to the window | window | click events reach the overlay, the keys do not | — |

What this means for WP19:
* `OnDemand` gives exactly §11.1's behaviour (Enter and Esc without a click, click-away works) and is the mode to
  use. `Exclusive` adds nothing on KWin.
* Because the keyboard moves at map, a key the owner happens to type in another app at that moment lands on the
  overlay. An Enter meant for a chat window would approve. So the overlay needs an arming delay (keys ignored for a
  few hundred ms after map, or only a press *and* release that both come after map count), and it should show it.
* The layer (OVERLAY or TOP) does not change the keyboard behaviour.

## (d) Restricted interfaces for a dev build
`run-d.sh`. Every case gets a fresh nested KWin. The probe is this spike's debug binary, which checks for itself
(`p3 globals`, `p3 shot workspace`). The `.desktop` carries ruling 16's two keys. Raw log: `.run/d.log`.

| Case | `org_kde_plasma_window_management`, `zkde_screencast_unstable_v1`, `org_kde_kwin_fake_input` | `ScreenShot2.CaptureWorkspace` |
|---|---|---|
| 1 no `.desktop` | not offered | `NoAuthorized` |
| 2 `.desktop` in `XDG_DATA_HOME/applications` before KWin starts, `Exec=<abs path> %u` | offered (v20, v6, v6) | ok |
| 3 written while KWin runs | new connections get the grant **1356 to 1374 ms** after the write (3 runs; KSycoca's refresh); removed: revoked for new connections | follows the same |
| 4 `Exec=` a symlink to the binary | offered (KWin compares canonical paths) | ok |
| 5 `Exec=p3` (bare name, on PATH) | **not offered**: KWin does not search PATH | `NoAuthorized` |
| 6 `.desktop` naming another binary | not offered | `NoAuthorized` |

What this means for WP14:
* The grant follows the executable's canonical path, from any `.desktop` in the XDG data dirs, including the
  user's own `~/.local/share/applications`. `just install-dev` already writes such a file for `target/debug/kai`.
  With ruling 16's keys added, a dev build holds the grants. "An installed Kai only" is therefore a choice, not a
  limit. Whether `install-dev` carries the keys is the owner's call; the spec is not edited here.
* Each worktree's binary is a different path, so a grant belongs to the path the `.desktop` names.
* The packaged `Exec=@EXEC@ %u` must stay an absolute path (it does: `/usr/bin/kai`). Field codes are fine.
* A Wayland connection is judged when it binds. A Kai started before its `.desktop` existed must reconnect.
  ScreenShot2 judges the calling pid on each call; caching across calls was not tested.
* The error a build without the grant answers (§11.1) can name the fix ("KWin grants <interface> per .desktop file").

## (e) ScreenShot2 `CaptureWindow` by the window management uuid
`run-ef.sh`. A green window behind a blue one. The probe lists windows over `org_kde_plasma_window_management`
(`window_with_uuid`, then `get_window_by_uuid`) and captures through `org.kde.KWin.ScreenShot2` with a pipe.
* The uuid arrives as `{a52066e9-…}`. `CaptureWindow` accepts it as given, and also without braces.
  Covered green window: 640x400, stride 2560, QImage format 6 (ARGB32 premultiplied), its own green
  (sampled 30,160,60), not the blue window on top of it. 5 to 8 ms in the software-rendered nested KWin.
* `include-decoration`: 640x428 (the 28 px title bar). `CaptureActiveWindow`: the blue one. An unknown uuid:
  `org.kde.KWin.ScreenShot2.Error.InvalidWindow`. The reply also carries `windowId`.
* Minimized (through KWin scripting): still captured, with the last buffer's content (an app may stop drawing while
  minimized, so the image can be stale).
* The same string is KWin's `internalId` (scripting) and the `uuid` of `org.kde.KWin.getWindowInfo` (see (f)).
  K22's window resolution can therefore use one id for every call. `getWindowInfo` also reports
  `excludeFromCapture`; capturing such a window was not tested.

## (f) `org.kde.kwin.Scripting` for geometry and tiling
`run-ef.sh` and `kwin_script.py` (python3, no `.desktop`, so **no grant**), with the scripts in `kwin-scripts/`.
The path Kai would take:
1. Write the script to a file of its own (`common.js` plus the operation).
2. Call `loadScript(path, plugin)`. The call is overloaded (`s` and `ss`); pass the signature explicitly.
3. Call `/Scripting/Script<id>` `org.kde.kwin.Script.run()`.
4. The script answers through `callDBus(<Kai's name>, "/", iface, "Result", json)`. A KWin script cannot return
   a value to the loader, and `print()` only reaches KWin's log.
5. Call `unloadScript(plugin)`. Afterwards no script object is left under `/Scripting`.

* **Round trip** (load, run, reply): 4.4 to 8.9 ms, median 5.0 ms (10 queries).
* **Ids:** `internalId` equals the window management uuid for both windows.
* **Geometry:** `w.frameGeometry = {x:40,y:40,width:500,height:300}` works (the protocol reports 40,40 500x300).
  Reading it back in the same script still gives the old value, because Wayland configures asynchronously. Verify
  with the next query or `frameGeometryChanged`.
* **Quick tiling:** `workspace.activeWindow = w; workspace.slotWindowQuickTileLeft()` (or Right) puts the windows
  at 0,0 640x720 and 640,0 640x720. Side effect: the window is activated. There is no readable `quickTileMode`
  property (only its signal); `w.tile` reflects the quick tile instead.
* **Maximize:** `w.setMaximize(true, true)` gives 1280x720, and `tile` becomes null.
* **Custom tiling:** in a fresh configuration, `workspace.tilingForScreen(w.output).rootTile` already holds KWin's
  default three columns (320/640/320). `w.tile = rootTile.tiles[0]` puts the window at 4,4 314x712 (tile
  padding 4).
* **Other APIs present:** `sendClientToScreen`, `slotWindowToNextScreen`, `slotWindowMinimize`/`Maximize`/
  `FullScreen`/`Close`, `minimized`/`fullScreen`/`keepAbove`, `desktops`, `output`.
* **Without any grant:** `org.kde.KWin` `/KWin` `getWindowInfo(uuid)` and `queryWindowInfo` return caption,
  resourceClass, desktopFile, pid, geometry, minimized, fullscreen, keepAbove, desktops and excludeFromCapture.
  Together with scripting, that is a Windows capability with no grant at all, but without live events (those
  come from the window management protocol).
* Scripting is open to every process on the session bus. That is the session boundary, not something Kai adds.

## (g) Baloo's query path without KRunner
`run-g.sh`. A private Baloo with its own HOME, `baloofilerc`, index under the nested `XDG_DATA_HOME` and bus.
It indexed 4 test files; 6 entries counting the folders, done in 6.3 s from a cold start.
* **`baloo_file`'s D-Bus surface** is control only: `org.kde.baloo.fileindexer` (monitor, currentFile),
  `org.kde.baloo.scheduler` (suspend, resume, state), `org.kde.baloo.main` (quit, updateConfig). It has no query.
* **Path 1, `baloosearch6`**, reads the index (LMDB) directly:
  * content words, `-d <dir>`, `-t Document`, `-l N` and `filename:quarterly*` all work;
  * about 15 ms per call, process start included (the query itself about 0.1 ms);
  * 29 of 29 hits with no `-l`;
  * works with `baloo_file` stopped, and still answers from the old index with `Indexing-Enabled=false`, so Kai
    must read the status itself (`balooctl6 status` or `baloofilerc`) to say "index off or stale";
  * output is one path per line plus an `Elapsed:` line, so a file name containing a newline is ambiguous.
* **Path 2, the baloo runner over D-Bus** (`org.kde.runners.baloo` `/runner` `org.kde.krunner1.Match(s)`; no
  KRunner process; on a real session it is D-Bus-activated as `plasma-baloorunner.service`):
  * about 5 ms per call;
  * structured results: `a(sssida{sv})` with the file URL, name, icon, relevance and `category`/`subtext`/`urls`;
  * **at most 10 results** (29 matching files gave 10).
* New files: found 0.5 s after creation (1 file) and 1.05 s after (25 files), with the indexer running.
* For WP15: `find_files` runs `baloosearch6` with an explicit `-l` that it reports (the project's rule: no silent
  caps), plus `-d` and `-t`. The runner's silent 10-result cap rules it out as the main path. `find_files` should
  say that only the indexed folders (`baloofilerc`'s `folders`) are searched. A third path, KF6Baloo's C++
  `Baloo::Query`, would need a C++ shim and is not needed.

## (h) RemoteDesktop portal against `org_kde_kwin_fake_input` for `type_text` and `key_press`
`run-h.sh`. A focused GTK entry, plus `kdialog --inputbox` (Qt 6) as a Qt target. The portal stack
(`xdg-permission-store`, `xdg-desktop-portal-kde`, `xdg-desktop-portal`) runs on the nested bus;
portal-kde gets fake_input through its own `/usr/share` `.desktop`.

| | fake_input (the probe's own grant, v6) | RemoteDesktop portal (v2, AvailableDeviceTypes 7) |
|---|---|---|
| `type_text` "Hallo Welt: äöü ß € ñ ç 日本 😀" | exact in GTK and Qt (`keyboard_keysym`, Latin-1 = codepoint, others 0x01000000+cp), no keymap work by the client | exact in GTK and Qt (`NotifyKeyboardKeysym`) |
| `key_press` | `keyboard_key` with evdev codes; chords as all-down, reverse-up (ctrl+a, BackSpace worked) | `NotifyKeyboardKeycode` (evdev) worked; keysyms also |
| pointer | `pointer_motion_absolute` + `button` worked (used in (c)) | (not tried) |
| consent | none at use time; the `.desktop` grant is the consent | `Start` opens a **"Remote Control" dialog** (portal-kde window) every session and blocks, unless pre-authorised: permission store `kde-authorized` / `remote-desktop` / `<app id>` = `yes` (a one-time owner command) or a **restore token** (`persist_mode` 2): a token issued once kept working after the pre-authorisation was revoked |
| app id | n/a | the host registry (`org.freedesktop.host.portal.Registry.Register`) refuses an id without a `.desktop` file (`App info not found`); Kai's app scope gives it anyway (WP0 (g)) |
| timing | 28 chars in about 0.9 s (a round trip and 2x15 ms pause per char, the probe's choice) | `Start` answered in 2 to 5 ms when pre-authorised; 28 chars in 0.45 s |
| other | — | `ConnectToEIS` returns an fd (a libei path exists; not driven); a real session shows portal-kde's "remote control" indicator (from its strings, not observable here) |

For WP16c:
* The `plasma` provider types through fake_input. It has no prompt, needs no keymap handling and is synchronous,
  and ruling 16 already lists the interface; per (d) a dev build can hold it.
* The `fdo` provider (K23 already lists the RemoteDesktop portal) uses the portal with a stored restore token.
  Its first session shows the dialog once (its "allow restore" checkbox issues the token; that dialog path was
  not clicked through here).
* K15's "confirmed by default" stays the only gate on Kai's side in both cases.

## (a) AT-SPI: pending, needs the owner
The owner runs `owner-checks.sh a`. Step 1 is read-only. Step 2 switches `org.a11y.Status` `IsEnabled`, then
`ScreenReaderEnabled`, for about a minute and restores both on exit and on Ctrl+C. **Both are GSettings-backed**
(`toolkit-accessibility`, `screen-reader-enabled`), i.e. persistent settings, which is why the restore matters.
The output is per-app tree sizes, roles and walk times, and CPU and RSS of the named apps in each state. It
covers Chromium plain and with `--force-renderer-accessibility`.

Nested pre-test (`run-a-pretest.sh`, test apps only; a private AT-SPI bus, whose `org.a11y.Status` needed
`GSETTINGS_BACKEND=memory` there because there is no dconf service):
* Qt 6.11 (`kdialog`) joins the bus at runtime once `IsEnabled` turns true, with no restart. Apps started while
  it is on join too. `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` joins regardless.
* GTK 3 is always on the bus (unless `NO_AT_BRIDGE`). KWin registers itself (one node).
* The cost was not measurable on a 7-node dialog (CPU 0 to 0.3 %, RSS unchanged).

The real trees (Dolphin, Kate, Firefox, Chromium) and their cost are the owner's run.

## (b) Klipper's D-Bus history: pending, needs the owner
The owner runs `owner-checks.sh b` (`klipper_probe.py`). It is read-only and reports:
* who owns `org.kde.klipper` (plasmashell in Plasma 6, expected) and the method signatures on `/klipper`;
* item counts from `getClipboardHistoryMenu` and `getClipboardHistoryItem(i)`, and whether the two agree;
* lengths, empty items (images?), multi-line items, timings, and sha1 fingerprints.

It prints no clipboard text unless `--show` is given, and then only on the terminal. It was checked against a mock
service in the nested session.

## Spec items touched by the findings (the spec is not edited here)
* Ruling 16, §11.1 "An installed Kai only", §13: a `.desktop` in the user's data dir grants a dev build (d). Whether
  `install-dev` should carry the keys is open to the owner.
* §11.1 approvals: the keyboard moves at map (c), so WP19 needs an arming delay before Enter or Esc count.
* K23 Windows: KWin scripting plus `getWindowInfo` work with no grant (f). The window management protocol adds
  events.
* §11.2 KRunner note: the baloo runner is a D-Bus runner reachable without KRunner, but capped at 10 results (g).

## Files
* `src/`: the probe binary `p3` (globals, windows, fake input, ScreenShot2, test window, layer-shell overlay).
  Build it with `cargo build --offline -j 4`.
* `nested.sh`: the nested-session runner.
* `run-c.sh`, `run-d.sh`, `run-ef.sh`, `run-g.sh`, `run-h.sh`: one driver per point.
* `run-a-pretest.sh`: the nested pre-test for (a).
* `kwin_script.py`, `kwin-scripts/`, `rd_portal.py`: helpers.
* `owner-checks.sh`, `atspi_probe.py`, `klipper_probe.py`: the owner's points.
* Raw logs stay in `.run/` (ignored; removed after the runs). The owner's results go to `results/` (ignored).
