#!/usr/bin/env bash
# The owner's check of Kai's global shortcuts (WP4, §8): what the automated checks could not do, because
# BindShortcuts opens KDE's "Global Shortcuts Requested" dialog and the presses need real keys.
# Each step asks first. Run it from a terminal in the Plasma session, with no Kai running.
#
# usage: scripts/owner-check-shortcuts.sh
#
#   A  the first start: the dialog lists Kai's three actions (Talk to Kai, new conversation, mute)
#      with no key preset; give Talk a key, then tap it, hold it, and press mute. The log shows
#      the portal's Activated/Deactivated as Kai receives them: strictly alternating, no repeat
#      while held, and each hold's length (a tap is under shortcuts.hold_ms, 300 ms);
#   B  a second start: the shortcuts are returning, so no dialog should appear;
#   C  cleanup: keep the keys for Kai, or unregister Kai's actions from KGlobalAccel again.
#
# This build has no lmgw link yet, so a talk press only says "Kai is not paired" (or "lmgw is not
# running"); the point here is delivery. Reports and logs: target/owner-shortcuts/.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/owner-shortcuts
res=$out/verdicts.txt
mkdir -p "$out"
cd "$here"
export CARGO_BUILD_JOBS=8
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
desktop=$apps/dev.rootshell.kai.desktop
rc=${XDG_CONFIG_HOME:-$HOME/.config}/kglobalshortcutsrc
kai=$here/target/debug/kai
component=dev.rootshell.kai
actions=(talk new-conversation mute hide-overlays)

ask() { local a; read -r -p "$1 [y/N] " a; [[ $a == [yY]* ]]; }
enter() { read -r -p "$1 (Enter when done) " _; }
verdict() { echo "$(date -Is) ($1) $2" | tee -a "$res"; }

if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then
    echo "A Kai is running (it owns dev.rootshell.kai). Quit it from its tray first."
    exit 1
fi

kai_pid=
installed_here=
cleanup() {
    if [[ -n $kai_pid ]]; then
        kill -TERM "$kai_pid" 2>/dev/null || true
        wait "$kai_pid" 2>/dev/null || true
    fi
    if [[ -n $installed_here ]]; then
        just uninstall-dev > "$out/uninstall.log" 2>&1 || echo "uninstall-dev failed: see $out/uninstall.log"
    fi
    return 0
}
trap cleanup EXIT

# Kai in its own app scope (K12), so the portal sees dev.rootshell.kai; $1 names the log.
start_kai() {
    systemd-run --user --scope --quiet --collect -u "app-dev.rootshell.kai-dev$(date +%s%N | cut -c1-12).scope" -- \
        env KAI_LOG="info,kai::shortcuts=debug" "$kai" > "$out/$1.log" 2>&1 &
    kai_pid=$!
}
stop_kai() {
    kill -TERM "$kai_pid" 2>/dev/null || true
    wait "$kai_pid" 2>/dev/null || true
    kai_pid=
}

# The edges Kai received, with each hold's length.
edges() { python3 - "$out/$1.log" <<'PY'
import re, sys
from datetime import datetime
rows = []
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    m = re.search(r'(\d{4}-\d\d-\d\dT[\d:.]+)Z.*shortcut id="?([\w-]+)"? pressed=(true|false)', line)
    if m:
        t = datetime.fromisoformat(m.group(1)[:26])
        rows.append((t, m.group(2), m.group(3) == "true"))
down = {}
seq = []
for t, sid, pressed in rows:
    seq.append(("A" if pressed else "D") + ":" + sid)
    if pressed:
        if sid in down:
            print(f"  {sid}: a repeated Activated while held")
        down[sid] = t
    elif sid in down:
        print(f"  {sid}: held {(t - down.pop(sid)).total_seconds() * 1000:.0f} ms")
print("  sequence: " + (" ".join(seq) or "nothing received"))
PY
}

[[ -f $desktop ]] || { just install-dev > "$out/install.log" 2>&1; installed_here=1; }
cargo build -q
grep '^\[' "$rc" 2>/dev/null | sort > "$out/groups-before.txt" || true

# ------------------------------------------------------------------------------------------- A
echo
echo "A. The first start. KDE's dialog should list Kai's three actions with no key preset."
echo "   Give 'Talk to Kai' a key (avoid Right Alt if your layout uses it as AltGr), optionally the others."
echo "   Then: tap the talk key once, wait 2 s, hold it for 2 s, release; press mute if you set it."
if ask "Run A?"; then
    start_kai kai-a
    enter "Answer the dialog, then tap, hold and release as above."
    grep -o 'global shortcuts bound.*' "$out/kai-a.log" | head -1 || echo "  (no bind answer in the log)"
    edges kai-a | tee "$out/edges-a.txt"
    stop_kai
    if ask "Did the dialog list the three actions with no key preset?"; then
        verdict A "dialog: three actions, no preset key"
    else
        verdict A "dialog: NOT as expected"
    fi
    verdict A "edges: $(tail -1 "$out/edges-a.txt" | sed 's/^ *//'); $(grep -c 'held' "$out/edges-a.txt" || true) holds measured"
    if grep -q 'repeated Activated' "$out/edges-a.txt"; then verdict A "a held key REPEATED Activated"; fi
fi

# ------------------------------------------------------------------------------------------- B
echo
echo "B. A second start: the shortcuts are bound already, so no dialog should appear."
if ask "Run B?"; then
    start_kai kai-b
    enter "Wait a few seconds; tap the talk key once."
    grep -o 'global shortcuts bound.*' "$out/kai-b.log" | head -1 || echo "  (no bind answer in the log)"
    edges kai-b | tee "$out/edges-b.txt"
    stop_kai
    if ask "Did a dialog appear?"; then
        verdict B "the dialog appeared again on a second start"
    else
        verdict B "no dialog on a second start"
    fi
fi

# ------------------------------------------------------------------------------------------- C
echo
echo "C. Cleanup."
if ask "Keep the keys you gave Kai (for later runs)?"; then
    verdict C "kept"
else
    for a in "${actions[@]}"; do
        r=$(busctl --user call org.kde.kglobalaccel /kglobalaccel org.kde.KGlobalAccel unregister ss "$component" "$a" 2>&1 || true)
        echo "  unregister $a: $r"
    done
    sleep 1
    if grep -q "^\[$component\]" "$rc" 2>/dev/null; then
        echo "  the group [$component] is still in $rc; remove it in System Settings → Shortcuts, or:"
        echo "  kwriteconfig6 --file kglobalshortcutsrc --group $component --delete-group"
        verdict C "unregistered; the [$component] group remained"
    else
        verdict C "unregistered; no [$component] group left"
    fi
fi
grep '^\[' "$rc" 2>/dev/null | sort > "$out/groups-after.txt" || true
diff -u "$out/groups-before.txt" "$out/groups-after.txt" || true
echo "Verdicts in $res"
