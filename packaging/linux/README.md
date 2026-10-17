# Linux packages

`scripts/build-linux.sh` builds the `key-tao` deb and rpm for the native
architecture of the Docker builder. `packaging/linux/build-packages.sh` stages
the tree and `packaging/linux/key-tao.spec` packages it as rpm.

## Installed files

- `/usr/lib/KeyTao/`: Flutter bundle, `keytao-ime`, `runtime/` (librime and its
  libraries, `rime-plugins`, `rime-data`), `rime-data -> runtime/rime-data`,
  and `addon-schemas/`.
- `/usr/bin/keytao-app`, `/usr/bin/keytao-ime`: relative symlinks into
  `/usr/lib/KeyTao`, so `current_exe()` and Dart `resolvedExecutable` find the
  bundle root.
- `/usr/share/applications/KeyTao.desktop`: app entry (`StartupWMClass=keytao-app`).
- `/usr/share/applications/keytao-wayland-launcher.desktop`: KDE virtual
  keyboard entry.
- `/usr/share/ibus/component/keytao.xml`: IBus component.
- `/etc/xdg/autostart/keytao-ime.desktop`: login autostart (`NotShowIn=GNOME;`).
- `/usr/share/icons/hicolor/<size>/apps/keytao-app.png`.

RPATHs: the app uses `$ORIGIN/lib`, bridge libraries `$ORIGIN:$ORIGIN/../runtime/lib`,
and the daemon `$ORIGIN/runtime/lib`. No system librime or WebKit is required.

## Dependencies

- deb: `libegl1, libgl1, procps` plus the owning packages of every external
  library the bundle links, computed with `ldd`/`dpkg-query` in
  `build-packages.sh`.
- rpm: `gtk3, libEGL.so.1()(64bit), libGL.so.1()(64bit), procps-ng` (see
  `key-tao.spec`).

## Versions

`rpmbuild` rejects hyphens in `Version`, so only the RPM header replaces them
with underscores (`1.2.1_alpha.N`); file names and the deb version keep the
hyphen. `scripts/verify-linux-bundles.sh` checks that both formats upgrade the
last Tauri-era release `1.2.1-alpha.88`.

## Checks

- `python3 packaging/linux/check.py`: offline staging and contract fixtures.
- `scripts/build-linux.sh`: build both packages into `dist/`.
- `scripts/verify-linux-bundles.sh [target/release/bundle] [<version>]`: recheck
  the built packages inside the existing builder image.
