# Kai: build, test, dev install, run.  `just --list` shows the recipes.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Bounds peak memory of parallel builds.
export CARGO_BUILD_JOBS := "8"

data_home := env("XDG_DATA_HOME", env("HOME") / ".local/share")
apps := data_home / "applications"
icons := data_home / "icons/hicolor/scalable"
exe := justfile_directory() / "target/debug/kai"

default:
    @just --list

build:
    cargo build

# Every test of the workspace (PD1): Kai and crates/kai-desktop.
test:
    cargo test --workspace

# lmgw's checkout next to this repository away from LMGW_PIN is warned about first, never failed.
[doc("Formatting, clippy with warnings as errors, the tests, the ring's page against lmgw, the publish script, the window's page")]
check:
    scripts/lmgw-pin.sh check
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace --all-targets --features live-test -- -D warnings
    cargo test --workspace
    cargo test --workspace --doc
    scripts/viz-check.sh
    scripts/dev-grant-check.sh
    python3 scripts/publish_github_test.py
    just ui-check
    just ui

# The configuration window's page (ui/, WP21b; Leptos, wasm): trunk builds it into ui/dist, which the
# kai binary reads from disk (a debug build) or embeds (a release build). Needs trunk and the
# wasm32-unknown-unknown target; lmgw's checkout next to this repository supplies lmgw-ui-kit.
[doc("Build the configuration window's page (ui/dist; trunk, debug)")]
ui:
    cd ui && trunk build

# The page's own gates: formatting, clippy (native and wasm, warnings as errors) and the model's
# tests, which run natively.
[doc("Format check, clippy and tests of the configuration window's page")]
ui-check:
    cd ui && cargo fmt --check
    cd ui && cargo clippy --all-targets -- -D warnings
    cd ui && cargo clippy --target wasm32-unknown-unknown -- -D warnings
    cd ui && cargo test

# WP21b's live check: the window in a nested KWin (nothing on the session's screen), driven through
# its DOM against the real operations on a temporary configuration; screenshots at 125 % in
# target/live-window/shots/. The Personality page against a dev lmgw on 127.0.0.1:8897 (lmgw's
# checkout needs its headless example built); its shot in target/window-shots/. Run `just ui` first.
[doc("Live check of the configuration window in a nested KWin (screenshots in target/live-window/)")]
live-check-window lmgw="../lmgw":
    scripts/live-check-window.sh "{{lmgw}}"

# The README's screenshots (docs/images/) from stand-in content only, in nested KWins (nothing on the
# session's screen) and a private PipeWire instance; no lmgw. Run `just ui` first.
[doc("Regenerate the README's screenshots in docs/images/ (nested KWin, stand-in content)")]
readme-shots:
    scripts/readme-shots.sh

# The .desktop file (scheme handler x-scheme-handler/lmgw-pair, KWin interface keys) and the icons,
# pointing at the debug build. Dev runs need the .desktop file too: without it the portal refuses
# the app id (K12, WP0 (g)).
[doc('Install the .desktop file and icons for dev runs (points at target/debug/kai)')]
install-dev: build
    # Without ruling 16's restricted-interface keys: a dev build holds them only by opting in,
    # `just grant-dev` (§16 Q21 as answered, 2026-10-09). Refused while this build's grant exists:
    # KWin reads the first entry naming a binary, so the grant would hold by chance.
    scripts/dev-grant.sh installable "{{exe}}"
    install -d "{{apps}}" "{{icons}}/apps" "{{icons}}/status"
    sed -e 's|@EXEC@|{{exe}}|' -e '/^X-KDE-.*Interfaces=/d' -e '/^# Ruling 16/d' \
        packaging/dev.rootshell.kai.desktop.in > "{{apps}}/dev.rootshell.kai.desktop"
    install -m644 assets/icons/hicolor/scalable/apps/dev.rootshell.kai.svg "{{icons}}/apps/"
    install -m644 assets/icons/hicolor/scalable/status/kai-tray-*.svg "{{icons}}/status/"
    update-desktop-database "{{apps}}"
    @echo "installed {{apps}}/dev.rootshell.kai.desktop -> {{exe}}"

[doc('Remove what install-dev installed')]
uninstall-dev:
    rm -f "{{apps}}/dev.rootshell.kai.desktop" "{{icons}}/apps/dev.rootshell.kai.svg"
    for i in idle offline held listening thinking speaking muted; do rm -f "{{icons}}/status/kai-tray-$i.svg"; done
    rmdir --ignore-fail-on-non-empty "{{icons}}/status" "{{icons}}/apps" "{{icons}}" 2>/dev/null || true
    update-desktop-database "{{apps}}"

