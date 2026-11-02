# Contributing to AltTabio

The rules for code changes live in [AGENTS.md](AGENTS.md), including where code goes and which checks a change must pass. This file covers building and releasing.

## Building

AltTabio uses the Rust version pinned in `rust-toolchain.toml`, plus, on Windows, the MSVC build tools, or, on macOS, the Command Line Tools. Close AltTabio before compiling or testing; the running Windows app locks its executable.

### Windows

```powershell
cargo fmt -- --check
cargo clippy --quiet --all-targets -- -D warnings
cargo test --quiet
cargo build --release --quiet
powershell.exe -NoProfile -NonInteractive -File scripts/windows/test-updater.ps1
```

The release executable must be written to this checkout's `target\release\AltTabio.exe`; override `CARGO_TARGET_DIR` if it points elsewhere. `cargo run -- --preview` shows the overlay without installing global hooks.

### macOS

```sh
scripts/mac/run.sh                 # build target/mac/AltTabio.app and start it with its log in the terminal
scripts/mac/run.sh -- --preview    # show the overlay once without taking over Cmd+Tab
scripts/mac/run.sh -- --list       # print the windows AltTabio sees and exit
scripts/mac/run.sh -- --settings   # start with the settings window open
scripts/mac/build-app.sh           # only build the bundle
```

Before submitting a macOS change, run the same formatting, Clippy, and test commands above, then `scripts/mac/build-app.sh`. CI also builds and verifies a universal bundle for Apple silicon and Intel.

macOS ties Accessibility and Screen Recording grants to the certificate an app is signed with, and ad-hoc signatures change with every build. Run `scripts/mac/make-signing-cert.sh` once so your grants survive rebuilds, and choose **Always Allow** if a build asks whether codesign may use the key. When AltTabio is started from a terminal, macOS applies the terminal's grants to it. A build signed with anything but the release certificate never updates itself.

`ALTTABIO_TRACE=1 scripts/mac/run.sh` prints every intercepted key event and switcher action. `--list` and `--activate <window id>` also work from a plain `cargo run`. Settings live in `~/Library/Application Support/AltTabio/AltTabio.ini`, or in an `AltTabio.ini` next to the executable if one exists there.

## Releasing

Routine fixes and local builds do not need a version bump. When preparing a release, set the version in `Cargo.toml` and `app.rc`, let `cargo` update `Cargo.lock`, and update the version shown in `docs/product.md`, `docs/llms-full.txt`, and the three product pages under `docs`. The macOS bundle version comes from `Cargo.toml`.

The version bump can be in the release PR, followed by more fixes under the same unreleased version. Once the PR is merged and the final commit on `main` passes CI, tag that commit `v<version>`. Do not tag the earlier version-bump commit. Use the next patch version for fixes released after a version has been published.

The Release workflow builds the Windows and macOS archives, signs the Windows archive with the dedicated Minisign update key and the macOS one with the [release certificate](#the-macos-release-certificate), and attaches both archives plus the Windows `.zip.minisig` to a draft release. Write its notes, one section per platform, and publish it; the install script and both updaters only see published releases. Preparing signing code or keys does not authorize running this workflow or publishing a release.

Windows MSVC builds statically link the C runtime through `.cargo/config.toml`, so the executable does not require a separate Visual C++ Redistributable installation. Runtime fixes are delivered by rebuilding and releasing AltTabio.

The Windows release job sets `ALTTABIO_RELEASE=1` at build time to enable the updater. Ordinary source builds leave it unset so they never download over local work. Windows verifies the archive's GitHub asset digest and Minisign signature before extraction, stages only `AltTabio.exe` in a protected folder beside the installation, and uses a helper to replace and restart it once the running copy exits. Run `powershell.exe -NoProfile -File scripts/windows/test-updater.ps1` to exercise verification failures, replacement, and rollback with ephemeral test keys, without downloading or launching a release. This builds the isolated signing tool using its own lockfile; no production secrets are needed.

Before publishing a Windows release, manually check Alt+Tab, Shift+Alt+Tab, Win+Tab, opening the switcher over Start and Search, and typing and editing a filter with an AltGr layout. Also verify a real install, update, and relaunch in a disposable Windows VM, starting from the autostart task and using a throwaway release repository or tag. Script mocks and hidden-window tests do not replace these checks.

The app pins `scripts/windows/update-signing.pub`. The `windows-release` GitHub environment admits only `v*` tags and holds `WINDOWS_UPDATE_SIGNING_KEY`, a password-encrypted Minisign secret key box, and `WINDOWS_UPDATE_SIGNING_PASSWORD`. Only the Windows release signing step receives these secrets. PR checks use test keys; the macOS job retains its separate `release` environment. Missing secrets or a key that does not match the app pin fail signing before asset upload. The release job also checks both embedded Windows version strings against the tag before packaging.

Keep this Windows key across releases. Its recovery files live under `windows-update/` in the private [vibeslop/release-signing](https://github.com/vibeslop/release-signing/tree/main/windows-update) repository, encrypted with age to the existing maintainer SSH recipients. Restore the same pair into `windows-release`; do not regenerate it as part of a normal release. The existing Mac rekey script does not cover the Windows folder, which must be re-encrypted separately when recipients change. See [Windows updater maintenance](docs/windows-updater.md) for recovery, key rotation and the trust boundary.

The updater's isolated environment and explicit module imports constrain its scripts. Windows PowerShell 5.1 and .NET Framework initialize before those scripts run, so this is not a sandbox for per-user Windows or .NET runtime startup configuration. A relaunched GUI process receives a fresh user environment; it does not inherit the helper's restricted or temporary environment.

Running the workflow by hand builds a tag's Windows archive alone. Select that same tag as the workflow ref and the `tag` input; a branch ref fails before draft creation. Re-running the tag's own run rebuilds both.

### The macOS release certificate

Every release carries the same release certificate, so users keep their permissions across updates. Its SHA-1 hash is pinned in `scripts/mac/release-certificate.sha1`, and its key lives in the secrets of the repository's `release` environment, which only `v*` tags can use. A maintainer creates it once with `scripts/mac/make-signing-cert.sh --release`, commits the pin, and stores the backup the script writes, with its password, in the private [vibeslop/release-signing](https://github.com/vibeslop/release-signing) repository, encrypted to the maintainers' SSH keys. Whoever holds the key can sign an app that macOS treats as AltTabio, and a new certificate would make every user grant both permissions again.

The [Release](#releasing) workflow signs the macOS archive with it. `scripts/mac/package.sh` builds and signs the same archive by hand on a Mac that imported the certificate with `scripts/mac/make-signing-cert.sh --import <backup.p12>`, and refuses a bundle signed with any other certificate.
