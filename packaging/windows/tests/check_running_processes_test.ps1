param(
    [string]$SetupHelper = (Join-Path $PSScriptRoot '..\..\..\target\x86_64-pc-windows-msvc\release\keytao-setup-helper.exe')
)

$ErrorActionPreference = 'Stop'
$SetupHelper = (Resolve-Path -LiteralPath $SetupHelper).ProviderPath

function Invoke-SetupHelper([string]$InstallDirectory, [string]$ReportPath, [switch]$StopOwnedProcesses) {
    $arguments = '--install-dir "' + $InstallDirectory + '" --report-path "' + $ReportPath + '"'
    if ($StopOwnedProcesses) { $arguments += ' --stop-owned-processes' }
    $process = Start-Process -FilePath $SetupHelper -ArgumentList $arguments -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(30000)) { throw 'The native setup helper timed out.' }
        return $process.ExitCode
    } finally { $process.Dispose() }
}

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('keytao-setup-processes-' + [Guid]::NewGuid().ToString('N'))
$appRoot = Join-Path $testRoot 'KeyTao 应用'
$otherRoot = Join-Path $testRoot '其他程序'
$report = Join-Path $testRoot 'report.txt'
$processes = @()
$stopSignals = @()
New-Item -ItemType Directory -Path $appRoot, $otherRoot -Force | Out-Null

# Use real isolated OS processes throughout; no registry access or discovery
# overrides are needed because only the supplied KeyTao directory is owned.

function Wait-FixtureReady([string]$Path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not [IO.File]::Exists($Path)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw "Fixture did not start: $Path" }
        Start-Sleep -Milliseconds 25
    }
}

function Start-Fixture([string]$Executable, [string]$Label, [string[]]$ExtraArguments = @()) {
    $ready = Join-Path $testRoot ($Label + '.ready')
    $stop = Join-Path $testRoot ($Label + '.stop')
    $arguments = @($ready, $stop, $resource) + $ExtraArguments
    $quoted = @($arguments | ForEach-Object { '"' + $_ + '"' })
    $process = Start-Process -FilePath $Executable -ArgumentList $quoted -WindowStyle Hidden -PassThru
    $null = $process.Handle
    $script:processes += $process
    $script:stopSignals += $stop
    Wait-FixtureReady $ready
    $process
}

