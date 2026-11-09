# Installs or updates AltTabio on Windows:
#
#   irm https://vibeslop.github.io/AltTabio/install.ps1 | iex
#
# Downloads the newest release's archive and its Minisign signature, and extracts the archive only
# after it passes the check the app's updater makes: the pinned release key, and the signed
# application, version, platform and archive name. The key lives here, never beside the archive
# it vouches for. The archive stays in memory from download to extraction, so nothing can swap it
# in between. Everything runs from Install-AltTabio on the last line, so a download cut short
# runs nothing.

# Keep this in step with scripts/windows/update-signing.pub; scripts/windows/test-bootstrap.ps1
# fails when they differ.
$AltTabioReleaseKey = 'RWRnojBRUdSpF5gy5IhcPeFdAm68b+gFyJfJjq0lH3Ap++5XgLvAD7t8'
$AltTabioInstallCommand = 'irm https://vibeslop.github.io/AltTabio/install.ps1 | iex'

# Minisign verification. Windows has no Ed25519 or BLAKE2b of its own, so both are here, written
# for the C# 5 compiler that Windows PowerShell 5.1 carries. Only the prehashed format is accepted,
# as in src/update_signature.rs.
$AltTabioVerifierSource = @'
using System;
using System.Numerics;
using System.Security.Cryptography;
using System.Text;

public static class AltTabioRelease
{
    // Returns null when the archive is authentic and signed as `identity`, else the reason.
    public static string Check(byte[] archive, string signatureText, string publicKey, string identity)
    {
        if (archive == null || archive.Length == 0) return "The archive is empty";
        if (signatureText == null || signatureText.Length > 4096) return "The signature is too large";
        string[] lines = signatureText.Replace("\r\n", "\n").Split('\n');
        int count = lines.Length;
        if (count > 0 && lines[count - 1].Length == 0) count--;
        const string trustedPrefix = "trusted comment: ";
        if (count != 4 || !lines[0].StartsWith("untrusted comment: ", StringComparison.Ordinal)
            || !lines[2].StartsWith(trustedPrefix, StringComparison.Ordinal))
            return "The signature is not a Minisign signature";
        byte[] key, signature, globalSignature;
        try
        {
            key = Convert.FromBase64String(publicKey);
            signature = Convert.FromBase64String(lines[1]);
            globalSignature = Convert.FromBase64String(lines[3]);
        }
        catch (FormatException) { return "The signature is malformed"; }
        if (key.Length != 42 || key[0] != (byte)'E' || key[1] != (byte)'d') return "The release key is malformed";
        if (signature.Length != 74 || globalSignature.Length != 64) return "The signature is malformed";
        if (signature[0] != (byte)'E' || signature[1] != (byte)'D') return "The signature uses an unsupported format";
        for (int i = 2; i < 10; i++)
            if (signature[i] != key[i]) return "The archive was signed with another key";
        byte[] pk = Slice(key, 10, 32);
        byte[] sig = Slice(signature, 10, 64);
        string trusted = lines[2].Substring(trustedPrefix.Length);
        if (!Ed25519Verify(pk, Blake2b512(archive), sig)) return "The archive does not match its signature";
        if (!Ed25519Verify(pk, Concat(sig, Encoding.UTF8.GetBytes(trusted)), globalSignature)) return "The signed release identity is not authentic";
        if (!string.Equals(trusted, identity, StringComparison.Ordinal)) return "The archive is signed as another release";
        return null;
    }

    static byte[] Slice(byte[] source, int offset, int length)
    {
        byte[] part = new byte[length];
        Buffer.BlockCopy(source, offset, part, 0, length);
        return part;
    }

    static byte[] Concat(byte[] a, byte[] b)
    {
        byte[] all = new byte[a.Length + b.Length];
        Buffer.BlockCopy(a, 0, all, 0, a.Length);
        Buffer.BlockCopy(b, 0, all, a.Length, b.Length);
        return all;
    }

    // BLAKE2b-512, unkeyed (RFC 7693).
    static readonly ulong[] IV = {
        0x6a09e667f3bcc908UL, 0xbb67ae8584caa73bUL, 0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL, 0x9b05688c2b3e6c1fUL, 0x1f83d9abfb41bd6bUL, 0x5be0cd19137e2179UL
    };
    static readonly byte[][] Sigma = {
        new byte[] { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15 },
        new byte[] { 14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3 },
        new byte[] { 11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4 },
        new byte[] { 7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8 },
        new byte[] { 9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13 },
        new byte[] { 2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9 },
        new byte[] { 12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11 },
        new byte[] { 13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10 },
        new byte[] { 6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5 },
        new byte[] { 10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0 }
    };

