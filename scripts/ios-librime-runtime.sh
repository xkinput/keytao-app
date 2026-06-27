#!/usr/bin/env bash
# Manage iOS librime runtime files for KeyTao.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_RIME_ROOT="${KEYTAO_IOS_VENDOR_ROOT:-$PROJECT_DIR/vendor/librime/ios}"
STAGE_ROOT="${KEYTAO_IOS_STAGE_ROOT:-$PROJECT_DIR/target/keytao-ios-runtime}"
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)

usage() {
    cat <<EOF
Usage: $0 COMMAND [options]

Commands:
  import-sdk --target TARGET --source DIR
      Import an iOS librime SDK. DIR may contain include/, lib/librime.a,
      rime-data/, librime-release.txt, and opencc-release.txt. The librime
      archive must include the merged librime-lua module.

  verify --target TARGET|--all
      Verify imported runtime layout under vendor/librime/ios/<runtime>.

  env --target TARGET
      Print shell exports for building Rust against one iOS target.

  stage --target TARGET|--all
      Copy imported runtime files into target/keytao-ios-runtime/<runtime>.

Options:
  --runtime-root DIR   Override vendor runtime root. Defaults to vendor/librime/ios.
  --stage-root DIR     Override staged runtime root. Defaults to target/keytao-ios-runtime.
  -h, --help           Show this help.

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

abs_path() {
    local path="$1"
    mkdir -p "$(dirname "$path")"
    printf '%s/%s\n' "$(cd "$(dirname "$path")" && pwd)" "$(basename "$path")"
}

copy_dir_contents() {
    local source="$1"
    local destination="$2"
    [ -d "$source" ] || die "missing directory: $source"
    mkdir -p "$destination"
    cp -R "$source"/. "$destination"/
}

find_keytao_ios_package_dir() {
    local candidates=()
    if [ -n "${KEYTAO_IOS_RIME_PACKAGE_DIR:-}" ]; then
        candidates+=("$KEYTAO_IOS_RIME_PACKAGE_DIR")
    fi
    candidates+=(
        "$PROJECT_DIR/../KeyTao/release/keytao-ios"
        "$PROJECT_DIR/KeyTao/release/keytao-ios"
        "$PROJECT_DIR/release/keytao-ios"
    )

    local candidate
    for candidate in "${candidates[@]}"; do
        if [ -f "$candidate/keytao.schema.yaml" ] &&
           [ -f "$candidate/keytao-cx.dict.yaml" ] &&
           [ -f "$candidate/rime.lua" ]; then
            abs_path "$candidate"
            return 0
        fi
    done
    return 1
}

overlay_keytao_ios_package() {
    local destination="$1"
    local package_dir
    package_dir="$(find_keytao_ios_package_dir || true)"
    if [ -z "$package_dir" ]; then
        note "No KeyTao iOS Rime package found; set KEYTAO_IOS_RIME_PACKAGE_DIR to bundle schemas and Lua"
        return 0
    fi

    note "Overlaying KeyTao iOS Rime package from $package_dir"
    mkdir -p "$destination/rime-data"
    copy_dir_contents "$package_dir" "$destination/rime-data"
}

runtime_dir_for_target() {
    printf '%s/%s\n' "$IOS_RIME_ROOT" "$(target_to_runtime "$1")"
}

stage_dir_for_target() {
    printf '%s/%s\n' "$STAGE_ROOT" "$(target_to_runtime "$1")"
}

find_include_dir() {
    local source="$1"
    for dir in "$source/include" "$source/Headers" "$source/librime/include"; do
        if [ -f "$dir/rime_api.h" ]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

find_librime_archive() {
    local source="$1"
    for file in "$source/lib/librime.a" "$source/librime.a" "$source/lib/libRime.a"; do
        if [ -f "$file" ]; then
            printf '%s\n' "$file"
            return 0
        fi
    done
    return 1
}

find_rime_data_dir() {
    local source="$1"
    for dir in "$source/rime-data" "$source/share/rime-data" "$source/assets/rime-data"; do
        if [ -f "$dir/default.yaml" ]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

archive_has_lua_module() {
    local archive="$1"
    if command -v nm >/dev/null 2>&1; then
        local symbols
        symbols="$(nm "$archive" 2>/dev/null || true)"
        if grep -q 'rime_require_module_lua' <<<"$symbols" &&
            grep -Eq 'rime_lua_initialize|rime_lua_finalize|LuaProcessor|LuaTranslator' <<<"$symbols"; then
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

verify_lua_module() {
    local archive="$1"
    archive_has_lua_module "$archive" || die "missing merged librime-lua module in $archive; rebuild the iOS SDK with scripts/build-ios-librime.sh"
}

write_env_file() {
    local target="$1"
    local destination="$2"
    local sdk
    sdk="$(sdk_for_target "$target")"
    cat > "$destination/env.sh" <<EOF
export KEYTAO_IOS_RIME_ROOT="$destination"
export RIME_INCLUDE_DIR="$destination/include"
export RIME_LIB_DIR="$destination/lib"
export KEYTAO_RIME_SHARED_DATA_DIR="$destination/rime-data"
export RIME_SHARED_DATA_DIR="$destination/rime-data"
export SDKROOT="\$(xcrun --sdk $sdk --show-sdk-path)"
export BINDGEN_EXTRA_CLANG_ARGS="\${BINDGEN_EXTRA_CLANG_ARGS:-} -I$destination/include -isysroot \$SDKROOT"
EOF
}

import_sdk() {
    local target="$1"
    local source="$2"
    [ -d "$source" ] || die "missing source directory: $source"
    source="$(abs_path "$source")"

    local include_dir archive data_dir destination
    include_dir="$(find_include_dir "$source" || true)"
    archive="$(find_librime_archive "$source" || true)"
    data_dir="$(find_rime_data_dir "$source" || true)"
    [ -n "$include_dir" ] || die "cannot find include/rime_api.h in $source"
    [ -n "$archive" ] || die "cannot find lib/librime.a in $source"
    verify_lua_module "$archive"

    destination="$(runtime_dir_for_target "$target")"
    note "Importing iOS librime runtime for $target into $destination"
    rm -rf "$destination"
    mkdir -p "$destination/include" "$destination/lib"
    copy_dir_contents "$include_dir" "$destination/include"
    cp "$archive" "$destination/lib/librime.a"
    if [ -d "$source/lib" ]; then
        find "$source/lib" -maxdepth 1 -type f -name '*.a' ! -name 'librime.a' -exec cp {} "$destination/lib/" \;
    fi
    if [ -n "$data_dir" ]; then
        copy_dir_contents "$data_dir" "$destination/rime-data"
    else
        mkdir -p "$destination/rime-data"
        "$PROJECT_DIR/scripts/fetch-librime.sh" --platform ios --destination "$destination/.rime-data-bootstrap" >/dev/null
        copy_dir_contents "$destination/.rime-data-bootstrap/rime-data" "$destination/rime-data"
        rm -rf "$destination/.rime-data-bootstrap"
    fi
    overlay_keytao_ios_package "$destination"
    for metadata in librime-release.txt opencc-release.txt; do
        if [ -f "$source/$metadata" ]; then
            cp "$source/$metadata" "$destination/$metadata"
        fi
    done
    write_env_file "$target" "$destination"
    verify_one "$target"
}

verify_runtime_layout() {
    local destination="$1"
    [ -f "$destination/include/rime_api.h" ] || die "missing $destination/include/rime_api.h"
    [ -f "$destination/lib/librime.a" ] || die "missing $destination/lib/librime.a"
    verify_lua_module "$destination/lib/librime.a"
    [ -f "$destination/rime-data/default.yaml" ] || die "missing $destination/rime-data/default.yaml"
}

verify_one() {
    local target="$1"
    local destination
    destination="$(runtime_dir_for_target "$target")"
    verify_runtime_layout "$destination"
    if find_keytao_ios_package_dir >/dev/null; then
        [ -f "$destination/rime-data/keytao.schema.yaml" ] || die "missing $destination/rime-data/keytao.schema.yaml"
        [ -f "$destination/rime-data/keytao-cx.dict.yaml" ] || die "missing $destination/rime-data/keytao-cx.dict.yaml"
        [ -f "$destination/rime-data/rime.lua" ] || die "missing $destination/rime-data/rime.lua"
        [ -f "$destination/rime-data/lua/keytao_filter.lua" ] || die "missing $destination/rime-data/lua/keytao_filter.lua"
        [ -f "$destination/rime-data/lua/for_topup.lua" ] || die "missing $destination/rime-data/lua/for_topup.lua"
    fi
    note "Verified iOS librime runtime: $destination"
}

stage_one() {
    local target="$1"
    local source destination
    source="$(runtime_dir_for_target "$target")"
    destination="$(stage_dir_for_target "$target")"
    verify_runtime_layout "$source"
    note "Staging iOS librime runtime for $target into $destination"
    rm -rf "$destination"
    mkdir -p "$destination"
    copy_dir_contents "$source" "$destination"
    overlay_keytao_ios_package "$destination"
    verify_runtime_layout "$destination"
}

print_env() {
    local target="$1"
    local destination
    destination="$(runtime_dir_for_target "$target")"
    verify_one "$target" >/dev/null
    cat "$destination/env.sh"
}

command="${1:-}"
[ -n "$command" ] || { usage; exit 1; }
shift || true

target=""
source_dir=""
all_targets=false

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            target="${2:-}"
            [ -n "$target" ] || die "--target requires a value"
            shift 2
            ;;
        --source)
            source_dir="${2:-}"
            [ -n "$source_dir" ] || die "--source requires a value"
            shift 2
            ;;
        --all)
            all_targets=true
            shift
            ;;
        --runtime-root)
            IOS_RIME_ROOT="$(abs_path "${2:-}")"
            shift 2
            ;;
        --stage-root)
            STAGE_ROOT="$(abs_path "${2:-}")"
            shift 2
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

selected_targets=()
if [ "$all_targets" = true ]; then
    selected_targets=("${TARGETS[@]}")
elif [ -n "$target" ]; then
    selected_targets=("$target")
else
    die "provide --target TARGET or --all"
fi

case "$command" in
    import-sdk)
        [ "${#selected_targets[@]}" -eq 1 ] || die "import-sdk accepts one --target"
        [ -n "$source_dir" ] || die "import-sdk requires --source DIR"
        import_sdk "${selected_targets[0]}" "$source_dir"
        ;;
    verify)
        for selected in "${selected_targets[@]}"; do
            verify_one "$selected"
        done
        ;;
    env)
        [ "${#selected_targets[@]}" -eq 1 ] || die "env accepts one --target"
        print_env "${selected_targets[0]}"
        ;;
    stage)
        for selected in "${selected_targets[@]}"; do
            stage_one "$selected"
        done
        ;;
    *)
        usage
        die "unknown command: $command"
        ;;
esac
