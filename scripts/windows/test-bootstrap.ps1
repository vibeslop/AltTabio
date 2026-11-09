# Checks docs/install.ps1 without installing anything: its key, its Minisign verifier against the
# fixtures the app's own tests use, its choice of release, and what it will and will not extract.
# Runs in Windows PowerShell 5.1 and in PowerShell 7. With -Live it also downloads the newest
# release and verifies it with the pinned key.
param([switch]$Live)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script = Join-Path $root 'docs/install.ps1'
$fixtures = Join-Path $root 'tests/fixtures/update-signature'
$failed = 0

function Check([string]$Name, [bool]$Passed) {
    if ($Passed) { Write-Host "ok    $Name" }
    else { Write-Host "FAIL  $Name" -ForegroundColor Red; $script:failed++ }
}

# Load the installer's definitions without running it: every top-level statement must be a
# function, an assignment, or the one call on the last line.
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($script, [ref]$tokens, [ref]$errors)
Check 'the installer parses' (-not $errors)
$calls = @()
foreach ($statement in $ast.EndBlock.Statements) {
    if ($statement -is [System.Management.Automation.Language.FunctionDefinitionAst] -or
        $statement -is [System.Management.Automation.Language.AssignmentStatementAst]) {
        . ([scriptblock]::Create($statement.Extent.Text))
    }
    else { $calls += $statement.Extent.Text }
}
Check 'the installer only acts from its last line' (($calls -join '|') -ceq 'Install-AltTabio')

$text = [System.IO.File]::ReadAllText($script)
# Windows PowerShell reads a script fetched without a charset as Latin-1.
Check 'the installer is plain ASCII' ($text -cnotmatch '[^\x09\x0A\x0D\x20-\x7E]')
$pinned = @(Get-Content -LiteralPath (Join-Path $root 'scripts/windows/update-signing.pub'))[1]
Check 'the installer pins the app''s release key' ($AltTabioReleaseKey -ceq $pinned)

# The verifier, against the fixtures src/update_signature.rs is tested with.
$archive = [System.IO.File]::ReadAllBytes((Join-Path $fixtures 'archive.txt'))
$valid = [System.IO.File]::ReadAllText((Join-Path $fixtures 'valid.minisig'))
$wrongKey = [System.IO.File]::ReadAllText((Join-Path $fixtures 'wrong-key.minisig'))
$key = @(Get-Content -LiteralPath (Join-Path $fixtures 'fixture.pub'))[1]
# A Windows checkout may give the fixture CRLF line endings.
$lines = @($valid -split "\r?\n" | Where-Object { $_ })

Check 'a signed archive verifies' ($null -eq (Test-AltTabioArchive $archive $valid '9.0.0' $key))
Check 'the signed identity is the one the app writes' ($lines[2] -ceq "trusted comment: $(Get-AltTabioIdentity '9.0.0')")
Check 'another version is refused' ($null -ne (Test-AltTabioArchive $archive $valid '9.0.1' $key))
Check 'the production key refuses the fixture' ($null -ne (Test-AltTabioArchive $archive $valid '9.0.0' $AltTabioReleaseKey))
Check 'a signature from another key is refused' ($null -ne (Test-AltTabioArchive $archive $wrongKey '9.0.0' $key))
$changed = [byte[]]$archive.Clone()
$changed[0] = $changed[0] -bxor 1
Check 'a changed archive is refused' ($null -ne (Test-AltTabioArchive $changed $valid '9.0.0' $key))
Check 'an empty archive is refused' ($null -ne (Test-AltTabioArchive (New-Object byte[] 0) $valid '9.0.0' $key))
foreach ($swap in @(@('windows-x64', 'macos'), @('AltTabio-9', 'OtherApp-9'), @('update v1', 'update v2'), @('9.0.0', '9.0.1'))) {
    $forged = $valid.Replace($swap[0], $swap[1])
    Check "a rewritten identity is refused ($($swap[1]))" ($null -ne (Test-AltTabioArchive $archive $forged '9.0.0' $key))
}
$legacy = $valid.Replace($lines[1], 'RWQ' + $lines[1].Substring(3))
Check 'the legacy signature format is refused' ($null -ne (Test-AltTabioArchive $archive $legacy '9.0.0' $key))
foreach ($broken in @('', 'malformed', ($lines[0..2] -join "`n"), ($valid + "extra`n"), ('x' * 4097))) {
    Check "a broken signature is refused ($($broken.Length) characters)" ($null -ne (Test-AltTabioArchive $archive $broken '9.0.0' $key))
}

