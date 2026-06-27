#!/usr/bin/env bash
# Build a reusable iOS librime SDK and import it into vendor/librime/ios.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_ROOT="${KEYTAO_IOS_LIBRIME_CACHE:-$PROJECT_DIR/.cache/ios-librime}"
BUILD_ROOT="${KEYTAO_IOS_LIBRIME_BUILD_ROOT:-$PROJECT_DIR/target/ios-librime-build}"
LIBRIME_URL="${KEYTAO_IOS_LIBRIME_URL:-https://github.com/rime/librime.git}"
LIBRIME_REF="${KEYTAO_IOS_LIBRIME_REF:-08dd95f5d9282346f0d4a3e8fc6b20811dc3d063}"
LIBRIME_LUA_URL="${KEYTAO_IOS_LIBRIME_LUA_URL:-https://github.com/hchunhui/librime-lua.git}"
LIBRIME_LUA_REF="${KEYTAO_IOS_LIBRIME_LUA_REF:-master}"
LIBRIME_LUA_THIRDPARTY_REF="${KEYTAO_IOS_LIBRIME_LUA_THIRDPARTY_REF:-thirdparty}"
LIBRIMEKIT_VERSION="${KEYTAO_IOS_LIBRIMEKIT_VERSION:-v0.1.0}"
BOOST_VERSION="${KEYTAO_IOS_BOOST_VERSION:-1.76.0}"
CMAKE_VERSION="${KEYTAO_IOS_CMAKE_VERSION:-4.3.3}"
IOS_DEPLOYMENT_TARGET="${KEYTAO_IOS_DEPLOYMENT_TARGET:-15.0}"
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --target TARGET       Build and import one iOS librime runtime.
  --all                 Build all iOS runtimes.
  --clean               Remove the target build directory before building.
  --no-import           Build the SDK but do not import it into vendor/librime/ios.
  -h, --help            Show this help.

Environment:
  KEYTAO_IOS_LIBRIME_REF        librime git ref. Defaults to the LibrimeKit v0.1.0 ref.
  KEYTAO_IOS_LIBRIME_LUA_REF    librime-lua git ref. Defaults to master.
  KEYTAO_IOS_BOOST_VERSION      Boost header version. Defaults to $BOOST_VERSION.
  KEYTAO_IOS_CMAKE_VERSION      Downloaded CMake version if cmake is missing.
  KEYTAO_IOS_DEPLOYMENT_TARGET  iOS deployment target. Defaults to $IOS_DEPLOYMENT_TARGET.

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
    echo "==> $*" >&2
}

target_to_runtime() {
    case "$1" in
        aarch64-apple-ios) echo "iphoneos-arm64" ;;
        aarch64-apple-ios-sim) echo "iphonesimulator-arm64" ;;
        x86_64-apple-ios) echo "iphonesimulator-x86_64" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

target_to_sdk() {
    case "$1" in
        aarch64-apple-ios) echo "iphoneos" ;;
        aarch64-apple-ios-sim|x86_64-apple-ios) echo "iphonesimulator" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

target_to_arch() {
    case "$1" in
        aarch64-apple-ios|aarch64-apple-ios-sim) echo "arm64" ;;
        x86_64-apple-ios) echo "x86_64" ;;
        *) die "unsupported iOS target: $1" ;;
    esac
}

download() {
    local url="$1"
    local destination="$2"
    local partial="$destination.part"
    mkdir -p "$(dirname "$destination")"
    if [ -f "$destination" ]; then
        return 0
    fi
    note "Downloading $url"
    curl --fail --location --retry 3 --retry-delay 2 --continue-at - "$url" -o "$partial"
    mv "$partial" "$destination"
}

ensure_cmake() {
    if command -v cmake >/dev/null 2>&1; then
        command -v cmake
        return 0
    fi

    local root="$CACHE_ROOT/cmake/$CMAKE_VERSION"
    local cmake_bin="$root/CMake.app/Contents/bin/cmake"
    if [ ! -x "$cmake_bin" ]; then
        local archive="$CACHE_ROOT/downloads/cmake-$CMAKE_VERSION-macos-universal.tar.gz"
        download "https://github.com/Kitware/CMake/releases/download/v$CMAKE_VERSION/cmake-$CMAKE_VERSION-macos-universal.tar.gz" "$archive"
        rm -rf "$root"
        mkdir -p "$root"
        tar -xzf "$archive" -C "$root" --strip-components=1
    fi
    printf '%s\n' "$cmake_bin"
}

