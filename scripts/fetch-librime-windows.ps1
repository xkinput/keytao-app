param(
    [string]$Version = "latest",
    [string]$Arch = "x64",
    [ValidateSet("msvc", "clang")]
    [string]$Toolset = "msvc",
    [string]$Destination = ""
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Assert-SupportedWindowsLibrimeArch([string]$RequestedArch) {
    if ($RequestedArch -notin @("x64", "x86", "arm64")) {
        throw "Unsupported Windows librime SDK arch '$RequestedArch'. Supported values: x64, x86, arm64."
    }
}

function Resolve-RepoRoot {
    $scriptDir = Split-Path -Parent $PSCommandPath
    return [System.IO.Path]::GetFullPath((Join-Path $scriptDir ".."))
}

function Invoke-GitHubApi($Uri) {
    $headers = @{ "User-Agent" = "keytao-librime-fetch" }
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }
    Invoke-RestMethod -Uri $Uri -Headers $headers
}

function Find-Extractor {
    foreach ($name in @("7z", "7zz", "7za")) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        if ($cmd) {
            return @{ Kind = "7z"; Command = $cmd.Source }
        }
    }

    $tar = Get-Command "tar" -ErrorAction SilentlyContinue
    if ($tar) {
        return @{ Kind = "tar"; Command = $tar.Source }
    }

    throw "No extractor found. Install 7-Zip or use Scoop: scoop install 7zip"
}

function Expand-SevenZipArchive($Archive, $OutputDir) {
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
    $extractor = Find-Extractor
    if ($extractor.Kind -eq "7z") {
        & $extractor.Command x "-o$OutputDir" -y $Archive | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw "7z failed to extract $Archive"
        }
        return
    }

    & $extractor.Command -xf $Archive -C $OutputDir
    if ($LASTEXITCODE -ne 0) {
        throw "tar failed to extract $Archive; install 7-Zip and retry"
    }
}

function Copy-FlatFiles($Files, $DestinationDir) {
    New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
    foreach ($file in $Files) {
        Copy-Item -Force -LiteralPath $file.FullName -Destination (Join-Path $DestinationDir $file.Name)
    }
}

function Save-GitHubContentFile($Repo, $Path, $DestinationFile) {
    $uri = "https://api.github.com/repos/$Repo/contents/$Path"
    $content = Invoke-GitHubApi $uri
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $DestinationFile) | Out-Null
    if ($content.encoding -eq "base64") {
        $bytes = [Convert]::FromBase64String(($content.content -replace "\s", ""))
        [System.IO.File]::WriteAllBytes($DestinationFile, $bytes)
        return
    }

    if ($content.download_url) {
        Invoke-WebRequest -Uri $content.download_url -OutFile $DestinationFile -Headers @{ "User-Agent" = "keytao-librime-fetch" }
        return
    }

    throw "Unexpected GitHub content encoding for $Repo/$Path"
}

function Find-PluginRefInVersionInfo([string]$Path, [string]$Plugin) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $lines = @(Get-Content -LiteralPath $Path)
    for ($index = 0; $index -lt $lines.Count - 1; $index++) {
        if ($lines[$index].Trim() -eq $Plugin) {
            $ref = $lines[$index + 1].Trim()
            if ($ref) {
                return $ref
            }
        }
    }
    return $null
}

$repoRoot = Resolve-RepoRoot
if ($Arch -in @("arm", "aarch64")) {
    $Arch = "arm64"
}
Assert-SupportedWindowsLibrimeArch $Arch
if (-not $Destination) {
    $Destination = Join-Path $repoRoot "vendor\librime\windows-$Arch"
}
$Destination = [System.IO.Path]::GetFullPath($Destination)

$release = if ($Version -eq "latest") {
    Invoke-GitHubApi "https://api.github.com/repos/rime/librime/releases/latest"
} else {
    Invoke-GitHubApi "https://api.github.com/repos/rime/librime/releases/tags/$Version"
}

