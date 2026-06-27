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
$appRimeDll = Join-Path $ReleaseDir "rime.dll"
$imeLuaPlugin = Find-LuaPlugin @($imeRuntimeDir, (Join-Path $imeRuntimeDir "rime-plugins"))
$appLuaPlugin = Find-LuaPlugin @($ReleaseDir, (Join-Path $ReleaseDir "rime-plugins"))
$hookFile = Join-Path $repoRoot "src-tauri\windows\nsis-hooks.nsh"

Require-File $appExe "Windows release payload is missing keytao-app.exe"
Require-File $imeDll "Windows release payload is missing keytao_windows_ime.dll"
Require-File $imeRimeDll "Windows IME runtime is missing rime.dll"
Require-File $imeRimeData "Windows IME runtime is missing rime-data\default.yaml"
Require-File $imeDefaultTheme "Windows IME runtime is missing default-theme.yaml"
Require-File $imeVcRuntime "Windows IME runtime is missing vcruntime140.dll"
Require-File $appRimeDll "Windows app payload is missing rime.dll next to keytao-app.exe"
if (-not $imeLuaPlugin) {
    throw "Windows IME runtime is missing the librime-lua plugin DLL"
}
if (-not $appLuaPlugin) {
    throw "Windows app payload is missing the librime-lua plugin DLL next to keytao-app.exe"
}
Require-File $hookFile "Missing NSIS installer hook file: $hookFile"

Require-Pattern $hookFile 'NSIS_HOOK_POSTINSTALL' "NSIS hook file does not define NSIS_HOOK_POSTINSTALL"
Require-Pattern $hookFile 'NSIS_HOOK_PREUNINSTALL' "NSIS hook file does not define NSIS_HOOK_PREUNINSTALL"
Require-Pattern $hookFile 'regsvr32\.exe' "NSIS hook file does not invoke regsvr32.exe"

$config = Get-Content (Join-Path $repoRoot "src-tauri\tauri.windows.conf.json") -Raw | ConvertFrom-Json
$resourceKeys = @($config.bundle.resources.PSObject.Properties.Name)
if ($resourceKeys -notcontains "../target/keytao-windows-ime-runtime/current") {
    throw "Windows resources must include the IME runtime directory"
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
Require-Pattern $installerScript.FullName 'default-theme\.yaml' "Generated Windows installer script does not install the shared default theme"
Require-Pattern $installerScript.FullName '/oname=.*rime\.dll' "Generated Windows installer script does not install rime.dll next to keytao-app.exe"
Require-Pattern $installerScript.FullName 'rime.*lua.*\.dll' "Generated Windows installer script does not install the librime-lua plugin DLL"

Write-Host "Windows bundle verification passed"
Write-Host "  Installer: $($installer.FullName)"
Write-Host "  App: $appExe"
Write-Host "  IME runtime: $imeRuntimeDir ($Arch)"