# The debug build in its own app scope, app-dev.rootshell.kai-dev<n>.scope with no dash in <n>, so
# the portal reads the app id dev.rootshell.kai (K12). Runs in the foreground; Ctrl+C quits cleanly.
[doc('Run the debug build in its own app scope (Ctrl+C quits)')]
run *args: build
    systemd-run --user --scope --quiet --collect -u "app-dev.rootshell.kai-dev$(date +%s).scope" -- "{{exe}}" {{args}}

# WP3's live check: everything in a private PipeWire instance (scripts/pw-private.sh), nothing
# audible, the session's defaults untouched; reports in target/live-audio/.
[doc('Live audio check in a private PipeWire instance (a 24 kHz mono PCM16 clip)')]
live-check-audio clip:
    scripts/live-check-audio.sh "{{clip}}"

# WP4's live check: the voice session's core with real PipeWire streams in the private instance,
# against the scripted lmgw (a cold push-to-talk bind, a reply on the sink, a barge-in).
[doc('Live check of the voice session in a private PipeWire instance (a 24 kHz mono PCM16 clip)')]
live-check-voice clip="target/live-audio/clip.wav":
    KAI_LIVE_ONLY=live_voice scripts/live-check-audio.sh "{{clip}}"

# WP4's real-lmgw half: the voice session against a dev lmgw on 127.0.0.1:8897 whose chat, ASR and
# TTS are mocks (no model, no GPU), in the private PipeWire instance: round trips, a barge-in, K16, a
# takeover, a restart. The dev lmgw's headless example is built in lmgw's checkout beforehand. Report
# in target/live-voice-lmgw/.
[doc('Live check of the voice session against a dev lmgw with mocked models (a 24 kHz mono PCM16 clip)')]
live-check-voice-lmgw clip="target/live-audio/clip.wav" lmgw="../lmgw":
    scripts/live-check-voice-lmgw.sh "{{clip}}" "{{lmgw}}"

# WP4: the shortcuts portal up to the bind in Kai's app scope, with no dialog and nothing registered.
[doc("Dialog-free live check of the shortcuts portal (CreateSession, ListShortcuts, the app id)")]
live-check-shortcuts:
    scripts/live-check-shortcuts.sh

[doc("The owner's check of the shortcuts: KDE's bind dialog, real presses, cleanup (asks first)")]
owner-check-shortcuts:
    scripts/owner-check-shortcuts.sh

[doc("The owner's audible audio checks and app AEC on the real devices (asks before each step)")]
owner-check-audio *clip:
    scripts/owner-check-audio.sh {{clip}}

# The ring's page (§9.2): engine.js and ring.js vendored byte-identical from lmgw (assets/overlay/viz,
# pinned in its PIN file). lmgw's main checkout next to this repository is read, never written. Next
# to a public clone of lmgw (no vendoring commit) it skips itself with one line saying why.
[doc("The ring's vendored files against lmgw (drift), the levels reference, the adapter and palette")]
viz-check lmgw="../lmgw":
    scripts/viz-check.sh "{{lmgw}}"

[doc("Copy ring.js and engine.js again from lmgw main and rewrite the pin")]
viz-vendor lmgw="../lmgw":
    scripts/viz-vendor.sh "{{lmgw}}"

# WP5's live check: the ring over the desktop for a few seconds per state, its levels from a private
# PipeWire instance (nothing audible); crops of the ring's rectangle in target/live-ring/. Then the
# ring's output away and back in a nested KWin of its own (scripts/nested-kwin.sh).
[doc('Live check of the ring over the desktop (a 24 kHz mono PCM16 clip)')]
live-check-ring clip="target/live-audio/clip.wav":
    scripts/live-check-ring.sh "{{clip}}"

# lmgw's engine.js is read from its main checkout next to this repository.
[doc("Regenerate src/audio/levels_reference.json from lmgw's engine.js")]
viz-reference engine="../lmgw/crates/lmgw-ui/assets/voice/viz/engine.js":
    node scripts/viz-reference.mjs "{{engine}}" > src/audio/levels_reference.json

# WP2's live check: Kai's live-test build against a dev lmgw on 127.0.0.1:8897 (build its headless
# example first): pairing, the hold badge, the dev lmgw stopping and starting, disable and expiry
# waiting with the key kept (Try again), rotate "pair again", each read by its kind, the old item
# deleted on a re-pair. Verdicts in target/live-link/.
[doc("Live check of the lmgw link against a dev lmgw on 127.0.0.1:8897")]
live-check-link lmgw="../lmgw":
    scripts/live-check-link.sh "{{lmgw}}"

