#!/usr/bin/env bash
# Wrap the installer pkg in a DMG with KeyTao Finder icons.
set -euo pipefail

if (( $# != 2 )); then
    echo "Usage: $0 <input.pkg> <output.dmg>" >&2
    exit 1
fi

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG_PATH="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
DMG_PATH="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
TMP_DIR="$(mktemp -d)"
MOUNT_POINT="$TMP_DIR/volume"
ICONSET_DIR="$TMP_DIR/KeyTao.iconset"
RW_DMG="$TMP_DIR/KeyTao.dmg"

cleanup() {
    if mount | grep -Fq " on $MOUNT_POINT ("; then
        hdiutil detach "$MOUNT_POINT" -quiet || hdiutil detach "$MOUNT_POINT" -force -quiet || return 1
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir "$MOUNT_POINT" "$ICONSET_DIR"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$PROJECT_DIR/packaging/icons/icon.png" \
        --out "$ICONSET_DIR/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$PROJECT_DIR/packaging/icons/icon.png" \
        --out "$ICONSET_DIR/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET_DIR" -o "$TMP_DIR/KeyTao.icns"

# Leave room for the filesystem and custom-icon metadata.
SIZE_MB="$(( $(/usr/bin/stat -f %z "$PKG_PATH") / 1024 / 1024 + 32 ))"
hdiutil create -size "${SIZE_MB}m" -fs HFS+ -volname KeyTao "$RW_DMG" -quiet
hdiutil attach "$RW_DMG" -nobrowse -mountpoint "$MOUNT_POINT" -quiet
cp "$PKG_PATH" "$MOUNT_POINT/KeyTao.pkg"
cp "$TMP_DIR/KeyTao.icns" "$MOUNT_POINT/.VolumeIcon.icns"
xcrun SetFile -a C "$MOUNT_POINT"

# NSWorkspace writes Finder metadata without changing the signed pkg data.
osascript -l JavaScript - "$TMP_DIR/KeyTao.icns" "$MOUNT_POINT/KeyTao.pkg" <<'JXA'
ObjC.import('AppKit');
function run(argv) {
    const icon = $.NSImage.alloc.initWithContentsOfFile(argv[0]);
    if (!$.NSWorkspace.sharedWorkspace.setIconForFileOptions(icon, argv[1], 0)) {
        throw new Error('Unable to set the installer Finder icon');
    }
}
JXA

hdiutil detach "$MOUNT_POINT" -quiet
hdiutil convert "$RW_DMG" -format UDZO -o "$DMG_PATH" -ov -quiet
echo "==> DMG complete: $DMG_PATH"
