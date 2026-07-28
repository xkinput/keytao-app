#!/usr/bin/env bash
# smoke.sh - runs Smoke/main.swift against a real librime and a real deployed
# user directory, checking the keytao-core-ffi contract the macOS frontend
# depends on (state JSON shape, candidate panel model, key policy, offsets).
#
# Manual tool, not part of any build or CI path. It composes and commits text,
# so point it at a scratch copy of the user directory rather than the one you
# type with:
#
#   rsync -a --exclude log ~/Library/keytao/ /tmp/keytao-smoke-user/
#   ./build.sh --release --skip-pkg
#   ./smoke.sh /tmp/keytao-smoke-user
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="${KEYTAO_MACOS_BUILD_DIR:-$SCRIPT_DIR/build}"
APP="$BUILD_DIR/KeyTao.app"
FRAMEWORKS="$APP/Contents/Frameworks"
HEADER_DIR="$SCRIPT_DIR/Sources/CKeytaoCore"

USER_DATA_DIR="${1:-}"
if [ -z "$USER_DATA_DIR" ]; then
    echo "usage: $0 <user_data_dir> [shared_data_dir]" >&2
    exit 2
fi

find_shared_data_dir() {
    for dir in \
        "${2:-}" \
        "${KEYTAO_RIME_SHARED_DATA_DIR:-}" \
        "$WORKSPACE_DIR/vendor/librime/macos-universal/rime-data" \
        "/Library/Input Methods/KeyTao.app/Contents/Resources/rime-data" \
        "/Applications/KeyTao.app/Contents/Resources/rime-data"; do
        [ -n "$dir" ] || continue
        if [ -f "$dir/default.yaml" ]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

SHARED_DATA_DIR="${2:-$(find_shared_data_dir "$@" || true)}"
if [ -z "$SHARED_DATA_DIR" ]; then
    echo "ERROR: no shared rime-data directory found; pass one as the second argument." >&2
    exit 1
fi

if [ ! -f "$FRAMEWORKS/libkeytao_core_ffi.dylib" ] || [ ! -f "$HEADER_DIR/keytao_core.h" ]; then
    echo "ERROR: build the IME bundle first: $SCRIPT_DIR/build.sh --release --skip-pkg" >&2
    exit 1
fi

OUT_DIR="$BUILD_DIR/smoke"
mkdir -p "$OUT_DIR"

echo "==> Building macOS FFI smoke checks..."
swiftc \
    "$SCRIPT_DIR/Smoke/main.swift" \
    "$SCRIPT_DIR/Sources/KeyTaoIME/ImeState.swift" \
    "$SCRIPT_DIR/Sources/KeyTaoIME/ImeTheme.swift" \
    -module-name KeyTaoSmoke \
    -disable-bridging-pch \
    -framework Cocoa \
    -I "$HEADER_DIR" \
    -L "$FRAMEWORKS" -lkeytao_core_ffi \
    -Xlinker -rpath -Xlinker "$FRAMEWORKS" \
    -o "$OUT_DIR/keytao-macos-smoke"

echo "==> user=$USER_DATA_DIR shared=$SHARED_DATA_DIR"
DYLD_FALLBACK_LIBRARY_PATH="$FRAMEWORKS:${DYLD_FALLBACK_LIBRARY_PATH:-}" \
    "$OUT_DIR/keytao-macos-smoke" "$USER_DATA_DIR" "$SHARED_DATA_DIR"
