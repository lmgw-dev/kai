#!/usr/bin/env bash
# The dialog-free live check of the shortcuts portal path (WP4, §8.2/§8.3): in Kai's app scope
# (app-dev.rootshell.kai-dev<n>.scope, K12) the portal's CreateSession and ListShortcuts answer,
# and the portal backend sees the app id dev.rootshell.kai. No BindShortcuts (it opens KDE's
# dialog: that is scripts/owner-check-shortcuts.sh), so nothing is registered; the check confirms
# that kglobalshortcutsrc and KGlobalAccel's components are unchanged afterwards.
#
# usage: scripts/live-check-shortcuts.sh        Reports in target/live-shortcuts/.
# The dev .desktop file the portal needs is installed for the run and removed again unless it was
# installed before.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/live-shortcuts
mkdir -p "$out"
cd "$here"
export CARGO_BUILD_JOBS=8
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
desktop=$apps/dev.rootshell.kai.desktop
rc=${XDG_CONFIG_HOME:-$HOME/.config}/kglobalshortcutsrc

groups() { grep '^\[' "$rc" 2>/dev/null | sort || true; }
components() { busctl --user --json=short call org.kde.kglobalaccel /kglobalaccel \
    org.kde.KGlobalAccel allComponents 2>/dev/null \
    | python3 -c 'import json, sys; print("\n".join(sorted(json.load(sys.stdin)["data"][0])))' || true; }

monitor_pid=
installed_here=
cleanup() {
    [[ -n $monitor_pid ]] && kill "$monitor_pid" 2>/dev/null && wait "$monitor_pid" 2>/dev/null
    if [[ -n $installed_here ]]; then
        just uninstall-dev > "$out/uninstall.log" 2>&1 || echo "uninstall-dev failed: see $out/uninstall.log"
    fi
    return 0
}
trap cleanup EXIT

groups > "$out/groups-before.txt"
components > "$out/components-before.txt"

if [[ ! -f $desktop ]]; then
    just install-dev > "$out/install.log" 2>&1
    installed_here=1
fi

test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])' | tail -1)
[[ -x "$test_bin" ]] || { echo "no test binary"; exit 1; }

# What the portal hands its KDE backend: the app id it derived from the scope.
busctl --user monitor --match "type='method_call',interface='org.freedesktop.impl.portal.GlobalShortcuts'" \
    > "$out/backend-calls.txt" 2>&1 &
monitor_pid=$!
sleep 0.5

scope="app-dev.rootshell.kai-dev$(date +%s).scope"
echo "== CreateSession and ListShortcuts in $scope"
systemd-run --user --scope --quiet --collect -u "$scope" -- \
    env KAI_LIVE_PORTAL=1 "$test_bin" --ignored --exact --nocapture \
    shortcuts::tests::live_portal_session_without_a_bind 2>&1 | tee "$out/portal.log" | grep -E '^PORTAL|test result'
grep -q '^PORTAL' "$out/portal.log" || { echo "the portal test did not run"; exit 1; }
sleep 0.5
kill "$monitor_pid" 2>/dev/null; wait "$monitor_pid" 2>/dev/null || true
monitor_pid=

app_ids=$(grep -A12 'Member=CreateSession' "$out/backend-calls.txt" | grep -o 'STRING "dev[^"]*"' | sort -u || true)
echo "the backend's CreateSession app id: ${app_ids:-<not seen>}"
grep -q 'STRING "dev.rootshell.kai"' <<< "$app_ids" || { echo "the portal did not see dev.rootshell.kai"; exit 1; }
grep -q 'Member=BindShortcuts' "$out/backend-calls.txt" && { echo "a BindShortcuts went out"; exit 1; }

groups > "$out/groups-after.txt"
components > "$out/components-after.txt"
if diff -u "$out/groups-before.txt" "$out/groups-after.txt" \
    && diff -u "$out/components-before.txt" "$out/components-after.txt"; then
    echo "== kglobalshortcutsrc and KGlobalAccel's components are unchanged: nothing was registered"
else
    echo "== WARNING: shortcut groups or components changed (see above)"
    exit 1
fi