# BLAKE2b across the block boundaries, against digests from another implementation.
$known = @{
    0   = '786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce'
    3   = 'ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923'
}
$known[128] = '2319e3789c47e2daa5fe807f61bec2a1a6537fa03f19ff32e87eecbfd64b7e0e8ccff439ac333b040f19b0c4ddd11a61e24ac1fe0f10a039806c5dcc0da3d115'
$known[257] = '9ca40e2ddee9436dbbd08efc65dbaf4870059f5eb3d76efd20241ae5bf13c60f250b882ea5c564838257a3fc95c496819ace2c6490b55b268535208dfc31822c'
foreach ($length in @($known.Keys | Sort-Object)) {
    # 'abc' for three bytes; otherwise the bytes 0, 1, 2 ... wrapping at 251.
    $data = New-Object byte[] $length
    for ($i = 0; $i -lt $length; $i++) { $data[$i] = $i % 251 }
    if ($length -eq 3) { $data = [Text.Encoding]::ASCII.GetBytes('abc') }
    $digest = -join ([AltTabioRelease]::Blake2b512($data) | ForEach-Object { $_.ToString('x2') })
    Check "BLAKE2b of $length bytes" ($digest -ceq $known[$length])
}

# Which release is chosen.
function Release($Tag, $Names, [switch]$Draft, [switch]$Pre, $Base) {
    if (-not $Base) { $Base = "https://github.com/vibeslop/AltTabio/releases/download/$Tag" }
    [pscustomobject]@{
        tag_name = $Tag; draft = [bool]$Draft; prerelease = [bool]$Pre
        assets = @($Names | ForEach-Object { [pscustomobject]@{ name = $_; browser_download_url = "$Base/$_" } })
    }
}
$zip = 'AltTabio-2.0.0-windows-x64.zip'
$both = @($zip, "$zip.minisig")
$older = @('AltTabio-1.9.0-windows-x64.zip', 'AltTabio-1.9.0-windows-x64.zip.minisig')
$picked = Select-AltTabioRelease @(
    (Release 'v3.0.0' @('AltTabio-3.0.0-windows-x64.zip', 'AltTabio-3.0.0-windows-x64.zip.minisig') -Draft),
    (Release 'v2.5.0' @('AltTabio-2.5.0-windows-x64.zip', 'AltTabio-2.5.0-windows-x64.zip.minisig') -Pre),
    (Release 'v2.2.0' @('AltTabio-2.2.0-macos.zip')),
    (Release 'v2.1.0' @('AltTabio-2.1.0-windows-x64.zip')),
    (Release 'v2.0.5' @('AltTabio-2.0.5-windows-x64.zip', 'AltTabio-2.0.5-windows-x64.zip.minisig') -Base 'https://example.com/elsewhere'),
    (Release '2.0.1' @('AltTabio-2.0.1-windows-x64.zip', 'AltTabio-2.0.1-windows-x64.zip.minisig')),
    (Release 'v2.0.0' $both),
    (Release 'v1.9.0' $older)
)
Check 'the newest complete release is chosen' ($picked.Version -ceq '2.0.0')
Check 'its archive comes from its own release' ($picked.Archive -ceq "https://github.com/vibeslop/AltTabio/releases/download/v2.0.0/$zip")
Check 'its signature sits beside the archive' ($picked.Signature -ceq "$($picked.Archive).minisig")
Check 'no release is chosen when none is complete' ($null -eq (Select-AltTabioRelease @((Release 'v2.2.0' @('AltTabio-2.2.0-macos.zip')))))

