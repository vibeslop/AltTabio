# AltTabio contributor rules

## Architecture

- Switcher behavior stays outside platform code. Windows callbacks and macOS event-tap, view, and completion callbacks translate input into bounded application events — no rendering, enumeration, file I/O, logging, or blocking work inside them.
- The library crate (`src/alttabio.rs` modules) compiles on both platforms and carries the tests; `src/windows_app.rs` and `src/macos/` are the adapters.

## Rust and Win32

- Verify every Rust change with `cargo fmt -- --check`, `cargo clippy --quiet --all-targets -- -D warnings`, and `cargo test --quiet`. Prefer `--quiet` on all cargo invocations; only failures matter, don't echo successful build output.
- Production paths never panic on external state. Propagate or handle errors explicitly; never silently discard a fallible Win32 or COM result.
- `unsafe` lives in narrow Windows adapters, and each block states the invariant that makes it sound. A Windows callback must catch and contain panics — never unwind across `extern "system"`.
- Owned handles release exactly once; borrowed `HWND` values are never destroyed. Pair COM/WinRT init and uninit on the same thread through an owning guard.
- Enable only the `windows` crate features the code actually uses. New dependencies (UI framework, async runtime, allocator, logging stack) need a measured justification.

## macOS

- `unsafe` lives in narrow `objc2` adapters with the same invariant comments as the Win32 code. Nothing native holds an app-state borrow across a nested run loop (menus, alerts); use `run_later` or `post_to_app`.
- The overlay draws from `SwitcherTokens` in `src/theme.rs`: one fixed palette per light and dark theme, authored in OKLCH as pure greys with no accent, with every text token a real color rather than an alpha of another. Emphasis is the inverse of the surface. The user's accent color is not read; the theme setting (system, light, dark) is the only switch. Contrast lives in the lightness gap, so a readability fix moves L. Edges are 1pt rings in pure black or white at low alpha, never palette grays, and separation comes from surfaces, not lines. Key names shown to the user come from `src/macos/shortcuts.rs`, which also feeds the Shortcuts settings tab.
- Verify with the same cargo commands, then `scripts/mac/build-app.sh`. The bundle is `target/mac/AltTabio.app`; `scripts/mac/run.sh` builds and starts it with logs in the terminal.

## Build gotchas

- Close AltTabio before compiling or testing — a running instance locks build artifacts.
- After finishing any Rust change, always run `cargo build --release --quiet` before considering the work done. Write the executable to this repo's `target\release\AltTabio.exe` — if `CARGO_TARGET_DIR` points at a sandbox cache, override it so the file the user runs actually updates. Start the new executable only when runtime verification is part of the task or explicitly requested.

## Git

- Commit only changes made for the current task; stage individual hunks when a file mixes unrelated work. Commit completed slices promptly after verification passes.
