#!/usr/bin/env bash
# WP20's live check (src/routines/tests/live_tests.rs), report in target/live-routines/: a routine's
# script in a real transient scope of the session's user manager calls a test tool through the
# real `kai` binary (this worktree's debug build) and the test's own CLI socket under target/, and
# the real cgroup names the caller routine:live_probe; the same binary from outside any routine is
# the caller cli, and so is that binary in a routine-named scope Kai did not start (systemd-run); the
# routine run with kai tool streams its script's stdout. Nothing with a window, never the running
# Kai's socket, no scope left behind.
# usage: scripts/live-check-routines.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-routines
mkdir -p "$out"
for c in systemctl python3; do
    command -v "$c" > /dev/null || { echo "needs $c"; exit 2; }
done
cd "$here"
cargo build --quiet
kai_bin=$here/target/debug/kai
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -I -c '
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])')
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }
status=0
KAI_LIVE_ROUTINES=1 KAI_BIN="$kai_bin" "$test_bin" --ignored --exact \
    routines::tests::live_tests::live_routine_in_a_real_scope --nocapture \
    > "$out/routines.txt" 2>&1 || status=$?
grep -E '^(live|skipped|test |thread)' "$out/routines.txt" || true
if grep -q '^skipped' "$out/routines.txt"; then
    echo "FAIL the check skipped itself"
    status=1
fi
left=$(systemctl --user list-units --type=scope --all --no-legend 'app-dev.rootshell.kai.routine.*' || true)
if [[ -n "$left" ]]; then
    echo "FAIL routine scopes left: $left"
    status=1
fi
echo "report: $out/"
exit "$status"
