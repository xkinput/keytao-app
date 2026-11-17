#!/usr/bin/env bash
set -euo pipefail

IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

xcrun swiftc \
    -module-cache-path "$BUILD_DIR/module-cache" \
    -o "$BUILD_DIR/chord-key-policy-tests" \
    "$IME_DIR/Sources/KeyTaoIME/ChordKeyPolicy.swift" \
    "$IME_DIR/Tests/ChordKeyPolicyTests/main.swift"

"$BUILD_DIR/chord-key-policy-tests"
