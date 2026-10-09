#!/usr/bin/env bash
# The live check of Kai's ring overlay (WP5) over the owner's desktop.
#
# usage: scripts/live-check-ring.sh [clip.wav]   (24 kHz mono PCM16; default target/live-audio/clip.wav)
#        KAI_LIVE_ONLY_IDLE=1 runs step 3 only (nothing shows on the screen).
#        KAI_LIVE_NO_SHOTS=1 takes no screen capture at all (the ring still shows; nothing is
#        cropped or written from the screen).
#
# 1. In the session, read only: the outputs' connector names and KWin's active output
#    (src/overlay/outputs.rs, `live_outputs_and_kwin`); the page in a view that never maps: its
#    stylesheet applies and its CSP refuses inline script and style (`live_page_loads_under_its_csp`).
# 2. The overlay as Kai runs it, on the session's Wayland display, its levels from a private
#    PipeWire instance (scripts/pw-private.sh: nothing audible, the session's audio untouched),
#    driven through the voice session's facts (src/overlay/live_tests.rs): listening between turns
#    (the ring up; "ring over fullscreen" toggled while it is up; its cost over 10 s of silence and
#    10 s of a quiet room's noise, with kwin_wayland's CPU and the GPU's power and P-state from
#    `nvidia-smi`, read only, against 10 s with no view and 10 s hidden; how late it shows tone
#    bursts on the microphone, held to a bound from the feed interval and the frame rate), a turn
#    and back to listening, mute, its own web process killed twice and the timed retries after
#    that, the session's end, push-to-talk (shown from a press through its turn, hidden between
#    presses).
#    About two minutes in all, the ring up for about 80 s of them.
#    The configured placement is used, and KWin's active output is named as `ring.output`.
# 2b. The ring when its output goes away and comes back (a monitor that powers off and unplugs),
#    in a nested KWin with two virtual outputs (scripts/nested-kwin.sh: its own bus and XDG
#    directories, nothing on the session's screen, the session's outputs untouched): KWin itself is
#    asked where the ring is after each switch (src/overlay/live_tests/output_removed.rs). Last, no
#    output at all, which KWin never produces: Kai's output list is taken away while an output
#    goes under the ring. A few seconds, report in target/live-ring/nested-report.txt.
# 3. The real `kai` at idle, in the private instance (its own bus, config and audio; no tray on the
#    session's panel): no web process while no session is bound (K1), the tray's Overlays check
#    item through dbusmenu (it writes overlay.ring_over_fullscreen), and a clean quit through the
#    overlay's quit path.
# Crops of the ring's rectangle only (each full capture is deleted at once) and the report go to
# target/live-ring/; screen crops stay local and are never committed.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-ring
# shellcheck source=scripts/shortcuts-guard.sh
source "$here/scripts/shortcuts-guard.sh"
clip=$(realpath "${1:-$here/target/live-audio/clip.wav}")
[[ -f $clip ]] || { echo "no clip at $clip"; exit 2; }
[[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" ]] || { echo "no Wayland session"; exit 2; }
mkdir -p "$out/shots"
[[ -n "${KAI_LIVE_ONLY_IDLE:-}" ]] || rm -f "$out"/shots/*.png "$out/report.txt" "$out/nested-report.txt"

# The session's socket by absolute path: inside the private instance XDG_RUNTIME_DIR is its own.
wl=$WAYLAND_DISPLAY
[[ $wl == /* ]] || wl=$XDG_RUNTIME_DIR/$wl
active=$(busctl --user call org.kde.KWin /KWin org.kde.KWin activeOutputName 2>/dev/null \
    | sed -n 's/^s "\(.*\)"$/\1/p' || true)
config=${XDG_CONFIG_HOME:-$HOME/.config}/kai/config.toml
place=$(python3 -c '
import sys, tomllib
try:
    ring = tomllib.load(open(sys.argv[1], "rb")).get("ring", {})
except FileNotFoundError:
    ring = {}
print(ring.get("anchor", "bottom-right"), ring.get("margin", 24), ring.get("size", 240), sep=",")
' "$config")
echo "== placement $place on output ${active:-(none: the compositor chooses)}, from $config and KWin"

# One capture of the session's screen, cropped to the ring. Full captures live only in $full_dir,
# which this script owns and removes at the end. A spectacle that is already running (the
# session's own, D-Bus activated) would take the request over and write the capture later, after
# the shot cleaned up: that once left a full capture behind. So a shot is skipped while any
# spectacle runs (shots run one at a time, so none of them is ours).
full_dir=$out/full
rm -rf "$full_dir"
mkdir -m 700 "$full_dir"
cleanup() {
    sleep 1
    rm -rf "$full_dir"
    [[ ! -e $full_dir ]] && echo "== no full capture is left: $full_dir is removed"
}
trap 'cleanup; kgs_check || exit 1' EXIT
shot=$out/shot.sh
{
    echo '#!/usr/bin/env bash'
    echo '# usage: shot.sh <name> <x> <y> <w> <h>   (one at a time; the full capture is deleted)'
    echo 'set -euo pipefail'
    printf 'exec 9> %q\n' "$out/shot.lock"
    echo 'flock 9'
    echo 'if ps -C spectacle > /dev/null; then echo "shot $1: skipped, a spectacle instance runs" >&2; exit 0; fi'
    printf 'full=$(mktemp %q/full-XXXXXX.png)\n' "$full_dir"
    echo 'trap '"'"'rm -f "$full"'"'"' EXIT'
    printf 'env -i HOME=%q PATH=%q DBUS_SESSION_BUS_ADDRESS=%q XDG_RUNTIME_DIR=%q WAYLAND_DISPLAY=%q \\\n' \
        "$HOME" "$PATH" "$DBUS_SESSION_BUS_ADDRESS" "$XDG_RUNTIME_DIR" "$wl"
    echo '    spectacle -b -n -f -o "$full" > /dev/null 2>&1 || true'
    echo 'for _ in $(seq 30); do [[ -s $full ]] && break; sleep 0.1; done'
    echo '[[ -s $full ]] || { echo "shot $1: spectacle wrote no capture" >&2; exit 1; }'
    printf 'magick "$full" -crop "${4}x${5}+${2}+${3}" +repage %q/"$1.png"\n' "$out/shots"
} > "$shot"
chmod +x "$shot"

cd "$here"
export CARGO_BUILD_JOBS=8
cargo build -q
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])' | tail -1)
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }

webkit() { ps -eo comm= | grep -c '^WebKit' || true; }
webkit_before=$(webkit)

nv=()
if [[ -e /dev/nvidiactl || -e /sys/module/nvidia ]]; then
    nv=(__NV_DISABLE_EXPLICIT_SYNC="${__NV_DISABLE_EXPLICIT_SYNC:-1}")
fi
status=0
if [[ -z "${KAI_LIVE_ONLY_IDLE:-}" ]]; then
echo "== in the session: the outputs and KWin's active output (read only); the page in a view that never maps"
"$test_bin" --ignored --exact overlay::outputs::live::live_outputs_and_kwin --nocapture 2>&1 \
    | tee "$out/outputs.log" | grep -E "^(wl_output|KWin|test )" || status=1
env "${nv[@]}" "$test_bin" --ignored --exact overlay::live_tests::live_page_loads_under_its_csp \
    --nocapture > "$out/page.log" 2>&1 || status=1
grep -E "^(page:|test )" "$out/page.log" || true

echo "== the ring over the desktop, levels from a private PipeWire instance"
shot_cmd=$shot
[[ -z "${KAI_LIVE_NO_SHOTS:-}" ]] || { shot_cmd=""; echo "== KAI_LIVE_NO_SHOTS: no screen captures"; }
scripts/pw-private.sh "$out/run" -- env KAI_LIVE_RING=1 KAI_LIVE_CLIP="$clip" KAI_LIVE_OUT="$out" \
    KAI_LIVE_SHOT="$shot_cmd" KAI_LIVE_RING_OUTPUT="$active" KAI_LIVE_RING_PLACE="$place" \
    WAYLAND_DISPLAY="$wl" GDK_BACKEND=wayland "${nv[@]}" KAI_LOG="${KAI_LOG:-kai=debug}" \
    "$test_bin" --ignored --exact overlay::live_tests::live_ring_over_the_desktop --nocapture \
    --test-threads=1 > "$out/live.log" 2>&1 || status=$?
grep -E '^(ok  |FAIL|placement|listening|latency|while|PSS|CPU|kwin_wayland|GPU|ring_over|retry|quiet|turn|output|speaking|fed|mute|unmute|crash|second|session|push|cold|reduced|the UI|test |pw-private|LEFT)' \
    "$out/live.log" || true

echo "== the ring's output away and back, in a nested KWin (nothing on the session's screen)"
scripts/nested-kwin.sh "$out/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$out" \
    KAI_LIVE_RING_PLACE="$place" GDK_BACKEND=wayland "${nv[@]}" KAI_LOG="${KAI_LOG:-kai=debug}" \
    "$test_bin" --ignored --exact overlay::live_tests::output_removed::live_ring_output_removed \
    --nocapture > "$out/nested.log" 2>&1 || status=$?
grep -E '^(ok  |FAIL|==|Virtual|KWin|test |skipped|LEFT)' "$out/nested.log" || true
if grep -q '^skipped' "$out/nested.log"; then
    echo "FAIL the nested KWin's check skipped itself"
    status=1
fi
fi
rm -f "$shot" "$out/shot.lock"

echo "== the kai binary at idle"
idle=$out/kai-idle.sh
cat > "$idle" <<'IDLE'
#!/usr/bin/env bash
# Inside the private instance: start kai (an lmgw.url and no Secret Service: the paired menu with
# the wallet error), look at it after 3 s, toggle the tray's Overlays -> "Ring over fullscreen
# windows" through dbusmenu on the private bus, quit it with SIGTERM.
set -euo pipefail
kai=$1
mkdir -p "$XDG_CONFIG_HOME/kai"
cfg=$XDG_CONFIG_HOME/kai/config.toml
# No global shortcuts registered (scripts/shortcuts-guard.sh).
printf '[lmgw]\nurl = "http://127.0.0.1:1"\n[shortcuts]\nbind = false\n' > "$cfg"
KAI_LOG=info "$kai" > "$KAI_IDLE_LOG" 2>&1 &
pid=$!
echo "$pid" >> "$KAI_PW_PRIVATE_PIDS"
sleep 3
kids=$(ps -o comm= --ppid "$pid" | grep -c '^WebKit' || true)
pss=$(awk '/^Pss:/ {print int($2 / 1024)}' "/proc/$pid/smaps_rollup")
echo "kai at idle: $kids WebKit processes, PSS $pss MB"

item=$(busctl --user list --no-pager | awk '/StatusNotifierItem/ {print $1; exit}')
# toggle <labels…>: the toggle state of that menu path (1 on, 0 off), or its id with --id.
entry() { busctl --user --json=short -- call "$item" /MenuBar com.canonical.dbusmenu GetLayout iias 0 -1 0 \
    | python3 -c '
import json, sys
want, field = sys.argv[2:], sys.argv[1]
def walk(n, path):
    nid, props, kids = n
    label = props.get("label", {}).get("data")
    here = path + [label] if label else path
    if here == want:
        print(nid if field == "id" else props.get("toggle-state", {}).get("data")); sys.exit(0)
    for k in kids: walk(k["data"], here)
walk(json.load(sys.stdin)["data"][1], [])
sys.exit(1)' "$@"; }
tray=ok
[[ -n $item ]] || tray="no tray item on the private bus"
path=(Overlays "Ring over fullscreen windows")
if [[ $tray == ok ]]; then
    before=$(entry state "${path[@]}" || echo missing)
    busctl --user -- call "$item" /MenuBar com.canonical.dbusmenu Event isvu \
        "$(entry id "${path[@]}")" clicked s "" 0 > /dev/null
    written=""
    for _ in $(seq 50); do
        written=$(python3 -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb")).get("overlay", {}).get("ring_over_fullscreen", ""))' "$cfg")
        [[ $written == False ]] && break
        sleep 0.1
    done
    sleep 0.3
    after=$(entry state "${path[@]}" || echo missing)
    echo "tray: Overlays -> Ring over fullscreen windows: checked $before, clicked: overlay.ring_over_fullscreen = $written, checked $after"
    [[ $before == 1 && $written == False && $after == 0 ]] || tray="the check item did not toggle"
fi
kill -TERM "$pid"
code=0
wait "$pid" || code=$?
echo "kai at idle: SIGTERM -> exit $code"
grep -q "Kai has quit" "$KAI_IDLE_LOG" && echo "kai at idle: the quit sequence ran"
[[ $tray == ok ]] || echo "FAIL tray: $tray"
[[ $kids == 0 && $code == 0 && $tray == ok ]]
IDLE
chmod +x "$idle"
scripts/pw-private.sh "$out/run-idle" -- env KAI_IDLE_LOG="$out/kai-idle.log" WAYLAND_DISPLAY="$wl" \
    GDK_BACKEND=wayland "${nv[@]}" bash "$idle" "$here/target/debug/kai" || status=$?
rm -f "$idle"
echo "== WebKit processes in the whole session: $webkit_before before, $(webkit) after"
echo "== crops in $out/shots: $(ls "$out/shots" | tr '\n' ' ')"
exit $status
