# Linux package compatibility

## Published alpha.88 evidence

The task owner supplied the following metadata extracted from the published
`keytao-app-1.2.1-alpha.88-linux-x64.deb` and `.rpm`. This is a transcription
of that evidence, not a fresh artifact extraction in this offline task.

- deb: `Package: key-tao`, `Version: 1.2.1-alpha.88`, `Architecture: amd64`
  (`arm64` for aarch64), `Maintainer: Rea <hi@rea.ink>`.
- deb dependencies: `libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0`.
  There was no `postinst`.
- rpm: `Name: key-tao`, `Version: 1.2.1-alpha.88`, `Release: 1`.
- Entries: `/usr/bin/keytao-app`, `/usr/bin/keytao-ime`.
- Prefix: `/usr/lib/KeyTao/`, including `addon-schemas/` and
  `runtime/{rime-data,default-theme.yaml,lib/}`.
- Runtime libraries: `librime.so.1`, `libmarisa.so.0`, `libopencc.so.1.1`,
  `libyaml-cpp.so.0.8`, `liblua5.4.so.0`, `libglog.so.1`, `libleveldb.so.1d`,
  and `rime-plugins/librime-lua.so`.
- Desktop: `/usr/share/applications/KeyTao.desktop`, with `Exec=keytao-app`,
  `StartupWMClass=keytao-app`, `Icon=keytao-app`, `Name=KeyTao`,
  `Comment=KeyTao input method app`, empty `Categories`, `Terminal=false`,
  `Type=Application`.
- Icons: `/usr/share/icons/hicolor/<size>/apps/keytao-app.png`.

## Flutter layout and verification

The plan's section 4 and the survey incorrectly named `/usr/lib/keytao-app`.
Use `/usr/lib/KeyTao` above. Keep the package name, entry points, desktop name
and icon names. The runner retains application ID `ink.rea.keytao-app` and sets
the X11 program class explicitly to `keytao-app`, matching `StartupWMClass`.

The two `/usr/bin` entries are relative symlinks to the real bundle executables.
Both Rust `current_exe()` and Dart `resolvedExecutable` then locate the bundle
root. Runtime data stays at `runtime/rime-data`; `rime-data` also links there
for the bridge resource lookup. Add-on schemas stay at `addon-schemas`.
The app uses `$ORIGIN/lib`, the bridge libraries use `$ORIGIN/../runtime/lib`,
and the daemon uses `$ORIGIN/runtime/lib`. The package carries the Rime closure;
system dependencies are derived from the remaining ELF dependencies, plus
GL/EGL (loaded dynamically) and process tools, without WebKit or a system
librime dependency. Missing base data uses the existing `fetch-librime.sh`
Linux fetcher during CI builds.

`rpmbuild` rejects hyphens in the Version tag. Only the RPM header replaces
hyphens with underscores (`1.2.1_alpha.89`); RPM compares these separators
equally. Release remains `1`, and the downloadable filename and deb version
keep `1.2.1-alpha.89`. The verifier checks RPM ordering against the published
alpha.88 version using RPM's own comparator.

Run `python3 packaging/linux/check.py` for offline staging/contract fixtures.
Run `bash scripts/build-linux.sh` on each native Linux architecture to build
and verify both packages. The build isolates source/cache writes in the
container. `bash scripts/verify-linux-bundles.sh` can recheck the output using
the existing builder image (no image pull); ELF checks run inside that image.
Actual apt/dnf upgrades and GTK/KDE/GNOME behavior require Linux acceptance.
