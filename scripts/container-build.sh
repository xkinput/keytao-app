#!/bin/sh
# Runs INSIDE the Docker container. Called by build-linux.sh via docker run.
# Arguments: $1=uid $2=gid
set -eu

UID_GID="$1:$2"
LINUX_RUNTIME_DIR="target/keytao-linux-runtime"
LINUX_RUNTIME_LIB_DIR="$LINUX_RUNTIME_DIR/lib"
LINUX_RUNTIME_RIME_DATA_DIR="$LINUX_RUNTIME_DIR/rime-data"
LINUX_RUNTIME_RPATH='$ORIGIN/lib:$ORIGIN/runtime/lib:$ORIGIN/resources/runtime/lib:$ORIGIN/../runtime/lib:$ORIGIN/../lib:$ORIGIN/../lib/keytao-app/runtime/lib:$ORIGIN/../lib/keytao-app/resources/runtime/lib'

restore_host_ownership() {
  chown -R "$UID_GID" /app/target /app/dist 2>/dev/null || true
}

trap restore_host_ownership EXIT

export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,$LINUX_RUNTIME_RPATH"
export CI="${CI:-true}"

echo "=== Cache contents ==="
ls -lah /root/.cache/tauri/ 2>/dev/null || echo "(empty)"

find_ldconfig_path() {
  pattern="$1"
  ldconfig -p 2>/dev/null | awk -v pattern="$pattern" '$1 ~ pattern { print $NF; exit }'
}

copy_runtime_lib() {
  src="$1"
  dst_dir="${2:-$LINUX_RUNTIME_LIB_DIR}"
  [ -n "$src" ] || return 0
  [ -f "$src" ] || return 0
  mkdir -p "$dst_dir"
  dst="$dst_dir/$(basename "$src")"
  if [ ! -f "$dst" ]; then
    cp -L "$src" "$dst"
    chmod u+w "$dst"
  fi
  printf '%s\n' "$dst"
}

