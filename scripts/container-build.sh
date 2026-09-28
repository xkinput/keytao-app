#!/usr/bin/env bash
# Runs inside the builder: /app is read-only source, /out is the package output.
# shellcheck disable=SC2016
set -euo pipefail
export LC_ALL=C
UID_GID="$1:$2"
WORK_DIR="$(mktemp -d /tmp/keytao-linux.XXXXXX)"
trap 'chown -R "$UID_GID" /out/deb /out/rpm 2>/dev/null || true' EXIT
# Keep Flutter native assets and Cargo outputs away from concurrent host builds.
tar -C /app --exclude=target --exclude=build --exclude=.build --exclude=.gradle \
  --exclude=.dart_tool --exclude=ephemeral \
  -cf - Cargo.toml Cargo.lock .cargo crates resources \
  packaging/icons \
  flutter_app/pubspec.yaml flutter_app/pubspec.lock flutter_app/lib \
  flutter_app/assets flutter_app/hook flutter_app/linux packaging/linux scripts \
  | tar -C "$WORK_DIR" -xf -
cd "$WORK_DIR"
export RUSTUP_TOOLCHAIN
RUSTUP_TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"/\1/p' /app/rust-toolchain.toml)"
# Native asset hooks scrub the environment. Their Cargo subprocess must also
# select the pinned host toolchain without installing the workspace mobile targets.
printf '[toolchain]\nchannel = "%s"\nprofile = "minimal"\n' "$RUSTUP_TOOLCHAIN" > rust-toolchain.toml
VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
[[ $(sed -n 's/^version: \([^+]*\).*/\1/p' flutter_app/pubspec.yaml) == "$VERSION" ]] || { echo 'Cargo/Flutter versions differ' >&2; exit 1; }
case "$(uname -m)" in
  x86_64) FLUTTER_ARCH=x64 ;;
  aarch64) FLUTTER_ARCH=arm64 ;;
  *) echo 'Only x86_64 and aarch64 are supported' >&2; exit 1 ;;
esac
LINUX_RUNTIME_DIR="$WORK_DIR/target/keytao-linux-runtime"
LINUX_RUNTIME_LIB_DIR="$LINUX_RUNTIME_DIR/lib"
LINUX_RUNTIME_RIME_DATA_DIR="$LINUX_RUNTIME_DIR/rime-data"

copy_runtime_lib() {
  local src="$1" dst_dir="${2:-$LINUX_RUNTIME_LIB_DIR}" dst
  [[ -f "$src" ]] || { echo "Missing runtime library: $src" >&2; return 1; }
  mkdir -p "$dst_dir"
  dst="$dst_dir/$(basename "$src")"
  # Only traverse newly copied libraries, so dependency cycles terminate.
  [[ ! -f "$dst" ]] || return 0
  cp -L "$src" "$dst"
  chmod u+w "$dst"
  printf '%s\n' "$dst"
}

copy_selected_deps() {
  local binary="$1" dep copied linked
  linked="$(ldd "$binary")"
  [[ "$linked" != *'not found'* ]] || { echo "$linked" >&2; return 1; }
  while IFS= read -r dep; do
    case "$(basename "$dep")" in
      librime*.so*|libopencc*.so*|libyaml-cpp*.so*|libglog*.so*|libmarisa*.so*|libleveldb*.so*|liblua*.so*|libboost*.so*|libcapnp*.so*|libkj*.so*|libkyotocabinet*.so*|libdouble-conversion*.so*|libabsl*.so*)
        copied="$(copy_runtime_lib "$dep")"
        if [[ -n "$copied" ]]; then copy_selected_deps "$copied"; fi
        ;;
    esac
  done < <(printf '%s\n' "$linked" | awk '/=> \// {print $3} /^[[:space:]]*\// {print $1}')
}

prepare_linux_runtime() {
  local candidate rime_data_src='' rime_lib libdir plugin copied lib
  mkdir -p "$LINUX_RUNTIME_LIB_DIR" "$LINUX_RUNTIME_RIME_DATA_DIR"
  cp crates/keytao-theme/default-theme.yaml "$LINUX_RUNTIME_DIR/default-theme.yaml"
  for candidate in "${KEYTAO_RIME_SHARED_DATA_DIR:-}" "${RIME_SHARED_DATA_DIR:-}" \
    "${RIME_DATA_DIR:-}" /usr/share/rime-data /usr/local/share/rime-data; do
    if [[ -f "$candidate/default.yaml" ]]; then rime_data_src="$candidate"; break; fi
  done
  if [[ -z "$rime_data_src" ]]; then
    # Reuse the old build's data fetcher when the image has no shared Rime data.
    bash scripts/fetch-librime.sh --platform linux --destination "$WORK_DIR/target/keytao-linux-rime"
    rime_data_src="$WORK_DIR/target/keytao-linux-rime/rime-data"
  fi
  cp -aL "$rime_data_src/." "$LINUX_RUNTIME_RIME_DATA_DIR/"
  if [[ ! -d "$LINUX_RUNTIME_RIME_DATA_DIR/opencc" ]]; then
    cp -aL /usr/share/opencc "$LINUX_RUNTIME_RIME_DATA_DIR/opencc"
  fi
  rime_lib="$(ldconfig -p | awk '$1 == "librime.so.1" {print $NF; exit}')"
  [[ -n "$rime_lib" ]] || { echo 'Missing librime.so.1 in builder' >&2; return 1; }
  copied="$(copy_runtime_lib "$rime_lib")"
  copy_selected_deps "$copied"
  for libdir in "$(dirname "$rime_lib")" /usr/lib/*-linux-gnu /usr/lib /usr/local/lib; do
    [[ -d "$libdir/rime-plugins" ]] || continue
    while IFS= read -r -d '' plugin; do
      copied="$(copy_runtime_lib "$plugin" "$LINUX_RUNTIME_LIB_DIR/rime-plugins")"
      if [[ -n "$copied" ]]; then copy_selected_deps "$copied"; fi
    done < <(find "$libdir/rime-plugins" -maxdepth 1 -name '*.so*' -print0)
  done
  [[ -f "$LINUX_RUNTIME_LIB_DIR/rime-plugins/librime-lua.so" ]] || { echo 'Missing Rime Lua plugin' >&2; return 1; }
  while IFS= read -r -d '' lib; do
    patchelf --set-rpath '$ORIGIN:$ORIGIN/..:$ORIGIN/rime-plugins' "$lib"
  done < <(find "$LINUX_RUNTIME_LIB_DIR" -type f -name '*.so*' -print0)
}

cargo build --locked -p keytao-linux-ime --bin keytao-ime --release
prepare_linux_runtime
(
  cd flutter_app
  flutter build linux --release --target-platform "linux-$FLUTTER_ARCH" --build-name "$VERSION"
)
bash packaging/linux/build-packages.sh \
  "flutter_app/build/linux/$FLUTTER_ARCH/release/bundle" target/release/keytao-ime \
  "$LINUX_RUNTIME_DIR" "$WORK_DIR/packages" "$VERSION" "$(uname -m)"
bash scripts/verify-linux-bundles.sh "$WORK_DIR/packages/bundle" "$VERSION" --inside-container
# Publish only verified artifacts; remove stale packages from previous builds.
mkdir -p /out/deb /out/rpm
find /out/deb /out/rpm -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) -delete
cp "$WORK_DIR/packages/bundle/deb/"*.deb /out/deb/
cp "$WORK_DIR/packages/bundle/rpm/"*.rpm /out/rpm/
