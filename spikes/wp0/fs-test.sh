#!/usr/bin/env bash
# usage: fs-test.sh <tag> <layer overlay|top> [swatch|ring]
# WP0 (b first half, n): a normal GTK window goes fullscreen (solid 20,60,30) 1.5 s after the layer surface appears;
# at +4.5 s one capture is taken, the ring rect and two reference strips are cropped to shots/<tag>-*.png, the full capture is deleted.
# For an example 5120x1440 output at scale 1: the 320 px surface with 24 px margins sits at 4776,1096.
set -euo pipefail
cd "$(dirname "$0")"
tag=$1; layer=$2; mode=${3:-swatch}
BIN=../overlay/target/release/wp0
$BIN --mode "$mode" --layer "$layer" --scenario speaking --fullscreen 20,60,30 --fs-delay 1500 --secs 9 > "results/$tag.log" 2>&1 &
pid=$!
sleep 5
full=$(mktemp /tmp/kai-full-XXXX.png)
spectacle -b -n -f -o "$full" >/dev/null 2>&1 || true
magick "$full" -crop 320x320+4776+1096 +repage "shots/$tag-ring.png"
magick "$full" -crop 400x60+2200+1380 +repage "shots/$tag-panel-strip.png"     # where the Plasma panel normally is
magick "$full" -crop 200x100+100+100 +repage "shots/$tag-topleft.png"
rm -f "$full"
wait $pid || true
