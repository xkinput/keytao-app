param(
    [string]$MakeNsis = (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
    [string]$Clang = 'clang.exe'
)

# Exercise the production macro against genuinely mapped native DLLs. No real
# installation, input method, user process or pending-reboot registry is touched.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).ProviderPath
$testRoot = Join-Path $repoRoot ('target\keytao-payload-tests\' + [Guid]::NewGuid().ToString('N'))
$bundle = Join-Path $testRoot 'bundle'
$installed = Join-Path $testRoot 'installed'
$utf8 = New-Object Text.UTF8Encoding($true)
New-Item -ItemType Directory -Path $bundle, $installed, (Join-Path $bundle 'nested') -Force | Out-Null
[IO.File]::WriteAllText((Join-Path $testRoot 'old.c'), '__declspec(dllexport) int fixture_value(void) { return 1; }')
[IO.File]::WriteAllText((Join-Path $testRoot 'new.c'), '__declspec(dllexport) int fixture_value(void) { return 2; }')
& $Clang -fuse-ld=lld -shared -nostdlib -Xlinker /noentry -o (Join-Path $installed 'fixture.dll') (Join-Path $testRoot 'old.c')
if ($LASTEXITCODE -ne 0) { throw 'Unable to compile the original native fixture DLL.' }
& $Clang -fuse-ld=lld -shared -nostdlib -Xlinker /noentry -o (Join-Path $bundle 'fixture.dll') (Join-Path $testRoot 'new.c')
if ($LASTEXITCODE -ne 0) { throw 'Unable to compile the replacement native fixture DLL.' }
[IO.File]::WriteAllText((Join-Path $bundle 'nested\data.txt'), 'new bundled data')
[IO.File]::WriteAllText((Join-Path $bundle 'collision.txt'), 'replacement')

$nativeSource = @'
using System;
using System.Runtime.InteropServices;
public static class KeyTaoPayloadFixture {
 [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr LoadLibrary(string path);
 [DllImport("kernel32", CharSet=CharSet.Ansi, SetLastError=true)] static extern IntPtr GetProcAddress(IntPtr module, string name);
 [DllImport("kernel32", SetLastError=true)] public static extern bool FreeLibrary(IntPtr module);
 [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate int ValueFunction();
 public static int Value(IntPtr module) {
  var pointer=GetProcAddress(module,"fixture_value");
  if(pointer==IntPtr.Zero) throw new Exception("Missing fixture export");
  return ((ValueFunction)Marshal.GetDelegateForFunctionPointer(pointer,typeof(ValueFunction)))();
 }
}
'@
Add-Type -TypeDefinition $nativeSource
$nativePath = Join-Path $testRoot 'native.cs'
[IO.File]::WriteAllText($nativePath, $nativeSource)
$readerSource = @'
param([string]$DllPath)
$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'native.cs')
$module = [KeyTaoPayloadFixture]::LoadLibrary($DllPath)
if ($module -eq [IntPtr]::Zero) { throw 'Fresh process could not load replacement DLL.' }
try {
 if ([KeyTaoPayloadFixture]::Value($module) -ne 2) { throw 'Fresh process loaded old DLL bytes.' }
} finally { $null = [KeyTaoPayloadFixture]::FreeLibrary($module) }
'@
$readerPath = Join-Path $testRoot 'read-replacement.ps1'
[IO.File]::WriteAllText($readerPath, $readerSource, $utf8)
$installerSource = @'
Unicode true
!define BUNDLE_DIR "@BUNDLE@"
!define KEYTAO_PAYLOAD_TEST_SKIP_REBOOT_DELETE
!include /CHARSET=UTF8 "@INCLUDE@"
Name "KeyTao payload fixture"
OutFile "@OUTPUT@"
RequestExecutionLevel user
SilentInstall silent
Section
  ${If} ${FileExists} "$INSTDIR\directory-case.txt"
    !insertmacro InstallPayloadFile "collision.txt"
    SetErrorLevel 99
    Return
  ${EndIf}
  StrCpy $0 "preserve-register-zero"
  StrCpy $1 "preserve-register-one"
  SetRebootFlag false
  !insertmacro InstallPayloadFile "fixture.dll"
  IfRebootFlag failed
  SetRebootFlag true
  !insertmacro InstallPayloadFile "nested\data.txt"
  IfRebootFlag +2
    Goto failed
  !insertmacro InstallPayloadFile "nested\data.txt"
  IfRebootFlag +2
    Goto failed
  StrCmp $0 "preserve-register-zero" 0 failed
  StrCmp $1 "preserve-register-one" 0 failed
  SetRebootFlag false
  SetErrorLevel 0
  Goto done
  failed:
    SetErrorLevel 88
  done:
SectionEnd
'@
$fixtureExe = Join-Path $testRoot 'payload-fixture.exe'
$installerSource = $installerSource.Replace('@BUNDLE@', $bundle).Replace(
    '@INCLUDE@', (Join-Path $repoRoot 'packaging\windows\replace-payload.nsh')).Replace('@OUTPUT@', $fixtureExe)
$installerPath = Join-Path $testRoot 'payload-fixture.nsi'
[IO.File]::WriteAllText($installerPath, $installerSource, $utf8)
& $MakeNsis '/INPUTCHARSET' 'UTF8' '/V2' $installerPath
if ($LASTEXITCODE -ne 0) { throw "NSIS compilation failed; artifacts: $testRoot" }

function Invoke-PayloadFixture([string]$Directory) {
    $process = Start-Process -FilePath $fixtureExe -ArgumentList ('/S /D=' + $Directory) -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(30000)) { throw "Isolated installer timed out: $($process.Id)" }
        return $process.ExitCode
    } finally { $process.Dispose() }
}

$dll = Join-Path $installed 'fixture.dll'
$module = [KeyTaoPayloadFixture]::LoadLibrary($dll)
if ($module -eq [IntPtr]::Zero) { throw 'Unable to map the original native DLL.' }
try {
    if ([KeyTaoPayloadFixture]::Value($module) -ne 1) { throw 'Original DLL fixture is invalid.' }
    $overwriteBlocked = $false
    try { Copy-Item -LiteralPath (Join-Path $bundle 'fixture.dll') -Destination $dll -Force } catch { $overwriteBlocked = $true }
    if (-not $overwriteBlocked) { throw 'Fixture does not reproduce Windows image-file locking.' }
    if ((Invoke-PayloadFixture $installed) -ne 0) { throw 'Production replacement macro or register/reboot preservation failed.' }
    if ([KeyTaoPayloadFixture]::Value($module) -ne 1) { throw 'Replacement changed the existing mapped module.' }
    if ((Get-FileHash -LiteralPath $dll).Hash -ne (Get-FileHash -LiteralPath (Join-Path $bundle 'fixture.dll')).Hash) {
        throw 'Replacement DLL bytes do not match the bundle.'
    }
    if ([IO.File]::ReadAllText((Join-Path $installed 'nested\data.txt')) -ne 'new bundled data') {
        throw 'Nested creation and repeated file replacement failed.'
    }
    $backups = @(Get-ChildItem -LiteralPath $installed -File -Filter '*.tmp')
    if ($backups.Count -ne 1) { throw 'Expected only the mapped old DLL backup to remain.' }
    $nativePowerShell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (-not [Environment]::Is64BitProcess) {
        $nativePowerShell = Join-Path $env:WINDIR 'Sysnative\WindowsPowerShell\v1.0\powershell.exe'
    }
    $arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $readerPath + '" -DllPath "' + $dll + '"'
    $reader = Start-Process -FilePath $nativePowerShell -ArgumentList $arguments -WindowStyle Hidden -PassThru
    try {
        if (-not $reader.WaitForExit(30000) -or $reader.ExitCode -ne 0) { throw 'Fresh process did not execute replacement DLL.' }
    } finally { $reader.Dispose() }
    Write-Host 'PASS: locked DLL replaced; existing host executes old code; fresh host executes new code.'
    Write-Host 'PASS: nested files, repeat calls, scratch registers and original reboot flags are preserved.'
} finally { $null = [KeyTaoPayloadFixture]::FreeLibrary($module) }

foreach ($backup in $backups) { Remove-Item -LiteralPath $backup.FullName -Force }
$collisionRoot = Join-Path $testRoot 'directory-collision'
New-Item -ItemType Directory -Path (Join-Path $collisionRoot 'collision.txt') -Force | Out-Null
[IO.File]::WriteAllText((Join-Path $collisionRoot 'directory-case.txt'), 'fixture mode')
[IO.File]::WriteAllText((Join-Path $collisionRoot 'collision.txt\keep.txt'), 'unrelated user file')
$collisionExit = Invoke-PayloadFixture $collisionRoot
if ($collisionExit -eq 0 -or $collisionExit -eq 99 -or
    [IO.File]::ReadAllText((Join-Path $collisionRoot 'collision.txt\keep.txt')) -ne 'unrelated user file') {
    throw 'A directory occupying an installed file path must be retained and block replacement.'
}
Write-Host 'PASS: occupied directory and unrelated contents are preserved; original DLL backup releases naturally.'
Write-Host "Payload replacement fixture passed. Artifacts: $testRoot"
