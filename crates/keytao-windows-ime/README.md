# KeyTao Windows IME

This crate builds a Windows TSF text service DLL for KeyTao. TSF is the standard
input method framework used by Windows 7, Windows 10, and Windows 11.

## Build

Install Rust, the MSVC toolchain, and LLVM/Clang. The KeyTao scripts download
the official librime Windows SDK from GitHub releases and place it under
`vendor\librime`.

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

The script creates a test runtime directory:

```text
target\keytao-windows-ime-runtime\x64
```

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
  rime.dll / librime.dll and dependency DLLs
  rime-data\
    default.yaml
    essay.txt
    ...
```

The IME first looks for `rime-data` next to `keytao_windows_ime.dll`, then under
`resources\rime-data` and `share\rime-data`.

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

On Windows 7, use the same commands from an elevated prompt. On 64-bit systems,
register the 64-bit DLL with the normal `System32\regsvr32.exe`; register the
32-bit DLL with `SysWOW64\regsvr32.exe` only if you need 32-bit client support.
