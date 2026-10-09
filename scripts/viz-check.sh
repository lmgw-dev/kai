#!/usr/bin/env bash
# The ring's page against lmgw (§9.2): engine.js and ring.js byte-identical to lmgw at the pin
# (assets/overlay/viz/PIN) and unchanged on lmgw main since (drift), the levels reference made from
# the same engine.js, the adapter's round trip through lmgw's own Tap.read, and the palette against
# lmgw's voice tokens. Reads lmgw's main checkout only.
#
# It needs lmgw's development history (the vendoring commit and main). A checkout without the
# vendoring commit (a clone of the public lmgw, which carries one squashed commit per release) skips
# the whole check with one line saying so and exits 0; where the commit is present, every part is a
# hard check.
#
# usage: scripts/viz-check.sh [lmgw checkout]   (default: ../lmgw next to this repository)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
lmgw=${1:-$here/../lmgw}
viz=$here/assets/overlay/viz
pin=$(awk '$1 == "lmgw_commit" {print $2}' "$viz/PIN")
src=$(awk '$1 == "source" {print $2}' "$viz/PIN")
[[ -n $pin && -n $src ]] || { echo "FAIL $viz/PIN names no lmgw_commit or source"; exit 1; }
[[ -d $lmgw/.git || -f $lmgw/.git ]] || { echo "FAIL no lmgw checkout at $lmgw"; exit 1; }
if ! git -C "$lmgw" cat-file -e "$pin^{commit}" 2>/dev/null; then
    echo "viz-check: SKIPPED, lmgw at $lmgw lacks the vendoring commit ${pin:0:10} (lmgw's development history, not in a public clone): it would check engine.js and ring.js against that commit and lmgw main (drift), the levels reference and the adapter and palette" >&2
    exit 0
fi

status=0
while read -r kind file sum; do
    [[ $kind == sha256 ]] || continue
    got=$(sha256sum "$viz/$file" | cut -d' ' -f1)
    if [[ $got != "$sum" ]]; then
        echo "FAIL $file: sha256 $got, the pin says $sum (edited here? the copies stay verbatim)"
        status=1
    fi
    if ! git -C "$lmgw" show "$pin:$src/$file" | cmp -s - "$viz/$file"; then
        echo "FAIL $file differs from lmgw ${pin:0:10}:$src/$file"
        status=1
    fi
    if ! git -C "$lmgw" diff --quiet "$pin" main -- "$src/$file"; then
        echo "DRIFT lmgw main changed $src/$file since the pin ${pin:0:10}: review it, then just viz-vendor"
        status=1
    fi
done < "$viz/PIN"

reference=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["engine_sha256"])' \
    "$here/src/audio/levels_reference.json")
if [[ $reference != "$(sha256sum "$viz/engine.js" | cut -d' ' -f1)" ]]; then
    echo "FAIL src/audio/levels_reference.json was made from another engine.js: just viz-reference"
    status=1
fi

node "$here/scripts/viz-adapter-check.mjs" "$lmgw/crates/lmgw-ui/assets/voice.css" || status=1
if [[ $status == 0 ]]; then
    echo "viz-check: engine.js and ring.js are lmgw ${pin:0:10}'s, and lmgw main has not changed them"
fi
exit $status
