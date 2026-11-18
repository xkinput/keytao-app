#!/usr/bin/env bash
set -euo pipefail

IOS_IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "$BUILD_DIR"' EXIT

SWIFTC="swiftc"
if command -v xcrun >/dev/null 2>&1; then
    SWIFTC="xcrun swiftc"
fi

# Compile the actual Foundation state models without the UIKit clipboard wrapper.
{
    printf 'import Foundation\n'
    sed -n '/^public struct KeyTaoCandidate/,$p' "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSState.swift"
} > "$BUILD_DIR/KeyTaoIOSState.swift"

# Exercise the controller's actual context/readiness gate with lightweight stubs.
{
    printf 'extension ChordGateHarness {\n'
    sed -n '/^    func keyboardViewWillBeginChord(/,/^    }/p' \
        "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoKeyboardViewController.swift"
    printf '}\n'
} > "$BUILD_DIR/ChordGate.swift"

# Exercise the real reset and touch-entry methods without a UIKit host.
{
    printf 'import Foundation\n#if canImport(CoreGraphics)\nimport CoreGraphics\n#endif\n'
    sed -n '/^public enum KeyTaoCommandType {/,/^public struct KeyTaoKeyStackItem:/p' \
        "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSConfig.swift" | sed '$d'
    printf 'extension KeyTaoIOSKeyboardView {\n'
    for method in 'func resetNavigation(' 'func clearOneShotShift(' 'private func applyShift(' \
        'private func beginKeyTouch(' 'private func cancelActiveTouch(' 'func releaseChordTouches('; do
        sed -n "/^    $method/,/^    }/p" \
            "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSKeyboardView.swift" | sed 's/^    private func /    func /'
    done
    printf 'func navigate(to destination: TouchNavigationDestination) {\n'
    printf 'let action: KeyTaoNavigationAction; switch destination { case .reset: action = .reset(.letters) }\n'
    sed -n '/^        switch action {/,/^        if case .reset = action {/p' \
        "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIOSKeyboardView.swift" | sed '$d'
    printf '}\n}\n'
    printf 'extension ContextResetHarness {\n'
    sed -n '/^    private func resetInputContextCaches(/,/^    }/p' \
        "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoKeyboardViewController.swift" | sed 's/^    private func /    func /'
    printf '}\n'
} > "$BUILD_DIR/TouchEntry.swift"

# shellcheck disable=SC2086
$SWIFTC \
    -module-cache-path "$BUILD_DIR/module-cache" \
    -o "$BUILD_DIR/touch-rollover-tests" \
    "$BUILD_DIR/KeyTaoIOSState.swift" \
    "$BUILD_DIR/ChordGate.swift" \
    "$BUILD_DIR/TouchEntry.swift" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoIMEInteractionPolicy.swift" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/KeyTaoTouchRolloverStore.swift" \
    "$IOS_IME_DIR/Tests/TouchRolloverTests/main.swift"

"$BUILD_DIR/touch-rollover-tests"
