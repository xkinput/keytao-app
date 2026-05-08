#!/bin/sh
# binfmt_misc handler for AppImage execution inside Docker on NixOS.
#
# NixOS registers /run/binfmt/appimage_type_{1,2} -> /nix/store/.../appimage-run
# which doesn't exist inside the Docker container.  build-linux.sh registers
# THIS script as the AppImage type-2 interpreter via binfmt_misc/register so
# the kernel calls us instead of the broken NixOS handler.
#
# Installed at: /opt/appimage-runner (referenced in binfmt register string)
# Calling convention (from kernel binfmt_misc):
#   $1 = path to the AppImage binary
#   $2.. = original arguments passed to the AppImage

APPIMAGE="$1"; shift
TMPDIR=$(mktemp -d /tmp/appimage-XXXXXX)

# Compute the squashfs offset from the ELF section table.
OFFSET=$(python3 -c "
import struct
hdr = open('$APPIMAGE', 'rb').read(64)
e_shoff   = struct.unpack_from('<Q', hdr, 40)[0]
e_shentsz = struct.unpack_from('<H', hdr, 58)[0]
e_shnum   = struct.unpack_from('<H', hdr, 60)[0]
print(e_shoff + e_shentsz * e_shnum)
")

unsquashfs -q -o "$OFFSET" -d "$TMPDIR/sq" "$APPIMAGE" >/dev/null 2>&1

"$TMPDIR/sq/AppRun" "$@"
_exit=$?

rm -rf "$TMPDIR"
exit $_exit
