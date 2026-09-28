#!/usr/bin/env bash
# Verify the macOS release pkg without installing it.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE_VERSION="$(awk '
    /^\[workspace\.package\]/ { section = 1; next }
    /^\[/ { section = 0 }
    section && /^version[[:space:]]*=/ { gsub(/["]/, "", $3); print $3; exit }
' "$PROJECT_DIR/Cargo.toml")"
PKG_PATH="${1:-$PROJECT_DIR/target/keytao-macos-pkg/keytao-app-${WORKSPACE_VERSION}-macos.pkg}"

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $command_name" >&2
        exit 1
    fi
}

require_file() {
    local path="$1"
    if [ ! -f "$path" ]; then
        echo "ERROR: missing file: $path" >&2
        exit 1
    fi
}

require_dir() {
    local path="$1"
    if [ ! -d "$path" ]; then
        echo "ERROR: missing directory: $path" >&2
        exit 1
    fi
}

require_glob() {
    local pattern="$1"
    if ! compgen -G "$pattern" >/dev/null; then
        echo "ERROR: missing match: $pattern" >&2
        exit 1
    fi
}

payload_contains() {
    local payload="$1"
    local path="$2"
    if ! grep -Fxq -- "$path" <<<"$payload"; then
        echo "ERROR: pkg payload is missing $path" >&2
        exit 1
    fi
}

plist_value() {
    local plist="$1"
    local key="$2"
    plutil -extract "$key" raw -o - "$plist"
}

require_universal() {
    local binary="$1"
    local arch
    require_file "$binary"
    echo "Architectures ($binary): $(lipo -archs "$binary")"
    for arch in x86_64 arm64; do
        if ! lipo "$binary" -verify_arch "$arch"; then
            echo "ERROR: release binary must contain x86_64 and arm64: $binary" >&2
            exit 1
        fi
    done
}

check_external_links() {
    local bundle="$1"
    local label="$2"
    local log="$3"

    : > "$log"
    while IFS= read -r file; do
        if file "$file" | grep -q 'Mach-O'; then
            echo "==> otool -L $file" >> "$log"
            otool -L "$file" >> "$log"
        fi
    done < <(find "$bundle/Contents/MacOS" "$bundle/Contents/Frameworks" -type f -print)

    if grep -E '/opt/homebrew|/usr/local|/nix/store' "$log"; then
        echo "ERROR: $label references package-manager libraries" >&2
        exit 1
    fi
}

if [ "$(uname -s)" != "Darwin" ]; then
    echo "ERROR: macOS pkg verification must run on macOS." >&2
    exit 1
fi

require_command pkgutil
require_command plutil
require_command otool
require_command codesign
require_command cpio
require_command gzip
require_command file
require_command lipo

if [ ! -f "$PKG_PATH" ]; then
    echo "ERROR: missing pkg: $PKG_PATH" >&2
    exit 1
fi

PKG_PATH="$(cd "$(dirname "$PKG_PATH")" && pwd)/$(basename "$PKG_PATH")"
echo "==> Verifying macOS pkg: $PKG_PATH"
ls -lh "$PKG_PATH"

PAYLOAD_FILES="$(pkgutil --payload-files "$PKG_PATH")"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/MacOS/KeyTao"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/Resources/rime-data/default.yaml"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/Frameworks/rime-plugins/librime-lua.dylib"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Resources/default-theme.yaml"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Resources/rime-data/default.yaml"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Frameworks/rime-plugins/librime-lua.dylib"

if grep -E '(^|/)\._|/\.__' <<<"$PAYLOAD_FILES" >/dev/null; then
    echo "WARNING: pkg payload contains AppleDouble metadata entries." >&2
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

EXPANDED="$TMP_DIR/pkg"
ROOT="$TMP_DIR/root"
pkgutil --expand "$PKG_PATH" "$EXPANDED"
mkdir -p "$ROOT"
(cd "$ROOT" && gzip -dc "$EXPANDED/Payload" | cpio -idm --quiet)

MAIN_APP="$ROOT/Applications/KeyTao.app"
IME_APP="$ROOT/Library/Input Methods/KeyTao.app"
POSTINSTALL="$EXPANDED/Scripts/postinstall"
PACKAGE_INFO="$EXPANDED/PackageInfo"

require_dir "$MAIN_APP"
require_dir "$IME_APP"
require_file "$POSTINSTALL"
require_file "$PACKAGE_INFO"
require_file "$MAIN_APP/Contents/MacOS/KeyTao"
require_file "$MAIN_APP/Contents/Info.plist"
require_file "$MAIN_APP/Contents/Resources/rime-data/default.yaml"
require_glob "$MAIN_APP/Contents/Frameworks/librime*.dylib"
require_file "$MAIN_APP/Contents/Frameworks/rime-plugins/librime-lua.dylib"
require_file "$IME_APP/Contents/MacOS/KeyTaoIME"
require_file "$IME_APP/Contents/Info.plist"
require_file "$IME_APP/Contents/Resources/default-theme.yaml"
require_file "$IME_APP/Contents/Resources/rime-data/default.yaml"
require_glob "$IME_APP/Contents/Frameworks/librime*.dylib"
require_file "$IME_APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
require_file "$IME_APP/Contents/Frameworks/rime-plugins/librime-lua.dylib"

require_universal "$MAIN_APP/Contents/MacOS/KeyTao"
require_universal "$MAIN_APP/Contents/Frameworks/keytao_app_bridge.framework/keytao_app_bridge"
require_universal "$IME_APP/Contents/MacOS/KeyTaoIME"
require_universal "$IME_APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
for bundle in "$MAIN_APP" "$IME_APP"; do
    for dylib in "$bundle/Contents/Frameworks/"librime*.dylib; do
        require_universal "$dylib"
    done
    while IFS= read -r -d '' binary; do
        if file "$binary" | grep -q 'Mach-O'; then
            require_universal "$binary"
        fi
    done < <(find "$bundle/Contents/Frameworks" -type f -print0)
done

MAIN_BUNDLE_ID="$(plist_value "$MAIN_APP/Contents/Info.plist" CFBundleIdentifier)"
IME_BUNDLE_ID="$(plist_value "$IME_APP/Contents/Info.plist" CFBundleIdentifier)"
if [ "$MAIN_BUNDLE_ID" != "ink.rea.keytao-app" ]; then
    echo "ERROR: unexpected main app bundle id: $MAIN_BUNDLE_ID" >&2
    exit 1
fi
if [ "$IME_BUNDLE_ID" != "ink.rea.inputmethod.keytao" ]; then
    echo "ERROR: unexpected IME bundle id: $IME_BUNDLE_ID" >&2
    exit 1
fi

PKG_VERSION="$(sed -n 's/.*<pkg-info[^>]*[[:space:]]version="\([^"]*\)".*/\1/p' "$PACKAGE_INFO" | head -1)"
echo "pkg version: $PKG_VERSION"
EXPECTED_BUNDLE_VERSION="$(printf '%s' "$PKG_VERSION" | sed -E 's/[^0-9.]+/./g; s/\.+/./g; s/^\.//; s/\.$//')"
for bundle in "$MAIN_APP" "$IME_APP"; do
    APP_VERSION="$(plist_value "$bundle/Contents/Info.plist" CFBundleShortVersionString)"
    BUNDLE_VERSION="$(plist_value "$bundle/Contents/Info.plist" CFBundleVersion)"
    echo "Bundle version ($bundle): $APP_VERSION ($BUNDLE_VERSION)"
    if [ "$APP_VERSION" != "$PKG_VERSION" ] || [ "$BUNDLE_VERSION" != "$EXPECTED_BUNDLE_VERSION" ]; then
        echo "ERROR: bundle version does not match pkg version $PKG_VERSION: $bundle" >&2
        exit 1
    fi
done

# cfprefsd owns preference writes for every process on the machine; an installer
# has no business terminating it.
if grep -Fq 'killall cfprefsd' "$POSTINSTALL"; then
    echo "ERROR: postinstall must not kill the system preferences daemon" >&2
    exit 1
fi

grep -Fq '"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --register-input-source' "$POSTINSTALL"
grep -Fq '"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --disable-legacy-input-sources' "$POSTINSTALL"
grep -Fq '"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --enable-input-source' "$POSTINSTALL"
grep -Fq '"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --select-input-source' "$POSTINSTALL"
grep -Fq 'postinstall-action="logout"' "$PACKAGE_INFO"

check_external_links "$MAIN_APP" "main app" "$TMP_DIR/main-otool.txt"
check_external_links "$IME_APP" "IME app" "$TMP_DIR/ime-otool.txt"

codesign --verify --deep --strict --verbose=2 "$MAIN_APP"
codesign --verify --deep --strict --verbose=2 "$IME_APP"

echo "==> macOS pkg verification passed"
