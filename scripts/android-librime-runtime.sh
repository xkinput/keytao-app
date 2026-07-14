#!/usr/bin/env bash
# Manage Android ABI librime runtime files for KeyTao.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_RIME_ROOT="$PROJECT_DIR/vendor/librime/android"
ANDROID_APP_DIR="$PROJECT_DIR/src-tauri/gen/android/app"
ABIS=(arm64-v8a armeabi-v7a x86 x86_64)
LIBRIME_HEADERS_VERSION="${KEYTAO_ANDROID_LIBRIME_HEADERS_VERSION:-1.17.0}"
FCITX5_RIME_VERSION="${KEYTAO_ANDROID_FCITX5_RIME_VERSION:-latest}"
CURL_RETRY_ARGS=(--retry 5 --retry-delay 2 --retry-all-errors)

usage() {
    cat <<EOF
Usage: $0 COMMAND [options]

Commands:
  import-sdk --abi ABI --source DIR
      Import an Android ABI librime SDK. DIR may contain include/, lib/, and rime-data/.

  import-apk --abi ABI --apk FILE --include-dir DIR [--rime-data DIR] [--dependency-apk FILE]
      Import native .so files from an Android APK. The APK must contain a pure
      lib/<abi>/librime.so, not an input-method JNI wrapper.

  import-fcitx5-rime --abi ABI [--version VERSION] [--include-dir DIR] [--rime-data DIR]
      Download the Fcitx5 Android Rime plugin APK for ABI and import its pure
      Android librime.so. VERSION defaults to latest. Headers default to
      vendor/librime/macos-universal/include,
      then a cached rime/librime source-header download.

  sync [--abi ABI|--all] [--allow-missing]
      Copy vendor/librime/android/<abi>/lib/*.so into Android jniLibs and copy rime-data
      into src/main/assets/keytao-rime-data.

  verify [--abi ABI|--all]
      Verify imported runtime layout.

  env --abi ABI
      Print shell exports for building Rust against one Android ABI.

Options:
  --android-app-dir DIR     Override Android app dir. Defaults to src-tauri/gen/android/app.
  --runtime-root DIR        Override Android librime vendor root. Defaults to vendor/librime/android.
  -h, --help                Show this help.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

note() {
    echo "==> $*"
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

is_abi() {
    local abi="$1"
    case "$abi" in
        arm64-v8a|armeabi-v7a|x86|x86_64) return 0 ;;
        *) return 1 ;;
    esac
}

target_for_abi() {
    case "$1" in
        arm64-v8a) echo "aarch64-linux-android" ;;
        armeabi-v7a) echo "armv7-linux-androideabi" ;;
        x86) echo "i686-linux-android" ;;
        x86_64) echo "x86_64-linux-android" ;;
        *) die "unsupported Android ABI: $1" ;;
    esac
}

clang_target_for_abi() {
    case "$1" in
        arm64-v8a) echo "aarch64-linux-android24" ;;
        armeabi-v7a) echo "armv7a-linux-androideabi24" ;;
        x86) echo "i686-linux-android24" ;;
        x86_64) echo "x86_64-linux-android24" ;;
        *) die "unsupported Android ABI: $1" ;;
    esac
}

host_tag() {
    case "$(uname -s)" in
        Darwin) echo "darwin-x86_64" ;;
        Linux) echo "linux-x86_64" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows-x86_64" ;;
        *) return 1 ;;
    esac
}

find_ndk_sysroot() {
    local root
    root="$(find_ndk_root || true)"
    if [ -n "$root" ] && [ -d "$root/toolchains/llvm/prebuilt/$(host_tag)/sysroot" ]; then
        printf '%s\n' "$root/toolchains/llvm/prebuilt/$(host_tag)/sysroot"
        return 0
    fi
    return 1
}

find_readelf() {
    if command_exists readelf; then
        command -v readelf
        return 0
    fi
    local root
    root="$(find_ndk_root || true)"
    if [ -n "$root" ]; then
        local tool="$root/toolchains/llvm/prebuilt/$(host_tag)/bin/llvm-readelf"
        if [ -x "$tool" ]; then
            printf '%s\n' "$tool"
            return 0
        fi
    fi
    return 1
}

find_ndk_root() {
    local root
    for key in ANDROID_NDK_HOME ANDROID_NDK_ROOT NDK_HOME; do
        root="${!key:-}"
        if [ -n "$root" ] && [ -d "$root/toolchains/llvm/prebuilt/$(host_tag)" ]; then
            printf '%s\n' "$root"
            return 0
        fi
    done
    if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
        local ndk
        ndk="$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1 || true)"
        if [ -n "$ndk" ] && [ -d "$ndk/toolchains/llvm/prebuilt/$(host_tag)" ]; then
            printf '%s\n' "$ndk"
            return 0
        fi
    fi
    return 1
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

has_glob() {
    compgen -G "$1" >/dev/null 2>&1
}

normalize_version_tag() {
    local tag="$1"
    tag="${tag#ver.}"
    tag="${tag#v}"
    printf '%s\n' "$tag"
}

write_librime_metadata() {
    local destination="$1"
    local version="$2"
    local source="$3"
    version="$(normalize_version_tag "$version")"
    [ -n "$version" ] || return 0
    cat > "$destination/librime-release.txt" <<EOF
platform=android
version=$version
source=$source
EOF
}

write_opencc_metadata() {
    local destination="$1"
    local version="$2"
    local source="$3"
    version="$(normalize_version_tag "$version")"
    [ -n "$version" ] || return 0
    cat > "$destination/opencc-release.txt" <<EOF
version=$version
source=$source
EOF
}

metadata_root_for_data_dir() {
    local data_dir="$1"
    dirname "$(dirname "$data_dir")"
}

copy_opencc_metadata_for_data_dir() {
    local source_data_dir="$1"
    local destination_root="$2"
    local source_root
    source_root="$(metadata_root_for_data_dir "$source_data_dir")"
    if [ -f "$source_root/opencc-release.txt" ]; then
        cp "$source_root/opencc-release.txt" "$destination_root/opencc-release.txt"
        return 0
    fi
    return 1
}

copy_runtime_metadata_from_source() {
    local source="$1"
    local destination="$2"
    if [ -f "$source/librime-release.txt" ]; then
        cp "$source/librime-release.txt" "$destination/librime-release.txt"
    fi
    if [ -f "$source/opencc-release.txt" ]; then
        cp "$source/opencc-release.txt" "$destination/opencc-release.txt"
    fi
}

ensure_android_metadata() {
    local destination="$1"
    local source="$2"
    if [ ! -f "$destination/librime-release.txt" ]; then
        write_librime_metadata "$destination" "$LIBRIME_HEADERS_VERSION" "$source"
    fi
    if [ ! -f "$destination/opencc-release.txt" ]; then
        for source_data in \
            "$PROJECT_DIR/vendor/librime/macos-universal/rime-data/opencc" \
            "$PROJECT_DIR/vendor/librime/android-data/rime-data/opencc"; do
            if [ -d "$source_data" ] && copy_opencc_metadata_for_data_dir "$source_data" "$destination"; then
                break
            fi
        done
    fi
}

contains_elf_string() {
    local file="$1"
    local pattern="$2"
    strings "$file" 2>/dev/null | grep -Eq "$pattern"
}

contains_dynamic_symbol() {
    local file="$1"
    local pattern="$2"
    if command_exists nm; then
        nm -D "$file" 2>/dev/null | grep -Eq "$pattern"
        return $?
    fi
    return 1
}

validate_librime_so() {
    local lib="$1"
    [ -f "$lib" ] || die "missing librime.so: $lib"
    if contains_dynamic_symbol "$lib" '(^|[[:space:]])(rime_get_api|rime_get_api_stdbool)($|[[:space:]])' ||
        contains_elf_string "$lib" '(^|[^A-Za-z0-9_])rime_get_api([^A-Za-z0-9_]|$)'; then
        :
    else
        die "librime.so does not export rime_get_api: $lib"
    fi

    if contains_dynamic_symbol "$lib" '(^|[[:space:]])(JNI_OnLoad|Java_)' ||
        contains_elf_string "$lib" 'JNI_OnLoad|Java_com_osfans_trime|com/osfans/trime|com\.osfans\.trime'; then
        die "librime.so looks like an input-method JNI wrapper, not a pure librime library: $lib"
    fi
}

validate_runtime_lib_dir() {
    local lib_dir="$1"
    local abi="${2:-}"
    validate_librime_so "$lib_dir/librime.so"
    if [ -f "$lib_dir/librime_jni.so" ]; then
        die "refusing librime_jni.so in Android runtime; import a pure librime.so instead: $lib_dir/librime_jni.so"
    fi
    local lib
    while IFS= read -r -d '' lib; do
        if contains_dynamic_symbol "$lib" '(^|[[:space:]])(JNI_OnLoad|Java_)' ||
            contains_elf_string "$lib" 'JNI_OnLoad|Java_com_osfans_trime|com/osfans/trime|com\.osfans\.trime'; then
            die "refusing JNI wrapper library in Android runtime: $lib"
        fi
    done < <(find "$lib_dir" -maxdepth 1 -type f -name '*.so' -print0)
    if [ -n "$abi" ]; then
        ensure_needed_closure "$lib_dir" "$abi"
    fi
}

system_needed_lib() {
    case "$1" in
        libandroid.so|libc.so|libdl.so|libEGL.so|libGLESv1_CM.so|libGLESv2.so|libGLESv3.so|libjnigraphics.so|liblog.so|libm.so|libOpenSLES.so|libz.so)
            return 0
            ;;
        *) return 1 ;;
    esac
}

elf_needed_libs() {
    local lib="$1"
    local reader
    reader="$(find_readelf || true)"
    [ -n "$reader" ] || die "readelf or Android NDK llvm-readelf is required to verify Android native dependency closure"
    "$reader" -d "$lib" 2>/dev/null |
        sed -n 's/.*Shared library: \[\([^]]*\)\].*/\1/p'
}

