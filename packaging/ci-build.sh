#!/usr/bin/env bash
# Kai's private test feed, built in GitLab CI (.gitlab-ci.yml's `rpm` job): the RPM of this push,
# versioned <Cargo.toml version>+<pipeline IID> (lmgw's convention: 0.1.0 < 0.1.0+12 < 0.1.0+13 <
# 0.1.1, for rpm and for the updater alike), its updater pointed at this project's generic package
# registry, and the manifest it polls there (latest.json). Output in target/rpm/dist/ (the RPM,
# latest.json, build.env), which packaging/ci-publish.sh uploads.
# Usage: bash packaging/ci-build.sh   (inside the GitLab job only; `just rpm` builds locally, with no
#        feed)
# KAI_UPDATE_DEPLOY_TOKEN (a masked CI/CD variable, the header comment of .gitlab-ci.yml) is
# compiled in with the feed; without it the build still succeeds, with a warning, and cannot read
# its own feed.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
[[ "${GITLAB_CI:-}" == "true" ]] || {
  echo "packaging/ci-build.sh runs inside GitLab CI; build locally with \`just rpm\`" >&2
  exit 1
}

base=$(grep -m1 -oE '^version = "[0-9]+\.[0-9]+\.[0-9]+"' Cargo.toml | grep -oE '[0-9.]+' || true)
[[ -n "$base" ]] || { echo "no X.Y.Z version in Cargo.toml" >&2; exit 1; }
export KAI_VERSION="${base}+${CI_PIPELINE_IID:?CI_PIPELINE_IID is unset under GitLab CI}"

# The feed is compiled in (option_env! in src/update.rs), so it is settled before cargo runs. By
# the project's id, not its path: a moved or renamed project keeps its id, so installed builds keep
# finding their feed.
registry="${CI_API_V4_URL}/projects/${CI_PROJECT_ID:?CI_PROJECT_ID is unset under GitLab CI}/packages/generic/kai"
export KAI_UPDATE_MANIFEST_URL="${registry}/latest/latest.json"
if [[ -n "${KAI_UPDATE_DEPLOY_TOKEN:-}" ]]; then
  export KAI_UPDATE_DEPLOY_TOKEN
else
  echo "WARNING: KAI_UPDATE_DEPLOY_TOKEN is not set: this build cannot read its own update feed." >&2
  echo "         Define it as a masked CI/CD variable (a deploy token of this project with only" >&2
  echo "         read_package_registry); a *protected* variable only reaches protected branches." >&2
fi

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
cat > "$dist/latest.json" <<JSON
{
  "version": "${KAI_VERSION}",
  "notes": "Automated build of ${CI_COMMIT_SHORT_SHA:-unknown} on ${CI_COMMIT_BRANCH:-${CI_COMMIT_REF_NAME:-unknown}}.",
  "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "rpm": {
    "file": "${name}",
    "url": "${registry}/${KAI_VERSION}/${name}",
    "sha256": "${sha}"
  }
}
JSON
{ echo "VERSION=${KAI_VERSION}"; echo "RPM_NAME=${name}"; } > "$dist/build.env"

echo "built ${name} (sha256 ${sha})"
cat "$dist/latest.json"
