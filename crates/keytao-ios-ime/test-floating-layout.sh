#!/usr/bin/env bash
# Builds and runs Tests/FloatingLayoutTests on the host toolchain.
#
# The iOS package as a whole needs UIKit and the keytao-core-ffi archive, so it
# cannot be unit tested on macOS. KeyTaoIOSFloatingLayout.swift is deliberately
# kept free of both, which lets the floating-scale decode rules — the part that
# has to stay in step with keytao-theme::mobile_layout — run as a real test.
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
    -o "$BUILD_DIR/floating-layout-tests" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSFloatingLayout.swift" \
    "$IOS_IME_DIR/Tests/FloatingLayoutTests/main.swift"

"$BUILD_DIR/floating-layout-tests"
