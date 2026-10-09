#!/usr/bin/env bash
# The live check of Kai's audio (WP3), all of it in a private PipeWire instance
# (scripts/pw-private.sh): nothing is audible and the session's defaults are never touched.
#
# usage: scripts/live-check-audio.sh <clip.wav>   (a 24 kHz mono PCM16 TTS clip; copied to target/)
#        KAI_LIVE_TRACE=1 traces the private part; KAI_LIVE_ONLY_BINARY=1 skips the in-process tests;
#        KAI_LIVE_ONLY=<filter> runs only the in-process tests matching <filter> (live_voice: the
#        voice session's check, `just live-check-voice`).
#
# In process (src/audio/live_tests.rs): the heard position against a sink's own timeline and
# PipeWire's reported delay, a barge-in flush, devices, capture and levels, the device warning, app
# AEC flip and restore. With the real binary: app AEC restored after `kill -9` of Kai (on the next
# start), restored on a clean exit, a foreign stream following the flipped default, and the
# fallback to half duplex without the WebRTC plugin. Reports go to target/live-audio/.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-audio
run=$out/run
# shellcheck source=scripts/shortcuts-guard.sh
source "$here/scripts/shortcuts-guard.sh"

if [[ "${1:-}" != "--inside" ]]; then
    clip_src=${1:?usage: $0 <clip.wav>}
    mkdir -p "$out"
    [[ "$(realpath "$clip_src")" == "$(realpath -m "$out/clip.wav")" ]] || cp "$clip_src" "$out/clip.wav"

    node_names() { timeout 5 pw-dump 2>/dev/null | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    if o.get("type") == "PipeWire:Interface:Node":
        p = o["info"]["props"]; print(p.get("node.name"), p.get("media.class"))
' | sort; }
    defaults() { timeout 5 pw-metadata -n default 0 2>/dev/null | grep -o "key:'default[^']*' value:'[^']*'" || true; }
    node_names > "$out/session-nodes-before.txt"
    defaults > "$out/session-defaults-before.txt"
    echo "== the session: $(wc -l < "$out/session-nodes-before.txt") PipeWire nodes before"

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

    status=0
    scripts/pw-private.sh "$run" -- env KAI_TEST_BIN="$test_bin" KAI_HERE="$here" \
        KAI_LIVE_ONLY_BINARY="${KAI_LIVE_ONLY_BINARY:-}" KAI_LIVE_ONLY="${KAI_LIVE_ONLY:-}" \
        bash ${KAI_LIVE_TRACE:+-x} "$0" --inside || status=$?

    node_names > "$out/session-nodes-after.txt"
    defaults > "$out/session-defaults-after.txt"
    if diff -u "$out/session-nodes-before.txt" "$out/session-nodes-after.txt" \
        && diff -u "$out/session-defaults-before.txt" "$out/session-defaults-after.txt"; then
        echo "== the session's PipeWire nodes and default devices are unchanged"
    else
        echo "== WARNING: the session's nodes or defaults differ (other programs may have come or gone)"
    fi
    kgs_check || status=1
    exit $status
fi

# ---------------------------------------------------------------- inside the private instance
here=$KAI_HERE
out=$here/target/live-audio
run=$out/run
pids=()
track() { pids+=("$1"); echo "$1" >> "$KAI_PW_PRIVATE_PIDS"; }
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
    for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
}
trap cleanup EXIT

