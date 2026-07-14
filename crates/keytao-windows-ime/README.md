# KeyTao Windows IME

This crate builds a Windows TSF text service DLL for KeyTao. TSF is the standard
input method framework used by Windows 7, Windows 10, and Windows 11.

## Build

Install Rust, the MSVC x86/x64/ARM64/ARM64EC toolchains, and LLVM/Clang. The
KeyTao scripts download the official x86/x64 librime SDK and build native
ARM64 librime from the same upstream release tag. The ARM64 source build pins
and merges the same `librime-lua` revision recorded by the official Windows
SDK, so native ARM64 hosts expose the same Lua translators, filters, and
processors as x86/x64 hosts.

Example with Scoop:

```powershell
scoop install llvm
```

Then build the IME:

```powershell
.\scripts\build-windows-ime.ps1
```

Or through pnpm:

```powershell
pnpm build:windows-ime
```

`build-windows-ime.ps1` creates the selected architecture directory. The full
`build-windows.ps1 -Arch x64` installer build creates both architecture
directories and points `current` at x64:

```text
target\keytao-windows-ime-runtime\x64
target\keytao-windows-ime-runtime\x86
target\keytao-windows-ime-runtime\arm64
target\keytao-windows-ime-runtime\arm64x
target\keytao-windows-ime-runtime\current
```

TSF loads the IME DLL into each target process, so the package includes x86,
x64, and ARM64 code. On Windows on ARM, an ARM64X forwarder dispatches to the
native ARM64 target or the x64 target according to the host process. The
installer copies the complete runtimes into a unique directory below
`%ProgramData%\KeyTao\keytao-windows-ime-runtime` before registration. Loaded
TIP files are therefore never overwritten during an upgrade.

To only download librime:

```powershell
.\scripts\fetch-librime-windows.ps1
```

Manual builds are still possible. Load the generated environment first:

```powershell
. .\vendor\librime\windows-x64\env.ps1
cargo build -p keytao-windows-ime --target x86_64-pc-windows-msvc --release
```

For 32-bit applications on 64-bit Windows, also build and register the i686 DLL:

```powershell
cargo build -p keytao-windows-ime --target i686-pc-windows-msvc --release
```

The DLL links to librime at build time. If librime is not in a standard
toolchain path, set:

```powershell
$env:RIME_INCLUDE_DIR = "C:\path\to\librime\include"
$env:RIME_LIB_DIR = "C:\path\to\librime\lib"
```

For release packages, ship librime with KeyTao. Users should not need to install
Weasel or any other input method. A minimal runtime layout is:

```text
KeyTao\
  keytao_windows_ime.dll
  rime.dll / rime-arm64.dll with merged librime-lua
  librime-features.txt
  rime-data\
    default.yaml
    essay.txt
    ...
```

The input-switcher branding icon is PE resource ID 1 inside each target DLL;
its TSF profile index is the zero-based value `0`, so no external profile ICO
is required. It uses the same white-star identity as the macOS input source,
rendered on a dark tile so it remains legible on the light Windows taskbar.
Resource IDs 2 and 3 remain dedicated Chinese and English language-bar icons.
The text service also publishes the standard input-mode conversion compartment.

Candidate text uses the installed Windows font stack: a CJK face first, then
Segoe UI Emoji and Segoe UI Symbol for missing emoji and symbol glyphs. The
system fonts are referenced in place and are not redistributed in the package.

The IME first looks for `rime-data` next to `keytao_windows_ime.dll`, then under
`resources\rime-data` and `share\rime-data`.

The build and bundle verification scripts reject a Windows runtime unless its
feature manifest declares merged `librime-lua` support and the matching Rime
DLL contains the Lua translator, filter, and processor registrations.

## Rime Data

The IME looks for shared Rime data in this order:

- `rime-data` next to `keytao_windows_ime.dll`
- `resources\rime-data` next to `keytao_windows_ime.dll`
- `share\rime-data` next to `keytao_windows_ime.dll`
- `KEYTAO_RIME_SHARED_DATA_DIR`
- `RIME_SHARED_DATA_DIR`
- `RIME_DATA_DIR`
- `%ProgramFiles%\KeyTao\rime-data`
- `%ProgramFiles%\KeyTao\share\rime-data`
- `%WEASEL_ROOT%\data`
- `%ProgramFiles%\Rime\weasel-data`
- `%ProgramFiles(x86)%\Rime\weasel-data`

User data is stored under the platform config directory, normally:

```text
%APPDATA%\keytao
```

Install the KeyTao schema there with the app before enabling the IME.

## Register

Run PowerShell as administrator:

```powershell
regsvr32 .\target\x86_64-pc-windows-msvc\release\keytao_windows_ime.dll
```

Then open Windows language/input settings and add or select "键道输入法" under
Chinese (Simplified). To uninstall:

```powershell
regsvr32 /u .\target\x86_64-pc-windows-msvc\release\keytao_windows_ime.dll
```

On Windows 7, use the same commands from an elevated prompt. On x64 systems,
register the x64 DLL with `System32\regsvr32.exe` and the x86 DLL with
`SysWOW64\regsvr32.exe`. On ARM64 systems, register the ARM64X forwarder with
native `System32\regsvr32.exe`; the forwarder selects the matching ARM64 or x64
target in each host process.

The TIP initializes librime without deployment. Install or update schemas with
the KeyTao app before selecting the IME; deployment must not run inside an
application process that hosts the TSF DLL.
