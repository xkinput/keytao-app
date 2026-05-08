#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DIST_DIR="$PROJECT_DIR/dist"
IMAGE="keytao-installer-builder"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found. Install Docker: https://docs.docker.com/engine/install/" >&2
  exit 1
fi

mkdir -p "$DIST_DIR"

echo "==> Caching linuxdeploy tools (download if missing)..."
CACHE_DIR="$SCRIPT_DIR/cache"
mkdir -p "$CACHE_DIR"
_gh="https://github.com"
if [ ! -f "$CACHE_DIR/linuxdeploy-x86_64.AppImage" ]; then
  curl -fSL "$_gh/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" \
       -o "$CACHE_DIR/linuxdeploy-x86_64.AppImage"
fi
if [ ! -f "$CACHE_DIR/linuxdeploy-plugin-appimage-x86_64.AppImage" ]; then
  curl -fSL "$_gh/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage" \
       -o "$CACHE_DIR/linuxdeploy-plugin-appimage-x86_64.AppImage"
fi
if [ ! -f "$CACHE_DIR/linuxdeploy-plugin-gtk.sh" ]; then
  curl -fSL "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" \
       -o "$CACHE_DIR/linuxdeploy-plugin-gtk.sh"
fi
if [ ! -f "$CACHE_DIR/linuxdeploy-plugin-gstreamer.sh" ]; then
  curl -fSL "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gstreamer/master/linuxdeploy-plugin-gstreamer.sh" \
       -o "$CACHE_DIR/linuxdeploy-plugin-gstreamer.sh"
fi
if [ ! -f "$CACHE_DIR/AppRun-x86_64" ]; then
  curl -fSL "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64" \
       -o "$CACHE_DIR/AppRun-x86_64"
fi
chmod +x "$CACHE_DIR"/linuxdeploy-x86_64.AppImage \
         "$CACHE_DIR"/linuxdeploy-plugin-appimage-x86_64.AppImage \
         "$CACHE_DIR"/linuxdeploy-plugin-gtk.sh \
         "$CACHE_DIR"/linuxdeploy-plugin-gstreamer.sh \
         "$CACHE_DIR"/AppRun-x86_64

echo "==> Building builder image..."
docker build -f "$SCRIPT_DIR/Dockerfile.linux-builder" -t "$IMAGE" "$PROJECT_DIR"

echo "==> Building deb + AppImage inside container..."
_uid=$(id -u)
_gid=$(id -g)
docker run --rm \
  --network=host \
  --privileged \
  -v "$PROJECT_DIR":/app \
  -v keytao-installer-cargo:/root/.cargo/registry \
  -v keytao-installer-cargo-git:/root/.cargo/git \
  -w /app \
  "$IMAGE" \
  sh /app/scripts/container-build.sh "$_uid" "$_gid"

echo ""
echo "==> Artifacts:"
ls -lh "$DIST_DIR"/*.AppImage "$DIST_DIR"/*.deb 2>/dev/null \
  || ls -lh "$PROJECT_DIR"/target/release/bundle/appimage/*.AppImage \
            "$PROJECT_DIR"/target/release/bundle/deb/*.deb 2>/dev/null \
  || echo "(check target/release/bundle/)"

exit 0
