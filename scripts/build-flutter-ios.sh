#!/usr/bin/env bash
# Build the Flutter containing app and keyboard, then package an unsigned device IPA.
set -euo pipefail
export COPYFILE_DISABLE=1

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLUTTER_BIN="${FLUTTER_BIN:-flutter}"
OUTPUT_DIR="$PROJECT_DIR/flutter_app/build/ios/ipa"
PUBSPEC_VERSION="$(awk '/^version:/ {print $2; exit}' "$PROJECT_DIR/flutter_app/pubspec.yaml")"
VERSION="${PUBSPEC_VERSION%%+*}"
BUILD_NUMBER="${PUBSPEC_VERSION##*+}"
BUILD_FFI=true

usage() {
    cat <<EOF
Usage: $0 [--version VERSION] [--build-number NUMBER] [--output-dir DIR] [--skip-ffi]

Builds iOS release with --no-codesign and writes:
  keytao-app-<version>-ios-arm64-unsigned.ipa

Requires Flutter, Xcode, the aarch64-apple-ios Rust target and an imported
vendor/librime/ios/iphoneos-arm64 runtime. No librime download is performed.
Set FLUTTER_BIN to an SDK's flutter executable if it is not on PATH.
--skip-ffi reuses target/keytao-ios-runtime/iphoneos-arm64 prepared earlier.
EOF
}

die() { echo "ERROR: $*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
        --build-number) BUILD_NUMBER="${2:?--build-number requires a value}"; shift 2 ;;
        --output-dir) OUTPUT_DIR="${2:?--output-dir requires a value}"; shift 2 ;;
        --skip-ffi) BUILD_FFI=false; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] || die "invalid version: $VERSION"
[[ "$BUILD_NUMBER" =~ ^[0-9]+$ ]] || die "build number must be numeric"
command -v "$FLUTTER_BIN" >/dev/null || die "Flutter not found: $FLUTTER_BIN"

if [ "$BUILD_FFI" = true ]; then
    "$PROJECT_DIR/scripts/build-ios-ffi.sh" --target aarch64-apple-ios --release
fi
test -f "$PROJECT_DIR/target/keytao-ios-runtime/iphoneos-arm64/lib/libkeytao_core_ffi.a" || \
    die "missing staged keyboard FFI; run scripts/build-ios-ffi.sh --target aarch64-apple-ios --release"

(
    cd "$PROJECT_DIR/flutter_app"
    "$FLUTTER_BIN" build ios --release --no-codesign \
        --build-name "${VERSION%%-*}" --build-number "$BUILD_NUMBER" \
        "--dart-define=KEYTAO_VERSION=$VERSION"
)

APP="$PROJECT_DIR/flutter_app/build/ios/iphoneos/KeyTao.app"
KEYBOARD="$APP/PlugIns/KeyTaoKeyboard.appex"
for required in \
    "$APP/KeyTao" "$APP/Info.plist" "$KEYBOARD/KeyTaoKeyboard" "$KEYBOARD/Info.plist" \
    "$APP/rime-data/default.yaml" "$KEYBOARD/rime-data/default.yaml" \
    "$APP/addon-schemas/easy_en/easy_en.schema.yaml"; do
    test -f "$required" || die "missing IPA input: $required"
done
test -x "$APP/KeyTao" && test -x "$KEYBOARD/KeyTaoKeyboard" || die "missing executable permissions"
plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Info.plist"; }
[ "$(plist_value "$APP" CFBundleIdentifier)" = "ink.rea.keytao-app" ] || die "wrong app identifier"
[ "$(plist_value "$KEYBOARD" CFBundleIdentifier)" = "ink.rea.keytao-app.keyboard" ] || die "wrong keyboard identifier"
for bundle in "$APP" "$KEYBOARD"; do
    [ "$(plist_value "$bundle" CFBundleShortVersionString)" = "${VERSION%%-*}" ] || die "wrong bundle version"
    [ "$(plist_value "$bundle" CFBundleVersion)" = "$BUILD_NUMBER" ] || die "wrong build number"
    if codesign -d "$bundle" >/dev/null 2>&1; then
        die "expected unsigned bundle: $bundle"
    fi
done

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/keytao-flutter-ios.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir -p "$WORK_DIR/Payload"
ditto "$APP" "$WORK_DIR/Payload/KeyTao.app"
IPA_NAME="keytao-app-${VERSION}-ios-arm64-unsigned.ipa"
(
    cd "$WORK_DIR"
    zip -qry "$IPA_NAME" Payload
)
unzip -t "$WORK_DIR/$IPA_NAME" > "$WORK_DIR/zip-check.txt"
unzip -Z1 "$WORK_DIR/$IPA_NAME" > "$WORK_DIR/manifest.txt"
for entry in \
    Payload/KeyTao.app/KeyTao \
    Payload/KeyTao.app/PlugIns/KeyTaoKeyboard.appex/KeyTaoKeyboard; do
    grep -Fx "$entry" "$WORK_DIR/manifest.txt" >/dev/null || die "missing IPA entry: $entry"
done
mv "$WORK_DIR/$IPA_NAME" "$OUTPUT_DIR/$IPA_NAME"
echo "Created unsigned IPA: $OUTPUT_DIR/$IPA_NAME"