should_copy_runtime_dep() {
  base="$(basename "$1")"
  case "$base" in
    librime*.so*|libopencc*.so*|libyaml-cpp*.so*|libglog*.so*|libmarisa*.so*|libleveldb*.so*|liblua*.so*|libboost*.so*|libcapnp*.so*|libkj*.so*|libkyotocabinet*.so*|libdouble-conversion*.so*|libabsl*.so*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

copy_selected_deps() {
  binary="$1"
  ldd "$binary" 2>/dev/null \
    | awk '/=> \// { print $3 } /^\// { print $1 }' \
    | while IFS= read -r dep; do
        if should_copy_runtime_dep "$dep"; then
          copied="$(copy_runtime_lib "$dep" "$LINUX_RUNTIME_LIB_DIR")"
          [ -n "$copied" ] && copy_selected_deps "$copied"
        fi
      done
}

copy_rime_plugins() {
  libdir="$1"
  plugin_dir="$libdir/rime-plugins"
  [ -d "$plugin_dir" ] || return 0
  mkdir -p "$LINUX_RUNTIME_LIB_DIR/rime-plugins"
  find "$plugin_dir" -maxdepth 1 -type f -name '*.so*' | while IFS= read -r plugin; do
    copied="$(copy_runtime_lib "$plugin" "$LINUX_RUNTIME_LIB_DIR/rime-plugins")"
    [ -n "$copied" ] && copy_selected_deps "$copied"
  done
}

has_rime_data() {
  dir="$1"
  [ -n "$dir" ] && [ -f "$dir/default.yaml" ]
}

ensure_linux_rime_data() {
  for candidate in \
    "${KEYTAO_RIME_SHARED_DATA_DIR:-}" \
    "${RIME_SHARED_DATA_DIR:-}" \
    "${RIME_DATA_DIR:-}" \
    "/usr/share/rime-data" \
    "/usr/local/share/rime-data"; do
    if has_rime_data "$candidate"; then
      return 0
    fi
  done

  echo "=== Fetching Linux rime-data ==="
  linux_data_dir="target/keytao-linux-rime"
  /app/scripts/fetch-librime.sh \
    --platform linux \
    --destination "$linux_data_dir"
  if [ -f "$linux_data_dir/env.sh" ]; then
    # shellcheck disable=SC1090
    . "$linux_data_dir/env.sh"
  fi
}

prepare_linux_runtime() {
  rm -rf "$LINUX_RUNTIME_DIR"
  mkdir -p "$LINUX_RUNTIME_LIB_DIR" "$LINUX_RUNTIME_RIME_DATA_DIR"
  cp crates/keytao-theme/default-theme.yaml "$LINUX_RUNTIME_DIR/default-theme.yaml"

  rime_data_src=""
  for candidate in \
    "${KEYTAO_RIME_SHARED_DATA_DIR:-}" \
    "${RIME_SHARED_DATA_DIR:-}" \
    "${RIME_DATA_DIR:-}" \
    "/usr/share/rime-data" \
    "/usr/local/share/rime-data"; do
    [ -n "$candidate" ] || continue
    if [ -f "$candidate/default.yaml" ]; then
      rime_data_src="$candidate"
      break
    fi
  done
  if [ -z "$rime_data_src" ]; then
    echo "ERROR: cannot find Linux rime-data/default.yaml" >&2
    exit 1
  fi
  cp -a "$rime_data_src/." "$LINUX_RUNTIME_RIME_DATA_DIR/"

  if [ ! -d "$LINUX_RUNTIME_RIME_DATA_DIR/opencc" ]; then
    for opencc_dir in /usr/share/opencc /usr/local/share/opencc; do
      if [ -d "$opencc_dir" ]; then
        mkdir -p "$LINUX_RUNTIME_RIME_DATA_DIR/opencc"
        cp -a "$opencc_dir/." "$LINUX_RUNTIME_RIME_DATA_DIR/opencc/"
        break
      fi
    done
  fi

  rime_lib="$(find_ldconfig_path '^librime\\.so\\.')"
  if [ -z "$rime_lib" ]; then
    rime_lib="$(find_ldconfig_path '^librime\\.so$')"
  fi
  if [ -z "$rime_lib" ]; then
    rime_lib="$(find /usr/lib /lib -type f -name 'librime.so*' 2>/dev/null | sort -V | tail -1 || true)"
  fi
  if [ -z "$rime_lib" ]; then
    echo "ERROR: cannot find librime.so in the Linux build image" >&2
    exit 1
  fi
  copied_rime="$(copy_runtime_lib "$rime_lib" "$LINUX_RUNTIME_LIB_DIR")"
  copy_selected_deps "$copied_rime"

  for libdir in \
    "$(dirname "$rime_lib")" \
    /usr/lib/*-linux-gnu \
    /usr/lib \
    /usr/local/lib; do
    [ -d "$libdir" ] || continue
    copy_rime_plugins "$libdir"
  done

  find "$LINUX_RUNTIME_LIB_DIR" -type f -name '*.so*' | while IFS= read -r lib; do
    patchelf --set-rpath '$ORIGIN:$ORIGIN/..:$ORIGIN/rime-plugins' "$lib" 2>/dev/null || true
  done
}

chmod -R u+w target/release/bundle/ 2>/dev/null || true
find target/release/bundle -type f \( -name '*.tar.gz' -o -iname '*.appimage' \) -delete 2>/dev/null || true
rm -rf target/release/bundle/appimage target/release/bundle/appimage* 2>/dev/null || true
pnpm install --frozen-lockfile
ensure_linux_rime_data
cargo build -p keytao-linux-ime --release
prepare_linux_runtime
target_triple="$(rustc -vV | sed -n 's/^host: //p')"
sidecar="src-tauri/binaries/keytao-ime-${target_triple}"
mkdir -p "$(dirname "$sidecar")"
cp /app/target/release/keytao-ime "$sidecar"
chmod +x "$sidecar"
patchelf --set-rpath "$LINUX_RUNTIME_RPATH" /app/target/release/keytao-ime
patchelf --set-rpath "$LINUX_RUNTIME_RPATH" "$sidecar"
export KEYTAO_IME_PATH=/app/target/release/keytao-ime
pnpm tauri build --bundles deb,rpm --config src-tauri/tauri.linux.conf.json
patchelf --set-rpath "$LINUX_RUNTIME_RPATH" target/release/keytao-app 2>/dev/null || true
patchelf --set-rpath "$LINUX_RUNTIME_RPATH" target/release/keytao-ime 2>/dev/null || true
rm -rf target/release/runtime
cp -a "$LINUX_RUNTIME_DIR" target/release/runtime
/app/scripts/verify-linux-bundles.sh /app/target/release/bundle
