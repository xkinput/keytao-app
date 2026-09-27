#!/usr/bin/env bash
# Type-check the Tauri shell (keytao-app) for Windows from macOS, so cfg(windows)
# code in src-tauri is compiled at least once before it reaches a Windows machine.
# zig supplies the C toolchain, the NDK's llvm-windres stands in for mingw windres,
# and empty placeholders satisfy the Windows resource paths in tauri.windows.conf.json.
set -euo pipefail
cd "$(dirname "$0")/.."

ndk_bin="${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/27.0.12077973/toolchains/llvm/prebuilt/darwin-x86_64/bin"
shims="$PWD/target/cross-shims"
mkdir -p "$shims" target/keytao-windows-ime-runtime/{current,arm64x,x86} target/keytao-windows-app-runtime
ln -sf "$ndk_bin/llvm-windres" "$shims/x86_64-w64-mingw32-windres"
touch target/keytao-windows-app-runtime/placeholder.dll

headers="${RIME_WINDOWS_HEADERS:-$PWD/vendor/librime/windows-x64/include}"
PATH="$shims:$PATH" RIME_INCLUDE_DIR="$headers" RIME_LIB_DIR=/nonexistent BINDGEN_EXTRA_CLANG_ARGS="-I$headers" \
  nix shell nixpkgs#zig nixpkgs#cargo-zigbuild -c cargo-zigbuild check -p keytao-app --target x86_64-pc-windows-gnu
