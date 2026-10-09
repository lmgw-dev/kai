#!/usr/bin/env bash
# Build the kai RPM in a Fedora podman container; output in target/rpm/out/.
# Usage: packaging/build-rpm.sh [--page-only]   (just rpm).  KAI_VERSION overrides the version (CI:
#        <X.Y.Z>+<IID>); --page-only stages the sources and builds only the configuration window's page
#        (ui/dist in target/rpm/src/kai), then stops: the check that trunk and the wasm target work.
#        LMGW_DIR names the lmgw checkout (default: derived from the lmgw-client path in Cargo.toml).
#        KAI_UPDATE_MANIFEST_URL and KAI_UPDATE_DEPLOY_TOKEN, when set, are compiled in as the
#        updater's feed and its token, KAI_UPDATE_PUBLIC_FEED as the mark of a public feed that
#        never gets a token (src/update.rs; CI sets them: packaging/ci-build.sh,
#        packaging/gh-build.sh); unset, the build has no update feed.
# lmgw's crates come from the tree LMGW_PIN names (`git archive`), never from that checkout's
# working tree or HEAD: the RPM is the same whatever is checked out there. The tree, not the commit:
# it is the same in lmgw's development history and in its public repository (other commits, the
# same tree per release), so this works in either clone. Where the checkout holds the pinned commit
# its tree must be the pinned tree.
# The configuration window's page (ui/, WP21b) is built here, by trunk in the same Fedora image, into
# the staged tree's ui/dist, which the tarball carries and cargo's release build embeds: rpmbuild
# itself never needs trunk or the wasm target. Its UI kit (lmgw-ui-kit) is staged from the pin too.
# Idempotent; the cargo registry and the build's target dir are cached in target/rpm/.
# With GITLAB_CI=true or GITHUB_ACTIONS=true (inside a CI job, already a Fedora container) it builds
# in place, no podman.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
rpmdir=$root/target/rpm
FEDORA_RELEASE=${FEDORA_RELEASE:-$(. /etc/os-release && echo "$VERSION_ID")}
page_only=false
[[ "${1:-}" == "--page-only" ]] && page_only=true
in_place=false
[[ "${GITLAB_CI:-}" == "true" || "${GITHUB_ACTIONS:-}" == "true" ]] && in_place=true

base=$(grep -m1 -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | grep -oE '[0-9.]+')
version=${KAI_VERSION:-$base}

