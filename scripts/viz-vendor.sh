#!/usr/bin/env bash
# Copies lmgw's engine.js and ring.js from lmgw main into assets/overlay/viz/ and rewrites the pin
# (the last main commit that touched them). Then: just viz-reference, just check.
#
# usage: scripts/viz-vendor.sh [lmgw checkout]   (default: ../lmgw next to this repository)
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
lmgw=${1:-$here/../lmgw}
viz=$here/assets/overlay/viz
src=crates/lmgw-ui/assets/voice/viz
files=(engine.js ring.js)
pin=$(git -C "$lmgw" log -1 --format=%H main -- "${files[@]/#/$src/}")
for f in "${files[@]}"; do
    git -C "$lmgw" show "$pin:$src/$f" > "$viz/$f"
done
{
    echo "# lmgw's ring files, vendored byte-identical (docs/design/2026-10-06-kai-design.md §9.2)."
    echo "# \`just viz-check\` verifies the copies against this pin and lmgw main against the pin (drift);"
    echo "# \`just viz-vendor\` copies them again from lmgw main and rewrites this file."
    echo "lmgw_commit $pin"
    echo "source $src"
    for f in "${files[@]}"; do echo "sha256 $f $(sha256sum "$viz/$f" | cut -d' ' -f1)"; done
} > "$viz/PIN"
echo "vendored ${files[*]} from lmgw ${pin:0:10}; next: just viz-reference, just check"
