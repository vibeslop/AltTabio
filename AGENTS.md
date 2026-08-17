# AltTabio contributor rules

## Architecture

- Switcher behavior stays outside Win32 code. Windows callbacks translate input into bounded application events — no rendering, enumeration, file I/O, logging, or blocking work inside them.

## Rust and Win32

- Verify every Rust change with `cargo fmt -- --check`, `cargo clippy --quiet --all-targets -- -D warnings`, and `cargo test --quiet`. Prefer `--quiet` on all cargo invocations; only failures matter, don't echo successful build output.
- Production paths never panic on external state. Propagate or handle errors explicitly; never silently discard a fallible Win32 or COM result.
- `unsafe` lives in narrow Windows adapters, and each block states the invariant that makes it sound. A Windows callback must catch and contain panics — never unwind across `extern "system"`.
- Owned handles release exactly once; borrowed `HWND` values are never destroyed. Pair COM/WinRT init and uninit on the same thread through an owning guard.
- Enable only the `windows` crate features the code actually uses. New dependencies (UI framework, async runtime, allocator, logging stack) need a measured justification.

## Build gotchas

- Close AltTabio before compiling or testing — a running instance locks build artifacts.
- After completing a Rust change, confirm committed `HEAD` builds with `cargo build --release --quiet`. Start the new executable only when runtime verification is part of the task or explicitly requested.

## Git

- Commit only changes made for the current task; stage individual hunks when a file mixes unrelated work. Commit completed slices promptly after verification passes.

## Cursor Cloud specific instructions

AltTabio is a Windows-only app (Win32/Direct2D/DWM, edition 2024, target `x86_64-pc-windows-msvc`), but the Cloud VM is Linux. Host-target commands (`cargo build`, `cargo test`, `cargo clippy` with no `--target`) fail to compile — always cross-compile to `x86_64-pc-windows-msvc`. The base environment already has `cargo-xwin`, `clang`/`clang-cl`, `lld` (`lld-link`, `llvm-lib`, `llvm-rc`), and `wine`; `cargo xwin` supplies the MSVC CRT/SDK and links with `lld-link`.

Linux equivalents of the Windows verification commands in "Rust and Win32" / README:

- Format (host, no cross-compile): `cargo fmt -- --check`
- Lint: `cargo xwin clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`
- Test (runs the MSVC test binaries under Wine): `WINEPREFIX="$HOME/.wine-alttabio" WINEDEBUG=-all cargo xwin test --target x86_64-pc-windows-msvc` (197 tests)
- Release build: `cargo xwin build --release --target x86_64-pc-windows-msvc` → `target/x86_64-pc-windows-msvc/release/AltTabio.exe`

Running the GUI on Linux (best-effort — the app is really meant for Windows; DWM live previews, global hooks, and admin elevation do not work under Wine):

- Use preview mode, which installs no global hooks: `WINEPREFIX="$HOME/.wine-alttabio" WINEDEBUG=-all DISPLAY=:1 wine target/x86_64-pc-windows-msvc/debug/AltTabio.exe --preview --no-dwm-preview`
- The overlay only appears if other switchable top-level windows exist. With none, `--preview` enumerates nothing, hides, and exits immediately (`exit_when_hidden`). Launch e.g. `wine notepad &` and `wine explorer &` in the same `WINEPREFIX` first, then start AltTabio. With the overlay focused, arrow keys navigate, typing filters, and Enter/left-click activates the selected window.

Benign, expected noise under Wine/cross-build: `warning: Compiler family detection failed ... clang-cl` during `embed-resource` builds, and `Could not read the Windows app theme; using Light` at startup (Wine registry lacks the theme key).
