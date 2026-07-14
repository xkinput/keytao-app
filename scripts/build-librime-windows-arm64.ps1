param(
    [string]$Version = "latest",
    [string]$Destination = "",
    [string]$LuaPluginRef = ""
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Resolve-RepoRoot {
    $scriptDir = Split-Path -Parent $PSCommandPath
    return [System.IO.Path]::GetFullPath((Join-Path $scriptDir ".."))
}

function Invoke-GitHubApi([string]$Uri) {
    $headers = @{ "User-Agent" = "keytao-librime-arm64-build" }
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }
    Invoke-RestMethod -Uri $Uri -Headers $headers
}

function Find-VsWhere {
    $candidates = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe",
        "${env:ProgramFiles}\Microsoft Visual Studio\Installer\vswhere.exe"
    )
    return $candidates |
        Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } |
        Select-Object -First 1
}

function Invoke-Checked([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

function Find-LuaPluginRefInVersionInfo([string]$Path) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $lines = @(Get-Content -LiteralPath $Path)
    for ($index = 0; $index -lt $lines.Count - 1; $index++) {
        if ($lines[$index].Trim() -eq "hchunhui/librime-lua") {
            $ref = $lines[$index + 1].Trim()
            if ($ref) {
                return $ref
            }
        }
    }
    return $null
}

$repoRoot = Resolve-RepoRoot
$release = if ($Version -eq "latest") {
    Invoke-GitHubApi "https://api.github.com/repos/rime/librime/releases/latest"
} else {
    Invoke-GitHubApi "https://api.github.com/repos/rime/librime/releases/tags/$Version"
}
$resolvedVersion = $release.tag_name
if (-not $resolvedVersion) {
    throw "Unable to resolve the librime release tag for '$Version'."
}

if (-not $Destination) {
    $Destination = Join-Path $repoRoot "vendor\librime\windows-arm64"
}
$Destination = [System.IO.Path]::GetFullPath($Destination)

if (-not $LuaPluginRef) {
    $LuaPluginRef = $env:KEYTAO_LIBRIME_LUA_REF
}
if (-not $LuaPluginRef) {
    $vendorRoot = Split-Path -Parent $Destination
    foreach ($versionInfo in @(
        (Join-Path $vendorRoot "windows-x64\librime-version-info.txt"),
        (Join-Path $vendorRoot "windows-x86\librime-version-info.txt")
    )) {
        $LuaPluginRef = Find-LuaPluginRefInVersionInfo $versionInfo
        if ($LuaPluginRef) {
            break
        }
    }
}
if (-not $LuaPluginRef) {
    $LuaPluginRef = (Invoke-GitHubApi "https://api.github.com/repos/hchunhui/librime-lua/commits/master").sha
}
if (-not $LuaPluginRef) {
    throw "Unable to resolve a librime-lua revision for the ARM64 build."
}

$cacheRoot = if ($env:KEYTAO_WINDOWS_BUILD_CACHE) {
    [System.IO.Path]::GetFullPath($env:KEYTAO_WINDOWS_BUILD_CACHE)
} else {
    Join-Path $repoRoot ".cache"
}
$cacheDir = Join-Path $cacheRoot "librime\$resolvedVersion\windows-msvc-arm64-source"
$sourceDir = Join-Path $cacheDir "source"
$buildDir = Join-Path $sourceDir "build-arm64"
$depsPrefix = Join-Path $cacheDir "deps-install"
$distDir = Join-Path $cacheDir "dist"
New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null

if (-not (Test-Path -LiteralPath (Join-Path $sourceDir ".git") -PathType Container)) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $sourceDir
    Invoke-Checked "git" @(
        "clone",
        "--branch", $resolvedVersion,
        "--depth", "1",
        "--recurse-submodules",
        "--shallow-submodules",
        "https://github.com/rime/librime.git",
        $sourceDir
    )
} else {
    Push-Location $sourceDir
    try {
        Invoke-Checked "git" @("submodule", "update", "--init", "--recursive", "--depth", "1")
    } finally {
        Pop-Location
    }
}

$luaPluginDir = Join-Path $sourceDir "plugins\lua"
if (-not (Test-Path -LiteralPath (Join-Path $luaPluginDir ".git") -PathType Container)) {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $luaPluginDir
    Invoke-Checked "git" @(
        "clone",
        "--no-checkout",
        "https://github.com/hchunhui/librime-lua.git",
        $luaPluginDir
    )
}
$checkoutRef = $LuaPluginRef
& git -C $luaPluginDir cat-file -e "$LuaPluginRef`^{commit}" 2>$null
if ($LASTEXITCODE -ne 0) {
    Invoke-Checked "git" @("-C", $luaPluginDir, "fetch", "--depth", "1", "origin", $LuaPluginRef)
    $checkoutRef = "FETCH_HEAD"
}
Invoke-Checked "git" @("-C", $luaPluginDir, "checkout", "--detach", $checkoutRef)
$resolvedLuaPluginRef = (& git -C $luaPluginDir rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or -not $resolvedLuaPluginRef) {
    throw "Unable to resolve the checked-out librime-lua revision."
}

