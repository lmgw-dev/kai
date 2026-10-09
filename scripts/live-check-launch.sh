#!/usr/bin/env bash
# WP16a's live check (src/desktop/fdo/launch/live_tests.rs), report in target/live-launch/:
# 1. K14 on the session's real user manager: `sleep` started as an application by a stand-in for
#    Kai survives the stand-in's SIGKILL, in its own app-….scope with only stdin, stdout and stderr
#    and no KAI_* variable; a command runs in its own scope, and one stopped at its time limit has
#    its whole scope killed (a `setsid` sleep too). Nothing with a window, and everything it starts
#    is stopped.
# 2. GUI applications only in a nested KWin (scripts/nested-kwin.sh: nothing on the session's
#    screen): app_launch and open show kdialog windows through a fake user manager on the nested
#    bus; both are found in the nested KWin's window list and closed.
# usage: scripts/live-check-launch.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-launch
mkdir -p "$out"
for c in kwin_wayland kdialog systemd-run python3; do
    command -v "$c" > /dev/null || { echo "needs $c"; exit 2; }
done
cd "$here"
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -I -c '
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"] \
            and m["target"]["name"] == "kai":
        print(m["executable"])')
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }
status=0
echo "== K14 on the session's user manager"
KAI_LIVE_SCOPE=1 "$test_bin" --ignored --exact desktop::fdo::launch::live_tests::live_scope_survives_kai \
    --nocapture > "$out/scope.txt" 2>&1 || status=$?
grep -E '^(command|application|live|skipped|test |thread)' "$out/scope.txt" || true
echo "== GUI applications in a nested KWin"
scripts/nested-kwin.sh "$out/nested" -- bash -c 'exec env KAI_LIVE_NESTED=1 "$0" --ignored --exact \
    desktop::fdo::launch::live_tests::live_launch_in_nested_kwin --nocapture' "$test_bin" \
    > "$out/nested.txt" 2>&1 || status=$?
grep -E '^(app_launch|open|scope|live|skipped|test |thread|LEFT)' "$out/nested.txt" || true
if grep -q '^skipped' "$out/scope.txt" "$out/nested.txt"; then
    echo "FAIL a check skipped itself"
    status=1
fi
echo "report: $out/"
exit "$status"