ndk_cxx_shared_for_abi() {
    local abi="$1"
    local root
    root="$(find_ndk_root || true)"
    [ -n "$root" ] || return 1
    local triple
    case "$abi" in
        arm64-v8a) triple="aarch64-linux-android" ;;
        armeabi-v7a) triple="arm-linux-androideabi" ;;
        x86) triple="i686-linux-android" ;;
        x86_64) triple="x86_64-linux-android" ;;
        *) return 1 ;;
    esac
    local lib="$root/toolchains/llvm/prebuilt/$(host_tag)/sysroot/usr/lib/$triple/libc++_shared.so"
    [ -f "$lib" ] || return 1
    printf '%s\n' "$lib"
}

resolve_needed_closure() {
    local lib_dir="$1"
    local abi="$2"
    shift 2
    local source_dirs=("$@")
    local changed=1
    while [ "$changed" -eq 1 ]; do
        changed=0
        local lib
        while IFS= read -r -d '' lib; do
            local needed
            while IFS= read -r needed; do
                [ -n "$needed" ] || continue
                system_needed_lib "$needed" && continue
                [ -f "$lib_dir/$needed" ] && continue
                local found=""
                local source
                for source in "${source_dirs[@]}"; do
                    if [ -f "$source/$needed" ]; then
                        found="$source/$needed"
                        break
                    fi
                done
                if [ -z "$found" ] && [ "$needed" = "libc++_shared.so" ]; then
                    found="$(ndk_cxx_shared_for_abi "$abi" || true)"
                fi
                [ -n "$found" ] || die "$(basename "$lib") needs $needed, but it is missing from Android runtime closure"
                cp -f "$found" "$lib_dir/$needed"
                validate_runtime_lib_file "$lib_dir/$needed"
                note "Added Android native dependency $needed"
                changed=1
            done < <(elf_needed_libs "$lib")
        done < <(find "$lib_dir" -maxdepth 1 -type f -name '*.so' -print0)
    done
}

