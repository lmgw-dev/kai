#!/usr/bin/env bash
# WP15's live check: the seeing and reading tools in a nested KWin of their own
# (scripts/nested-kwin.sh: two virtual outputs, a private bus, home and XDG directories, nothing on
# the session's screen; the session's KWin, clipboard, accessibility bus and file index are never
# read). Captures without and with ScreenShot2's grant (a .desktop written in the nested data
# directory), a private AT-SPI bus with a GTK window and a Qt dialog (and Kai's accessibility switch
# around a read of a Qt dialog that follows it), a private Baloo index of test
# files, a recent list (src/desktop/read/live_tests.rs). Report and the captured PNGs in
# target/live-read/.
# usage: scripts/live-check-read.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-read
mkdir -p "$out"
for c in kwin_wayland python3 kdialog baloosearch6 /usr/libexec/kf6/baloo_file \
    /usr/libexec/at-spi-bus-launcher /usr/libexec/at-spi2-registryd; do
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
scripts/nested-kwin.sh "$out/nested" -- bash -c 'exec env KAI_LIVE_NESTED=1 KAI_LIVE_OUT="$1" \
    DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" "$0" --ignored --exact \
    desktop::read::live_tests::live_read_in_nested_kwin --nocapture' "$test_bin" "$out" \
    > "$out/report.txt" 2>&1 || status=$?
grep -E '^(table|==|  |see_|read_|find_|recent_|switch |granted|test |skipped|LEFT|thread|done)' \
    "$out/report.txt" || true
if grep -q '^skipped' "$out/report.txt"; then
    echo "FAIL the nested check skipped itself"
    status=1
fi
echo "report: $out/report.txt; captures: $out/*.png"
exit "$status"
