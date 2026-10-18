#!/usr/bin/env bash
# Publish to the public GitHub repo as one squashed commit: a local ref's exact tree on top of the public tip.
#
# Usage: scripts/publish-github.sh [--ref REF] -F MSGFILE [--tag vX.Y.Z] [--dry-run]
#        scripts/publish-github.sh --approve TREE_SHA --note "WHO AUDITED" [--ack-scan]
#        scripts/publish-github.sh --install-guard
#
#   --ref REF         what to publish, default main: its tree, never its history
#   -F MSGFILE        the public commit's message (required unless only a tag is new); scanned
#   --tag vX.Y.Z      also tag the public commit (GitHub's release workflow builds from it). The
#                     local annotated tag vX.Y.Z (scripts/release.sh) must be on a commit with the
#                     published tree; its message becomes the public tag's, i.e. the release notes.
#                     A tree public already (the tip, or an earlier publish in publish-map) gets
#                     only the tag, on the public commit that carries it
#   --dry-run         everything except the push: audit files, scan, gate, the commit (and tag)
#                     object created locally and printed; exits 3 when the gate would refuse
#   --approve SHA     record that the diff for this tree (its full sha, as a run prints it) was
#                     audited; --ack-scan acknowledges every hit of that tree's scan
#   --install-guard   for another clone: add the github remote if it is missing, disable its push
#                     URL and its tag auto-follow, install scripts/pre-push-github.sh as pre-push
#
# The commit is `git commit-tree <ref>^{tree} -p github/main`: a tree copy, never a merge, so
# the private history never reaches GitHub. A run fetches main from the very URL it pushes to
# (read-only, no tags; the github remote must name that URL, so its remote-tracking ref is that
# repository's), refuses when that public tip or the commit to push descends from the private
# history's root commit (checked without replace refs, as the pre-push hook does),
# writes target/publish/<tree12>.diff (the whole diff from the public tip), .binaries (the added
# or changed binary files) and .scan (scripts/publish_scan.py: secret shapes, every non-GitHub
# remote host, and the private denylist), then gates: the diff and scan must be the ones
# approved in target/publish/<tree sha>.approved, the messages must scan clean, the author
# and committer e-mail must be GitHub no-reply addresses, and the tree's LMGW_PIN must name a
# published lmgw release with the pinned tree, which GitHub's CI clones (scripts/lmgw-pin.sh
# verify-published; between lmgw releases the pin says `unpublished`, for GitLab's builds only).
# Then it shows the commit, asks for the word "publish" on the terminal and pushes with
# KAI_PUBLISH=1, which the pre-push hook requires. Each publish is recorded in
# .git/info/publish-map as "<ref sha> <public sha> <date>", and a ref older than one recorded
# there is refused: copying it would revert the public repo.
#
# The first publish: while github has no main at all (the repository was created empty) and
# publish-map records no publish, the commit is a root commit, `git commit-tree <ref>^{tree}`
# with no parent, and the diff and the scan cover the whole tree (against the empty tree). An
# empty github after a recorded publish is refused: the public history is never started over.
#
# The denylist is private and lives outside the repo: KAI_PUBLISH_DENYLIST, else the same file
# lmgw's publish reads, ${XDG_CONFIG_HOME:-~/.config}/lmgw/publish-denylist, one case-insensitive
# regular expression per line, '#' comments (format and examples: scripts/publish_scan.py).
# KAI_PUBLISH_URL overrides the public URL (tests point it at a local bare repo): the github
# remote must name it, and every fetch and the push go to it. LMGW_PUBLIC_URL does the same for the
# public lmgw the pin is checked against, and is refused without KAI_PUBLISH_URL.
set -euo pipefail

REMOTE=github
PUBLIC_URL=git@github.com:lmgw-dev/kai.git
URL="${KAI_PUBLISH_URL:-$PUBLIC_URL}"
# The private development history's root commit (as in scripts/pre-push-github.sh): nothing that
# descends from it is ever published.
PRIVATE_ROOT=89dae3db1697e14887872af3b99f09f0b873e685
OUT=target/publish
DENYLIST="${KAI_PUBLISH_DENYLIST:-${XDG_CONFIG_HOME:-$HOME/.config}/lmgw/publish-denylist}"

die() { echo "publish-github: $*" >&2; exit 1; }
# The commit $1 carries the private history: it descends from PRIVATE_ROOT (real ancestry, no
# replace refs). A clone without that root has nothing of it to carry.
private_history() {
    git --no-replace-objects cat-file -e "$PRIVATE_ROOT^{commit}" 2>/dev/null || return 1
    git --no-replace-objects merge-base --is-ancestor "$PRIVATE_ROOT" "$1"
}
sha256() { sha256sum < "$1" | cut -d' ' -f1; }

