param(
    [string]$InstallDir,
    [string]$ReportPath,
    [switch]$StopOwnedProcesses
)

$ErrorActionPreference = 'Stop'

function Initialize-KeyTaoProcessControl {
    if ('KeyTao.Setup.OwnedProcesses' -as [type]) { return }
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace KeyTao.Setup {
    public sealed class OwnedProcess {
        public uint ProcessId;
        public string Name;
        public string ImagePath;
        public uint SessionId;
        public long CreationTime;
    }

    public static class OwnedProcesses {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct ProcessEntry {
            public uint Size, Usage, ProcessId;
            public UIntPtr DefaultHeap;
            public uint ModuleId, Threads, ParentId;
            public int Priority;
            public uint Flags;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)]
            public string Name;
        }
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, StringBuilder name, ref uint length);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool GetProcessTimes(IntPtr process, out long created, out long exited, out long kernel, out long user);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool ProcessIdToSessionId(uint pid, out uint session);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool TerminateProcess(IntPtr process, uint exitCode);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll")]
        private static extern bool CloseHandle(IntPtr handle);

        private static bool IsOwnedName(string name) {
            return String.Equals(name, "keytao-app.exe", StringComparison.OrdinalIgnoreCase)
                || String.Equals(name, "keytao.exe", StringComparison.OrdinalIgnoreCase);
        }
        private static string ImagePath(IntPtr process) {
            uint length = 32768;
            StringBuilder path = new StringBuilder((int)length);
            if (!QueryFullProcessImageNameW(process, 0, path, ref length))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to verify process executable path");
            return Path.GetFullPath(path.ToString());
        }
        private static long CreationTime(IntPtr process) {
            long created, exited, kernel, user;
            if (!GetProcessTimes(process, out created, out exited, out kernel, out user))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to verify process creation time");
            return created;
        }
        private static bool Exited(IntPtr process) {
            uint result = WaitForSingleObject(process, 0);
            if (result == 0xFFFFFFFF)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to inspect process state");
            return result == 0;
        }

        public static OwnedProcess[] Find(string[] allowedPaths) {
            HashSet<string> allowed = new HashSet<string>(allowedPaths, StringComparer.OrdinalIgnoreCase);
            List<OwnedProcess> found = new List<OwnedProcess>();
            IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
            if (snapshot == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
            try {
                ProcessEntry entry = new ProcessEntry();
                entry.Size = (uint)Marshal.SizeOf(typeof(ProcessEntry));
                bool more = Process32FirstW(snapshot, ref entry);
                while (more) {
                    if (IsOwnedName(entry.Name)) {
                        // Query only: a matching name outside this KeyTao
                        // installation does not establish process ownership.
                        IntPtr process = OpenProcess(0x1000 | 0x100000, false, entry.ProcessId);
                        if (process == IntPtr.Zero) {
                            int error = Marshal.GetLastWin32Error();
                            if (error != 87)
                                throw new Win32Exception(error, "Unable to verify " + entry.Name + " (PID " + entry.ProcessId + ")");
                        } else {
                            try {
                                if (!Exited(process)) {
                                    string path = ImagePath(process);
                                    if (allowed.Contains(path)) {
                                        uint session;
                                        if (!ProcessIdToSessionId(entry.ProcessId, out session)) {
                                            int error = Marshal.GetLastWin32Error();
                                            if (Exited(process)) { more = Process32NextW(snapshot, ref entry); continue; }
                                            throw new Win32Exception(error, "Unable to inspect process session");
                                        }
                                        found.Add(new OwnedProcess {
                                            ProcessId = entry.ProcessId, Name = entry.Name,
                                            ImagePath = path, SessionId = session,
                                            CreationTime = CreationTime(process)
                                        });
                                    }
                                }
                            } catch (Win32Exception) {
                                if (!Exited(process)) throw;
                            } finally { CloseHandle(process); }
                        }
                    }
                    more = Process32NextW(snapshot, ref entry);
                }
                int lastError = Marshal.GetLastWin32Error();
                if (lastError != 18)
                    throw new Win32Exception(lastError, "Unable to enumerate running processes");
                return found.ToArray();
            } finally { CloseHandle(snapshot); }
        }

        public static void Stop(OwnedProcess target) {
            // A handle binds termination to one kernel process object. Check
            // both image path and creation time after opening so PID reuse
            // cannot convert an old scan result into permission to kill a host.
            IntPtr process = OpenProcess(0x1000 | 0x100000 | 1, false, target.ProcessId);
            if (process == IntPtr.Zero) {
                int error = Marshal.GetLastWin32Error();
                if (error == 87) return;
                throw new Win32Exception(error, "Unable to stop " + target.Name + " (PID " + target.ProcessId + ")");
            }
            try {
                if (Exited(process)) return;
                if (CreationTime(process) != target.CreationTime
                    || !String.Equals(ImagePath(process), target.ImagePath, StringComparison.OrdinalIgnoreCase)) return;
                if (!IsOwnedName(Path.GetFileName(target.ImagePath)))
                    throw new InvalidOperationException("Refusing to stop a non-owned executable.");
                if (!TerminateProcess(process, 0)) {
                    int error = Marshal.GetLastWin32Error();
                    if (Exited(process)) return;
                    throw new Win32Exception(error, "Unable to stop " + target.Name);
                }
                uint wait = WaitForSingleObject(process, 5000);
                if (wait != 0) throw new InvalidOperationException("Timed out stopping " + target.Name + ".");
            } finally { CloseHandle(process); }
        }
    }
}
'@
}

