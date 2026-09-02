#!/usr/bin/env bash
set -euo pipefail

IOS_IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

SWIFTC="swiftc"
if command -v xcrun >/dev/null 2>&1; then
    SWIFTC="xcrun swiftc"
fi

# shellcheck disable=SC2086
$SWIFTC \
    -o "$BUILD_DIR/touch-rollover-tests" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoTouchRolloverStore.swift" \
    "$IOS_IME_DIR/Tests/TouchRolloverTests/main.swift"

"$BUILD_DIR/touch-rollover-tests"
