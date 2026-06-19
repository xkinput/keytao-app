param(
    [ValidateSet("native", "x64", "arm64", "x86")]
    [string]$Arch = "native"
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path (Split-Path -Parent $PSCommandPath) "..")).Path

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
        throw "Windows ARM64 packages are currently unsupported because rime/librime does not publish Windows ARM64 SDK assets. Build the supported x64 package with 'powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Arch x64', or add an experimental source-built librime ARM64 pipeline before enabling this target."
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

function Initialize-MsvcEnvironment([string]$MsvcArch) {
    if ((Find-OnPath "link.exe") -and ($env:VSCMD_ARG_TGT_ARCH -eq $MsvcArch)) {
        return
    }

    $vswhereCandidates = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
        "${env:ProgramFiles}\Microsoft Visual Studio\Installer\vswhere.exe"
    )
    $vswhere = $vswhereCandidates | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
    if ($vswhere) {
        $vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($LASTEXITCODE -eq 0 -and $vsInstall) {
            $vcvarsall = Join-Path ($vsInstall | Select-Object -First 1) "VC\Auxiliary\Build\vcvarsall.bat"
            if (Test-Path -LiteralPath $vcvarsall) {
                if (Import-CmdEnvironment $vcvarsall @($MsvcArch)) {
                    if (Find-OnPath "link.exe") {
                        return
                    }
                }
            }
        }
    }

    if (Find-OnPath "link.exe") {
        return
    }

    $componentHint = "Install Visual Studio Build Tools with the Desktop development with C++ workload and Windows SDK."
    throw "MSVC linker link.exe was not found for $MsvcArch. $componentHint"
}

function Select-RustToolchainForTarget([string]$TargetTriple) {
    $currentHost = Get-RustHostTriple "rustc" @()
    if ($currentHost -eq $TargetTriple) {
        return $null
    }

    if ($TargetTriple -in @("x86_64-pc-windows-msvc", "i686-pc-windows-msvc")) {
        $toolchain = "stable-$TargetTriple"
        $toolchainHost = Get-RustHostTriple "rustup" @("run", $toolchain, "rustc")
        if ($toolchainHost -eq $TargetTriple) {
            return $toolchain
        }

        throw "Building the $TargetTriple Windows package from this host requires the matching Rust host toolchain. Run 'rustup toolchain install stable-$TargetTriple --force-non-host', then retry."
    }

    return $null
}

$selectedRustToolchain = Select-RustToolchainForTarget $target
$previousRustupToolchain = $env:RUSTUP_TOOLCHAIN
if ($selectedRustToolchain) {
    $env:RUSTUP_TOOLCHAIN = $selectedRustToolchain
}

$rustHostTriple = if ($selectedRustToolchain) {
    Get-RustHostTriple "rustup" @("run", $selectedRustToolchain, "rustc")
} else {
    Get-RustHostTriple "rustc" @()
}
$msvcArch = Get-MsvcArchForRustHost $rustHostTriple

$preferredLibclang = Join-Path $env:USERPROFILE "scoop\apps\llvm\current\bin"
if (Test-Path (Join-Path $preferredLibclang "libclang.dll")) {
    $env:LIBCLANG_PATH = $preferredLibclang
}

Initialize-MsvcEnvironment $msvcArch

Push-Location $repoRoot
try {
    & powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch $Arch
    if ($LASTEXITCODE -ne 0) {
        throw "build-windows-ime.ps1 failed with exit code $LASTEXITCODE"
    }

    . (Join-Path $repoRoot "vendor\librime\windows-$Arch\env.ps1")

    $tauriArgs = @("tauri", "build", "--bundles", "nsis", "--config", "src-tauri/tauri.windows.conf.json")
    $releaseDir = Join-Path $repoRoot "target\release"
    if ($selectedRustToolchain) {
        $tauriArgs = @("tauri", "build", "--target", $target, "--bundles", "nsis", "--config", "src-tauri/tauri.windows.conf.json")
        $releaseDir = Join-Path $repoRoot "target\$target\release"
    }

    & $pnpmPath @tauriArgs
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }

    & powershell -ExecutionPolicy Bypass -File scripts\verify-windows-bundle.ps1 -Arch $Arch -ReleaseDir $releaseDir
    if ($LASTEXITCODE -ne 0) {
        throw "verify-windows-bundle.ps1 failed with exit code $LASTEXITCODE"
    }

    $canonicalNsisDir = Join-Path $repoRoot "target\release\bundle\nsis"
    $actualNsisDir = Join-Path $releaseDir "bundle\nsis"
    if ((Resolve-Path -LiteralPath $actualNsisDir).Path -ne (Resolve-Path -LiteralPath $canonicalNsisDir -ErrorAction SilentlyContinue).Path) {
        New-Item -ItemType Directory -Force -Path $canonicalNsisDir | Out-Null
        Copy-Item -Force -Path (Join-Path $actualNsisDir "*.exe") -Destination $canonicalNsisDir
    }
} finally {
    Pop-Location
    if ($null -eq $previousRustupToolchain) {
        Remove-Item Env:\RUSTUP_TOOLCHAIN -ErrorAction SilentlyContinue
    } else {
        $env:RUSTUP_TOOLCHAIN = $previousRustupToolchain
    }
}
