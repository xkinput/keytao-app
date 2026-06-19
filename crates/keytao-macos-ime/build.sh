#!/usr/bin/env bash
# build.sh - builds KeyTao.app and, by default, an IME-only KeyTao.pkg package.
# Usage: ./build.sh [--release | --debug] [--skip-pkg]
set -euo pipefail
export COPYFILE_DISABLE=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="${KEYTAO_MACOS_BUILD_DIR:-$SCRIPT_DIR/build}"
PROFILE="release"
BUILD_PKG=1
while [ $# -gt 0 ]; do
    case "$1" in
        release|--release)
            PROFILE="release"
            shift
            ;;
        debug|--debug)
            PROFILE="debug"
            shift
            ;;
        --skip-pkg)
            BUILD_PKG=0
            shift
            ;;
        *)
            echo "Usage: $0 [--release | --debug] [--skip-pkg]" >&2
            exit 2
            ;;
    esac
done
CARGO_PROFILE="$( [[ "$PROFILE" == "release" ]] && echo "release" || echo "debug" )"
CARGO_FLAGS="$( [[ "$PROFILE" == "release" ]] && echo "--release" || echo "" )"
APP="$BUILD_DIR/KeyTao.app"
VENDOR_DIR="$WORKSPACE_DIR/vendor/librime/macos-universal"
VENDOR_ENV="$VENDOR_DIR/env.sh"