try {
    $fixture = Join-Path $testRoot 'fixture.exe'
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
public static class KeyTaoProcessFixture {
    private static string Quote(string value) { return "\"" + value + "\""; }
    public static void Main(string[] args) {
        using (FileStream resource = File.Open(args[2], FileMode.Open, FileAccess.Read, FileShare.Read)) {
            if (args.Length == 6) {
                ProcessStartInfo child = new ProcessStartInfo(args[3]);
                child.Arguments = Quote(args[4]) + " " + Quote(args[5]) + " " + Quote(args[2]);
                child.UseShellExecute = false;
                child.CreateNoWindow = true;
                child.WindowStyle = ProcessWindowStyle.Hidden;
                Process.Start(child).Dispose();
            }
            File.WriteAllText(args[0], Process.GetCurrentProcess().Id.ToString());
            DateTime deadline = DateTime.UtcNow.AddSeconds(180);
            while (!File.Exists(args[1]) && DateTime.UtcNow < deadline) Thread.Sleep(25);
        }
    }
}
'@ -OutputAssembly $fixture -OutputType ConsoleApplication

    # Every fixture also holds the old input-method DLL resource. That fact
    # must neither authorize termination nor prevent the final clear result.
    $resource = Join-Path $appRoot 'keytao_windows_ime.dll'
    [IO.File]::WriteAllText($resource, 'old input-method runtime fixture')
    foreach ($name in @('keytao-app.exe', 'keytao.exe')) {
        Copy-Item -LiteralPath $fixture -Destination (Join-Path $appRoot $name)
    }
    foreach ($name in @('WeaselServer.exe', 'WeaselDeployer.exe')) {
        Copy-Item -LiteralPath $fixture -Destination (Join-Path $appRoot $name)
    }
    foreach ($name in @('keytao-app.exe', 'keytao.exe', 'WeaselServer.exe', 'WeaselDeployer.exe', 'chrome.exe', 'explorer.exe', 'ctfmon.exe')) {
        Copy-Item -LiteralPath $fixture -Destination (Join-Path $otherRoot $name)
    }
    Assert-True ((Invoke-SetupHelper $appRoot $report) -eq 0) 'No owned running executables must pass.'

    $unrelated = @()
    foreach ($name in @('keytao-app.exe', 'keytao.exe', 'WeaselServer.exe', 'WeaselDeployer.exe', 'explorer.exe', 'ctfmon.exe')) {
        $unrelated += Start-Fixture (Join-Path $otherRoot $name) ('unrelated-' + $name)
    }
    # Other input methods remain unrelated even if somebody places their
    # executable inside KeyTao's installation directory.
    foreach ($name in @('WeaselServer.exe', 'WeaselDeployer.exe')) {
        $unrelated += Start-Fixture (Join-Path $appRoot $name) ('co-located-' + $name)
    }
    Assert-True ((Invoke-SetupHelper $appRoot $report) -eq 0) 'External KeyTao copies, Weasel programs at either path, and input-method hosts must be ignored.'

    $childReady = Join-Path $testRoot 'browser-child.ready'
    $childStop = Join-Path $testRoot 'browser-child.stop'
    $stopSignals += $childStop
    $app = Start-Fixture (Join-Path $appRoot 'keytao-app.exe') 'owned-app' @((Join-Path $otherRoot 'chrome.exe'), $childReady, $childStop)
    Wait-FixtureReady $childReady
    $browser = Get-Process -Id ([int][IO.File]::ReadAllText($childReady))
    $null = $browser.Handle
    $processes += $browser
    $unrelated += $browser
    $legacy = Start-Fixture (Join-Path $appRoot 'keytao.exe') 'owned-legacy'
    $owned = @($app, $legacy)
    Assert-True ((Invoke-SetupHelper (Join-Path $appRoot '.') $report) -eq 10) 'The read-only check must report verified standalone processes after path normalization.'
    foreach ($process in $owned) { Assert-True (-not $process.HasExited) 'Read-only enumeration must not stop any process.' }
    $text = [IO.File]::ReadAllText($report, [Text.Encoding]::Unicode)
    foreach ($process in $owned) { Assert-True ($text.Contains("PID $($process.Id)")) 'Each owned standalone process must be listed.' }
    foreach ($process in $unrelated) { Assert-True (-not $text.Contains("PID $($process.Id)")) 'Unrelated processes must not appear in the stop list.' }
    $bytes = [IO.File]::ReadAllBytes($report)
    Assert-True (-not ($bytes[0] -eq 255 -and $bytes[1] -eq 254)) 'The report must be UTF-16LE without a BOM.'
    Assert-True ($text.StartsWith('检测到') -and $text.Length -le 700) 'The report must preserve Chinese text and fit the NSIS buffer.'

    Assert-True ((Invoke-SetupHelper $appRoot $report -StopOwnedProcesses) -eq 0) 'Stopping owned standalone processes must clear the check despite running input-method hosts.'
    foreach ($process in $owned) {
        Assert-True ($process.WaitForExit(5000)) 'Every approved standalone fixture must be terminated.'
    }
    foreach ($process in $unrelated) {
        Assert-True (-not $process.HasExited) 'Weasel, external KeyTao copies, hosts, and the browser child must remain running.'
    }
    Assert-True ((Invoke-SetupHelper $appRoot $report) -eq 0) 'A rescan must ignore old DLL use by preserved programs.'
    Assert-True ((Invoke-SetupHelper (Join-Path $testRoot 'new-install') $report) -eq 0) 'An absent directory must allow first installation.'
    Assert-True ((Invoke-SetupHelper $resource $report) -eq 20) 'An invalid install directory must fail closed.'
    Assert-True ((Invoke-SetupHelper 'relative-install' $report) -eq 20) 'A relative install directory must fail closed.'
    Assert-True ((Invoke-SetupHelper $appRoot (Join-Path $testRoot 'missing\report.txt')) -eq 20) 'A report write failure must fail closed.'
    Write-Host 'PASS: Native helper CLI, KeyTao-only shutdown, preserved Weasel at all paths, external KeyTao copies, hosts/browser child, UTF-16 report, and error exits.'
} finally {
    # Ask only our unique fixture processes to exit; never use name-based kill.
    foreach ($signal in $stopSignals) { [IO.File]::WriteAllText($signal, 'stop') }
    foreach ($process in $processes) {
        if (-not $process.HasExited) { $null = $process.WaitForExit(5000) }
        $process.Dispose()
    }
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Leaf $resolvedRoot) -notmatch '^keytao-setup-processes-[0-9a-f]{32}$') {
        throw 'Refusing to remove an unexpected fixture directory.'
    }
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
}
