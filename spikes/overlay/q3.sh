#!/usr/bin/env bash
# usage: q3.sh <scenario speaking|idle> <run#>   (20 s no-overlay GPU baseline, then 20 s measured window; overlay lives ~30 s)
set -euo pipefail
cd "$(dirname "$0")"
./measure.py q3-$1-$2 --base 20 --warm 6 --cpu 20 -- --mode ring --bg alpha --scenario "$1" --size 320 --secs 30 >/dev/null
