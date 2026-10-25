# Embedded in the executable, run with Windows PowerShell 5.1 on a worker thread.
# $Mode, $Curl, $Version and $Work are literal values supplied by the adapter.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Get-ReleaseJson([string] $Uri) {
    $json = & $Curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' `
        --max-time 30 --max-filesize 2097152 --user-agent AltTabio `
        --header 'Accept: application/vnd.github+json' $Uri
    if ($LASTEXITCODE -ne 0) { throw 'Could not read releases from GitHub.' }
    $text = $json -join "`n"
    if ($Mode -eq 'check' -and -not $text.TrimStart().StartsWith('[')) { throw 'GitHub did not return a release list.' }
    return ($text | ConvertFrom-Json)
}

try {
    $api = 'https://api.github.com/repos/vibeslop/AltTabio/releases'
    if ($Mode -eq 'check') {
        $releases = @(Get-ReleaseJson "$api`?per_page=30")
        # Only metadata crosses stdout. The shared Rust selector chooses the version.
        foreach ($release in $releases) {
            if ($release.tag_name -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') { continue }
            if ($release.draft -isnot [bool] -or $release.prerelease -isnot [bool]) { continue }
            foreach ($asset in $release.assets) {
                if ($asset.name -notmatch '^AltTabio-[0-9]+\.[0-9]+\.[0-9]+-windows-x64\.zip$') { continue }
                $expected = "https://github.com/vibeslop/AltTabio/releases/download/$($release.tag_name)/$($asset.name)"
                if ($asset.browser_download_url -cne $expected) { continue }
                "{0}`t{1}`t{2}`t{3}`t{4}" -f $release.tag_name, $release.draft, $release.prerelease, $asset.name, $expected
            }
        }
    } elseif ($Mode -eq 'stage') {
        $release = Get-ReleaseJson "$api/tags/v$Version"
        if ($release.draft -ne $false -or $release.prerelease -ne $false -or $release.tag_name -cne "v$Version") {
            throw 'This release is no longer a published stable release.'
        }
        $name = "AltTabio-$Version-windows-x64.zip"
        $assets = @($release.assets | Where-Object { $_.name -ceq $name })
        if ($assets.Count -ne 1) { throw 'The release has no unique Windows archive.' }
        $asset = $assets[0]
        $url = "https://github.com/vibeslop/AltTabio/releases/download/v$Version/$name"
        if ($asset.browser_download_url -cne $url -or $asset.digest -cnotmatch '^sha256:[a-fA-F0-9]{64}$') {
            throw 'The release has no verifiable Windows archive.'
        }
        if ($asset.size -le 0 -or $asset.size -gt 104857600) { throw 'The update archive is too large or empty.' }
        $archive = Join-Path $Work 'download.zip'
        & $Curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' `
            --max-time 600 --max-filesize 104857600 --user-agent AltTabio --output $archive $url
        if ($LASTEXITCODE -ne 0) { throw 'Could not download the update.' }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ine $asset.digest.Substring(7)) {
            throw 'The downloaded archive failed its SHA-256 check.'
        }
        Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            # Extract only the exact executable. Archive paths never become filesystem paths.
            $entries = @($zip.Entries | Where-Object { $_.FullName -ceq 'AltTabio.exe' })
            if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or $entries[0].Length -gt 52428800) {
                throw 'The archive has no unique Windows executable of an allowed size.'
            }
            $exe = Join-Path $Work 'AltTabio.exe'
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], $exe, $false)
        } finally { $zip.Dispose() }
        $found = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
        if ($found.ProductVersion -cne $Version -or $found.OriginalFilename -cne 'AltTabio.exe') {
            throw 'The downloaded executable does not match the release version or application.'
        }
        (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    } else { throw 'Unknown update operation.' }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
