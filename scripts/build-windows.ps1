param(
    [ValidateSet("native", "x64", "arm64", "x86")]
    [string]$Arch = "native"
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSCommandPath) ".."))

function Resolve-NativeWindowsArch {
    switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
        "X64" { return "x64" }
        "Arm64" { return "arm64" }
        "X86" { return "x86" }
        default { throw "Unsupported Windows OS architecture: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
    }
}

if ($Arch -eq "native") {
    $Arch = Resolve-NativeWindowsArch
}

function Assert-SupportedWindowsPackageArch([string]$WindowsArch) {
    if ($WindowsArch -eq "arm64") {
        throw "Build the universal Windows installer with -Arch x64. It includes x64, x86, native ARM64, and ARM64X text-service runtimes while keeping the desktop app x64-compatible."
    }
    if ($WindowsArch -notin @("x64", "x86")) {
        throw "Unsupported Windows package arch: $WindowsArch. Supported values: x64, x86."
    }
}

Assert-SupportedWindowsPackageArch $Arch

function Get-WindowsRustTarget([string]$WindowsArch) {
    switch ($WindowsArch) {
        "x64" { return "x86_64-pc-windows-msvc" }
        "x86" { return "i686-pc-windows-msvc" }
        default { throw "Unsupported Windows arch: $WindowsArch" }
    }
}

$target = Get-WindowsRustTarget $Arch

function Add-PathIfExists([string]$Path) {
    if ($Path -and (Test-Path -LiteralPath $Path)) {
        $parts = $env:Path -split ";"
        if ($parts -notcontains $Path) {
            $env:Path = "$Path;$env:Path"
        }
    }
}

if (-not $env:PNPM_HOME) {
    $scoopPnpm = Join-Path $env:USERPROFILE "scoop\apps\pnpm\current"
    if (Test-Path -LiteralPath $scoopPnpm) {
        $env:PNPM_HOME = $scoopPnpm
    }
}
if ($env:PNPM_HOME) {
    Add-PathIfExists $env:PNPM_HOME
    Add-PathIfExists (Join-Path $env:PNPM_HOME "bin")
}
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\nodejs-lts\current")
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\nodejs-lts\current\bin")
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\nodejs\current")
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\nodejs\current\bin")
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\llvm\current\bin")
Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\shims")

$pnpmCommand = Get-Command pnpm -ErrorAction SilentlyContinue
$pnpmPath = if ($pnpmCommand) { $pnpmCommand.Source } else { $null }
if (-not $pnpmCommand -and $env:PNPM_HOME) {
    $pnpmExe = Join-Path $env:PNPM_HOME "pnpm.exe"
    if (Test-Path -LiteralPath $pnpmExe) {
        $pnpmPath = (Resolve-Path -LiteralPath $pnpmExe).Path
    }
}
if (-not $pnpmPath) {
    throw "pnpm was not found. Install pnpm or set PNPM_HOME before running this script."
}

function Get-RustHostTriple([string]$Command, [string[]]$Arguments) {
    $allArgs = @($Arguments) + @("-vV")
    $output = & $Command @allArgs
    if ($LASTEXITCODE -ne 0) {
        return $null
    }
    foreach ($line in $output) {
        if ($line -match "^host:\s*(.+)$") {
            return $Matches[1].Trim()
        }
    }
    return $null
}

function Find-OnPath([string]$Name) {
    $result = & cmd.exe /d /c "where `"$Name`" 2>nul"
    if ($LASTEXITCODE -eq 0 -and $result) {
        return $result | Select-Object -First 1
    }
    return $null
}

function Import-CmdEnvironment([string]$BatchFile, [string[]]$Arguments) {
    $argumentLine = $Arguments -join " "
    $command = "call `"$BatchFile`" $argumentLine >nul && set"
    $output = & cmd.exe /d /s /c $command
    if ($LASTEXITCODE -ne 0) {
        return $false
    }

    foreach ($line in $output) {
        $index = $line.IndexOf("=")
        if ($index -le 0) {
            continue
        }
        $name = $line.Substring(0, $index)
        $value = $line.Substring($index + 1)
        [Environment]::SetEnvironmentVariable($name, $value, "Process")
    }
    return $true
}

function Get-MsvcArchForRustHost([string]$HostTriple) {
    if ($HostTriple -like "x86_64-*") {
        return "x64"
    }
    if ($HostTriple -like "i686-*") {
        return "x86"
    }
    throw "Unsupported Rust host triple for MSVC environment: $HostTriple"
}

