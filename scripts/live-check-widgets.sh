#!/usr/bin/env bash
# WP10's live check of the tool-controlled overlays, entirely in a nested KWin with two virtual
# outputs (scripts/nested-kwin.sh: its own bus and XDG directories under target/, nothing on the
# session's screen, the session's compositor and outputs untouched) inside a private PipeWire
# instance (scripts/pw-private.sh: null sinks, no hardware, nothing audible; only
# PIPEWIRE_RUNTIME_DIR reaches the nested session, so nothing there finds the session's PipeWire):
# the sandbox (an HTML document that tries http:, file:, kai:, fetch, a WebSocket, preconnect,
# prefetch, preload, CSS and font loads, EventSource, sendBeacon, a meta refresh, a form, a popup
# and another widget's document against a loopback server that counts connections), no sound (the
# APIs, and no PipeWire client of Kai's processes, against a GStreamer control), no dialog, WebGL or
# fullscreen, PSS of the first and a second widget, every position on both outputs against KWin's
# geometry with crops of each kind, overlay_list, a fitted size, expiry, an output going away, the
# screen lock (section lock: every surface unmapped while locked, back in place after), no
# web process after the last widget, a script that never yields, a widget's crash beside the ring,
# and no widget content in a trace log (src/widgets/live_tests.rs); WP12's MCP Apps bridge (section
# apps: the postMessage path into Kai's isolated world, then overlay_app against a dev lmgw on
# 127.0.0.1:8897, the checkout's headless example on its own data dir, with
# scripts/mcp-apps-fixture.py registered under the prefix fx and a device key minted for the check;
# nothing talks to 127.0.0.1:8001); then a second run under
# WAYLAND_DEBUG=client whose log shows the widgets' input regions
# (scripts/wayland-input-regions.py).
#
# usage: scripts/live-check-widgets.sh [lmgw checkout]   (report, logs, crops and full-output
#        shots, shots/full-*.png, in target/live-widgets/; the checkout, default ../lmgw, needs its headless example built:
#        cargo build -p lmgw-core --example headless)
#        KAI_LIVE_SECTIONS=sandbox,sound,… runs only those sections of the first run
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-widgets
mkdir -p "$out"
rm -rf "$out/shots" "$out/report.txt" "$out/kai.log" "$out/input.log" "$out/wayland.log"
cd "$here"
export CARGO_BUILD_JOBS=8
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])' | tail -1)
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }

