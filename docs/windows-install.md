# Verify the first Windows install

Verify the Windows ZIP before extracting it or running `AltTabio.exe`. The release executable asks for administrator rights on its first launch. A successful download or an accepted UAC prompt does not authenticate it.

The offline `verify-install` command checks the complete archive's Minisign signature and its signed application, version, Windows x64 platform and archive name. It uses the same embedded [public key](../scripts/windows/update-signing.pub) and verifier as the app's updater. Its key ID is `17A9D4515130A267`. It cannot accept a replacement key from an asset or a command-line option. It never extracts or executes the archive, installs the app, or needs administrator rights or signing secrets.

## Build a trusted verifier

Use a separately trusted checkout of the [AltTabio source repository](https://github.com/vibeslop/AltTabio), with [Git](https://git-scm.com/downloads/win), the [Rust MSVC toolchain](https://www.rust-lang.org/tools/install) and the Windows MSVC build tools installed. The source checkout, its pinned public key, the compiler and Cargo dependencies are the trust prerequisites. Obtain and review them independently of the release assets you are checking. Source obtained from a compromised repository cannot establish trust in that repository's releases.

For example, in an ordinary, non-administrator PowerShell window, clone the source repository into a new directory, then build the tool:

```powershell
git clone https://github.com/vibeslop/AltTabio.git AltTabio-source
if ($LASTEXITCODE -ne 0) { throw 'Source checkout failed.' }
Set-Location AltTabio-source
cargo build --quiet --locked --manifest-path tools/windows-update-signing/Cargo.toml --target-dir target/install-verifier
if ($LASTEXITCODE -ne 0) { throw 'Verifier build failed.' }
```

This builds only the offline tool, not the AltTabio app. Do not run a verifier, script or public-key file supplied inside the release ZIP or alongside its assets. The verifier comes from the source checkout above; no release-asset executable has to run before authentication.

## Verify, then extract

From [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest), choose the intended Windows release and download both `AltTabio-<version>-windows-x64.zip` and `AltTabio-<version>-windows-x64.zip.minisig` to the same directory. Keep their exact filenames. Use that release's canonical version, without the `v` prefix. Older releases without a sidecar cannot pass this check.

Still in the source checkout, edit the version and path below to match the release you chose. `1.3.0` is an example, not a freshness pin:

```powershell
$version = '1.3.0'
$archive = Join-Path $env:USERPROFILE "Downloads\AltTabio-$version-windows-x64.zip"
& .\target\install-verifier\debug\windows-update-signing.exe verify-install $archive $version
if ($LASTEXITCODE -ne 0) { throw 'Verification failed. Do not extract or run this archive.' }
```

Proceed only after exit code zero and the `Verified AltTabio ... Windows x64 archive and signed release identity` message. Missing, malformed, oversized, foreign-key or invalid signatures fail. An authentic signature for another version, application, platform or archive name also fails. No download or verification failure is a reason to skip this step.

Extract only the archive you just verified, without replacing or changing it in between. Use a folder that only administrators can change, such as `C:\Program Files\AltTabio`. Then run the extracted `AltTabio.exe` and accept UAC. You can remove the verifier build and source checkout afterwards.

Verification authenticates the selected release; it cannot prove that an asset server has shown you the newest release. Choose the version deliberately from a trusted release announcement. The app has no Authenticode certificate, so Minisign verification does not change the publisher shown by UAC or SmartScreen. Subsequent in-app updates use the [Windows updater trust model](windows-updater.md).
