#!/usr/bin/env bash
# WP19's live check (§11.1, K17, PD4's hand-over): Kai's core with its hosted server and the
# approval card, against a dev lmgw on 127.0.0.1:8897 whose chat is lmgw's scripts/mock-openai.py
# with TOOL_SCRIPT (each turn calls kailive__live_note, the live-test build's state-changing tool)
# and whose speech-to-text answers "Ja." (scripts/mock-voice-audio.py), entirely in a nested KWin
# (scripts/nested-kwin.sh: its own bus and XDG directories, nothing on the session's screen) inside
# a private PipeWire instance (scripts/pw-private.sh: nothing audible). The in-process check
# (src/voice/approval_live_tests.rs) drives: the card from a feed call (an Enter before it armed
# counts nothing, nor does a plain Enter after it then; Ctrl+Enter approves through the route, no
# dialog, the tool runs), Esc declining, a device's self-approval meeting Kai's dialog, the bound
# session's mcp_approval_request answered by voice (a press before the card armed is talk; no
# audio to the session while it waits), and another thread's call (its card holds the keyboard, so
# Kai's key_press refuses; Kai's push-to-talk turn streams and commits meanwhile), and the screen
# lock (a stand-in screen saver service on the nested session's bus and Kai's own lock watch: the
# card goes and comes back, an approval given in lmgw while locked meets Kai's refusal). Report and
# the card's crop: target/live-approvals/.
#
# Usage: scripts/live-check-approvals.sh [clip.wav] [lmgw checkout]
#   clip           a 24 kHz mono PCM16 clip (default target/live-audio/clip.wav; never committed)
#   lmgw checkout  default ../lmgw, read only; its headless example built there beforehand
#                  (`cargo build -p lmgw-core --example headless`) and copied
#
# The dev lmgw runs on its own data dir under target/ and the container prefix lmgw-kaiappr; the
# test starts and stops it through the launcher written here. Its device keys live in the test's
# memory. Nothing talks to 127.0.0.1:8001. Everything started here is stopped here; the dev lmgw's
# data dir and its owner login are removed at the end.
set -euo pipefail
cd "$(dirname "$0")/.."
here=$PWD
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh

CLIP=$(realpath "${1:-target/live-audio/clip.wav}")
LMGW=$(realpath "${2:-../lmgw}")
ADDR=127.0.0.1:8897
OUT=$PWD/target/live-approvals
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
    left=$(podman ps -a --format '{{.Names}}' 2>/dev/null | grep '^lmgw-kaiappr' || true)
    [[ -z $left ]] || echo "WARNING: dev containers left: $left"
    [[ -f $OUT/lmgw.log ]] && sed -i -E 's/lmgw-owner-[0-9a-f]+/lmgw-owner-<removed>/g; s/token=[^ ]+/token=<removed>/g' "$OUT/lmgw.log"
    rm -rf "$DATA" "$OUT/headless"
}
trap 'cleanup; kgs_check || exit 1' EXIT

[[ -f $CLIP ]] || fail "no clip at $CLIP"
[[ -x $LMGW/target/debug/examples/headless ]] \
    || fail "no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"
grep -q TOOL_SCRIPT "$LMGW/scripts/mock-openai.py" || fail "$LMGW's mock-openai.py has no TOOL_SCRIPT (M7)"
git -C "$LMGW" grep -q 'mcp_approval_request' -- crates/lmgw-core/src || fail "$LMGW has no approvals (M3)"
for c in kwin_wayland spectacle magick; do command -v "$c" >/dev/null || fail "needs $c"; done
# 8898: the dev lmgw's device listener, which Kai pairs over (loopback, the HTTP port plus one).
for port in 8897 8898; do
    if ss -ltnH "sport = :$port" | grep -q .; then fail "something listens on $port already"; fi
done

