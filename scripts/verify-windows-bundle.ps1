param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [string]$ReleaseDir,
    [string]$BundleDir
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSCommandPath) ".."))
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

function Require-AsciiMarkers([string]$Path, [string[]]$Markers) {
    $text = [System.Text.Encoding]::ASCII.GetString(
        [System.IO.File]::ReadAllBytes($Path)
    )
    foreach ($marker in $Markers) {
        if (-not $text.Contains($marker)) {
            throw "$Path is missing required runtime marker '$marker'"
        }
    }
}

function Require-MergedLuaManifest([string]$Path) {
    Require-File $Path "Missing librime feature manifest: $Path"
    $features = Get-Content -LiteralPath $Path -Raw
    if ($features -notmatch '(?m)^librime-lua=merged\r?$') {
        throw "$Path does not declare merged librime-lua support"
    }
    if ($features -notmatch '(?m)^librime-lua-ref=[0-9a-f]{7,40}\r?$') {
        throw "$Path does not record the librime-lua source revision"
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

function Require-NoCrtDependency([string]$Dll) {
    $dumpbin = Find-OnPath "dumpbin.exe"
    if (-not $dumpbin) {
        Write-Warning "dumpbin.exe was not found; skipping static CRT verification for $Dll."
        return
    }

    $output = & $dumpbin /dependents $Dll 2>$null | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin.exe failed while checking CRT imports for $Dll"
    }
    if ($output -match "(?im)^\s*(VCRUNTIME|MSVCP|UCRTBASE)[^\s]*\.dll\s*$") {
        throw "$Dll must statically link the Rust/MSVC CRT so background TSF work cannot call an unloaded CRT module."
    }
}

function Require-Arm64X([string]$Dll) {
    $dumpbin = Find-OnPath "dumpbin.exe"
    if (-not $dumpbin) {
        throw "dumpbin.exe is required to verify the ARM64X forwarder: $Dll"
    }
    $output = & $dumpbin /headers $Dll 2>$null | Out-String
    if ($LASTEXITCODE -ne 0 -or $output -notmatch "(?i)ARM64X") {
        throw "$Dll is not a valid ARM64X binary"
    }
}

function Require-Exports([string]$Dll, [string[]]$Exports) {
    $dumpbin = Find-OnPath "dumpbin.exe"
    if (-not $dumpbin) {
        throw "dumpbin.exe is required to verify COM exports: $Dll"
    }
    $output = & $dumpbin /exports $Dll 2>$null | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin.exe failed while checking exports for $Dll"
    }
    foreach ($export in $Exports) {
        if ($output -notmatch "(?m)\b$([regex]::Escape($export))\b") {
            throw "$Dll is missing required COM export $export"
        }
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

Require-Directory $ReleaseDir "Missing Windows release directory: $ReleaseDir"
Require-Directory $BundleDir "Missing Windows bundle directory: $BundleDir"

$nsisDir = Join-Path $BundleDir "nsis"
Require-Directory $nsisDir "Missing Windows NSIS bundle directory: $nsisDir"

$configPath = Join-Path $repoRoot "src-tauri\tauri.conf.json"
$config = Get-Content $configPath -Raw | ConvertFrom-Json
$expectedInstallerName = "$($config.productName)_$($config.version)_$Arch-setup.exe"
$expectedInstallerPath = Join-Path $nsisDir $expectedInstallerName
Require-File $expectedInstallerPath "Missing current Windows NSIS installer: $expectedInstallerPath"
$installer = Get-Item -LiteralPath $expectedInstallerPath
$unexpectedExeInstallers = Get-ChildItem -Path $nsisDir -Filter "*.exe" -File |
    Where-Object { $_.FullName -ne $installer.FullName }
if ($unexpectedExeInstallers) {
    throw "Windows NSIS directory contains stale installers: $($unexpectedExeInstallers.FullName -join ', ')"
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
$imeLibrimeFeatures = Join-Path $imeRuntimeDir "librime-features.txt"
$imeVcRuntime = Join-Path $imeRuntimeDir "vcruntime140.dll"
$imeX86RuntimeDir = Join-Path $ReleaseDir "keytao-windows-ime-runtime\x86"
$imeX86Dll = Join-Path $imeX86RuntimeDir "keytao_windows_ime.dll"
$imeX86RimeDll = Join-Path $imeX86RuntimeDir "rime.dll"
$imeX86RimeData = Join-Path $imeX86RuntimeDir "rime-data\default.yaml"
$imeX86DefaultTheme = Join-Path $imeX86RuntimeDir "default-theme.yaml"
$imeX86LibrimeFeatures = Join-Path $imeX86RuntimeDir "librime-features.txt"
$imeX86VcRuntime = Join-Path $imeX86RuntimeDir "vcruntime140.dll"
$imeArm64XRuntimeDir = Join-Path $ReleaseDir "keytao-windows-ime-runtime\arm64x"
$imeArm64XForwarder = Join-Path $imeArm64XRuntimeDir "keytao_windows_ime.dll"
$imeArm64X64Target = Join-Path $imeArm64XRuntimeDir "keytao_windows_ime_x64.dll"
$imeArm64Target = Join-Path $imeArm64XRuntimeDir "keytao_windows_ime_arm64.dll"
$imeArm64XRimeX64 = Join-Path $imeArm64XRuntimeDir "rime.dll"
$imeArm64XRimeArm64 = Join-Path $imeArm64XRuntimeDir "rime-arm64.dll"
$imeArm64XRimeData = Join-Path $imeArm64XRuntimeDir "rime-data\default.yaml"
$imeArm64XDefaultTheme = Join-Path $imeArm64XRuntimeDir "default-theme.yaml"
$imeArm64XLibrimeFeatures = Join-Path $imeArm64XRuntimeDir "librime-features.txt"
$imeArm64LibrimeFeatures = Join-Path $imeArm64XRuntimeDir "librime-arm64-features.txt"
$appRimeDll = Join-Path $ReleaseDir "rime.dll"
$hookFile = Join-Path $repoRoot "src-tauri\windows\nsis-hooks.nsh"
$appSource = Join-Path $repoRoot "src-tauri\src\lib.rs"
$coreSource = Join-Path $repoRoot "crates\keytao-core\src\lib.rs"
$registrationSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\registration.rs"
$globalsSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\globals.rs"
$languageBarSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\language_bar.rs"
$imeLibSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\lib.rs"
$imeStateSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\state.rs"
$themeSource = Join-Path $repoRoot "crates\keytao-theme\src\lib.rs"
$imeBrandIconSource = Join-Path $repoRoot "crates\keytao-windows-ime\ime-brand.ico"
$imeBrandSvgSource = Join-Path $repoRoot "crates\keytao-windows-ime\ime-brand.svg"
$imeChineseModeIconSource = Join-Path $repoRoot "crates\keytao-windows-ime\mode-zh.ico"
$imeEnglishModeIconSource = Join-Path $repoRoot "crates\keytao-windows-ime\mode-en.ico"

Require-File $appExe "Windows release payload is missing keytao-app.exe"
Require-File $imeDll "Windows release payload is missing keytao_windows_ime.dll"
Require-File $imeRimeDll "Windows IME runtime is missing rime.dll"
Require-File $imeRimeData "Windows IME runtime is missing rime-data\default.yaml"
Require-File $imeDefaultTheme "Windows IME runtime is missing default-theme.yaml"
Require-File $imeVcRuntime "Windows IME runtime is missing vcruntime140.dll"
Require-File $appRimeDll "Windows app payload is missing rime.dll next to keytao-app.exe"
Require-File $imeBrandSvgSource "Windows IME branding source is missing ime-brand.svg"
Require-MergedLuaManifest $imeLibrimeFeatures
Require-AsciiMarkers $imeRimeDll @("lua_translator", "lua_filter", "lua_processor")
Require-AsciiMarkers $appRimeDll @("lua_translator", "lua_filter", "lua_processor")
$nativeMachine = if ($Arch -eq "x64") { [UInt16]0x8664 } else { [UInt16]0x014C }
Require-PeMachine $appExe $nativeMachine "$Arch application"
Require-PeMachine $imeDll $nativeMachine $Arch
Require-PeMachine $imeRimeDll $nativeMachine $Arch
Require-PeMachine $imeVcRuntime $nativeMachine $Arch
Require-DelayLoadedDependency $imeDll "rime.dll"
Require-NoCrtDependency $imeDll
Require-EmbeddedIcon $imeDll 1
Verify-AuthenticodeSignature $imeDll
if ($Arch -eq "x64") {
    Require-File $imeX86Dll "Windows x64 packages must include the x86 TSF DLL for 32-bit applications"
    Require-File $imeX86RimeDll "Windows x86 IME runtime is missing rime.dll"
    Require-File $imeX86RimeData "Windows x86 IME runtime is missing rime-data\default.yaml"
    Require-File $imeX86DefaultTheme "Windows x86 IME runtime is missing default-theme.yaml"
    Require-File $imeX86VcRuntime "Windows x86 IME runtime is missing vcruntime140.dll"
    Require-MergedLuaManifest $imeX86LibrimeFeatures
    Require-AsciiMarkers $imeX86RimeDll @("lua_translator", "lua_filter", "lua_processor")
    Require-PeMachine $imeX86Dll ([UInt16]0x014C) "x86"
    Require-PeMachine $imeX86RimeDll ([UInt16]0x014C) "x86"
    Require-PeMachine $imeX86VcRuntime ([UInt16]0x014C) "x86"
    Require-DelayLoadedDependency $imeX86Dll "rime.dll"
    Require-NoCrtDependency $imeX86Dll
    Require-EmbeddedIcon $imeX86Dll 1
    Verify-AuthenticodeSignature $imeX86Dll

    Require-File $imeArm64XForwarder "Windows x64 packages must include the ARM64X TSF forwarder"
    Require-File $imeArm64X64Target "Windows ARM64X runtime is missing its x64 TSF target"
    Require-File $imeArm64Target "Windows ARM64X runtime is missing its native ARM64 TSF target"
    Require-File $imeArm64XRimeX64 "Windows ARM64X runtime is missing x64 rime.dll"
    Require-File $imeArm64XRimeArm64 "Windows ARM64X runtime is missing native rime-arm64.dll"
    Require-File $imeArm64XRimeData "Windows ARM64X runtime is missing rime-data\default.yaml"
    Require-File $imeArm64XDefaultTheme "Windows ARM64X runtime is missing default-theme.yaml"
    Require-MergedLuaManifest $imeArm64XLibrimeFeatures
    Require-MergedLuaManifest $imeArm64LibrimeFeatures
    Require-AsciiMarkers $imeArm64XRimeX64 @("lua_translator", "lua_filter", "lua_processor")
    Require-AsciiMarkers $imeArm64XRimeArm64 @("lua_translator", "lua_filter", "lua_processor")
    Require-PeMachine $imeArm64XForwarder ([UInt16]0xAA64) "ARM64X"
    Require-PeMachine $imeArm64X64Target ([UInt16]0x8664) "x64"
    Require-PeMachine $imeArm64Target ([UInt16]0xAA64) "ARM64"
    Require-PeMachine $imeArm64XRimeX64 ([UInt16]0x8664) "x64"
    Require-PeMachine $imeArm64XRimeArm64 ([UInt16]0xAA64) "ARM64"
    Require-Arm64X $imeArm64XForwarder
    Require-Exports $imeArm64XForwarder @("DllCanUnloadNow", "DllGetClassObject", "DllRegisterServer", "DllUnregisterServer")
    Require-DelayLoadedDependency $imeArm64X64Target "rime.dll"
    Require-DelayLoadedDependency $imeArm64Target "rime-arm64.dll"
    Require-NoCrtDependency $imeArm64X64Target
    Require-NoCrtDependency $imeArm64Target
    foreach ($targetDll in @($imeArm64X64Target, $imeArm64Target)) {
        Require-EmbeddedIcon $targetDll 1
        Require-EmbeddedIcon $targetDll 2
        Require-EmbeddedIcon $targetDll 3
        Verify-AuthenticodeSignature $targetDll
    }
    Verify-AuthenticodeSignature $imeArm64XForwarder
}
Require-File $hookFile "Missing NSIS installer hook file: $hookFile"
Require-File $appSource "Missing Tauri application source: $appSource"
Require-File $coreSource "Missing KeyTao core source: $coreSource"
Require-File $registrationSource "Missing Windows TSF registration source: $registrationSource"
Require-File $globalsSource "Missing Windows TSF lifecycle source: $globalsSource"
Require-File $languageBarSource "Missing Windows TSF language bar source: $languageBarSource"
Require-File $imeLibSource "Missing Windows TSF library source: $imeLibSource"
Require-File $imeStateSource "Missing Windows TSF state source: $imeStateSource"
Require-File $themeSource "Missing shared IME theme source: $themeSource"
Require-File $imeBrandIconSource "Missing dedicated Windows IME branding icon: $imeBrandIconSource"
Require-File $imeChineseModeIconSource "Missing Windows IME Chinese mode icon: $imeChineseModeIconSource"
Require-File $imeEnglishModeIconSource "Missing Windows IME English mode icon: $imeEnglishModeIconSource"
Require-IcoFrames $imeBrandIconSource @(16, 20, 24, 32, 40, 48)
Require-IcoFrames $imeChineseModeIconSource @(16, 20, 24, 32)
Require-IcoFrames $imeEnglishModeIconSource @(16, 20, 24, 32)

Require-Pattern $hookFile 'NSIS_HOOK_POSTINSTALL' "NSIS hook file does not define NSIS_HOOK_POSTINSTALL"
Require-Pattern $hookFile 'NSIS_HOOK_PREUNINSTALL' "NSIS hook file does not define NSIS_HOOK_PREUNINSTALL"
Require-Pattern $hookFile 'regsvr32\.exe' "NSIS hook file does not invoke regsvr32.exe"
Require-Pattern $hookFile 'ExecWait.*regsvr32\.exe' "NSIS hook must wait for regsvr32.exe so TSF registration is complete before install finishes"
Require-Pattern $hookFile 'SysWOW64\\regsvr32\.exe' "NSIS hook does not register the x86 TSF DLL"
Require-Pattern $hookFile 'IsNativeARM64' "NSIS hook must select the ARM64X runtime on native ARM64 Windows"
Require-Pattern $hookFile 'ReadEnvStr.*ProgramData' "NSIS hook must resolve the system ProgramData directory"
Require-Pattern $hookFile '\$R6\\KeyTao\\keytao-windows-ime-runtime' "NSIS hook must stage the text service outside the replaceable app directory"
Require-Pattern $hookFile 'GetTempFileName' "NSIS hook must allocate a unique runtime directory so loaded TIP DLLs are never overwritten"
Require-Pattern $hookFile 'WindowsImeRuntimeDir' "NSIS hook must persist the active versioned runtime for uninstall"
Require-Pattern $hookFile 'robocopy\.exe' "NSIS hook must copy complete native and x86 runtimes before registration"
Require-Pattern $appSource 'SourceDirectory' "The elevated app registration flow must track the x86 runtime source directory"
Require-Pattern $appSource 'Copy-Item -Destination' "The elevated app registration flow must stage the complete x86 runtime before regsvr32"
Require-Pattern $appSource 'windows_ime_versioned_runtime_root' "The app registration repair must use a unique versioned runtime directory"
Require-Pattern $registrationSource 'InstallLayoutOrTip' "Windows TSF registration must call InstallLayoutOrTip so the profile is added to the current user's input methods"
Require-Pattern $registrationSource 'PROFILE_ICON_INDEX' "Windows TSF registration must use the embedded branding icon resource"
Require-Pattern $imeLibSource 'PROFILE_ICON_INDEX:\s*u32\s*=\s*0' "Windows TSF profile icon index must be zero-based"
Require-Pattern $imeBrandSvgSource 'id="keytao-star"' "Windows TSF profile icon must use the KeyTao star identity"
if ((Get-Content -Raw -LiteralPath $imeBrandSvgSource) -match '<text(?:\s|>)') {
    throw "Windows TSF profile icon must not fall back to a text glyph"
}
Require-Pattern $globalsSource 'GET_MODULE_HANDLE_EX_FLAG_PIN' "The in-process TSF module must remain loaded while background engine work can execute"
Require-Pattern $languageBarSource 'ITfLangBarItemButton' "Windows TSF must expose a standard Chinese/English language bar item"
Require-Pattern $languageBarSource 'item\.Show\(BOOL::from\(true\)\)' "Windows TSF must explicitly show its language bar item after registration"
Require-Pattern $languageBarSource 'GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION' "Windows TSF must publish its input mode through the standard conversion compartment"
Require-Pattern $coreSource 'KeyTao\.WindowsIme\.EngineInit' "Windows IME engine mutex name must be shared by the app and TSF"
Require-Pattern $imeStateSource 'WINDOWS_IME_ENGINE_INIT_MUTEX_NAME' "Windows TSF must use the shared cross-process engine mutex"
Require-Pattern $appSource 'WindowsImeEngineInitGuard::acquire' "The Windows app must serialize deployment with TSF engine initialization"
Require-Pattern $imeStateSource 'session_reset_pending' "Windows TSF focus callbacks must defer librime session reset"
Require-Pattern $themeSource 'RegGetValueW' "Windows candidate rendering must read the system theme without spawning a child process"

$windowsConfig = Get-Content (Join-Path $repoRoot "src-tauri\tauri.windows.conf.json") -Raw | ConvertFrom-Json
$resourceKeys = @($windowsConfig.bundle.resources.PSObject.Properties.Name)
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/current") {
    throw "Windows resources must include the IME runtime directory"
}
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/x86") {
    throw "Windows resources must include the x86 IME runtime directory"
}
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/arm64x") {
    throw "Windows resources must include the ARM64X IME runtime directory"
}
if ($resourceKeys -notcontains "../target/keytao-windows-app-runtime/*.dll") {
    throw "Windows resources must include all app runtime DLLs at the installer root"
}
if ($windowsConfig.bundle.resources.PSObject.Properties["../target/keytao-windows-app-runtime/*.dll"].Value -ne "") {
    throw "Windows app runtime DLLs must be installed next to keytao-app.exe"
}
if ($windowsConfig.bundle.windows.nsis.installMode -ne "perMachine") {
    throw "Windows NSIS installer must use perMachine install mode for TSF registration"
}
if ($windowsConfig.bundle.windows.nsis.installerHooks -ne "windows/nsis-hooks.nsh") {
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
Require-Pattern $installerScript.FullName 'keytao-windows-ime-runtime\\arm64x' "Generated Windows installer script does not install the ARM64X IME runtime"
Require-Pattern $installerScript.FullName 'default-theme\.yaml' "Generated Windows installer script does not install the shared default theme"
Require-Pattern $installerScript.FullName 'librime-features\.txt' "Generated Windows installer script does not install the librime feature manifest"
Require-Pattern $installerScript.FullName 'librime-arm64-features\.txt' "Generated Windows installer script does not install the native ARM64 librime feature manifest"
Require-Pattern $installerScript.FullName '/oname=.*rime\.dll' "Generated Windows installer script does not install rime.dll next to keytao-app.exe"

Verify-AuthenticodeSignature $installer.FullName

Write-Host "Windows bundle verification passed"
Write-Host "  Installer: $($installer.FullName)"
Write-Host "  App: $appExe"
Write-Host "  IME runtime: $imeRuntimeDir ($Arch)"