ensure_needed_closure() {
    local lib_dir="$1"
    local abi="$2"
    local lib
    while IFS= read -r -d '' lib; do
        local needed
        while IFS= read -r needed; do
            [ -n "$needed" ] || continue
            system_needed_lib "$needed" && continue
            [ -f "$lib_dir/$needed" ] || die "$(basename "$lib") needs $needed, but $lib_dir/$needed is missing"
        done < <(elf_needed_libs "$lib")
    done < <(find "$lib_dir" -maxdepth 1 -type f -name '*.so' -print0)
}

validate_runtime_lib_file() {
    local lib="$1"
    if contains_dynamic_symbol "$lib" '(^|[[:space:]])(JNI_OnLoad|Java_)' ||
        contains_elf_string "$lib" 'JNI_OnLoad|Java_com_osfans_trime|com/osfans/trime|com\.osfans\.trime'; then
        die "refusing JNI wrapper library in Android runtime: $lib"
    fi
}

copy_opencc_data_if_available() {
    local data_dir="$1"
    if has_glob "$data_dir/opencc/*.ocd2"; then
        return 0
    fi
    local source
    for source in \
        "$PROJECT_DIR/vendor/librime/macos-universal/rime-data/opencc" \
        "$PROJECT_DIR/vendor/librime/android-data/rime-data/opencc"; do
        if [ -d "$source" ] && has_glob "$source/*.ocd2"; then
            mkdir -p "$data_dir/opencc"
            copy_dir_contents "$source" "$data_dir/opencc"
            note "Completed Android rime-data opencc files from $source"
            return 0
        fi
    done
}

