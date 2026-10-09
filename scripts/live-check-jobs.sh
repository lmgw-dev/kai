#!/usr/bin/env bash
# WP18's live check (ruling 22, K26): Kai's asynchronous jobs end to end against a dev lmgw on
# 127.0.0.1:8897 (lmgw's M4: MCP Tasks on the host link, late results in the thread, the bound
# session's lmgw.task.done and its continuation) whose chat is lmgw's scripts/mock-openai.py with
# TOOL_SCRIPT (each turn calls kailive__live_slow, the live-test build's K26 open-ended tool, then
# echoes the tool's result) and whose speech-to-text and text-to-speech are
# scripts/mock-voice-audio.py; Kai's audio in a private PipeWire instance (scripts/pw-private.sh:
# nothing audible). The in-process check (src/voice/jobs_live_tests.rs) drives: a bound session
# speaking a job's result after jobs.quiet_ms with one bare response.create; with no session Kai
# binding its conversation itself (takeover=never) to speak one; an owner's job notified, not
# spoken; the owner's cancel in lmgw reaching Kai as tasks/cancel; a second paired device's job ended by
# the feed's device.revoked when its key is deleted. Report: target/live-jobs/ (about
# a minute and a half).
#
# Usage: scripts/live-check-jobs.sh [clip.wav] [lmgw checkout]
#   clip           a 24 kHz mono PCM16 clip (default target/live-audio/clip.wav; never committed)
#   lmgw checkout  default ../lmgw, read only; its headless example built there beforehand
#                  (`cargo build -p lmgw-core --example headless`) and copied
#
# The dev lmgw runs on its own data dir under target/ and the container prefix lmgw-kaijobs; the
# test starts and stops it through the launcher written here. The device key lives in the test's
# memory. Nothing talks to 127.0.0.1:8001. Everything started here is stopped here; the dev lmgw's
# data dir and its owner login are removed at the end.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh

CLIP=$(realpath "${1:-target/live-audio/clip.wav}")
LMGW=$(realpath "${2:-../lmgw}")
ADDR=127.0.0.1:8897
OUT=$PWD/target/live-jobs
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
    left=$(podman ps -a --format '{{.Names}}' 2>/dev/null | grep '^lmgw-kaijobs' || true)
    [[ -z $left ]] || echo "WARNING: dev containers left: $left"
    [[ -f $OUT/lmgw.log ]] && sed -i -E 's/lmgw-owner-[0-9a-f]+/lmgw-owner-<removed>/g; s/token=[^ ]+/token=<removed>/g' "$OUT/lmgw.log"
    rm -rf "$DATA" "$OUT/headless"
}
trap 'cleanup; kgs_check || exit 1' EXIT

[[ -f $CLIP ]] || fail "no clip at $CLIP"
[[ -x $LMGW/target/debug/examples/headless ]] \
    || fail "no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"
grep -q TOOL_SCRIPT "$LMGW/scripts/mock-openai.py" || fail "$LMGW's mock-openai.py has no TOOL_SCRIPT (M7)"
git -C "$LMGW" grep -q 'lmgw.task.done' -- crates/lmgw-core/src || fail "$LMGW has no bound task results (M4 WP3)"
if ss -ltnH "sport = :8897" | grep -q .; then fail "something listens on 8897 already"; fi

rm -rf "$OUT"
mkdir -p "$OUT" "$DATA"
echo "lmgw: $(git -C "$LMGW" rev-parse --short HEAD) ($(git -C "$LMGW" status --short | wc -l) changed files, headless built $(date -r "$LMGW/target/debug/examples/headless" +%H:%M)); kai: $(git rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)" \
    | tee "$OUT/versions.txt"
scripts/lmgw-pin.sh check "$LMGW" 2>&1 | tee -a "$OUT/versions.txt"

cp --reflink=auto "$LMGW/target/debug/examples/headless" "$OUT/headless"
cat > "$OUT/lmgw-run.sh" <<RUN
#!/usr/bin/env bash
# Starts the dev lmgw of scripts/live-check-jobs.sh (the test runs this; written per run).
exec env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG="${LANG:-C.UTF-8}" \\
    XDG_RUNTIME_DIR="/run/user/$(id -u)" LMGW_DATA_DIR="$DATA" LMGW_DEV=1 \\
    LMGW_CONTAINER_PREFIX=lmgw-kaijobs "$OUT/headless" "$ADDR" >>"$OUT/lmgw.log" 2>&1
RUN
chmod +x "$OUT/lmgw-run.sh"

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
CHAT=$(free_port)
AUDIO=$(free_port)
# One call per turn, in the check's order: 5 s (bound), 40 s (past voice.grace_s), 2 s (the
# owner's thread), 600 s (cancelled), 600 s (the phone's, its key deleted). The texts are marked so the checks find them, and the log
# check finds none of them in Kai's log.
marks=()
script='['
n=0
for secs in 5 40 2 600 600; do
    n=$((n + 1))
    m="j$n-mark-$RANDOM$RANDOM"
    marks+=("$m")
    script+="[{\"name\": \"kailive__live_slow\", \"arguments\": {\"text\": \"$m\", \"seconds\": $secs}}]"
    [[ $n == 5 ]] || script+=', '
done
script+=']'
TOOL_SCRIPT=$script STREAM_DELAY=0 setsid python3 "$LMGW/scripts/mock-openai.py" "$CHAT" \
    >"$OUT/mock-chat.log" 2>&1 &
PIDS+=($!)
setsid python3 scripts/mock-voice-audio.py "$AUDIO" "$CLIP" >"$OUT/mock-audio.log" 2>&1 &
PIDS+=($!)
for port in "$CHAT" "$AUDIO"; do
    for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$port/v1/models" && break; sleep 0.1; done
    curl -s -o /dev/null "http://127.0.0.1:$port/v1/models" || fail "the mock on $port did not start"
done

export CARGO_BUILD_JOBS=8
test_bin=$(cargo test --features live-test --no-run --message-format=json 2>/dev/null | python3 -I -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])' | tail -1)
[[ -x $test_bin ]] || fail "no test binary"

status=0
marks_csv=$(IFS=,; echo "${marks[*]}")
scripts/pw-private.sh "$OUT/pw" -- env KAI_LIVE_PRIVATE=1 KAI_LIVE_CLIP="$CLIP" KAI_LIVE_OUT="$OUT" \
    KAI_LIVE_LMGW_URL="http://$ADDR" KAI_LIVE_LMGW_RUN="$OUT/lmgw-run.sh" KAI_LIVE_LMGW_LOG="$OUT/lmgw.log" \
    KAI_LIVE_LMGW_PID="$OUT/lmgw.pid" KAI_LIVE_MOCK_CHAT="$CHAT" KAI_LIVE_MOCK_AUDIO="$AUDIO" \
    KAI_LIVE_JOB_MARKS="$marks_csv" KAI_LOG="${KAI_LOG:-trace}" \
    "$test_bin" --ignored --exact voice::jobs_live_tests::live_jobs --nocapture --test-threads=1 \
    >"$OUT/test.log" 2>&1 || status=$?
grep -E '^([1-6] |dev lmgw|the |FAIL|test result|thread .* panicked)' "$OUT/test.log" || true
if [[ $status == 0 ]] && grep -q 'test result: ok. 1 passed' "$OUT/test.log"; then
    echo "== the jobs against the dev lmgw passed; report: $OUT/report.txt"
else
    echo "== FAILED (see $OUT/test.log, $OUT/kai.log, $OUT/lmgw.log)"
    status=1
fi
exit $status
