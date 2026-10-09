#!/usr/bin/env bash
# Cut a release: set the version, commit it, and make the annotated tag v<X.Y.Z>. Pushes nothing.
#
# Usage: scripts/release.sh <X.Y.Z> [notes-file]
#
# Sets the version in every package of Kai (Cargo.toml, crates/*/Cargo.toml, ui/Cargo.toml), the
# spec's default (packaging/kai.spec) and both lock files (Cargo.lock, ui/Cargo.lock), commits
# "release X.Y.Z" on main and tags that commit. The tag's message becomes the GitHub release notes,
# Markdown with wrapped lines joined (packaging/unwrap-notes.awk): the notes file when given, else
# git opens your editor. When the tree already carries the version (the first release, 0.1.0), it
# only tags. Then publish:
#   git push origin main vX.Y.Z    GitLab: a private build X.Y.Z+<pipeline> of main
#   scripts/publish-github.sh --ref vX.Y.Z -F <message> --tag vX.Y.Z
#                                  GitHub: one squashed commit with the tag's tree, and the tag
#                                  on it (.github/workflows/release.yml builds the release)
# LMGW_PIN's lmgw_tag must be a published lmgw release by then: GitHub's build clones it.
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: scripts/release.sh <X.Y.Z> [notes-file]}"
notes="${2:-}"
die() { echo "$*" >&2; exit 1; }

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "not an X.Y.Z version: $version"
[[ "$(git branch --show-current)" == main ]] || die "releases are cut from main"
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || die "commit or stash your changes first"
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then die "tag v$version already exists"; fi
[[ -z "$notes" || -f "$notes" ]] || die "no notes file $notes"

manifests=(Cargo.toml crates/*/Cargo.toml ui/Cargo.toml)
current="$(grep -m1 -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | grep -oE '[0-9.]+')"
for m in "${manifests[@]}"; do
    [[ "$(grep -m1 -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' "$m" | grep -oE '[0-9.]+')" == "$current" ]] \
        || die "$m does not carry Kai's version $current: make the packages agree first"
done
if [[ "$current" != "$version" ]]; then
    [[ "$(printf '%s\n' "$current" "$version" | sort -V | tail -n1)" == "$version" ]] \
        || die "$version is older than the current $current"
    cur="${current//./\\.}"
    # The first `version = ` line of each manifest is its [package]'s.
    sed -i -E "0,/^version = \"$cur\"/s//version = \"$version\"/" "${manifests[@]}"
    sed -i -E "s/^(%\{!\?kai_version: %global kai_version )$cur\}/\1$version}/" packaging/kai.spec
    grep -q "kai_version $version}" packaging/kai.spec || die "packaging/kai.spec's default version was not set"
    cargo update --workspace --offline --quiet
    (cd ui && cargo update --workspace --offline --quiet)
    git commit -q -m "release $version" -- "${manifests[@]}" Cargo.lock ui/Cargo.lock packaging/kai.spec
fi

if [[ -n "$notes" ]]; then
    git tag -a "v$version" -F "$notes"
else
    git tag -a "v$version"
fi

echo "tagged v$version at $(git rev-parse --short HEAD). Publish with:"
echo "  git push origin main v$version"
echo "  scripts/publish-github.sh --dry-run --ref v$version -F <message> --tag v$version"
echo "  (audit, --approve, then the same without --dry-run; see the script's header)"
