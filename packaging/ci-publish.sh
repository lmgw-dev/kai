#!/usr/bin/env bash
# Upload what packaging/ci-build.sh built (target/rpm/dist/) to this project's GitLab generic package
# registry with the job's token: the RPM under its version, the updater's manifest under the mutable
# `latest` (.gitlab-ci.yml's `publish` job, serialized by its resource_group). `latest` never goes
# backwards: when the published latest.json names a newer version (a later pipeline published
# first), only the RPM is uploaded. The job token is sent as a header read from a pipe, never in
# curl's argv.
# Usage: bash packaging/ci-publish.sh   (inside the GitLab job only)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
dist=target/rpm/dist

# shellcheck disable=SC1091
source "$dist/build.env" # VERSION, RPM_NAME

base="${CI_API_V4_URL}/projects/${CI_PROJECT_ID}/packages/generic/kai"

# The job token's header, on a file descriptor of its own for each curl (printf is a builtin, so
# the token is in no process's argv).
token_header() { printf 'JOB-TOKEN: %s\n' "${CI_JOB_TOKEN:?CI_JOB_TOKEN is unset}"; }

upload() { # <local file> <registry url>
  echo "uploading -> $2"
  curl --fail-with-body --silent --show-error \
    --header @<(token_header) \
    --upload-file "$1" "$2"
  echo
}

# MAJOR MINOR PATCH BUILD of X.Y.Z[-pre][+BUILD], in the order the updater compares them
# (lmgw-update's is_newer): a `-pre` suffix is ignored, a missing or non-numeric part counts as 0.
version_key() {
  local v=$1 core build=0 a b c p out=()
  core=${v%%+*}
  [[ $v == *+* ]] && build=${v#*+}
  core=${core%%-*}
  IFS=. read -r a b c _ <<< "$core"
  for p in "${a:-0}" "${b:-0}" "${c:-0}" "$build"; do
    [[ $p =~ ^[0-9]+$ ]] || p=0
    out+=("$((10#$p))")
  done
  echo "${out[*]}"
}

# True when $1 is a strictly higher version than $2.
newer() {
  local x y i
  read -ra x <<< "$(version_key "$1")"
  read -ra y <<< "$(version_key "$2")"
  for i in 0 1 2 3; do
    ((x[i] > y[i])) && return 0
    ((x[i] < y[i])) && return 1
  done
  return 1
}

upload "$dist/$RPM_NAME" "${base}/${VERSION}/${RPM_NAME}"

published=$(mktemp)
trap 'rm -f "$published"' EXIT
code=$(curl --silent --show-error --output "$published" --write-out '%{http_code}' \
  --header @<(token_header) "${base}/latest/latest.json")
case "$code" in
  200)
    current=$(sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$published" | head -n1)
    if [[ -z "$current" ]]; then
      echo "the published latest.json names no version:" >&2
      cat "$published" >&2
      exit 1
    fi
    ;;
  404) current="" ;; # the first publish
  *)
    echo "reading the published latest.json failed (HTTP $code):" >&2
    cat "$published" >&2
    exit 1
    ;;
esac

if [[ -n "$current" ]] && newer "$current" "$VERSION"; then
  echo "latest.json stays at ${current}, newer than this build's ${VERSION}; only the RPM was published"
  exit 0
fi
# `latest/latest.json` is overwritten; the newest upload wins on GET. That needs the project to
# allow duplicate generic packages (GitLab's default); if it rejects them, this upload fails here,
# loudly.
upload "$dist/latest.json" "${base}/latest/latest.json"

echo "published kai ${VERSION}${current:+ (latest was ${current})}"
