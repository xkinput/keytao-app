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

    $installer = Get-ChildItem -Path "target\release\bundle\nsis" -Filter "*.exe" -File | Select-Object -First 1
    if (-not $installer) {
        throw "missing Windows NSIS .exe installer"
    }

    $forbiddenInstallers = Get-ChildItem -Path "target\release\bundle" -Recurse -File |
        Where-Object { $_.Extension -in @(".msi", ".zip", ".appx", ".msix", ".msixbundle") }
    if ($forbiddenInstallers) {
        throw "Windows build must only produce the NSIS .exe installer. Unexpected artifacts: $($forbiddenInstallers.FullName -join ', ')"
    }
} finally {
    Pop-Location
}
