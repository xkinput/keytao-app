param(
    [Parameter(Mandatory = $true)]
    [string]$DllPath,
    [ValidateRange(5, 120)]
    [int]$TimeoutSeconds = 20,
    [Parameter(DontShow = $true)]
    [switch]$Probe
)

$ErrorActionPreference = "Stop"
$DllPath = (Resolve-Path -LiteralPath $DllPath).ProviderPath

if ($Probe) {
    # Compile before restricting the native search path, so compiler/CLR setup
    # is outside the behavior under test. This process never activates the TIP.
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

public static class KeyTaoImeLoadProbe {
    [DllImport("kernel32.dll")]
    private static extern uint SetErrorMode(uint mode);
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetDefaultDllDirectories(uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
    private static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandleW(string name);
    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool FreeLibrary(IntPtr module);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetClassObject(ref Guid clsid, ref Guid iid, out IntPtr factory);

    private static void AssertRimeNotLoaded() {
        foreach (string name in new[] { "rime.dll", "rime-arm64.dll" }) {
            if (GetModuleHandleW(name) != IntPtr.Zero) {
                throw new InvalidOperationException(name + " loaded before TIP activation");
            }
        }
    }

    public static void Run(string path) {
        // Suppress Windows error dialogs when testing a known-bad DLL. A native
        // delay-load SEH exception must terminate only this disposable process.
        SetErrorMode(0x0001 | 0x0002 | 0x8000);
        if (!SetDefaultDllDirectories(0x0800)) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "SetDefaultDllDirectories");
        }
        AssertRimeNotLoaded();
        // Resolve the TIP's immediate dependencies beside it; subsequent bare
        // delay-load requests must not find rime through PATH, cwd or the host.
        IntPtr module = LoadLibraryExW(path, IntPtr.Zero, 0x0100 | 0x0800);
        if (module == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "LoadLibraryExW");
        }
        IntPtr factory = IntPtr.Zero;
        try {
            IntPtr address = GetProcAddress(module, "DllGetClassObject");
            if (address == IntPtr.Zero) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "DllGetClassObject export");
            }
            GetClassObject getClassObject = (GetClassObject)Marshal.GetDelegateForFunctionPointer(
                address, typeof(GetClassObject));
            Guid clsid = new Guid("4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D");
            Guid iid = new Guid("00000001-0000-0000-C000-000000000046"); // IClassFactory
            int hr = getClassObject(ref clsid, ref iid, out factory);
            Marshal.ThrowExceptionForHR(hr);
            if (factory == IntPtr.Zero) {
                throw new InvalidOperationException("DllGetClassObject returned no factory");
            }
            Console.WriteLine("Class factory acquired; waiting for background log initialization.");
            Console.Out.Flush();
            Stopwatch elapsed = Stopwatch.StartNew();
            do {
                AssertRimeNotLoaded();
                Thread.Sleep(100);
            } while (elapsed.ElapsedMilliseconds < 2000);
            AssertRimeNotLoaded();
            Console.WriteLine("PASS: class factory and background logging leave librime unloaded.");
        } finally {
            if (factory != IntPtr.Zero) {
                Marshal.Release(factory);
            }
            FreeLibrary(module);
        }
    }
}
'@
    try {
        [KeyTaoImeLoadProbe]::Run($DllPath)
        exit 0
    } catch {
        [Console]::Error.WriteLine($_.Exception.ToString())
        exit 1
    }
}

# Select a host of the same architecture as the DLL without installing or
# registering anything. Release CI builds on x64 and also ships an x86 TIP.
$stream = [System.IO.File]::OpenRead($DllPath)
$reader = [System.IO.BinaryReader]::new($stream)
try {
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Not a PE file: $DllPath" }
    $stream.Position = 0x3C
    $peOffset = $reader.ReadInt32()
    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) { throw "Invalid PE header: $DllPath" }
    $machine = $reader.ReadUInt16()
} finally {
    $reader.Dispose()
    $stream.Dispose()
}
$osArchitecture = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
if ($machine -eq 0x014C) {
    $systemDirectory = if ([Environment]::Is64BitOperatingSystem) { "SysWOW64" } else { "System32" }
    $architecture = "x86"
} elseif ($machine -eq 0x8664 -and $osArchitecture -eq "AMD64") {
    $systemDirectory = if ([Environment]::Is64BitProcess) { "System32" } else { "Sysnative" }
    $architecture = "x64"
} else {
    throw ("No matching smoke-test host for PE machine 0x{0:X4} on {1}." -f $machine, $osArchitecture)
}
$powershell = Join-Path $env:SystemRoot "$systemDirectory\WindowsPowerShell\v1.0\powershell.exe"
if (-not (Test-Path -LiteralPath $powershell -PathType Leaf)) {
    throw "Missing $architecture PowerShell host: $powershell"
}

$temporaryBase = Join-Path ([System.IO.Path]::GetTempPath()) ("keytao-ime-load-" + [Guid]::NewGuid().ToString("N"))
$stdoutPath = "$temporaryBase.stdout"
$stderrPath = "$temporaryBase.stderr"
$process = $null
try {
    # Windows paths cannot contain quotes and both arguments end in filenames,
    # so quoting them preserves spaces without a command interpreter.
    $arguments = @(
        "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", ('"' + $PSCommandPath + '"'), "-DllPath", ('"' + $DllPath + '"'), "-Probe"
    )
    $process = Start-Process -FilePath $powershell -ArgumentList $arguments -WindowStyle Hidden `
        -WorkingDirectory $env:SystemRoot -PassThru -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath
    # Retain the native handle before it exits; Windows PowerShell otherwise
    # may return a null ExitCode for a process started without -Wait.
    $null = $process.Handle
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        throw "Windows IME load test timed out after $TimeoutSeconds seconds: $DllPath"
    }
    # Complete redirected stream handling after the bounded wait.
    $process.WaitForExit()
    $exitCode = $process.ExitCode
    $stdout = if (Test-Path -LiteralPath $stdoutPath) { Get-Content -LiteralPath $stdoutPath -Raw } else { "" }
    $stderr = if (Test-Path -LiteralPath $stderrPath) { Get-Content -LiteralPath $stderrPath -Raw } else { "" }
    if ($exitCode -ne 0) {
        $nativeCode = [BitConverter]::ToUInt32([BitConverter]::GetBytes([int]$exitCode), 0)
        throw ("Windows IME load test failed ({0}, exit 0x{1:X8}): {2}`n{3}{4}" -f `
            $architecture, $nativeCode, $DllPath, $stdout, $stderr)
    }
    Write-Host "Windows IME load test passed ($architecture): $DllPath"
    if ($stdout) { Write-Host $stdout.TrimEnd() }
} finally {
    if ($process) {
        if (-not $process.HasExited) {
            $process.Kill()
            $process.WaitForExit(5000) | Out-Null
        }
        $process.Dispose()
    }
    Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
}