rm -rf "$OUT"
mkdir -p "$OUT" "$DATA"
echo "lmgw: $(git -C "$LMGW" rev-parse --short HEAD) ($(git -C "$LMGW" status --short | wc -l) changed files, headless built $(date -r "$LMGW/target/debug/examples/headless" +%H:%M)); kai: $(git rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)" \
    | tee "$OUT/versions.txt"
scripts/lmgw-pin.sh check "$LMGW" 2>&1 | tee -a "$OUT/versions.txt"

cp --reflink=auto "$LMGW/target/debug/examples/headless" "$OUT/headless"
cat > "$OUT/lmgw-run.sh" <<RUN
#!/usr/bin/env bash
# Starts the dev lmgw of scripts/live-check-approvals.sh (the test runs this; written per run).
exec env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG="${LANG:-C.UTF-8}" \\
    XDG_RUNTIME_DIR="/run/user/$(id -u)" LMGW_DATA_DIR="$DATA" LMGW_DEV=1 \\
    LMGW_CONTAINER_PREFIX=lmgw-kaiappr "$OUT/headless" "$ADDR" >>"$OUT/lmgw.log" 2>&1
RUN
chmod +x "$OUT/lmgw-run.sh"

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
CHAT=$(free_port)
AUDIO=$(free_port)
# One call per turn, in the check's order; the arguments are marked so the log check finds them.
script='['
for n in 1 2 3 4 5 6 7; do
    script+="[{\"name\": \"kailive__live_note\", \"arguments\": {\"text\": \"s$n-arg-$RANDOM\"}}]"
    [[ $n == 7 ]] || script+=', '
done
script+=']'
TOOL_SCRIPT=$script STREAM_DELAY=0 setsid python3 "$LMGW/scripts/mock-openai.py" "$CHAT" \
    >"$OUT/mock-chat.log" 2>&1 &
PIDS+=($!)
ASR_TEXT="Ja." setsid python3 scripts/mock-voice-audio.py "$AUDIO" "$CLIP" >"$OUT/mock-audio.log" 2>&1 &
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

nv=()
if [[ -e /dev/nvidiactl || -e /sys/module/nvidia ]]; then
    nv=(__NV_DISABLE_EXPLICIT_SYNC="${__NV_DISABLE_EXPLICIT_SYNC:-1}")
fi
status=0
scripts/pw-private.sh "$OUT/pw" -- \
    "$here/scripts/nested-kwin.sh" "$OUT/nested" -- env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$OUT" \
    PIPEWIRE_RUNTIME_DIR="$OUT/pw/rt" KAI_LIVE_CLIP="$CLIP" KAI_LIVE_LMGW_URL="http://$ADDR" \
    KAI_LIVE_LMGW_RUN="$OUT/lmgw-run.sh" KAI_LIVE_LMGW_LOG="$OUT/lmgw.log" \
    KAI_LIVE_LMGW_PID="$OUT/lmgw.pid" KAI_LIVE_MOCK_CHAT="$CHAT" KAI_LIVE_MOCK_AUDIO="$AUDIO" \
    GDK_BACKEND=wayland "${nv[@]}" KAI_LOG="${KAI_LOG:-trace}" \
    "$test_bin" --ignored --exact voice::approval_live_tests::live_approvals_nested --nocapture \
    --test-threads=1 >"$OUT/test.log" 2>&1 || status=$?
grep -E '^([1-6]b? |crop|dev lmgw|the |no |Kai.s log|FAIL|skipped|test result|thread .* panicked|LEFT)' "$OUT/test.log" || true
if [[ $status == 0 ]] && grep -q 'test result: ok. 1 passed' "$OUT/test.log" && ! grep -q '^skipped' "$OUT/test.log"; then
    echo "== the approvals against the dev lmgw passed; report: $OUT/report.txt"
else
    echo "== FAILED (see $OUT/test.log, $OUT/kai.log, $OUT/lmgw.log)"
    status=1
fi
exit $status