prepare_librime_source() {
    local source_dir="$CACHE_ROOT/src/librime"
    if [ ! -d "$source_dir/.git" ]; then
        rm -rf "$source_dir"
        mkdir -p "$(dirname "$source_dir")"
        git clone "$LIBRIME_URL" "$source_dir" >&2
    fi
    git -C "$source_dir" fetch --depth 1 origin "$LIBRIME_REF" >&2
    git -C "$source_dir" checkout --detach FETCH_HEAD >&2
    git -C "$source_dir" submodule update --init --depth 1 \
        deps/glog \
        deps/leveldb \
        deps/marisa-trie \
        deps/opencc \
        deps/yaml-cpp >&2
    patch_opencc_for_ios "$source_dir/deps/opencc"
    ensure_librime_lua_plugin "$source_dir"
    printf '%s\n' "$source_dir"
}

ensure_librime_lua_plugin() {
    local source_dir="$1"
    local plugin_dir="$source_dir/plugins/lua"

    if [ ! -d "$plugin_dir/.git" ]; then
        rm -rf "$plugin_dir"
        note "Checking out librime-lua into $plugin_dir"
        git clone --depth 1 "$LIBRIME_LUA_URL" "$plugin_dir" >&2
    fi
    git -C "$plugin_dir" fetch --depth 1 origin "$LIBRIME_LUA_REF" >&2
    git -C "$plugin_dir" checkout --detach FETCH_HEAD >&2

    if [ ! -f "$plugin_dir/thirdparty/lua5.4/lua.h" ]; then
        rm -rf "$plugin_dir/thirdparty"
        note "Checking out librime-lua thirdparty Lua sources"
        git clone --depth 1 --branch "$LIBRIME_LUA_THIRDPARTY_REF" "$LIBRIME_LUA_URL" "$plugin_dir/thirdparty" >&2
    fi
    patch_librime_lua_for_ios "$plugin_dir"
}

patch_librime_lua_for_ios() {
    local plugin_dir="$1"
    local types_cc="$plugin_dir/src/types.cc"
    local cmake_lists="$plugin_dir/CMakeLists.txt"
    [ -f "$types_cc" ] || die "missing librime-lua source: $types_cc"
    [ -f "$cmake_lists" ] || die "missing librime-lua CMakeLists: $cmake_lists"

    if ! grep -q "KEYTAO_IOS_LIBRIME_LUA_FILESYSTEM_COMPAT" "$types_cc"; then
        perl -0pi -e 's/#include <boost\/regex\.hpp>\n/#include <boost\/regex.hpp>\n#include <boost\/filesystem.hpp>\n\n\/\/ KEYTAO_IOS_LIBRIME_LUA_FILESYSTEM_COMPAT: librime 1.8.x iOS builds use C++14 and Boost filesystem.\nusing path = boost::filesystem::path;\n/' "$types_cc"
    fi
    perl -0pi -e 's/std::filesystem::exists/boost::filesystem::exists/g' "$types_cc"

    if ! grep -q "KEYTAO_IOS_LIBRIME_LUA_CMAKE_COMPAT" "$cmake_lists"; then
        perl -0pi -e 's/add_definitions\(-DLUA_COMPAT_5_3\)\n/add_definitions(-DLUA_COMPAT_5_3)\n# KEYTAO_IOS_LIBRIME_LUA_CMAKE_COMPAT: avoid unavailable system() in Lua oslib on iOS.\nif(CMAKE_SYSTEM_NAME MATCHES "iOS")\n  add_definitions(-DLUA_USE_IOS)\nendif()\n/' "$cmake_lists"
    fi
}

patch_opencc_for_ios() {
    local opencc_dir="$1"
    local src_cmake="$opencc_dir/src/CMakeLists.txt"
    local root_cmake="$opencc_dir/CMakeLists.txt"

    if ! grep -q "KEYTAO_IOS_SKIP_OPENCC_TOOLS" "$src_cmake"; then
        perl -0pi -e 's/add_subdirectory\(tools\)/# KEYTAO_IOS_SKIP_OPENCC_TOOLS\nif (NOT CMAKE_SYSTEM_NAME MATCHES "iOS")\n  add_subdirectory(tools)\nendif()/g' "$src_cmake"
    fi
    if ! grep -q "KEYTAO_IOS_SKIP_OPENCC_DATA" "$root_cmake"; then
        perl -0pi -e 's/add_subdirectory\(data\)\nadd_subdirectory\(test\)/# KEYTAO_IOS_SKIP_OPENCC_DATA\nif (NOT CMAKE_SYSTEM_NAME MATCHES "iOS")\n  add_subdirectory(data)\n  add_subdirectory(test)\nendif()/g' "$root_cmake"
    fi
}