$luaHeader = Join-Path $luaPluginDir "thirdparty\lua5.4\lua.h"
if (-not (Test-Path -LiteralPath $luaHeader -PathType Leaf)) {
    Push-Location $luaPluginDir
    try {
        & cmd.exe /d /s /c "call action-install.bat"
        if ($LASTEXITCODE -ne 0) {
            throw "librime-lua third-party source installation failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path -LiteralPath $luaHeader -PathType Leaf)) {
    throw "librime-lua did not install its bundled Lua source at $luaHeader."
}

$boostVersion = "1.84.0"
$boostFolder = "boost_" + $boostVersion.Replace(".", "_")
$boostCacheDir = Join-Path $cacheRoot "boost"
$boostRoot = Join-Path $boostCacheDir $boostFolder
if (-not (Test-Path -LiteralPath (Join-Path $boostRoot "boost") -PathType Container)) {
    New-Item -ItemType Directory -Force -Path $boostCacheDir | Out-Null
    $boostArchive = Join-Path $boostCacheDir "$boostFolder.zip"
    if (-not (Test-Path -LiteralPath $boostArchive -PathType Leaf)) {
        $boostUrl = "https://archives.boost.io/release/$boostVersion/source/$boostFolder.zip"
        Write-Host "Downloading Boost $boostVersion headers"
        Invoke-WebRequest -Uri $boostUrl -OutFile $boostArchive -Headers @{ "User-Agent" = "keytao-librime-arm64-build" }
    }
    Expand-Archive -LiteralPath $boostArchive -DestinationPath $boostCacheDir -Force
}

$pythonVersion = "3.12.10"
$pythonRoot = Join-Path $cacheRoot "python\$pythonVersion-amd64"
$pythonExecutable = Join-Path $pythonRoot "python.exe"
if (-not (Test-Path -LiteralPath $pythonExecutable -PathType Leaf)) {
    New-Item -ItemType Directory -Force -Path $pythonRoot | Out-Null
    $pythonArchive = Join-Path $pythonRoot "python-embed-amd64.zip"
    if (-not (Test-Path -LiteralPath $pythonArchive -PathType Leaf)) {
        $pythonUrl = "https://www.python.org/ftp/python/$pythonVersion/python-$pythonVersion-embed-amd64.zip"
        Write-Host "Downloading Python $pythonVersion build interpreter"
        Invoke-WebRequest -Uri $pythonUrl -OutFile $pythonArchive -Headers @{ "User-Agent" = "keytao-librime-arm64-build" }
    }
    Expand-Archive -LiteralPath $pythonArchive -DestinationPath $pythonRoot -Force
}

$pythonPathFile = Join-Path $pythonRoot "python312._pth"
$openccScripts = [System.IO.Path]::GetFullPath((Join-Path $sourceDir "deps\opencc\data\scripts"))
if (-not (Test-Path -LiteralPath $pythonPathFile -PathType Leaf)) {
    throw "Embedded Python path configuration was not found at $pythonPathFile."
}
$pythonPathEntries = @(Get-Content -LiteralPath $pythonPathFile)
if ($pythonPathEntries -notcontains $openccScripts) {
    $pythonPathEntries += $openccScripts
    Set-Content -LiteralPath $pythonPathFile -Encoding ASCII -Value $pythonPathEntries
}

$vswhere = Find-VsWhere
if (-not $vswhere) {
    throw "vswhere.exe was not found. Install Visual Studio Build Tools."
}
$vsInstall = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath |
    Select-Object -First 1
if (-not $vsInstall) {
    throw "Visual Studio Build Tools does not include the ARM64 C++ toolchain."
}
$vsInstallationVersion = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationVersion |
    Select-Object -First 1
$vsMajorVersion = 0
if (-not $vsInstallationVersion -or
    -not [int]::TryParse(($vsInstallationVersion -split '\.')[0], [ref]$vsMajorVersion)) {
    throw "Unable to determine the Visual Studio version at $vsInstall."
}
$vcvarsall = Join-Path $vsInstall "VC\Auxiliary\Build\vcvarsall.bat"
$cmakeBin = Join-Path $vsInstall "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
$cmakeExecutable = Join-Path $cmakeBin "cmake.exe"
if (-not (Test-Path -LiteralPath $vcvarsall -PathType Leaf)) {
    throw "vcvarsall.bat was not found under $vsInstall."
}
if (-not (Test-Path -LiteralPath $cmakeExecutable -PathType Leaf)) {
    throw "Visual Studio CMake was not found under $vsInstall."
}
$cmakeHelp = (& $cmakeExecutable --help 2>&1 | Out-String)
$generatorMatch = [regex]::Match(
    $cmakeHelp,
    "(?m)^\*?\s*(Visual Studio $vsMajorVersion \d{4})\s*="
)
if (-not $generatorMatch.Success) {
    throw "Visual Studio $vsMajorVersion is installed, but its bundled CMake does not provide a matching generator."
}
$cmakeGenerator = $generatorMatch.Groups[1].Value
Write-Host "Using CMake generator '$cmakeGenerator' from Visual Studio $vsInstallationVersion"

$sourceCmake = Join-Path $sourceDir "src\CMakeLists.txt"
$cmakeText = Get-Content -LiteralPath $sourceCmake -Raw
if ($cmakeText -notmatch 'OUTPUT_NAME\s+"rime-arm64"') {
    $needle = '    DEFINE_SYMBOL "RIME_EXPORTS"'
    if (-not $cmakeText.Contains($needle)) {
        throw "Unable to patch librime output name in $sourceCmake."
    }
    $cmakeText = $cmakeText.Replace(
        $needle,
        "$needle`r`n    OUTPUT_NAME `"rime-arm64`""
    )
    Set-Content -LiteralPath $sourceCmake -Encoding UTF8 -NoNewline -Value $cmakeText
}

Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $buildDir, $distDir
New-Item -ItemType Directory -Force -Path $depsPrefix, $distDir | Out-Null

$oldEnvironment = @{}
foreach ($name in @(
    "ARCH",
    "BOOST_ROOT",
    "DEVTOOLS_PATH",
    "RIME_ROOT",
    "CMAKE_GENERATOR",
    "PYTHON_EXECUTABLE",
    "RIME_PLUGINS",
    "common_cmake_flags",
    "build_dir",
    "deps_install_prefix",
    "rime_install_prefix"
)) {
    $oldEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

try {
    $env:ARCH = "ARM64"
    $env:BOOST_ROOT = $boostRoot
    $env:DEVTOOLS_PATH = $cmakeBin
    $env:RIME_ROOT = $sourceDir
    $env:CMAKE_GENERATOR = $cmakeGenerator
    $env:PYTHON_EXECUTABLE = $pythonExecutable
    $env:RIME_PLUGINS = "hchunhui/librime-lua@$resolvedLuaPluginRef"
    $env:common_cmake_flags = "-DPYTHON_EXECUTABLE:FILEPATH=$pythonExecutable"
    $env:build_dir = "build-arm64"
    $env:deps_install_prefix = $depsPrefix
    $env:rime_install_prefix = $distDir

    @"
set RIME_ROOT=$sourceDir
set BOOST_ROOT=$boostRoot
set ARCH=ARM64
set CMAKE_GENERATOR="$cmakeGenerator"
set PYTHON_EXECUTABLE=$pythonExecutable
set RIME_PLUGINS=hchunhui/librime-lua@$resolvedLuaPluginRef
"@ | Set-Content -LiteralPath (Join-Path $sourceDir "env.bat") -Encoding ASCII

    Push-Location $sourceDir
    try {
        $command = "call `"$vcvarsall`" x64_arm64 >nul && call build.bat deps librime shared nologging"
        & cmd.exe /d /s /c $command
        if ($LASTEXITCODE -ne 0) {
            throw "librime ARM64 source build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
} finally {
    foreach ($entry in $oldEnvironment.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "Process")
    }
}

$rimeDll = Get-ChildItem -Path @($distDir, $buildDir) -Recurse -File -Filter "rime-arm64.dll" |
    Select-Object -First 1
$rimeLib = Get-ChildItem -Path @($distDir, $buildDir) -Recurse -File -Filter "rime-arm64.lib" |
    Select-Object -First 1
$rimeHeader = Get-ChildItem -Path @($distDir, $sourceDir) -Recurse -File -Filter "rime_api.h" |
    Select-Object -First 1
if (-not $rimeDll -or -not $rimeLib -or -not $rimeHeader) {
    throw "The librime ARM64 build did not produce rime-arm64.dll, rime-arm64.lib, and rime_api.h."
}

$rimeDllText = [System.Text.Encoding]::ASCII.GetString(
    [System.IO.File]::ReadAllBytes($rimeDll.FullName)
)
foreach ($marker in @("lua_translator", "lua_filter", "lua_processor")) {
    if (-not $rimeDllText.Contains($marker)) {
        throw "The librime ARM64 build is missing merged librime-lua marker '$marker'."
    }
}

$includeDir = Join-Path $Destination "include"
$libDir = Join-Path $Destination "lib"
$binDir = Join-Path $Destination "bin"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Destination
New-Item -ItemType Directory -Force -Path $includeDir, $libDir, $binDir | Out-Null

$headerDir = Split-Path -Parent $rimeHeader.FullName
Get-ChildItem -LiteralPath $headerDir -File -Filter "rime*.h" |
    Copy-Item -Force -Destination $includeDir
Copy-Item -Force -LiteralPath $rimeLib.FullName -Destination $libDir
Copy-Item -Force -LiteralPath $rimeDll.FullName -Destination $binDir
Set-Content -LiteralPath (Join-Path $Destination "librime-release.txt") -Encoding ASCII -Value $resolvedVersion
@(
    "librime=$resolvedVersion",
    "librime-lua=merged",
    "librime-lua-ref=$resolvedLuaPluginRef"
) | Set-Content -LiteralPath (Join-Path $Destination "librime-features.txt") -Encoding ASCII

Write-Host ""
Write-Host "librime ARM64 SDK is ready:"
Write-Host "  $Destination"
