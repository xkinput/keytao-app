#!/usr/bin/env bash
set -euo pipefail
export COPYFILE_DISABLE=1

REPO_DIR="$(cd "${PROJECT_DIR:?}/../.." && pwd)"
VENDOR_DIR="$REPO_DIR/vendor/librime/macos-universal"
RESOURCES_DIR="${TARGET_BUILD_DIR:?}/${UNLOCALIZED_RESOURCES_FOLDER_PATH:?}"
FRAMEWORKS_DIR="${TARGET_BUILD_DIR:?}/${FRAMEWORKS_FOLDER_PATH:?}"
RIME_DATA_DIR="$VENDOR_DIR/rime-data"
ADDON_SCHEMAS_DIR="$REPO_DIR/resources/addon-schemas"
RIME_LIB_DIR="$VENDOR_DIR/lib"

for required in \
    "$RIME_DATA_DIR/default.yaml" \
    "$ADDON_SCHEMAS_DIR/easy_en/easy_en.schema.yaml" \
    "$RIME_LIB_DIR/librime.1.dylib" \
    "$RIME_LIB_DIR/rime-plugins/librime-lua.dylib"; do
    if [ ! -f "$required" ]; then
        echo "error: Missing macOS Rime bundle input: $required" >&2
        exit 1
    fi
done

mkdir -p "$RESOURCES_DIR" "$FRAMEWORKS_DIR"
rm -rf "$RESOURCES_DIR/rime-data" "$RESOURCES_DIR/addon-schemas" "$FRAMEWORKS_DIR/rime-plugins"
# Remove the former Flutter code asset on incremental builds.
rm -rf "$FRAMEWORKS_DIR/librime.framework"
ditto "$RIME_DATA_DIR" "$RESOURCES_DIR/rime-data"
ditto "$ADDON_SCHEMAS_DIR" "$RESOURCES_DIR/addon-schemas"
mkdir -p "$FRAMEWORKS_DIR/rime-plugins"

copy_dylib() {
    local source="$1" destination="$2" install_name="$3"
    cp -L "$source" "$destination"
    chmod u+w "$destination"
    install_name_tool -id "$install_name" "$destination"
    # install_name_tool invalidates the signature; sign before Xcode seals the app.
    if [ "${CODE_SIGNING_ALLOWED:-YES}" != "NO" ]; then
        codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" --timestamp=none "$destination"
    fi
}

copy_dylib "$RIME_LIB_DIR/librime.1.dylib" "$FRAMEWORKS_DIR/librime.1.dylib" "@rpath/librime.1.dylib"
for plugin in "$RIME_LIB_DIR/rime-plugins/"*.dylib; do
    base="$(basename "$plugin")"
    copy_dylib "$plugin" "$FRAMEWORKS_DIR/rime-plugins/$base" "@rpath/rime-plugins/$base"
done