ensure_rime_data_closure() {
    local data_dir="$1"
    [ -f "$data_dir/default.yaml" ] || die "rime-data is missing default.yaml: $data_dir"
    copy_opencc_data_if_available "$data_dir"
    [ -d "$data_dir/opencc" ] || die "rime-data is missing opencc data: $data_dir/opencc"
    if ! has_glob "$data_dir/opencc/*.ocd2"; then
        die "rime-data/opencc is missing compiled .ocd2 dictionaries: $data_dir/opencc"
    fi
}

find_child_dir_with_file() {
    local root="$1"
    local file_name="$2"
    find "$root" -type f -name "$file_name" -print -quit | xargs -r dirname
}

default_include_dir() {
    local include_dir="$PROJECT_DIR/vendor/librime/macos-universal/include"
    if [ -f "$include_dir/rime_api.h" ]; then
        printf '%s\n' "$include_dir"
        return 0
    fi
    fetch_default_include_dir
}

fetch_default_include_dir() {
    local include_dir="$PROJECT_DIR/.cache/android-librime/headers/$LIBRIME_HEADERS_VERSION/include"
    local header
    mkdir -p "$include_dir"
    for header in rime_api.h rime_api_deprecated.h rime_api_stdbool.h rime_levers_api.h; do
        if [ ! -f "$include_dir/$header" ]; then
            local url="https://raw.githubusercontent.com/rime/librime/$LIBRIME_HEADERS_VERSION/src/$header"
            note "Downloading librime header $header from rime/librime $LIBRIME_HEADERS_VERSION" >&2
            local tmp="$include_dir/$header.tmp"
            rm -f "$tmp"
            if ! curl -fsSL "${CURL_RETRY_ARGS[@]}" \
                -H "User-Agent: keytao-android-runtime" "$url" -o "$tmp"; then
                [ -n "${GITHUB_TOKEN:-}" ] || return 1
                curl -fsSL "${CURL_RETRY_ARGS[@]}" \
                    -H "User-Agent: keytao-android-runtime" \
                    -H "Authorization: Bearer $GITHUB_TOKEN" "$url" -o "$tmp"
            fi
            mv "$tmp" "$include_dir/$header"
        fi
    done
    [ -f "$include_dir/rime_api.h" ] || die "failed to prepare librime headers"
    printf '%s\n' "$include_dir"
}

runtime_dir() {
    local abi="$1"
    printf '%s/%s\n' "$ANDROID_RIME_ROOT" "$abi"
}

runtime_staging_dir() {
    local abi="$1"
    mkdir -p "$ANDROID_RIME_ROOT"
    mktemp -d "$ANDROID_RIME_ROOT/.${abi}.XXXXXX"
}

write_env_file() {
    local abi="$1"
    local destination="$2"
    local target
    local clang_target
    local ndk_root
    local sysroot
    target="$(target_for_abi "$abi")"
    clang_target="$(clang_target_for_abi "$abi")"
    ndk_root="$(find_ndk_root || true)"
    sysroot="$(find_ndk_sysroot || true)"

    cat > "$destination/env.sh" <<EOF
export KEYTAO_ANDROID_RIME_ROOT="$destination"
export RIME_INCLUDE_DIR="$destination/include"
export RIME_LIB_DIR="$destination/lib"
export KEYTAO_RIME_SHARED_DATA_DIR="$destination/rime-data"
export RIME_SHARED_DATA_DIR="$destination/rime-data"
export CARGO_BUILD_TARGET="$target"
EOF
    if [ -n "$ndk_root" ]; then
        local llvm_bin="$ndk_root/toolchains/llvm/prebuilt/$(host_tag)/bin"
        local cargo_target
        cargo_target="$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')"
        cat >> "$destination/env.sh" <<EOF
export ANDROID_NDK_HOME="$ndk_root"
export CARGO_TARGET_${cargo_target}_LINKER="$llvm_bin/$clang_target-clang"
export CC_${cargo_target}="$llvm_bin/$clang_target-clang"
export CXX_${cargo_target}="$llvm_bin/$clang_target-clang++"
export AR_${cargo_target}="$llvm_bin/llvm-ar"
EOF
    fi
    if [ -n "$sysroot" ]; then
        cat >> "$destination/env.sh" <<EOF
export BINDGEN_EXTRA_CLANG_ARGS="\${BINDGEN_EXTRA_CLANG_ARGS:-} --target=$clang_target --sysroot=$sysroot -I$destination/include"
EOF
    else
        cat >> "$destination/env.sh" <<EOF
export BINDGEN_EXTRA_CLANG_ARGS="\${BINDGEN_EXTRA_CLANG_ARGS:-} --target=$clang_target -I$destination/include"
EOF
    fi
}