function Find-VcVarsAll {
    $vswhereCandidates = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
        "${env:ProgramFiles}\Microsoft Visual Studio\Installer\vswhere.exe"
    )
    $vswhere = $vswhereCandidates | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
    if (-not $vswhere) {
        return $null
    }
    $vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vsInstall) {
        return $null
    }
    $vcvarsall = Join-Path ($vsInstall | Select-Object -First 1) "VC\Auxiliary\Build\vcvarsall.bat"
    if (Test-Path -LiteralPath $vcvarsall) {
        return $vcvarsall
    }
    return $null
}

function Initialize-MsvcEnvironment([string]$MsvcArch) {
    if ((Find-OnPath "link.exe") -and ($env:VSCMD_ARG_TGT_ARCH -eq $MsvcArch)) {
        return
    }

    $vcvarsall = Find-VcVarsAll
    if ($vcvarsall) {
        if (Import-CmdEnvironment $vcvarsall @($MsvcArch)) {
            if (Find-OnPath "link.exe") {
                return
            }
        }
    }

    if (Find-OnPath "link.exe") {
        return
    }

    $componentHint = "Install Visual Studio Build Tools with the Desktop development with C++ workload and Windows SDK."
    throw "MSVC linker link.exe was not found for $MsvcArch. $componentHint"
}

$rustHostTriple = Get-RustHostTriple "rustc" @()
if (-not $rustHostTriple) {
    throw "Unable to determine the active Rust host triple."
}

# The universal package contains an x64 desktop app and builds x86/ARM64 TIP
# payloads through the x64-hosted MSVC cross tools. Native ARM64 Windows can
# run this toolchain under emulation, which also matches the release runners.
if ($Arch -eq "x64" -and $rustHostTriple -like "aarch64-*") {
    $x64Toolchain = "stable-x86_64-pc-windows-msvc"
    $installedToolchains = & rustup toolchain list
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to list installed Rust toolchains."
    }
    $toolchainPattern = "^$([regex]::Escape($x64Toolchain))(\s|$)"
    if (-not ($installedToolchains | Where-Object { $_ -match $toolchainPattern })) {
        & rustup toolchain install $x64Toolchain --profile minimal
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to install the $x64Toolchain toolchain required by the universal Windows package."
        }
    }
    $env:RUSTUP_TOOLCHAIN = $x64Toolchain
    $rustHostTriple = Get-RustHostTriple "rustc" @()
    if ($rustHostTriple -notlike "x86_64-*") {
        throw "Unable to activate the $x64Toolchain toolchain on Windows ARM64."
    }
}

$crossCompiling = $rustHostTriple -ne $target
$nativeWindowsArch = Resolve-NativeWindowsArch
$explicitTauriTarget = $crossCompiling -or $nativeWindowsArch -ne $Arch
if ($crossCompiling) {
    & rustup target add $target
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to install Rust target $target."
    }
}
$msvcArch = Get-MsvcArchForRustHost $rustHostTriple

$preferredLibclang = Join-Path $env:USERPROFILE "scoop\apps\llvm\current\bin"
if (Test-Path (Join-Path $preferredLibclang "libclang.dll")) {
    $env:LIBCLANG_PATH = $preferredLibclang
}

Initialize-MsvcEnvironment $msvcArch

if ($Arch -eq "x86" -and $rustHostTriple -like "x86_64-*") {
    $vcvarsall = Find-VcVarsAll
    if (-not $vcvarsall) {
        throw "vcvarsall.bat is required to configure the x64-to-x86 MSVC linker."
    }
    $toolDir = Join-Path $repoRoot "target\keytao-windows-tools"
    New-Item -ItemType Directory -Force -Path $toolDir | Out-Null
    $linkerWrapper = Join-Path $toolDir "link-i686-pc-windows-msvc.cmd"
    $compilerWrapper = Join-Path $toolDir "cl-i686-pc-windows-msvc.cmd"
    $archiverWrapper = Join-Path $toolDir "lib-i686-pc-windows-msvc.cmd"
    @"
@echo off
call "$vcvarsall" x64_x86 >nul
link.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $linkerWrapper
    @"
@echo off
call "$vcvarsall" x64_x86 >nul
cl.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $compilerWrapper
    @"
@echo off
call "$vcvarsall" x64_x86 >nul
lib.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $archiverWrapper
    $env:CARGO_TARGET_I686_PC_WINDOWS_MSVC_LINKER = $linkerWrapper
    [Environment]::SetEnvironmentVariable("CC_i686_pc_windows_msvc", $compilerWrapper, "Process")
    [Environment]::SetEnvironmentVariable("CXX_i686_pc_windows_msvc", $compilerWrapper, "Process")
    [Environment]::SetEnvironmentVariable("AR_i686_pc_windows_msvc", $archiverWrapper, "Process")
}

