#!/usr/bin/env bash
# usage: shots.sh <tag> <x> <y> <w> <h> <t1,t2,..> -- <overlay args...>
# One foreground run: starts overlay (self-terminating), crops its rect from a full-screen capture at given seconds,
# deletes the full capture. Crops go to results/shots/<tag>-<t>s.png.
set -euo pipefail
cd "$(dirname "$0")"
tag=$1; x=$2; y=$3; w=$4; h=$5; times=$6; shift 7
full=$(mktemp /tmp/kai-full-XXXX.png)
spectacle -b -n -f -o "$full" >/dev/null 2>&1 || true
magick "$full" -crop "${w}x${h}+${x}+${y}" +repage "results/shots/$tag-base.png"
rm -f "$full"
./target/release/overlay "$@" > "results/$tag.log" 2>&1 &
pid=$!
start=$(date +%s.%N)
for t in ${times//,/ }; do
  now=$(date +%s.%N); sleep "$(python3 -c "print(max(0,$t-($now-$start)))")"
  full=$(mktemp /tmp/kai-full-XXXX.png)
  spectacle -b -n -f -o "$full" >/dev/null 2>&1 || true
  magick "$full" -crop "${w}x${h}+${x}+${y}" +repage "results/shots/$tag-${t}s.png"
  rm -f "$full"
done
wait $pid