verify_one() {
    local abi="$1"
    local root
    root="$(runtime_dir "$abi")"
    [ -f "$root/include/rime_api.h" ] || die "$abi is missing include/rime_api.h"
    [ -f "$root/lib/librime.so" ] || die "$abi is missing lib/librime.so"
    [ -f "$root/rime-data/default.yaml" ] || die "$abi is missing rime-data/default.yaml"
    validate_runtime_lib_dir "$root/lib" "$abi"
    ensure_rime_data_closure "$root/rime-data"
}

import_sdk() {
    local abi=""
    local source=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) abi="${2:?missing value for --abi}"; shift 2 ;;
            --source) source="${2:?missing value for --source}"; shift 2 ;;
            *) die "unknown import-sdk option: $1" ;;
        esac
    done
    [ -n "$abi" ] || die "missing --abi"
    is_abi "$abi" || die "unsupported Android ABI: $abi"
    [ -n "$source" ] || die "missing --source"
    [ -d "$source" ] || die "source directory does not exist: $source"

    local include_dir="$source/include"
    local lib_dir="$source/lib"
    local data_dir="$source/rime-data"
    [ -f "$include_dir/rime_api.h" ] || include_dir="$(find_child_dir_with_file "$source" rime_api.h)"
    [ -n "$include_dir" ] || die "cannot find rime_api.h under $source"
    [ -f "$lib_dir/librime.so" ] || lib_dir="$(find_child_dir_with_file "$source" librime.so)"
    [ -n "$lib_dir" ] || die "cannot find librime.so under $source"
    [ -f "$data_dir/default.yaml" ] || data_dir="$(find_child_dir_with_file "$source" default.yaml)"

    local destination
    local staging
    destination="$(runtime_dir "$abi")"
    staging="$(runtime_staging_dir "$abi")"
    mkdir -p "$staging/include" "$staging/lib" "$staging/rime-data"
    copy_dir_contents "$include_dir" "$staging/include"
    copy_dir_contents "$lib_dir" "$staging/lib"
    copy_runtime_metadata_from_source "$source" "$staging"
    validate_runtime_lib_dir "$staging/lib" "$abi"
    if [ -n "$data_dir" ]; then
        copy_dir_contents "$data_dir" "$staging/rime-data"
        copy_opencc_metadata_for_data_dir "$data_dir/opencc" "$staging" || true
    else
        local data_tmp
        data_tmp="$(mktemp -d "${TMPDIR:-/tmp}/keytao-android-data.XXXXXX")"
        "$PROJECT_DIR/scripts/fetch-librime.sh" --platform android --destination "$data_tmp"
        copy_dir_contents "$data_tmp/rime-data" "$staging/rime-data"
        copy_opencc_metadata_for_data_dir "$data_tmp/rime-data/opencc" "$staging" || true
        rm -rf "$data_tmp"
    fi
    ensure_rime_data_closure "$staging/rime-data"
    ensure_android_metadata "$staging" "android-sdk:$source"
    rm -rf "$destination"
    mv "$staging" "$destination"
    write_env_file "$abi" "$destination"
    verify_one "$abi"
    note "Imported Android librime SDK for $abi into $destination"
}

