# Windows update authenticity

Windows checks and installs updates inside the Rust executable. HTTPS uses WinHTTP, SHA-256 uses Windows CNG, JSON uses `serde_json`, and ZIP extraction uses `zip` with the Rust `flate2` backend. No PowerShell, curl, script file, or script interpreter runs during updates.

Each HTTP request has one deadline covering connection setup, redirects, response headers, and body reads. The update worker waits for bounded asynchronous WinHTTP completions and closes the request when its deadline expires. Pending read buffers remain owned until WinHTTP reports that the request has finished closing.

Windows locks a running executable, so the final replacement uses a temporary copy of the current AltTabio executable in a private `--apply-update` mode. It runs as a GUI-subsystem process without a console, acquires and validates the original process handle before acknowledging readiness, then waits for that process to exit. Only an explicit approval marker permits replacement and restart. The new app receives a normal user environment built by Windows, with no inherited helper configuration. The locked helper image stays in its protected staging directory until a later maintenance pass can remove it.

Windows updates use [Minisign](https://jedisct1.github.io/minisign/), with prehashed Ed25519 signatures. The app embeds `scripts/windows/update-signing.pub`; neither the release API nor an asset can supply a trusted replacement key. The runtime uses [`minisign-verify` 0.3.0](https://github.com/jedisct1/rust-minisign-verify), a maintained verifier with no external dependencies. Key generation and signing use [`minisign` 0.10.0](https://github.com/jedisct1/rust-minisign) in the separate `tools/windows-update-signing` package. Signing and password-derivation dependencies are absent from the app's dependency tree. No custom cryptography is implemented.

## What is authenticated

Each future Windows release includes `AltTabio-<version>-windows-x64.zip.minisig` beside its archive. Minisign authenticates every archive byte and this exact trusted comment, substituting the canonical release version in both places:

```text
AltTabio windows update v1; version=1.3.0; platform=windows-x64; asset=AltTabio-1.3.0-windows-x64.zip
```

This binds the archive to AltTabio, the protocol, version, platform and asset name. Editing the comment breaks its global signature. A correctly signed archive from another version or platform is also rejected. Legacy non-prehashed signatures are not accepted.

The release list remains untrusted discovery metadata. The updater selects a newer stable published version, rechecks that release, requires exact GitHub asset URLs and a unique archive and signature, and checks GitHub's archive digest and reported size. Limits remain 100 MiB for archives, 4 KiB for signatures and 50 MiB for the extracted executable. The blocking worker downloads into a protected staging directory, verifies the full archive and signed identity with the embedded key, then invokes extraction. No unsigned archive is opened as a ZIP. It accepts single-disk ZIP32 release archives, rejects duplicate filenames, extracts only the unique root `AltTabio.exe`, and checks its embedded product version and original filename.

The installation-folder ACL checks, protected staging ACL, approval marker, helper readiness handshake, final executable rehash, permission preservation, replacement, rollback and failure cooldown still apply. Input and window callbacks do no verification or network work.

Signatures protect updates when release metadata or assets are changed without the Windows private key. They do not protect a compromised signing job, stolen signing key, administrator-controlled app files, or the initial unverified manual installation. HTTPS and GitHub checks remain required. Withholding releases can prevent discovery; this protocol is not a transparency log or a signed freshness service. Older app builds do not enforce the new signature policy. No Authenticode certificate is used, so UAC and SmartScreen behavior is unchanged.

## Release automation and recovery

The protected `windows-release` environment admits only `v*` tags. `WINDOWS_UPDATE_SIGNING_KEY` contains the password-encrypted Minisign secret key text, and `WINDOWS_UPDATE_SIGNING_PASSWORD` contains its password. They are passed only to the Windows signing step, never PR CI or the macOS job. The release tool signs the archive, then verifies it using the app's embedded public key before writing the sidecar. Missing secrets, an incorrect password or a mismatched key fail the job before upload. The workflow publishes the archive and sidecar together during a future authorized release. Manual workflow runs must select the same tag as the workflow ref and tag input. Maintainers who can create version tags and change release code are part of the signing trust boundary.

Public key ID: `17A9D4515130A267`. Retain this pair across releases. The encrypted recovery backup is in the private [release-signing Windows folder](https://github.com/vibeslop/release-signing/tree/main/windows-update), separate from every macOS file. It contains the password-encrypted key, its password, public key and restoration instructions. Both secret files have an additional age encryption layer addressed to the existing maintainer SSH keys. Restore with a private SSH identity from a file, piping decrypted values directly into `gh secret set --env windows-release --repo vibeslop/AltTabio` with tracing disabled. Do not echo them, write plaintext production keys to a checkout, or place them in build outputs. Test recovery by signing an inert local archive and verifying against the public pin, without creating a tag or release.

When recipients change, re-encrypt both Windows recovery files separately and verify recovery before retiring the old SSH identity. The existing Mac `rekey.sh` does not process this directory. Never decrypt, reuse or replace the Mac private key for Windows signing.

Changing the embedded public key alone strands existing clients. For planned rotation, first ship a release authenticated by the old key that explicitly trusts the replacement, then transition signing after clients can verify it. If the old key is compromised, a signature from that key cannot establish safe recovery; distribute and verify a new installation through an independent trusted channel. Do not rotate automatically or take replacement keys from release assets.

## Local verification

Run the required Cargo checks and release build. Rust tests cover archive tampering, signature corruption, key substitution, signed identity mismatches, legacy signatures, bounds, worker ordering, native metadata validation, extraction, duplicate entries, version resources, final rehash, permissions, approval, replacement, rollback, and readiness deadlines. Native permission and desktop tests need an elevated Windows desktop session. Permission assertions compare the ACEs and DACL protection; Windows may add the auto-inheritance bookkeeping flag when restoring a legacy DACL through `SetNamedSecurityInfoW`.

`cargo test --quiet native_https_reads_live_github_metadata -- --ignored` explicitly exercises WinHTTP against the public GitHub API. It does not download or install a release. Offline replacement tests use inert files and launch mocks. These checks do not claim a live signed GitHub release-to-install test.

## Measured dependency and size cost

On Windows x64 MSVC, Rust 1.95.0, the unchanged `d04b0ce` baseline and this implementation were built with the same release profile, static CRT and `ALTTABIO_RELEASE=1`. This matters because ordinary development builds can optimize away disabled updater paths.

| Artifact | Baseline bytes | With signing bytes | Added bytes |
| --- | ---: | ---: | ---: |
| Executable | 927,232 | 974,848 | 47,616 |
| ZIP of executable | 485,987 | 510,035 | 24,048 |
| Windows archive including licenses | 488,203 | 512,512 | 24,309 |

The executable cost is 5.14%. A signature sidecar with this comment is 360 bytes. `dumpbin /DEPENDENTS` found no external VC runtime DLL. The app adds one Rust dependency and zero transitive dependencies; the signing tool's larger dependency tree is build tooling and is not shipped. This cost buys archive and release-identity authentication without a native crypto DLL or signing process inside the app.

The later native Windows updater was measured against the script-based updater with the same release profile, static CRT, and `ALTTABIO_RELEASE=1`. The executable grew from 973,824 to 1,133,056 bytes, and an optimally compressed ZIP containing only `AltTabio.exe` grew from 509,793 to 597,657 bytes. The additional runtime crates implement JSON parsing and ZIP/Deflate decoding inside the executable. WinHTTP, CNG, version resources, permissions, environment creation, and process replacement use Windows APIs; no HTTP client, TLS runtime, async runtime, or crypto crate was added.