# WP6's live check: the Kai conversation from a first run against a dev lmgw on 127.0.0.1:8897 with
# mocked models, Kai's audio in the private PipeWire instance: the folder created and the owner's
# steps said, recovery on folder.updated, Open in lmgw's URL (no browser), the idle rollover, a new
# conversation, a 4004 when the owner takes the thread out of the device's reach (Kai binds the
# folder's current thread again), K5 against a second device key. About three minutes. Report in
# target/live-conversation/.
[doc('Live check of the Kai conversation against a dev lmgw with mocked models (a 24 kHz mono PCM16 clip)')]
live-check-conversation clip="target/live-audio/clip.wav" lmgw="../lmgw":
    scripts/live-check-conversation.sh "{{clip}}" "{{lmgw}}"

# The RPM (packaging/kai.spec) built in a Fedora podman container of the host's release; the image
# (packaging/Containerfile) is built once. Only target/rpm is mounted (:Z): staged sources, the
# cargo registry and target caches, the output in target/rpm/out/x86_64/. The window's page (ui/) is
# built there with trunk (`packaging/build-rpm.sh --page-only` stops after it). lmgw-client,
# lmgw-api-types, lmgw-update and lmgw-ui-kit come from the tree LMGW_PIN names, read with `git archive` from
# the lmgw checkout Cargo.toml's path names (or LMGW_DIR), whatever that checkout has checked out.
# KAI_UPDATE_MANIFEST_URL / KAI_UPDATE_DEPLOY_TOKEN, when set, bake in an update feed; unset (a
# local build), the RPM has none.
[doc('Build the kai RPM in a Fedora podman container (target/rpm/out/)')]
rpm:
    packaging/build-rpm.sh

# WP8's live check: Kai's hosted MCP server against a dev lmgw on 127.0.0.1:8897 with lmgw's host
# link (M1) and the tool-calling mock (M7): the grant, the device's row Ready, /mcp listing Kai's
# tools to the owner and not to a deny key, a mock turn calling the live-test build's live_echo, the
# dev lmgw's restart and Kai's reconnect, no key or argument in the log. Verdicts in
# target/live-host/.
[doc("Live check of the hosted MCP server against a dev lmgw with M1 and M7")]
live-check-host lmgw="../lmgw":
    scripts/live-check-host.sh "{{lmgw}}"

# PD6, K22: the token cost of Kai's whole tool list as lmgw sends it to a model under <label>,
# through lmgw's /v1/count_tokens for each alias (none named: the Kai folder's model). A figure,
# not a cap. LMGW_URL (default: lmgw.url) and LMGW_KEY (a key holding Inference) from the
# environment; the key goes as a header from a pipe, never in argv.
[doc("Count the hosted tool list's tokens through lmgw's /v1/count_tokens (LMGW_KEY needed)")]
tool-cost label *aliases:
    scripts/tool-cost.sh "{{label}}" {{aliases}}

# WP14's live check: the desktop provider and the window tools in a nested KWin with two virtual
# outputs (its own bus and XDG directories; nothing on the session's screen, the session's KWin
# never scripted or read): the list, desktop_context, activate, move between outputs and desktops,
# tile, maximize, fullscreen, minimize, close, the resolution's errors; once with KWin scripting
# and once with the window management protocol granted by a .desktop in the nested data
# directory; then `kai --tool-list` there. Report in target/live-desktop/.
[doc("Live check of the desktop tools in a nested KWin (scripting, then the granted protocol)")]
live-check-desktop:
    scripts/live-check-desktop.sh

# WP15's live check: see_window and see_screen without and with ScreenShot2's grant, read_window
# and read_selection over a private AT-SPI bus, Kai's accessibility switch around a read of a Qt
# dialog that follows it (K33), find_files over a private Baloo index, recent_files; all in a nested
# KWin with its own home. Report and captures in target/live-read/.
[doc("Live check of the seeing and reading tools in a nested KWin")]
live-check-read:
    scripts/live-check-read.sh

