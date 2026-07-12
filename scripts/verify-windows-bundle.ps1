param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [string]$ReleaseDir,
    [string]$BundleDir
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path (Split-Path -Parent $PSCommandPath) "..")).Path
if (-not $ReleaseDir) {
    $ReleaseDir = Join-Path $repoRoot "target\release"
}
if (-not $BundleDir) {
    $BundleDir = Join-Path $ReleaseDir "bundle"
}

function Require-File([string]$Path, [string]$Message) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw $Message
    }
}

function Require-Directory([string]$Path, [string]$Message) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw $Message
    }
}

function Require-Pattern([string]$Path, [string]$Pattern, [string]$Message) {
    if (-not (Select-String -Path $Path -Pattern $Pattern -Quiet)) {
        throw $Message
    }
}

function Find-OnPath([string]$Name) {
    $result = & cmd.exe /d /c "where `"$Name`" 2>nul"
    if ($LASTEXITCODE -eq 0 -and $result) {
        return $result | Select-Object -First 1
    }
    return $null
}

function Require-DelayLoadedDependency([string]$Dll, [string]$Dependency) {
    $dumpbin = Find-OnPath "dumpbin.exe"
    if (-not $dumpbin) {
        Write-Warning "dumpbin.exe was not found; skipping delay-load import verification for $Dll."
        return
    }

    $output = & $dumpbin /dependents $Dll 2>$null | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin.exe failed while checking delay-load imports for $Dll"
    }

    if ($output -notmatch "(?is)delay load dependencies:\s*.*$([regex]::Escape($Dependency))") {
        throw "$Dependency must be delay-loaded by $Dll; otherwise TSF may load librime while switching input methods."
    }
}

function Require-PeMachine([string]$Path, [UInt16]$ExpectedMachine, [string]$ExpectedName) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "$Path is not a valid PE file"
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "$Path does not contain a valid PE header"
        }
        $machine = $reader.ReadUInt16()
        if ($machine -ne $ExpectedMachine) {
            throw "$Path has PE machine 0x$($machine.ToString('X4')); expected $ExpectedName (0x$($ExpectedMachine.ToString('X4')))"
        }
    } finally {
        $stream.Dispose()
    }
}

function Require-IcoFrames([string]$Path, [int[]]$RequiredSizes) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 6 -or [BitConverter]::ToUInt16($bytes, 0) -ne 0 -or [BitConverter]::ToUInt16($bytes, 2) -ne 1) {
        throw "$Path is not a valid ICO file"
    }
    $count = [BitConverter]::ToUInt16($bytes, 4)
    if ($bytes.Length -lt (6 + 16 * $count)) {
        throw "$Path has a truncated ICO directory"
    }

    $frames = @{}
    for ($index = 0; $index -lt $count; $index++) {
        $offset = 6 + 16 * $index
        $width = if ($bytes[$offset] -eq 0) { 256 } else { [int]$bytes[$offset] }
        $height = if ($bytes[$offset + 1] -eq 0) { 256 } else { [int]$bytes[$offset + 1] }
        $bitCount = [BitConverter]::ToUInt16($bytes, $offset + 6)
        if ($width -eq $height -and $bitCount -eq 32) {
            $frames[$width] = $true
        }
    }

    foreach ($size in $RequiredSizes) {
        if (-not $frames.ContainsKey($size)) {
            throw "$Path must contain a ${size}x${size} 32-bit icon frame"
        }
    }
}

function Require-EmbeddedIcon([string]$Dll, [int]$ResourceId) {
    if (-not ("KeyTaoResourceProbe" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class KeyTaoResourceProbe {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr LoadLibraryEx(string fileName, IntPtr file, uint flags);

    [DllImport("kernel32.dll")]
    public static extern bool FreeLibrary(IntPtr module);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr LoadImage(IntPtr instance, IntPtr name, uint type, int width, int height, uint flags);

    [DllImport("user32.dll")]
    public static extern bool DestroyIcon(IntPtr icon);
}
"@
    }

    $loadLibraryAsDataFile = 0x00000002
    $loadLibraryAsImageResource = 0x00000020
    $imageIcon = 1
    $loadDefaultSize = 0x00000040
    $module = [KeyTaoResourceProbe]::LoadLibraryEx(
        $Dll,
        [IntPtr]::Zero,
        $loadLibraryAsDataFile -bor $loadLibraryAsImageResource
    )
    if ($module -eq [IntPtr]::Zero) {
        throw "Unable to open $Dll as an image resource"
    }

    try {
        $icon = [KeyTaoResourceProbe]::LoadImage(
            $module,
            [IntPtr]$ResourceId,
            $imageIcon,
            0,
            0,
            $loadDefaultSize
        )
        if ($icon -eq [IntPtr]::Zero) {
            throw "Windows IME DLL does not contain branding icon resource $ResourceId"
        }
        [KeyTaoResourceProbe]::DestroyIcon($icon) | Out-Null
    } finally {
        [KeyTaoResourceProbe]::FreeLibrary($module) | Out-Null
    }
}

function Verify-AuthenticodeSignature([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -eq "Valid") {
        return
    }
    $message = "Windows artifact is not signed by a trusted Authenticode certificate: $Path ($($signature.Status))"
    if ($env:KEYTAO_REQUIRE_WINDOWS_SIGNATURE -eq "1") {
        throw $message
    }
    Write-Warning "$message. Set KEYTAO_REQUIRE_WINDOWS_SIGNATURE=1 for production release enforcement."
}

function Find-LuaPlugin([string[]]$Dirs) {
    foreach ($dir in $Dirs) {
        if (-not $dir -or -not (Test-Path -LiteralPath $dir -PathType Container)) {
            continue
        }
        $plugin = Get-ChildItem -Path $dir -File |
            Where-Object {
                $name = $_.Name.ToLowerInvariant()
                $name.Contains("rime") -and $name.Contains("lua") -and $name.EndsWith(".dll")
            } |
            Select-Object -First 1
        if ($plugin) {
            return $plugin.FullName
        }
    }
    return $null
}

Require-Directory $ReleaseDir "Missing Windows release directory: $ReleaseDir"
Require-Directory $BundleDir "Missing Windows bundle directory: $BundleDir"

$nsisDir = Join-Path $BundleDir "nsis"
Require-Directory $nsisDir "Missing Windows NSIS bundle directory: $nsisDir"

$installer = Get-ChildItem -Path $nsisDir -Filter "*.exe" -File | Select-Object -First 1
if (-not $installer) {
    throw "Missing Windows NSIS .exe installer"
}

$forbiddenInstallers = Get-ChildItem -Path $BundleDir -Recurse -File |
    Where-Object { $_.Extension -in @(".msi", ".zip", ".appx", ".msix", ".msixbundle") }
if ($forbiddenInstallers) {
    throw "Windows build must only produce the NSIS .exe installer. Unexpected artifacts: $($forbiddenInstallers.FullName -join ', ')"
}

$appExe = Join-Path $ReleaseDir "keytao-app.exe"
$imeRuntimeDir = Join-Path $ReleaseDir "keytao-windows-ime-runtime\current"
$imeDll = Join-Path $imeRuntimeDir "keytao_windows_ime.dll"
$imeRimeDll = Join-Path $imeRuntimeDir "rime.dll"
$imeRimeData = Join-Path $imeRuntimeDir "rime-data\default.yaml"
$imeDefaultTheme = Join-Path $imeRuntimeDir "default-theme.yaml"
$imeVcRuntime = Join-Path $imeRuntimeDir "vcruntime140.dll"
$imeX86RuntimeDir = Join-Path $ReleaseDir "keytao-windows-ime-runtime\x86"
$imeX86Dll = Join-Path $imeX86RuntimeDir "keytao_windows_ime.dll"
$imeX86RimeDll = Join-Path $imeX86RuntimeDir "rime.dll"
$imeX86RimeData = Join-Path $imeX86RuntimeDir "rime-data\default.yaml"
$imeX86DefaultTheme = Join-Path $imeX86RuntimeDir "default-theme.yaml"
$imeX86VcRuntime = Join-Path $imeX86RuntimeDir "vcruntime140.dll"
$appRimeDll = Join-Path $ReleaseDir "rime.dll"
$imeLuaPlugin = Find-LuaPlugin @($imeRuntimeDir, (Join-Path $imeRuntimeDir "rime-plugins"))
$appLuaPlugin = Find-LuaPlugin @($ReleaseDir, (Join-Path $ReleaseDir "rime-plugins"))
$hookFile = Join-Path $repoRoot "src-tauri\windows\nsis-hooks.nsh"
$appSource = Join-Path $repoRoot "src-tauri\src\lib.rs"
$registrationSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\registration.rs"
$imeBrandIconSource = Join-Path $repoRoot "crates\keytao-windows-ime\ime-brand.ico"

Require-File $appExe "Windows release payload is missing keytao-app.exe"
Require-File $imeDll "Windows release payload is missing keytao_windows_ime.dll"
Require-File $imeRimeDll "Windows IME runtime is missing rime.dll"
Require-File $imeRimeData "Windows IME runtime is missing rime-data\default.yaml"
Require-File $imeDefaultTheme "Windows IME runtime is missing default-theme.yaml"
Require-File $imeVcRuntime "Windows IME runtime is missing vcruntime140.dll"
Require-File $appRimeDll "Windows app payload is missing rime.dll next to keytao-app.exe"
$nativeMachine = if ($Arch -eq "x64") { [UInt16]0x8664 } else { [UInt16]0x014C }
Require-PeMachine $imeDll $nativeMachine $Arch
Require-PeMachine $imeRimeDll $nativeMachine $Arch
Require-PeMachine $imeVcRuntime $nativeMachine $Arch
Require-DelayLoadedDependency $imeDll "rime.dll"
Require-EmbeddedIcon $imeDll 1
Verify-AuthenticodeSignature $imeDll
if ($Arch -eq "x64") {
    Require-File $imeX86Dll "Windows x64 packages must include the x86 TSF DLL for 32-bit applications"
    Require-File $imeX86RimeDll "Windows x86 IME runtime is missing rime.dll"
    Require-File $imeX86RimeData "Windows x86 IME runtime is missing rime-data\default.yaml"
    Require-File $imeX86DefaultTheme "Windows x86 IME runtime is missing default-theme.yaml"
    Require-File $imeX86VcRuntime "Windows x86 IME runtime is missing vcruntime140.dll"
    Require-PeMachine $imeX86Dll ([UInt16]0x014C) "x86"
    Require-PeMachine $imeX86RimeDll ([UInt16]0x014C) "x86"
    Require-PeMachine $imeX86VcRuntime ([UInt16]0x014C) "x86"
    Require-DelayLoadedDependency $imeX86Dll "rime.dll"
    Require-EmbeddedIcon $imeX86Dll 1
    Verify-AuthenticodeSignature $imeX86Dll
}
if (-not $imeLuaPlugin) {
    Write-Warning "Windows IME runtime does not include the librime-lua plugin DLL; Lua extensions will be unavailable in this Windows package."
}
if (-not $appLuaPlugin) {
    Write-Warning "Windows app payload does not include the librime-lua plugin DLL next to keytao-app.exe."
}
Require-File $hookFile "Missing NSIS installer hook file: $hookFile"
Require-File $appSource "Missing Tauri application source: $appSource"
Require-File $registrationSource "Missing Windows TSF registration source: $registrationSource"
Require-File $imeBrandIconSource "Missing dedicated Windows IME branding icon: $imeBrandIconSource"
Require-IcoFrames $imeBrandIconSource @(16, 20, 24, 32, 40, 48)

Require-Pattern $hookFile 'NSIS_HOOK_POSTINSTALL' "NSIS hook file does not define NSIS_HOOK_POSTINSTALL"
Require-Pattern $hookFile 'NSIS_HOOK_PREUNINSTALL' "NSIS hook file does not define NSIS_HOOK_PREUNINSTALL"
Require-Pattern $hookFile 'regsvr32\.exe' "NSIS hook file does not invoke regsvr32.exe"
Require-Pattern $hookFile 'ExecWait.*regsvr32\.exe' "NSIS hook must wait for regsvr32.exe so TSF registration is complete before install finishes"
Require-Pattern $hookFile 'SysWOW64\\regsvr32\.exe' "NSIS hook does not register the x86 TSF DLL"
Require-Pattern $hookFile 'KEYTAO_IME_X86_INSTALL_DIR.*\$PROGRAMFILES32\\KeyTao' "NSIS hook must install the x86 text service under Program Files (x86)"
Require-Pattern $hookFile 'robocopy\.exe' "NSIS hook must copy the complete x86 runtime into Program Files (x86) before registration"
Require-Pattern $appSource 'ProgramFiles\(x86\)' "The app must locate and repair the standard Program Files (x86) text service runtime"
Require-Pattern $appSource 'SourceDirectory' "The elevated app registration flow must track the x86 runtime source directory"
Require-Pattern $appSource 'Copy-Item -Destination' "The elevated app registration flow must stage the complete x86 runtime before regsvr32"
Require-Pattern $registrationSource 'InstallLayoutOrTip' "Windows TSF registration must call InstallLayoutOrTip so the profile is added to the current user's input methods"
Require-Pattern $registrationSource 'PROFILE_ICON_INDEX' "Windows TSF registration must use the embedded branding icon resource"

$config = Get-Content (Join-Path $repoRoot "src-tauri\tauri.windows.conf.json") -Raw | ConvertFrom-Json
$resourceKeys = @($config.bundle.resources.PSObject.Properties.Name)
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/current") {
    throw "Windows resources must include the IME runtime directory"
}
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/x86") {
    throw "Windows resources must include the x86 IME runtime directory"
}
if ($resourceKeys -notcontains "../target/keytao-windows-app-runtime/*.dll") {
    throw "Windows resources must include all app runtime DLLs at the installer root"
}
if ($config.bundle.resources.PSObject.Properties["../target/keytao-windows-app-runtime/*.dll"].Value -ne "") {
    throw "Windows app runtime DLLs must be installed next to keytao-app.exe"
}
if ($config.bundle.windows.nsis.installMode -ne "perMachine") {
    throw "Windows NSIS installer must use perMachine install mode for TSF registration"
}
if ($config.bundle.windows.nsis.installerHooks -ne "windows/nsis-hooks.nsh") {
    throw "Windows NSIS installerHooks must point to windows/nsis-hooks.nsh"
}

$installerScript = Get-ChildItem -Path $ReleaseDir -Recurse -Filter "installer.nsi" -File |
    Select-Object -First 1
if (-not $installerScript) {
    throw "Missing generated NSIS installer script"
}

Require-Pattern $installerScript.FullName 'nsis-hooks\.nsh' "Generated Windows installer script does not include the KeyTao NSIS hook file"
Require-Pattern $installerScript.FullName 'keytao_windows_ime\.dll' "Generated Windows installer script does not install keytao_windows_ime.dll"
Require-Pattern $installerScript.FullName 'keytao-windows-ime-runtime' "Generated Windows installer script does not install the IME runtime directory"
Require-Pattern $installerScript.FullName 'keytao-windows-ime-runtime\\x86' "Generated Windows installer script does not install the x86 IME runtime"
Require-Pattern $installerScript.FullName 'default-theme\.yaml' "Generated Windows installer script does not install the shared default theme"
Require-Pattern $installerScript.FullName '/oname=.*rime\.dll' "Generated Windows installer script does not install rime.dll next to keytao-app.exe"
if ($imeLuaPlugin -or $appLuaPlugin) {
    Require-Pattern $installerScript.FullName 'rime.*lua.*\.dll' "Generated Windows installer script does not install the librime-lua plugin DLL"
}

Verify-AuthenticodeSignature $installer.FullName

Write-Host "Windows bundle verification passed"
Write-Host "  Installer: $($installer.FullName)"
Write-Host "  App: $appExe"
Write-Host "  IME runtime: $imeRuntimeDir ($Arch)"
