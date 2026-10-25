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
    -o "$BUILD_DIR/input-context-tests" \
    "$IME_DIR/Sources/KeyTaoIME/InputContextGeneration.swift" \
    "$IME_DIR/Tests/InputContextTests/main.swift"

"$BUILD_DIR/input-context-tests"
