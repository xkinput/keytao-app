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

The standalone installer restores the original directory, stops verified
KeyTao processes in that directory, and installs the Flutter payload with the
same HKLM uninstall key. Existing Flutter installations are updated in place,
including partially completed upgrades and versions whose old uninstaller
incorrectly blocked browsers holding input method DLLs. Their complete old
ProgramData runtimes remain available to running applications; each install
registers a fresh runtime directory. Superseded Flutter payload files can remain
until a future cleanup. Legacy Tauri installs still run their uninstaller with
`/S /UPDATE _?=...`. User data at
`%LOCALAPPDATA%\ink.rea.keytao-app` and `%APPDATA%\keytao` is never removed.
`nsis-hooks.nsh` owns the IME installation hooks, including
versioned ProgramData runtimes, AppContainer RX, protocol ownership and TSF
registration/unregistration. New uninstallers delete only the generated list
of installed files; unrelated files in a custom install directory are retained.
`replace-payload.nsh` writes files from the generated install manifest. An
existing destination is first renamed to a unique backup in the same directory;
if copying the new file fails, that file is restored. This also replaces a
legacy app-root `rime.dll` while browsers still map it. Mapped backups remain
valid for those processes and are scheduled for deletion at a later reboot.
Cleanup alone does not force an immediate restart.

Installation and uninstallation first show a read-only preview of this
installation's KeyTao processes. Clicking Continue force-stops those programs
and proceeds automatically. Finish KeyTao configuration/deployment first.
Weasel, browsers, Explorer, editors and other input method hosts stay open and never
block installation merely because they loaded a KeyTao/Rime DLL. Reopen those
applications later to activate the new input method code; switching input
methods alone may leave an old DLL loaded.

`check-running-processes.ps1` recognizes only `keytao-app.exe` and the legacy
`keytao.exe` at exact paths within the selected KeyTao installation. It does not
inspect Weasel registration or stop WeaselServer/WeaselDeployer, even if those
executables happen to be in the same directory. KeyTao uses its own in-process
librime library; it does not depend on Weasel's Rime server.
`-StopOwnedProcesses` enables stopping;
without it, the helper only previews. Before terminating a process it rechecks
its executable path and creation time using the same held handle, and never
terminates a process tree. The installer repeats this step immediately before
payload changes, including silent installation/uninstallation. Exit code 32
means a target KeyTao program remains running; 1 means detection/stopping
failed. These failures preserve installed files and registration. Interactive
users can retry or cancel. Loading an input method DLL is never proof that the
host process belongs to KeyTao.

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
Neither builds nor runs a Windows executable. On Windows with NSIS 3, also run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/windows/tests/check_running_processes_test.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/windows/tests/running_processes_gate_test.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/windows/tests/replace_payload_test.ps1
```

These start isolated fixture processes to verify selective stopping and compile
harmless NSIS fixtures to verify read-only previews, Continue/section stopping,
upgrade recovery, loaded-DLL replacement, Unicode reports and silent exit codes.
The replacement test also requires LLVM/clang and leaves the system reboot
deletion queue untouched. The fixtures never
launch the real installer or stop a user application. All checks also run in
the Windows release workflow.

## Required Windows CI and device acceptance

- Parse PowerShell scripts, build MSVC runner/resources and all IME targets,
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
