# source me: spike-local pkg-config shim for gtk-layer-shell
export PKG_CONFIG_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/shim/pkgconfig:${PKG_CONFIG_PATH:-}"
export CARGO_BUILD_JOBS=8
