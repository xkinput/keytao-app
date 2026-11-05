#!/usr/bin/env bash
# Build the Android librime SDK and import it with the existing runtime layout.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
requested_version="${LIBRIME_VERSION:-}"
source "$PROJECT_DIR/scripts/librime-versions.env"
export LIBRIME_VERSION="${requested_version:-$LIBRIME_VERSION}"
source "$PROJECT_DIR/scripts/android-librime-runtime.sh"

NDK_VERSION=27.0.12077973
BOOST_VERSION=1.84.0
CACHE_ROOT="${KEYTAO_ANDROID_LIBRIME_CACHE:-$PROJECT_DIR/.cache/android-librime/source}"
BUILD_ROOT="${KEYTAO_ANDROID_LIBRIME_BUILD_ROOT:-$PROJECT_DIR/target/android-librime-build}"
BUILD_ABIS=(arm64-v8a armeabi-v7a x86_64)
PATCH="$PROJECT_DIR/scripts/patches/librime-android.patch"

usage() {
    cat <<EOF
Usage: $0 [--abi ABI | --all] [--clean]

Build and import arm64-v8a (default), armeabi-v7a or x86_64.
--all builds the three release ABIs. --clean removes that ABI's build directory.
LIBRIME_VERSION overrides librime-versions.env and accepts a release tag or latest.
ANDROID_NDK_HOME must point to NDK $NDK_VERSION (API 24, c++_shared).
KEYTAO_ANDROID_BUILD_JOBS controls parallel compilation.
EOF
}

download() {
    local url="$1" destination="$2"
    [ ! -f "$destination" ] || return 0
    mkdir -p "$(dirname "$destination")"
    curl --fail --location --retry 3 "$url" -o "$destination.part"
    mv "$destination.part" "$destination"
}

checkout_repo() {
    local url="$1" ref="$2" destination="$3"
    if [ ! -d "$destination/.git" ]; then
        git clone --no-checkout "$url" "$destination"
    fi
    git -C "$destination" fetch --depth 1 origin "$ref"
    git -C "$destination" checkout --detach FETCH_HEAD
}

prepare_source() {
    checkout_repo https://github.com/rime/librime.git "$LIBRIME_VERSION" "$source_dir"
    git -C "$source_dir" submodule update --init --depth 1 \
        deps/glog deps/leveldb deps/marisa-trie deps/opencc deps/yaml-cpp
    # Upstream: fcitx5-android/prebuilder d7774b67a46237ed53aae913c7f02ddb367f647c.
    # Keep RimeSetOption save_options persistence and deprecated-config handling.
    if git -C "$source_dir" apply --check "$PATCH"; then
        git -C "$source_dir" apply "$PATCH"
    elif ! git -C "$source_dir" apply --reverse --check "$PATCH"; then
        die "Android librime patch does not apply to $LIBRIME_VERSION"
    fi
    checkout_repo https://github.com/hchunhui/librime-lua.git "$LIBRIME_LUA_REF" "$source_dir/plugins/lua"
    checkout_repo https://github.com/hchunhui/librime-lua.git "$LIBRIME_LUA_THIRDPARTY_REF" "$source_dir/plugins/lua/thirdparty"
    checkout_repo https://github.com/lotem/librime-octagram.git "$LIBRIME_OCTAGRAM_REF" "$source_dir/plugins/octagram"
    checkout_repo https://github.com/rime/librime-predict.git "$LIBRIME_PREDICT_REF" "$source_dir/plugins/predict"

    # Cross builds must never run OpenCC tools, data generators or tests on the host.
    perl -0pi -e 's/^[ \t]*add_subdirectory\((tools|data|test)\)[ \t]*\r?\n//mg' \
        "$source_dir/deps/opencc/CMakeLists.txt" "$source_dir/deps/opencc/src/CMakeLists.txt"
    if grep -E 'add_subdirectory\((tools|data|test)\)' \
        "$source_dir/deps/opencc/CMakeLists.txt" "$source_dir/deps/opencc/src/CMakeLists.txt"; then
        die "OpenCC host tools, data and tests must be disabled"
    fi

    local archive="$CACHE_ROOT/downloads/boost_1_84_0.tar.bz2"
    if [ ! -d "$boost_source/boost" ]; then
        download "https://archives.boost.io/release/$BOOST_VERSION/source/boost_1_84_0.tar.bz2" "$archive"
        tar -xjf "$archive" -C "$CACHE_ROOT" boost_1_84_0/boost
    fi
}

