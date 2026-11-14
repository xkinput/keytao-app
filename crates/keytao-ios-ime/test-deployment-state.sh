#!/usr/bin/env bash
set -euo pipefail

IOS_IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

xcrun swiftc -module-cache-path "$BUILD_DIR/module-cache" \
    -o "$BUILD_DIR/deployment-state-tests" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSDeploymentState.swift" \
    "$IOS_IME_DIR/Tests/DeploymentStateTests/main.swift"
"$BUILD_DIR/deployment-state-tests"
