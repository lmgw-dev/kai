#!/usr/bin/env bash
# Kai's public release build, in GitHub Actions (.github/workflows/release.yml), one per pushed tag
# v<X.Y.Z> (or dispatched for an existing one): the RPM of that tag, version X.Y.Z (which must equal the version in Cargo.toml, so the
# tagged source says what it is), its updater pointed at the public feed, and the manifest that feed
# serves (latest.json). Output in target/rpm/dist/ (the RPM, latest.json), which the workflow
# attaches to the tag's GitHub release.
# Usage: bash packaging/gh-build.sh   (inside the GitHub job only; `just rpm` builds locally, with
#        no feed; packaging/ci-build.sh is GitLab's private feed)
#
# The public feed is `latest.json` as an asset of the newest release of this repository, which
# GitHub redirects to the file itself: https://github.com/lmgw-dev/kai/releases/latest/download/
# latest.json. It needs no token, and none is compiled in: KAI_UPDATE_DEPLOY_TOKEN is unset, not
# merely left alone, so a stray repository variable can never bake a private feed's token into a
# public artifact. KAI_UPDATE_PUBLIC_FEED marks the build public: its updater never sends the
# compiled-in feed a token, not even a user's KAI_REGISTRY_TOKEN from the environment
# (src/update.rs). The updater follows the redirect to GitHub's asset host because the feed carries
# no token. Every release is marked latest, so the feed is always the newest one.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PUBLIC_MANIFEST_URL=https://github.com/lmgw-dev/kai/releases/latest/download/latest.json

[[ "${GITHUB_ACTIONS:-}" == "true" ]] || {
  echo "packaging/gh-build.sh runs inside GitHub Actions; build locally with \`just rpm\`" >&2
  exit 1
}
# The tag: the workflow's KAI_RELEASE_TAG (the pushed tag, or the dispatch input, whose checkout is
# that tag); a tag push without it falls back to its ref name.
tag=${KAI_RELEASE_TAG:-}
if [[ -z "$tag" && "${GITHUB_REF_TYPE:-}" == "tag" ]]; then tag=${GITHUB_REF_NAME:-}; fi
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "public builds are made from a v<X.Y.Z> tag, not '${tag}' (${GITHUB_REF:-an unknown ref})" >&2
  exit 1
fi
base=$(grep -m1 -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | grep -oE '[0-9.]+' || true)
[[ -n "$base" ]] || { echo "no X.Y.Z version in Cargo.toml" >&2; exit 1; }
version=${tag#v}
if [[ "$version" != "$base" ]]; then
  echo "tag ${tag} does not match version ${base} in Cargo.toml" >&2
  echo "(scripts/release.sh bumps the version and tags in one go)" >&2
  exit 1
fi
export KAI_VERSION="$version"

# The feed is compiled in (option_env! in src/update.rs), so it is settled before cargo runs.
export KAI_UPDATE_MANIFEST_URL="$PUBLIC_MANIFEST_URL"
export KAI_UPDATE_PUBLIC_FEED=1
unset KAI_UPDATE_DEPLOY_TOKEN

bash packaging/build-rpm.sh

# The RPM of exactly this version, never a glob over whatever else the directory holds: a manifest
# must not advertise a version its RPM is not.
rpm=$(ls -1 target/rpm/out/x86_64/"kai-${KAI_VERSION}"-*.x86_64.rpm 2>/dev/null | head -n1 || true)
if [[ -z "$rpm" || ! -f "$rpm" ]]; then
  echo "no RPM matching kai-${KAI_VERSION}-*.x86_64.rpm was built; target/rpm/out/x86_64/ holds:" >&2
  ls -la target/rpm/out/x86_64/ >&2 || true
  exit 1
fi
name=$(basename "$rpm")
sha=$(sha256sum "$rpm" | cut -d' ' -f1)

dist=target/rpm/dist
rm -rf "$dist"
mkdir -p "$dist"
cp "$rpm" "$dist/$name"
commit=$(git rev-parse HEAD 2>/dev/null || echo unknown)
cat > "$dist/latest.json" <<JSON
{
  "version": "${KAI_VERSION}",
  "notes": "Automated build of ${commit:0:8} on ${tag}.",
  "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "rpm": {
    "file": "${name}",
    "url": "https://github.com/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is unset}/releases/download/v${KAI_VERSION}/${name}",
    "sha256": "${sha}"
  }
}
JSON

echo "built ${name} (sha256 ${sha}); update feed baked in: ${KAI_UPDATE_MANIFEST_URL}"
cat "$dist/latest.json"
