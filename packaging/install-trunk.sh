#!/usr/bin/env bash
# Installs trunk (the build tool of the configuration window's page, ui/) from upstream's prebuilt
# release, pinned and hash-checked, as lmgw's CI does: from source it pulls libdeflate-sys, whose
# vendored C Fedora's current GCC refuses. Usage: install-trunk.sh <bin-dir>  (as root in the
# build image, or into the CI job's cargo bin). Idempotent: a trunk of the pinned version stays.
# wasm-bindgen and wasm-opt come from install-wasm-tools.sh; the build runs trunk --offline --locked.
set -euo pipefail
dest=${1:?usage: install-trunk.sh <bin-dir>}
version=0.21.14
sha256=f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b
if [[ "$("$dest/trunk" --version 2>/dev/null || true)" == "trunk $version" ]]; then
  echo "trunk $version is in $dest"
  exit 0
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v$version/trunk-x86_64-unknown-linux-gnu.tar.gz" \
  -o "$tmp/trunk.tar.gz"
echo "$sha256  $tmp/trunk.tar.gz" | sha256sum -c -
tar -xzf "$tmp/trunk.tar.gz" -C "$tmp" trunk
install -D -m 0755 "$tmp/trunk" "$dest/trunk"
"$dest/trunk" --version
