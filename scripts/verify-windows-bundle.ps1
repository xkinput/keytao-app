param(
    [ValidateSet("x64")]
    [string]$Arch = "x64",
    [string]$ReleaseDir,
    [string]$BundleDir,
    [string]$InstallerPath,
    [switch]$SkipInstaller,
    [switch]$LayoutOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSCommandPath) ".."))
if (-not $ReleaseDir) {
    $ReleaseDir = Join-Path $repoRoot "flutter_app\build\windows\x64\runner\Release"
}
if (-not $BundleDir) {
    $BundleDir = Join-Path $repoRoot "target\release\bundle"
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

Require-Directory $ReleaseDir "Missing Windows Flutter release directory: $ReleaseDir"
$manifest = Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*"(?<version>[^"\r\n]+)"') {
    throw "Missing workspace.package.version"
}
$version = $Matches.version
if (-not $InstallerPath) {
    $InstallerPath = Join-Path $BundleDir "nsis\keytao-app-$version-windows-$Arch-setup.exe"
}

$appExe = Join-Path $ReleaseDir "keytao-app.exe"
foreach ($file in @("keytao-app.exe", "flutter_windows.dll", "rime.dll", "vcruntime140.dll",
        "vcruntime140_1.dll", "msvcp140.dll", "data\icudtl.dat", "data\app.so", "rime-data\default.yaml")) {
    Require-File (Join-Path $ReleaseDir $file) "Missing Flutter bundle file: $file"
}
Require-Directory (Join-Path $ReleaseDir "data\flutter_assets") "Missing Flutter assets"
$bridgeDlls = @(Get-ChildItem -LiteralPath $ReleaseDir -Filter "*keytao_app_bridge*.dll" -File)
if ($bridgeDlls.Count -ne 1) { throw "Expected exactly one KeyTao Rust bridge DLL next to the executable" }
if (Test-Path -LiteralPath (Join-Path $ReleaseDir "keytao.exe")) { throw "Stale template keytao.exe in bundle" }
Require-PeMachine $appExe ([UInt16]0x8664) "x64 application"
foreach ($dll in Get-ChildItem -LiteralPath $ReleaseDir -Filter "*.dll" -File) {
    Require-PeMachine $dll.FullName ([UInt16]0x8664) "x64 app dependency"
}
Require-AsciiMarkers (Join-Path $ReleaseDir "rime.dll") @("lua_translator", "lua_filter", "lua_processor")

function Require-MatchingTree([string]$Source, [string]$Destination) {
    Require-Directory $Source "Missing packaging input: $Source"
    foreach ($file in Get-ChildItem -LiteralPath $Source -File -Recurse) {
        $relative = $file.FullName.Substring($Source.Length + 1)
        $copy = Join-Path $Destination $relative
        Require-File $copy "Missing bundled resource: $copy"
        if ((Get-FileHash -LiteralPath $file.FullName).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) {
            throw "Bundled resource differs from its packaging input: $copy"
        }
    }
}

Require-MatchingTree (Join-Path $repoRoot "resources\addon-schemas") (Join-Path $ReleaseDir "addon-schemas")
Require-MatchingTree (Join-Path $repoRoot "target\keytao-windows-ime-runtime\current\rime-data") (Join-Path $ReleaseDir "rime-data")
foreach ($dll in Get-ChildItem -LiteralPath (Join-Path $repoRoot "target\keytao-windows-app-runtime") -Filter "*.dll" -File) {
    $copy = Join-Path $ReleaseDir $dll.Name
    Require-File $copy "Missing app runtime dependency: $($dll.Name)"
    if ((Get-FileHash -LiteralPath $dll.FullName).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) {
        throw "App runtime dependency differs from the built x64 runtime: $copy"
    }
}
foreach ($runtime in @("current", "x86", "arm64x")) {
    $root = Join-Path $ReleaseDir "keytao-windows-ime-runtime\$runtime"
    Require-MatchingTree (Join-Path $repoRoot "target\keytao-windows-ime-runtime\$runtime") $root
    foreach ($file in @("keytao_windows_ime.dll", "rime.dll", "rime-data\default.yaml", "default-theme.yaml", "msvcp140.dll", "vcruntime140.dll")) {
        Require-File (Join-Path $root $file) "Missing $runtime TSF payload: $file"
    }
    Require-MergedLuaManifest (Join-Path $root "librime-features.txt")
    Require-AsciiMarkers (Join-Path $root "rime.dll") @("lua_translator", "lua_filter", "lua_processor")
    $machine = switch ($runtime) { "current" { 0x8664 }; "x86" { 0x014C }; "arm64x" { 0xAA64 } }
    Require-PeMachine (Join-Path $root "keytao_windows_ime.dll") ([UInt16]$machine) "$runtime TSF"
    if ($runtime -ne "arm64x") {
        foreach ($dll in Get-ChildItem -LiteralPath $root -Filter "*.dll" -File) {
            Require-PeMachine $dll.FullName ([UInt16]$machine) "$runtime dependency"
        }
    } else {
        foreach ($file in @("keytao_windows_ime_x64.dll", "keytao_windows_ime_arm64.dll", "rime-arm64.dll", "librime-arm64-features.txt")) {
            Require-File (Join-Path $root $file) "Missing ARM64X payload: $file"
        }
        Require-PeMachine (Join-Path $root "keytao_windows_ime_x64.dll") ([UInt16]0x8664) "x64 TSF target"
        Require-PeMachine (Join-Path $root "keytao_windows_ime_arm64.dll") ([UInt16]0xAA64) "ARM64 TSF target"
        Require-PeMachine (Join-Path $root "rime.dll") ([UInt16]0x8664) "x64 librime"
        Require-PeMachine (Join-Path $root "rime-arm64.dll") ([UInt16]0xAA64) "ARM64 librime"
        Require-MergedLuaManifest (Join-Path $root "librime-arm64-features.txt")
        Require-AsciiMarkers (Join-Path $root "rime-arm64.dll") @("lua_translator", "lua_filter", "lua_processor")
    }
}

