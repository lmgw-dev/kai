#!/usr/bin/env bash
# The lmgw source Kai is built and tested against (LMGW_PIN, review KAI-3 K3-5).
# Usage: scripts/lmgw-pin.sh check [lmgw checkout]   warn, never fail, when the checkout (default
#                                                     ../lmgw) is neither at the pinned commit nor
#                                                     at the pinned tree, or has changes, or when
#                                                     the pinned commit's tree is not the pinned tree
#        scripts/lmgw-pin.sh verify-tree <checkout>   fail unless the checkout's HEAD has the pinned
#                                                     tree and no changes (GitHub's CI, on its clone
#                                                     of the public lmgw at the pinned tag)
#        scripts/lmgw-pin.sh commit | tag | tree      print the pinned commit, tag or tree
# `just check` and the live checks run `check`; packaging/build-rpm.sh stages from `tree`.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
field() { awk -v k="$1" '$1 == k {print $2}' "$root/LMGW_PIN"; }
pin=$(field lmgw_commit)
tag=$(field lmgw_tag)
tree=$(field lmgw_tree)
[[ "$pin" =~ ^[0-9a-f]{40}$ ]] || { echo "LMGW_PIN holds no 40-hex lmgw_commit" >&2; exit 1; }
[[ "$tree" =~ ^[0-9a-f]{40}$ ]] || { echo "LMGW_PIN holds no 40-hex lmgw_tree" >&2; exit 1; }
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "LMGW_PIN holds no lmgw_tag vX.Y.Z" >&2; exit 1; }

case "${1:-check}" in
  commit)
    echo "$pin"
    ;;
  tag)
    echo "$tag"
    ;;
  tree)
    echo "$tree"
    ;;
  verify-tree)
    lmgw=$(realpath -m "${2:?usage: $0 verify-tree <lmgw checkout>}")
    fail() { echo "LMGW_PIN: $*" >&2; exit 1; }
    head=$(git -C "$lmgw" rev-parse --verify -q 'HEAD^{tree}' 2>/dev/null) || fail "no lmgw checkout at $lmgw"
    [[ "$head" == "$tree" ]] \
      || fail "lmgw at $lmgw has the tree $head, not the pinned $tree ($tag): the tag is not the" \
        "release Kai is pinned to, or LMGW_PIN's three lines disagree (docs/development.md, \"The lmgw pin\")"
    changed=$(git -C "$lmgw" status --porcelain --untracked-files=no | wc -l)
    ((changed == 0)) || fail "lmgw at $lmgw has $changed changed files"
    echo "lmgw at $lmgw has the pinned tree ${tree:0:12} ($tag)"
    ;;
  check)
    lmgw=$(realpath -m "${2:-$root/../lmgw}")
    warn() { echo "warning (LMGW_PIN): $*" >&2; }
    if ! head=$(git -C "$lmgw" rev-parse HEAD 2>/dev/null); then
      warn "no lmgw checkout at $lmgw; Kai is pinned to lmgw ${pin:0:7} ($tag, tree ${tree:0:7})"
      exit 0
    fi
    if git -C "$lmgw" cat-file -e "$pin^{commit}" 2>/dev/null; then
      pinned_tree=$(git -C "$lmgw" rev-parse "$pin^{tree}")
      if [[ "$pinned_tree" != "$tree" ]]; then
        warn "lmgw_commit ${pin:0:7}'s tree is ${pinned_tree:0:7}, not lmgw_tree ${tree:0:7}: the" \
          "three lines of LMGW_PIN must name the same source (docs/development.md, \"The lmgw pin\")"
      fi
    fi
    crates=(crates/lmgw-client crates/lmgw-api-types crates/lmgw-update crates/lmgw-ui-kit Cargo.toml)
    # At the pinned tree is at the pin, whichever commit carries it (a clone of the public lmgw at
    # lmgw_tag has other commits than lmgw's development history).
    if [[ "$head" != "$pin" && "$(git -C "$lmgw" rev-parse 'HEAD^{tree}')" != "$tree" ]]; then
      branch=$(git -C "$lmgw" branch --show-current 2>/dev/null || true)
      if ! git -C "$lmgw" cat-file -e "$pin^{commit}" 2>/dev/null; then
        same="the pin is not in that checkout (git -C $lmgw fetch)"
      elif git -C "$lmgw" diff --quiet "$pin" HEAD -- "${crates[@]}"; then
        same="lmgw's four crates are as at the pin; the dev lmgw may differ"
      else
        same="lmgw-client, lmgw-api-types, lmgw-update or lmgw-ui-kit differ from the pin"
      fi
      warn "lmgw at $lmgw is at ${head:0:7}${branch:+ ($branch)}, not the pinned ${pin:0:7}: Kai's" \
        "build and the live checks use that checkout ($same). Check out the pin, or bump LMGW_PIN" \
        "once Kai passes on it (docs/development.md)."
    fi
    changed=$(git -C "$lmgw" status --porcelain --untracked-files=no | wc -l)
    if ((changed > 0)); then
      warn "lmgw at $lmgw has $changed changed files: the build uses its working tree, not a commit."
    fi
    ;;
  *)
    echo "usage: $0 check [lmgw checkout] | verify-tree <lmgw checkout> | commit | tag | tree" >&2
    exit 2
    ;;
esac
