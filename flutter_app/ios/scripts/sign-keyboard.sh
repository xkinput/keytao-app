#!/usr/bin/env bash
set -euo pipefail

# Match the old simulator workaround without signing a --no-codesign device IPA.
if [ "${PLATFORM_NAME:-}" != "iphonesimulator" ] || [ "${CODE_SIGNING_ALLOWED:-YES}" = "NO" ]; then
    exit 0
fi
appex="${TARGET_BUILD_DIR:?}/${PLUGINS_FOLDER_PATH:?}/KeyTaoKeyboard.appex"
test -d "$appex"
/usr/bin/codesign --force --sign - \
    --entitlements "${SRCROOT:?}/KeyTaoKeyboard/KeyTaoKeyboardSimulator.entitlements" \
    --timestamp=none --generate-entitlement-der "$appex"
/usr/bin/codesign --verify --strict "$appex"
