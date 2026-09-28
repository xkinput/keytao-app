#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
IMAGE="keytao-app-builder"
command -v docker >/dev/null || { echo 'docker is required' >&2; exit 1; }
VERSION="$(sed -n 's/^version: \([^+]*\).*/\1/p' "$PROJECT_DIR/flutter_app/pubspec.yaml")"
[[ -n "$VERSION" ]] || { echo 'Missing Flutter version' >&2; exit 1; }
BUNDLE_DIR="$PROJECT_DIR/target/release/bundle"
mkdir -p "$BUNDLE_DIR" "$PROJECT_DIR/dist"

echo '==> Building Flutter Linux builder image'
tar -C "$PROJECT_DIR" -cf - scripts/Dockerfile.linux-builder rust-toolchain.toml \
  | docker build -f scripts/Dockerfile.linux-builder -t "$IMAGE" -
echo '==> Building daemon, Flutter release bundle, deb and rpm'
docker run --rm --pull=never \
  -v "$PROJECT_DIR":/app:ro -v "$BUNDLE_DIR":/out \
  -v keytao-app-cargo:/root/.cargo/registry \
  -v keytao-app-cargo-git:/root/.cargo/git \
  "$IMAGE" bash /app/scripts/container-build.sh "$(id -u)" "$(id -g)"

# Read the image architecture; the host may be macOS or use a remote Docker daemon.
case "$(docker image inspect "$IMAGE" --format '{{.Architecture}}')" in
  amd64) ASSET_ARCH=x64 ;;
  arm64) ASSET_ARCH=arm64 ;;
  *) echo 'Unsupported builder architecture' >&2; exit 1 ;;
esac
find "$PROJECT_DIR/dist" -maxdepth 1 -type f \
  \( -name '*.deb' -o -name '*.rpm' \) -delete
for format in deb rpm; do
  artifact="keytao-app-$VERSION-linux-$ASSET_ARCH.$format"
  cp "$BUNDLE_DIR/$format/$artifact" "$PROJECT_DIR/dist/$artifact"
  ls -lh "$PROJECT_DIR/dist/$artifact"
done
