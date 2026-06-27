#!/usr/bin/env bash
# Build and stage the KeyTao C FFI static library for iOS targets.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_ROOT="${KEYTAO_IOS_STAGE_ROOT:-$PROJECT_DIR/target/keytao-ios-runtime}"
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --target TARGET       Build one iOS Rust target.
  --all                 Build all supported iOS Rust targets.
  --release|--debug     Cargo profile. Defaults to release.
  --allow-missing       Skip targets without an imported iOS librime runtime.
  -h, --help            Show this help.

Targets:
  aarch64-apple-ios
  aarch64-apple-ios-sim
  x86_64-apple-ios
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

note() {
    echo "==> $*"
}

target_to_runtime() {
    case "$1" in
        aarch64-apple-ios) echo "iphoneos-arm64" ;;
        aarch64-apple-ios-sim) echo "iphonesimulator-arm64" ;;
        x86_64-apple-ios) echo "iphonesimulator-x86_64" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

sdk_for_target() {
    case "$1" in
        aarch64-apple-ios) echo "iphoneos" ;;
        aarch64-apple-ios-sim|x86_64-apple-ios) echo "iphonesimulator" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

runtime_root_for_target() {
    local target="$1"
    local runtime
    runtime="$(target_to_runtime "$target")"
    if [ -n "${KEYTAO_IOS_RIME_ROOT:-}" ] && [ -f "$KEYTAO_IOS_RIME_ROOT/include/rime_api.h" ]; then
        printf '%s\n' "$KEYTAO_IOS_RIME_ROOT"
        return 0
    fi
    printf '%s/vendor/librime/ios/%s\n' "$PROJECT_DIR" "$runtime"
}

archive_has_lua_module() {
    local archive="$1"
    if command -v nm >/dev/null 2>&1; then
        local symbols
        symbols="$(nm "$archive" 2>/dev/null || true)"
        if grep -Eq 'rime_lua_initialize|rime_lua_finalize|LuaProcessor|LuaTranslator' <<<"$symbols"; then
            return 0
        fi
    fi
    if command -v strings >/dev/null 2>&1; then
        local raw_strings
        raw_strings="$(strings "$archive" 2>/dev/null || true)"
        if grep -q 'lua_processor' <<<"$raw_strings" &&
            grep -q 'lua_translator' <<<"$raw_strings"; then
            return 0
        fi
    fi
    return 1
}

stage_runtime() {
    local target="$1"
    "$PROJECT_DIR/scripts/ios-librime-runtime.sh" stage --target "$target" --stage-root "$STAGE_ROOT"
}

build_one() {
    local target="$1"
    local runtime source_lib sdk sdkroot runtime_name stage_dir profile_flag profile_dir
    runtime="$(runtime_root_for_target "$target")"
    if [ ! -f "$runtime/include/rime_api.h" ] || [ ! -f "$runtime/lib/librime.a" ]; then
        if [ "$ALLOW_MISSING" = true ]; then
            note "Skipping $target: no iOS librime runtime at $runtime"
            return 0
        fi
        die "missing iOS librime runtime for $target at $runtime; import it with scripts/ios-librime-runtime.sh import-sdk"
    fi
    if ! archive_has_lua_module "$runtime/lib/librime.a"; then
        die "iOS librime runtime for $target is missing the merged librime-lua module; rebuild it with scripts/build-ios-librime.sh --target $target"
    fi

    runtime_name="$(target_to_runtime "$target")"
    sdk="$(sdk_for_target "$target")"
    sdkroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
    profile_flag="--release"
    profile_dir="release"
    if [ "$PROFILE" = "debug" ]; then
        profile_flag=""
        profile_dir="debug"
    fi

    note "Building keytao-core-ffi for $target"
    rustup target add "$target" >/dev/null
    (
        cd "$PROJECT_DIR"
        KEYTAO_IOS_RIME_ROOT="$runtime" \
        RIME_INCLUDE_DIR="$runtime/include" \
        RIME_LIB_DIR="$runtime/lib" \
        KEYTAO_RIME_SHARED_DATA_DIR="$runtime/rime-data" \
        RIME_SHARED_DATA_DIR="$runtime/rime-data" \
        SDKROOT="$sdkroot" \
        IPHONEOS_DEPLOYMENT_TARGET="${IOS_DEPLOYMENT_TARGET:-15.0}" \
        BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} -I$runtime/include -isysroot $sdkroot" \
        cargo rustc -p keytao-core-ffi --target "$target" $profile_flag --lib -- --crate-type staticlib
    )

    source_lib="$PROJECT_DIR/target/$target/$profile_dir/libkeytao_core_ffi.a"
    [ -f "$source_lib" ] || die "missing built static library: $source_lib"
    stage_runtime "$target"
    stage_dir="$STAGE_ROOT/$runtime_name"
    mkdir -p "$stage_dir/lib" "$stage_dir/include"
    cp "$source_lib" "$stage_dir/lib/libkeytao_core_ffi.a"
    cp "$PROJECT_DIR/crates/keytao-core-ffi/include/keytao_core.h" "$stage_dir/include/keytao_core.h"
    note "Staged iOS FFI runtime: $stage_dir"
}

PROFILE="release"
ALLOW_MISSING=false
target=""
all_targets=false

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            target="${2:-}"
            [ -n "$target" ] || die "--target requires a value"
            shift 2
            ;;
        --all)
            all_targets=true
            shift
            ;;
        --release)
            PROFILE="release"
            shift
            ;;
        --debug)
            PROFILE="debug"
            shift
            ;;
        --allow-missing)
            ALLOW_MISSING=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

if ! command -v xcrun >/dev/null 2>&1; then
    die "xcrun is required to build iOS targets"
fi

selected_targets=()
if [ "$all_targets" = true ]; then
    selected_targets=("${TARGETS[@]}")
elif [ -n "$target" ]; then
    selected_targets=("$target")
else
    selected_targets=(aarch64-apple-ios)
fi

for selected in "${selected_targets[@]}"; do
    build_one "$selected"
done
