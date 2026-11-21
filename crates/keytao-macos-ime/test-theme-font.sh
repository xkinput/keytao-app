#!/usr/bin/env bash
set -euo pipefail

IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

SWIFTC="swiftc"
if command -v xcrun >/dev/null 2>&1; then
    SWIFTC="xcrun swiftc"
fi

# shellcheck disable=SC2086
$SWIFTC \
    -framework Cocoa \
    -o "$BUILD_DIR/theme-font-tests" \
    "$IME_DIR/Sources/KeyTaoIME/ThemeFont.swift" \
    "$IME_DIR/Tests/ThemeFontTests/main.swift"

"$BUILD_DIR/theme-font-tests"
