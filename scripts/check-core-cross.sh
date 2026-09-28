#!/usr/bin/env bash
# Type-check keytao-app-core and keytao_app_bridge for Linux and Windows from macOS.
# zig (via cargo-zigbuild) supplies the C toolchain that ring and librime-sys build scripts need;
# the Rime C API headers are platform independent, so bindgen can read the vendored ones.
# windows-gnu stands in for windows-msvc: same Rust cfg(windows) code, no MSVC SDK required.
set -euo pipefail
cd "$(dirname "$0")/.."

check() {
  local target="$1" headers="$2"
  echo "== $target"
  RIME_INCLUDE_DIR="$headers" RIME_LIB_DIR=/nonexistent BINDGEN_EXTRA_CLANG_ARGS="-I$headers" \
    nix shell nixpkgs#zig nixpkgs#cargo-zigbuild -c cargo-zigbuild check -p keytao-app-core --target "$target"
  RIME_INCLUDE_DIR="$headers" RIME_LIB_DIR=/nonexistent BINDGEN_EXTRA_CLANG_ARGS="-I$headers" \
    nix shell nixpkgs#zig nixpkgs#cargo-zigbuild -c cargo-zigbuild check -p keytao_app_bridge --all-targets --target "$target"
}

check x86_64-unknown-linux-gnu "$PWD/vendor/librime/macos-universal/include"
check x86_64-pc-windows-gnu "$PWD/vendor/librime/windows-x64/include"
