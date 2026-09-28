#!/usr/bin/env bash
# Inspect extracted packages; ELF resolution always runs in the Linux builder.
# shellcheck disable=SC2016
set -euo pipefail
export LC_ALL=C
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE_DIR="${1:-$PROJECT_DIR/target/release/bundle}"
VERSION="${2:-$(sed -n 's/^version: \([^+]*\).*/\1/p' "$PROJECT_DIR/flutter_app/pubspec.yaml")}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]] || { echo 'Invalid expected version' >&2; exit 2; }
BUNDLE_DIR="$(cd "$BUNDLE_DIR" && pwd)"
if [[ ${3:-} != --inside-container ]]; then
  command -v docker >/dev/null || { echo 'Verification requires the existing Linux builder image' >&2; exit 1; }
  exec docker run --rm --pull=never --network=none \
    -v "$PROJECT_DIR":/app:ro -v "$BUNDLE_DIR":/bundles:ro \
    keytao-app-builder bash /app/scripts/verify-linux-bundles.sh /bundles "$VERSION" --inside-container
fi
[[ $(uname -s) == Linux ]] || { echo 'ELF checks require Linux' >&2; exit 1; }
for command_name in dpkg dpkg-deb rpm rpm2cpio cpio patchelf readelf ldd python3; do
  command -v "$command_name" >/dev/null || { echo "Missing $command_name" >&2; exit 1; }
done
case "$(uname -m)" in
  x86_64) DEB_ARCH=amd64; RPM_ARCH=x86_64; ASSET_ARCH=x64; ELF_MACHINE='Advanced Micro Devices X86-64' ;;
  aarch64) DEB_ARCH=arm64; RPM_ARCH=aarch64; ASSET_ARCH=arm64; ELF_MACHINE=AArch64 ;;
  *) echo 'Unsupported verification architecture' >&2; exit 1 ;;
esac
fail() { echo "ERROR: $*" >&2; exit 1; }
DEB="$BUNDLE_DIR/deb/keytao-app-$VERSION-linux-$ASSET_ARCH.deb"
RPM="$BUNDLE_DIR/rpm/keytao-app-$VERSION-linux-$ASSET_ARCH.rpm"
[[ -f "$DEB" && -f "$RPM" ]] || fail "Missing deb/rpm pair for $VERSION $ASSET_ARCH"
[[ -z $(find "$BUNDLE_DIR" -type f \( -iname '*.appimage' -o -name '*.tar.gz' \) -print) ]] || fail 'Unexpected AppImage/tarball'
[[ $(dpkg-deb -f "$DEB" Package) == key-tao ]] || fail 'Wrong deb package name'
[[ $(dpkg-deb -f "$DEB" Version) == "$VERSION" ]] || fail 'Wrong deb version'
[[ $(dpkg-deb -f "$DEB" Architecture) == "$DEB_ARCH" ]] || fail 'Wrong deb architecture'
[[ $(dpkg-deb -f "$DEB" Maintainer) == 'Rea <hi@rea.ink>' ]] || fail 'Wrong deb maintainer'
depends="$(dpkg-deb -f "$DEB" Depends)"
[[ "$depends" == *libgtk-3-* && "$depends" != *webkit* && "$depends" != *librime* ]] || fail "Wrong deb dependencies: $depends"
RPM_VERSION="${VERSION//-/_}"
[[ $(rpm -qp --qf '%{NAME} %{VERSION} %{RELEASE} %{ARCH}' "$RPM") == "key-tao $RPM_VERSION 1 $RPM_ARCH" ]] || fail 'Wrong rpm identity'
requires="$(rpm -qp --requires "$RPM")"
[[ "$requires" == *gtk3* && "$requires" != *webkit* ]] || fail 'Wrong rpm dependencies'
[[ -z $(rpm -qp --scripts "$RPM") ]] || fail 'Unexpected rpm scriptlets'
[[ $(rpm --eval "%{lua:print(rpm.vercmp('$RPM_VERSION', '$VERSION'))}") == 0 ]] || fail 'RPM version normalization changed ordering'
[[ $(rpm --eval "%{lua:print(rpm.vercmp('$RPM_VERSION', '1.2.1-alpha.88'))}") == 1 ]] || fail 'RPM does not upgrade alpha.88'
dpkg --compare-versions "$VERSION" gt 1.2.1-alpha.88 || fail 'deb does not upgrade alpha.88'

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir "$WORK_DIR/deb" "$WORK_DIR/rpm"
dpkg-deb -R "$DEB" "$WORK_DIR/deb"
(cd "$WORK_DIR/rpm" && rpm2cpio "$RPM" | cpio --quiet -idm --no-absolute-filenames)
# Inspect files themselves, not presentation-dependent dpkg/rpm listing text.
python3 "$PROJECT_DIR/packaging/linux/check.py" "$WORK_DIR/deb" "$WORK_DIR/rpm"
for format in deb rpm; do
  app="$WORK_DIR/$format/usr/lib/KeyTao"
  [[ $(patchelf --print-rpath "$app/keytao-app") == '$ORIGIN/lib' ]] || fail 'Wrong app RPATH'
  [[ $(patchelf --print-rpath "$app/keytao-ime") == '$ORIGIN/runtime/lib' ]] || fail 'Wrong daemon RPATH'
  while IFS= read -r -d '' binary; do
    case "$binary" in
      "$app/keytao-app"|"$app/keytao-ime"|*.so|*.so.*) ;;
      *) continue ;;
    esac
    header="$(readelf -h "$binary")" || fail "Invalid ELF: $binary"
    [[ "$header" == *"$ELF_MACHINE"* ]] || fail "Wrong ELF architecture: $binary"
    # Dart's AOT image can have no DT_NEEDED entries.
    dynamic="$(readelf -d "$binary")"
    [[ "$dynamic" == *'(NEEDED)'* ]] || continue
    linked="$(env -u LD_LIBRARY_PATH ldd "$binary")" || fail "ldd failed: $binary"
    [[ "$linked" != *'not found'* ]] || fail "$binary: $linked"
    # A builder's installed librime must not conceal a broken private RPATH.
    while IFS= read -r library; do
      [[ "$library" == "$app/"* ]] || fail "System Rime dependency leaked: $library"
    done < <(printf '%s\n' "$linked" | awk '/^[[:space:]]*lib(rime|opencc|yaml-cpp|glog|marisa|leveldb|lua)[^ ]* =>/ {print $3}')
  done < <(find "$app" -type f -print0)
  echo "PASS: $format key-tao $VERSION $RPM_ARCH; all ELF dependencies resolve"
done
echo '==> Linux bundle verification passed'