ref=main msg="" tag="" dry=false approve="" note="" ack=false guard=false
while (($#)); do
    case "$1" in
        --ref) ref="${2:?--ref needs a ref}"; shift ;;
        -F) msg="${2:?-F needs a file}"; shift ;;
        --tag) tag="${2:?--tag needs vX.Y.Z}"; shift ;;
        --dry-run) dry=true ;;
        --approve) approve="${2:?--approve needs the tree sha}"; shift ;;
        --note) note="${2:?--note needs text}"; shift ;;
        --ack-scan) ack=true ;;
        --install-guard) guard=true ;;
        -h | --help) sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
    shift
done
# Paths given relative to where the owner stands, before moving to the repo root.
if [[ -n "$msg" ]]; then msg="$(realpath -e "$msg")" || die "no message file $msg"; fi
DENYLIST="$(realpath -m "$DENYLIST")"
cd "$(dirname "$0")/.."

# --- --install-guard -----------------------------------------------------------------------------
if $guard; then
    if have="$(git remote get-url "$REMOTE" 2>/dev/null)"; then
        [[ "$have" == "$URL" ]] \
            || die "the $REMOTE remote names $have, not the public $URL: fix it first (git remote set-url $REMOTE $URL)"
    else
        git remote add "$REMOTE" "$URL"
    fi
    git config "remote.$REMOTE.pushurl" DISABLED-publish-with-scripts/publish-github.sh
    # A public tag sits on the public commit, a local one on the private commit of the same
    # name: auto-following github's tags would clash with the local ones.
    git config "remote.$REMOTE.tagOpt" --no-tags
    hooks="$(git rev-parse --git-path hooks)"
    mkdir -p "$hooks"
    if [[ -e "$hooks/pre-push" ]] && ! cmp -s scripts/pre-push-github.sh "$hooks/pre-push"; then
        die "$hooks/pre-push exists and differs from scripts/pre-push-github.sh: compare them and remove yours first"
    fi
    install -m 755 scripts/pre-push-github.sh "$hooks/pre-push"
    echo "guard installed: $REMOTE's push URL disabled, its tags not auto-followed, $hooks/pre-push in place"
    exit 0
fi