# ---- WP12's dev lmgw with the MCP Apps fixture (section apps)
LMGW=$(realpath "${1:-../lmgw}")
ADDR=127.0.0.1:8897
URL=http://$ADDR
LMGW_PID=""
apps_file=""
stop_lmgw() {
    [[ -n $LMGW_PID ]] || return 0
    kill -INT "$LMGW_PID" 2>/dev/null || true
    for _ in $(seq 1 75); do kill -0 "$LMGW_PID" 2>/dev/null || break; sleep 0.2; done
    kill -KILL "$LMGW_PID" 2>/dev/null || true
    wait "$LMGW_PID" 2>/dev/null || true
    LMGW_PID=""
}
cleanup_apps() {
    stop_lmgw
    # The dev lmgw's log carries its owner login link, its data dir and the key file the key.
    rm -rf "$out/lmgw-data" "$out/lmgw.log" "$out/headless" "$out/apps-lmgw"
}
trap cleanup_apps EXIT
sections=${KAI_LIVE_SECTIONS:-}
if [[ -z $sections || ,$sections, == *,apps,* ]]; then
    [[ -x $LMGW/target/debug/examples/headless ]] \
        || { echo "FAIL no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"; exit 1; }
    if ss -ltnH "sport = :8897" | grep -q .; then echo "FAIL something listens on 8897 already"; exit 1; fi
    scripts/lmgw-pin.sh check "$LMGW" || true
    cp --reflink=auto "$LMGW/target/debug/examples/headless" "$out/headless"
    mkdir -p "$out/lmgw-data"
    LMGW_DATA_DIR=$out/lmgw-data LMGW_DEV=1 LMGW_CONTAINER_PREFIX=lmgw-kaiapps \
        "$out/headless" "$ADDR" >>"$out/lmgw.log" 2>&1 &
    LMGW_PID=$!
    for _ in $(seq 1 300); do
        curl -s -o /dev/null --max-time 1 "$URL/v1/models" && break
        kill -0 "$LMGW_PID" 2>/dev/null || { echo "FAIL the dev lmgw exited (see $out/lmgw.log)"; exit 1; }
        sleep 0.2
    done
    token=$(grep -o 'dashboard login: [^ ]*token=[^ ]*' "$out/lmgw.log" | tail -1 | sed 's/.*token=//')
    [[ -n $token ]] || { echo "FAIL no owner login line in the dev lmgw's log"; exit 1; }
    # Requests with the bearer from a pipe, never in curl's argv.
    op() {
        printf 'Authorization: Bearer %s\n' "$token" | curl -sS --max-time 10 -X POST \
            "$URL/api/op/$1" -H @- -H 'Content-Type: application/json' -d "$2"
    }
    made=$(op mcp_server_set "{\"action\": \"create\", \"name\": \"kai-apps-fixture\", \"transport\": \"stdio\", \"command\": \"python3\", \"args\": \"-I $here/scripts/mcp-apps-fixture.py\", \"tool_prefix\": \"fx\"}")
    echo "$made" | jq -e '.id // .ok' >/dev/null 2>&1 || { echo "FAIL the fixture was not registered: $made"; exit 1; }
    created=$(op key_create "{\"kind\": \"device\", \"name\": \"kai-apps-live\", \"url\": \"$URL\"}")
    key=$(echo "$created" | jq -r .key)
    [[ -n $key && $key != null ]] || { echo "FAIL no device key: $(echo "$created" | jq -c 'del(.. | .key?, .link?)')"; exit 1; }
    apps_file=$out/apps-lmgw
    (umask 077; printf '%s\n%s\n' "$URL" "$key" >"$apps_file")
    echo "apps: dev lmgw on $URL ($(git -C "$LMGW" rev-parse --short HEAD)) with the fixture under fx"
fi

nv=()
if [[ -e /dev/nvidiactl || -e /sys/module/nvidia ]]; then
    nv=(__NV_DISABLE_EXPLICIT_SYNC="${__NV_DISABLE_EXPLICIT_SYNC:-1}")
fi
# Both runs in one nested session: the check, then the input run with libwayland's request log.
# shellcheck disable=SC2016
runs='status=0
"$1" --ignored --exact widgets::live_tests::live_widgets_nested --nocapture --test-threads=1 \
    > "$2/live.log" 2>&1 || status=$?
WAYLAND_DEBUG=client "$1" --ignored --exact widgets::live_tests::live_widgets_input_nested \
    --nocapture --test-threads=1 > "$2/input.log" 2> "$2/wayland.log" || status=1
exit $status'
status=0
scripts/pw-private.sh "$out/pw" -- \
    "$here/scripts/nested-kwin.sh" "$out/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$out" \
    PIPEWIRE_RUNTIME_DIR="$out/pw/rt" KAI_LIVE_SECTIONS="${KAI_LIVE_SECTIONS:-}" \
    KAI_LIVE_APPS="$apps_file" \
    GDK_BACKEND=wayland "${nv[@]}" KAI_LOG="${KAI_LOG:-trace}" \
    bash -c "$runs" kai-live "$test_bin" "$out" || status=$?
grep -E '^(ok  |FAIL|==|PSS|sandbox:|sound:|apps:|overlay_list|  [0-9]|crops|skipped|test |LEFT)' "$out/live.log" || true
grep -E '^(ok  |FAIL|==|INPUT|skipped|test )' "$out/input.log" || true
python3 -I scripts/wayland-input-regions.py "$out/wayland.log" "$out/input.log" | tee -a "$out/report.txt" \
    || status=1
if grep -q '^skipped' "$out/live.log" "$out/input.log"; then
    echo "FAIL the nested check skipped itself"
    status=1
fi
echo "== report: $out/report.txt; crops and full-output shots (full-*.png): $out/shots/"
exit $status