import_apk() {
    local abi=""
    local apk=""
    local include_dir=""
    local data_dir=""
    local dependency_apks=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) abi="${2:?missing value for --abi}"; shift 2 ;;
            --apk) apk="${2:?missing value for --apk}"; shift 2 ;;
            --include-dir) include_dir="${2:?missing value for --include-dir}"; shift 2 ;;
            --rime-data) data_dir="${2:?missing value for --rime-data}"; shift 2 ;;
            --dependency-apk) dependency_apks+=("${2:?missing value for --dependency-apk}"); shift 2 ;;
            *) die "unknown import-apk option: $1" ;;
        esac
    done
    [ -n "$abi" ] || die "missing --abi"
    is_abi "$abi" || die "unsupported Android ABI: $abi"
    [ -n "$apk" ] || die "missing --apk"
    [ -f "$apk" ] || die "APK does not exist: $apk"
    local dependency_apk
    for dependency_apk in "${dependency_apks[@]}"; do
        [ -f "$dependency_apk" ] || die "dependency APK does not exist: $dependency_apk"
    done
    if [ -z "$include_dir" ]; then
        include_dir="$(default_include_dir || true)"
    fi
    [ -n "$include_dir" ] || die "missing --include-dir; APKs usually do not contain rime_api.h"
    [ -f "$include_dir/rime_api.h" ] || die "include dir is missing rime_api.h: $include_dir"

    local destination
    local staging
    local extract_dir
    destination="$(runtime_dir "$abi")"
    staging="$(runtime_staging_dir "$abi")"
    extract_dir="$(mktemp -d "${TMPDIR:-/tmp}/keytao-android-apk.XXXXXX")"
    unzip -q "$apk" "lib/$abi/*.so" -d "$extract_dir" || die "APK does not contain lib/$abi/*.so"
    local extracted_lib_dir="$extract_dir/lib/$abi"
    local dependency_lib_dirs=()
    local index=0
    for dependency_apk in "${dependency_apks[@]}"; do
        local dep_extract_dir="$extract_dir/dependency-$index"
        unzip -q "$dependency_apk" "lib/$abi/*.so" -d "$dep_extract_dir" || die "dependency APK does not contain lib/$abi/*.so: $dependency_apk"
        dependency_lib_dirs+=("$dep_extract_dir/lib/$abi")
        index=$((index + 1))
    done
    local apk_shared_dir=""
    if unzip -l "$apk" "assets/shared/default.yaml" >/dev/null 2>&1; then
        unzip -q "$apk" "assets/shared/*" -d "$extract_dir"
        apk_shared_dir="$extract_dir/assets/shared"
        note "Using shared rime-data from APK assets/shared"
    fi
    [ -f "$extracted_lib_dir/librime.so" ] || die "APK lib/$abi is missing pure librime.so; refusing librime_jni.so or input-method wrappers"
    validate_librime_so "$extracted_lib_dir/librime.so"

    mkdir -p "$staging/include" "$staging/lib" "$staging/rime-data"
    copy_dir_contents "$include_dir" "$staging/include"
    cp -f "$extracted_lib_dir/librime.so" "$staging/lib/librime.so"
    resolve_needed_closure "$staging/lib" "$abi" "$extracted_lib_dir" "${dependency_lib_dirs[@]}"
    validate_runtime_lib_dir "$staging/lib" "$abi"
    if [ -n "$data_dir" ]; then
        copy_dir_contents "$data_dir" "$staging/rime-data"
        copy_opencc_metadata_for_data_dir "$data_dir/opencc" "$staging" || true
    elif [ -n "$apk_shared_dir" ]; then
        copy_dir_contents "$apk_shared_dir" "$staging/rime-data"
        copy_opencc_metadata_for_data_dir "$apk_shared_dir/opencc" "$staging" || true
    else
        local data_tmp
        data_tmp="$(mktemp -d "${TMPDIR:-/tmp}/keytao-android-data.XXXXXX")"
        "$PROJECT_DIR/scripts/fetch-librime.sh" --platform android --destination "$data_tmp"
        copy_dir_contents "$data_tmp/rime-data" "$staging/rime-data"
        copy_opencc_metadata_for_data_dir "$data_tmp/rime-data/opencc" "$staging" || true
        rm -rf "$data_tmp"
    fi
    ensure_rime_data_closure "$staging/rime-data"
    ensure_android_metadata "$staging" "android-apk:$apk"
    rm -rf "$destination"
    mv "$staging" "$destination"
    write_env_file "$abi" "$destination"
    verify_one "$abi"
    rm -rf "$extract_dir"
    note "Imported Android pure librime runtime for $abi from $apk into $destination"
}

fcitx5_rime_assets() {
    local version="$1"
    local abi="$2"
    local release_url
    if [ "$version" = "latest" ]; then
        release_url="https://api.github.com/repos/fcitx5-android/fcitx5-android/releases/latest"
    else
        release_url="https://api.github.com/repos/fcitx5-android/fcitx5-android/releases/tags/$version"
    fi
    local release_json
    release_json="$(mktemp "${TMPDIR:-/tmp}/keytao-fcitx5-rime-release.XXXXXX")"
    curl -fsSL "${CURL_RETRY_ARGS[@]}" -H "User-Agent: keytao-android-runtime" "$release_url" -o "$release_json"
    python3 - "$release_json" "$version" "$abi" <<'PY'
import json
import sys

release_path, version, abi = sys.argv[1], sys.argv[2], sys.argv[3]
with open(release_path, "r", encoding="utf-8") as response:
    release = json.load(response)

plugin_asset = None
base_asset = None
abi_suffix = f"-{abi}-release.apk"
for item in release.get("assets", []):
    name = item.get("name", "")
    if (
        name.startswith("org.fcitx.fcitx5.android.plugin.rime-")
        and name.endswith(abi_suffix)
    ):
        plugin_asset = item
    if (
        name.startswith("org.fcitx.fcitx5.android-")
        and ".plugin." not in name
        and name.endswith(abi_suffix)
    ):
        base_asset = item
if not plugin_asset or not base_asset:
    names = ", ".join(item.get("name", "") for item in release.get("assets", []))
    raise SystemExit(f"missing Fcitx5 Android Rime/base APK for {abi} in {release.get('tag_name')}; assets: {names}")
print("\t".join([
    release.get("tag_name", version),
    plugin_asset.get("name", ""),
    plugin_asset.get("browser_download_url", ""),
    base_asset.get("name", ""),
    base_asset.get("browser_download_url", ""),
]))
PY
    rm -f "$release_json"
}

