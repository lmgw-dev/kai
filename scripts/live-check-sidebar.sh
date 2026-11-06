#!/usr/bin/env bash
# kai-firefox WP8b's real run of the sidebar (its design §8, §13): its page in a headless Firefox,
# through the real kai-browser-host and Kai's browser server and sidebar relay, over lmgw's device
# link with a device Kai pairs itself, to a dev lmgw on 127.0.0.1:8897 whose chat model is lmgw's
# echoing mock (scripts/mock-openai.py): the thread listed, a message sent and its reply streamed
# back, the site strip's rule from Kai, Attach this page (src/browser/tests/sidebar_live.rs; the
# page is driven by kai-firefox's tests/real-run.mjs).
# Usage: scripts/live-check-sidebar.sh [kai-firefox checkout] [lmgw checkout] [lmgw headless binary]
#   defaults: ../kai-firefox, ../lmgw, <lmgw checkout>/target/debug/examples/headless; the binary
#   must have lmgw's Chat-plane routes (lmgw 0.6.0 or later), and kai-firefox's sidebar
#   page must be built (its scripts/sidebar.sh).
#
# Isolation: the dev lmgw is a copy of the binary on its own data dir under target/live-sidebar and
# its own container prefix (lmgw-kaisidebar), no GPU; Firefox's profile, HOME and XDG folders are
# under the kai-firefox checkout's target/real-run/ and removed after the run; Kai's browser server
# is the test's, on a socket in target/live-sidebar. Nothing here talks to 127.0.0.1:8001 or the
# session's Kai. Verdicts: target/live-sidebar/report.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

FIREFOX=$(realpath "${1:-../kai-firefox}")
LMGW=$(realpath "${2:-../lmgw}")
HEADLESS=$(realpath "${3:-$LMGW/target/debug/examples/headless}")
ADDR=127.0.0.1:8897
OUT=$PWD/target/live-sidebar
DATA=$OUT/lmgw-data
PIDS=()

fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
    set +e
    for p in "${PIDS[@]}"; do kill -TERM -- "-$p" 2>/dev/null || kill -TERM "$p" 2>/dev/null; done
    for p in "${PIDS[@]}"; do wait "$p" 2>/dev/null; done
    if [[ -f $OUT/lmgw.pid ]]; then
        pid=$(cat "$OUT/lmgw.pid")
        if kill -0 "$pid" 2>/dev/null; then
            echo "the dev lmgw ($pid) still runs: stopping it"
            kill -INT "$pid"
            for _ in $(seq 1 75); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
            kill -KILL "$pid" 2>/dev/null
        fi
        rm -f "$OUT/lmgw.pid"
    fi
    left=$(podman ps -a --format '{{.Names}}' 2>/dev/null | grep '^lmgw-kaisidebar' || true)
    [[ -z $left ]] || echo "WARNING: dev containers left: $left"
    [[ -f $OUT/lmgw.log ]] && sed -i -E 's/lmgw-owner-[0-9a-f]+/lmgw-owner-<removed>/g; s/token=[^ ]+/token=<removed>/g' "$OUT/lmgw.log"
    rm -rf "$DATA" "$OUT/headless"
}
trap cleanup EXIT

[[ -x $HEADLESS ]] || fail "no dev lmgw build at $HEADLESS: cargo build -p lmgw-core --example headless in $LMGW"
[[ -f $LMGW/scripts/mock-openai.py ]] || fail "no $LMGW/scripts/mock-openai.py"
[[ -f $FIREFOX/extension/sidebar/sidebar.html ]] || fail "kai-firefox's sidebar page is not built: $FIREFOX/scripts/sidebar.sh"
for port in 8897 8898; do
    if ss -ltnH "sport = :$port" | grep -q .; then fail "something listens on $port already"; fi
done

rm -rf "$OUT"
mkdir -p "$OUT" "$DATA"
echo "lmgw: $HEADLESS (built $(date -r "$HEADLESS" '+%F %H:%M')); kai: $(git rev-parse --short HEAD); kai-firefox: $(git -C "$FIREFOX" rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)" \
    | tee "$OUT/versions.txt"

cp --reflink=auto "$HEADLESS" "$OUT/headless"
cat > "$OUT/lmgw-run.sh" <<EOF
exec env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG="${LANG:-C.UTF-8}" \\
    XDG_RUNTIME_DIR="/run/user/$(id -u)" LMGW_DATA_DIR="$DATA" LMGW_DEV=1 \\
    LMGW_CONTAINER_PREFIX=lmgw-kaisidebar "$OUT/headless" "$ADDR" >>"$OUT/lmgw.log" 2>&1
EOF
chmod +x "$OUT/lmgw-run.sh"

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
CHAT=$(free_port)
ECHO=1 STREAM_DELAY=0.05 setsid python3 "$LMGW/scripts/mock-openai.py" "$CHAT" >"$OUT/mock-chat.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$CHAT/v1/models" && break; sleep 0.1; done
curl -s -o /dev/null "http://127.0.0.1:$CHAT/v1/models" || fail "the mock on $CHAT did not start"

export CARGO_BUILD_JOBS=8
cargo build --workspace --bin kai-browser-host
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])' | tail -1)
[[ -x $test_bin ]] || fail "no test binary"

status=0
env KAI_LIVE_OUT="$OUT" KAI_FIREFOX="$FIREFOX" KAI_BROWSER_HOST="$PWD/target/debug/kai-browser-host" \
    KAI_LIVE_LMGW_URL="http://$ADDR" KAI_LIVE_LMGW_RUN="$OUT/lmgw-run.sh" KAI_LIVE_LMGW_LOG="$OUT/lmgw.log" \
    KAI_LIVE_LMGW_PID="$OUT/lmgw.pid" KAI_LIVE_MOCK_CHAT="$CHAT" KAI_LOG="${KAI_LOG:-info}" \
    "$test_bin" --ignored --exact browser::tests::sidebar_live::sidebar_live --nocapture --test-threads=1 \
    2>&1 | tee "$OUT/test.log" || status=$?
if [[ $status == 0 ]] && grep -q 'test result: ok. 1 passed' "$OUT/test.log"; then
    echo "== the sidebar against the dev lmgw passed; report: $OUT/report.txt"
else
    echo "== FAILED (see $OUT/test.log, $OUT/lmgw.log)"
    status=1
fi
exit $status
