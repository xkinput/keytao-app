#!/usr/bin/env bash
set -euo pipefail

IOS_IME_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="${KEYTAO_IOS_TYPECHECK_SDK:-iphoneos}"
TARGET="${KEYTAO_IOS_TYPECHECK_TARGET:-arm64-apple-ios15.0}"
LOG_FILE="$(mktemp "${TMPDIR:-/tmp}/keytao-ios-typecheck.XXXXXX")"
trap 'rm -f "$LOG_FILE"' EXIT

if ! command -v xcrun >/dev/null 2>&1; then
    echo '::error title=iOS SDK typecheck unavailable::xcrun and the Apple iOS SDK are required.'
    exit 1
fi
SDKROOT="$(xcrun --sdk "$SDK" --show-sdk-path)"

# A full UIKit source check without building or linking the Rust runtime.
set +e
xcrun --sdk "$SDK" swiftc \
    -typecheck \
    -application-extension \
    -swift-version 5 \
    -target "$TARGET" \
    -sdk "$SDKROOT" \
    -I "$IOS_IME_DIR/Sources/CKeytaoCore" \
    "$IOS_IME_DIR/Sources/KeyTaoIOSIME/"*.swift > "$LOG_FILE" 2>&1
result=$?
set -e
cat "$LOG_FILE"
if [ "$result" -ne 0 ]; then
    # Public GitHub annotations truncate individual messages at 4096 chars.
    # Publish bounded chunks with the actual error tail first.
    python3 - "$LOG_FILE" <<'PY'
from pathlib import Path
import sys

detail = Path(sys.argv[1]).read_text(errors="replace")[-20000:]
parts = [detail[max(0, end - 2500):end] for end in range(len(detail), 0, -2500)]
for index, part in enumerate(parts, 1):
    encoded = part.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::error title=iOS SDK typecheck detail {index}::{encoded}")
PY
fi
exit "$result"