bundle_dylib_deps() {
    local binary="$1"
    local dep
    while IFS= read -r dep; do
        case "$dep" in
            ""|/usr/lib/*|/System/*|@rpath/*|@loader_path/*|@executable_path/*)
                continue
                ;;
        esac

        local base
        base="$(basename "$dep")"
        local dest="$APP/Contents/Frameworks/$base"
        if [ ! -e "$dest" ]; then
            cp "$dep" "$dest"
            chmod u+w "$dest"
            install_name_tool -id "@rpath/$base" "$dest"
            bundle_dylib_deps "$dest"
        fi
        install_name_tool -change "$dep" "@rpath/$base" "$binary"
    done < <(otool -L "$binary" | awk 'NR > 1 { print $1 }')
}

bundle_rime_plugins() {
    local plugin_src_dir="$RIME_LIB_DIR/rime-plugins"
    local plugin_dst_dir="$APP/Contents/Frameworks/rime-plugins"

    if [ ! -d "$plugin_src_dir" ]; then
        echo "    WARNING: no rime plugins found at $plugin_src_dir"
        return
    fi

    mkdir -p "$plugin_dst_dir"
    while IFS= read -r -d '' plugin; do
        local base
        base="$(basename "$plugin")"
        local dest="$plugin_dst_dir/$base"
        cp "$plugin" "$dest"
        chmod u+w "$dest"
        install_name_tool -id "@rpath/rime-plugins/$base" "$dest"
        if otool -L "$dest" | awk 'NR > 1 { print $1 }' | grep -qx '@rpath/librime.1.dylib'; then
            install_name_tool \
                -change "@rpath/librime.1.dylib" "@rpath/$RIME_DYLIB_BASENAME" \
                "$dest"
        fi
        bundle_dylib_deps "$dest"
    done < <(find "$plugin_src_dir" -maxdepth 1 -type f -name '*.dylib' -print0)
}

write_info_plist_strings() {
    local output="$1"
    local bundle_name="$2"
    local display_name="$3"
    local input_method_name="$4"
    local hans_name="$5"
    local copyright="$6"
    local tmp_xml
    local tmp_utf16
    tmp_xml="$(mktemp "${TMPDIR:-/tmp}/keytao-info-plist.XXXXXX.xml")"
    tmp_utf16="$(mktemp "${TMPDIR:-/tmp}/keytao-info-plist.XXXXXX.utf16")"

    cat > "$tmp_xml" << XML_EOF
<?xml version="1.0" encoding="UTF-16"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>$display_name</string>
  <key>CFBundleName</key>
  <string>$bundle_name</string>
  <key>ink.rea.inputmethod.keytao</key>
  <string>$input_method_name</string>
  <key>ink.rea.inputmethod.keytao.Hans</key>
  <string>$hans_name</string>
  <key>NSHumanReadableCopyright</key>
  <string>$copyright</string>
</dict>
</plist>
XML_EOF

    iconv -f UTF-8 -t UTF-16LE "$tmp_xml" > "$tmp_utf16"
    printf '\xff\xfe' > "$output"
    cat "$tmp_utf16" >> "$output"
    rm -f "$tmp_xml" "$tmp_utf16"
}

if { [ -z "${RIME_INCLUDE_DIR:-}" ] || [ -z "${RIME_LIB_DIR:-}" ]; } &&
    { [ ! -f "$VENDOR_ENV" ] ||
        [ ! -f "$VENDOR_DIR/include/rime_api.h" ] ||
        [ ! -e "$VENDOR_DIR/lib/librime.1.dylib" ] ||
        [ ! -f "$VENDOR_DIR/rime-data/default.yaml" ]; }; then
    "$WORKSPACE_DIR/scripts/fetch-librime.sh" \
        --platform macos \
        --version "${LIBRIME_VERSION:-latest}" \
        --destination "$VENDOR_DIR"
fi

if [ -f "$VENDOR_ENV" ]; then
    # shellcheck disable=SC1090
    source "$VENDOR_ENV"
fi

if [ -z "${RIME_INCLUDE_DIR:-}" ] || [ -z "${RIME_LIB_DIR:-}" ]; then
    for prefix in \
        "${RIME_PREFIX:-}" \
        "$VENDOR_DIR" \
        "/tmp/keytao-librime" \
        "/opt/homebrew/opt/librime" \
        "/usr/local/opt/librime" \
        "/run/current-system/sw"; do
        [ -n "$prefix" ] || continue
        if [ -f "$prefix/include/rime_api.h" ] && compgen -G "$prefix/lib/librime*.dylib" >/dev/null; then
            export RIME_INCLUDE_DIR="${RIME_INCLUDE_DIR:-$prefix/include}"
            export RIME_LIB_DIR="${RIME_LIB_DIR:-$prefix/lib}"
            break
        fi
    done
fi

if [ ! -f "${RIME_INCLUDE_DIR:-}/rime_api.h" ] || ! compgen -G "${RIME_LIB_DIR:-}/librime*.dylib" >/dev/null; then
    echo "ERROR: librime development files were not found." >&2
    echo "Set RIME_INCLUDE_DIR to the directory containing rime_api.h and RIME_LIB_DIR to the directory containing librime.dylib." >&2
    echo "For example: brew install librime, or run from nix develop where those variables are exported." >&2
    exit 1
fi

export BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} -I$RIME_INCLUDE_DIR"

find_rime_data_dir() {
    for dir in \
        "${KEYTAO_RIME_SHARED_DATA_DIR:-}" \
        "${RIME_SHARED_DATA_DIR:-}" \
        "${RIME_DATA_DIR:-}" \
        "$VENDOR_DIR/rime-data" \
        "$WORKSPACE_DIR/vendor/rime-data" \
        "/Library/Input Methods/Squirrel.app/Contents/SharedSupport" \
        "/opt/homebrew/share/rime-data" \
        "/usr/local/share/rime-data"; do
        [ -n "$dir" ] || continue
        if [ -f "$dir/default.yaml" ]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

RIME_DATA_DIR_RESOLVED="$(find_rime_data_dir || true)"
if [ -z "$RIME_DATA_DIR_RESOLVED" ]; then
    echo "ERROR: rime-data was not found." >&2
    echo "Set KEYTAO_RIME_SHARED_DATA_DIR or run scripts/fetch-librime.sh --platform macos." >&2
    exit 1
fi

echo "==> Generating input source icons..."
"$SCRIPT_DIR/generate-icons.sh"

echo "==> Building keytao-core-ffi ($CARGO_PROFILE)..."
cargo build $CARGO_FLAGS \
    --manifest-path "$WORKSPACE_DIR/Cargo.toml" \
    -p keytao-core-ffi \
    --target-dir "$WORKSPACE_DIR/target"

DYLIB_SRC="$WORKSPACE_DIR/target/$CARGO_PROFILE/libkeytao_core_ffi.dylib"
if [ ! -f "$DYLIB_SRC" ]; then
    echo "ERROR: dylib not found at $DYLIB_SRC" >&2
    exit 1
fi

echo "==> Creating app bundle skeleton..."
if [ -e "$APP" ]; then
    chmod -R u+w "$APP" 2>/dev/null || true
fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Frameworks"
mkdir -p "$APP/Contents/Resources"

cp "$SCRIPT_DIR/Resources/Info.plist" "$APP/Contents/Info.plist"
cp "$SCRIPT_DIR/Resources/keytao-menu-icon.pdf" "$APP/Contents/Resources/"
cp "$SCRIPT_DIR/Resources/KeyTaoInputSource.icns" "$APP/Contents/Resources/"
cp "$WORKSPACE_DIR/crates/keytao-theme/default-theme.yaml" "$APP/Contents/Resources/"
ditto "$RIME_DATA_DIR_RESOLVED" "$APP/Contents/Resources/rime-data"
printf 'APPL????' > "$APP/Contents/PkgInfo"

mkdir -p "$APP/Contents/Resources/en.lproj"
write_info_plist_strings \
    "$APP/Contents/Resources/en.lproj/InfoPlist.strings" \
    "KeyTao Input Method" \
    "KeyTao Input Method" \
    "KeyTao" \
    "KeyTao - Simplified" \
    "Copyleft, KeyTao Developers"
mkdir -p "$APP/Contents/Resources/English.lproj"
write_info_plist_strings \
    "$APP/Contents/Resources/English.lproj/InfoPlist.strings" \
    "KeyTao Input Method" \
    "KeyTao Input Method" \
    "KeyTao" \
    "KeyTao - Simplified" \
    "Copyleft, KeyTao Developers"
mkdir -p "$APP/Contents/Resources/zh-Hans.lproj"
write_info_plist_strings \
    "$APP/Contents/Resources/zh-Hans.lproj/InfoPlist.strings" \
    "键道输入法" \
    "键道输入法" \
    "键道" \
    "键道" \
    "键道开发者"
mkdir -p "$APP/Contents/Resources/zh-Hant.lproj"
write_info_plist_strings \
    "$APP/Contents/Resources/zh-Hant.lproj/InfoPlist.strings" \
    "鍵道輸入法" \
    "鍵道輸入法" \
    "鍵道" \
    "鍵道" \
    "鍵道開發者"

cp "$DYLIB_SRC" "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
install_name_tool \
    -id "@rpath/libkeytao_core_ffi.dylib" \
    "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"

RIME_DYLIB="$(find "$RIME_LIB_DIR" -maxdepth 1 \( -type f -o -type l \) -name 'librime*.dylib' | sort | head -1)"
RIME_DYLIB_BASENAME="$(basename "$RIME_DYLIB")"
cp "$RIME_DYLIB" "$APP/Contents/Frameworks/$RIME_DYLIB_BASENAME"
chmod u+w "$APP/Contents/Frameworks/$RIME_DYLIB_BASENAME"
install_name_tool \
    -id "@rpath/$RIME_DYLIB_BASENAME" \
    "$APP/Contents/Frameworks/$RIME_DYLIB_BASENAME"

RIME_LINKED_NAME="$(otool -L "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib" \
    | awk '/librime.*dylib/ { print $1; exit }')"
if [ -n "$RIME_LINKED_NAME" ]; then
    install_name_tool \
        -change "$RIME_LINKED_NAME" "@rpath/$RIME_DYLIB_BASENAME" \
        "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
fi
bundle_dylib_deps "$APP/Contents/Frameworks/$RIME_DYLIB_BASENAME"
bundle_dylib_deps "$APP/Contents/Frameworks/libkeytao_core_ffi.dylib"
bundle_rime_plugins

echo "==> Copying C header for Swift build..."
HEADER_DIR="$SCRIPT_DIR/Sources/CKeytaoCore"
mkdir -p "$HEADER_DIR"
cp "$WORKSPACE_DIR/crates/keytao-core-ffi/include/keytao_core.h" "$HEADER_DIR/"
cat > "$HEADER_DIR/module.modulemap" << 'MMEOF'
module CKeytaoCore [system] {
  header "keytao_core.h"
  link "keytao_core_ffi"
  export *
}
MMEOF

echo "==> Building Swift IME executable..."
swiftc \
    "$SCRIPT_DIR/Sources/KeyTaoIME/"*.swift \
    -module-name KeyTaoIME \
    -disable-bridging-pch \
    -framework Cocoa \
    -framework InputMethodKit \
    -framework Carbon \
    -I "$HEADER_DIR" \
    -L "$APP/Contents/Frameworks" -lkeytao_core_ffi \
    -Xlinker -rpath -Xlinker @executable_path/../Frameworks \
    $( [[ "$PROFILE" == "release" ]] && echo "-O" || echo "-g" ) \
    -o "$APP/Contents/MacOS/KeyTaoIME"

echo "==> Signing (Apple Development cert)..."
ENTITLEMENTS="$SCRIPT_DIR/dev.entitlements.plist"
cat > "$ENTITLEMENTS" << 'ENTEOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.app-sandbox</key><false/>
<key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
ENTEOF

APPLE_DEV_CERT=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep "Apple Development" | head -1 | sed 's/.*"\(.*\)"/\1/')
if [ -n "$APPLE_DEV_CERT" ]; then
    SIGN_ID="$APPLE_DEV_CERT"
    echo "    Using cert: $SIGN_ID"
else
    SIGN_ID="-"
    echo "    WARNING: No Apple Development cert found, falling back to ad-hoc"
fi

while IFS= read -r -d '' dylib; do
    codesign --force --sign "$SIGN_ID" --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$dylib"
done < <(find "$APP/Contents/Frameworks" -type f -name '*.dylib' -print0)
codesign --force --sign "$SIGN_ID" --options runtime \
    --entitlements "$ENTITLEMENTS" \
    "$APP"

echo ""
echo "==> Build complete: $APP"

if [ "$BUILD_PKG" -eq 0 ]; then
    exit 0
fi

echo ""
echo "==> Building .pkg package (installs via system_installd, no provenance xattr)..."
PKG_PAYLOAD="$BUILD_DIR/pkg_payload"
PKG_SCRIPTS="$BUILD_DIR/pkg_scripts"
PKG_COMPONENTS="$BUILD_DIR/KeyTao-components.plist"
rm -rf "$PKG_PAYLOAD" "$PKG_SCRIPTS"
mkdir -p "$PKG_PAYLOAD/Library/Input Methods"
mkdir -p "$PKG_SCRIPTS"

ditto --noextattr --norsrc "$APP" "$PKG_PAYLOAD/Library/Input Methods/KeyTao.app"
find "$PKG_PAYLOAD" \( -name '._*' -o -name '.DS_Store' \) -delete
find "$PKG_SCRIPTS" \( -name '._*' -o -name '.DS_Store' \) -delete
xattr -cr "$PKG_PAYLOAD" 2>/dev/null || true
xattr -cr "$PKG_SCRIPTS" 2>/dev/null || true

cat > "$PKG_COMPONENTS" << 'PLISTEOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>BundleHasStrictIdentifier</key>
    <false/>
    <key>BundleIsRelocatable</key>
    <false/>
    <key>BundleIsVersionChecked</key>
    <true/>
    <key>BundleOverwriteAction</key>
    <string>upgrade</string>
    <key>RootRelativeBundlePath</key>
    <string>Library/Input Methods/KeyTao.app</string>
  </dict>
</array>
</plist>
PLISTEOF

# Post-install script: register with TIS
cat > "$PKG_SCRIPTS/postinstall" << 'SCRIPTEOF'
#!/bin/bash
# Stop an old input method server before refreshing Launch Services.
killall KeyTaoIME 2>/dev/null || true
killall imklaunchagent 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true
killall cfprefsd 2>/dev/null || true
# Give Launch Services time to index the new bundle
sleep 2
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "/Library/Input Methods/KeyTao.app"
xattr -dr com.apple.quarantine "/Library/Input Methods/KeyTao.app" 2>/dev/null || true
xattr -dr com.apple.provenance "/Library/Input Methods/KeyTao.app" 2>/dev/null || true
# Register, enable, and select through the input method's installer command.
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --register-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --enable-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --select-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --list-input-sources || true
exit 0
SCRIPTEOF
chmod +x "$PKG_SCRIPTS/postinstall"

COPYFILE_DISABLE=1 pkgbuild \
    --root "$PKG_PAYLOAD" \
    --component-plist "$PKG_COMPONENTS" \
    --scripts "$PKG_SCRIPTS" \
    --identifier "ink.rea.inputmethod.keytao-package" \
    --version "1.0.0" \
    --install-location "/" \
    "$BUILD_DIR/KeyTao.pkg" 2>&1

echo ""
echo "==> pkg complete: $BUILD_DIR/KeyTao.pkg"
echo ""
echo "To install via pkg (recommended — avoids provenance xattr):"
echo "  sudo installer -pkg \"$BUILD_DIR/KeyTao.pkg\" -target /"
echo ""
echo "Or direct install (files may have provenance xattr, TIS may not register):"
echo "  sudo rm -rf \"/Library/Input Methods/KeyTao.app\""
echo "  sudo ditto \"$APP\" \"/Library/Input Methods/KeyTao.app\""
