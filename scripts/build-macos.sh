#!/usr/bin/env bash
# Build a macOS pkg containing the main KeyTao app and the system IME bundle.
set -euo pipefail
export COPYFILE_DISABLE=1

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IME_BUILD_DIR="$PROJECT_DIR/target/keytao-macos-ime"
FLUTTER_DIR="$PROJECT_DIR/flutter_app"
MAIN_APP="$FLUTTER_DIR/build/macos/Build/Products/Release/KeyTao.app"
PKG_BUILD_DIR="$PROJECT_DIR/target/keytao-macos-pkg"
VENDOR_DIR="$PROJECT_DIR/vendor/librime/macos-universal"
VENDOR_ENV="$VENDOR_DIR/env.sh"
PACKAGE_VERSION="$(awk '
    /^\[workspace\.package\]/ { section = 1; next }
    /^\[/ { section = 0 }
    section && /^version[[:space:]]*=/ { gsub(/["]/, "", $3); print $3; exit }
' "$PROJECT_DIR/Cargo.toml")"
if [ -z "$PACKAGE_VERSION" ]; then
    echo "ERROR: workspace package version was not found." >&2
    exit 1
fi
# Match the IME's numeric Installer version (e.g. 1.2.1-alpha.89 -> 1.2.1.89).
BUNDLE_VERSION="$(printf '%s' "$PACKAGE_VERSION" | sed -E 's/[^0-9.]+/./g; s/\.+/./g; s/^\.//; s/\.$//')"

if { [ -z "${RIME_INCLUDE_DIR:-}" ] || [ -z "${RIME_LIB_DIR:-}" ]; } &&
    { [ ! -f "$VENDOR_ENV" ] ||
        [ ! -f "$VENDOR_DIR/include/rime_api.h" ] ||
        [ ! -e "$VENDOR_DIR/lib/librime.1.dylib" ] ||
        [ ! -f "$VENDOR_DIR/rime-data/default.yaml" ]; }; then
    "$PROJECT_DIR/scripts/fetch-librime.sh" \
        --platform macos \
        --version "${LIBRIME_VERSION:-latest}" \
        --destination "$VENDOR_DIR"
fi

if [ -f "$VENDOR_ENV" ]; then
    # shellcheck disable=SC1090
    source "$VENDOR_ENV"
fi

find_rime_prefix() {
    for prefix in \
        "${RIME_PREFIX:-}" \
        "$VENDOR_DIR" \
        "/tmp/keytao-librime" \
        "/opt/homebrew/opt/librime" \
        "/usr/local/opt/librime" \
        "/run/current-system/sw"; do
        [ -n "$prefix" ] || continue
        if [ -f "$prefix/include/rime_api.h" ] && compgen -G "$prefix/lib/librime*.dylib" >/dev/null; then
            printf '%s\n' "$prefix"
            return 0
        fi
    done
    return 1
}

find_rime_data_dir() {
    for dir in \
        "${KEYTAO_RIME_SHARED_DATA_DIR:-}" \
        "${RIME_SHARED_DATA_DIR:-}" \
        "${RIME_DATA_DIR:-}" \
        "$VENDOR_DIR/rime-data" \
        "$PROJECT_DIR/vendor/rime-data" \
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

RIME_PREFIX_RESOLVED="$(find_rime_prefix || true)"
if [ -z "$RIME_PREFIX_RESOLVED" ]; then
    echo "ERROR: librime development files were not found." >&2
    echo "Install or provide librime, or set RIME_PREFIX/RIME_INCLUDE_DIR/RIME_LIB_DIR." >&2
    exit 1
fi
export RIME_INCLUDE_DIR="${RIME_INCLUDE_DIR:-$RIME_PREFIX_RESOLVED/include}"
export RIME_LIB_DIR="${RIME_LIB_DIR:-$RIME_PREFIX_RESOLVED/lib}"
export BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} -I$RIME_INCLUDE_DIR"

RIME_DATA_DIR="$(find_rime_data_dir || true)"
if [ -z "$RIME_DATA_DIR" ]; then
    echo "ERROR: rime-data was not found." >&2
    echo "Set KEYTAO_RIME_SHARED_DATA_DIR or install Squirrel/Homebrew rime-data." >&2
    exit 1
fi

echo "==> Building macOS IME runtime..."
export KEYTAO_RIME_SHARED_DATA_DIR="$RIME_DATA_DIR"
KEYTAO_MACOS_BUILD_DIR="$IME_BUILD_DIR" \
    "$PROJECT_DIR/crates/keytao-macos-ime/build.sh" --release --skip-pkg

echo "==> Building KeyTao Flutter macOS app bundle..."
# Incremental builds can retain an invalid outer seal. Remove only the app,
# preserving Flutter's build cache and the other products in this directory.
rm -rf "$MAIN_APP"
(
    cd "$FLUTTER_DIR"
    flutter build macos --release --build-name "$PACKAGE_VERSION"
)
IME_APP="$IME_BUILD_DIR/KeyTao.app"
if [ ! -d "$MAIN_APP" ]; then
    echo "ERROR: Flutter main app bundle was not produced." >&2
    exit 1
fi
if [ ! -d "$IME_APP" ]; then
    echo "ERROR: macOS IME bundle was not produced." >&2
    exit 1
fi

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $PACKAGE_VERSION" "$MAIN_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUNDLE_VERSION" "$MAIN_APP/Contents/Info.plist"

MAIN_BUNDLE_ID="$(plutil -extract CFBundleIdentifier raw -o - "$MAIN_APP/Contents/Info.plist" 2>/dev/null || true)"
IME_BUNDLE_ID="$(plutil -extract CFBundleIdentifier raw -o - "$IME_APP/Contents/Info.plist" 2>/dev/null || true)"
if [ "$MAIN_BUNDLE_ID" = "$IME_BUNDLE_ID" ]; then
    echo "ERROR: main app and IME bundle identifiers are the same: $MAIN_BUNDLE_ID" >&2
    exit 1
fi
if [ "$MAIN_BUNDLE_ID" != "ink.rea.keytao-app" ]; then
    echo "ERROR: unexpected main app bundle identifier: ${MAIN_BUNDLE_ID:-<empty>}" >&2
    exit 1
fi
if [ "$IME_BUNDLE_ID" != "ink.rea.inputmethod.keytao" ]; then
    echo "ERROR: unexpected IME bundle identifier: ${IME_BUNDLE_ID:-<empty>}" >&2
    exit 1
fi

# Flutter's bundle-rime.sh already copies the runtime and resources.
MAIN_FRAMEWORKS_DIR="$MAIN_APP/Contents/Frameworks"
if [ ! -f "$MAIN_FRAMEWORKS_DIR/rime-plugins/librime-lua.dylib" ]; then
    echo "ERROR: macOS main app bundle is missing rime-plugins/librime-lua.dylib" >&2
    exit 1
fi

echo "==> Re-signing KeyTao macOS app bundle..."
ENTITLEMENTS="$PKG_BUILD_DIR/app.entitlements.plist"
mkdir -p "$PKG_BUILD_DIR"
cat > "$ENTITLEMENTS" << 'ENTEOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.app-sandbox</key><false/>
<key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
ENTEOF

APPLE_DEV_CERT=""
if [ "${CI:-}" != "true" ]; then
    APPLE_DEV_CERT="$(security find-identity -v -p codesigning 2>/dev/null \
        | grep "Apple Development" | head -1 | sed 's/.*"\(.*\)"/\1/' || true)"
fi
if [ -n "${KEYTAO_CODESIGN_IDENTITY:-}" ]; then
    SIGN_ID="$KEYTAO_CODESIGN_IDENTITY"
elif [ -n "$APPLE_DEV_CERT" ]; then
    SIGN_ID="$APPLE_DEV_CERT"
else
    SIGN_ID="-"
fi
echo "    Using signing identity: $SIGN_ID"
# Sign from the inside out: plugins, other dylibs, nested frameworks, app.
while IFS= read -r -d '' plugin; do
    codesign --force --sign "$SIGN_ID" --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$plugin"
done < <(find "$MAIN_FRAMEWORKS_DIR/rime-plugins" -type f -name '*.dylib' -print0)
while IFS= read -r -d '' dylib; do
    codesign --force --sign "$SIGN_ID" --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$dylib"
done < <(find "$MAIN_FRAMEWORKS_DIR" -type f -name '*.dylib' ! -path '*/rime-plugins/*' -print0)
while IFS= read -r -d '' framework; do
    codesign --force --sign "$SIGN_ID" --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$framework"
done < <(find "$MAIN_FRAMEWORKS_DIR" -depth -type d -name '*.framework' -print0)
codesign --force --sign "$SIGN_ID" --options runtime \
    --entitlements "$ENTITLEMENTS" \
    "$MAIN_APP"
codesign --verify --deep --strict --verbose=2 "$MAIN_APP"

echo "==> Building KeyTao installer pkg..."
PKG_PAYLOAD="$PKG_BUILD_DIR/payload"
PKG_SCRIPTS="$PKG_BUILD_DIR/scripts"
PKG_COMPONENTS="$PKG_BUILD_DIR/KeyTao-components.plist"
rm -rf "$PKG_BUILD_DIR"
mkdir -p "$PKG_PAYLOAD/Applications"
mkdir -p "$PKG_PAYLOAD/Library/Input Methods"
mkdir -p "$PKG_SCRIPTS"

ditto --noextattr --norsrc "$MAIN_APP" "$PKG_PAYLOAD/Applications/KeyTao.app"
ditto --noextattr --norsrc "$IME_APP" "$PKG_PAYLOAD/Library/Input Methods/KeyTao.app"
find "$PKG_PAYLOAD" \( -name '._*' -o -name '.DS_Store' \) -delete
find "$PKG_SCRIPTS" \( -name '._*' -o -name '.DS_Store' \) -delete
xattr -cr "$PKG_PAYLOAD" 2>/dev/null || true
xattr -cr "$PKG_SCRIPTS" 2>/dev/null || true
find "$PKG_PAYLOAD" -exec xattr -d com.apple.provenance {} \; 2>/dev/null || true
find "$PKG_SCRIPTS" -exec xattr -d com.apple.provenance {} \; 2>/dev/null || true

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
    <string>Applications/KeyTao.app</string>
  </dict>
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

cat > "$PKG_SCRIPTS/postinstall" << 'SCRIPTEOF'
#!/bin/bash
killall KeyTaoIME 2>/dev/null || true
killall imklaunchagent 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true
sleep 2
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "/Applications/KeyTao.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "/Library/Input Methods/KeyTao.app"
xattr -dr com.apple.quarantine "/Applications/KeyTao.app" 2>/dev/null || true
xattr -dr com.apple.provenance "/Applications/KeyTao.app" 2>/dev/null || true
xattr -dr com.apple.quarantine "/Library/Input Methods/KeyTao.app" 2>/dev/null || true
xattr -dr com.apple.provenance "/Library/Input Methods/KeyTao.app" 2>/dev/null || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --register-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --disable-legacy-input-sources || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --enable-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --select-input-source || true
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --list-input-sources || true
exit 0
SCRIPTEOF
chmod +x "$PKG_SCRIPTS/postinstall"

PKG_OUTPUT="$PKG_BUILD_DIR/keytao-app-${PACKAGE_VERSION}-macos.pkg"
COPYFILE_DISABLE=1 pkgbuild \
    --root "$PKG_PAYLOAD" \
    --component-plist "$PKG_COMPONENTS" \
    --scripts "$PKG_SCRIPTS" \
    --identifier "ink.rea.keytao-app.pkg" \
    --version "$PACKAGE_VERSION" \
    --install-location "/" \
    "$PKG_OUTPUT"

PKG_REPACK_DIR="$PKG_BUILD_DIR/repack"
PKG_EXPANDED_DIR="$PKG_REPACK_DIR/expanded"
PKG_FLAT_DIR="$PKG_REPACK_DIR/flat"
PKG_CLEAN_OUTPUT="$PKG_REPACK_DIR/KeyTao.clean.pkg"

echo "==> Removing AppleDouble metadata from installer pkg..."
rm -rf "$PKG_REPACK_DIR"
mkdir -p "$PKG_FLAT_DIR"
pkgutil --expand "$PKG_OUTPUT" "$PKG_EXPANDED_DIR"
cp "$PKG_EXPANDED_DIR/PackageInfo" "$PKG_FLAT_DIR/PackageInfo"
perl -0pi -e 's/postinstall-action="none"/postinstall-action="logout"/' "$PKG_FLAT_DIR/PackageInfo"
if ! grep -Fq 'postinstall-action="logout"' "$PKG_FLAT_DIR/PackageInfo"; then
    echo "ERROR: failed to mark the macOS installer as requiring logout." >&2
    exit 1
fi
mkbom -s "$PKG_PAYLOAD" "$PKG_FLAT_DIR/Bom"
(
    cd "$PKG_PAYLOAD"
    find . -print | LC_ALL=C sort | COPYFILE_DISABLE=1 cpio -o --format odc 2>/dev/null | gzip -c
) > "$PKG_FLAT_DIR/Payload"
(
    cd "$PKG_SCRIPTS"
    find . -print | LC_ALL=C sort | COPYFILE_DISABLE=1 cpio -o --format odc 2>/dev/null | gzip -c
) > "$PKG_FLAT_DIR/Scripts"
(
    cd "$PKG_FLAT_DIR"
    xar --compression gzip \
        --no-compress='Payload' \
        --no-compress='Scripts' \
        --prop-exclude='.*' \
        -cf "$PKG_CLEAN_OUTPUT" \
        Bom Payload Scripts PackageInfo
)
mv "$PKG_CLEAN_OUTPUT" "$PKG_OUTPUT"
rm -rf "$PKG_REPACK_DIR"

echo ""
echo "==> pkg complete: $PKG_OUTPUT"
echo "    /Applications/KeyTao.app"
echo "    /Library/Input Methods/KeyTao.app"
