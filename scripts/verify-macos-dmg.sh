#!/usr/bin/env bash
# Verify the DMG wrapper without installing the pkg.
set -euo pipefail

if (( $# != 2 )); then
    echo "Usage: $0 <dmg> <pkg>" >&2
    exit 1
fi

DMG_PATH="$1"
PKG_PATH="$2"
TMP_DIR="$(mktemp -d)"
MOUNT_POINT="$TMP_DIR/volume"

cleanup() {
    if mount | grep -Fq " on $MOUNT_POINT ("; then
        hdiutil detach "$MOUNT_POINT" -quiet || hdiutil detach "$MOUNT_POINT" -force -quiet || return 1
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir "$MOUNT_POINT"
hdiutil attach "$DMG_PATH" -readonly -nobrowse -mountpoint "$MOUNT_POINT" -quiet
PKG_SHA="$(shasum -a 256 "$PKG_PATH" | awk '{print $1}')"
DMG_PKG_SHA="$(shasum -a 256 "$MOUNT_POINT/KeyTao.pkg" | awk '{print $1}')"
case "$DMG_PKG_SHA" in
    "$PKG_SHA") ;;
    *)
        echo "ERROR: KeyTao.pkg differs from the original pkg." >&2
        exit 1
        ;;
esac
case "$(/usr/bin/stat -f %HT "$MOUNT_POINT/.VolumeIcon.icns")" in
    'Regular File') ;;
    *)
        echo "ERROR: missing DMG volume icon." >&2
        exit 1
        ;;
esac

hdiutil detach "$MOUNT_POINT" -quiet
echo "==> macOS DMG verification passed"
