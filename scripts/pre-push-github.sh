#!/usr/bin/env bash
# pre-push guard for the public GitHub repo; scripts/publish-github.sh --install-guard installs it
# as .git/hooks/pre-push. Two rules:
# 1. Nothing goes to a github.com URL (however its case is spelled) by accident: only
#    scripts/publish-github.sh pushes there, with KAI_PUBLISH=1 set. GitHub gets one squashed
#    commit per publish; the granular history stays private. The `github` remote's pushurl is
#    disabled as well, so `git push github` fails before this runs.
# 2. Nothing that carries the private development history goes anywhere but the private origin
#    (`git remote get-url origin`, or its push URL): not to GitHub, nor to a mirror, an SSH alias or
#    a local path that leads there. Its root commit is below (the public history starts at a root
#    commit of its own, the first publish).
#    Checked without replace refs: a push sends the real ancestry, and a local graft (git replace)
#    must neither hide it nor make every push look tainted.
set -euo pipefail
remote="$1" url="$2"

# The private origin, compared exactly: it gets everything.
while read -r own; do
    if [[ -n "$own" && "$url" == "$own" ]]; then exit 0; fi
done < <(git remote get-url --all origin 2>/dev/null; git remote get-url --push --all origin 2>/dev/null)

shopt -s nocasematch
if [[ "$url" == *github.com* && "${KAI_PUBLISH:-}" != "1" ]]; then
    echo "pre-push: refusing to push to GitHub ($url). Publish with scripts/publish-github.sh." >&2
    exit 1
fi
shopt -u nocasematch

old_root=89dae3db1697e14887872af3b99f09f0b873e685
git --no-replace-objects cat-file -e "$old_root^{commit}" 2>/dev/null || exit 0   # not here: nothing to leak
zero=0000000000000000000000000000000000000000
while read -r local_ref local_sha _ _; do
    [[ "$local_sha" == "$zero" ]] && continue
    if git --no-replace-objects merge-base --is-ancestor "$old_root" "$local_sha"; then
        echo "pre-push: $local_ref carries the private history; refusing to push it to $remote ($url): only the private origin gets it" >&2
        exit 1
    fi
done
