# Windows may expose a shortcut target using an 8.3 alias. Verify the file itself,
# not its spelling or contents: two distinct executables with identical bytes fail.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public static class MailBeaconFileIdentity {
    [StructLayout(LayoutKind.Sequential)]
    private struct FileInformation {
        public uint Attributes, CreationLow, CreationHigh, AccessLow, AccessHigh;
        public uint WriteLow, WriteHigh, VolumeSerialNumber, SizeHigh, SizeLow;
        public uint LinkCount, FileIndexHigh, FileIndexLow;
    }
    public sealed class Identity {
        public string FinalPath { get; set; }
        public uint VolumeSerialNumber { get; set; }
        public uint FileIndexHigh { get; set; }
        public uint FileIndexLow { get; set; }
    }
    public sealed class Comparison {
        public Identity Expected { get; set; }
        public Identity Actual { get; set; }
        public bool SameFile { get; set; }
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle file, out FileInformation info);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandleW(SafeFileHandle file, StringBuilder path, uint count, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern uint GetShortPathNameW(string path, StringBuilder shortPath, uint count);

    private static Identity Read(SafeFileHandle file) {
        FileInformation info;
        if (!GetFileInformationByHandle(file, out info)) throw new Win32Exception(Marshal.GetLastWin32Error());
        var path = new StringBuilder(32768);
        uint length = GetFinalPathNameByHandleW(file, path, (uint)path.Capacity, 0);
        if (length == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
        if (length >= path.Capacity) throw new IOException("Resolved file path exceeds the Windows path limit.");
        return new Identity { FinalPath = path.ToString(), VolumeSerialNumber = info.VolumeSerialNumber,
            FileIndexHigh = info.FileIndexHigh, FileIndexLow = info.FileIndexLow };
    }
    public static Comparison Compare(string expected, string actual) {
        // Keep both handles open throughout the comparison so file IDs cannot be
        // recycled between reads. Missing/unreadable targets are always an error.
        using (var left = File.Open(expected, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
        using (var right = File.Open(actual, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
            var a = Read(left.SafeFileHandle);
            var b = Read(right.SafeFileHandle);
            return new Comparison { Expected = a, Actual = b,
                SameFile = a.VolumeSerialNumber == b.VolumeSerialNumber &&
                    a.FileIndexHigh == b.FileIndexHigh && a.FileIndexLow == b.FileIndexLow };
        }
    }
    public static string ShortPath(string path) {
        var shortPath = new StringBuilder(32768);
        uint length = GetShortPathNameW(path, shortPath, (uint)shortPath.Capacity);
        if (length == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
        if (length >= shortPath.Capacity) throw new IOException("Short path exceeds the Windows path limit.");
        return shortPath.ToString();
    }
}
'@

function Test-WindowsFileIdentity {
  # Real files on the disposable runner exercise the native helper before it is
  # trusted to validate an installer. This test never needs application data.
  $directory = Join-Path $env:RUNNER_TEMP ('MailBeacon 路径验证 ' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $directory | Out-Null
  try {
    $expected = Join-Path $directory '邮件 应用.exe'
    $different = Join-Path $directory 'different.exe'
    $alias = Join-Path $directory 'alias.exe'
    [IO.File]::WriteAllText($expected, 'same contents, different file identities')
    [IO.File]::WriteAllText($different, 'same contents, different file identities')
    New-Item -ItemType HardLink -Path $alias -Target $expected | Out-Null
    if (-not [MailBeaconFileIdentity]::Compare($expected, $expected).SameFile) { throw 'Same file was rejected.' }
    if (-not [MailBeaconFileIdentity]::Compare($expected, $alias).SameFile) { throw 'Hard-link alias was rejected.' }
    $short = [MailBeaconFileIdentity]::ShortPath($expected)
    if (-not [MailBeaconFileIdentity]::Compare($expected, $short).SameFile) { throw 'Short-path alias was rejected.' }
    if ([MailBeaconFileIdentity]::Compare($expected, $different).SameFile) { throw 'Different file was accepted.' }
    $missingRejected = $false
    try { [MailBeaconFileIdentity]::Compare($expected, (Join-Path $directory 'missing.exe')) | Out-Null }
    catch { $missingRejected = $true }
    if (-not $missingRejected) { throw 'Missing file was accepted.' }
    return [ordered]@{ same = $true; hardLink = $true; shortPath = $true; differentRejected = $true; missingRejected = $true; shortAliasAvailable = ($short -ne $expected) }
  } finally { Remove-Item -LiteralPath $directory -Recurse -Force }
}
