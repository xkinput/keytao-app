param(
    [string]$X64RuntimeDir = "",
    [string]$Arm64RuntimeDir = "",
    [string]$OutputDir = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSCommandPath) ".."))
if (-not $X64RuntimeDir) {
    $X64RuntimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\x64"
}
if (-not $Arm64RuntimeDir) {
    $Arm64RuntimeDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\arm64"
}
if (-not $OutputDir) {
    $OutputDir = Join-Path $repoRoot "target\keytao-windows-ime-runtime\arm64x"
}

$x64Dll = Join-Path $X64RuntimeDir "keytao_windows_ime.dll"
$arm64Dll = Join-Path $Arm64RuntimeDir "keytao_windows_ime.dll"
$arm64RimeDll = Join-Path $Arm64RuntimeDir "rime-arm64.dll"
$arm64LibrimeFeatures = Join-Path $Arm64RuntimeDir "librime-features.txt"
foreach ($required in @($x64Dll, $arm64Dll, $arm64RimeDll, $arm64LibrimeFeatures)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Missing ARM64X input: $required"
    }
}

$vswhereCandidates = @(
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
    "${env:ProgramFiles}\Microsoft Visual Studio\Installer\vswhere.exe"
)
$vswhere = $vswhereCandidates |
    Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } |
    Select-Object -First 1
if (-not $vswhere) {
    throw "vswhere.exe was not found. Install Visual Studio 2022 Build Tools."
}
$vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64EC -property installationPath |
    Select-Object -First 1
if (-not $vsInstall) {
    throw "Visual Studio Build Tools does not include the ARM64EC toolchain."
}
$vsDevCmd = Join-Path $vsInstall "Common7\Tools\VsDevCmd.bat"
if (-not (Test-Path -LiteralPath $vsDevCmd -PathType Leaf)) {
    throw "VsDevCmd.bat was not found under $vsInstall."
}

$sourceDir = Join-Path $repoRoot "crates\keytao-windows-ime\arm64x"
$workDir = Join-Path $repoRoot "target\keytao-windows-tools\arm64x"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $workDir, $OutputDir
New-Item -ItemType Directory -Force -Path $workDir, $OutputDir | Out-Null

Copy-Item -Recurse -Force -Path (Join-Path $X64RuntimeDir "*") -Destination $OutputDir
Move-Item -Force -LiteralPath (Join-Path $OutputDir "keytao_windows_ime.dll") `
    -Destination (Join-Path $OutputDir "keytao_windows_ime_x64.dll")
Copy-Item -Force -LiteralPath $arm64Dll `
    -Destination (Join-Path $OutputDir "keytao_windows_ime_arm64.dll")
Copy-Item -Force -LiteralPath $arm64RimeDll -Destination $OutputDir
Copy-Item -Force -LiteralPath $arm64LibrimeFeatures `
    -Destination (Join-Path $OutputDir "librime-arm64-features.txt")

$dummySource = Join-Path $sourceDir "dummy.c"
$x64Def = Join-Path $sourceDir "keytao-windows-ime-x64.def"
$arm64Def = Join-Path $sourceDir "keytao-windows-ime-arm64.def"
$dummyArm64 = Join-Path $workDir "dummy-arm64.obj"
$dummyArm64Ec = Join-Path $workDir "dummy-arm64ec.obj"
$x64ImportLib = Join-Path $workDir "keytao-windows-ime-x64.lib"
$arm64ImportLib = Join-Path $workDir "keytao-windows-ime-arm64.lib"
$wrapper = Join-Path $OutputDir "keytao_windows_ime.dll"

$commands = @(
    "call `"$vsDevCmd`" -arch=arm64 -host_arch=x64 >nul",
    "cl.exe /nologo /c /Fo:`"$dummyArm64`" `"$dummySource`"",
    "cl.exe /nologo /c /arm64EC /Fo:`"$dummyArm64Ec`" `"$dummySource`"",
    "link.exe /lib /machine:arm64ec /def:`"$x64Def`" /out:`"$x64ImportLib`" /ignore:4104 /nologo",
    "link.exe /lib /machine:arm64 /def:`"$arm64Def`" /out:`"$arm64ImportLib`" /ignore:4104 /nologo",
    "link.exe /dll /noentry /machine:arm64x /defArm64Native:`"$arm64Def`" /def:`"$x64Def`" /out:`"$wrapper`" `"$dummyArm64`" `"$dummyArm64Ec`" `"$x64ImportLib`" `"$arm64ImportLib`" /ignore:4104 /nologo"
)
$commandLine = $commands -join " && "
& cmd.exe /d /s /c $commandLine
if ($LASTEXITCODE -ne 0) {
    throw "ARM64X wrapper build failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $wrapper -PathType Leaf)) {
    throw "ARM64X wrapper was not produced: $wrapper"
}

Write-Host ""
Write-Host "Windows ARM64X IME runtime is ready:"
Write-Host "  $OutputDir"
