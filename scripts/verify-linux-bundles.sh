#!/usr/bin/env bash
# Verify Linux release artifacts without installing them.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE_DIR="${1:-$PROJECT_DIR/target/release/bundle}"
RUNTIME_DIR="$PROJECT_DIR/target/keytao-linux-runtime"
RELEASE_RUNTIME_DIR="$PROJECT_DIR/target/release/runtime"

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $command_name" >&2
        exit 1
    fi
}

require_file() {
    local path="$1"
    if [ ! -f "$path" ]; then
        echo "ERROR: missing file: $path" >&2
        exit 1
    fi
}

require_executable() {
    local path="$1"
    if [ ! -x "$path" ]; then
        echo "ERROR: missing executable: $path" >&2
        exit 1
    fi
}

require_glob() {
    local pattern="$1"
    if ! compgen -G "$pattern" >/dev/null; then
        echo "ERROR: missing match: $pattern" >&2
        exit 1
    fi
}

find_one() {
    local result_var="$1"
    local dir="$2"
    local pattern="$3"
    local label="$4"
    local path
    path="$(find "$dir" -maxdepth 2 -name "$pattern" -type f -print -quit 2>/dev/null || true)"
    if [ -z "$path" ]; then
        echo "ERROR: missing $label in $dir" >&2
        find "$dir" -maxdepth 4 -type f -print 2>/dev/null | sort >&2 || true
        exit 1
    fi
    printf -v "$result_var" '%s' "$path"
}

require_listing_match() {
    local label="$1"
    local listing="$2"
    local pattern="$3"
    if ! printf '%s\n' "$listing" | grep -Eq "$pattern"; then
        echo "ERROR: $label is missing pattern: $pattern" >&2
        exit 1
    fi
}

require_runtime_listing() {
    local label="$1"
    local listing="$2"
    require_listing_match "$label" "$listing" '(^|/)keytao-app$'
    require_listing_match "$label" "$listing" '(^|/)keytao-ime(-x86_64-unknown-linux-gnu)?$'
    require_listing_match "$label" "$listing" '(^|/)runtime/rime-data/default\.yaml$'
    require_listing_match "$label" "$listing" '(^|/)runtime/rime-data/opencc/'
    require_listing_match "$label" "$listing" '(^|/)runtime/lib/librime[^/]*\.so'
    require_listing_match "$label" "$listing" '(^|/)runtime/lib/rime-plugins/librime-lua\.so'
}

require_command dpkg-deb
require_command rpm

echo "==> Verifying Linux runtime directory"
require_executable "$PROJECT_DIR/target/release/keytao-app"
require_executable "$PROJECT_DIR/target/release/keytao-ime"
require_file "$RUNTIME_DIR/rime-data/default.yaml"
require_glob "$RUNTIME_DIR/lib/librime*.so*"
require_glob "$RUNTIME_DIR/lib/rime-plugins/librime-lua.so*"
require_file "$RELEASE_RUNTIME_DIR/rime-data/default.yaml"
require_glob "$RELEASE_RUNTIME_DIR/lib/librime*.so*"
require_glob "$RELEASE_RUNTIME_DIR/lib/rime-plugins/librime-lua.so*"

if command -v patchelf >/dev/null 2>&1; then
    keytao_app_rpath="$(patchelf --print-rpath "$PROJECT_DIR/target/release/keytao-app" 2>/dev/null || true)"
    keytao_ime_rpath="$(patchelf --print-rpath "$PROJECT_DIR/target/release/keytao-ime" 2>/dev/null || true)"
    require_listing_match "keytao-app rpath" "$keytao_app_rpath" 'runtime/lib'
    require_listing_match "keytao-ime rpath" "$keytao_ime_rpath" 'runtime/lib'
fi

echo "==> Verifying Linux deb/rpm bundles"
forbidden_artifacts="$(find "$BUNDLE_DIR" -maxdepth 2 -type f \( -iname '*.appimage' -o -name '*.tar.gz' \) -print 2>/dev/null | sort || true)"
if [ -n "$forbidden_artifacts" ]; then
    echo "ERROR: Linux release must only produce deb/rpm, but found:" >&2
    printf '%s\n' "$forbidden_artifacts" >&2
    exit 1
fi
find_one deb "$BUNDLE_DIR/deb" '*.deb' 'deb bundle'
find_one rpm_pkg "$BUNDLE_DIR/rpm" '*.rpm' 'rpm bundle'
require_runtime_listing "deb bundle" "$(dpkg-deb -c "$deb")"
require_runtime_listing "rpm bundle" "$(rpm -qpl "$rpm_pkg")"

echo "==> Linux bundle verification passed"
