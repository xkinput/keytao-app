param(
    [ValidateSet("x64")]
    [string]$Arch = "x64",
    [string]$MakeNsis = "makensis.exe",
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$manifest = Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*"(?<version>[^"\r\n]+)"') {
    throw "Missing workspace.package.version"
}
$version = $Matches.version
if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
    throw "Invalid workspace version: $version"
}
$numericVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"
$config = Get-Content -LiteralPath (Join-Path $repoRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
if ($config.version -ne $version -or $config.productName -ne "KeyTao" -or $config.identifier -ne "ink.rea.keytao-app") {
    throw "Workspace version or legacy Windows identity is out of sync"
}
$releaseDir = Join-Path $repoRoot "flutter_app\build\windows\x64\runner\Release"
$workDir = Join-Path $repoRoot "target\keytao-windows-flutter"
$bundleDir = Join-Path $workDir "bundle"
$uninstallFiles = Join-Path $workDir "uninstall-files.nsh"
$installer = Join-Path $repoRoot "target\release\bundle\nsis\keytao-app-$version-windows-$Arch-setup.exe"

if ($DryRun) {
    Write-Host "Version: $version ($numericVersion); perMachine; HKLM uninstall key: KeyTao"
    Write-Host "IME: x86 -> x64 -> arm64 -> arm64x (scripts/build-windows-ime.ps1 + build-windows-arm64x.ps1)"
    Write-Host "flutter build windows --release --target-platform windows-x64 --build-name $version --build-number 0"
    Write-Host "Bundle: $bundleDir"
    Write-Host "makensis: packaging/windows/keytao.nsi -> $installer"
    return
}
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "Building the Windows bundle requires Windows; use -DryRun elsewhere"
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}

$flutter = (Get-Command flutter -ErrorAction Stop).Source
$powershell = (Get-Command powershell.exe -ErrorAction Stop).Source
# Keep dumpbin and the matching x64 CRT tools available to final verification,
# even when invoked from a plain CI PowerShell session.
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
if ($LASTEXITCODE -ne 0 -or -not $vsInstall) { throw "Visual Studio C++ tools were not found" }
$vcvars = Join-Path $vsInstall "VC\Auxiliary\Build\vcvarsall.bat"
$buildEnvironment = & cmd.exe /d /s /c "call `"$vcvars`" x64 >nul && set"
if ($LASTEXITCODE -ne 0) { throw "Unable to initialize the x64 MSVC environment" }
foreach ($line in $buildEnvironment) {
    $separator = $line.IndexOf("=")
    if ($separator -gt 0) {
        [Environment]::SetEnvironmentVariable($line.Substring(0, $separator), $line.Substring($separator + 1), "Process")
    }
}
if (-not (Get-Command $MakeNsis -ErrorAction SilentlyContinue)) {
    $MakeNsis = Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"
}
if (-not (Test-Path -LiteralPath $MakeNsis) -and -not (Get-Command $MakeNsis -ErrorAction SilentlyContinue)) {
    throw "Install NSIS 3 (including its standard plugins), or pass -MakeNsis"
}

Push-Location $repoRoot
try {
    # Separate processes prevent cross-architecture MSVC/RIME environment leaks.
    foreach ($imeArch in @("x86", "x64", "arm64")) {
        $imeArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts\build-windows-ime.ps1", "-Arch", $imeArch)
        if ($imeArch -ne "x64") { $imeArgs += "-SkipAppRuntime" }
        Invoke-Checked $powershell $imeArgs
    }
    Invoke-Checked $powershell @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts\build-windows-arm64x.ps1")

    # Remove only this build's replaceable output, avoiding stale native assets.
    if (Test-Path -LiteralPath $releaseDir) { Remove-Item -LiteralPath $releaseDir -Recurse -Force }
    Push-Location (Join-Path $repoRoot "flutter_app")
    try {
        Invoke-Checked $flutter @("build", "windows", "--release", "--target-platform", "windows-x64", "--build-name", $version, "--build-number", "0")
    } finally { Pop-Location }

    if (Test-Path -LiteralPath $bundleDir) { Remove-Item -LiteralPath $bundleDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $bundleDir | Out-Null
    Copy-Item -Path (Join-Path $releaseDir "*") -Destination $bundleDir -Recurse -Force

    # Generate exact owned paths for safe uninstall, including future Flutter
    # native assets. Reject NSIS metacharacters instead of interpolating them.
    $lines = @()
    foreach ($file in Get-ChildItem -LiteralPath $bundleDir -Recurse -File | Sort-Object FullName) {
        $relative = $file.FullName.Substring($bundleDir.Length + 1)
        if ($relative -match '[\r\n$"`]') { throw "Unsafe NSIS payload filename: $relative" }
        $lines += 'Delete "$INSTDIR\' + $relative + '"'
    }
    foreach ($directory in Get-ChildItem -LiteralPath $bundleDir -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending) {
        $relative = $directory.FullName.Substring($bundleDir.Length + 1)
        if ($relative -match '[\r\n$"`]') { throw "Unsafe NSIS payload directory: $relative" }
        $lines += 'RMDir "$INSTDIR\' + $relative + '"'
    }
    $lines | Set-Content -LiteralPath $uninstallFiles -Encoding UTF8
    & (Join-Path $PSScriptRoot "verify-windows-bundle.ps1") -ReleaseDir $bundleDir -SkipInstaller -LayoutOnly

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $installer) | Out-Null
    if (Test-Path -LiteralPath $installer) { Remove-Item -LiteralPath $installer -Force }
    Invoke-Checked $MakeNsis @("/V3", "/DVERSION=$version", "/DVERSION_NUMERIC=$numericVersion",
        "/DREPO_ROOT=$repoRoot", "/DBUNDLE_DIR=$bundleDir", "/DUNINSTALL_FILES=$uninstallFiles",
        "/DOUTPUT_FILE=$installer", (Join-Path $repoRoot "packaging\windows\keytao.nsi"))
    & (Join-Path $PSScriptRoot "verify-windows-bundle.ps1") -ReleaseDir $bundleDir -InstallerPath $installer
    Write-Host "Windows Flutter installer ready: $installer"
} finally { Pop-Location }
