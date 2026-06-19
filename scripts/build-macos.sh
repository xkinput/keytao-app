#!/usr/bin/env bash
# Build a macOS pkg containing the main KeyTao app and the system IME bundle.
set -euo pipefail
export COPYFILE_DISABLE=1

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IME_BUILD_DIR="$PROJECT_DIR/target/keytao-macos-ime"
APP_RUNTIME_DIR="$PROJECT_DIR/target/keytao-macos-app-runtime"
APP_FRAMEWORKS_DIR="$APP_RUNTIME_DIR/Frameworks"
PKG_BUILD_DIR="$PROJECT_DIR/target/keytao-macos-pkg"
VENDOR_DIR="$PROJECT_DIR/vendor/librime/macos-universal"
VENDOR_ENV="$VENDOR_DIR/env.sh"

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
        "$PROJECT_DIR/target/keytao-macos-app-runtime/rime-data" \
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

echo "==> Preparing macOS app runtime..."
rm -rf "$APP_RUNTIME_DIR"
mkdir -p "$APP_FRAMEWORKS_DIR"
ditto "$RIME_DATA_DIR" "$APP_RUNTIME_DIR/rime-data"

RIME_RUNTIME_DYLIB="$(find "$RIME_LIB_DIR" -maxdepth 1 \( -type f -o -type l \) -name 'librime.1.dylib' | sort | head -1)"
if [ -z "$RIME_RUNTIME_DYLIB" ]; then
    RIME_RUNTIME_DYLIB="$(find "$RIME_LIB_DIR" -maxdepth 1 \( -type f -o -type l \) -name 'librime.*.dylib' | sort | head -1)"
fi
if [ -z "$RIME_RUNTIME_DYLIB" ]; then
    echo "ERROR: no librime runtime dylib found in $RIME_LIB_DIR" >&2
    exit 1
fi
cp -L "$RIME_RUNTIME_DYLIB" "$APP_FRAMEWORKS_DIR/librime.1.dylib"
chmod u+w "$APP_FRAMEWORKS_DIR/librime.1.dylib"
install_name_tool -id "@rpath/librime.1.dylib" "$APP_FRAMEWORKS_DIR/librime.1.dylib"

if [ -d "$RIME_LIB_DIR/rime-plugins" ]; then
    mkdir -p "$APP_FRAMEWORKS_DIR/rime-plugins"
    while IFS= read -r -d '' plugin; do
        base="$(basename "$plugin")"
        cp "$plugin" "$APP_FRAMEWORKS_DIR/rime-plugins/$base"
        chmod u+w "$APP_FRAMEWORKS_DIR/rime-plugins/$base"
        install_name_tool \
            -id "@rpath/rime-plugins/$base" \
            "$APP_FRAMEWORKS_DIR/rime-plugins/$base"
    done < <(find "$RIME_LIB_DIR/rime-plugins" -maxdepth 1 -type f -name '*.dylib' -print0)
else
    echo "WARNING: no rime plugins found at $RIME_LIB_DIR/rime-plugins"
fi

while IFS= read -r -d '' dylib; do
    base="$(basename "$dylib")"
    [ "$base" = "libkeytao_core_ffi.dylib" ] && continue
    [[ "$base" == librime* ]] && continue
    cp "$dylib" "$APP_FRAMEWORKS_DIR/$base"
done < <(find "$IME_BUILD_DIR/KeyTao.app/Contents/Frameworks" -maxdepth 1 -type f -name '*.dylib' -print0)

echo "==> Building KeyTao macOS app bundle..."
cd "$PROJECT_DIR"
rm -rf \
    "$PROJECT_DIR/target/release/KeyTao.app" \
    "$PROJECT_DIR/target/release/bundle/macos/KeyTao.app" \
    "$PROJECT_DIR/target/release/bundle/dmg"
pnpm tauri build --bundles app --config src-tauri/tauri.macos.conf.json

MAIN_APP="$PROJECT_DIR/target/release/bundle/macos/KeyTao.app"
if [ ! -d "$MAIN_APP" ]; then
    MAIN_APP="$PROJECT_DIR/target/release/KeyTao.app"
fi
IME_APP="$IME_BUILD_DIR/KeyTao.app"
if [ ! -d "$MAIN_APP" ]; then
    echo "ERROR: Tauri main app bundle was not produced." >&2
    exit 1
fi
if [ ! -d "$IME_APP" ]; then
    echo "ERROR: macOS IME bundle was not produced." >&2
    exit 1
fi

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

echo "==> Completing KeyTao macOS app runtime..."
MAIN_FRAMEWORKS_DIR="$MAIN_APP/Contents/Frameworks"
mkdir -p "$MAIN_FRAMEWORKS_DIR"
while IFS= read -r -d '' dylib; do
    base="$(basename "$dylib")"
    cp -f "$dylib" "$MAIN_FRAMEWORKS_DIR/$base"
    chmod u+w "$MAIN_FRAMEWORKS_DIR/$base"
done < <(find "$APP_FRAMEWORKS_DIR" -maxdepth 1 -type f -name '*.dylib' -print0)
if [ -d "$APP_FRAMEWORKS_DIR/rime-plugins" ]; then
    rm -rf "$MAIN_FRAMEWORKS_DIR/rime-plugins"
    ditto "$APP_FRAMEWORKS_DIR/rime-plugins" "$MAIN_FRAMEWORKS_DIR/rime-plugins"
    find "$MAIN_FRAMEWORKS_DIR/rime-plugins" \( -name '._*' -o -name '.DS_Store' \) -delete
    xattr -cr "$MAIN_FRAMEWORKS_DIR/rime-plugins" 2>/dev/null || true
fi
if [ -d "$RIME_LIB_DIR/rime-plugins" ] && [ ! -f "$MAIN_FRAMEWORKS_DIR/rime-plugins/librime-lua.dylib" ]; then
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

APPLE_DEV_CERT="$(security find-identity -v -p codesigning 2>/dev/null \
    | grep "Apple Development" | head -1 | sed 's/.*"\(.*\)"/\1/')"
if [ -n "${KEYTAO_CODESIGN_IDENTITY:-}" ]; then
    SIGN_ID="$KEYTAO_CODESIGN_IDENTITY"
elif [ -n "$APPLE_DEV_CERT" ]; then
    SIGN_ID="$APPLE_DEV_CERT"
else
    SIGN_ID="-"
fi
echo "    Using signing identity: $SIGN_ID"
while IFS= read -r -d '' dylib; do
    codesign --force --sign "$SIGN_ID" --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$dylib"
done < <(find "$MAIN_APP/Contents/Frameworks" -type f -name '*.dylib' -print0)
codesign --force --sign "$SIGN_ID" --options runtime \
    --entitlements "$ENTITLEMENTS" \
    "$MAIN_APP"

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
killall cfprefsd 2>/dev/null || true
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

PACKAGE_VERSION="$(node -p "JSON.parse(require('fs').readFileSync('package.json', 'utf8')).version")"
COPYFILE_DISABLE=1 pkgbuild \
    --root "$PKG_PAYLOAD" \
    --component-plist "$PKG_COMPONENTS" \
    --scripts "$PKG_SCRIPTS" \
    --identifier "ink.rea.keytao-app.pkg" \
    --version "$PACKAGE_VERSION" \
    --install-location "/" \
    "$PKG_BUILD_DIR/KeyTao.pkg"

echo ""
echo "==> pkg complete: $PKG_BUILD_DIR/KeyTao.pkg"
echo "    /Applications/KeyTao.app"
echo "    /Library/Input Methods/KeyTao.app"
