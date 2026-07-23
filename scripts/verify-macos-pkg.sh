#!/usr/bin/env bash
# Verify the macOS release pkg without installing it.
set -euo pipefail

PKG_PATH="${1:-target/keytao-macos-pkg/KeyTao.pkg}"

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
    if ! printf '%s\n' "$payload" | grep -Fxq "$path"; then
        echo "ERROR: pkg payload is missing $path" >&2
        exit 1
    fi
}

plist_value() {
    local plist="$1"
    local key="$2"
    plutil -extract "$key" raw -o - "$plist"
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
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/MacOS/keytao-app"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/Resources/rime-data/default.yaml"
payload_contains "$PAYLOAD_FILES" "./Applications/KeyTao.app/Contents/Frameworks/rime-plugins/librime-lua.dylib"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Resources/default-theme.yaml"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Resources/rime-data/default.yaml"
payload_contains "$PAYLOAD_FILES" "./Library/Input Methods/KeyTao.app/Contents/Frameworks/rime-plugins/librime-lua.dylib"

if printf '%s\n' "$PAYLOAD_FILES" | grep -E '(^|/)\._|/\.__' >/dev/null; then
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
require_file "$MAIN_APP/Contents/MacOS/keytao-app"
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

MAIN_ARCHS="$(lipo -archs "$MAIN_APP/Contents/MacOS/keytao-app")"
IME_ARCHS="$(lipo -archs "$IME_APP/Contents/MacOS/KeyTaoIME")"
FFI_ARCHS="$(lipo -archs "$IME_APP/Contents/Frameworks/libkeytao_core_ffi.dylib")"
MAIN_RIME_DYLIB="$(find "$MAIN_APP/Contents/Frameworks" -maxdepth 1 -type f -name 'librime*.dylib' -print -quit)"
IME_RIME_DYLIB="$(find "$IME_APP/Contents/Frameworks" -maxdepth 1 -type f -name 'librime*.dylib' -print -quit)"
MAIN_RIME_ARCHS="$(lipo -archs "$MAIN_RIME_DYLIB")"
IME_RIME_ARCHS="$(lipo -archs "$IME_RIME_DYLIB")"

echo "Main app archs: $MAIN_ARCHS"
echo "IME app archs: $IME_ARCHS"
echo "Core FFI archs: $FFI_ARCHS"
echo "Main librime archs: $MAIN_RIME_ARCHS"
echo "IME librime archs: $IME_RIME_ARCHS"

if [ "$MAIN_ARCHS" != "$IME_ARCHS" ] || [ "$MAIN_ARCHS" != "$FFI_ARCHS" ]; then
    echo "ERROR: main app, IME app, and FFI dylib must use the same release arch set" >&2
    exit 1
fi
for arch in $MAIN_ARCHS; do
    if ! printf '%s\n' "$MAIN_RIME_ARCHS" | grep -Eq "(^| )$arch( |$)"; then
        echo "ERROR: main app librime does not contain $arch" >&2
        exit 1
    fi
    if ! printf '%s\n' "$IME_RIME_ARCHS" | grep -Eq "(^| )$arch( |$)"; then
        echo "ERROR: IME librime does not contain $arch" >&2
        exit 1
    fi
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
