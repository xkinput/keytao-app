#!/usr/bin/env bash
set -euo pipefail
export COPYFILE_DISABLE=1

REPO_DIR="$(cd "${SRCROOT:?}/../.." && pwd)"
RESOURCES_DIR="${TARGET_BUILD_DIR:?}/${UNLOCALIZED_RESOURCES_FOLDER_PATH:?}"
case "${PLATFORM_NAME:?}" in
    iphoneos) runtime="iphoneos-arm64" ;;
    iphonesimulator)
        case " ${ARCHS:?} " in
            *" arm64 "*) runtime="iphonesimulator-arm64" ;;
            *" x86_64 "*) runtime="iphonesimulator-x86_64" ;;
            *) echo "error: Unsupported simulator architectures: $ARCHS" >&2; exit 1 ;;
        esac
        ;;
    *) echo "error: Unsupported iOS platform: $PLATFORM_NAME" >&2; exit 1 ;;
esac

# The staged runtime includes the same optional schema overlay as the old iOS app.
RIME_DATA_DIR="$REPO_DIR/target/keytao-ios-runtime/$runtime/rime-data"
if [ ! -f "$RIME_DATA_DIR/default.yaml" ]; then
    RIME_DATA_DIR="$REPO_DIR/vendor/librime/ios/$runtime/rime-data"
fi
ADDON_DIR="$REPO_DIR/resources/addon-schemas"
CONFIG_DIR="$REPO_DIR/crates/keytao-ios-ime/Sources/KeyTaoIOSIME/Resources"
required=("$RIME_DATA_DIR/default.yaml")
if [ "${PRODUCT_NAME:?}" = "KeyTaoKeyboard" ]; then
    required+=("$CONFIG_DIR/keytao_ios_ime.json" "$CONFIG_DIR/keytao-logo.png")
else
    required+=("$ADDON_DIR/easy_en/easy_en.schema.yaml")
fi
for source in "${required[@]}"; do
    if [ ! -f "$source" ]; then
        echo "error: Missing iOS bundle input: $source" >&2
        exit 1
    fi
done

mkdir -p "$RESOURCES_DIR"
rm -rf "$RESOURCES_DIR/rime-data"
ditto "$RIME_DATA_DIR" "$RESOURCES_DIR/rime-data"
if [ "$PRODUCT_NAME" = "KeyTaoKeyboard" ]; then
    cp "$CONFIG_DIR/keytao_ios_ime.json" "$CONFIG_DIR/keytao-logo.png" "$RESOURCES_DIR/"
else
    rm -rf "$RESOURCES_DIR/addon-schemas"
    ditto "$ADDON_DIR" "$RESOURCES_DIR/addon-schemas"
fi
