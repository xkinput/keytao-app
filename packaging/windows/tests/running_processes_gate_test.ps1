param(
    [string]$MakeNsis = 'makensis.exe'
)

# Exercise the production NSIS gate without launching the real installer,
# loading an input method, changing registration, or stopping any application.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).ProviderPath
$nsisCommand = Get-Command $MakeNsis -ErrorAction SilentlyContinue
if ($nsisCommand) {
    $MakeNsis = $nsisCommand.Source
} elseif ($MakeNsis -eq 'makensis.exe') {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'),
        (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe')
    )
    $MakeNsis = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $MakeNsis) { throw 'NSIS was not found. Pass -MakeNsis with its executable path.' }
} else {
    throw "NSIS was not found: $MakeNsis"
}
$testRoot = Join-Path $repoRoot ('target\keytao-windows-preflight-tests\nsis-gate-' + [Guid]::NewGuid().ToString('N'))
$helperDirectory = Join-Path $testRoot 'packaging\windows'
New-Item -ItemType Directory -Path $helperDirectory -Force | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($true)
$reportText = [Regex]::Unescape('\u5019\u9009\u7a97\u53e3\u5360\u7528\u68c0\u6d4b\uff1a\u6d4b\u8bd5\u62a5\u544a')

# A missing report deliberately returns success: the gate must fail closed
# when the helper exits normally but its expected output is unavailable.
$helper = @'
param([string]$InstallDir, [string]$ReportPath, [switch]$StopOwnedProcesses)
$ErrorActionPreference = 'Stop'
$invocation = if ($StopOwnedProcesses) { 'stop' } else { 'preview' }
[IO.File]::AppendAllText((Join-Path $InstallDir 'helper-calls.txt'), $invocation + "`r`n")
$mode = [IO.File]::ReadAllText((Join-Path $InstallDir 'helper-mode.txt')).Trim()
if ($mode -eq 'missing-report') { exit 0 }
$report = [Regex]::Unescape('\u5019\u9009\u7a97\u53e3\u5360\u7528\u68c0\u6d4b\uff1a\u6d4b\u8bd5\u62a5\u544a')
$reportEncoding = New-Object System.Text.UnicodeEncoding($false, $false)
[IO.File]::WriteAllText($ReportPath, $report + "`r`n", $reportEncoding)
exit ([int]$mode)
'@
[IO.File]::WriteAllText((Join-Path $helperDirectory 'check-running-processes.ps1'), $helper, $utf8)

# Both sections place their first mutation after the real gate. Each test
# receives a separate /D= or _?= directory, so no cleanup can mask a failure.
$fixtureSource = @'
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!define REPO_ROOT "@FIXTURE_ROOT@"
Name "KeyTao preflight test fixture"
OutFile "@FIXTURE_ROOT@\gate-fixture.exe"
InstallDir "$EXEDIR\unused-default"
RequestExecutionLevel user
Page custom CreateRunningProcessesPage LeaveRunningProcessesPage
!insertmacro MUI_PAGE_INSTFILES
UninstPage custom un.CreateRunningProcessesPage un.LeaveRunningProcessesPage
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"
!include /CHARSET=UTF8 "@REAL_INCLUDE@"

Function .onInit
  IfFileExists "$INSTDIR\preview-only.txt" 0 preview_done
  StrCpy $PreflightStopArg ""
  Call CheckRunningProcesses
  SetErrorLevel 0
  Quit
  preview_done:
FunctionEnd

Function un.onInit
  IfFileExists "$INSTDIR\preview-only.txt" 0 preview_done
  StrCpy $PreflightStopArg ""
  Call un.CheckRunningProcesses
  SetErrorLevel 0
  Quit
  preview_done:
FunctionEnd

