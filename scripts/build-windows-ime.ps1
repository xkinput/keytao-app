param(
    [ValidateSet("native", "x64", "arm64", "x86")]
    [string]$Arch = "native",
    [string]$Version = "latest",
    [switch]$DebugBuild,
    [switch]$SkipAppRuntime
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSCommandPath) ".."))

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
    if ($WindowsArch -notin @("x64", "x86", "arm64")) {
        throw "Unsupported Windows IME runtime arch: $WindowsArch. Supported values: x64, x86, arm64."
    }
}

Assert-SupportedWindowsPackageArch $Arch

function Get-WindowsRustTarget([string]$WindowsArch) {
    switch ($WindowsArch) {
        "x64" { return "x86_64-pc-windows-msvc" }
        "arm64" { return "aarch64-pc-windows-msvc" }
        "x86" { return "i686-pc-windows-msvc" }
        default { throw "Unsupported Windows arch: $WindowsArch" }
    }
}

$target = Get-WindowsRustTarget $Arch
$vendorDir = Join-Path $repoRoot "vendor\librime\windows-$Arch"
$envFile = Join-Path $vendorDir "env.ps1"

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

function Get-MsvcArchForRustHost([string]$HostTriple) {
    if ($HostTriple -like "aarch64-*") {
        return "arm64"
    }
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

function Get-WindowsLuaPluginFiles([string]$Root) {
    if (-not $Root) {
        return @()
    }
    $binDir = Join-Path $Root "bin"
    if (-not (Test-Path -LiteralPath $binDir -PathType Container)) {
        return @()
    }
    return @(
        Get-ChildItem -File -Recurse -Path $binDir |
            Where-Object {
                $name = $_.Name.ToLowerInvariant()
                $name.Contains("rime") -and $name.Contains("lua") -and $name.EndsWith(".dll")
            }
    )
}

function Assert-MergedLibrimeLua([string]$Root, [string]$DllName) {
    $featureFile = Join-Path $Root "librime-features.txt"
    if (-not (Test-Path -LiteralPath $featureFile -PathType Leaf)) {
        throw "Missing librime feature manifest: $featureFile"
    }
    $features = Get-Content -LiteralPath $featureFile -Raw
    if ($features -notmatch '(?m)^librime-lua=merged\r?$') {
        throw "Windows librime runtime does not declare merged librime-lua support: $featureFile"
    }

    $rimeDll = Join-Path $Root "bin\$DllName"
    $rimeDllText = [System.Text.Encoding]::ASCII.GetString(
        [System.IO.File]::ReadAllBytes($rimeDll)
    )
    foreach ($marker in @("lua_translator", "lua_filter", "lua_processor")) {
        if (-not $rimeDllText.Contains($marker)) {
            throw "Windows librime runtime $rimeDll is missing merged librime-lua marker '$marker'."
        }
    }
}

$rimeLibName = if ($Arch -eq "arm64") { "rime-arm64.lib" } else { "rime.lib" }
$rimeDllName = if ($Arch -eq "arm64") { "rime-arm64.dll" } else { "rime.dll" }
$needsFetch = -not (Test-Path (Join-Path $vendorDir "include\rime_api.h")) -or
    -not (Test-Path (Join-Path $vendorDir "lib\$rimeLibName")) -or
    -not (Test-Path (Join-Path $vendorDir "bin\$rimeDllName")) -or
    -not (Test-Path (Join-Path $vendorDir "librime-features.txt")) -or
    -not (Test-Path (Join-Path $vendorDir "rime-data\default.yaml")) -or
    -not (Test-Path $envFile)

if ($needsFetch) {
    & (Join-Path $repoRoot "scripts\fetch-librime-windows.ps1") -Arch $Arch -Version $Version -Destination $vendorDir
}

Assert-MergedLibrimeLua $vendorDir $rimeDllName
$luaPlugins = Get-WindowsLuaPluginFiles $vendorDir

$oldErrorActionPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = "Continue"
    . $envFile
} finally {
    $ErrorActionPreference = $oldErrorActionPreference
}
$env:RIME_INCLUDE_DIR = Join-Path $vendorDir "include"
$env:RIME_LIB_DIR = Join-Path $vendorDir "lib"
$env:KEYTAO_RIME_LIB_NAME = if ($Arch -eq "arm64") { "rime-arm64" } else { "rime" }
$env:KEYTAO_RIME_DLL_NAME = if ($Arch -eq "arm64") { "rime-arm64.dll" } else { "rime.dll" }
Add-PathIfExists (Join-Path $vendorDir "bin")

