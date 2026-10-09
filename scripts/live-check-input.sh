#!/usr/bin/env bash
# WP16c's live check: the input tools in a nested KWin of their own (scripts/nested-kwin.sh: a
# private bus, home and XDG directories, nothing on the session's screen; the session's KWin,
# portal, accessibility bus and keyboard are never driven). Two runs in one nested session:
# KWin's fake input (without, then with the .desktop grant; typing into a GTK and a Qt entry, key
# chords, a call dropped half way, ui_press on GTK and Qt buttons), and the RemoteDesktop portal
# (xdg-desktop-portal and portal-kde on the private bus: the dialog wait, pre-authorisation, the
# stored restore token). Report in target/live-input/report.txt.
# usage: scripts/live-check-input.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-input
mkdir -p "$out"
for c in kwin_wayland python3 kdialog /usr/libexec/at-spi-bus-launcher /usr/libexec/at-spi2-registryd \
    /usr/libexec/xdg-desktop-portal /usr/libexec/xdg-desktop-portal-kde /usr/libexec/xdg-permission-store; do
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
    DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" "$0" --ignored --test-threads 1 --nocapture \
    desktop::tests::input::live::live_input_fake_in_nested_kwin \
    desktop::tests::input::live::live_input_portal_in_nested_kwin' "$test_bin" "$out" \
    > "$out/report.txt" 2>&1 || status=$?
grep -E '^(table|note|==|  |granted|type_text|key_press|ui_press|window_|windows|skipped|LEFT|thread|test |done|assertion|left|right)' \
    "$out/report.txt" || true
if grep -q '^skipped' "$out/report.txt"; then
    echo "FAIL the nested check skipped itself"
    status=1
fi
echo "report: $out/report.txt"
exit "$status"
