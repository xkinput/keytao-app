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
$imeProfileIcon = Join-Path $imeRuntimeDir "keytao.ico"
$imeVcRuntime = Join-Path $imeRuntimeDir "vcruntime140.dll"
$appRimeDll = Join-Path $ReleaseDir "rime.dll"
$imeLuaPlugin = Find-LuaPlugin @($imeRuntimeDir, (Join-Path $imeRuntimeDir "rime-plugins"))
$appLuaPlugin = Find-LuaPlugin @($ReleaseDir, (Join-Path $ReleaseDir "rime-plugins"))
$hookFile = Join-Path $repoRoot "src-tauri\windows\nsis-hooks.nsh"
$registrationSource = Join-Path $repoRoot "crates\keytao-windows-ime\src\registration.rs"

Require-File $appExe "Windows release payload is missing keytao-app.exe"
Require-File $imeDll "Windows release payload is missing keytao_windows_ime.dll"
Require-File $imeRimeDll "Windows IME runtime is missing rime.dll"
Require-File $imeRimeData "Windows IME runtime is missing rime-data\default.yaml"
Require-File $imeDefaultTheme "Windows IME runtime is missing default-theme.yaml"
Require-File $imeProfileIcon "Windows IME runtime is missing keytao.ico for TSF profile registration"
Require-File $imeVcRuntime "Windows IME runtime is missing vcruntime140.dll"
Require-File $appRimeDll "Windows app payload is missing rime.dll next to keytao-app.exe"
Require-DelayLoadedDependency $imeDll "rime.dll"
if (-not $imeLuaPlugin) {
    Write-Warning "Windows IME runtime does not include the librime-lua plugin DLL; Lua extensions will be unavailable in this Windows package."
}
if (-not $appLuaPlugin) {
    Write-Warning "Windows app payload does not include the librime-lua plugin DLL next to keytao-app.exe."
}
Require-File $hookFile "Missing NSIS installer hook file: $hookFile"
Require-File $registrationSource "Missing Windows TSF registration source: $registrationSource"

Require-Pattern $hookFile 'NSIS_HOOK_POSTINSTALL' "NSIS hook file does not define NSIS_HOOK_POSTINSTALL"
Require-Pattern $hookFile 'NSIS_HOOK_PREUNINSTALL' "NSIS hook file does not define NSIS_HOOK_PREUNINSTALL"
Require-Pattern $hookFile 'regsvr32\.exe' "NSIS hook file does not invoke regsvr32.exe"
Require-Pattern $hookFile 'ExecWait.*regsvr32\.exe' "NSIS hook must wait for regsvr32.exe so TSF registration is complete before install finishes"
Require-Pattern $registrationSource 'InstallLayoutOrTip' "Windows TSF registration must call InstallLayoutOrTip so the profile is added to the current user's input methods"
Require-Pattern $registrationSource 'keytao\.ico' "Windows TSF registration must register an ICO file for the input switcher icon"

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
Require-Pattern $installerScript.FullName 'keytao\.ico' "Generated Windows installer script does not install keytao.ico for TSF profile registration"
Require-Pattern $installerScript.FullName '/oname=.*rime\.dll' "Generated Windows installer script does not install rime.dll next to keytao-app.exe"
if ($imeLuaPlugin -or $appLuaPlugin) {
    Require-Pattern $installerScript.FullName 'rime.*lua.*\.dll' "Generated Windows installer script does not install the librime-lua plugin DLL"
}

Write-Host "Windows bundle verification passed"
Write-Host "  Installer: $($installer.FullName)"
Write-Host "  App: $appExe"
Write-Host "  IME runtime: $imeRuntimeDir ($Arch)"
