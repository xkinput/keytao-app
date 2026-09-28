#!/usr/bin/env bash
# Stage an existing release bundle; --stage-only supports offline fixture checks.
# shellcheck disable=SC2016
set -euo pipefail
export LC_ALL=C
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# -lt 6 || $# -gt 7 || (${7:-} != '' && ${7:-} != --stage-only) ]]; then
  echo "Usage: $0 BUNDLE IME RUNTIME OUTPUT VERSION {x86_64|aarch64} [--stage-only]" >&2
  exit 2
fi
BUNDLE="$(cd "$1" && pwd)"
IME="$2"
RUNTIME="$(cd "$3" && pwd)"
OUTPUT="$4"
VERSION="$5"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]] || { echo 'Invalid package version' >&2; exit 2; }
case "$6" in
  x86_64) DEB_ARCH=amd64; RPM_ARCH=x86_64; ASSET_ARCH=x64 ;;
  aarch64) DEB_ARCH=arm64; RPM_ARCH=aarch64; ASSET_ARCH=arm64 ;;
  *) echo "Unsupported Linux architecture: $6" >&2; exit 2 ;;
esac
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"
ROOT="$OUTPUT/root"
# Refuse stale staging trees rather than mixing releases or deleting user files.
mkdir "$ROOT"
APP="$ROOT/usr/lib/KeyTao"
mkdir -p "$APP" "$ROOT/usr/bin" "$ROOT/usr/share/applications" \
  "$ROOT/usr/share/ibus/component" "$ROOT/etc/xdg/autostart"
cp -a "$BUNDLE/." "$APP/"
install -m 755 "$IME" "$APP/keytao-ime"
cp -a "$RUNTIME" "$APP/runtime"
ln -s runtime/rime-data "$APP/rime-data"
cp -a "$PROJECT_DIR/resources/addon-schemas" "$APP/addon-schemas"
ln -s ../lib/KeyTao/keytao-app "$ROOT/usr/bin/keytao-app"
ln -s ../lib/KeyTao/keytao-ime "$ROOT/usr/bin/keytao-ime"
install -m 644 "$PROJECT_DIR/packaging/linux/KeyTao.desktop" "$ROOT/usr/share/applications/KeyTao.desktop"
install -m 644 "$PROJECT_DIR/packaging/linux/keytao-ime.desktop" "$ROOT/etc/xdg/autostart/keytao-ime.desktop"
install -m 644 "$PROJECT_DIR/crates/keytao-linux-ime/keytao-wayland-launcher.desktop" "$ROOT/usr/share/applications/"
install -m 644 "$PROJECT_DIR/crates/keytao-linux-ime/keytao.xml" "$ROOT/usr/share/ibus/component/"
# Install the packaging PNG icons at their real sizes.
python3 - "$PROJECT_DIR" "$ROOT" <<'PY'
import pathlib, shutil, struct, sys
project, root = map(pathlib.Path, sys.argv[1:])
for source in sorted((project / 'packaging/icons').rglob('*.png')):
    header = source.read_bytes()[:24]
    assert header[:8] == b'\x89PNG\r\n\x1a\n', source
    width, height = struct.unpack('>II', header[16:24])
    assert width == height, source
    dest = root / f'usr/share/icons/hicolor/{width}x{height}/apps/keytao-app.png'
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, dest)
PY
if [[ ${7:-} == --stage-only ]]; then
  echo "Staged key-tao $VERSION $DEB_ARCH/$RPM_ARCH at $ROOT"
  exit 0
fi
[[ $(uname -s) == Linux ]] || { echo 'Package builds require Linux' >&2; exit 1; }

# The bridge hook links system Rime; give every Flutter native library a path to
# the private runtime closure. The runner keeps the template's $ORIGIN/lib.
patchelf --set-rpath '$ORIGIN/lib' "$APP/keytao-app"
patchelf --set-rpath '$ORIGIN/runtime/lib' "$APP/keytao-ime"
while IFS= read -r -d '' library; do
  patchelf --set-rpath '$ORIGIN:$ORIGIN/../runtime/lib' "$library"
done < <(find "$APP/lib" -type f -name '*.so*' -print0)

# dpkg-query and ldd are already in the builder. Declare the owning packages of
# external libraries; private bundled libraries deliberately have no system dep.
dependencies="$OUTPUT/dependencies"
# libepoxy opens GL/EGL dynamically; process discovery uses pgrep.
printf '%s\n' libegl1 libgl1 procps > "$dependencies"
while IFS= read -r -d '' binary; do
  file -b "$binary" | grep -q '^ELF ' || continue
  dynamic="$(readelf -d "$binary")"
  [[ "$dynamic" == *'(NEEDED)'* ]] || continue
  linked="$(env -u LD_LIBRARY_PATH ldd "$binary")"
  if [[ "$linked" == *'not found'* ]]; then
    printf 'Unresolved libraries in %s:\n%s\n' "$binary" "$linked" >&2
    exit 1
  fi
  while IFS= read -r library; do
    [[ "$library" == "$APP/"* ]] && continue
    resolved="$(readlink -f "$library")"
    owner="$(dpkg-query -S "$resolved" 2>/dev/null || dpkg-query -S "$library" 2>/dev/null || dpkg-query -S "${resolved#/usr}" 2>/dev/null)"
    owner="${owner%%: /*}"
    dpkg-query -W -f='${Package} (>= ${Version})\n' "$owner" >> "$dependencies"
  done < <(printf '%s\n' "$linked" | awk '/=> \// {print $3} /^[[:space:]]*\// {print $1}')
done < <(find "$APP" -type f -print0)
DEB_DEPENDS="$(sort -u "$dependencies" | paste -sd, - | sed 's/,/, /g')"
[[ "$DEB_DEPENDS" == *libgtk-3-* ]] || { echo 'Missing GTK dependency' >&2; exit 1; }
mkdir -p "$OUTPUT/bundle/deb" "$OUTPUT/bundle/rpm" "$OUTPUT/rpmbuild"
RPM_VERSION="${VERSION//-/_}"
rpmbuild -bb --target "$RPM_ARCH" \
  --define "_topdir $OUTPUT/rpmbuild" \
  --define "keytao_root $ROOT" --define "keytao_version $RPM_VERSION" \
  "$PROJECT_DIR/packaging/linux/key-tao.spec"
cp "$OUTPUT/rpmbuild/RPMS/$RPM_ARCH/key-tao-$RPM_VERSION-1.$RPM_ARCH.rpm" \
  "$OUTPUT/bundle/rpm/keytao-app-$VERSION-linux-$ASSET_ARCH.rpm"
mkdir "$ROOT/DEBIAN"
cat > "$ROOT/DEBIAN/control" <<EOF
Package: key-tao
Version: $VERSION
Architecture: $DEB_ARCH
Maintainer: Rea <hi@rea.ink>
Depends: $DEB_DEPENDS
Section: utils
Priority: optional
Installed-Size: $(du -sk "$ROOT/usr" | awk '{print $1}')
Description: KeyTao input method app
 Flutter application and Linux input method daemon with bundled Rime data.
EOF
dpkg-deb --root-owner-group --build "$ROOT" "$OUTPUT/bundle/deb/keytao-app-$VERSION-linux-$ASSET_ARCH.deb"
