#!/usr/bin/env bash
# Build the complete KeyTao iOS package through one reusable entrypoint.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGETS=(aarch64-apple-ios)
PROFILE="release"
BUILD_LIBRIME=true
BUILD_APP=true
TAURI_ARGS=()

usage() {
    cat <<EOF
Usage: $0 [options] [-- tauri-ios-build-args...]

Options:
  --target TARGET       Build one Rust/iOS runtime target before packaging.
  --all-runtimes        Build all supported iOS librime and FFI runtimes.
  --debug               Build Rust FFI in debug profile.
  --release             Build Rust FFI in release profile. This is the default.
  --skip-librime        Reuse the already imported vendor/librime/ios runtime.
  --skip-app            Build runtimes and FFI only; skip tauri ios build.
  -h, --help            Show this help.

Examples:
  pnpm build:ios
  pnpm build:ios -- --no-sign --ci
  scripts/build-ios.sh --target aarch64-apple-ios-sim --skip-app
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

note() {
    echo "==> $*"
}

target_to_swift() {
    case "$1" in
        aarch64-apple-ios) echo "arm64-apple-ios15.0" ;;
        aarch64-apple-ios-sim) echo "arm64-apple-ios15.0-simulator" ;;
        x86_64-apple-ios) echo "x86_64-apple-ios15.0-simulator" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

target_to_sdk() {
    case "$1" in
        aarch64-apple-ios) echo "iphoneos" ;;
        aarch64-apple-ios-sim|x86_64-apple-ios) echo "iphonesimulator" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

sign_simulator_apps() {
    local build_root="$PROJECT_DIR/src-tauri/gen/apple/build"
    local app_entitlements="$PROJECT_DIR/src-tauri/gen/apple/KeyTaoAppSimulator.generated.entitlements"
    local keyboard_entitlements="$PROJECT_DIR/src-tauri/gen/apple/KeyTaoKeyboard/KeyTaoKeyboardSimulator.entitlements"

    [ -d "$build_root" ] || return 0
    [ -f "$app_entitlements" ] || return 0

    while IFS= read -r app; do
        [ -f "$app/Info.plist" ] || continue
        platform="$(plutil -extract CFBundleSupportedPlatforms.0 raw -o - "$app/Info.plist" 2>/dev/null || true)"
        [ "$platform" = "iPhoneSimulator" ] || continue

        note "Signing simulator app with App Group entitlements: $app"
        appex="$app/PlugIns/KeyTaoKeyboard.appex"
        if [ -d "$appex" ] && [ -f "$keyboard_entitlements" ]; then
            /usr/bin/codesign --force --sign - \
                --entitlements "$keyboard_entitlements" \
                --timestamp=none \
                --generate-entitlement-der \
                "$appex"
        fi
        /usr/bin/codesign --force --sign - \
            --entitlements "$app_entitlements" \
            --timestamp=none \
            --generate-entitlement-der \
            "$app"
        /usr/bin/codesign --verify --deep --strict "$app"
    done < <(find "$build_root" -type d -name "KeyTao.app" -print)
}

clean_tauri_ios_outputs() {
    local build_root="$PROJECT_DIR/src-tauri/gen/apple/build"
    [ -d "$build_root" ] || return 0

    # Tauri's iOS bundler can fail on repeated builds when these output
    # directories already exist, so keep the one-command path idempotent.
    rm -rf \
        "$build_root/keytao-app_iOS.xcarchive" \
        "$build_root/arm64-sim/KeyTao.app" \
        "$build_root/x86_64-sim/KeyTao.app" \
        "$build_root/arm64/KeyTao.app"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGETS=("${2:-}")
            [ -n "${TARGETS[0]}" ] || die "--target requires a value"
            shift 2
            ;;
        --all-runtimes)
            TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
            shift
            ;;
        --debug)
            PROFILE="debug"
            shift
            ;;
        --release)
            PROFILE="release"
            shift
            ;;
        --skip-librime)
            BUILD_LIBRIME=false
            shift
            ;;
        --skip-app)
            BUILD_APP=false
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            TAURI_ARGS+=("$@")
            break
            ;;
        *)
            TAURI_ARGS+=("$1")
            shift
            ;;
    esac
done

cd "$PROJECT_DIR"

if [ "$BUILD_LIBRIME" = true ]; then
    for target in "${TARGETS[@]}"; do
        note "Building iOS librime runtime for $target"
        scripts/build-ios-librime.sh --target "$target"
    done
fi

for target in "${TARGETS[@]}"; do
    note "Building iOS FFI runtime for $target"
    if [ "$PROFILE" = "debug" ]; then
        scripts/build-ios-ffi.sh --target "$target" --debug
    else
        scripts/build-ios-ffi.sh --target "$target" --release
    fi
done

primary_target="${TARGETS[0]}"
note "Verifying iOS keyboard sources against $primary_target"
KEYTAO_IOS_SDK="$(target_to_sdk "$primary_target")" \
KEYTAO_IOS_RUST_TARGET="$primary_target" \
KEYTAO_IOS_SWIFT_TARGET="$(target_to_swift "$primary_target")" \
    scripts/verify-ios-ime.sh

note "Patching generated iOS Xcode project"
if [ ! -f "$PROJECT_DIR/src-tauri/gen/apple/project.yml" ]; then
    PATH="$PROJECT_DIR/.cache/bin:$PATH" pnpm tauri ios init --ci --skip-targets-install
fi
PATH="$PROJECT_DIR/.cache/bin:$PATH" scripts/setup-ios-ime-xcode.rb

if [ "$BUILD_APP" = true ]; then
    note "Building Tauri iOS package"
    clean_tauri_ios_outputs
    pnpm tauri ios build "${TAURI_ARGS[@]}"
    sign_simulator_apps
fi