Push-Location $repoRoot
try {
    if ($Arch -eq "x64") {
        & powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch x86 -SkipAppRuntime
        if ($LASTEXITCODE -ne 0) {
            throw "build-windows-ime.ps1 x86 failed with exit code $LASTEXITCODE"
        }
    }

    & powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch $Arch
    if ($LASTEXITCODE -ne 0) {
        throw "build-windows-ime.ps1 $Arch failed with exit code $LASTEXITCODE"
    }

    if ($Arch -eq "x64") {
        & powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch arm64 -SkipAppRuntime
        if ($LASTEXITCODE -ne 0) {
            throw "build-windows-ime.ps1 arm64 failed with exit code $LASTEXITCODE"
        }
        & powershell -ExecutionPolicy Bypass -File scripts\build-windows-arm64x.ps1
        if ($LASTEXITCODE -ne 0) {
            throw "build-windows-arm64x.ps1 failed with exit code $LASTEXITCODE"
        }
    }

    $tauriVendorDir = Join-Path $repoRoot "vendor\librime\windows-$Arch"
    . (Join-Path $tauriVendorDir "env.ps1")
    # env.ps1 can come from a cached SDK created in another checkout. Resolve
    # every build path from this repository after importing optional settings.
    $env:RIME_INCLUDE_DIR = Join-Path $tauriVendorDir "include"
    $env:RIME_LIB_DIR = Join-Path $tauriVendorDir "lib"
    $env:KEYTAO_RIME_LIB_NAME = "rime"
    $env:KEYTAO_RIME_DLL_NAME = "rime.dll"
    Add-PathIfExists (Join-Path $tauriVendorDir "bin")

    $cargoTargetDir = if ($env:CARGO_TARGET_DIR) {
        if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
            [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
        } else {
            [System.IO.Path]::GetFullPath((Join-Path $repoRoot $env:CARGO_TARGET_DIR))
        }
    } else {
        Join-Path $repoRoot "target"
    }

    $tauriArgs = @("tauri", "build", "--bundles", "nsis", "--config", "src-tauri/tauri.windows.conf.json")
    $releaseDir = Join-Path $cargoTargetDir "release"
    if ($explicitTauriTarget) {
        $tauriArgs = @("tauri", "build", "--target", $target, "--bundles", "nsis", "--config", "src-tauri/tauri.windows.conf.json")
        $releaseDir = Join-Path $cargoTargetDir "$target\release"
    }

    $actualNsisDir = Join-Path $releaseDir "bundle\nsis"
    if (Test-Path -LiteralPath $actualNsisDir) {
        Get-ChildItem -Path $actualNsisDir -Filter "*.exe" -File | Remove-Item -Force
    }

    $previousCi = $env:CI
    try {
        # pnpm may need to replace node_modules when the repository is shared
        # between macOS and Windows. Force its documented non-interactive path
        # so packaging cannot stall or abort on a hidden confirmation prompt.
        $env:CI = "true"
        & $pnpmPath @tauriArgs
        if ($LASTEXITCODE -ne 0) {
            throw "tauri build failed with exit code $LASTEXITCODE"
        }
    } finally {
        $env:CI = $previousCi
    }

    & powershell -ExecutionPolicy Bypass -File scripts\verify-windows-bundle.ps1 -Arch $Arch -ReleaseDir $releaseDir
    if ($LASTEXITCODE -ne 0) {
        throw "verify-windows-bundle.ps1 failed with exit code $LASTEXITCODE"
    }

    $canonicalNsisDir = Join-Path $repoRoot "target\release\bundle\nsis"
    if ((Resolve-Path -LiteralPath $actualNsisDir).Path -ne (Resolve-Path -LiteralPath $canonicalNsisDir -ErrorAction SilentlyContinue).Path) {
        New-Item -ItemType Directory -Force -Path $canonicalNsisDir | Out-Null
        Get-ChildItem -Path $canonicalNsisDir -Filter "*.exe" -File | Remove-Item -Force
        Copy-Item -Force -Path (Join-Path $actualNsisDir "*.exe") -Destination $canonicalNsisDir
    }
} finally {
    Pop-Location
}