download_cached_apk() {
    local url="$1"
    local apk="$2"
    local label="$3"
    mkdir -p "$(dirname "$apk")"
    if [ -f "$apk" ] && unzip -tq "$apk" >/dev/null 2>&1; then
        note "Using cached $label"
        return
    fi
    if [ -f "$apk" ]; then
        note "Resuming incomplete cached $label"
    else
        note "Downloading $label"
    fi
    local attempt
    for attempt in $(seq 1 20); do
        if curl --http1.1 -fL -C - \
            --connect-timeout 30 \
            --speed-time 25 \
            --speed-limit 1024 \
            -H "User-Agent: keytao-android-runtime" \
            -o "$apk" \
            "$url"; then
            if unzip -tq "$apk" >/dev/null 2>&1; then
                return
            fi
        fi
        note "Retrying $label download ($attempt/20)"
        sleep 2
    done
    unzip -tq "$apk" >/dev/null
}

import_fcitx5_rime() {
    local abi=""
    local version="$FCITX5_RIME_VERSION"
    local include_dir=""
    local data_dir=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) abi="${2:?missing value for --abi}"; shift 2 ;;
            --version) version="${2:?missing value for --version}"; shift 2 ;;
            --include-dir) include_dir="${2:?missing value for --include-dir}"; shift 2 ;;
            --rime-data) data_dir="${2:?missing value for --rime-data}"; shift 2 ;;
            *) die "unknown import-fcitx5-rime option: $1" ;;
        esac
    done
    [ -n "$abi" ] || die "missing --abi"
    is_abi "$abi" || die "unsupported Android ABI: $abi"
    if [ -z "$include_dir" ]; then
        include_dir="$(default_include_dir || true)"
    fi
    [ -n "$include_dir" ] || die "missing --include-dir and no vendor/librime/macos-universal/include/rime_api.h found"

    local tag
    local asset_name
    local asset_url
    local base_asset_name
    local base_asset_url
    IFS=$'\t' read -r tag asset_name asset_url base_asset_name base_asset_url < <(fcitx5_rime_assets "$version" "$abi")
    local cache_dir="$PROJECT_DIR/.cache/android-librime/fcitx5-rime/$tag"
    local apk="$cache_dir/$asset_name"
    local base_apk="$cache_dir/$base_asset_name"
    mkdir -p "$cache_dir"
    download_cached_apk "$asset_url" "$apk" "$asset_name"
    download_cached_apk "$base_asset_url" "$base_apk" "$base_asset_name"

    local args=(--abi "$abi" --apk "$apk" --dependency-apk "$base_apk" --include-dir "$include_dir")
    if [ -n "$data_dir" ]; then
        args+=(--rime-data "$data_dir")
    fi
    import_apk "${args[@]}"
}

import_trime() {
    die "import-trime is disabled: Trime ships librime_jni.so, a Trime Java JNI wrapper. Use import-fcitx5-rime or import-sdk with a pure Android librime.so."
}

