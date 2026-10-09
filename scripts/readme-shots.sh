#!/usr/bin/env bash
# The README's screenshots, from stand-in content only, in nested KWins (scripts/nested-kwin.sh: their
# own bus and XDG directories, nothing on the session's screen, the session never captured) and a
# private PipeWire instance (scripts/pw-private.sh: nothing audible):
#   docs/images/desktop.png    one 1920x1080 output at 125 % on a plain backdrop
#                              (scripts/readme-backdrop.py): the ring while Kai speaks, a weather
#                              card from a loopback stand-in, a text overlay
#   docs/images/approval.png   the same screen: the approval card for a type_text call
#   docs/images/settings.png   the settings window's Tools page (its own 125 % zoom), on a
#                              configuration with Kai's defaults, unpaired
# Needs kwin_wayland, kscreen-doctor, spectacle, magick, python3-gobject with gtk-layer-shell, and the
# window's page built (`just ui`). No lmgw is started or contacted.
#
# usage: scripts/readme-shots.sh   (`just readme-shots`; raw captures and logs in target/readme-shots/)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/readme-shots
cd "$here"
for c in kwin_wayland kscreen-doctor spectacle magick; do
    command -v "$c" > /dev/null || { echo "needs $c" >&2; exit 1; }
done
[[ -f ui/dist/index.html ]] || { echo "ui/dist is not built: just ui" >&2; exit 1; }
mkdir -p "$out"
rm -rf "$out/shots" "$out"/*.log
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])' | tail -1)
[[ -x "$test_bin" ]] || { echo "no test binary" >&2; exit 1; }

nv=()
if [[ -e /dev/nvidiactl || -e /sys/module/nvidia ]]; then
    nv=(__NV_DISABLE_EXPLICIT_SYNC="${__NV_DISABLE_EXPLICIT_SYNC:-1}")
fi

# ---- the desktop: overlays on one output at 125 %, over the backdrop
# shellcheck disable=SC2016
desktop='python3 -I "$1/scripts/readme-backdrop.py" 2> /dev/null &
backdrop=$!
sleep 1
status=0
"$2" --ignored --exact widgets::live_tests::readme::readme_desktop_nested --nocapture \
    --test-threads=1 > "$3/desktop.log" 2>&1 || status=$?
kill "$backdrop"
wait "$backdrop" 2> /dev/null || true
exit $status'
# pw-private.sh clears the environment: the output's geometry goes through env.
scripts/pw-private.sh "$out/pw" -- env KAI_NESTED_OUTPUTS=1 KAI_NESTED_WIDTH=1536 \
    KAI_NESTED_HEIGHT=864 KAI_NESTED_SCALE=1.25 "$here/scripts/nested-kwin.sh" "$out/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$out" \
    PIPEWIRE_RUNTIME_DIR="$out/pw/rt" GDK_BACKEND=wayland "${nv[@]}" \
    bash -c "$desktop" readme "$here" "$test_bin" "$out" \
    || { echo "FAIL the desktop shots (see $out/desktop.log)" >&2; exit 1; }

# ---- the settings window in KWin's default frame, with Kai's icon
# shellcheck disable=SC2016
window='mkdir -p "$XDG_DATA_HOME/applications" "$XDG_DATA_HOME/icons/hicolor/scalable/apps"
cp "$1/assets/icons/hicolor/scalable/apps/dev.rootshell.kai.svg" "$XDG_DATA_HOME/icons/hicolor/scalable/apps/"
printf "[Desktop Entry]\nType=Application\nName=Kai\nIcon=dev.rootshell.kai\nExec=true\nNoDisplay=true\n" \
    > "$XDG_DATA_HOME/applications/dev.rootshell.kai.desktop"
"$2" --ignored --exact settings::window::live_tests::readme_window_nested --nocapture \
    --test-threads=1 > "$3/window.log" 2>&1'
KAI_NESTED_OUTPUTS=1 KAI_NESTED_WIDTH=1600 KAI_NESTED_HEIGHT=900 KAI_NESTED_SCALE=1 \
    scripts/nested-kwin.sh "$out/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$out" \
    GDK_BACKEND=wayland "${nv[@]}" bash -c "$window" readme "$here" "$test_bin" "$out" \
    || { echo "FAIL the window shot (see $out/window.log)" >&2; exit 1; }

# ---- into docs/images/, metadata stripped, recompressed
mkdir -p docs/images
for pair in desktop:desktop approval:approval window-tools:settings; do
    from=$out/shots/${pair%%:*}.png
    to=docs/images/${pair#*:}.png
    [[ -s $from ]] || { echo "FAIL no capture $from" >&2; exit 1; }
    magick "$from" -strip -define png:compression-level=9 -define png:compression-filter=5 "$to"
    echo "$to $(magick identify -format '%wx%h' "$to") $(du -k "$to" | cut -f1) KiB"
done