Section "Fixture install"
  IfFileExists "$INSTDIR\continue-first.txt" 0 +2
  Call LeaveRunningProcessesPage
  Call EnsureProcessesStopped
  FileOpen $0 "$INSTDIR\install-marker.txt" w
  FileWriteUTF16LE $0 "$PreflightReport"
  FileClose $0
  WriteUninstaller "$INSTDIR\uninstall-fixture.exe"
SectionEnd

Section "Uninstall"
  IfFileExists "$INSTDIR\continue-first.txt" 0 +2
  Call un.LeaveRunningProcessesPage
  Call un.EnsureProcessesStopped
  FileOpen $0 "$INSTDIR\uninstall-marker.txt" w
  FileWriteUTF16LE $0 "$PreflightReport"
  FileClose $0
SectionEnd
'@
$fixtureSource = $fixtureSource.Replace('@FIXTURE_ROOT@', $testRoot).Replace(
    '@REAL_INCLUDE@', (Join-Path $repoRoot 'packaging\windows\running-processes.nsh'))
$sourcePath = Join-Path $testRoot 'gate-fixture.nsi'
[IO.File]::WriteAllText($sourcePath, $fixtureSource, $utf8)
$compilerOutput = @(& $MakeNsis '/INPUTCHARSET' 'UTF8' '/V2' $sourcePath 2>&1)
$compilerExit = $LASTEXITCODE
[IO.File]::WriteAllLines((Join-Path $testRoot 'compile.log'), [string[]]$compilerOutput, $utf8)
if ($compilerExit -ne 0) {
    throw "NSIS fixture compilation failed (exit $compilerExit). See $testRoot\compile.log"
}

$cases = @(
    @{ Name = 'clear'; Mode = '0'; Exit = 0; Marker = $true; Calls = @('stop', 'stop') },
    @{ Name = 'busy'; Mode = '10'; Exit = 32; Marker = $false; Calls = @('stop') },
    @{ Name = 'failed'; Mode = '20'; Exit = 1; Marker = $false; Calls = @('stop') },
    @{ Name = 'missing-report'; Mode = 'missing-report'; Exit = 1; Marker = $false; Calls = @('stop') },
    @{ Name = 'preview'; Mode = '10'; Exit = 0; Marker = $false; Calls = @('preview') }
)
$fixtureExe = Join-Path $testRoot 'gate-fixture.exe'
$uninstaller = Join-Path $testRoot 'install-clear\uninstall-fixture.exe'
$passed = 0
foreach ($operation in @('install', 'uninstall')) {
    foreach ($case in $cases) {
        $caseName = "$operation-$($case.Name)"
        $caseDirectory = Join-Path $testRoot $caseName
        New-Item -ItemType Directory -Path $caseDirectory | Out-Null
        [IO.File]::WriteAllText((Join-Path $caseDirectory 'helper-mode.txt'), $case.Mode)
        if ($case.Name -eq 'clear') {
            # Exercise the page's Continue callback and the later section gate.
            [IO.File]::WriteAllText((Join-Path $caseDirectory 'continue-first.txt'), '')
        } elseif ($case.Name -eq 'preview') {
            [IO.File]::WriteAllText((Join-Path $caseDirectory 'preview-only.txt'), '')
        }
        $markerPath = Join-Path $caseDirectory "$operation-marker.txt"
        if ($operation -eq 'install') {
            $executable = $fixtureExe
            # NSIS requires /D= and _?= to be last, without surrounding quotes;
            # both consume the remainder of the command line, including spaces.
            $arguments = '/S /D=' + $caseDirectory
        } else {
            $executable = $uninstaller
            # _?= avoids the uninstaller's detached temporary copy, so the
            # observed exit code belongs to the process executing our gate.
            $arguments = '/S _?=' + $caseDirectory
        }
        $process = Start-Process -FilePath $executable -ArgumentList $arguments -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(90000)) {
            throw "$caseName timed out; fixture PID $($process.Id), artifacts: $testRoot"
        }
        $exitCode = $process.ExitCode
        $process.Dispose()
        if ($exitCode -ne $case.Exit) {
            throw "$caseName returned $exitCode instead of $($case.Exit); artifacts: $testRoot"
        }
        $calls = [IO.File]::ReadAllLines((Join-Path $caseDirectory 'helper-calls.txt'))
        if (($calls -join ',') -cne ($case.Calls -join ',')) {
            throw "$caseName used helper modes [$($calls -join ',')] instead of [$($case.Calls -join ',')]; artifacts: $testRoot"
        }
        if ((Test-Path -LiteralPath $markerPath) -ne $case.Marker) {
            throw "$caseName mutation marker did not match expected gate result; artifacts: $testRoot"
        }
        if ($case.Marker) {
            $actualReport = [IO.File]::ReadAllText($markerPath, [Text.Encoding]::Unicode)
            if (-not $actualReport.Contains($reportText)) {
                throw "$caseName did not read the Unicode helper report correctly; artifacts: $testRoot"
            }
        }
        $passed += 1
        Write-Host "PASS $caseName (exit $exitCode)"
    }
}
# Exercise the real upgrade decision separately. The mock old uninstaller is
# a genuine NSIS uninstaller, so ExecWait, /UPDATE and _?= follow production
# semantics. It only records its command line and returns the configured code.
$upgradeSource = @'
Unicode true
!include "LogicLib.nsh"
Name "KeyTao upgrade test fixture"
OutFile "@FIXTURE_ROOT@\upgrade-fixture.exe"
InstallDir "$EXEDIR\unused-default"
RequestExecutionLevel user
!include /CHARSET=UTF8 "@UPGRADE_INCLUDE@"

