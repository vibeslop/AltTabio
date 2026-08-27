# AltTabio

AltTabio is a free, open-source **Alt+Tab replacement** for **64-bit Windows 10 and Windows 11**. It replaces the standard Windows Alt+Tab (and optionally Win+Tab) switcher with a numbered list of open windows, a live preview, and search that starts the moment you type.

- **Website:** https://vibeslop.github.io/AltTabio/
- **Download:** https://github.com/vibeslop/AltTabio/releases/latest
- **Source:** https://github.com/vibeslop/AltTabio
- **License:** MIT
- **Price:** $0. No trial, no ads, no account, no paid tier.
- **Version:** 1.0.3
- **Platform:** 64-bit Windows
- **Language:** Native Rust on Windows APIs (Win32, Direct2D, DirectWrite, DWM)
- **Network:** None. The app does not connect to the internet and includes no analytics.

AltTabio is an independent project and is not affiliated with Alt+Tab Terminator. Windows is a trademark of Microsoft Corporation.

## What problem it solves

Windows Alt+Tab shows a strip of small thumbnails. With many windows open it is easy to land on the wrong document, browser tab, or mail window. AltTabio keeps the Alt+Tab habit and adds a readable list, a large live preview, typed search, and number-key jumps.

It is a practical alternative for people searching for:

- Alt+Tab replacement for Windows
- better Alt+Tab on Windows 11
- Windows window switcher with live preview
- Alt+Tab Terminator alternative
- task switcher that can search open windows
- close or kill a frozen window from Alt+Tab

## Features

- Replaces Alt+Tab and, optionally, Win+Tab
- Application icons, window titles, optional application names, optional numbers
- Live preview of the selected window, or a full-desktop preview that shows it in place
- Compact task list by default so the preview has room
- Type to filter by window title, application name, or number
- Keys 1-9 activate the corresponding visible window
- Keyboard and mouse control, including right mouse button plus wheel switching
- Close, minimize, maximize, restore, terminate, and run-another-instance commands
- Optional current-monitor filter for multi-monitor setups
- Light, dark, or follow Windows theme
- Eight app-icon colors (Azure default, plus Copper, Ember, Indigo, Orchid, Rosewood, Vermilion, Violet)
- Portable settings in `AltTabio.ini` next to the executable
- Optional start with Windows via a scheduled task
- Single self-contained `AltTabio.exe`

## Install

1. Download the latest Windows archive from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest).
2. Extract it to a folder you will keep. For autostart, use a folder that non-elevated processes cannot modify, such as `C:\Program Files\AltTabio`.
3. Run `AltTabio.exe` and accept the Windows administrator prompt.

Windows asks for permission because AltTabio installs a global input hook (so it can handle Alt+Tab before Windows does) and window-management commands (so it can close a frozen program when you ask). That approval is for those jobs only.

To uninstall: turn off start-with-Windows if you enabled it, exit from the tray icon, and delete the folder.

## Keyboard and mouse

| Input | Action |
| --- | --- |
| Alt+Tab or Win+Tab | Open and move through the switcher |
| Arrow keys, Tab, Shift+Tab, or mouse wheel | Change the selected window |
| Enter or left click | Activate the selected window |
| 1-9 or numpad 1-9 | Activate that visible window |
| Type | Filter by title, application name, or number |
| Backspace | Remove a filter character |
| Home or End | First or last window |
| Escape | Close the switcher |
| F4 | Close the selected window |
| F5 | Minimize |
| F6 | Maximize |
| F7 | Restore |
| F8 | Terminate the selected window's process |
| F9 | Launch another instance of the selected app |
| Right click | Window commands for the selected row |

## Privacy

AltTabio contains no telemetry, no ads, and no network stack for product use. Settings stay in a local INI file. Source code is public so this can be verified.

## FAQ

**Is it really free?** Yes. MIT-licensed, no trial, no ads, no account, no premium version.

**Will it slow the PC down?** It is one small native executable. It waits until you press Alt+Tab. There is no background sync and no phoning home.

**Does it collect data?** No.

**How do I get stock Alt+Tab back?** Right-click the tray icon and choose Exit. You can also replace only Alt+Tab or only Win+Tab.

**Which PCs?** Any modern 64-bit Windows PC, including multi-monitor and high-DPI displays.

## Related searches this page should answer

AltTabio; Alt Tabio; Alt+Tab replacement Windows 11; Windows 10 window switcher; live preview Alt+Tab; type to search open windows; number keys to switch windows; open source Alt+Tab; Alt+Tab Terminator alternative; Task Manager close from switcher; portable Windows switcher.