function Get-KeyTaoOwnedExecutablePaths([string]$AppDirectory) {
    if ([string]::IsNullOrWhiteSpace($AppDirectory) -or -not [IO.Path]::IsPathRooted($AppDirectory)) {
        throw 'InstallDir must be an absolute directory path.'
    }
    $appRoot = [IO.Path]::GetFullPath($AppDirectory)
    if ((Test-Path -LiteralPath $appRoot -ErrorAction Stop) -and
        -not (Test-Path -LiteralPath $appRoot -PathType Container -ErrorAction Stop)) {
        throw "Expected an installation directory: $appRoot"
    }
    # KeyTao is independent of other input methods. Only its two executable
    # names at this exact installation location belong to this installer.
    Join-Path $appRoot 'keytao-app.exe'
    Join-Path $appRoot 'keytao.exe'
}

function Write-KeyTaoPreflightReport([string]$Path, [string]$Summary, [string]$Details) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'ReportPath is required.' }
    if ($Summary.Length -gt 700) { $Summary = $Summary.Substring(0, 697) + '...' }
    $encoding = New-Object Text.UnicodeEncoding($false, $false)
    [IO.File]::WriteAllText($Path, $Summary, $encoding)
    [IO.File]::WriteAllText(($Path + '.log'), $Details, $encoding)
}

function Invoke-KeyTaoRunningProcessCheck([string]$AppDirectory, [string]$OutputPath, [switch]$StopOwnedProcesses) {
    try {
        if (-not [Environment]::Is64BitProcess) { throw 'This check requires native 64-bit PowerShell.' }
        $paths = @(Get-KeyTaoOwnedExecutablePaths $AppDirectory | Sort-Object -Unique)
        Initialize-KeyTaoProcessControl
        $applications = @([KeyTao.Setup.OwnedProcesses]::Find([string[]]$paths))
        $stopped = @()
        if ($StopOwnedProcesses) {
            foreach ($application in $applications) {
                [KeyTao.Setup.OwnedProcesses]::Stop($application)
                $stopped += "$($application.Name) (PID $($application.ProcessId))"
            }
            $applications = @([KeyTao.Setup.OwnedProcesses]::Find([string[]]$paths))
        }
        $details = "允许停止的可执行文件：`r`n" + ($paths -join "`r`n")
        if ($stopped.Count) { $details += "`r`n`r`n已处理：`r`n" + ($stopped -join "`r`n") }
        if ($applications.Count -eq 0) {
            Write-KeyTaoPreflightReport $OutputPath 'KeyTao 自身进程已就绪，可以继续安装。其他输入法、浏览器、资源管理器及其他应用会保留。' $details
            return 0
        }
        $lines = @($applications | Sort-Object SessionId, ProcessId | ForEach-Object {
            '{0}（PID {1}，会话 {2}）' -f $_.Name, $_.ProcessId, $_.SessionId
        })
        $heading = "检测到以下 KeyTao 自身进程：`r`n"
        $ending = "`r`n继续安装时将自动强制停止这些 KeyTao 进程。其他输入法、浏览器、资源管理器及其他应用不会被关闭。"
        if ($StopOwnedProcesses) { $ending = "`r`n以上 KeyTao 进程仍在运行，请重试。其他输入法、浏览器、资源管理器及其他应用不会被关闭。" }
        $visible = @()
        foreach ($line in $lines) {
            if (($heading + (($visible + $line) -join "`r`n") + $ending).Length -gt 630) { break }
            $visible += $line
        }
        if ($visible.Count -lt $lines.Count) { $visible += "另有 $($lines.Count - $visible.Count) 个自身进程，详见检测日志。" }
        $details += "`r`n`r`n仍在运行：`r`n" + (@($applications | ForEach-Object { "$($_.Name) PID=$($_.ProcessId) Session=$($_.SessionId) Path=$($_.ImagePath)" }) -join "`r`n")
        Write-KeyTaoPreflightReport $OutputPath ($heading + ($visible -join "`r`n") + $ending) $details
        return 10
    } catch {
        $failure = $_.Exception.Message
        try {
            Write-KeyTaoPreflightReport $OutputPath "无法检查或停止 KeyTao 自身进程，安装尚未开始。请重试；若仍失败，请手动退出 KeyTao 后再安装。`r`n$failure" $failure
        } catch { [Console]::Error.WriteLine($_.Exception.Message) }
        [Console]::Error.WriteLine($failure)
        return 20
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    exit (Invoke-KeyTaoRunningProcessCheck $InstallDir $ReportPath -StopOwnedProcesses:$StopOwnedProcesses)
}