build_project() {
    local name="$1" source="$2"
    shift 2
    rm -rf "$work/build/$name"
    cmake -S "$source" -B "$work/build/$name" "${common[@]}" "$@"
    cmake --build "$work/build/$name" --target install --parallel "$build_jobs"
}

build_abi() {
    local abi="$1" work="$BUILD_ROOT/$1"
    local prefix="$work/prefix" sdk_dir="$work/sdk" cxx_shared source_sha
    if [ "$clean" = true ]; then
        rm -rf "$work"
    fi
    rm -rf "$prefix" "$sdk_dir"
    mkdir -p "$prefix/include" "$sdk_dir/include" "$sdk_dir/lib"
    # librime and its merged plugins use header-only Boost.
    cp -R "$boost_source/boost" "$prefix/include/"
    local -a common=(
        -DCMAKE_TOOLCHAIN_FILE="$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake"
        -DANDROID_ABI="$abi" -DANDROID_PLATFORM=android-24 -DANDROID_STL=c++_shared
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_POLICY_VERSION_MINIMUM=3.5
        -DCMAKE_POSITION_INDEPENDENT_CODE=ON
        -DCMAKE_C_FLAGS="-ffunction-sections -fdata-sections"
        -DCMAKE_CXX_FLAGS="-ffunction-sections -fdata-sections"
        -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_PREFIX_PATH="$prefix"
        -DCMAKE_FIND_ROOT_PATH="$prefix" -DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=BOTH
        -DBUILD_SHARED_LIBS=OFF
    )
    note "Building Android librime dependencies for $abi"
    build_project glog "$source_dir/deps/glog" -DBUILD_TESTING=OFF -DWITH_GFLAGS=OFF \
        -DWITH_UNWIND=none -DCMAKE_CXX_STANDARD_LIBRARIES=-llog
    build_project yaml-cpp "$source_dir/deps/yaml-cpp" \
        -DYAML_CPP_BUILD_CONTRIB=OFF -DYAML_CPP_BUILD_TESTS=OFF -DYAML_CPP_BUILD_TOOLS=OFF
    # Today's Android runtime has no snappy; existing LevelDB data was written without it.
    build_project leveldb "$source_dir/deps/leveldb" \
        -DLEVELDB_BUILD_BENCHMARKS=OFF -DLEVELDB_BUILD_TESTS=OFF \
        -DHAVE_CRC32C=OFF -DHAVE_SNAPPY=OFF -DHAVE_TCMALLOC=OFF
    build_project marisa "$source_dir/deps/marisa-trie" -DBUILD_TESTING=OFF -DENABLE_TOOLS=OFF
    build_project opencc "$source_dir/deps/opencc" \
        -DCMAKE_CXX_FLAGS="-I$prefix/include -ffunction-sections -fdata-sections" \
        -DLIBMARISA="$prefix/lib/libmarisa.a" -DUSE_SYSTEM_MARISA=ON \
        -DBUILD_DOCUMENTATION=OFF -DBUILD_PYTHON=OFF -DENABLE_GTEST=OFF -DENABLE_BENCHMARK=OFF

    note "Building librime $LIBRIME_VERSION for $abi"
    build_project librime "$source_dir" \
        -DBUILD_SHARED_LIBS=ON -DBUILD_STATIC=OFF -DBUILD_MERGED_PLUGINS=ON \
        -DENABLE_EXTERNAL_PLUGINS=OFF -DBUILD_TEST=OFF -DBUILD_DATA=OFF -DBUILD_SAMPLE=OFF -DBUILD_TOOLS=OFF \
        -DCMAKE_CXX_STANDARD_LIBRARIES=-llog \
        -DBoost_NO_SYSTEM_PATHS=ON -DBoost_NO_BOOST_CMAKE=ON \
        -DBoost_ROOT="$prefix" -DBoost_INCLUDE_DIR="$prefix/include" -DBOOST_ROOT="$prefix" \
        -DGlog_INCLUDE_PATH="$prefix/include" -DGlog_LIBRARY="$prefix/lib/libglog.a" \
        -DYamlCpp_INCLUDE_PATH="$prefix/include" -DYamlCpp_NEW_API="$prefix/include" -DYamlCpp_LIBRARY="$prefix/lib/libyaml-cpp.a" \
        -DLevelDb_INCLUDE_PATH="$prefix/include" -DLevelDb_LIBRARY="$prefix/lib/libleveldb.a" \
        -DMarisa_INCLUDE_PATH="$prefix/include" -DMarisa_LIBRARY="$prefix/lib/libmarisa.a" \
        -DOpencc_INCLUDE_PATH="$prefix/include" -DOpencc_LIBRARY="$prefix/lib/libopencc.a" \
        -DCMAKE_SHARED_LINKER_FLAGS="-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384 -Wl,--exclude-libs,ALL -Wl,--gc-sections"
    [ -f "$prefix/lib/librime.so" ] || die "missing built librime.so for $abi"
    cp "$prefix/include"/rime*.h "$sdk_dir/include/"
    cp -L "$prefix/lib/librime.so" "$sdk_dir/lib/librime.so"
    "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$(host_tag)/bin/llvm-strip" --strip-unneeded "$sdk_dir/lib/librime.so"
    # import-sdk validates dependency closure but does not copy missing libraries.
    cxx_shared="$(ndk_cxx_shared_for_abi "$abi")" || die "missing NDK libc++_shared.so for $abi"
    cp "$cxx_shared" "$sdk_dir/lib/libc++_shared.so"
    source_sha="$(git -C "$source_dir" rev-parse HEAD)"
    cat > "$sdk_dir/librime-release.txt" <<EOF
platform=android
version=$(normalize_version_tag "$LIBRIME_VERSION")
source=source-$source_sha
EOF
    # Let import-sdk fetch today's rime-data and compiled OpenCC data as before.
    "$PROJECT_DIR/scripts/android-librime-runtime.sh" import-sdk --abi "$abi" --source "$sdk_dir"
}

