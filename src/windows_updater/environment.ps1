# Embedded helper code. Build the GUI child's environment without changing the helper's own.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class AltTabioUpdateEnvironment {
    [DllImport("userenv.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CreateEnvironmentBlock(out IntPtr block, IntPtr token, [MarshalAs(UnmanagedType.Bool)] bool inherit);
    [DllImport("userenv.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyEnvironmentBlock(IntPtr block);

    public static Dictionary<string, string> Read(IntPtr token) {
        IntPtr block;
        if (!CreateEnvironmentBlock(out block, token, false))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            var values = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            IntPtr cursor = block;
            while (Marshal.ReadInt16(cursor) != 0) {
                string entry = Marshal.PtrToStringUni(cursor);
                int separator = entry.IndexOf('=');
                if (separator > 0)
                    values[entry.Substring(0, separator)] = entry.Substring(separator + 1);
                cursor = IntPtr.Add(cursor, (entry.Length + 1) * 2);
            }
            return values;
        } finally {
            if (!DestroyEnvironmentBlock(block))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
    }
}
'@

function Start-UpdatedApp([string] $FilePath) {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $FilePath
    $start.WorkingDirectory = [IO.Path]::GetDirectoryName($FilePath)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.EnvironmentVariables.Clear()
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    try {
        $environment = [AltTabioUpdateEnvironment]::Read($identity.Token)
        foreach ($key in $environment.Keys) { $start.EnvironmentVariables[$key] = $environment[$key] }
    } finally { $identity.Dispose() }
    # No WindowStyle override: a GUI app must retain control of its first ShowWindow call.
    $process = [Diagnostics.Process]::Start($start)
    if ($null -eq $process) { throw 'Could not start AltTabio after the update.' }
    $process.Dispose()
}