if ($Arch -eq "arm64") {
    & (Join-Path $repoRoot "scripts\build-librime-windows-arm64.ps1") `
        -Version $release.tag_name `
        -Destination $Destination
    if ($LASTEXITCODE -ne 0) {
        throw "build-librime-windows-arm64.ps1 failed with exit code $LASTEXITCODE"
    }

    $includeDest = Join-Path $Destination "include"
    $libDest = Join-Path $Destination "lib"
    $binDest = Join-Path $Destination "bin"
    $rimeDataDest = Join-Path $Destination "rime-data"
    New-Item -ItemType Directory -Force -Path $rimeDataDest | Out-Null

    Write-Host "Fetching base rime-data"
    foreach ($file in @("default.yaml", "key_bindings.yaml", "punctuation.yaml", "symbols.yaml")) {
        Save-GitHubContentFile "rime/rime-prelude" $file (Join-Path $rimeDataDest $file)
    }
    Save-GitHubContentFile "rime/rime-essay" "essay.txt" (Join-Path $rimeDataDest "essay.txt")

    $envFile = Join-Path $Destination "env.ps1"
    @"
if (-not `$env:LIBCLANG_PATH) {
    `$libclang = & cmd.exe /d /c "where `"libclang.dll`" 2>nul" | Select-Object -First 1
    if (`$libclang) {
        `$env:LIBCLANG_PATH = Split-Path -Parent `$libclang
    }
}
`$env:RIME_INCLUDE_DIR = "$includeDest"
`$env:RIME_LIB_DIR = "$libDest"
`$env:KEYTAO_RIME_LIB_NAME = "rime-arm64"
`$env:KEYTAO_RIME_DLL_NAME = "rime-arm64.dll"
`$env:Path = "$binDest;`$env:Path"
"@ | Set-Content -Encoding UTF8 -Path $envFile

    Write-Host ""
    Write-Host "librime ARM64 SDK is ready:"
    Write-Host "  $Destination"
    return
}

$assetPattern = "Windows-$Toolset-$Arch.7z"
$mainAsset = $release.assets |
    Where-Object { $_.name -like "rime-*" -and $_.name -notlike "rime-deps-*" -and $_.name -like "*$assetPattern" } |
    Select-Object -First 1
$depsAsset = $release.assets |
    Where-Object { $_.name -like "rime-deps-*" -and $_.name -like "*$assetPattern" } |
    Select-Object -First 1

if (-not $mainAsset) {
    $windowsAssets = ($release.assets | Where-Object { $_.name -like "*Windows*" } | Select-Object -ExpandProperty name) -join ", "
    throw "No librime asset matching $assetPattern in release $($release.tag_name). Available Windows assets: $windowsAssets"
}
if (-not $depsAsset) {
    $windowsAssets = ($release.assets | Where-Object { $_.name -like "*Windows*" } | Select-Object -ExpandProperty name) -join ", "
    throw "No librime dependency asset matching $assetPattern in release $($release.tag_name). Available Windows assets: $windowsAssets"
}

$cacheDir = Join-Path $repoRoot ".cache\librime\$($release.tag_name)\windows-$Toolset-$Arch"
$extractDir = Join-Path $cacheDir "extract"
New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $extractDir
New-Item -ItemType Directory -Force -Path $extractDir | Out-Null

foreach ($asset in @($mainAsset, $depsAsset)) {
    $archive = Join-Path $cacheDir $asset.name
    if (-not (Test-Path $archive)) {
        Write-Host "Downloading $($asset.name)"
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $archive -Headers @{ "User-Agent" = "keytao-librime-fetch" }
    } else {
        Write-Host "Using cached $($asset.name)"
    }

    $out = Join-Path $extractDir ([System.IO.Path]::GetFileNameWithoutExtension($asset.name))
    Expand-SevenZipArchive $archive $out
}

$header = Get-ChildItem -Recurse -File -Path $extractDir -Filter "rime_api.h" | Select-Object -First 1
if (-not $header) {
    throw "Extracted librime SDK does not contain rime_api.h"
}

$lib = Get-ChildItem -Recurse -File -Path $extractDir -Filter "rime.lib" | Select-Object -First 1
if (-not $lib) {
    throw "Extracted librime SDK does not contain rime.lib for MSVC linking"
}
$versionInfo = Get-ChildItem -Recurse -File -Path $extractDir -Filter "version-info.txt" |
    Select-Object -First 1
