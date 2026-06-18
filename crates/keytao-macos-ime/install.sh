#!/usr/bin/env bash
# Build, install, register, and refresh the KeyTao macOS input method.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="${KEYTAO_MACOS_BUILD_DIR:-/tmp/keytao-macos-ime-build}"
PROFILE="${1:-release}"
APP="/Library/Input Methods/KeyTao.app"
USER_APP="$HOME/Library/Input Methods/KeyTao.app"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

case "$PROFILE" in
    release|--release)
        PROFILE="release"
        ;;
    debug|--debug)
        PROFILE="debug"
        ;;
    *)
        echo "Usage: $0 [--release | --debug]" >&2
        exit 2
        ;;
esac

echo "==> Building KeyTao macOS IME ($PROFILE)..."
KEYTAO_MACOS_BUILD_DIR="$BUILD_DIR" "$SCRIPT_DIR/build.sh" "$PROFILE"

echo "==> Requesting administrator permission for system input method install..."
sudo -v

echo "==> Stopping running KeyTao IME processes..."
killall KeyTaoIME 2>/dev/null || true
killall imklaunchagent 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true
killall cfprefsd 2>/dev/null || true
sleep 1

echo "==> Unregistering old KeyTao input method bundles..."
for path in \
    "$USER_APP" \
    "$APP" \
    "$SCRIPT_DIR/build/KeyTao.app" \
    "$SCRIPT_DIR/build/pkg_payload/Library/Input Methods/KeyTao.app" \
    "$BUILD_DIR/KeyTao.app" \
    "$BUILD_DIR/pkg_payload/Library/Input Methods/KeyTao.app"; do
    "$LSREGISTER" -u "$path" 2>/dev/null || true
done

echo "==> Removing user-level test bundle and installing system bundle..."
rm -rf "$USER_APP"
sudo rm -rf "$APP"
sudo installer -pkg "$BUILD_DIR/KeyTao.pkg" -target /
sudo chown -R root:wheel "$APP"
sudo xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true
sudo xattr -d com.apple.provenance "$APP" 2>/dev/null || true

echo "==> Registering KeyTao with LaunchServices and Text Input Sources..."
"$LSREGISTER" -f "$APP"
"$LSREGISTER" -gc 2>/dev/null || true
KEYTAO_BIN="$APP/Contents/MacOS/KeyTaoIME"

echo "==> Refreshing macOS input source agents..."
killall KeyTaoIME 2>/dev/null || true
killall TextInputMenuAgent 2>/dev/null || true
killall imklaunchagent 2>/dev/null || true
killall KeyboardSettings 2>/dev/null || true
killall "System Settings" 2>/dev/null || true
killall SystemUIServer 2>/dev/null || true
killall cfprefsd 2>/dev/null || true
sleep 1

"$KEYTAO_BIN" --register-input-source
"$KEYTAO_BIN" --disable-legacy-input-sources
"$KEYTAO_BIN" --enable-input-source
"$KEYTAO_BIN" --select-input-source
"$KEYTAO_BIN" --list-input-sources
open -gja "$APP"

echo ""
echo "Installed: $APP"
echo "Build output: $BUILD_DIR/KeyTao.pkg"
echo "Open System Settings > Keyboard > Input Sources to verify the refreshed icon/name."
