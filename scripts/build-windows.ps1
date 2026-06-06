param(
    [ValidateSet("x64")]
    [string]$Arch = "x64"
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path (Split-Path -Parent $PSCommandPath) "..")).Path

Push-Location $repoRoot
try {
    & powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch $Arch
    if ($LASTEXITCODE -ne 0) {
        throw "build-windows-ime.ps1 failed with exit code $LASTEXITCODE"
    }

    . (Join-Path $repoRoot "vendor\librime\windows-$Arch\env.ps1")

    pnpm tauri build --bundles nsis --config src-tauri/tauri.windows.conf.json
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
