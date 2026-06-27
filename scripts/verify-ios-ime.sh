#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_IME_DIR="$PROJECT_DIR/crates/keytao-ios-ime"
HEADER="$PROJECT_DIR/crates/keytao-core-ffi/include/keytao_core.h"
IOS_INFO_PLIST="$PROJECT_DIR/src-tauri/Info.ios.plist"
KEYBOARD_INFO_PLIST="$IOS_IME_DIR/Resources/Info.plist"
APP_ENTITLEMENTS="$IOS_IME_DIR/Resources/KeyTaoApp.entitlements"
KEYBOARD_ENTITLEMENTS="$IOS_IME_DIR/Resources/KeyTaoKeyboard.entitlements"
SDK="${KEYTAO_IOS_SDK:-iphonesimulator}"
HOST_ARCH="$(uname -m)"
if [ "$HOST_ARCH" = "arm64" ]; then
    DEFAULT_SWIFT_TARGET="arm64-apple-ios15.0-simulator"
    DEFAULT_RUST_TARGET="aarch64-apple-ios-sim"
else
    DEFAULT_SWIFT_TARGET="x86_64-apple-ios15.0-simulator"
    DEFAULT_RUST_TARGET="x86_64-apple-ios"
fi
TARGET="${KEYTAO_IOS_SWIFT_TARGET:-$DEFAULT_SWIFT_TARGET}"
RUST_TARGET="${KEYTAO_IOS_RUST_TARGET:-$DEFAULT_RUST_TARGET}"

if ! command -v xcrun >/dev/null 2>&1; then
    echo "ERROR: xcrun is required for iOS Swift typecheck." >&2
    exit 1
fi

if [ ! -f "$HEADER" ]; then
    echo "ERROR: missing FFI header: $HEADER" >&2
    echo "Run: cargo check -p keytao-core-ffi" >&2
    exit 1
fi

SDKROOT="$(xcrun --sdk "$SDK" --show-sdk-path)"

if command -v plutil >/dev/null 2>&1; then
    plutil -lint "$IOS_INFO_PLIST" "$KEYBOARD_INFO_PLIST" "$APP_ENTITLEMENTS" "$KEYBOARD_ENTITLEMENTS" >/dev/null
fi

if command -v ruby >/dev/null 2>&1; then
    ruby -c "$PROJECT_DIR/scripts/setup-ios-ime-xcode.rb" >/dev/null
fi

node -e "const fs=require('fs'); const c=JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json','utf8')); if(!c.bundle || !c.bundle.iOS || c.bundle.iOS.infoPlist !== 'Info.ios.plist') { throw new Error('missing bundle.iOS.infoPlist'); }" >/dev/null

echo "Checking Swift iOS keyboard sources"
(
    cd "$IOS_IME_DIR"
    xcrun --sdk "$SDK" swiftc \
        -typecheck \
        -target "$TARGET" \
        -sdk "$SDKROOT" \
        -I Sources/CKeytaoCore \
        Sources/KeyTaoIOSIME/*.swift
)

runtime_root="${KEYTAO_IOS_RIME_ROOT:-$PROJECT_DIR/vendor/librime/ios}"
case "$RUST_TARGET" in
    aarch64-apple-ios) runtime_name="iphoneos-arm64" ;;
    aarch64-apple-ios-sim) runtime_name="iphonesimulator-arm64" ;;
    x86_64-apple-ios) runtime_name="iphonesimulator-x86_64" ;;
    *) runtime_name="$RUST_TARGET" ;;
esac

candidate="$runtime_root"
if [ -d "$runtime_root/$runtime_name" ]; then
    candidate="$runtime_root/$runtime_name"
elif [ -d "$runtime_root/$RUST_TARGET" ]; then
    candidate="$runtime_root/$RUST_TARGET"
fi

archive_has_lua_module() {
    local archive="$1"
    if command -v nm >/dev/null 2>&1; then
        local symbols
        symbols="$(nm "$archive" 2>/dev/null || true)"
        if grep -Eq 'rime_lua_initialize|rime_lua_finalize|LuaProcessor|LuaTranslator' <<<"$symbols"; then
            return 0
        fi
    fi
    if command -v strings >/dev/null 2>&1; then
        local raw_strings
        raw_strings="$(strings "$archive" 2>/dev/null || true)"
        if grep -q 'lua_processor' <<<"$raw_strings" &&
            grep -q 'lua_translator' <<<"$raw_strings"; then
            return 0
        fi
    fi
    return 1
}

if [ -f "$candidate/include/rime_api.h" ] && [ -f "$candidate/lib/librime.a" ]; then
    if ! archive_has_lua_module "$candidate/lib/librime.a"; then
        echo "ERROR: iOS librime runtime is missing the merged librime-lua module: $candidate/lib/librime.a" >&2
        echo "Rebuild it with: scripts/build-ios-librime.sh --target $RUST_TARGET" >&2
        exit 1
    fi
    echo "Checking Rust iOS FFI target with librime runtime: $candidate"
    KEYTAO_IOS_RIME_ROOT="$candidate" \
    SDKROOT="$SDKROOT" \
    cargo check -p keytao-core-ffi --target "$RUST_TARGET"
else
    echo "Skipping Rust iOS link check: no iOS librime runtime at $candidate"
    echo "Expected include/rime_api.h and lib/librime.a."
    echo "Import one with: scripts/ios-librime-runtime.sh import-sdk --target $RUST_TARGET --source /path/to/ios-librime-sdk"
fi

echo "iOS IME source checks passed"
