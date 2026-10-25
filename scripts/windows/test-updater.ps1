# Run with Windows PowerShell 5.1 after cargo build --release --quiet.
# Uses local archives and inert launch mocks. Never downloads or launches an updated app.
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$powershell = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell/v1.0/powershell.exe'
$download = Get-Content -LiteralPath (Join-Path $repo 'src/windows_updater/download.ps1') -Raw
$relaunch = Get-Content -LiteralPath (Join-Path $repo 'src/windows_updater/relaunch.ps1') -Raw
$artifact = Join-Path $repo 'target/release/AltTabio.exe'
if (-not (Test-Path -LiteralPath $artifact)) { throw 'Build target/release/AltTabio.exe first.' }
$version = [Diagnostics.FileVersionInfo]::GetVersionInfo($artifact).ProductVersion
$root = Join-Path ([IO.Path]::GetTempPath()) ('AltTabio-updater-test-' + [Guid]::NewGuid())
$null = New-Item -ItemType Directory -Path $root

function Literal([string] $Text) { return "'" + $Text.Replace("'", "''") + "'" }
function Assert([bool] $Condition, [string] $Message) { if (-not $Condition) { throw $Message } }
function Run-Script([string] $Script, [Diagnostics.Process] $Parent = $null) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Script))
    # Read the child streams directly, including deliberate failures, without PowerShell's
    # parent/child CLIXML conversion changing the diagnostics.
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $powershell
    $info.Arguments = '-NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' + $encoded
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    try {
        $output = ''
        if ($Parent) {
            # The helper must acquire its parent and say READY before the test lets it exit.
            $output = $process.StandardOutput.ReadLine() + "`n"
            if (-not $Parent.HasExited) { $Parent.Kill() }
        }
        $output += $process.StandardOutput.ReadToEnd()
        $errorOutput = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        return @{ Code = $process.ExitCode; Output = ($output + $errorOutput) }
    } finally { $process.Dispose() }
}

function New-Archive([string] $Name, [bool] $Duplicate = $false) {
    $path = Join-Path $root $Name
    $zip = [IO.Compression.ZipFile]::Open($path, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $null = [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $artifact, 'AltTabio.exe')
        $escape = $zip.CreateEntry('../escape.exe')
        $stream = $escape.Open()
        try { $stream.WriteByte(42) } finally { $stream.Dispose() }
        if ($Duplicate) { $null = $zip.CreateEntry('AltTabio.exe') }
    } finally { $zip.Dispose() }
    return $path
}

function Download-Case([string] $Name, [string] $Archive, [string] $Digest, [string] $ExpectedVersion = $version) {
    $work = Join-Path $root $Name
    $null = New-Item -ItemType Directory -Path $work
    $release = @{
        tag_name = "v$ExpectedVersion"; draft = $false; prerelease = $false
        assets = @(@{
            name = "AltTabio-$ExpectedVersion-windows-x64.zip"
            browser_download_url = "https://github.com/vibeslop/AltTabio/releases/download/v$ExpectedVersion/AltTabio-$ExpectedVersion-windows-x64.zip"
            digest = $Digest; size = (Get-Item -LiteralPath $Archive).Length
        })
    } | ConvertTo-Json -Depth 4 -Compress
    $preamble = @'
function Mock-Curl {
    $global:LASTEXITCODE = 0
    $index = [Array]::IndexOf($args, '--output')
    if ($index -ge 0) { Copy-Item -LiteralPath $Fixture -Destination $args[$index + 1] }
    else { $ReleaseJson }
}
$Curl = 'Mock-Curl'
$Mode = 'stage'
'@
    $result = Run-Script ("`$Fixture = $(Literal $Archive)`n`$ReleaseJson = $(Literal $release)`n`$Version = $(Literal $ExpectedVersion)`n`$Work = $(Literal $work)`n" + $preamble + "`n" + $download)
    return @{ Result = $result; Work = $work }
}

