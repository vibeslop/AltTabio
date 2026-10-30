# Embedded in the executable. All paths and the expected hash are supplied as literals.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$previous = Join-Path $Work 'previous.exe'
$staged = Join-Path $Work 'AltTabio.exe'
$replaced = $false
$parentExited = $false
function Restore-Access([string] $Sddl) {
    $access = [Security.AccessControl.AccessControlSections]::Access
    $permissions = New-Object Security.AccessControl.FileSecurity
    $permissions.SetSecurityDescriptorSddlForm($Sddl, $access)
    # Apply only the DACL, without requesting owner or audit privileges. Each call needs a fresh
    # object because SetAccessControl clears its modification flags after persisting them.
    [IO.File]::SetAccessControl($Target, $permissions)
}
try {
    # Acquire the old process before acknowledging readiness, so a reused PID cannot be waited on.
    $parent = [Diagnostics.Process]::GetProcessById($ParentId)
    $null = $parent.Handle
    [Console]::Out.WriteLine('READY')
    [Console]::Out.Flush()
    # The app waits out an open switcher or modal dialog before quitting. Hold the original
    # process handle until then rather than timing out and leaving it without a relauncher.
    $parent.WaitForExit()
    $parentExited = $true
    $parent.Dispose()
    if ((Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash -cne $Hash) {
        throw 'The staged executable failed its SHA-256 check.'
    }
    # ReplaceFile can recalculate inherited ACEs against the staging folder's restrictive ACL.
    # Restore the installation's policy before launching, including after a rollback.
    $access = [Security.AccessControl.AccessControlSections]::Access
    $permissions = (Get-Acl -LiteralPath $Target).GetSecurityDescriptorSddlForm($access)
    [IO.File]::Replace($staged, $Target, $previous)
    $replaced = $true
    try {
        Restore-Access $permissions
        Start-Process -FilePath $Target -WorkingDirectory ([IO.Path]::GetDirectoryName($Target)) -WindowStyle Hidden -ErrorAction Stop
    } catch {
        [IO.File]::Replace($previous, $Target, (Join-Path $Work 'failed.exe'))
        Restore-Access $permissions
        $replaced = $false
        throw
    }
    Remove-Item -LiteralPath $Work -Recurse -Force
} catch {
    # A failure after the old process exited must leave it usable and start it again.
    $message = $_.Exception.Message
    try { [IO.File]::WriteAllText((Join-Path $Work 'error.txt'), $message) }
    catch { [Console]::Error.WriteLine("Could not record the update failure: $($_.Exception.Message)") }
    if (-not $replaced -and $parentExited -and (Test-Path -LiteralPath $Target)) {
        try { Start-Process -FilePath $Target -WorkingDirectory ([IO.Path]::GetDirectoryName($Target)) -WindowStyle Hidden }
        catch { [Console]::Error.WriteLine("Could not restart the previous AltTabio: $($_.Exception.Message)") }
    }
    [Console]::Error.WriteLine($message)
    exit 1
}
