#!/usr/bin/env bash
# WP4's real-lmgw half (§15 WP4): Kai's voice session against a dev lmgw on 127.0.0.1:8897 whose
# chat, speech-to-text and text-to-speech are mocks (no model, no GPU), in a private PipeWire
# instance (nothing audible, the session's devices untouched). The in-process check
# (src/voice/lmgw_live_tests.rs) runs Kai's core with the real feed and realtime adapter and checks:
# push-to-talk round trips (a cold bind from a press, and push-to-talk mode), an automatic round
# trip, a barge-in leaving the heard text in the thread, a dashboard text turn silencing automatic
# listening until it is done, a dashboard bind taking over (named), and a dev lmgw restart turning
# listening off with a notification. Report: target/live-voice-lmgw/report.txt.
#
# Usage: scripts/live-check-voice-lmgw.sh [clip.wav] [lmgw checkout]
#   clip           a 24 kHz mono PCM16 TTS clip (default target/live-audio/clip.wav; never committed):
#                  the speech played into the test microphone, and every reply clause's audio
#   lmgw checkout  default ../lmgw (lmgw main), read only; its headless example is built
#                  there beforehand (`cargo build -p lmgw-core --example headless`) and copied
#
# The mocks: lmgw's own scripts/mock-openai.py (ECHO=1: the reply repeats the user's words) for the
# chat, and scripts/mock-voice-audio.py for ASR and TTS (lmgw has no standalone ASR mock). The dev
# lmgw runs on its own data dir under target/ and the container prefix lmgw-kaivoice; the test starts
# and stops it through the launcher written here. The device key lives in the test's memory (no
# wallet item). Nothing talks to 127.0.0.1:8001. Everything started here is stopped here; the
# dev lmgw's data dir and log (it carries the owner login) are deleted at the end.
set -euo pipefail
cd "$(dirname "$0")/.."
# The Kai this check runs is the in-process core (no shortcuts task); its configuration sets
# shortcuts.bind = false all the same, and the owner's kglobalshortcutsrc is checked.
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh

CLIP=$(realpath "${1:-target/live-audio/clip.wav}")
LMGW=$(realpath "${2:-../lmgw}")
ADDR=127.0.0.1:8897
OUT=$PWD/target/live-voice-lmgw
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
    left=$(podman ps -a --format '{{.Names}}' 2>/dev/null | grep '^lmgw-kaivoice' || true)
    [[ -z $left ]] || echo "WARNING: dev containers left: $left"
    # The owner login line goes; the rest of the log stays for a look.
    [[ -f $OUT/lmgw.log ]] && sed -i -E 's/lmgw-owner-[0-9a-f]+/lmgw-owner-<removed>/g; s/token=[^ ]+/token=<removed>/g' "$OUT/lmgw.log"
    rm -rf "$DATA" "$OUT/headless"
}
trap 'cleanup; kgs_check || exit 1' EXIT

# ---------------------------------------------------------------- preconditions
[[ -f $CLIP ]] || fail "no clip at $CLIP"
[[ -x $LMGW/target/debug/examples/headless ]] \
    || fail "no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"
[[ -f $LMGW/scripts/mock-openai.py ]] || fail "no $LMGW/scripts/mock-openai.py"
# 8898: the dev lmgw's device listener, which Kai pairs over (loopback, the HTTP port plus one).
for port in 8897 8898; do
    if ss -ltnH "sport = :$port" | grep -q .; then fail "something listens on $port already"; fi
done

rm -rf "$OUT"
mkdir -p "$OUT" "$DATA"
echo "lmgw: $(git -C "$LMGW" rev-parse --short HEAD) ($(git -C "$LMGW" status --short | wc -l) changed files, headless built $(date -r "$LMGW/target/debug/examples/headless" +%H:%M)); kai: $(git rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)" \
    | tee "$OUT/versions.txt"
# A checkout away from the pin is said, not refused (review KAI-3 K3-5).
scripts/lmgw-pin.sh check "$LMGW" 2>&1 | tee -a "$OUT/versions.txt"

# The dev lmgw this run uses, fixed for its length (a rebuild in the checkout cannot change it).
cp --reflink=auto "$LMGW/target/debug/examples/headless" "$OUT/headless"
cat > "$OUT/lmgw-run.sh" <<EOF
#!/usr/bin/env bash
# Starts the dev lmgw of scripts/live-check-voice-lmgw.sh (the test runs this; written per run).
exec env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG="${LANG:-C.UTF-8}" \\
    XDG_RUNTIME_DIR="/run/user/$(id -u)" LMGW_DATA_DIR="$DATA" LMGW_DEV=1 \\
    LMGW_CONTAINER_PREFIX=lmgw-kaivoice "$OUT/headless" "$ADDR" >>"$OUT/lmgw.log" 2>&1
EOF
chmod +x "$OUT/lmgw-run.sh"

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
CHAT=$(free_port)
AUDIO=$(free_port)
ECHO=1 STREAM_DELAY=0.05 setsid python3 "$LMGW/scripts/mock-openai.py" "$CHAT" >"$OUT/mock-chat.log" 2>&1 &
PIDS+=($!)
setsid python3 scripts/mock-voice-audio.py "$AUDIO" "$CLIP" >"$OUT/mock-audio.log" 2>&1 &
PIDS+=($!)
for port in "$CHAT" "$AUDIO"; do
    for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$port/v1/models" && break; sleep 0.1; done
    curl -s -o /dev/null "http://127.0.0.1:$port/v1/models" || fail "the mock on $port did not start"
done

export CARGO_BUILD_JOBS=8
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])' | tail -1)
[[ -x $test_bin ]] || fail "no test binary"

status=0
scripts/pw-private.sh "$OUT/pw" -- env KAI_LIVE_PRIVATE=1 KAI_LIVE_CLIP="$CLIP" KAI_LIVE_OUT="$OUT" \
    KAI_LIVE_LMGW_URL="http://$ADDR" KAI_LIVE_LMGW_RUN="$OUT/lmgw-run.sh" KAI_LIVE_LMGW_LOG="$OUT/lmgw.log" \
    KAI_LIVE_LMGW_PID="$OUT/lmgw.pid" KAI_LIVE_MOCK_CHAT="$CHAT" KAI_LIVE_MOCK_AUDIO="$AUDIO" \
    KAI_LOG="${KAI_LOG:-info}" \
    "$test_bin" --ignored --exact voice::lmgw_live_tests::lmgw_voice_live --nocapture --test-threads=1 \
    2>&1 | tee "$OUT/test.log" || status=$?
if [[ $status == 0 ]] && grep -q 'test result: ok. 1 passed' "$OUT/test.log"; then
    echo "== the voice session against the dev lmgw passed; report: $OUT/report.txt"
else
    echo "== FAILED (see $OUT/test.log, $OUT/kai.log, $OUT/lmgw.log)"
    status=1
fi
exit $status