if (-not $LayoutOnly) {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        throw "Native bundle verification requires Windows; -LayoutOnly checks files and PE headers only"
    }
    if (-not (Find-OnPath "dumpbin.exe")) { throw "dumpbin.exe is required; run from a Visual Studio developer shell" }
    # Resolve all app DLL imports against the payload or the operating system.
    foreach ($binary in @($appExe) + @(Get-ChildItem -LiteralPath $ReleaseDir -Filter "*.dll" -File | ForEach-Object FullName)) {
        $imports = & dumpbin.exe /dependents $binary | Out-String
        if ($LASTEXITCODE -ne 0) { throw "dumpbin /dependents failed: $binary" }
        foreach ($match in [regex]::Matches($imports, '(?im)^\s*([a-z0-9_.-]+\.dll)\s*$')) {
            $name = $match.Groups[1].Value
            if ($name -match '^(api-ms-|ext-ms-)') { continue }
            if (-not (Test-Path -LiteralPath (Join-Path $ReleaseDir $name)) -and
                -not (Test-Path -LiteralPath (Join-Path $env:WINDIR "System32\$name"))) {
                throw "Unresolved app dependency $name imported by $binary"
            }
        }
    }
    $tsfRoot = Join-Path $ReleaseDir "keytao-windows-ime-runtime"
    foreach ($relative in @("current\keytao_windows_ime.dll", "x86\keytao_windows_ime.dll",
            "arm64x\keytao_windows_ime_x64.dll", "arm64x\keytao_windows_ime_arm64.dll")) {
        $dll = Join-Path $tsfRoot $relative
        $dependency = if ($relative -like "*_arm64.dll") { "rime-arm64.dll" } else { "rime.dll" }
        Require-DelayLoadedDependency $dll $dependency
        Require-NoCrtDependency $dll
        Require-Exports $dll @("DllCanUnloadNow", "DllGetClassObject", "DllRegisterServer", "DllUnregisterServer")
        foreach ($id in @(1, 2, 3)) { Require-EmbeddedIcon $dll $id }
        Verify-AuthenticodeSignature $dll
    }
    $forwarder = Join-Path $tsfRoot "arm64x\keytao_windows_ime.dll"
    Require-Arm64X $forwarder
    Require-Exports $forwarder @("DllCanUnloadNow", "DllGetClassObject", "DllRegisterServer", "DllUnregisterServer")
    Verify-AuthenticodeSignature $forwarder
    foreach ($runtime in @("current", "x86")) {
        & (Join-Path $PSScriptRoot "test-windows-ime-load.ps1") -DllPath (Join-Path $tsfRoot "$runtime\keytao_windows_ime.dll")
    }
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($appExe)
    if ($info.ProductName -ne "KeyTao" -or $info.CompanyName -ne "rea" -or $info.ProductVersion -ne $version) {
        throw "Executable product/company/version resources do not match the workspace"
    }
    Verify-AuthenticodeSignature $appExe
}
if (-not $SkipInstaller) {
    Require-File $InstallerPath "Missing Windows NSIS installer: $InstallerPath"
    if ((Split-Path -Leaf $InstallerPath) -ne "keytao-app-$version-windows-$Arch-setup.exe") {
        throw "Installer name must match the release.yml Windows artifact name"
    }
    Require-PeMachine $InstallerPath ([UInt16]0x014C) "NSIS installer"
    if (-not $LayoutOnly) { Verify-AuthenticodeSignature $InstallerPath }
}
Write-Host "Windows Flutter bundle verification passed (LayoutOnly=$LayoutOnly, SkipInstaller=$SkipInstaller)"
Write-Host "  App: $appExe"
if (-not $SkipInstaller) { Write-Host "  Installer: $InstallerPath" }
