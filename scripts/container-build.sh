#!/bin/sh
# Runs INSIDE the Docker container. Called by build-linux.sh via docker run.
# Arguments: $1=uid $2=gid
set -eu

UID_GID="$1:$2"

echo "=== Cache contents ==="
ls -lah /root/.cache/tauri/ 2>/dev/null || echo "(empty)"

# -----------------------------------------------------------------------
# binfmt_misc setup
# On NixOS, the host's binfmt_misc is bind-mounted read-only into the
# container.  The NixOS AppImage handler points into /nix/store which
# doesn't exist here.  Fix: unmount the host bind-mount and mount a
# fresh binfmt_misc instance scoped to this container's mount namespace.
# With --privileged, we have CAP_SYS_ADMIN + a separate mount namespace,
# so this doesn't affect the host.
# -----------------------------------------------------------------------
echo "=== binfmt_misc setup ==="
umount /proc/sys/fs/binfmt_misc 2>/dev/null && echo "Unmounted host binfmt_misc" || echo "(was not mounted)"

if mount -t binfmt_misc none /proc/sys/fs/binfmt_misc 2>/dev/null; then
  echo "Mounted fresh binfmt_misc"
else
  modprobe binfmt_misc 2>/dev/null || true
  mount -t binfmt_misc none /proc/sys/fs/binfmt_misc 2>/dev/null \
    && echo "Mounted binfmt_misc (after modprobe)" \
    || echo "WARNING: could not mount binfmt_misc"
fi

# Register AppImage type 1 + 2 handlers
# printf interprets \x escapes -> kernel receives raw bytes for magic matching
printf ':docker-ai1:M:8:\x41\x49\x01::/opt/appimage-runner:\n' \
  > /proc/sys/fs/binfmt_misc/register 2>&1 \
  && echo "binfmt type1 registered" || echo "binfmt type1 FAILED"
printf ':docker-ai2:M:8:\x41\x49\x02::/opt/appimage-runner:\n' \
  > /proc/sys/fs/binfmt_misc/register 2>&1 \
  && echo "binfmt type2 registered" || echo "binfmt type2 FAILED"

echo "binfmt entries:"
ls /proc/sys/fs/binfmt_misc/ 2>/dev/null || echo "(none)"

# -----------------------------------------------------------------------

chmod -R u+w target/release/bundle/ 2>/dev/null || true
pnpm install --frozen-lockfile
pnpm tauri build --bundles deb,appimage
chown -R "$UID_GID" /app/target /app/dist 2>/dev/null || true
