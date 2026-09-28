# Flutter Windows package

Build on x64 Windows with Flutter, Rust, LLVM, Visual Studio C++ (including
ARM64/ARM64EC tools), and NSIS 3 with its standard plugins installed:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows-flutter.ps1 -Arch x64
```

The workspace Cargo version is authoritative. The output is
`target/release/bundle/nsis/keytao-app-<version>-windows-x64-setup.exe`.
The app is x64; its TSF payload also serves x86 and native ARM64/ARM64EC hosts.
`-DryRun` reports the build plan without building or installing anything.

## Compatibility evidence

- `src-tauri/tauri.conf.json:3-5`: KeyTao, workspace-synchronized version,
  `ink.rea.keytao-app`. The Tauri publisher default is the second identifier
  component (`rea`), documented in local `tauri-utils-2.9.0/src/config.rs:1572`.
- `src-tauri/tauri.windows.conf.json:7-15`: current/arm64x/x86 staging,
  app DLLs at the executable root, perMachine NSIS.
- `.github/workflows/release.yml:184-189`: canonical output directory and
  `keytao-app-<version>-windows-x64-setup.exe` release asset name.
- `crates/keytao-app-core/src/ime_status/windows.rs:408-448,641-645`:
  runtime lookup under `resource_dir/keytao-windows-ime-runtime`, then rime-data.
  `crates/keytao-app-core/src/addon.rs:159-165`: root addon-schemas lookup.
- The installed Tauri CLI 2.11.0 binary embeds its NSIS template. Offline
  `strings` evidence was saved at `/private/tmp/keytao-b4w-tauri-cli-strings.txt`:
  lines 7011-7013 define the KeyTao uninstall key and manufacturer/product key;
  7387-7404 choose Program Files/KeyTao for x64 perMachine;
  7725-7729 restore the default value of `Software\rea\KeyTao`;
  7616-7718 define `/UPDATE` uninstall behavior that preserves user data.

The standalone installer restores that original directory, stops the old app,
runs its own uninstaller with `/S /UPDATE _?=...`, and installs the Flutter
payload with the same HKLM uninstall key. User data at
`%LOCALAPPDATA%\ink.rea.keytao-app` and `%APPDATA%\keytao` is never removed.
`nsis-hooks.nsh` is a byte-for-byte port of the existing Tauri hooks, including
versioned ProgramData runtimes, AppContainer RX, protocol ownership and TSF
registration/unregistration. New uninstallers delete only the generated list
of installed files; unrelated files in a custom install directory are retained.

The native runner sends only `appArgs(List<String>)` on `keytao/windows`.
First-launch arguments still use `dart_entrypoint_arguments`. Forwarded launches
wait for the native channel and queue until the first frame. B3 owns installing
the Dart handler during startup, before the first frame.

## Offline checks

```sh
python3 packaging/windows/check-static.py
clang++ -std=c++17 -Wall -Wextra -Werror -Ipackaging/windows/tests -Iflutter_app/windows/runner packaging/windows/tests/single_instance_test.cpp flutter_app/windows/runner/single_instance.cpp -o /private/tmp/keytao-b4w-single-instance-test
/private/tmp/keytao-b4w-single-instance-test
```

The first uses the real CMake configuration/install rules with a fake engine
and non-executable payload; missing staging/version cases must fail. The second
compiles the real single-instance state machine against mock Win32 calls.
Neither builds nor runs a Windows executable. PowerShell and NSIS parsing were
unavailable on the macOS implementation host; those checks require CI tooling.

## Required Windows CI and device acceptance

- B5 must switch the workflow to `build-windows-flutter.ps1` and upload its exact
  returned artifact. The old Tauri build entry no longer matches the migrated
  verifier. Avoid copying the canonical installer onto itself in the old upload
  rename step. B4w does not edit the workflow or the old build script.
- Parse both PowerShell scripts, build MSVC runner/resources and all IME targets,
  compile NSIS, then pass `verify-windows-bundle.ps1` without `-LayoutOnly` or
  `-SkipInstaller`. It checks PE architecture, resource hashes, app dependency
  closure, TSF exports/icons/load behavior, version resources and the installer.
- Install and upgrade an actual old Tauri package in default and custom paths;
  preserve app data/credentials/Rime data and keep one uninstall entry. Test
  silent upgrade, locked files and failed uninstaller creation; uninstall must
  unregister TSF/protocol and retain user data/unrelated custom-directory files.
- Test first launch and concurrent/second launch, minimized-window restore,
  Unicode/empty/quoted arguments, and `keytao://ime/redeploy` reaching Dart once.
  Check UAC registration, x86/x64 apps and native ARM64/ARM64EC on actual hosts.
