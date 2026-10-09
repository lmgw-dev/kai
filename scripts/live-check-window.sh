#!/usr/bin/env bash
# WP21b's live check of the configuration window, entirely in a nested KWin (scripts/nested-kwin.sh:
# its own bus and XDG directories under target/, nothing on the session's screen, the session's
# compositor untouched): the real GTK window and WebKit view on the embedded page (ui/dist, built
# first by `just ui`), driven through the page's DOM against a stand-in core that runs the real
# operations on a temporary configuration file (src/settings/window/live_tests.rs): the policy table,
# the providers, an accepted and a refused setting, the zoom, KWin's frame, what the page cannot
# reach; spectacle's captures of the window land in target/live-window/shots/. The Personality
# page runs against a dev lmgw on 127.0.0.1:8897 (the checkout's headless example on its own data
# dir and container prefix; never the owner's lmgw): a device key minted there goes to the stand-in
# core in a file of mode 600, which forwards the page's profile-editor requests with it as Kai's core
# does; the page lists, creates, saves and deletes a profile. Its capture is copied to
# target/window-shots/.
#
# usage: scripts/live-check-window.sh [lmgw checkout]   (report and shots in target/live-window/;
#        the checkout, default ../lmgw, needs its headless example built:
#        cargo build -p lmgw-core --example headless)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-window
mkdir -p "$out"
rm -rf "$out/shots" "$out/report.txt" "$out/live.log"
cd "$here"
[[ -f ui/dist/index.html ]] || { echo "ui/dist is not built: just ui" >&2; exit 1; }
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6}
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])' | tail -1)
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }

# ---- the dev lmgw for the Personality page
LMGW=$(realpath "${1:-../lmgw}")
ADDR=127.0.0.1:8897
URL=http://$ADDR
LMGW_PID=""
lmgw_file=$out/window-lmgw
stop_lmgw() {
    [[ -n $LMGW_PID ]] || return 0
    kill -INT "$LMGW_PID" 2>/dev/null || true
    for _ in $(seq 1 75); do kill -0 "$LMGW_PID" 2>/dev/null || break; sleep 0.2; done
    kill -KILL "$LMGW_PID" 2>/dev/null || true
    wait "$LMGW_PID" 2>/dev/null || true
    LMGW_PID=""
}
cleanup() {
    stop_lmgw
    # The dev lmgw's log carries its owner login link, its data dir and the key file the key.
    rm -rf "$out/lmgw-data" "$out/lmgw.log" "$out/headless" "$lmgw_file"
}
trap cleanup EXIT
[[ -x $LMGW/target/debug/examples/headless ]] \
    || { echo "FAIL no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"; exit 1; }
if ss -ltnH "sport = :8897" | grep -q .; then echo "FAIL something listens on 8897 already"; exit 1; fi
scripts/lmgw-pin.sh check "$LMGW" || true
rm -rf "$out/lmgw-data"
cp --reflink=auto "$LMGW/target/debug/examples/headless" "$out/headless"
mkdir -p "$out/lmgw-data"
LMGW_DATA_DIR=$out/lmgw-data LMGW_DEV=1 LMGW_CONTAINER_PREFIX=lmgw-kaiwindow \
    "$out/headless" "$ADDR" >>"$out/lmgw.log" 2>&1 &
LMGW_PID=$!
for _ in $(seq 1 300); do
    curl -s -o /dev/null --max-time 1 "$URL/v1/models" && break
    kill -0 "$LMGW_PID" 2>/dev/null || { echo "FAIL the dev lmgw exited (see $out/lmgw.log)"; exit 1; }
    sleep 0.2
done
token=$(grep -o 'dashboard login: [^ ]*token=[^ ]*' "$out/lmgw.log" | tail -1 | sed 's/.*token=//')
[[ -n $token ]] || { echo "FAIL no owner login line in the dev lmgw's log"; exit 1; }
# The owner's bearer from a pipe, never in curl's argv.
created=$(printf 'Authorization: Bearer %s\n' "$token" | curl -sS --max-time 10 -X POST \
    "$URL/api/op/key_create" -H @- -H 'Content-Type: application/json' \
    -d "{\"kind\": \"device\", \"name\": \"kai-window-live\", \"url\": \"$URL\"}")
key=$(echo "$created" | jq -r .key)
[[ -n $key && $key != null ]] || { echo "FAIL no device key: $(echo "$created" | jq -c 'del(.. | .key?, .link?)')"; exit 1; }
(umask 077; printf '%s\n%s\n' "$URL" "$key" >"$lmgw_file")
unset key created token
echo "personality: dev lmgw on $URL ($(git -C "$LMGW" rev-parse --short HEAD))"

nv=()
if [[ -e /dev/nvidiactl || -e /sys/module/nvidia ]]; then
    nv=(__NV_DISABLE_EXPLICIT_SYNC="${__NV_DISABLE_EXPLICIT_SYNC:-1}")
fi
status=0
scripts/nested-kwin.sh "$out/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$out" \
    KAI_LIVE_LMGW="$lmgw_file" GDK_BACKEND=wayland KAI_WINDOW_CONSOLE=1 "${nv[@]}" \
    "$test_bin" --ignored --exact settings::window::live_tests::live_window_nested --nocapture \
    --test-threads=1 > "$out/live.log" 2>&1 || status=$?
grep -E '^(ok  |FAIL|test |LEFT)' "$out/live.log" || true
# A CSP violation or a page error on the console fails the run, whatever the checks said.
if grep -iE "Refused to|Content Security|CONSOLE.*(ERROR|Uncaught)" "$out/live.log"; then
    echo "FAIL the page's console reports a violation or an error"
    status=1
fi
if [[ -f $out/shots/personality.png ]]; then
    mkdir -p target/window-shots
    cp "$out/shots/"personality*.png target/window-shots/
    echo "== the Personality page at 125 %: target/window-shots/personality.png (and -foot)"
fi
echo "== report: $out/report.txt; shots: $out/shots/"
exit $status
