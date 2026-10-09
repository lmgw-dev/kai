#!/usr/bin/env bash
# WP14's live check: Kai's desktop provider and window tools in a nested KWin of their own
# (scripts/nested-kwin.sh: two virtual outputs, a private bus and XDG directories, nothing on the
# session's screen, the session's KWin never scripted or read). Test windows of GTK 3 and kdialog;
# the steps run twice, with KWin scripting and with the window management protocol granted by a
# .desktop written in the nested data directory (src/desktop/live_tests.rs). Report in
# target/live-desktop/.
# usage: scripts/live-check-desktop.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-desktop
mkdir -p "$out"
for c in kwin_wayland python3 kdialog busctl; do
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
# The system bus is the private one too: logind and UPower of the real system are not read.
scripts/nested-kwin.sh "$out/nested" -- bash -c 'exec env KAI_LIVE_NESTED=1 \
    DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" "$0" --ignored --exact \
    desktop::live_tests::live_windows_in_nested_kwin --nocapture' "$test_bin" \
    > "$out/report.txt" 2>&1 || status=$?
grep -E '^(table|==|  |window_|desktop_|granted|test |skipped|LEFT|thread)' "$out/report.txt" || true
if grep -q '^skipped' "$out/report.txt"; then
    echo "FAIL the nested check skipped itself"
    status=1
fi

# `kai --tool-list` (what `just tool-cost` counts) in a nested Plasma session: its probe runs
# before the list is printed, so the desktop's tools are in it.
echo "== kai --tool-list in the nested session"
cargo build -q
scripts/nested-kwin.sh "$out/nested-list" -- env XDG_CURRENT_DESKTOP=KDE \
    DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent "$here/target/debug/kai" --tool-list kai \
    > "$out/tool-list.json" 2> "$out/tool-list.log" || status=$?
python3 -I -c '
import json, sys
names = [t["function"]["name"] for t in json.load(open(sys.argv[1]))]
want = ["kai__desktop_context", "kai__window_list", "kai__window_tile"]
missing = [w for w in want if w not in names]
print("tools:", len(names), "listed;", "missing: " + ", ".join(missing) if missing else "the desktop tools are in")
sys.exit(1 if missing else 0)' "$out/tool-list.json" || status=1
grep -E "desktop: windows" "$out/tool-list.log" || true
echo "report: $out/report.txt"
exit "$status"
