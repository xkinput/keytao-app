#!/usr/bin/env bash
# Generate macOS input source icons from the current KeyTao app logo.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESOURCES_DIR="$SCRIPT_DIR/Resources"
SOURCE_ICON="$WORKSPACE_DIR/src-tauri/icons/icon.png"
MENU_PNG="$RESOURCES_DIR/keytao-menu-icon.png"
MENU_PDF="$RESOURCES_DIR/keytao-menu-icon.pdf"
INPUT_ICNS="$RESOURCES_DIR/KeyTaoInputSource.icns"

have_generated_icons() {
    [ -f "$MENU_PNG" ] && [ -f "$MENU_PDF" ] && [ -f "$INPUT_ICNS" ]
}

if ! command -v magick >/dev/null 2>&1; then
    if have_generated_icons; then
        echo "ImageMagick 'magick' is not available; reusing checked-in macOS IME icons."
        exit 0
    fi
    echo "ERROR: ImageMagick 'magick' is required to generate macOS IME icons." >&2
    exit 1
fi
if ! command -v iconutil >/dev/null 2>&1; then
    if have_generated_icons; then
        echo "macOS 'iconutil' is not available; reusing checked-in macOS IME icons."
        exit 0
    fi
    echo "ERROR: macOS 'iconutil' is required to generate KeyTaoInputSource.icns." >&2
    exit 1
fi
if [ ! -f "$SOURCE_ICON" ]; then
    echo "ERROR: source icon not found: $SOURCE_ICON" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/keytao-ime-icons.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

MASK="$TMP_DIR/keytao-logo-mask.png"
MENU_SVG="$TMP_DIR/keytao-menu-icon.svg"
INPUT_BG="$TMP_DIR/keytao-input-bg.png"
INPUT_MASK="$TMP_DIR/keytao-input-mask.png"
INPUT_GLYPH="$TMP_DIR/keytao-input-glyph.png"
INPUT_PNG="$TMP_DIR/KeyTaoInputSource.png"
ICONSET="$TMP_DIR/KeyTaoInputSource.iconset"

mkdir -p "$RESOURCES_DIR" "$ICONSET"

magick "$SOURCE_ICON" \
    -alpha extract \
    -threshold 20% \
    -morphology Open Disk:0.6 \
    -alpha off \
    "$MASK"

cat > "$MENU_SVG" << 'SVGEOF'
<svg xmlns="http://www.w3.org/2000/svg" width="28" height="20" viewBox="0 0 28 20">
  <path fill="#000" fill-rule="evenodd" d="
    M5.2 0H22.8C25.8 0 28 2.2 28 5.2V14.8C28 17.8 25.8 20 22.8 20H5.2C2.2 20 0 17.8 0 14.8V5.2C0 2.2 2.2 0 5.2 0Z
    M14 2.7L15.95 8.05H21.7L17.05 11.38L18.82 16.85L14 13.48L9.18 16.85L10.95 11.38L6.3 8.05H12.05Z"/>
</svg>
SVGEOF
magick -background none "$MENU_SVG" "$MENU_PDF"
magick -background none -density 288 "$MENU_SVG" -resize 28x20 "$MENU_PNG"

magick -size 1024x1024 xc:none \
    -fill '#c8cdd4' \
    -draw 'roundrectangle 104,104 920,920 186,186' \
    "$INPUT_BG"
magick "$MASK" \
    -resize 700x700 \
    -gravity center \
    -background black \
    -extent 1024x1024 \
    "$INPUT_MASK"
magick -size 1024x1024 xc:'#202124' "$INPUT_MASK" \
    -compose CopyOpacity \
    -composite \
    "$INPUT_GLYPH"
magick "$INPUT_BG" "$INPUT_GLYPH" \
    -compose over \
    -composite \
    "$INPUT_PNG"

magick "$INPUT_PNG" -resize 16x16 "PNG32:$ICONSET/icon_16x16.png"
magick "$INPUT_PNG" -resize 32x32 "PNG32:$ICONSET/icon_16x16@2x.png"
magick "$INPUT_PNG" -resize 32x32 "PNG32:$ICONSET/icon_32x32.png"
magick "$INPUT_PNG" -resize 64x64 "PNG32:$ICONSET/icon_32x32@2x.png"
magick "$INPUT_PNG" -resize 128x128 "PNG32:$ICONSET/icon_128x128.png"
magick "$INPUT_PNG" -resize 256x256 "PNG32:$ICONSET/icon_128x128@2x.png"
magick "$INPUT_PNG" -resize 256x256 "PNG32:$ICONSET/icon_256x256.png"
magick "$INPUT_PNG" -resize 512x512 "PNG32:$ICONSET/icon_256x256@2x.png"
magick "$INPUT_PNG" -resize 512x512 "PNG32:$ICONSET/icon_512x512.png"
magick "$INPUT_PNG" -resize 1024x1024 "PNG32:$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$RESOURCES_DIR/KeyTaoInputSource.icns"

echo "Generated:"
echo "  $MENU_PDF"
echo "  $MENU_PNG"
echo "  $RESOURCES_DIR/KeyTaoInputSource.icns"