# lmgw-client: the crate directory Cargo.toml names, its lmgw checkout two levels up; the tree
# LMGW_PIN names, which that checkout must hold (review KAI-3 K3-5).
crate=$(sed -nE 's|^lmgw-client = \{ path = "([^"]+)".*|\1|p' Cargo.toml)
[[ -n "$crate" ]] || { echo "no lmgw-client path in Cargo.toml" >&2; exit 1; }
[[ "$crate" = /* ]] || crate=$(realpath -m "$root/$crate")
lmgw=${LMGW_DIR:-$(realpath -m "$crate/../..")}
pin=$(scripts/lmgw-pin.sh commit)
tree=$(scripts/lmgw-pin.sh tree)
tag=$(scripts/lmgw-pin.sh tag)
[[ "$(git -C "$lmgw" cat-file -t "$tree" 2>/dev/null)" == tree ]] || {
  echo "the pinned lmgw tree $tree (LMGW_PIN, $tag) is not in $lmgw: fetch it there, or set LMGW_DIR" >&2
  exit 1
}
# How the package's description names the source: the commit where the checkout has it (lmgw's
# development history), else the public release whose tree it is.
if git -C "$lmgw" cat-file -e "$pin^{commit}" 2>/dev/null; then
  [[ "$(git -C "$lmgw" rev-parse "$pin^{tree}")" == "$tree" ]] || {
    echo "LMGW_PIN disagrees with itself: lmgw_commit ${pin:0:12}'s tree is not lmgw_tree ${tree:0:12}" >&2
    exit 1
  }
  lmgw_source="commit $pin"
else
  lmgw_source="$tag (tree $tree)"
fi
for d in lmgw-client lmgw-api-types lmgw-update lmgw-ui-kit; do
  git -C "$lmgw" cat-file -e "$tree:crates/$d/Cargo.toml" 2>/dev/null || {
    echo "lmgw's tree ${tree:0:12} has no crates/$d" >&2
    exit 1
  }
done
echo "lmgw-client, lmgw-api-types, lmgw-update and lmgw-ui-kit from lmgw $lmgw_source (LMGW_PIN), via git archive in $lmgw"

# The source tarball: kai/ (tracked files, working tree state) next to lmgw/ (the four crates at
# the pinned tree and a minimal workspace root carrying the pin's [workspace.package]); the staged
# Cargo.toml's paths are set to ../lmgw/crates/<crate>, the tarball's layout, whatever this
# checkout's Cargo.toml says (ui/Cargo.toml's lmgw-ui-kit path, ../../lmgw/crates/lmgw-ui-kit from
# kai/ui, is that layout already).
src=$rpmdir/src
rm -rf "$src" "$rpmdir/SOURCES"
mkdir -p "$src/kai" "$src/lmgw/crates" "$rpmdir/SOURCES" "$rpmdir/out"
git ls-files -z --cached --exclude-standard | tar --null -T - -cf - | tar -xf - -C "$src/kai"
paths=()
for d in lmgw-client lmgw-api-types lmgw-update lmgw-ui-kit; do paths+=("crates/$d/Cargo.toml" "crates/$d/src"); done
paths+=("crates/lmgw-ui-kit/assets")  # kit.css and profiles.css, which the page's index.html links
git -C "$lmgw" archive --format=tar "$tree" -- "${paths[@]}" | tar -xf - -C "$src/lmgw"
{
  printf '[workspace]\nresolver = "2"\nmembers = ["crates/lmgw-api-types", "crates/lmgw-client", "crates/lmgw-update", "crates/lmgw-ui-kit"]\n\n'
  git -C "$lmgw" show "$tree:Cargo.toml" | awk '/^\[workspace\.package\]/{p=1;print;next} /^\[/{p=0} p&&NF'
} > "$src/lmgw/Cargo.toml"
sed -i -E 's#^(lmgw-(client|update) = \{ path = )"[^"]+"#\1"../lmgw/crates/lmgw-\2"#' "$src/kai/Cargo.toml"
install -D -m644 packaging/kai.spec "$rpmdir/spec/kai.spec"

# The page, in the staged tree. Locally in the build image (trunk and the wasm target are in it);
# in CI the job is that image already. wasm-bindgen and wasm-opt are
# the image's pinned ones (install-wasm-tools.sh); trunk runs --offline, so it never downloads them.
# The tag carries a hash of what the image is built from, so a changed pin rebuilds it.
image=localhost/kai-rpm-build:$FEDORA_RELEASE-$(cat packaging/Containerfile packaging/install-trunk.sh packaging/install-wasm-tools.sh | sha256sum | cut -c1-12)
mkdir -p "$rpmdir/cargo-home" "$rpmdir/cargo-target" "$rpmdir/rpmbuild" "$rpmdir/cache" "$rpmdir/ui-target"
rm -rf "$src/kai/ui/dist"
if $in_place; then
  (cd "$src/kai/ui" && XDG_CACHE_HOME=$rpmdir/cache CARGO_TARGET_DIR=$rpmdir/ui-target trunk build --release --offline --locked)
else
  podman image exists "$image" || podman build --build-arg FEDORA_RELEASE="$FEDORA_RELEASE" \
    -t "$image" -f packaging/Containerfile packaging
  # Only target/rpm is mounted (relabelled :Z), as for rpmbuild below.
  podman run --rm --name kai-rpm-page \
    -v "$rpmdir:/work:Z" -w /work/src/kai/ui \
    -e CARGO_HOME=/work/cargo-home -e CARGO_TARGET_DIR=/work/ui-target -e XDG_CACHE_HOME=/work/cache \
    "$image" trunk build --release --offline --locked
fi
[[ -f "$src/kai/ui/dist/index.html" ]] || { echo "the page was not built: $src/kai/ui/dist/index.html is missing" >&2; exit 1; }
echo "the configuration window's page: $(du -sh "$src/kai/ui/dist" | cut -f1) in $src/kai/ui/dist"
$page_only && exit 0
tar -C "$src" -czf "$rpmdir/SOURCES/kai-$version.tar.gz" kai lmgw

# The updater's feed is decided at compile time (option_env! in src/update.rs): what is set here
# reaches cargo, in place or through podman's -e (by name, so a token never sits in an argv).
feed_env=()
for v in KAI_UPDATE_MANIFEST_URL KAI_UPDATE_DEPLOY_TOKEN KAI_UPDATE_PUBLIC_FEED; do
  # shellcheck disable=SC2163 # exports the variable $v names
  if [[ -n "${!v:-}" ]]; then export "$v"; feed_env+=(-e "$v"); fi
done
echo "kai $version; update feed baked in: ${KAI_UPDATE_MANIFEST_URL:-none (this build does not poll)}" \
  "(deploy token: $([[ -n "${KAI_UPDATE_DEPLOY_TOKEN:-}" ]] && echo yes || echo no);" \
  "public, never a token: $([[ -n "${KAI_UPDATE_PUBLIC_FEED:-}" ]] && echo yes || echo no))"

rpmbuild_args=(-bb --define "kai_version $version" --define "lmgw_commit $pin" --define "lmgw_source $lmgw_source"
               --define "_topdir $rpmdir/rpmbuild" --define "_sourcedir $rpmdir/SOURCES" --define "_rpmdir $rpmdir/out" "$root/packaging/kai.spec")
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}

if $in_place; then
  export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$rpmdir/cargo-target}
  rpmbuild "${rpmbuild_args[@]}"
else
  # Only target/rpm is mounted (relabelled :Z): the staged sources, the caches and the output.
  podman run --rm --name kai-rpm-build \
    -v "$rpmdir:/work:Z" -w /work \
    -e CARGO_HOME=/work/cargo-home -e CARGO_TARGET_DIR=/work/cargo-target \
    -e CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" "${feed_env[@]}" \
    "$image" rpmbuild -bb --define "kai_version $version" --define "lmgw_commit $pin" \
      --define "lmgw_source $lmgw_source" \
      --define "_topdir /work/rpmbuild" \
      --define "_sourcedir /work/SOURCES" --define "_rpmdir /work/out" /work/spec/kai.spec
fi
