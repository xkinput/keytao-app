param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [string]$Version = "latest",
    [switch]$DebugBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path (Split-Path -Parent $PSCommandPath) "..")).Path
$vendorDir = Join-Path $repoRoot "vendor\librime\windows-$Arch"
$envFile = Join-Path $vendorDir "env.ps1"

$needsFetch = -not (Test-Path (Join-Path $vendorDir "include\rime_api.h")) -or
    -not (Test-Path (Join-Path $vendorDir "lib\rime.lib")) -or
    -not (Test-Path (Join-Path $vendorDir "bin\rime.dll")) -or
    -not (Test-Path (Join-Path $vendorDir "rime-data\default.yaml")) -or
    -not (Test-Path $envFile)

if ($needsFetch) {
    & (Join-Path $repoRoot "scripts\fetch-librime-windows.ps1") -Arch $Arch -Version $Version -Destination $vendorDir
}

. $envFile

if (-not $env:LIBCLANG_PATH) {
    $libclang = & where.exe libclang.dll 2>$null | Select-Object -First 1
    if ($libclang) {
        $env:LIBCLANG_PATH = Split-Path -Parent $libclang
    }
}

if (-not $env:LIBCLANG_PATH -or -not (Test-Path (Join-Path $env:LIBCLANG_PATH "libclang.dll"))) {
    throw "LIBCLANG_PATH does not point to libclang.dll. Install LLVM, e.g. 'scoop install llvm', then retry."
}

function Find-WindowsRuntimeDll($Name) {
    $candidates = @()
    $whereResult = & where.exe $Name 2>$null
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

$target = if ($Arch -eq "x64") { "x86_64-pc-windows-msvc" } else { "i686-pc-windows-msvc" }
$cargoArgs = @("build", "-p", "keytao-windows-ime", "--target", $target)
if (-not $DebugBuild) {
    $cargoArgs += "--release"
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
$dll = Join-Path $repoRoot "target\$target\$profile\keytao_windows_ime.dll"
$runtimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\$Arch"
New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
Copy-Item -Force -LiteralPath $dll -Destination $runtimeDir
Copy-Item -Force -Path (Join-Path $vendorDir "bin\*.dll") -Destination $runtimeDir
Copy-Item -Recurse -Force -LiteralPath (Join-Path $vendorDir "rime-data") -Destination $runtimeDir

$appRuntimeDir = Join-Path $repoRoot "target\keytao-windows-app-runtime"
New-Item -ItemType Directory -Force -Path $appRuntimeDir | Out-Null
Copy-Item -Force -LiteralPath (Join-Path $vendorDir "bin\rime.dll") -Destination $appRuntimeDir

foreach ($runtimeDll in @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll")) {
    $source = Find-WindowsRuntimeDll $runtimeDll
    if ($source) {
        Copy-Item -Force -LiteralPath $source -Destination $runtimeDir
        Copy-Item -Force -LiteralPath $source -Destination $appRuntimeDir
    }
}

Write-Host ""
Write-Host "Windows IME runtime is ready:"
Write-Host "  $runtimeDir"
Write-Host ""
Write-Host "Register as administrator:"
Write-Host "  regsvr32 `"$runtimeDir\keytao_windows_ime.dll`""
