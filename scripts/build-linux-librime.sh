#!/usr/bin/env bash
# Build the pinned Linux librime with static dependencies and merged Lua support.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/librime-versions.env"
[[ $# -le 1 ]] || { echo "Usage: $0 [PREFIX]" >&2; exit 2; }
PREFIX="${1:-/usr}"
mkdir -p "$PREFIX"
PREFIX="$(cd "$PREFIX" && pwd)"
WORK_DIR="$(mktemp -d /tmp/keytao-librime.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT
SOURCE_DIR="$WORK_DIR/librime"
DEPS_PREFIX="$WORK_DIR/deps"

git clone --depth 1 --branch "$LIBRIME_VERSION" https://github.com/rime/librime.git "$SOURCE_DIR"
git -C "$SOURCE_DIR" submodule update --init --depth 1 \
  deps/glog deps/leveldb deps/marisa-trie deps/opencc deps/yaml-cpp

checkout_lua_revision() {
  local revision="$1" destination="$2"
  git init -q "$destination"
  git -C "$destination" remote add origin https://github.com/hchunhui/librime-lua.git
  git -C "$destination" fetch --depth 1 origin "$revision"
  git -C "$destination" checkout --detach FETCH_HEAD
}
checkout_lua_revision "$LIBRIME_LUA_REF" "$SOURCE_DIR/plugins/lua"
checkout_lua_revision "$LIBRIME_LUA_THIRDPARTY_REF" "$SOURCE_DIR/plugins/lua/thirdparty"

DEPS_CMAKE_ARGS=(
  -G Ninja
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5
  -DCMAKE_BUILD_TYPE=Release
  "-DCMAKE_INSTALL_PREFIX=$DEPS_PREFIX"
  -DCMAKE_INSTALL_LIBDIR=lib
  "-DCMAKE_PREFIX_PATH=$DEPS_PREFIX"
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON
  -DBUILD_SHARED_LIBS=OFF
)
build_dependency() {
  local name="$1" source="$2"
  shift 2
  cmake -S "$source" -B "$WORK_DIR/build/$name" "${DEPS_CMAKE_ARGS[@]}" "$@"
  cmake --build "$WORK_DIR/build/$name" --target install
}

build_dependency glog "$SOURCE_DIR/deps/glog" \
  -DBUILD_TESTING=OFF -DWITH_GFLAGS=OFF -DWITH_UNWIND=none
build_dependency yaml "$SOURCE_DIR/deps/yaml-cpp" \
  -DYAML_CPP_BUILD_CONTRIB=OFF -DYAML_CPP_BUILD_TESTS=OFF -DYAML_CPP_BUILD_TOOLS=OFF
build_dependency leveldb "$SOURCE_DIR/deps/leveldb" \
  -DLEVELDB_BUILD_BENCHMARKS=OFF -DLEVELDB_BUILD_TESTS=OFF \
  -DHAVE_CRC32C=OFF -DHAVE_TCMALLOC=OFF -DHAVE_SNAPPY=ON
# Existing user databases may contain Snappy-compressed blocks.
grep -Eq '^#define HAVE_SNAPPY 1$' "$WORK_DIR/build/leveldb/include/port/port_config.h" \
  || { echo 'LevelDB must support Snappy to read existing user databases' >&2; exit 1; }
build_dependency marisa "$SOURCE_DIR/deps/marisa-trie" -DBUILD_TESTING=OFF -DENABLE_TOOLS=OFF
sed -i -e 's/-std=c++14/-std=c++17/' -e 's/CMAKE_CXX_STANDARD 14/CMAKE_CXX_STANDARD 17/' \
  "$SOURCE_DIR/deps/opencc/CMakeLists.txt"
build_dependency opencc "$SOURCE_DIR/deps/opencc" \
  -DBUILD_DOCUMENTATION=OFF -DBUILD_PYTHON=OFF -DENABLE_GTEST=OFF -DENABLE_BENCHMARK=OFF \
  -DUSE_SYSTEM_MARISA=ON "-DLIBMARISA=$DEPS_PREFIX/lib/libmarisa.a" \
  "-DCMAKE_CXX_FLAGS=-I$DEPS_PREFIX/include" "-DCMAKE_EXE_LINKER_FLAGS=-L$DEPS_PREFIX/lib"

cmake -S "$SOURCE_DIR" -B "$WORK_DIR/build/librime" -G Ninja \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DCMAKE_BUILD_TYPE=Release \
  "-DCMAKE_INSTALL_PREFIX=$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib "-DCMAKE_PREFIX_PATH=$DEPS_PREFIX" \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DBUILD_SHARED_LIBS=ON -DBUILD_STATIC=ON -DBUILD_MERGED_PLUGINS=ON \
  -DENABLE_EXTERNAL_PLUGINS=OFF -DBUILD_TEST=OFF -DBUILD_DATA=OFF \
  -DCMAKE_CXX_STANDARD_LIBRARIES=-lsnappy -DCMAKE_SHARED_LINKER_FLAGS=-Wl,--exclude-libs,ALL
cmake --build "$WORK_DIR/build/librime" --target install
strip --strip-all "$PREFIX/lib/librime.so.1"
mkdir -p "$PREFIX/share/opencc"
cp -a "$DEPS_PREFIX/share/opencc/." "$PREFIX/share/opencc/"
if [[ "$PREFIX" == /usr ]]; then ldconfig; fi
