#!/usr/bin/env bash
# WP13 (c): layer-shell keyboard on demand for the approval overlay (§11.1: Enter approves, Esc
# declines). A nested KWin with a focused test window (a text entry); the overlay (a 400x160
# layer surface in the output's centre, gtk-layer-shell like Kai's ring) maps over it with each
# keyboard mode; keys and clicks come from org_kde_kwin_fake_input (the probe holds the grant
# through a .desktop in the nested data directory). Who got which key is read from the two
# processes' logs (GTK focus-in/out = wl_keyboard enter/leave; key presses; the entry's text).
# usage: spikes/p3/run-c.sh     (output: spikes/p3/.run/c.log)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$here/target/debug/p3

if [[ ${1:-} == --inside ]]; then
    set +e
    d=$P3_DIR
    declare -A seen=()
    show() { # the lines each log gained since the last call
        sleep 0.4
        for f in "$d"/testwin.log "$d"/overlay-*.log; do
            [[ -f $f ]] || continue
            local n; n=$(wc -l < "$f")
            local s=${seen[$f]:-0}
            (( n > s )) && sed -n "$((s + 1)),${n}p" "$f" | sed "s|^|  [$(basename "$f" .log)] |"
            seen[$f]=$n
        done
    }
    step() { echo "-- $*"; }
    key() { "$bin" fake key "$@" > /dev/null; }      # evdev: 28 Enter, 1 Esc, 45 x, 46 c
    click() { "$bin" fake click "$@" > /dev/null; }
    overlay_c=640; overlay_y=360                       # the overlay's centre
    win_x=350; win_y=520                               # the test window, outside the overlay

    "$bin" testwin org.example.target Target 30,160,60 120 > "$d/testwin.log" 2>&1 &
    sleep 1.5
    step "test window up and focused; type x"; key 45; show

    for mode in on-demand exclusive none; do
        for layer in overlay top; do
            echo; echo "==== keyboard mode $mode, layer $layer, A: Enter right away"
            log="$d/overlay-$mode-$layer-a.log"
            "$bin" overlay "$mode" "$layer" 6 > "$log" 2>&1 &
            opid=$!
            sleep 1
            step "overlay mapped, no click yet"; show
            step "Enter (28) without a click"; key 28; show
            wait "$opid" 2>/dev/null
            step "overlay gone; type x (where does the keyboard go back to?)"; key 45; show
            click $win_x $win_y; sleep 0.2; show > /dev/null

            echo; echo "==== keyboard mode $mode, layer $layer, B: click away first"
            log="$d/overlay-$mode-$layer-b.log"
            "$bin" overlay "$mode" "$layer" 6 > "$log" 2>&1 &
            opid=$!
            sleep 1
            show > /dev/null
            step "click on the test window outside the overlay, then type c"; click $win_x $win_y; key 46; show
            step "click on the overlay, then Esc (1)"; click $overlay_c $overlay_y; key 1; show
            wait "$opid" 2>/dev/null
            step "overlay gone; type x"; key 45; show
            click $win_x $win_y; sleep 0.2; show > /dev/null
        done
    done
    kill %1 2>/dev/null; wait
    echo; echo "final:"; tail -n 1 "$d/testwin.log"
    exit 0
fi

[[ -x $bin ]] || { echo "build first: (cd $here && cargo build --offline -j 4)" >&2; exit 1; }
seed=$here/.run/c-seed
rm -rf "$seed"
mkdir -p "$seed/applications"
cat > "$seed/applications/dev.rootshell.kai-p3.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kai (WP13 probe)
Exec=$bin %u
NoDisplay=true
X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input
EOF
"$here/nested.sh" c --seed "$seed" -- "$0" --inside 2>&1 | tee "$here/.run/c.log"