ensure_librimekit_frameworks() {
    local frameworks_dir="$CACHE_ROOT/librimekit/$LIBRIMEKIT_VERSION/Frameworks"
    if [ ! -d "$frameworks_dir" ]; then
        local archive="$CACHE_ROOT/downloads/LibrimeKit-$LIBRIMEKIT_VERSION-Frameworks.tgz"
        download "https://github.com/amorphobia/LibrimeKit/releases/download/$LIBRIMEKIT_VERSION/Frameworks.tgz" "$archive"
        rm -rf "$CACHE_ROOT/librimekit/$LIBRIMEKIT_VERSION"
        mkdir -p "$CACHE_ROOT/librimekit/$LIBRIMEKIT_VERSION"
        tar -xzf "$archive" -C "$CACHE_ROOT/librimekit/$LIBRIMEKIT_VERSION"
    fi
    printf '%s\n' "$frameworks_dir"
}

ensure_boost_source() {
    local boost_name="boost_${BOOST_VERSION//./_}"
    local root="$CACHE_ROOT/boost/$BOOST_VERSION"
    if [ ! -d "$root/$boost_name/boost" ]; then
        local archive="$CACHE_ROOT/downloads/$boost_name.tar.bz2"
        download "https://archives.boost.io/release/$BOOST_VERSION/source/$boost_name.tar.bz2" "$archive"
        rm -rf "$root"
        mkdir -p "$root"
        tar -xjf "$archive" -C "$root"
    fi
    printf '%s\n' "$root/$boost_name"
}

copy_boost_runtime() {
    local frameworks_dir="$1"
    local target="$2"
    local prefix="$3"
    local boost_source="$4"
    local arch runtime slice source
    arch="$(target_to_arch "$target")"
    runtime="$(target_to_runtime "$target")"

    case "$runtime" in
        iphoneos-arm64) slice="ios-arm64" ;;
        iphonesimulator-*) slice="ios-arm64_x86_64-simulator" ;;
        *) die "unsupported runtime: $runtime" ;;
    esac

    mkdir -p "$prefix/lib"
    for lib in atomic regex system; do
        source="$frameworks_dir/boost_${lib}.xcframework/$slice/libboost_${lib}.a"
        [ -f "$source" ] || die "missing Boost library: $source"
        if lipo -info "$source" 2>/dev/null | grep -q "Non-fat"; then
            cp "$source" "$prefix/lib/libboost_${lib}.a"
        else
            lipo "$source" -thin "$arch" -output "$prefix/lib/libboost_${lib}.a"
        fi
    done
    build_boost_filesystem "$boost_source" "$target" "$prefix"
}