Section "Fixture upgrade"
  IfFileExists "$INSTDIR\create-mock-uninstaller.txt" 0 upgrade
  WriteUninstaller "$INSTDIR\uninstall.exe"
  SetErrorLevel 0
  Quit
  upgrade:
  Call PrepareExistingInstallation
  FileOpen $0 "$INSTDIR\upgrade-marker.txt" w
  FileWrite $0 "upgrade continued"
  FileClose $0
SectionEnd

Section "Uninstall"
  FileOpen $0 "$INSTDIR\old-uninstaller-called.txt" w
  FileWriteUTF16LE $0 "$CMDLINE$\r$\n$INSTDIR"
  FileClose $0
  FileOpen $0 "$INSTDIR\old-uninstaller-exit.txt" r
  FileRead $0 $1
  FileClose $0
  SetErrorLevel $1
  Quit
SectionEnd
'@
$upgradeSource = $upgradeSource.Replace('@FIXTURE_ROOT@', $testRoot).Replace(
    '@UPGRADE_INCLUDE@', (Join-Path $repoRoot 'packaging\windows\upgrade-existing.nsh'))
$upgradeSourcePath = Join-Path $testRoot 'upgrade-fixture.nsi'
[IO.File]::WriteAllText($upgradeSourcePath, $upgradeSource, $utf8)
$compilerOutput = @(& $MakeNsis '/INPUTCHARSET' 'UTF8' '/V2' $upgradeSourcePath 2>&1)
$compilerExit = $LASTEXITCODE
[IO.File]::WriteAllLines((Join-Path $testRoot 'upgrade-compile.log'), [string[]]$compilerOutput, $utf8)
if ($compilerExit -ne 0) {
    throw "NSIS upgrade fixture compilation failed (exit $compilerExit). See $testRoot\upgrade-compile.log"
}

function Invoke-UpgradeFixture([string]$InstallDirectory) {
    $process = Start-Process -FilePath (Join-Path $testRoot 'upgrade-fixture.exe') -ArgumentList (
        '/S /D=' + $InstallDirectory) -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(90000)) {
        throw "Upgrade fixture timed out; PID $($process.Id), artifacts: $testRoot"
    }
    $exitCode = $process.ExitCode
    $process.Dispose()
    return $exitCode
}

