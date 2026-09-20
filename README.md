# AltTabio

**Website:** [https://vibeslop.github.io/AltTabio/](https://vibeslop.github.io/AltTabio/)
**Download:** [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest)

AltTabio is a free, open-source **Alt+Tab replacement** and window switcher for 64-bit Windows 10 and Windows 11, and a **Cmd+Tab replacement** for macOS 26 and later. It shows a numbered list of open windows, a live preview of the selected window, and typed search. No ads, no account, no telemetry.

It was created because Alt+Tab Terminator has had a critical issue for years that causes Alt+Tab to stop working correctly. The issue was reported, but never fixed.

AltTabio is an independent project and is not affiliated with Alt+Tab Terminator.

## Features

- Replaces the standard Windows Alt+Tab and Win+Tab switchers.
- Shows application icons, window titles, optional application names, and a live preview.
- Uses a compact task list by default to leave more room for the live preview.
- Can show the selected window by itself or in its position on the full desktop.
- Supports keyboard and mouse control, including right mouse button + wheel switching.
- Activates the first nine visible entries directly with the number keys.
- Filters entries by window title, application name, or number as you type.
- Provides close, minimize, maximize, restore, terminate, and run commands for the selected window.
- Supports automatic startup and stores its settings in a portable INI file.

## Installation

1. Download the latest Windows archive from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases).
2. Extract it to a permanent folder that non-elevated processes cannot modify, such as `C:\Program Files\AltTabio`.
3. Run `AltTabio.exe` and accept the Windows administrator prompt.

AltTabio is distributed as one self-contained executable with no separate runtime installation.

AltTabio requires administrator privileges for its global input hooks and window-management commands.

Enabling **Autostart** creates a highest-privilege Windows scheduled task that launches the executable from its current location. If a non-elevated process can replace that file, it can gain administrator privileges at the next logon. Move AltTabio to an administrator-writable-only folder before enabling Autostart.

## Implementation

AltTabio is a native Rust application. The switcher logic, settings, layout, and theme live in a platform-neutral library crate shared by both platforms.

On Windows it builds directly on Windows APIs: low-level keyboard and mouse hooks, a hand-written Win32 overlay, Direct2D/DirectWrite rendering, and DWM previews of the selected window or its full desktop. Window enumeration and management, the tray icon, settings UI, scheduled-task autostart, portable settings, and single-instance enforcement are also implemented natively. The build produces one `AltTabio.exe` with its icon, version information, per-monitor DPI manifest, and administrator requirement embedded.

On macOS it builds on AppKit through the `objc2` bindings: a CGEventTap for Cmd+Tab, Accessibility for window lists and control, a non-activating panel over `NSGlassEffectView`, ScreenCaptureKit for live previews, a menu bar item, an AppKit settings window, and `SMAppService` for launch at login.

## Controls

| Input | Action |
| --- | --- |
| Alt+Tab or Win+Tab | Open and move through the switcher |
| Arrow keys, Tab, Shift+Tab, or mouse wheel | Change the selected window |
| Enter or left click | Activate the selected window |
| 1-9 or numpad 1-9 | Immediately activate the corresponding visible window |
| Type | Filter entries by window title, application name, or number |
| Backspace | Remove a filter character |
| Home or End | Select the first or last window |
| Escape | Close the switcher |
| F4 | Close the selected window |
| F5 | Minimize the selected window |
| F6 | Maximize the selected window |
| F7 | Restore the selected window |
| F8 | Terminate the selected window's process |
| F9 | Launch another instance of the selected application without inheriting AltTabio's elevation |

The tray icon provides access to settings and exit. Right-clicking the selected row opens its window-command menu.

## Settings

Settings include automatic startup; independent Alt+Tab and Win+Tab replacement; typed search; switching when Alt or the right mouse button is released; right mouse button + wheel switching; mouse-over selection; an Azure-default choice of eight app-icon colors; compact list density; large icons; number labels; optional application names; visible borders; live and full-desktop previews; and filtering by the current monitor.

Settings are stored in `AltTabio.ini` next to `AltTabio.exe`.

## macOS