clean=false
selected_abis=(arm64-v8a)
while [ "$#" -gt 0 ]; do
    case "$1" in
        --abi) selected_abis=("${2:?missing value for --abi}"); shift 2 ;;
        --all) selected_abis=("${BUILD_ABIS[@]}"); shift ;;
        --clean) clean=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done
for abi in "${selected_abis[@]}"; do
    case "$abi" in arm64-v8a|armeabi-v7a|x86_64) ;; *) die "unsupported release ABI: $abi" ;; esac
done
for tool in cmake git curl perl tar python3; do
    command_exists "$tool" || die "$tool is required to build Android librime"
done
sdk_root="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
if [ "$(uname -s)" = Darwin ]; then
    sdk_root="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
fi
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${NDK_HOME:-$sdk_root/ndk/$NDK_VERSION}}}"
[ -f "$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake" ] || die "missing NDK $NDK_VERSION: $ANDROID_NDK_HOME"
[ "$(sed -n 's/^Pkg.Revision[[:space:]]*=[[:space:]]*//p' "$ANDROID_NDK_HOME/source.properties")" = "$NDK_VERSION" ] ||
    die "ANDROID_NDK_HOME must point to NDK $NDK_VERSION"
if [ "$LIBRIME_VERSION" = latest ]; then
    LIBRIME_VERSION="$(curl -fsSL --retry 3 https://api.github.com/repos/rime/librime/releases/latest | python3 -c '
import json, sys
r = json.load(sys.stdin)
if r.get("prerelease") or r.get("draft") or r.get("tag_name") in (None, "", "latest"):
    sys.exit("expected a stable librime release")
print(r["tag_name"])
')"
fi
[[ "$LIBRIME_VERSION" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "invalid librime release tag: $LIBRIME_VERSION"
source_dir="$CACHE_ROOT/src/librime-$LIBRIME_VERSION"
boost_source="$CACHE_ROOT/boost_1_84_0"
build_jobs="${KEYTAO_ANDROID_BUILD_JOBS:-$(getconf _NPROCESSORS_ONLN)}"
mkdir -p "$CACHE_ROOT/src"
prepare_source
for abi in "${selected_abis[@]}"; do
    build_abi "$abi"
done