sync_one() {
    local abi="$1"
    verify_one "$abi"
    local root
    root="$(runtime_dir "$abi")"
    local jni_dir="$ANDROID_APP_DIR/src/main/jniLibs/$abi"
    mkdir -p "$jni_dir"
    find "$jni_dir" -maxdepth 1 -type f \( \
        -name 'librime*.so' -o \
        -name 'libopencc*.so' -o \
        -name 'liblua*.so' -o \
        -name 'libmarisa*.so' -o \
        -name 'libyaml-cpp*.so' -o \
        -name 'libleveldb*.so' -o \
        -name 'libglog*.so' -o \
        -name 'libboost*.so' -o \
        -name 'libFcitx5*.so' -o \
        -name 'libc++_shared.so' \
    \) -delete
    cp -f "$root/lib"/*.so "$jni_dir"/
    note "Synced $abi native libs into $jni_dir"
}

sync_assets() {
    local source_data=""
    local abi
    for abi in "${ABIS[@]}"; do
        if [ -f "$(runtime_dir "$abi")/rime-data/default.yaml" ]; then
            source_data="$(runtime_dir "$abi")/rime-data"
            break
        fi
    done
    [ -n "$source_data" ] || return 0
    local assets_dir="$ANDROID_APP_DIR/src/main/assets"
    mkdir -p "$assets_dir"
    rm -rf "$assets_dir/keytao-rime-data"
    copy_dir_contents "$source_data" "$assets_dir/keytao-rime-data"
    local source_root
    source_root="$(metadata_root_for_data_dir "$source_data/opencc")"
    for metadata_file in librime-release.txt opencc-release.txt; do
        if [ -f "$source_root/$metadata_file" ]; then
            cp "$source_root/$metadata_file" "$assets_dir/$metadata_file"
        fi
    done
    cat > "$assets_dir/keytao-rime-runtime.txt" <<EOF
source=$source_data
generated_by=scripts/android-librime-runtime.sh sync
EOF
    note "Synced rime-data assets into $assets_dir/keytao-rime-data"
}

sync_runtime() {
    local mode=""
    local allow_missing=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) mode="${2:?missing value for --abi}"; shift 2 ;;
            --all) mode="all"; shift ;;
            --allow-missing) allow_missing=1; shift ;;
            *) die "unknown sync option: $1" ;;
        esac
    done
    [ -n "$mode" ] || mode="all"

    local synced=0
    if [ "$mode" = "all" ]; then
        local abi
        for abi in "${ABIS[@]}"; do
            if [ -d "$(runtime_dir "$abi")" ]; then
                sync_one "$abi"
                synced=1
            fi
        done
    else
        is_abi "$mode" || die "unsupported Android ABI: $mode"
        sync_one "$mode"
        synced=1
    fi

    if [ "$synced" -eq 0 ]; then
        if [ "$allow_missing" -eq 1 ]; then
            echo "WARNING: no Android librime runtime found under $ANDROID_RIME_ROOT" >&2
            return 0
        fi
        die "no Android librime runtime found under $ANDROID_RIME_ROOT"
    fi
    sync_assets
}

verify_runtime() {
    local mode=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) mode="${2:?missing value for --abi}"; shift 2 ;;
            --all) mode="all"; shift ;;
            *) die "unknown verify option: $1" ;;
        esac
    done
    [ -n "$mode" ] || mode="all"
    if [ "$mode" = "all" ]; then
        local found=0
        local abi
        for abi in "${ABIS[@]}"; do
            if [ -d "$(runtime_dir "$abi")" ]; then
                verify_one "$abi"
                note "$abi runtime OK"
                found=1
            fi
        done
        [ "$found" -eq 1 ] || die "no Android librime runtime found under $ANDROID_RIME_ROOT"
    else
        is_abi "$mode" || die "unsupported Android ABI: $mode"
        verify_one "$mode"
        note "$mode runtime OK"
    fi
}

print_env() {
    local abi=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --abi) abi="${2:?missing value for --abi}"; shift 2 ;;
            *) die "unknown env option: $1" ;;
        esac
    done
    [ -n "$abi" ] || die "missing --abi"
    is_abi "$abi" || die "unsupported Android ABI: $abi"
    verify_one "$abi"
    cat "$(runtime_dir "$abi")/env.sh"
}

if [ $# -eq 0 ]; then
    usage
    exit 2
fi
if [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    usage
    exit 0
fi

remaining=()
while [ $# -gt 0 ]; do
    case "$1" in
        --android-app-dir)
            ANDROID_APP_DIR="$(abs_path "${2:?missing value for --android-app-dir}")"
            shift 2
            ;;
        --runtime-root)
            ANDROID_RIME_ROOT="$(abs_path "${2:?missing value for --runtime-root}")"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            remaining+=("$1")
            shift
            ;;
    esac
done
set -- "${remaining[@]}"
[ $# -gt 0 ] || die "missing command"

COMMAND="$1"
shift

case "$COMMAND" in
    import-sdk) import_sdk "$@" ;;
    import-apk) import_apk "$@" ;;
    import-fcitx5-rime) import_fcitx5_rime "$@" ;;
    import-trime) import_trime "$@" ;;
    sync) sync_runtime "$@" ;;
    verify) verify_runtime "$@" ;;
    env) print_env "$@" ;;
    *)
        die "unknown command: $COMMAND"
        ;;
esac
