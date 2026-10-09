#!/usr/bin/env bash
# WP13 (e) ScreenShot2's CaptureWindow by the window management protocol's uuid, and (f)
# org.kde.kwin.Scripting for geometry and tiling. One nested KWin with two test windows (green,
# then blue on top of it); the probe binary holds ruling 16's grant through a .desktop in the
# nested data directory (run-d.sh shows that works); kwin_script.py holds none.
# usage: spikes/p3/run-ef.sh     (output: spikes/p3/.run/ef.log, captures in .run/ef/)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$here/target/debug/p3

if [[ ${1:-} == --inside ]]; then
    out=$P3_DIR
    set +e
    say() { echo; echo "## $*"; }
    "$bin" testwin org.example.green Green 30,160,60 30 > "$out/green.log" 2>&1 &
    sleep 1.5
    "$bin" testwin org.example.blue Blue 30,60,200 30 > "$out/blue.log" 2>&1 &
    sleep 1.5
    say "(e) windows by the window management protocol"
    "$bin" windows | tee "$out/windows.txt"
    green=$(sed -n 's/^window uuid=\([^ ]*\) app_id="org.example.green".*/\1/p' "$out/windows.txt")
    blue=$(sed -n 's/^window uuid=\([^ ]*\) app_id="org.example.blue".*/\1/p' "$out/windows.txt")
    say "(e) CaptureWindow(green uuid as given, braces) — green is behind blue"
    "$bin" shot window "$green" "$out/green.ppm"
    say "(e) CaptureWindow(green uuid without braces)"
    "$bin" shot window "$(tr -d '{}' <<< "$green")"
    say "(e) CaptureWindow(green, include-decoration)"
    "$bin" shot window "$green" --decoration
    say "(e) CaptureWindow(blue uuid)"
    "$bin" shot window "$blue" "$out/blue.ppm"
    say "(e) CaptureActiveWindow"
    "$bin" shot active
    say "(e) CaptureWindow(an unknown uuid)"
    "$bin" shot window "{00000000-0000-0000-0000-000000000001}" || true
    say "(e) CaptureWindow(a minimized window): minimize green through KWin scripting, capture, restore"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/set.js" "{\"id\": \"$green\", \"prop\": \"minimized\", \"value\": true}"
    sleep 0.5
    "$bin" shot window "$green"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/set.js" "{\"id\": \"$green\", \"prop\": \"minimized\", \"value\": false}"
    sleep 0.5

    say "(f) Scripting from a process with no grant (python3): query"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/query.js" | tee "$out/query.json"
    say "(f) the scripting ids against the protocol's uuids"
    python3 - "$out/query.json" "$green" "$blue" <<'PY'
import json, sys
q = json.load(open(sys.argv[1]))
ids = {w["id"]: w["resourceClass"] for w in q["windows"]}
for u in sys.argv[2:]:
    print(u, "->", ids.get(u, "NOT FOUND in scripting's internalId list"))
PY
    say "(f) geometry: green to 40,40 500x300"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/geometry.js" \
        "{\"id\": \"$green\", \"frame\": {\"x\": 40, \"y\": 40, \"width\": 500, \"height\": 300}}"
    "$bin" windows | grep green
    say "(f) tiling: quick tile left (green)"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/tile.js" "{\"id\": \"$green\", \"how\": \"quick-left\"}"
    say "(f) tiling: quick tile right (blue)"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/tile.js" "{\"id\": \"$blue\", \"how\": \"quick-right\"}"
    "$bin" windows
    say "(f) tiling: maximize (green)"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/tile.js" "{\"id\": \"$green\", \"how\": \"maximize\"}"
    say "(f) tiling: custom tiling, left tile (blue)"
    python3 "$here/kwin_script.py" "$here/kwin-scripts/tile.js" "{\"id\": \"$blue\", \"how\": \"custom-left\"}" || true
    "$bin" windows
    say "(f) round trip, 10 queries"
    for i in $(seq 10); do python3 "$here/kwin_script.py" "$here/kwin-scripts/query.js" | python3 -c 'import json,sys; print(json.load(sys.stdin)["_round_trip_ms"], end=" ")'; done; echo
    say "(f) scripts left loaded in KWin"
    busctl --user tree org.kde.KWin | grep -i script || true
    kill %1 %2 2>/dev/null || true
    wait
    exit 0
fi

[[ -x $bin ]] || { echo "build first: (cd $here && cargo build --offline -j 4)" >&2; exit 1; }
seed=$here/.run/ef-seed
rm -rf "$seed"
mkdir -p "$seed/applications"
cat > "$seed/applications/dev.rootshell.kai-p3.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kai (WP13 probe)
Exec=$bin %u
NoDisplay=true
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,zkde_screencast_unstable_v1,org_kde_kwin_fake_input
EOF
"$here/nested.sh" ef --seed "$seed" -- "$0" --inside 2>&1 | tee "$here/.run/ef.log"