function Relaunch-Case([string] $Name, [bool] $FailLaunch, [bool] $BadHash = $false) {
    $folder = Join-Path $root $Name
    $work = Join-Path $folder 'stage'
    $null = New-Item -ItemType Directory -Path $work -Force
    $target = Join-Path $folder "user's [copy] `$test.exe"
    $staged = Join-Path $work 'AltTabio.exe'
    [IO.File]::WriteAllText($target, 'old executable')
    [IO.File]::WriteAllText($staged, 'new executable')
    [IO.File]::WriteAllText((Join-Path $folder 'AltTabio.ini'), 'keep these settings')
    $hash = (Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash
    if ($BadHash) { $hash = '0' * 64 }
    $sleep = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('Start-Sleep -Seconds 300'))
    $parent = Start-Process -FilePath $powershell -WindowStyle Hidden -ArgumentList @('-NoProfile', '-EncodedCommand', $sleep) -PassThru
    $launchLog = Join-Path $folder 'launches.txt'
    $mock = @'
$script:LaunchCount = 0
function Start-Process {
    param($FilePath, $WorkingDirectory, $WindowStyle, $ErrorAction)
    $script:LaunchCount++
    if ($FailLaunch -and $script:LaunchCount -eq 1) { throw 'Simulated launch failure' }
    [IO.File]::AppendAllText($LaunchLog, [IO.File]::ReadAllText($FilePath) + "`n")
}
'@
    $script = "`$Work = $(Literal $work)`n`$Target = $(Literal $target)`n`$Hash = '$hash'`n`$ParentId = $($parent.Id)`n`$LaunchLog = $(Literal $launchLog)`n`$FailLaunch = `$$FailLaunch`n" + $mock + "`n" + $relaunch
    $originalAcl = (Get-Acl -LiteralPath $target).Sddl
    try { $result = Run-Script $script $parent }
    finally {
        if (-not $parent.HasExited) { $parent.Kill() }
        $parent.WaitForExit()
        $parent.Dispose()
    }
    Assert ($result.Output.Contains('READY')) "$Name did not acknowledge readiness: $($result.Output)"
    Assert ([IO.File]::ReadAllText((Join-Path $folder 'AltTabio.ini')) -ceq 'keep these settings') "$Name changed settings"
    $expected = if ($FailLaunch -or $BadHash) { 'old executable' } else { 'new executable' }
    Assert ([IO.File]::ReadAllText($target) -ceq $expected) "$Name left the wrong executable: $($result.Output)"
    Assert ((Get-Acl -LiteralPath $target).Sddl -ceq $originalAcl) "$Name changed executable permissions"
    Assert ([IO.File]::ReadAllText($launchLog).Trim() -ceq $expected) "$Name relaunched the wrong executable"
    if ($FailLaunch -or $BadHash) {
        Assert ($result.Code -eq 1) "$Name should report a failure"
        Assert (Test-Path -LiteralPath (Join-Path $work 'error.txt')) "$Name lost the failure diagnostic"
    } else {
        Assert ($result.Code -eq 0) "$Name failed: $($result.Output)"
        Assert (-not (Test-Path -LiteralPath $work)) "$Name left its work directory"
    }
}

try {
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    foreach ($text in @($download, $relaunch)) {
        $tokens = $null; $errors = $null
        $null = [Management.Automation.Language.Parser]::ParseInput($text, [ref] $tokens, [ref] $errors)
        Assert ($errors.Count -eq 0) "PowerShell syntax errors: $errors"
    }
    $archive = New-Archive 'release.zip'
    $digest = 'sha256:' + (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    $valid = Download-Case 'verified' $archive $digest
    Assert ($valid.Result.Code -eq 0) "Valid archive failed: $($valid.Result.Output)"
    Assert ((Get-FileHash -LiteralPath (Join-Path $valid.Work 'AltTabio.exe')).Hash -ceq (Get-FileHash -LiteralPath $artifact).Hash) 'Verified executable differs'
    Assert (-not (Test-Path -LiteralPath (Join-Path $root 'escape.exe'))) 'Archive path traversal escaped staging'
    foreach ($badDigest in @('', ('sha256:' + ('0' * 64)))) {
        $bad = Download-Case ([Guid]::NewGuid().ToString()) $archive $badDigest
        Assert ($bad.Result.Code -eq 1) 'Unverified archive was accepted'
        Assert (-not (Test-Path -LiteralPath (Join-Path $bad.Work 'AltTabio.exe'))) 'Unverified archive was extracted'
    }
    $wrongVersion = Download-Case 'wrong-version' $archive $digest '999.0.0'
    Assert ($wrongVersion.Result.Code -eq 1) 'Executable version mismatch was accepted'
    $duplicate = New-Archive 'duplicate.zip' $true
    $duplicateDigest = 'sha256:' + (Get-FileHash -LiteralPath $duplicate -Algorithm SHA256).Hash.ToLowerInvariant()
    $bad = Download-Case 'duplicate' $duplicate $duplicateDigest
    Assert ($bad.Result.Code -eq 1) 'Duplicate executable was accepted'
    Relaunch-Case 'successful-replacement' $false
    Relaunch-Case 'failed-launch-rollback' $true
    Relaunch-Case 'tampered-stage' $false $true
    Write-Output 'Windows updater verification, replacement, and rollback checks passed.'
} finally {
    # Resolve and constrain the only recursive cleanup to this run's unique test directory.
    $resolved = [IO.Path]::GetFullPath($root)
    $temporary = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notlike 'AltTabio-updater-test-*') {
        throw 'Refusing cleanup outside the updater test directory.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
