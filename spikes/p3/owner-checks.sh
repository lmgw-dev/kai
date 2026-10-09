#!/usr/bin/env bash
# WP13, the owner's points: (a) AT-SPI trees of Qt and Chromium apps on Plasma 6 Wayland and the
# cost of switching accessibility on; (b) Klipper's D-Bus history. Run from a terminal in the
# Plasma session, a few minutes:  spikes/p3/owner-checks.sh [a] [b]   (default: both)
# Results go to spikes/p3/results/owner-<pt>.txt (kept out of the repository). They hold counts,
# roles, process names and timings only: no window titles, no node names, no clipboard text.
# Nothing is typed, clicked or sent anywhere. (a) changes one setting for about a minute and puts
# it back: org.a11y.Status IsEnabled and ScreenReaderEnabled (GSettings toolkit-accessibility and
# screen-reader-enabled), restored on exit and on Ctrl+C.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/results"
points=("$@"); [ ${#points[@]} -eq 0 ] && points=(a b)
say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

point_a() {
    say "(a) AT-SPI. Open the apps you want measured first: e.g. Dolphin, Kate, System Settings,"
    echo "    Firefox, and Chromium twice: once plain, once started with --force-renderer-accessibility."
    echo "    Step 1 is read-only. Step 2 switches accessibility on for ~1 minute, measures, switches it back."
    read -r -p "  [Enter] when the apps are open... " _
    local out=$here/results/owner-a.txt
    { echo "# $(date -Is) owner-checks a"; echo "chromium processes: $(pgrep -u "$USER" -fa 'chrom' | grep -c -- '--type=renderer') renderers, with the flag: $(pgrep -u "$USER" -fa -- '--force-renderer-accessibility' | grep -c .)"; } > "$out"
    python3 "$here/atspi_probe.py" | tee -a "$out"
    local go; read -r -p "  Step 2: switch accessibility on for the measurement (restored after)? [y/N] " go
    if [ "$go" = y ]; then
        python3 "$here/atspi_probe.py" --enable --secs 10 | tee -a "$out"
    fi
    echo "  -> $out"
}

point_b() {
    say "(b) Klipper's history over D-Bus. Read-only; prints counts and lengths, not the text."
    local out=$here/results/owner-b.txt
    { echo "# $(date -Is) owner-checks b"; python3 "$here/klipper_probe.py"; } 2>&1 | tee "$out"
    echo "  -> $out   (klipper_probe.py --show prints the first 40 characters per item on the terminal only)"
}

for p in "${points[@]}"; do
    case $p in
        a) point_a ;;
        b) point_b ;;
        *) echo "unknown point $p (a or b)" ;;
    esac
done