# Waits up to $1 s for a command to succeed (foreground, bounded).
until_ok() {
    local secs=$1; shift
    local end=$((SECONDS + secs))
    until "$@" >/dev/null 2>&1; do
        (( SECONDS < end )) || return 1
        sleep 0.1
    done
}
meta() { { timeout 5 pw-metadata -n default 0 "$1" 2>/dev/null | grep -o "value:'[^']*'" | head -1 | sed "s/value:'\(.*\)'/\1/"; } || true; }
# The node name in a default's value (`{"name": "…"}`), or nothing when the key is absent.
name_of() { python3 -c 'import json, sys
s = sys.stdin.read().strip()
print(json.loads(s)["name"] if s else "")'; }
configured() { meta "default.configured.audio.$1" | name_of; }
effective() { meta "default.audio.$1" | name_of; }
node_id() { timeout 5 pw-dump | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    if o.get("type") == "PipeWire:Interface:Node" and o["info"]["props"].get("node.name") == sys.argv[1]:
        print(o["id"])' "$1"; }

echo "== private defaults: sink $(effective sink), source $(effective source)"

echo "== in-process live tests"
run_tests() {
    env KAI_LIVE_PRIVATE=1 KAI_LIVE_CLIP="$out/clip.wav" KAI_LIVE_OUT="$out" "$@" \
        "$KAI_TEST_BIN" --ignored --test-threads=1 --nocapture "${LIVE_FILTER:-live_}" 2>&1 \
        | grep -v '^  event:' | tee -a "$out/live-tests.log"
    return "${PIPESTATUS[0]}"
}
if [[ -n "${KAI_LIVE_ONLY:-}" ]]; then
    : > "$out/live-tests.log"
    LIVE_FILTER=$KAI_LIVE_ONLY run_tests
    echo "== the in-process live tests matching '$KAI_LIVE_ONLY' passed"
    exit 0
fi
if [[ -z "${KAI_LIVE_ONLY_BINARY:-}" ]]; then
    : > "$out/live-tests.log"
    LIVE_FILTER=live_heard run_tests KAI_LIVE_SINK=kai-test-sink-follower
    LIVE_FILTER=live_heard run_tests KAI_LIVE_SINK=kai-test-sink
    LIVE_FILTER=live_barge run_tests
    LIVE_FILTER=live_devices run_tests
    LIVE_FILTER=live_app_aec run_tests
fi

# ---------------------------------------------------------------- the real binary
kai=$here/target/debug/kai
mkdir -p "$XDG_CONFIG_HOME/kai"
# GTK needs a display; broadwayd gives it one that shows nothing anywhere (no TCP, no compositor).
broadwayd -u "$XDG_RUNTIME_DIR/broadway-http.sock" :7 > "$out/broadwayd.log" 2>&1 &
broadway=$!
track $broadway
# Its display socket is abstract (no file): the log line says it is up.
until_ok 5 grep -q "Listening on" "$out/broadwayd.log" && kill -0 $broadway \
    || { echo "broadwayd did not start"; cat "$out/broadwayd.log"; exit 1; }
start_kai() { # $1: log name; sets KAI
    GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 KAI_LOG=info "$kai" > "$out/$1.log" 2>&1 &
    KAI=$!
    track $KAI
}
aec_file=$XDG_STATE_HOME/kai/aec.json

# The configured defaults before: the sink set explicitly, the source key absent.
timeout 5 wpctl set-default "$(node_id kai-test-sink)"
until_ok 5 bash -c "timeout 5 pw-metadata -n default 0 default.configured.audio.sink | grep -q kai-test-sink"
sink_before=$(configured sink); source_before=$(configured source)
echo "== configured before: sink '$sink_before', source '${source_before:-<absent>}'"

# A foreign stream that follows the default (silence into a null sink: nothing audible).
python3 -c '
import sys, wave
w = wave.open(sys.argv[1], "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(48000)
w.writeframes(b"\0\0" * 48000 * 60); w.close()' "$out/silence.wav"
pw-play --media-role=Music "$out/silence.wav" > /dev/null 2>&1 &
track $!
play_pid=$!
# The node the foreign stream (pw-play) is linked to.
foreign_target() { timeout 5 pw-link -l | python3 -c '
import sys
cur = None
for line in sys.stdin:
    if not line.startswith(" "):
        cur = line.strip()
    elif cur and cur.startswith("pw-play") and "|->" in line:
        print(line.split("|->")[1].strip().split(":")[0]); break'; }
export -f foreign_target
until_ok 5 bash -c '[[ -n "$(foreign_target)" ]]'
echo "the foreign stream plays into: $(foreign_target)"

echo "== kill -9 of Kai with app AEC on"
kai_config "$XDG_CONFIG_HOME/kai/config.toml" '[audio]\necho_mode = "app"\n'
start_kai kai-app-1
until_ok 15 bash -c "timeout 5 pw-metadata -n default 0 default.configured.audio.sink | grep -q kai_aec_sink" \
    || { echo "app AEC did not flip"; tail -20 "$out/kai-app-1.log"; exit 1; }
until_ok 5 bash -c "timeout 5 pw-metadata -n default 0 default.audio.sink | grep -q kai_aec_sink"
child=$(pgrep -P "$KAI" -x pipewire || true)
until_ok 5 bash -c '[[ "$(foreign_target)" == kai_aec_sink ]]' || true
echo "flipped: configured sink '$(configured sink)', source '$(configured source)'; aec.json $([[ -f $aec_file ]] && echo present || echo MISSING); echo-cancel child pid ${child:-NONE}; the foreign stream now plays into $(foreign_target)"
[[ -f $aec_file && -n $child ]]
cp "$aec_file" "$out/aec-while-active.json"
t0=$(date +%s%N)
kill -9 "$KAI"
wait "$KAI" 2>/dev/null || true
until_ok 3 bash -c "! kill -0 $child 2>/dev/null" || { echo "the echo-cancel child outlived kill -9"; exit 1; }
t1=$(date +%s%N)
until_ok 3 bash -c '! timeout 5 pw-dump | grep -q "\"node.name\": \"kai_aec_sink\""'
t2=$(date +%s%N)
echo "after kill -9: the child was gone after $(( (t1 - t0) / 1000000 )) ms, its nodes after $(( (t2 - t0) / 1000000 )) ms; configured sink '$(configured sink)', effective '$(effective sink)'; aec.json $([[ -f $aec_file ]] && echo kept || echo GONE); the foreign stream plays into $(foreign_target)"
[[ $(configured sink) == kai_aec_sink && -f $aec_file ]]

echo "== the next start (device mode) restores"
kai_config "$XDG_CONFIG_HOME/kai/config.toml" '[audio]\necho_mode = "device"\n'
start_kai kai-restore
until_ok 10 bash -c "[[ ! -f '$aec_file' ]]" || { echo "aec.json was not removed"; tail -20 "$out/kai-restore.log"; exit 1; }
echo "restored: configured sink '$(configured sink)', source '$(configured source)' (before: '$sink_before', '${source_before}'); the foreign stream plays into $(foreign_target)"
[[ "$(configured sink)" == "$sink_before" && "$(configured source)" == "$source_before" ]]
grep -o "restored the default devices[^\"]*" "$out/kai-restore.log" | head -1
kill -TERM "$KAI"; code=0; wait "$KAI" || code=$?; echo "Kai exited with $code"

echo "== a clean exit restores"
kai_config "$XDG_CONFIG_HOME/kai/config.toml" '[audio]\necho_mode = "app"\n'
start_kai kai-app-2
until_ok 15 bash -c "timeout 5 pw-metadata -n default 0 default.configured.audio.sink | grep -q kai_aec_sink"
kill -TERM "$KAI"; code=0; wait "$KAI" || code=$?
echo "Kai exited with $code; configured sink '$(configured sink)', source '$(configured source)'; aec.json $([[ -f $aec_file ]] && echo LEFT || echo gone)"
[[ $code == 0 && "$(configured sink)" == "$sink_before" && "$(configured source)" == "$source_before" && ! -f $aec_file ]]
until_ok 3 bash -c '! timeout 5 pw-dump | grep -q "\"node.name\": \"kai_aec_sink\""'

echo "== no WebRTC plugin: half duplex with the reason"
# PipeWire's plugin directory with every plugin but the WebRTC canceller (links, nothing copied).
nospa=$XDG_RUNTIME_DIR/spa-without-webrtc
mkdir -p "$nospa/aec"
for d in /usr/lib64/spa-0.2/*; do [[ $(basename "$d") == aec ]] || ln -s "$d" "$nospa/"; done
for f in /usr/lib64/spa-0.2/aec/*; do [[ $(basename "$f") == libspa-aec-webrtc* ]] || ln -s "$f" "$nospa/aec/"; done
SPA_PLUGIN_DIR="$nospa" start_kai kai-no-plugin
until_ok 10 grep -q "half duplex instead" "$out/kai-no-plugin.log" || { echo "no fallback"; tail "$out/kai-no-plugin.log"; exit 1; }
grep -o "echo: Kai's echo cancellation failed: [^\"]*" "$out/kai-no-plugin.log" | head -1
[[ "$(configured sink)" == "$sink_before" && ! -f $aec_file ]]
kill -TERM "$KAI"; wait "$KAI" || true

echo "== the tray, through its dbusmenu on the private bus (no input reaches the session)"
# With an lmgw.url and no Secret Service, Kai shows the wallet error, a paired state: its menu has
# the audio entries.
kai_config "$XDG_CONFIG_HOME/kai/config.toml" '[lmgw]\nurl = "http://127.0.0.1:1"\n'
start_kai kai-tray
until_ok 10 bash -c 'busctl --user list --no-pager | grep -q StatusNotifierItem' || { echo "no tray item"; exit 1; }
item=$(busctl --user list --no-pager | awk '/StatusNotifierItem/ {print $1; exit}')
# menu [label…]: prints the layout; with labels, the id of that path ("Speaker" "Kai test sink 2").
menu() { busctl --user --json=short -- call "$item" /MenuBar com.canonical.dbusmenu GetLayout iias 0 -1 0 \
    | python3 -c '
import json, sys
want = sys.argv[1:]
root = json.load(sys.stdin)["data"][1]
def label(n): return n[1].get("label", {}).get("data") or n[1].get("type", {}).get("data")
def walk(n, path, depth):
    nid, props, kids = n
    l = label(n)
    if not want and l:
        t = props.get("toggle-type", {}).get("data")
        on = props.get("toggle-state", {}).get("data") == 1
        print("  " * depth + l + (" (" + t + (", on" if on else "") + ")" if t else ""))
    if want and path + [l] == want:
        print(nid); sys.exit(0)
    for k in kids: walk(k["data"], path + [l] if l else path, depth + (1 if l else 0))
walk(root, [], 0)
if want: sys.exit(1)' "$@"; }
click() { busctl --user -- call "$item" /MenuBar com.canonical.dbusmenu Event isvu "$(menu "$@")" clicked s "" 0 >/dev/null; }
tooltip() { busctl --user --json=short get-property "$item" /StatusNotifierItem org.kde.StatusNotifierItem ToolTip \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["data"][3].replace("<br/>", " | "))'; }
cfg_value() { python3 -c 'import sys, tomllib; c = tomllib.load(open(sys.argv[1], "rb")); print(c.get("audio", {}).get(sys.argv[2], ""))' \
    "$XDG_CONFIG_HOME/kai/config.toml" "$1"; }
menu | tee "$out/tray-menu.txt"
menu "Echo mode" "Kai cancels echo" > /dev/null && menu "Speaker" "Kai test sink 2" > /dev/null
# A device that comes and goes.
timeout 5 pw-cli create-node adapter '{ factory.name = support.null-audio-sink node.name = kai-test-hotplug
    node.description = "Kai hotplug sink" media.class = Audio/Sink audio.position = [ MONO ] object.linger = true }' > /dev/null
until_ok 5 bash -c "$(declare -f menu); item=$item; menu Speaker 'Kai hotplug sink'" && echo "a new sink appeared in the Speaker menu"
timeout 5 pw-cli destroy "$(node_id kai-test-hotplug)" > /dev/null
until_ok 5 bash -c "$(declare -f menu); item=$item; ! menu Speaker 'Kai hotplug sink'" && echo "and left it when it went away"
# Choices through the menu: a speaker that is not the default warns and offers half duplex.
click "Speaker" "Kai test sink 2"
until_ok 5 bash -c "[[ \$($(declare -f cfg_value); cfg_value output) == kai-test-sink-2 ]]" && echo "audio.output = kai-test-sink-2 written"
until_ok 5 bash -c "$(declare -f tooltip); item=$item; tooltip | grep -q \"'Kai test sink 2', not the default output\""
echo "tooltip: $(tooltip)"
click "Use half duplex (Kai's speaker is not the default output)"
until_ok 5 bash -c "[[ \$($(declare -f cfg_value); cfg_value echo_mode) == half_duplex ]]" && echo "Use half duplex: audio.echo_mode = half_duplex written"
click "Speaker" "Default (Kai test sink)"
until_ok 5 bash -c "[[ -z \$($(declare -f cfg_value); cfg_value output) ]]" && echo "Speaker Default: audio.output removed"
menu > "$out/tray-menu-after.txt"
kill -TERM "$KAI"; wait "$KAI" || true

kill "$play_pid" 2>/dev/null || true
echo "== all live checks passed"