$rustHostTriple = Get-RustHostTriple "rustc" @()
if (-not $rustHostTriple) {
    throw "Unable to determine the active Rust host triple."
}
if ($rustHostTriple -ne $target) {
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

if ($Arch -in @("x86", "arm64") -and $rustHostTriple -like "x86_64-*") {
    $vcvarsall = Find-VcVarsAll
    if (-not $vcvarsall) {
        throw "vcvarsall.bat is required to configure the x64-to-$Arch MSVC linker."
    }
    $crossMsvcArch = if ($Arch -eq "x86") { "x64_x86" } else { "x64_arm64" }
    $targetEnvName = $target.Replace("-", "_")
    $targetEnvNameUpper = $targetEnvName.ToUpperInvariant()
    $toolDir = Join-Path $repoRoot "target\keytao-windows-tools"
    New-Item -ItemType Directory -Force -Path $toolDir | Out-Null
    $linkerWrapper = Join-Path $toolDir "link-$target.cmd"
    $compilerWrapper = Join-Path $toolDir "cl-$target.cmd"
    $archiverWrapper = Join-Path $toolDir "lib-$target.cmd"
    @"
@echo off
call "$vcvarsall" $crossMsvcArch >nul
link.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $linkerWrapper
    @"
@echo off
call "$vcvarsall" $crossMsvcArch >nul
cl.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $compilerWrapper
    @"
@echo off
call "$vcvarsall" $crossMsvcArch >nul
lib.exe %*
exit /b %errorlevel%
"@ | Set-Content -Encoding ASCII -Path $archiverWrapper
    [Environment]::SetEnvironmentVariable("CARGO_TARGET_${targetEnvNameUpper}_LINKER", $linkerWrapper, "Process")
    [Environment]::SetEnvironmentVariable("CC_$targetEnvName", $compilerWrapper, "Process")
    [Environment]::SetEnvironmentVariable("CXX_$targetEnvName", $compilerWrapper, "Process")
    [Environment]::SetEnvironmentVariable("AR_$targetEnvName", $archiverWrapper, "Process")
}

function Find-WindowsRuntimeDll([string]$Name, [string]$WindowsArch) {
    $candidates = @()

    if ($env:VCToolsRedistDir) {
        $redistArchDir = Join-Path $env:VCToolsRedistDir $WindowsArch
        if (Test-Path -LiteralPath $redistArchDir -PathType Container) {
            $redistDll = Get-ChildItem -Path $redistArchDir -Filter $Name -File -Recurse -ErrorAction SilentlyContinue |
                Select-Object -First 1
            if ($redistDll) {
                $candidates += $redistDll.FullName
            }
        }
    }

    if ($env:WINDIR) {
        $nativeArch = Resolve-NativeWindowsArch
        $systemDir = if ($WindowsArch -eq "x86" -and $nativeArch -in @("x64", "arm64")) {
            "SysWOW64"
        } elseif ($WindowsArch -eq $nativeArch) {
            "System32"
        } else {
            $null
        }
        if ($systemDir) {
            $candidates += (Join-Path $env:WINDIR "$systemDir\$Name")
        }
    }

    if ($WindowsArch -eq $msvcArch) {
        $whereResult = Find-OnPath $Name
        if ($whereResult) {
            $candidates += $whereResult
        }
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

$staticCrtFlag = "-C target-feature=+crt-static"
if (-not $env:RUSTFLAGS) {
    $env:RUSTFLAGS = $staticCrtFlag
} elseif ($env:RUSTFLAGS -notlike "*target-feature=+crt-static*") {
    $env:RUSTFLAGS = "$($env:RUSTFLAGS) $staticCrtFlag"
}

Push-Location $repoRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$profile = if ($DebugBuild) { "debug" } else { "release" }
$cargoTargetRoot = if ($env:CARGO_TARGET_DIR) {
    if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $repoRoot $env:CARGO_TARGET_DIR))
    }
} else {
    Join-Path $repoRoot "target"
}
$dll = Join-Path $cargoTargetRoot "$target\$profile\keytao_windows_ime.dll"
$runtimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\$Arch"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $runtimeDir
New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
Copy-Item -Force -LiteralPath $dll -Destination $runtimeDir
Copy-Item -Force -LiteralPath (Join-Path $repoRoot "crates\keytao-theme\default-theme.yaml") -Destination $runtimeDir
Copy-Item -Force -LiteralPath (Join-Path $vendorDir "librime-features.txt") -Destination $runtimeDir
Copy-Item -Force -Path (Join-Path $vendorDir "bin\*.dll") -Destination $runtimeDir
Copy-Item -Recurse -Force -LiteralPath (Join-Path $vendorDir "rime-data") -Destination $runtimeDir
if ($luaPlugins) {
    $runtimePluginDir = Join-Path $runtimeDir "rime-plugins"
    New-Item -ItemType Directory -Force -Path $runtimePluginDir | Out-Null
    foreach ($plugin in $luaPlugins) {
        Copy-Item -Force -LiteralPath $plugin.FullName -Destination $runtimePluginDir
    }
}

$appRuntimeDir = Join-Path $repoRoot "target\keytao-windows-app-runtime"
$populateAppRuntime = -not $SkipAppRuntime
if ($populateAppRuntime) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $appRuntimeDir
    New-Item -ItemType Directory -Force -Path $appRuntimeDir | Out-Null
    Copy-Item -Force -Path (Join-Path $vendorDir "bin\*.dll") -Destination $appRuntimeDir
}

$requiredRuntimeDlls = @("vcruntime140.dll", "msvcp140.dll")
if ($Arch -ne "x86") {
    $requiredRuntimeDlls += "vcruntime140_1.dll"
}

foreach ($runtimeDll in $requiredRuntimeDlls) {
    $source = Find-WindowsRuntimeDll $runtimeDll $Arch
    if ($source) {
        Copy-Item -Force -LiteralPath $source -Destination $runtimeDir
        if ($populateAppRuntime) {
            Copy-Item -Force -LiteralPath $source -Destination $appRuntimeDir
        }
    } elseif (-not (Test-Path -LiteralPath (Join-Path $runtimeDir $runtimeDll) -PathType Leaf)) {
        throw "Unable to locate the $Arch $runtimeDll required by the Windows IME runtime. Install the matching Visual C++ Redistributable components in Visual Studio Build Tools."
    }
}

if ($Arch -ne "arm64") {
    $currentRuntimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\current"
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $currentRuntimeDir
    New-Item -ItemType Directory -Force -Path $currentRuntimeDir | Out-Null
    Copy-Item -Recurse -Force -Path (Join-Path $runtimeDir "*") -Destination $currentRuntimeDir
}

Write-Host ""
Write-Host "Windows IME runtime is ready:"
Write-Host "  $runtimeDir"
Write-Host ""
Write-Host "Register as administrator:"
Write-Host "  regsvr32 `"$runtimeDir\keytao_windows_ime.dll`""
