#!/usr/bin/env bash
# Installs wasm-bindgen (the CLI) and wasm-opt (binaryen) for trunk from upstream's prebuilt
# releases, pinned and hash-checked like install-trunk.sh, so trunk never downloads them itself
# (the build runs trunk with --offline). Usage: install-wasm-tools.sh <bin-dir>  (as root in the
# build image, or into the CI job's cargo bin). Idempotent.
# The wasm-bindgen version must equal `wasm-bindgen` in ui/Cargo.lock: check-lock-pin below fails
# otherwise (the CLI and the crate must match exactly). wasm-opt's must be the version trunk
# expects (trunk 0.21.14: version_123), or trunk would look for a download.
set -euo pipefail
dest=${1:?usage: install-wasm-tools.sh <bin-dir>}
bindgen_version=0.2.123
bindgen_sha256=80fc5c1d5c128fd67e99b14318eeabf79dfbadf459ece2d38ba93a82e55730c6
binaryen_version=version_123
binaryen_sha256=e959f2170af4c20c552e9de3a0253704d6a9d2766e8fdb88e4d6ac4bae9388fe

here=$(dirname "${BASH_SOURCE[0]}")
lock=$here/../ui/Cargo.lock
if [[ -f "$lock" ]]; then
  locked=$(grep -A1 '^name = "wasm-bindgen"$' "$lock" | sed -nE 's/^version = "([^"]+)"/\1/p')
  [[ "$locked" == "$bindgen_version" ]] || {
    echo "ui/Cargo.lock has wasm-bindgen $locked, this script installs $bindgen_version: update both pins" >&2
    exit 1
  }
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fetch() { # url sha256 name
  curl -fsSL "$1" -o "$tmp/$3.tar.gz"
  echo "$2  $tmp/$3.tar.gz" | sha256sum -c -
}

if [[ "$("$dest/wasm-bindgen" --version 2>/dev/null || true)" != "wasm-bindgen $bindgen_version"* ]]; then
  fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$bindgen_version/wasm-bindgen-$bindgen_version-x86_64-unknown-linux-musl.tar.gz" \
    "$bindgen_sha256" bindgen
  tar -xzf "$tmp/bindgen.tar.gz" -C "$tmp" --strip-components=1 "wasm-bindgen-$bindgen_version-x86_64-unknown-linux-musl/wasm-bindgen"
  install -D -m 0755 "$tmp/wasm-bindgen" "$dest/wasm-bindgen"
fi
if [[ "$("$dest/wasm-opt" --version 2>/dev/null || true)" != *"${binaryen_version#version_}"* ]]; then
  fetch "https://github.com/WebAssembly/binaryen/releases/download/$binaryen_version/binaryen-$binaryen_version-x86_64-linux.tar.gz" \
    "$binaryen_sha256" binaryen
  tar -xzf "$tmp/binaryen.tar.gz" -C "$tmp" --strip-components=2 "binaryen-$binaryen_version/bin/wasm-opt"
  install -D -m 0755 "$tmp/wasm-opt" "$dest/wasm-opt"
fi
"$dest/wasm-bindgen" --version
"$dest/wasm-opt" --version
