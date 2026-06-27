param(
    [ValidateSet("native", "x64", "arm64", "x86")]
    [string]$Arch = "native",
    [string]$Version = "latest",
    [switch]$DebugBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path (Split-Path -Parent $PSCommandPath) "..")).Path

function Add-PathIfExists([string]$Path) {
    if ($Path -and (Test-Path -LiteralPath $Path)) {
        $parts = $env:Path -split ";"
        if ($parts -notcontains $Path) {
            $env:Path = "$Path;$env:Path"
        }
    }
}

Add-PathIfExists (Join-Path $env:USERPROFILE "scoop\apps\llvm\current\bin")

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
        throw "Windows ARM64 IME runtime is currently unsupported because rime/librime does not publish Windows ARM64 SDK assets. Build the supported x64 runtime with 'powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch x64', or add an experimental source-built librime ARM64 pipeline before enabling this target."
    }
    if ($WindowsArch -notin @("x64", "x86")) {
        throw "Unsupported Windows IME runtime arch: $WindowsArch. Supported values: x64, x86."
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
$vendorDir = Join-Path $repoRoot "vendor\librime\windows-$Arch"
$envFile = Join-Path $vendorDir "env.ps1"
$cargoCommand = "cargo"
$cargoPrefixArgs = @()

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

function Use-RustToolchainForTarget([string]$TargetTriple) {
    $currentHost = Get-RustHostTriple "rustc" @()
    if ($currentHost -eq $TargetTriple) {
        return $currentHost
    }

    if ($TargetTriple -in @("x86_64-pc-windows-msvc", "i686-pc-windows-msvc")) {
        $toolchain = "stable-$TargetTriple"
        $toolchainHost = Get-RustHostTriple "rustup" @("run", $toolchain, "rustc")
        if ($toolchainHost -eq $TargetTriple) {
            $script:cargoCommand = "rustup"
            $script:cargoPrefixArgs = @("run", $toolchain, "cargo")
            return $toolchainHost
        }

        throw "Building the $TargetTriple Windows package from this host requires the matching Rust host toolchain. Run 'rustup toolchain install stable-$TargetTriple --force-non-host', then retry."
    }

    return $currentHost
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

function Get-WindowsLuaPluginFiles([string]$Root) {
    if (-not $Root) {
        return @()
    }
    $binDir = Join-Path $Root "bin"
    if (-not (Test-Path -LiteralPath $binDir -PathType Container)) {
        return @()
    }
    return @(
        Get-ChildItem -File -Path $binDir |
            Where-Object {
                $name = $_.Name.ToLowerInvariant()
                $name.Contains("rime") -and $name.Contains("lua") -and $name.EndsWith(".dll")
            }
    )
}

$needsFetch = -not (Test-Path (Join-Path $vendorDir "include\rime_api.h")) -or
    -not (Test-Path (Join-Path $vendorDir "lib\rime.lib")) -or
    -not (Test-Path (Join-Path $vendorDir "bin\rime.dll")) -or
    -not (Test-Path (Join-Path $vendorDir "rime-data\default.yaml")) -or
    -not (Test-Path $envFile)

if ($needsFetch) {
    & (Join-Path $repoRoot "scripts\fetch-librime-windows.ps1") -Arch $Arch -Version $Version -Destination $vendorDir
}

$luaPlugins = Get-WindowsLuaPluginFiles $vendorDir
if (-not $luaPlugins) {
    Write-Warning "Windows librime runtime does not include librime-lua plugin DLL in $vendorDir\bin. Official rime/librime Windows SDK assets currently do not ship it, so Windows Lua extensions will be unavailable in this build."
}

$oldErrorActionPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = "Continue"
    . $envFile
} finally {
    $ErrorActionPreference = $oldErrorActionPreference
}

$rustHostTriple = Use-RustToolchainForTarget $target
$msvcArch = Get-MsvcArchForRustHost $rustHostTriple

$preferredLibclang = Join-Path $env:USERPROFILE "scoop\apps\llvm\current\bin"
if (Test-Path (Join-Path $preferredLibclang "libclang.dll")) {
    $env:LIBCLANG_PATH = $preferredLibclang
}

if (-not $env:LIBCLANG_PATH) {
    $libclang = Find-OnPath "libclang.dll"
    if ($libclang) {
        $env:LIBCLANG_PATH = Split-Path -Parent $libclang
    }
}

if (-not $env:LIBCLANG_PATH -or -not (Test-Path (Join-Path $env:LIBCLANG_PATH "libclang.dll"))) {
    throw "LIBCLANG_PATH does not point to libclang.dll. Install LLVM, e.g. 'scoop install llvm', then retry."
}

Initialize-MsvcEnvironment $msvcArch

function Find-WindowsRuntimeDll($Name) {
    $candidates = @()
    $whereResult = Find-OnPath $Name
    if ($whereResult) {
        $candidates += $whereResult
    }
    if ($env:WINDIR) {
        $candidates += (Join-Path $env:WINDIR "System32\$Name")
    }
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return (Resolve-Path $candidate).Path
        }
    }
    return $null
}

$cargoArgs = @("build", "-p", "keytao-windows-ime", "--target", $target)
if (-not $DebugBuild) {
    $cargoArgs += "--release"
}

Push-Location $repoRoot
try {
    $fullCargoArgs = @($cargoPrefixArgs) + @($cargoArgs)
    & $cargoCommand @fullCargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$profile = if ($DebugBuild) { "debug" } else { "release" }
$dll = Join-Path $repoRoot "target\$target\$profile\keytao_windows_ime.dll"
$runtimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\$Arch"
New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
Copy-Item -Force -LiteralPath $dll -Destination $runtimeDir
Copy-Item -Force -LiteralPath (Join-Path $repoRoot "crates\keytao-theme\default-theme.yaml") -Destination $runtimeDir
Copy-Item -Force -Path (Join-Path $vendorDir "bin\*.dll") -Destination $runtimeDir
Copy-Item -Recurse -Force -LiteralPath (Join-Path $vendorDir "rime-data") -Destination $runtimeDir
$runtimePluginDir = Join-Path $runtimeDir "rime-plugins"
New-Item -ItemType Directory -Force -Path $runtimePluginDir | Out-Null
foreach ($plugin in $luaPlugins) {
    Copy-Item -Force -LiteralPath $plugin.FullName -Destination $runtimePluginDir
}

$appRuntimeDir = Join-Path $repoRoot "target\keytao-windows-app-runtime"
New-Item -ItemType Directory -Force -Path $appRuntimeDir | Out-Null
Copy-Item -Force -Path (Join-Path $vendorDir "bin\*.dll") -Destination $appRuntimeDir

foreach ($runtimeDll in @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll")) {
    $source = Find-WindowsRuntimeDll $runtimeDll
    if ($source) {
        Copy-Item -Force -LiteralPath $source -Destination $runtimeDir
        Copy-Item -Force -LiteralPath $source -Destination $appRuntimeDir
    }
}

$currentRuntimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\current"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $currentRuntimeDir
New-Item -ItemType Directory -Force -Path $currentRuntimeDir | Out-Null
Copy-Item -Recurse -Force -Path (Join-Path $runtimeDir "*") -Destination $currentRuntimeDir

Write-Host ""
Write-Host "Windows IME runtime is ready:"
Write-Host "  $runtimeDir"
Write-Host ""
Write-Host "Register as administrator:"
Write-Host "  regsvr32 `"$runtimeDir\keytao_windows_ime.dll`""
