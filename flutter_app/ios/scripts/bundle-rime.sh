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
CONFIG_DIR="$REPO_DIR/crates/keytao-ios-ime/Sources/KeyTaoIOSIME/Resources"
KEYBOARD_YAML="$REPO_DIR/crates/keytao-theme/default-keyboard.yaml"
core_version="$(awk '
    /^\[package\]$/ { package_section = 1; next }
    /^\[/ { package_section = 0 }
    package_section && /^version[[:space:]]*=/ {
        sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*/, ""); print; exit
    }
    package_section && /^version\.workspace[[:space:]]*=[[:space:]]*true/ { print "workspace"; exit }
' "$REPO_DIR/crates/keytao-core/Cargo.toml")"
if [ "$core_version" = "workspace" ]; then
    core_version="$(awk '
        /^\[workspace\.package\]$/ { workspace_package = 1; next }
        /^\[/ { workspace_package = 0 }
        workspace_package && /^version[[:space:]]*=/ {
            sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*/, ""); print; exit
        }
    ' "$REPO_DIR/Cargo.toml")"
fi
if [ -z "$core_version" ]; then
    echo "error: Missing keytao-core package version" >&2
    exit 1
fi
required=("$RIME_DATA_DIR/default.yaml")
if [ "${PRODUCT_NAME:?}" = "KeyTaoKeyboard" ]; then
    required+=("$CONFIG_DIR/keytao_ios_ime.json" "$CONFIG_DIR/keytao-logo.png")
else
    required+=("$KEYBOARD_YAML")
fi
for source in "${required[@]}"; do
    if [ ! -f "$source" ]; then
        echo "error: Missing iOS bundle input: $source" >&2
        exit 1
    fi
done

mkdir -p "$RESOURCES_DIR"
rm -rf "$RESOURCES_DIR/rime-data" "$RESOURCES_DIR/addon-schemas"
ditto "$RIME_DATA_DIR" "$RESOURCES_DIR/rime-data"
printf '%s' "$core_version" > "$RESOURCES_DIR/keytao-core-version"
if [ "$PRODUCT_NAME" = "KeyTaoKeyboard" ]; then
    cp "$CONFIG_DIR/keytao_ios_ime.json" "$CONFIG_DIR/keytao-logo.png" "$RESOURCES_DIR/"
else
    cp "$KEYBOARD_YAML" "$RESOURCES_DIR/default-keyboard.yaml"
fi