if (-not $versionInfo) {
    throw "Extracted librime SDK does not contain version-info.txt"
}
$luaPluginRef = Find-PluginRefInVersionInfo $versionInfo.FullName "hchunhui/librime-lua"
if (-not $luaPluginRef) {
    throw "Official librime SDK $($release.tag_name) does not declare merged librime-lua support."
}

$includeSource = Split-Path -Parent $header.FullName
$libSource = Split-Path -Parent $lib.FullName
$includeDest = Join-Path $Destination "include"
$libDest = Join-Path $Destination "lib"
$binDest = Join-Path $Destination "bin"
$rimeDataDest = Join-Path $Destination "rime-data"

Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Destination
New-Item -ItemType Directory -Force -Path $includeDest, $libDest, $binDest, $rimeDataDest | Out-Null

Copy-Item -Recurse -Force -Path (Join-Path $includeSource "*") -Destination $includeDest
Copy-FlatFiles (Get-ChildItem -File -Path $libSource -Filter "*.lib") $libDest
Copy-FlatFiles (Get-ChildItem -Recurse -File -Path $extractDir -Filter "*.dll") $binDest
Copy-Item -Force -LiteralPath $versionInfo.FullName `
    -Destination (Join-Path $Destination "librime-version-info.txt")

$rimeDll = Join-Path $binDest "rime.dll"
if (-not (Test-Path -LiteralPath $rimeDll -PathType Leaf)) {
    throw "Extracted librime SDK does not contain rime.dll"
}
$rimeDllText = [System.Text.Encoding]::ASCII.GetString(
    [System.IO.File]::ReadAllBytes($rimeDll)
)
foreach ($marker in @("lua_translator", "lua_filter", "lua_processor")) {
    if (-not $rimeDllText.Contains($marker)) {
        throw "Official librime SDK $($release.tag_name) is missing merged librime-lua marker '$marker'."
    }
}
@(
    "librime=$($release.tag_name)",
    "librime-lua=merged",
    "librime-lua-ref=$luaPluginRef"
) | Set-Content -LiteralPath (Join-Path $Destination "librime-features.txt") -Encoding ASCII

$rimeData = Get-ChildItem -Recurse -File -Path $extractDir -Filter "default.yaml" |
    Where-Object { $_.FullName -match "rime-data|share" } |
    Select-Object -First 1
if ($rimeData) {
    $dataSource = Split-Path -Parent $rimeData.FullName
    Copy-Item -Recurse -Force -Path (Join-Path $dataSource "*") -Destination $rimeDataDest
}

Write-Host "Fetching base rime-data"
foreach ($file in @("default.yaml", "key_bindings.yaml", "punctuation.yaml", "symbols.yaml")) {
    Save-GitHubContentFile "rime/rime-prelude" $file (Join-Path $rimeDataDest $file)
}
Save-GitHubContentFile "rime/rime-essay" "essay.txt" (Join-Path $rimeDataDest "essay.txt")

$envFile = Join-Path $Destination "env.ps1"
@"
if (-not `$env:LIBCLANG_PATH) {
    `$libclang = & cmd.exe /d /c "where `"libclang.dll`" 2>nul" | Select-Object -First 1
    if (`$libclang) {
        `$env:LIBCLANG_PATH = Split-Path -Parent `$libclang
    }
}
`$env:RIME_INCLUDE_DIR = "$includeDest"
`$env:RIME_LIB_DIR = "$libDest"
`$env:KEYTAO_RIME_LIB_NAME = "rime"
`$env:KEYTAO_RIME_DLL_NAME = "rime.dll"
`$env:Path = "$binDest;`$env:Path"
"@ | Set-Content -Encoding UTF8 -Path $envFile

Write-Host ""
Write-Host "librime SDK is ready:"
Write-Host "  $Destination"
Write-Host ""
Write-Host "For this PowerShell session, run:"
Write-Host "  . `"$envFile`""
Write-Host ""
Write-Host "Or use:"
Write-Host "  .\scripts\build-windows-ime.ps1"