build_boost_filesystem() {
    local boost_source="$1"
    local target="$2"
    local prefix="$3"
    local sdk arch min_flag build_dir src obj
    sdk="$(target_to_sdk "$target")"
    arch="$(target_to_arch "$target")"
    if [ "$sdk" = "iphoneos" ]; then
        min_flag="-miphoneos-version-min=$IOS_DEPLOYMENT_TARGET"
    else
        min_flag="-mios-simulator-version-min=$IOS_DEPLOYMENT_TARGET"
    fi

    build_dir="$prefix/../build/boost-filesystem"
    rm -rf "$build_dir"
    mkdir -p "$build_dir/obj"
    for src in "$boost_source"/libs/filesystem/src/*.cpp; do
        obj="$build_dir/obj/$(basename "${src%.cpp}").o"
        xcrun --sdk "$sdk" clang++ \
            -arch "$arch" \
            "$min_flag" \
            -std=c++14 \
            -O2 \
            -fvisibility=hidden \
            -fvisibility-inlines-hidden \
            -I"$boost_source" \
            -DBOOST_ALL_NO_LIB \
            -DBOOST_FILESYSTEM_STATIC_LINK \
            -DBOOST_SYSTEM_STATIC_LINK \
            -c "$src" \
            -o "$obj"
    done
    /usr/bin/libtool -static -o "$prefix/lib/libboost_filesystem.a" "$build_dir"/obj/*.o
}

common_cmake_args() {
    local sdkroot="$1"
    local arch="$2"
    local prefix="$3"
    printf '%s\n' \
        -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
        -DCMAKE_SYSTEM_NAME=iOS \
        -DCMAKE_OSX_SYSROOT="$sdkroot" \
        -DCMAKE_OSX_ARCHITECTURES="$arch" \
        -DCMAKE_OSX_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$prefix" \
        -DCMAKE_PREFIX_PATH="$prefix" \
        -DCMAKE_LIBRARY_PATH="$prefix/lib" \
        -DCMAKE_INCLUDE_PATH="$prefix/include" \
        -DBUILD_SHARED_LIBS=OFF
}

build_cmake_project() {
    local cmake_bin="$1"
    local source_dir="$2"
    local build_dir="$3"
    shift 3
    rm -rf "$build_dir"
    "$cmake_bin" -S "$source_dir" -B "$build_dir" "$@"
    "$cmake_bin" --build "$build_dir" --target install --parallel "${KEYTAO_IOS_BUILD_JOBS:-$(sysctl -n hw.ncpu)}"
}

build_target() {
    local target="$1"
    local cmake_bin="$2"
    local source_dir="$3"
    local frameworks_dir="$4"
    local boost_source="$5"
    local sdk arch runtime sdkroot work prefix install_dir sdk_dir
    sdk="$(target_to_sdk "$target")"
    arch="$(target_to_arch "$target")"
    runtime="$(target_to_runtime "$target")"
    sdkroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
    work="$BUILD_ROOT/$runtime"
    prefix="$work/prefix"
    install_dir="$work/install"
    sdk_dir="$work/sdk"

    if [ "$CLEAN" = true ]; then
        rm -rf "$work"
    fi
    mkdir -p "$prefix/include" "$prefix/lib" "$install_dir" "$sdk_dir/lib"

    note "Preparing Boost for $runtime"
    cp -R "$boost_source/boost" "$prefix/include/"
    copy_boost_runtime "$frameworks_dir" "$target" "$prefix" "$boost_source"

    local -a common
    common=()
    while IFS= read -r arg; do
        common+=("$arg")
    done < <(common_cmake_args "$sdkroot" "$arch" "$prefix")

    note "Building iOS librime dependencies for $runtime"
    build_cmake_project "$cmake_bin" "$source_dir/deps/glog" "$work/build/glog" \
        "${common[@]}" \
        -DBUILD_TESTING=OFF \
        -DWITH_GFLAGS=OFF
    build_cmake_project "$cmake_bin" "$source_dir/deps/yaml-cpp" "$work/build/yaml-cpp" \
        "${common[@]}" \
        -DYAML_CPP_BUILD_CONTRIB=OFF \
        -DYAML_CPP_BUILD_TESTS=OFF \
        -DYAML_CPP_BUILD_TOOLS=OFF
    build_cmake_project "$cmake_bin" "$source_dir/deps/leveldb" "$work/build/leveldb" \
        "${common[@]}" \
        -DLEVELDB_BUILD_BENCHMARKS=OFF \
        -DLEVELDB_BUILD_TESTS=OFF \
        -DHAVE_CRC32C=OFF \
        -DHAVE_SNAPPY=OFF \
        -DHAVE_TCMALLOC=OFF
    build_cmake_project "$cmake_bin" "$source_dir/deps" "$work/build/marisa" \
        "${common[@]}"
    build_cmake_project "$cmake_bin" "$source_dir/deps/opencc" "$work/build/opencc" \
        "${common[@]}" \
        -DCMAKE_CXX_FLAGS="-I$prefix/include" \
        -DLIBMARISA="$prefix/lib/libmarisa.a" \
        -DBUILD_DOCUMENTATION=OFF \
        -DBUILD_PYTHON=OFF \
        -DENABLE_GTEST=OFF \
        -DENABLE_BENCHMARK=OFF \
        -DUSE_SYSTEM_MARISA=ON

    note "Building librime for $runtime"
    rm -rf "$work/build/librime" "$install_dir"
    "$cmake_bin" -S "$source_dir" -B "$work/build/librime" \
        "${common[@]}" \
        -DCMAKE_INSTALL_PREFIX="$install_dir" \
        -DBoost_NO_SYSTEM_PATHS=ON \
        -DBoost_NO_BOOST_CMAKE=ON \
        -DBoost_ROOT="$prefix" \
        -DBoost_INCLUDE_DIR="$prefix/include" \
        -DBoost_INCLUDE_DIRS="$prefix/include" \
        -DBoost_LIBRARY_DIR_RELEASE="$prefix/lib" \
        -DBoost_LIBRARY_DIRS="$prefix/lib" \
        -DBoost_FILESYSTEM_LIBRARY_RELEASE="$prefix/lib/libboost_filesystem.a" \
        -DBoost_REGEX_LIBRARY_RELEASE="$prefix/lib/libboost_regex.a" \
        -DBoost_SYSTEM_LIBRARY_RELEASE="$prefix/lib/libboost_system.a" \
        -DBoost_ATOMIC_LIBRARY_RELEASE="$prefix/lib/libboost_atomic.a" \
        -DBOOST_ROOT="$prefix" \
        -DBOOST_INCLUDEDIR="$prefix/include" \
        -DBOOST_LIBRARYDIR="$prefix/lib" \
        -DGlog_INCLUDE_PATH="$prefix/include" \
        -DGlog_LIBRARY="$prefix/lib/libglog.a" \
        -DYamlCpp_INCLUDE_PATH="$prefix/include" \
        -DYamlCpp_NEW_API="$prefix/include" \
        -DYamlCpp_LIBRARY="$prefix/lib/libyaml-cpp.a" \
        -DLevelDb_INCLUDE_PATH="$prefix/include" \
        -DLevelDb_LIBRARY="$prefix/lib/libleveldb.a" \
        -DMarisa_INCLUDE_PATH="$prefix/include" \
        -DMarisa_LIBRARY="$prefix/lib/libmarisa.a" \
        -DOpencc_INCLUDE_PATH="$prefix/include" \
        -DOpencc_LIBRARY="$prefix/lib/libopencc.a" \
        -DBUILD_STATIC=ON \
        -DBUILD_SHARED_LIBS=OFF \
        -DBUILD_MERGED_PLUGINS=ON \
        -DENABLE_EXTERNAL_PLUGINS=OFF \
        -DBUILD_TEST=OFF \
        -DBUILD_DATA=OFF
    "$cmake_bin" --build "$work/build/librime" --target install --parallel "${KEYTAO_IOS_BUILD_JOBS:-$(sysctl -n hw.ncpu)}"

    [ -f "$install_dir/lib/librime.a" ] || die "missing built librime archive: $install_dir/lib/librime.a"
    mkdir -p "$sdk_dir/include" "$sdk_dir/lib" "$sdk_dir/rime-data"
    cp "$install_dir/include"/rime*.h "$sdk_dir/include/"
    cp "$install_dir/lib/librime.a" "$sdk_dir/lib/librime.a"
    for static_lib in \
        libboost_atomic.a \
        libboost_filesystem.a \
        libboost_regex.a \
        libboost_system.a \
        libglog.a \
        libleveldb.a \
        libmarisa.a \
        libopencc.a \
        libyaml-cpp.a
    do
        cp "$prefix/lib/$static_lib" "$sdk_dir/lib/$static_lib"
    done

    if [ -d "$PROJECT_DIR/vendor/librime/macos-universal/rime-data" ]; then
        cp -R "$PROJECT_DIR/vendor/librime/macos-universal/rime-data"/. "$sdk_dir/rime-data/"
    else
        "$PROJECT_DIR/scripts/fetch-librime.sh" --platform ios --destination "$work/rime-data-bootstrap" >/dev/null
        cp -R "$work/rime-data-bootstrap/rime-data"/. "$sdk_dir/rime-data/"
    fi

    cat > "$sdk_dir/librime-release.txt" <<EOF
version=source-$LIBRIME_REF
target=$target
runtime=$runtime
arch=$arch
source=$LIBRIME_URL
boost_version=$BOOST_VERSION
production=true
EOF

    lipo -info "$sdk_dir/lib/librime.a"
    note "Built iOS librime SDK: $sdk_dir"
    if [ "$IMPORT_SDK" = true ]; then
        "$PROJECT_DIR/scripts/ios-librime-runtime.sh" import-sdk --target "$target" --source "$sdk_dir"
    fi
}

CLEAN=false
IMPORT_SDK=true
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
        --clean)
            CLEAN=true
            shift
            ;;
        --no-import)
            IMPORT_SDK=false
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
    die "xcrun is required to build iOS librime"
fi

selected_targets=()
if [ "$all_targets" = true ]; then
    selected_targets=("${TARGETS[@]}")
elif [ -n "$target" ]; then
    selected_targets=("$target")
else
    selected_targets=(aarch64-apple-ios)
fi

cmake_bin="$(ensure_cmake)"
source_dir="$(prepare_librime_source)"
frameworks_dir="$(ensure_librimekit_frameworks)"
boost_source="$(ensure_boost_source)"

for selected in "${selected_targets[@]}"; do
    build_target "$selected" "$cmake_bin" "$source_dir" "$frameworks_dir" "$boost_source"
done