$mockDirectory = Join-Path $testRoot 'mock-old-installation'
New-Item -ItemType Directory -Path $mockDirectory | Out-Null
[IO.File]::WriteAllText((Join-Path $mockDirectory 'create-mock-uninstaller.txt'), '')
if ((Invoke-UpgradeFixture $mockDirectory) -ne 0) {
    throw "Unable to generate mock old uninstaller; artifacts: $testRoot"
}
$upgradeCases = @(
    @{ Name = 'flutter-blocking-uninstaller'; Flutter = $true; OldUninstaller = $true; OldExit = 32; Called = $false; Continue = $true },
    @{ Name = 'flutter-missing-uninstaller'; Flutter = $true; OldUninstaller = $false; OldExit = 0; Called = $false; Continue = $true },
    @{ Name = 'tauri-success'; Flutter = $false; OldUninstaller = $true; OldExit = 0; Called = $true; Continue = $true },
    @{ Name = 'tauri-failed'; Flutter = $false; OldUninstaller = $true; OldExit = 32; Called = $true; Continue = $false },
    @{ Name = 'first-install'; Flutter = $false; OldUninstaller = $false; OldExit = 0; Called = $false; Continue = $true }
)
foreach ($case in $upgradeCases) {
    $caseDirectory = Join-Path $testRoot ('upgrade cases\' + $case.Name)
    New-Item -ItemType Directory -Path $caseDirectory -Force | Out-Null
    if ($case.Name -ne 'first-install') {
        [IO.File]::WriteAllText((Join-Path $caseDirectory 'keytao-app.exe'), 'fixture')
    }
    if ($case.Flutter) {
        # Real interrupted upgrades may have lost data/app.so already. The
        # Flutter engine and KeyTao-specific bridge still identify this layout.
        foreach ($file in @('flutter_windows.dll', 'keytao_app_bridge.dll')) {
            [IO.File]::WriteAllText((Join-Path $caseDirectory $file), 'fixture')
        }
    }
    if ($case.OldUninstaller) {
        Copy-Item -LiteralPath (Join-Path $mockDirectory 'uninstall.exe') -Destination (
            Join-Path $caseDirectory 'uninstall.exe')
        [IO.File]::WriteAllText((Join-Path $caseDirectory 'old-uninstaller-exit.txt'), [string]$case.OldExit)
    }
    $exitCode = Invoke-UpgradeFixture $caseDirectory
    if (($exitCode -eq 0) -ne $case.Continue) {
        throw "Upgrade $($case.Name) returned unexpected exit $exitCode; artifacts: $testRoot"
    }
    if ((Test-Path -LiteralPath (Join-Path $caseDirectory 'upgrade-marker.txt')) -ne $case.Continue) {
        throw "Upgrade $($case.Name) continued unexpectedly or failed to continue; artifacts: $testRoot"
    }
    $calledPath = Join-Path $caseDirectory 'old-uninstaller-called.txt'
    if ((Test-Path -LiteralPath $calledPath) -ne $case.Called) {
        throw "Upgrade $($case.Name) called/skipped the old uninstaller incorrectly; artifacts: $testRoot"
    }
    if ($case.Called) {
        $commandLine = [IO.File]::ReadAllText($calledPath, [Text.Encoding]::Unicode)
        # NSIS removes its internal _?= argument from $CMDLINE; the resolved
        # $INSTDIR on the second line verifies the destination, including spaces.
        if (-not $commandLine.Contains('/S /UPDATE') -or -not $commandLine.EndsWith("`r`n" + $caseDirectory)) {
            throw "Upgrade $($case.Name) did not preserve the old uninstaller arguments; artifacts: $testRoot"
        }
    }
    $passed += 1
    Write-Host "PASS upgrade-$($case.Name) (exit $exitCode)"
}
Write-Host "NSIS preflight and upgrade: $passed tests passed. Artifacts: $testRoot"