# What is extracted.
Add-Type -AssemblyName System.IO.Compression
function Zip($Files) {
    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.Compression.ZipArchive($stream, [System.IO.Compression.ZipArchiveMode]::Create, $true)
    foreach ($name in $Files.Keys) {
        $entry = $writer.CreateEntry($name).Open()
        $bytes = [Text.Encoding]::ASCII.GetBytes($Files[$name])
        $entry.Write($bytes, 0, $bytes.Length)
        $entry.Close()
    }
    $writer.Dispose()
    , $stream.ToArray()
}
function Refuses($Files) {
    try { Expand-AltTabioArchive (Zip $Files) $folder; $false } catch { $true }
}
$folder = Join-Path ([System.IO.Path]::GetTempPath()) "alttabio-bootstrap-test-$([guid]::NewGuid())"
$null = New-Item -ItemType Directory -Path $folder
try {
    [System.IO.File]::WriteAllText((Join-Path $folder 'AltTabio.ini'), 'kept')
    [System.IO.File]::WriteAllText((Join-Path $folder 'AltTabio.exe'), 'old')
    Expand-AltTabioArchive (Zip ([ordered]@{ 'AltTabio.exe' = 'new'; 'LICENSE' = 'mit'; 'THIRD_PARTY_LICENSES.md' = 'others' })) $folder
    Check 'the app is replaced' ([System.IO.File]::ReadAllText((Join-Path $folder 'AltTabio.exe')) -ceq 'new')
    Check 'the licences come with it' ((Test-Path (Join-Path $folder 'LICENSE')) -and (Test-Path (Join-Path $folder 'THIRD_PARTY_LICENSES.md')))
    Check 'the settings are kept' ([System.IO.File]::ReadAllText((Join-Path $folder 'AltTabio.ini')) -ceq 'kept')
    Check 'nothing is left half-written' (@(Get-ChildItem -LiteralPath $folder -Filter '*.new').Count -eq 0)
    Check 'an archive without the app is refused' (Refuses ([ordered]@{ 'LICENSE' = 'mit' }))
    Check 'a settings file in the archive is refused' (Refuses ([ordered]@{ 'AltTabio.exe' = 'x'; 'AltTabio.ini' = 'x' }))
    Check 'a path in the archive is refused' (Refuses ([ordered]@{ 'AltTabio.exe' = 'x'; '../escape.txt' = 'x' }))
    Check 'a folder in the archive is refused' (Refuses ([ordered]@{ 'AltTabio.exe' = 'x'; 'sub/file.txt' = 'x' }))
    Check 'refused archives change no settings' ([System.IO.File]::ReadAllText((Join-Path $folder 'AltTabio.ini')) -ceq 'kept')
}
finally { Remove-Item -LiteralPath $folder -Recurse -Force }

if ($Live) {
    $list = Get-AltTabioBytes 'https://api.github.com/repos/vibeslop/AltTabio/releases?per_page=30' 2MB 30
    $release = Select-AltTabioRelease (ConvertFrom-Json ([Text.Encoding]::UTF8.GetString($list)))
    Check 'a published release is found' ($null -ne $release)
    $archive = Get-AltTabioBytes $release.Archive 100MB 180
    $signature = [Text.Encoding]::UTF8.GetString((Get-AltTabioBytes $release.Signature 4096 30))
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $problem = Test-AltTabioArchive $archive $signature $release.Version $AltTabioReleaseKey
    Check "AltTabio $($release.Version) ($($archive.Length) bytes) verifies with the pinned key in $([int]$clock.Elapsed.TotalMilliseconds) ms" ($null -eq $problem)
    $archive[$archive.Length - 1] = $archive[$archive.Length - 1] -bxor 1
    Check 'the same release with one bit changed is refused' ($null -ne (Test-AltTabioArchive $archive $signature $release.Version $AltTabioReleaseKey))
    $names = @((New-Object System.IO.Compression.ZipArchive((New-Object System.IO.MemoryStream(, (Get-AltTabioBytes $release.Archive 100MB 180))))).Entries | ForEach-Object { $_.FullName })
    Check "the release holds only plain files ($($names -join ', '))" (@($names | Where-Object { $_ -cnotmatch '^[A-Za-z0-9][A-Za-z0-9_.-]{0,80}$' }).Count -eq 0 -and $names -ccontains 'AltTabio.exe')
}

if ($failed) { Write-Host "$failed check(s) failed." -ForegroundColor Red; exit 1 }
Write-Host 'All checks passed.'
