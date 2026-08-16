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
- After finishing any Rust change, always run `cargo build --release --quiet` before considering the work done. Write the executable to this repo's `target\release\AltTabio.exe` — if `CARGO_TARGET_DIR` points at a sandbox cache, override it so the file the user runs actually updates. Start the new executable only when runtime verification is part of the task or explicitly requested.

## Git

- Commit only changes made for the current task; stage individual hunks when a file mixes unrelated work. Commit completed slices promptly after verification passes.