# WP10's live check of the tool-controlled overlays in a nested KWin with two virtual outputs
# (nothing on the session's screen) inside a private PipeWire instance (nothing audible): the
# sandbox's refusals against a loopback server, no sound (the APIs and PipeWire), no dialog, WebGL
# or fullscreen, PSS of the first and a second widget, every position on both outputs against KWin's
# geometry with crops, overlay_list, expiry, an output going away, the screen lock (the surfaces
# unmapped while locked, back after), no web process after the last
# widget, a script that never yields, a widget's crash beside the ring, no content in a trace log,
# and the input regions from a Wayland log; WP12's MCP Apps bridge (the postMessage path, then
# overlay_app against a dev lmgw on 127.0.0.1:8897 with scripts/mcp-apps-fixture.py, then a call in
# Kai's conversation showing its view with its input and result; lmgw's checkout needs its headless
# example built). Report, crops and full-output shots (shots/full-*.png) in target/live-widgets/.
[doc("Live check of the overlays in a nested KWin (sandbox, sound, hangs, input, positions, PSS, apps)")]
live-check-widgets lmgw="../lmgw":
    scripts/live-check-widgets.sh "{{lmgw}}"

# WP16a's live check: K14 on the session's user manager (a non-GUI program survives kill -9 of its
# starter, in its own scope) and GUI launches only in a nested KWin; report in target/live-launch/.
[doc('Live check of open, app_launch and run_command (real scopes; GUI only in a nested KWin)')]
live-check-launch:
    scripts/live-check-launch.sh

# WP16c's live check of the input tools in a nested KWin (nothing is typed into the session): KWin's
# fake input without and with the .desktop grant, text into GTK and Qt entries, chords, a dropped
# call, ui_press; then the RemoteDesktop portal on the private bus with its dialog wait,
# pre-authorisation and stored restore token. Report in target/live-input/.
[doc("Live check of type_text, key_press and ui_press in a nested KWin (fake input and the portal)")]
live-check-input:
    scripts/live-check-input.sh

# WP20's live check: a routine's script in a real scope of the session's user manager calls a test
# tool through the real `kai` binary and the test's own CLI socket (never the running Kai's); the
# cgroup names the caller. Report in target/live-routines/.
[doc("Live check of a routine's script in a real scope calling kai tool (the cgroup names it)")]
live-check-routines:
    scripts/live-check-routines.sh

# WP19's live check: the approval card and the spoken answer against a dev lmgw on 127.0.0.1:8897
# (mock chat with TOOL_SCRIPT, the speech-to-text mock answering "Ja."), in a nested KWin with a
# private bus and PipeWire: the card's arming, Enter and Esc, the route, a self-approval meeting
# Kai's dialog, the bound session's answer by voice, the screen lock from a stand-in screen saver
# service (K32). Report and a crop in target/live-approvals/.
[doc("Live check of approvals (card, keys, route, voice answer) in a nested KWin against a dev lmgw")]
live-check-approvals clip="target/live-audio/clip.wav" lmgw="../lmgw":
    scripts/live-check-approvals.sh "{{clip}}" "{{lmgw}}"

# WP18's live check: Kai's asynchronous jobs against a dev lmgw on 127.0.0.1:8897 (MCP Tasks on
# the host link, late results, the bound session's continuation, Kai's own bind for a result, an
# owner's job notified, lmgw's cancel), Kai's audio in a private PipeWire instance; report in
# target/live-jobs/. lmgw's checkout needs its headless example built.
[doc("Live check of Kai's asynchronous jobs against a dev lmgw (report in target/live-jobs/)")]
live-check-jobs clip="target/live-audio/clip.wav" lmgw="../lmgw":
    scripts/live-check-jobs.sh "{{clip}}" "{{lmgw}}"

# §16 Q21 (the owner's answer, 2026-10-09): a dev build holds ruling 16's restricted interfaces only
# by opting in. KWin grants them to the binary a .desktop file in the applications directory names
# by its canonical absolute path (WP13 (d)), so each checkout or worktree gets its own file,
# dev.rootshell.kai-devgrant-<checkout>-<hash>.desktop (no menu entry, no scheme handler); new
# connections get the grant about 1.4 s after it appears. One entry per binary: KWin reads the first
# one naming it, so grant-dev refuses beside install-dev's stripped entry for the same build (and
# install-dev beside a grant). `scripts/dev-grant-check.sh` (in `check`) tests the files in a
# scratch HOME.
[doc("Grant this build (target/debug/kai) KWin's restricted interfaces: its own .desktop file")]
grant-dev: build
    scripts/dev-grant.sh grant "{{exe}}" "{{justfile_directory()}}"

[doc("Remove this build's grant file (what grant-dev wrote)")]
revoke-dev:
    scripts/dev-grant.sh revoke "{{exe}}" "{{justfile_directory()}}"

# Lists, never removes: a stale file (its binary gone with a removed worktree) is the owner's to delete.
[doc("List every Kai dev grant file: its binary gone (stale) or another entry shadowing it show")]
grants-dev:
    scripts/dev-grant.sh list