    public static byte[] Blake2b512(byte[] data)
    {
        unchecked
        {
            ulong[] h = (ulong[])IV.Clone();
            h[0] ^= 0x01010040UL;
            byte[] block = new byte[128];
            int offset = 0;
            // Every block but the last; the last may be full, and an empty input is one zero block.
            while (data.Length - offset > 128)
            {
                Buffer.BlockCopy(data, offset, block, 0, 128);
                offset += 128;
                Compress(h, block, (ulong)offset, false);
            }
            Array.Clear(block, 0, 128);
            Buffer.BlockCopy(data, offset, block, 0, data.Length - offset);
            Compress(h, block, (ulong)data.Length, true);
            byte[] digest = new byte[64];
            for (int i = 0; i < 8; i++)
                for (int j = 0; j < 8; j++)
                    digest[i * 8 + j] = (byte)(h[i] >> (8 * j));
            return digest;
        }
    }

    static void Compress(ulong[] h, byte[] block, ulong counted, bool last)
    {
        unchecked
        {
            ulong[] m = new ulong[16];
            for (int i = 0; i < 16; i++) m[i] = BitConverter.ToUInt64(block, i * 8);
            ulong[] v = new ulong[16];
            for (int i = 0; i < 8; i++) { v[i] = h[i]; v[i + 8] = IV[i]; }
            v[12] ^= counted;
            if (last) v[14] = ~v[14];
            for (int round = 0; round < 12; round++)
            {
                byte[] s = Sigma[round % 10];
                Mix(v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
                Mix(v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
                Mix(v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
                Mix(v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
                Mix(v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
                Mix(v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
                Mix(v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
                Mix(v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
            }
            for (int i = 0; i < 8; i++) h[i] ^= v[i] ^ v[i + 8];
        }
    }

    static void Mix(ulong[] v, int a, int b, int c, int d, ulong x, ulong y)
    {
        unchecked
        {
            v[a] = v[a] + v[b] + x; v[d] = Rotate(v[d] ^ v[a], 32);
            v[c] = v[c] + v[d]; v[b] = Rotate(v[b] ^ v[c], 24);
            v[a] = v[a] + v[b] + y; v[d] = Rotate(v[d] ^ v[a], 16);
            v[c] = v[c] + v[d]; v[b] = Rotate(v[b] ^ v[c], 63);
        }
    }

    static ulong Rotate(ulong value, int bits) { return (value >> bits) | (value << (64 - bits)); }

    // Ed25519 verification (RFC 8032), on points in extended coordinates.
    static readonly BigInteger P = BigInteger.Pow(2, 255) - 19;
    static readonly BigInteger L = BigInteger.Pow(2, 252) + BigInteger.Parse("27742317777372353535851937790883648493");
    static readonly BigInteger D = BigInteger.Parse("37095705934669439343138083508754565189542113879843219016388785533085940283555");
    static readonly BigInteger RootOfMinusOne = BigInteger.ModPow(2, (P - 1) / 4, P);
    static readonly BigInteger BaseX = BigInteger.Parse("15112221349535400772501151409588531511454012693041857206046113283949847762202");
    static readonly BigInteger BaseY = BigInteger.Parse("46316835694926478169428394003475163141307993866256225615783033603165251855960");

    static BigInteger Mod(BigInteger value)
    {
        BigInteger r = value % P;
        return r.Sign < 0 ? r + P : r;
    }

    static BigInteger FromLittleEndian(byte[] bytes)
    {
        // A zero byte on top keeps the number positive.
        byte[] unsigned = new byte[bytes.Length + 1];
        Buffer.BlockCopy(bytes, 0, unsigned, 0, bytes.Length);
        return new BigInteger(unsigned);
    }

    static BigInteger[] Add(BigInteger[] p, BigInteger[] q)
    {
        BigInteger a = Mod((p[1] - p[0]) * (q[1] - q[0]));
        BigInteger b = Mod((p[1] + p[0]) * (q[1] + q[0]));
        BigInteger c = Mod(p[3] * 2 * D * q[3]);
        BigInteger d = Mod(p[2] * 2 * q[2]);
        BigInteger e = b - a, f = d - c, g = d + c, h = b + a;
        return new BigInteger[] { Mod(e * f), Mod(g * h), Mod(f * g), Mod(e * h) };
    }

    static BigInteger[] Multiply(BigInteger scalar, BigInteger[] point)
    {
        BigInteger[] result = { BigInteger.Zero, BigInteger.One, BigInteger.One, BigInteger.Zero };
        BigInteger[] addend = point;
        while (scalar.Sign > 0)
        {
            if (!scalar.IsEven) result = Add(result, addend);
            addend = Add(addend, addend);
            scalar >>= 1;
        }
        return result;
    }

    static BigInteger[] Decode(byte[] encoded)
    {
        byte[] copy = (byte[])encoded.Clone();
        bool odd = (copy[31] & 0x80) != 0;
        copy[31] &= 0x7f;
        BigInteger y = FromLittleEndian(copy);
        if (y >= P) return null;
        BigInteger y2 = Mod(y * y);
        BigInteger xx = Mod((y2 - 1) * BigInteger.ModPow(Mod(D * y2 + 1), P - 2, P));
        BigInteger x = BigInteger.ModPow(xx, (P + 3) / 8, P);
        if (Mod(x * x - xx).Sign != 0) x = Mod(x * RootOfMinusOne);
        if (Mod(x * x - xx).Sign != 0) return null;
        if (x.Sign == 0 && odd) return null;
        if (x.IsEven == odd) x = P - x;
        return new BigInteger[] { x, y, BigInteger.One, Mod(x * y) };
    }

    public static bool Ed25519Verify(byte[] publicKey, byte[] message, byte[] signature)
    {
        if (publicKey.Length != 32 || signature.Length != 64) return false;
        byte[] encodedR = Slice(signature, 0, 32);
        BigInteger s = FromLittleEndian(Slice(signature, 32, 32));
        if (s >= L) return false;
        BigInteger[] a = Decode(publicKey);
        BigInteger[] r = Decode(encodedR);
        if (a == null || r == null) return false;
        byte[] digest;
        using (SHA512 sha = SHA512.Create())
            digest = sha.ComputeHash(Concat(Concat(encodedR, publicKey), message));
        BigInteger k = FromLittleEndian(digest) % L;
        BigInteger[] left = Multiply(s, new BigInteger[] { BaseX, BaseY, BigInteger.One, Mod(BaseX * BaseY) });
        BigInteger[] right = Add(r, Multiply(k, a));
        return Mod(left[0] * right[2] - right[0] * left[2]).Sign == 0
            && Mod(left[1] * right[2] - right[1] * left[2]).Sign == 0;
    }
}
'@

# The words an AltTabio release signs, as update_signature::identity writes them.
function Get-AltTabioIdentity([string]$Version) {
    "AltTabio windows update v1; version=$Version; platform=windows-x64; asset=AltTabio-$Version-windows-x64.zip"
}

# Returns nothing when the archive is this release, else the reason it is not.
function Test-AltTabioArchive([byte[]]$Archive, [string]$Signature, [string]$Version, [string]$Key) {
    if (-not ('AltTabioRelease' -as [type])) {
        # Windows PowerShell must be told where BigInteger lives; PowerShell 7 already knows.
        $references = @{}
        if ($PSVersionTable.PSEdition -ne 'Core') { $references.ReferencedAssemblies = 'System.Numerics' }
        Add-Type -TypeDefinition $AltTabioVerifierSource @references
    }
    [AltTabioRelease]::Check($Archive, $Signature, $Key, (Get-AltTabioIdentity $Version))
}

# The newest published release that carries a Windows archive and its signature.
function Select-AltTabioRelease($Releases) {
    foreach ($release in $Releases) {
        if ($release.draft -or $release.prerelease) { continue }
        if ("$($release.tag_name)" -cnotmatch '^v(\d{1,5}\.\d{1,5}\.\d{1,5})$') { continue }
        $version = $Matches[1]
        $name = "AltTabio-$version-windows-x64.zip"
        $base = "https://github.com/vibeslop/AltTabio/releases/download/v$version"
        $found = @{}
        foreach ($asset in $release.assets) {
            # An asset counts only at the address its name and release give it.
            if ("$($asset.browser_download_url)" -ceq "$base/$($asset.name)") { $found["$($asset.name)"] = $true }
        }
        if ($found[$name] -and $found["$name.minisig"]) {
            return [pscustomobject]@{ Version = $version; Archive = "$base/$name"; Signature = "$base/$name.minisig" }
        }
    }
}

# Downloads over HTTPS into memory, and gives up past a size or a time.
function Get-AltTabioBytes([string]$Url, [int]$MaxBytes, [int]$Seconds) {
    $request = [System.Net.HttpWebRequest]::Create($Url)
    $request.UserAgent = 'AltTabio-installer'
    $request.Accept = 'application/vnd.github+json, application/octet-stream'
    $request.Timeout = 15000
    $request.ReadWriteTimeout = 15000
    $response = $request.GetResponse()
    try {
        if ($response.ResponseUri.Scheme -cne 'https') { throw 'A download left HTTPS.' }
        $source = $response.GetResponseStream()
        $kept = New-Object System.IO.MemoryStream
        $buffer = New-Object byte[] 65536
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        while (($read = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $kept.Write($buffer, 0, $read)
            if ($kept.Length -gt $MaxBytes) { throw 'A download was larger than expected.' }
            if ($clock.Elapsed.TotalSeconds -gt $Seconds) { throw 'A download took too long.' }
        }
        , $kept.ToArray()
    }
    finally { $response.Close() }
}

# Writes the archive's files into the folder. Only plain file names are accepted, and the
# settings file beside the app is never replaced.
function Expand-AltTabioArchive([byte[]]$Archive, [string]$Folder) {
    Add-Type -AssemblyName System.IO.Compression
    $zip = New-Object System.IO.Compression.ZipArchive((New-Object System.IO.MemoryStream(, $Archive)), [System.IO.Compression.ZipArchiveMode]::Read)
    try {
        $names = @($zip.Entries | ForEach-Object { $_.FullName })
        if ($names -cnotcontains 'AltTabio.exe') { throw 'The archive has no AltTabio.exe.' }
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            if ($name -cnotmatch '^[A-Za-z0-9][A-Za-z0-9_.-]{0,80}$' -or $name -match '\.ini$' -or $entry.Length -gt 100MB) {
                throw "The archive holds a file this installer does not expect: $name"
            }
            $target = Join-Path $Folder $name
            $source = $entry.Open()
            try {
                $file = [System.IO.File]::Open("$target.new", [System.IO.FileMode]::Create)
                try { $source.CopyTo($file) } finally { $file.Close() }
            }
            finally { $source.Close() }
            Move-Item -LiteralPath "$target.new" -Destination $target -Force
        }
    }
    finally { $zip.Dispose() }
}

function Install-AltTabio {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    try {
        if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) {
            throw 'AltTabio needs 64-bit Windows 10 or 11.'
        }
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $admin = (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        if (-not $admin) {
            # Program Files is a folder only administrators can change, which AltTabio's own
            # updater and Start at sign-in require. Windows asks once, for PowerShell itself.
            Write-Host 'Windows will ask for permission to install AltTabio. Choose Yes.'
            $shell = Join-Path ([Environment]::GetFolderPath('System')) 'WindowsPowerShell\v1.0\powershell.exe'
            $again = "[Net.ServicePointManager]::SecurityProtocol = 'Tls12'; $AltTabioInstallCommand; Read-Host 'Press Enter to close this window'"
            Start-Process -FilePath $shell -Verb RunAs -ArgumentList '-NoProfile', '-Command', $again
            Write-Host 'The install continues in the new window.'
            return
        }

        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        Write-Host 'Finding the newest AltTabio...'
        $list = Get-AltTabioBytes 'https://api.github.com/repos/vibeslop/AltTabio/releases?per_page=30' 2MB 30
        $releases = ConvertFrom-Json ([Text.Encoding]::UTF8.GetString($list))
        $release = Select-AltTabioRelease $releases
        if (-not $release) { throw 'No signed Windows release is published yet.' }

        Write-Host "Downloading AltTabio $($release.Version)..."
        $archive = Get-AltTabioBytes $release.Archive 100MB 180
        $signature = [Text.Encoding]::UTF8.GetString((Get-AltTabioBytes $release.Signature 4096 30))

        Write-Host 'Checking that the download is really AltTabio...'
        $problem = Test-AltTabioArchive $archive $signature $release.Version $AltTabioReleaseKey
        if ($problem) { throw "$problem. Nothing was installed." }

        $programs = if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
        $folder = Join-Path $programs 'AltTabio'
        $app = Join-Path $folder 'AltTabio.exe'
        foreach ($running in @(Get-Process -Name AltTabio -ErrorAction SilentlyContinue)) {
            if ($running.Path -and $running.Path -ne $app) {
                throw "AltTabio is already running from $($running.Path). Exit it from its tray icon, then run this again."
            }
            Stop-Process -InputObject $running -Force
            $null = $running.WaitForExit(10000)
        }

        Write-Host "Installing in $folder..."
        $null = New-Item -ItemType Directory -Path $folder -Force
        Expand-AltTabioArchive $archive $folder
        Start-Process -FilePath $app
        Write-Host "AltTabio $($release.Version) is installed and running. Press Alt+Tab to try it." -ForegroundColor Green
    }
    catch {
        Write-Host "AltTabio was not installed. $($_.Exception.Message)" -ForegroundColor Red
    }
}

Install-AltTabio