# --- --approve -----------------------------------------------------------------------------------
if [[ -n "$approve" ]]; then
    [[ "$approve" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] \
        || die "--approve takes the full tree sha a run printed, not an abbreviation"
    [[ "$(git cat-file -t "$approve" 2>/dev/null)" == tree ]] || die "$approve is no tree in this repo"
    [[ -n "${note// /}" ]] || die "--approve needs --note \"who audited the diff\""
    t12="${approve:0:12}"
    [[ -f "$OUT/$t12.diff" && -f "$OUT/$t12.scan" && -f "$OUT/$t12.tree" ]] \
        || die "no audit files for this tree in $OUT: run a --dry-run first"
    [[ "$(cat "$OUT/$t12.tree")" == "$approve" ]] || die "$OUT/$t12.* belong to another tree"
    hits="$(grep -c . "$OUT/$t12.scan" || true)"
    if ((hits > 0)) && ! $ack; then
        die "the scan has $hits hit(s): read $OUT/$t12.scan, and approve with --ack-scan once every one is fine"
    fi
    {
        echo "tree $approve"
        echo "diff-sha256 $(sha256 "$OUT/$t12.diff")"
        echo "scan-sha256 $(sha256 "$OUT/$t12.scan")"
        echo "scan-hits $hits"
        echo "note $note"
        echo "date $(date -Iseconds)"
    } > "$OUT/$approve.approved"
    echo "approved: $OUT/$approve.approved (scan hits acknowledged: $hits)"
    exit 0
fi

# --- a publish, or its dry run -------------------------------------------------------------------
if [[ -n "$tag" ]]; then
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--tag takes vX.Y.Z, not $tag"
fi
[[ -f "$DENYLIST" ]] \
    || die "no private denylist at $DENYLIST (one case-insensitive regex per line; see scripts/publish_scan.py)"
case "$(realpath "$DENYLIST")" in
    "$(pwd -P)"/*) die "the denylist $DENYLIST is inside the repo: it holds private words, keep it outside" ;;
esac

# The pin is checked against another public lmgw than GitHub's only where the publish goes
# elsewhere too (the test).
if [[ -n "${LMGW_PUBLIC_URL:-}" && -z "${KAI_PUBLISH_URL:-}" ]]; then
    die "LMGW_PUBLIC_URL is set ($LMGW_PUBLIC_URL): a publish to GitHub checks the pin against GitHub's lmgw; unset it"
fi

map="$(git rev-parse --git-common-dir)/info/publish-map"
# github's main, or none at all: the repository was created empty and nothing is published yet.
# ls-remote's exit code 2 is "no such ref" (anything else failing is a real error).
have="$(git remote get-url "$REMOTE" 2>/dev/null)" \
    || die "no $REMOTE remote: run scripts/publish-github.sh --install-guard first"
# The parent is read from where the commit goes: the push URL, which the remote must name too.
[[ "$have" == "$URL" ]] \
    || die "the $REMOTE remote names $have, but the push goes to $URL: the public parent must come from the push target (git remote set-url $REMOTE $URL)"
set +e
git ls-remote --exit-code --heads "$URL" refs/heads/main >/dev/null
heads=$?
set -e
case "$heads" in
    0)
        git fetch --quiet --no-tags "$URL" "+refs/heads/main:refs/remotes/$REMOTE/main"
        public="$(git rev-parse --verify -q "refs/remotes/$REMOTE/main^{commit}")" \
            || die "fetched $REMOTE's main but cannot read it"
        if private_history "$public"; then
            die "$URL's main ($public) carries the private history (it descends from $PRIVATE_ROOT): that is no public repository, refusing to build on it"
        fi
        ;;
    2)
        if [[ -s "$map" ]]; then
            die "$REMOTE has no main, but $map records earlier publishes: the public history is never started over (look at $REMOTE first)"
        fi
        public=""
        ;;
    *) die "cannot read $REMOTE's branches (git ls-remote exited $heads)" ;;
esac
# The tree the diff starts from: the public tip's, or the empty tree on the first publish.
if [[ -n "$public" ]]; then
    base="$(git rev-parse "$public^{tree}")"
else
    base="$(git hash-object -t tree /dev/null)"
fi
src="$(git rev-parse --verify -q "$ref^{commit}")" || die "no commit $ref"
tree="$(git rev-parse "$src^{tree}")"
t12="${tree:0:12}"
# A public commit that carries this tree already: the tip, or (for a tag) an earlier publish.
already=""
if [[ -n "$public" && "$tree" == "$base" ]]; then
    already="$public"
elif [[ -n "$public" && -n "$tag" && -f "$map" ]]; then
    while read -r _ pub _; do
        if [[ "$(git rev-parse -q --verify "$pub^{tree}" 2>/dev/null)" == "$tree" ]] \
            && git merge-base --is-ancestor "$pub" "$public" 2>/dev/null; then
            already="$pub"
            break
        fi
    done < "$map"
fi
if [[ -z "$already" && -f "$map" ]]; then
    # Copying an older state onto the public tip would revert everything published since.
    while read -r done_src _; do
        if [[ "$done_src" != "$src" ]] && git merge-base --is-ancestor "$src" "$done_src" 2>/dev/null; then
            die "$ref is older than $done_src, which is public already: publishing it would revert the public repo"
        fi
    done < "$map"
fi
fresh=true
[[ -n "$already" ]] && fresh=false
if ! $fresh && [[ -z "$tag" ]]; then
    echo "nothing to publish: $ref's tree is the public tip's already ($public)"
    exit 0
fi
if $fresh; then
    [[ -n "$msg" ]] || die "-F <message file> is required: the public commit's message is part of what is audited"
    [[ -s "$msg" ]] || die "$msg is empty"
fi
mkdir -p "$OUT"
reasons=()

# The tag: a local annotated tag on the very tree being published, whose message is the notes.
tag_msg=""
if [[ -n "$tag" ]]; then
    [[ "$(git cat-file -t "refs/tags/$tag" 2>/dev/null)" == tag ]] \
        || die "$tag must be a local annotated tag (scripts/release.sh makes it): its message is the release notes"
    [[ "$(git rev-parse "refs/tags/$tag^{tree}")" == "$tree" ]] \
        || die "$tag's tree is not $ref's: publish the tagged commit itself (--ref $tag)"
    tag_msg="$OUT/$t12.tag-message"
    git cat-file tag "refs/tags/$tag" | sed '1,/^$/d' | sed '/^-----BEGIN [A-Z]* SIGNATURE-----$/,$d' > "$tag_msg"
    [[ -s "$tag_msg" ]] || die "$tag has an empty message: the release would have no notes"
fi

# The lmgw release the tree is pinned to, as committed in it: GitHub's CI and release build clone it.
if pin_check="$(scripts/lmgw-pin.sh --at "$src" verify-published 2>&1)"; then
    echo "lmgw pin: $pin_check"
else
    reasons+=("${pin_check//$'\n'/; }")
fi

# Who the commit and the tag name. Only GitHub's no-reply addresses are published.
for who in AUTHOR COMMITTER; do
    ident="$(git var "GIT_${who}_IDENT")"
    email="${ident#*<}" email="${email%%>*}"
    if [[ "$email" != *@users.noreply.github.com ]]; then
        reasons+=("the ${who,,} e-mail <$email> would be published: set a GitHub no-reply address for this repository, git -C \"$(pwd -P)\" config user.email <id>+<login>@users.noreply.github.com (GitHub shows yours under Settings, Emails; lmgw's checkout may have it set already: git -C ../lmgw config user.email)")
    fi
done

# The messages: no hit is acknowledgeable, they are the publisher's own words.
msg_scan="$OUT/$t12.message-scan"
: > "$msg_scan"
if [[ -n "$msg" ]]; then
    python3 scripts/publish_scan.py text --label message --denylist "$DENYLIST" "$msg" > "$msg_scan"
fi
if [[ -n "$tag_msg" ]]; then
    python3 scripts/publish_scan.py text --label tag-message --denylist "$DENYLIST" "$tag_msg" >> "$msg_scan"
fi
if [[ -s "$msg_scan" ]]; then
    reasons+=("the message scan has $(grep -c . "$msg_scan") hit(s), see $msg_scan: reword the message")
fi

if ! $fresh; then
    # Published already; only the tag is new, on the public commit that carries the tree.
    target="$already"
    echo "the tree is public already ($already): tagging that commit"
else
    diff="$OUT/$t12.diff" scan="$OUT/$t12.scan"
    echo "$tree" > "$OUT/$t12.tree"
    git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames \
        "$base" "$tree" > "$diff"
    python3 scripts/publish_scan.py tree --base "$base" --tree "$tree" --diff "$diff" \
        --binaries-out "$OUT/$t12.binaries" --denylist "$DENYLIST" > "$scan"
    if [[ -z "$public" ]]; then
        echo "first publish: $REMOTE has no main yet, so the commit has no parent and the audit is the whole tree"
    fi
    echo "audit: $diff ($(git diff --shortstat "$base" "$tree" | sed 's/^ //'))"
    echo "       $OUT/$t12.binaries ($(grep -c . "$OUT/$t12.binaries" || true) binary file(s))"
    echo "       $scan ($(grep -c . "$scan" || true) hit(s))"
    echo "tree:  $tree"
    approval="$OUT/$tree.approved"
    if [[ ! -f "$approval" ]]; then
        reasons+=("no approval for tree $tree: audit $diff, then: scripts/publish-github.sh --approve $tree --note \"who audited\" [--ack-scan]")
    elif [[ "$(sed -n 's/^tree //p' "$approval")" != "$tree" ]]; then
        reasons+=("$approval names another tree")
    elif [[ "$(sed -n 's/^diff-sha256 //p' "$approval")" != "$(sha256 "$diff")" ]]; then
        reasons+=("the diff is not the one approved (the public tip moved?): audit $diff again and re-approve")
    elif [[ "$(sed -n 's/^scan-sha256 //p' "$approval")" != "$(sha256 "$scan")" ]]; then
        reasons+=("the scan is not the one approved (new hits, or a changed denylist): read $scan and re-approve")
    fi
    parent=()
    [[ -n "$public" ]] && parent=(-p "$public")
    target="$(git commit-tree "$tree" "${parent[@]}" -F "$msg")"
fi

if private_history "$target"; then
    die "the commit to push ($target) carries the private history (it descends from $PRIVATE_ROOT): refusing"
fi

tag_obj=""
if [[ -n "$tag" ]]; then
    tag_obj="$({
        printf 'object %s\ntype commit\ntag %s\ntagger %s\n\n' "$target" "$tag" "$(git var GIT_COMMITTER_IDENT)"
        cat "$tag_msg"
    } | git mktag)"
fi

refs=()
$fresh && refs+=("$target:refs/heads/main")
[[ -n "$tag_obj" ]] && refs+=("$tag_obj:refs/tags/$tag")
echo
git --no-pager show --stat --format=fuller "$target"
[[ -n "$tag_obj" ]] && echo && git --no-pager cat-file tag "$tag_obj"
echo
echo "push target: $URL"
for r in "${refs[@]}"; do echo "  $r"; done

if ((${#reasons[@]})); then
    echo
    echo "gate: $($dry && echo "would refuse" || echo "refused"):"
    for r in "${reasons[@]}"; do echo "  - $r"; done
    exit 3
fi
if $dry; then
    echo
    echo "gate: open. Dry run: nothing pushed (the objects above stay unreferenced)."
    exit 0
fi

[[ -t 0 ]] || die "the confirmation is read from a terminal: run this in one"
read -r -p "type 'publish' to push the above to $URL: " answer
[[ "$answer" == publish ]] || { echo "not published"; exit 1; }
KAI_PUBLISH=1 git push --atomic "$URL" "${refs[@]}"
mkdir -p "$(dirname "$map")"
echo "$src $target $(date -Iseconds)${tag:+ $tag}" >> "$map"
echo "published: $target${tag:+ (tag $tag)}; recorded in $map"