The macOS build switches between **windows**, not apps, which is what people miss most in the system Cmd+Tab: minimized windows, windows of hidden apps, and windows on other Spaces are all listed and reachable. It uses the same list, preview, search, and number-key layout as the Windows build, drawn on Liquid Glass, and needs macOS 26 or later.

| Input | Action |
| --- | --- |
| Cmd+Tab or Option+Tab | Open the switcher and step through the windows; Shift steps back |
| Release Cmd (or Option) | Switch to the selected window |
| 1-9 while holding Cmd | Switch to that row; the row numbers light up as keys while Cmd is down |
| W, M, H, Q while holding Cmd | Close or minimize the selected window; hide or quit its app |
| ` (backtick) while holding Cmd | Step through the windows of the selected window's app |
| K while holding Cmd | Open the actions panel: every window command with its shortcut, driven by arrows and Enter or the mouse |
| After releasing Cmd | The list stays open when "Switch when the modifier is released" is off, or whenever the actions panel is open: arrows move, typing searches, Enter switches, Cmd+1-9 and Cmd+W/M/H/Q/K still act on the selected row |
| Everything else | Same keys as on Windows: Home, End, Escape, F4-F9 |

An action bar under the list works like a launcher's: a short status on the left ("Release ⌘ to switch · 1–9 jumps", the match count while searching) and "Switch ↵" plus "Actions ⌘K" on the right. It can be turned off in the settings, which also carry a Shortcuts tab with the full list. Rows carry a badge when the window is minimized, its app is hidden, or it lives on another Space. Typed searches appear in a search row above the list with the matched text emphasized. On a trackpad the list works by hovering, clicking, scrolling, and right-clicking once the list stays open.

Terminate (F8) force-quits the selected window's app. Run (F9) starts another instance of it.

### Permissions

AltTabio asks for two permissions in **System Settings > Privacy & Security**:

- **Accessibility** to see Cmd+Tab before macOS does and to raise, minimize, and close windows.
- **Screen Recording** to draw the live preview. Without it the list still works; the preview area shows a hint instead.

Both can be opened from the settings window (menu bar icon > Settings). AltTabio picks up an Accessibility grant while it runs; after granting Screen Recording, quit and start it again so ScreenCaptureKit sees the change. When AltTabio is started from a terminal, macOS applies the terminal's grants to it.

### Building on macOS

Xcode is not required; the Command Line Tools and the Rust toolchain are enough.

```sh
scripts/mac/run.sh                 # build target/mac/AltTabio.app and start it with its log in the terminal
scripts/mac/run.sh -- --preview    # show the overlay once without taking over Cmd+Tab
scripts/mac/run.sh -- --list       # print the windows AltTabio sees and exit
scripts/mac/run.sh -- --settings   # start with the settings window open
scripts/mac/build-app.sh           # only build the bundle
```

The bundle is written to `target/mac/AltTabio.app`; copy it to `/Applications` to keep it. macOS ties Accessibility and Screen Recording grants to the app's code signature, and ad-hoc signatures change with every build. Run `scripts/mac/make-dev-cert.sh` once to create a self-signed certificate that the build script then uses, so the permissions survive rebuilds; macOS asks for the login password once while it marks the certificate as trusted.

`ALTTABIO_TRACE=1 scripts/mac/run.sh` prints every intercepted key event and switcher action to the terminal.

Settings live in `~/Library/Application Support/AltTabio/AltTabio.ini`, or in an `AltTabio.ini` next to the executable if one exists there. The file uses the same keys as the Windows build; `ReplaceAltTab` maps to Cmd+Tab and `ReplaceWinTab` to Option+Tab.

The debug helpers `--list` and `--activate <window id>` work from a plain `cargo run` as well.

## Building from source

AltTabio requires the Rust toolchain plus, on Windows, the MSVC build tools, or, on macOS, the Command Line Tools.

```powershell
cargo check --all-targets --message-format short
cargo clippy --all-targets --quiet --message-format short -- -D warnings
cargo test -q
cargo build --release
```

To inspect the overlay without installing global hooks:

```powershell
cargo run -- --preview
```

The release executable is written to:

```text
target\release\AltTabio.exe
```

## License

AltTabio is licensed under the [MIT License](LICENSE).

Third-party components and their licenses are listed in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
