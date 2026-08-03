# AltTabio

AltTabio is an open-source application for 64-bit Windows, inspired by Alt+Tab Terminator.

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
2. Extract it to a permanent folder.
3. Run `AltTabio.exe` and accept the Windows administrator prompt.

AltTabio is distributed as one self-contained executable with no separate runtime installation.

AltTabio requires administrator privileges for its global input hooks and window-management commands.

Enabling **Autostart** creates a Windows scheduled task that launches the executable from its current location, so move it to its permanent folder first.

## Implementation

AltTabio is a native Rust application built directly on Windows APIs. It uses low-level keyboard and mouse hooks, a hand-written Win32 overlay, Direct2D/DirectWrite rendering, and DWM previews of the selected window or its full desktop. Window enumeration and management, the tray icon, settings UI, scheduled-task autostart, portable settings, and single-instance enforcement are also implemented natively.

The build produces one `AltTabio.exe` with its icon, version information, per-monitor DPI manifest, and administrator requirement embedded.

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
| F9 | Launch another instance of the selected application |

The tray icon provides access to settings and exit. Right-clicking the selected row opens its window-command menu.

## Settings

Settings include automatic startup; independent Alt+Tab and Win+Tab replacement; typed search; switching when Alt or the right mouse button is released; right mouse button + wheel switching; mouse-over selection; an Azure-default choice of eight app-icon colors; compact list density; large icons; number labels; optional application names; visible borders; live and full-desktop previews; and filtering by the current monitor.

Settings are stored in `AltTabio.ini` next to `AltTabio.exe`.

## Building from source

AltTabio requires Windows, the Rust toolchain, and the MSVC build tools.

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
