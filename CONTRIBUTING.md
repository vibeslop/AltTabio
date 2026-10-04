# Contributing to AltTabio

The rules for code changes live in [AGENTS.md](AGENTS.md), including where code goes and which checks a change must pass. This file covers building and releasing.

## Building

AltTabio requires the Rust toolchain plus, on Windows, the MSVC build tools, or, on macOS, the Command Line Tools.

### Windows

```powershell
cargo check --all-targets --message-format short
cargo clippy --all-targets --quiet --message-format short -- -D warnings
cargo test -q
cargo build --release
```

The release executable is written to `target\release\AltTabio.exe`. `cargo run -- --preview` shows the overlay without installing global hooks.

### macOS

```sh
scripts/mac/run.sh                 # build target/mac/AltTabio.app and start it with its log in the terminal
scripts/mac/run.sh -- --preview    # show the overlay once without taking over Cmd+Tab
scripts/mac/run.sh -- --list       # print the windows AltTabio sees and exit
scripts/mac/run.sh -- --settings   # start with the settings window open
scripts/mac/build-app.sh           # only build the bundle
```

macOS ties Accessibility and Screen Recording grants to the certificate an app is signed with, and ad-hoc signatures change with every build. Run `scripts/mac/make-signing-cert.sh` once so your grants survive rebuilds, and choose **Always Allow** if a build asks whether codesign may use the key. When AltTabio is started from a terminal, macOS applies the terminal's grants to it. A build signed with anything but the release certificate never updates itself.

`ALTTABIO_TRACE=1 scripts/mac/run.sh` prints every intercepted key event and switcher action. `--list` and `--activate <window id>` also work from a plain `cargo run`. Settings live in `~/Library/Application Support/AltTabio/AltTabio.ini`, or in an `AltTabio.ini` next to the executable if one exists there.

## Releasing

Set the version in `Cargo.toml` and `app.rc`, let `cargo` update `Cargo.lock`, and push the tag `v<version>` once that commit is on `main`. The Release workflow builds the Windows and macOS archives, signs the macOS one with the [release certificate](#the-macos-release-certificate), and attaches both to a draft release. Write its notes, one section per platform, and publish it; the install script and the Mac updater only see published releases.

Running the workflow by hand with a tag builds that tag's Windows archive alone. Re-running the tag's own run rebuilds both.

### The macOS release certificate

Every release carries the same release certificate, so users keep their permissions across updates. Its SHA-1 hash is pinned in `scripts/mac/release-certificate.sha1`, and its key lives in the secrets of the repository's `release` environment, which only `v*` tags can use. A maintainer creates it once with `scripts/mac/make-signing-cert.sh --release`, commits the pin, and stores the backup the script writes, with its password, in the private [vibeslop/release-signing](https://github.com/vibeslop/release-signing) repository, encrypted to the maintainers' SSH keys. Whoever holds the key can sign an app that macOS treats as AltTabio, and a new certificate would make every user grant both permissions again.

The [Release](#releasing) workflow signs the macOS archive with it. `scripts/mac/package.sh` builds and signs the same archive by hand on a Mac that imported the certificate with `scripts/mac/make-signing-cert.sh --import <backup.p12>`, and refuses a bundle signed with any other certificate.
