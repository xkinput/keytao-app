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

## Package contract

- `scripts/build-windows-flutter.ps1`: KeyTao / `ink.rea.keytao-app`,
  workspace version, build order and verified installer output.
- `packaging/windows/keytao.nsi`: publisher `rea`, perMachine installation,
  `Software\Microsoft\Windows\CurrentVersion\Uninstall\KeyTao` and
  `Software\rea\KeyTao` registry keys.
- `flutter_app/windows/CMakeLists.txt`: current/arm64x/x86 staging,
  app DLLs at the executable root and packaged resources.
- `.github/workflows/release.yml`: canonical output directory and
  `keytao-app-<version>-windows-x64-setup.exe` release asset name.
- `crates/keytao-app-core/src/ime_status/windows.rs`:
  runtime lookup under `resource_dir/keytao-windows-ime-runtime`, then rime-data.
  `crates/keytao-app-core/src/addon.rs`: root addon-schemas lookup.

The standalone installer restores that original directory, stops the old app,
runs its own uninstaller with `/S /UPDATE _?=...`, and installs the Flutter
payload with the same HKLM uninstall key. User data at
`%LOCALAPPDATA%\ink.rea.keytao-app` and `%APPDATA%\keytao` is never removed.
`nsis-hooks.nsh` owns the IME installation hooks, including
versioned ProgramData runtimes, AppContainer RX, protocol ownership and TSF
registration/unregistration. New uninstallers delete only the generated list
of installed files; unrelated files in a custom install directory are retained.

The native runner sends only `appArgs(List<String>)` on `keytao/windows`.
First-launch arguments still use `dart_entrypoint_arguments`. Forwarded launches
wait for the native channel and queue until the first frame. The Dart handler
is installed during startup, before the first frame.

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
