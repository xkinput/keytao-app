#!/usr/bin/env bash
# Rebuild the test-only schema fixture. Usage: make-android-test-fixture.sh [output.zip]
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-$repo_root/flutter_app/android/app/src/androidTest/assets/keytao-schema-fixture.zip}"
mkdir -p "$(dirname "$output")"
output="$(cd "$(dirname "$output")" && pwd)/$(basename "$output")"

# Create a fresh archive so removed source files cannot linger after a refresh.
work_dir="$(mktemp -d "$(dirname "$output")/.keytao-schema-fixture.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT
cd "$repo_root/vendor/librime/ios/iphoneos-arm64/rime-data"
zip -qrX "$work_dir/fixture.zip" default.custom.yaml keytao*.yaml rime.lua symbols.yaml lua
mv "$work_dir/fixture.zip" "$output"
printf 'Created %s\n' "$output"
