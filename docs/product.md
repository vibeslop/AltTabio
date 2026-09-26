# AltTabio

AltTabio is a free, open-source window switcher. On **64-bit Windows 10 and 11** it replaces **Alt+Tab** (and optionally Win+Tab) with a numbered list of open windows, a live preview, and search that starts the moment you type. On **macOS 26 and later** it replaces **Cmd+Tab** and lists every window of the selected app, including minimized windows, windows of hidden apps, and windows on other desktops.

- **Website:** https://vibeslop.github.io/AltTabio/
- **Windows page:** https://vibeslop.github.io/AltTabio/windows/
- **Mac page:** https://vibeslop.github.io/AltTabio/mac/
- **Download:** https://github.com/vibeslop/AltTabio/releases/latest
- **Source:** https://github.com/vibeslop/AltTabio
- **License:** MIT
- **Price:** $0. No trial, no ads, no account, no paid tier.
- **Version:** 1.1.0
- **Platforms:** 64-bit Windows 10 and 11; macOS 26 or later on Apple silicon and Intel
- **Language:** Native Rust. Win32, Direct2D, DirectWrite, and DWM on Windows; AppKit and ScreenCaptureKit on macOS.
- **Network:** None. The app does not connect to the internet and includes no analytics.

AltTabio is an independent project. It is not affiliated with Microsoft, Apple, or Alt+Tab Terminator. Windows is a trademark of Microsoft Corporation; macOS is a trademark of Apple Inc.

## What problem it solves

Windows Alt+Tab shows a strip of small thumbnails. With many windows open it is easy to land on the wrong document, browser window, or mail window. AltTabio keeps the Alt+Tab habit and adds a readable list, a large live preview, typed search, and number-key jumps.

The macOS Cmd+Tab switcher lists apps only. Reaching one particular window, a minimized one, or one on another desktop takes extra steps. AltTabio keeps Cmd+Tab and its habits, and lists the selected app's windows right in the switcher.

It is a practical answer for people searching for:

- Alt+Tab replacement for Windows 11 or Windows 10
- better Alt+Tab on Windows with search or live preview
- Alt+Tab Terminator alternative
- Cmd+Tab replacement for Mac
- Mac app switcher that shows all windows
- Cmd+Tab that shows minimized windows or windows on other desktops
- Windows-style Alt+Tab on a Mac
- close, minimize, or force-quit a window from the switcher

## Features on Windows

- Replaces Alt+Tab and, optionally, Win+Tab
- Application icons, window titles, optional application names, optional numbers
- Live preview of the selected window, or a full-desktop preview that shows it in place
- Compact task list by default so the preview has room
- Type to filter by window title, application name, or number
- Keys 1-9 switch to the corresponding visible window
- Keyboard and mouse control, including right mouse button plus wheel switching
- Close, minimize, maximize, restore, end-process, and run-another-instance commands
- Optional current-monitor filter for multi-monitor setups
- Light, dark, or follow-Windows theme
- Eight app-icon colors (Azure default, plus Copper, Ember, Indigo, Orchid, Rosewood, Vermilion, Violet)
- Portable settings in `AltTabio.ini` next to the executable
- Optional start with Windows via a scheduled task
- Single self-contained `AltTabio.exe`

## Features on macOS

- Replaces Cmd+Tab and, optionally, Option+Tab
- The first Tab lands on the previous app, as in macOS; releasing Cmd switches
- Running apps along the top, most recently used first; every window of the selected app below
- Minimized windows, windows of hidden apps, and windows on other desktops are listed and marked
- Keys 1-9 switch to that window of the selected app
- W, M, H, and Q close, minimize, hide, or quit while Cmd is held; the right-click menu adds Force Quit
- Optional live preview of the selected window (needs Screen Recording)
- Optional filter for the current display
- Drawn on Liquid Glass, with a system, light, or dark appearance
- Keeps working inside remote desktop and virtual machine apps that capture Cmd+Tab
- Universal app for Apple silicon and Intel; launch at login through the system

## Install

### Windows

1. Download the latest Windows archive from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest).
2. Extract it to a folder you will keep. For autostart, use a folder that non-elevated processes cannot modify, such as `C:\Program Files\AltTabio`.
3. Run `AltTabio.exe` and accept the Windows administrator prompt.

Windows asks for permission because AltTabio installs a global input hook (so it can handle Alt+Tab before Windows does) and window-management commands (so it can close a frozen program when you ask). That approval is for those jobs only.

To uninstall: turn off start-with-Windows if you enabled it, exit from the tray icon, and delete the folder.

### macOS

Paste this line into Terminal:

```sh
curl -fsSL https://vibeslop.github.io/AltTabio/install.sh | sh
```

The script downloads the newest release, checks its signature, puts `AltTabio.app` in `/Applications` (or `~/Applications`), and starts it. Run the same line again to update. AltTabio asks for **Accessibility**, to take over Cmd+Tab and control windows, and for **Screen Recording** only once window previews are turned on. Every release is signed with the same certificate, so the permissions carry over to updates.

AltTabio is not notarized by Apple, so a copy downloaded in a browser has to be allowed with **Open Anyway** in System Settings, Privacy & Security. The install script avoids that.

To uninstall: turn off Launch at login, quit from the menu bar icon, delete `AltTabio.app`, then run:

```sh
rm -rf ~/Library/Application\ Support/AltTabio
tccutil reset All com.vibeslop.AltTabio
```

## Keyboard and mouse on Windows

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

## Keyboard and mouse on macOS

| Input | Action |
| --- | --- |
| Cmd+Tab | Open the switcher on the previous app; Tab and Shift+Tab step through apps |
| Option+Tab | The same, when "Also open with ⌥ Tab" is on |
| Backtick, Left, Right | Previous or next app |
| Up, Down | The selected app's windows |
| 1-9 | Switch to that window of the selected app |
| Release Cmd, Return, or click | Switch to the selected window, or bring forward an app that has none |
| W or M while holding Cmd | Close or minimize the selected window |
| H or Q while holding Cmd | Hide or quit the selected app |
| Escape or a click outside | Close the switcher |
| Right-click | Window and app commands, including Force Quit |

## Privacy

AltTabio contains no telemetry, no ads, and no network code for product use. Settings stay in a local INI file. Window previews are drawn in memory and never saved or sent. Source code is public so this can be verified.

## FAQ

**Is it really free?** Yes. MIT-licensed, no trial, no ads, no account, no premium version.

**Will it slow the computer down?** It is one small native program. It waits until you press the shortcut. There is no background sync and no phoning home.

**Does it collect data?** No.

**How do I get the built-in switcher back?** On Windows, right-click the tray icon and choose Exit; you can also replace only Alt+Tab or only Win+Tab. On a Mac, turn off "Replace ⌘ Tab" in the settings or quit from the menu bar icon.

**Which computers?** Any 64-bit Windows 10 or 11 PC, including multi-monitor and high-DPI setups, and any Mac with macOS 26 or later.

## Related searches this page should answer

AltTabio; Alt Tabio; Alt+Tab replacement Windows 11; Windows 10 window switcher; live preview Alt+Tab; type to search open windows; number keys to switch windows; open source Alt+Tab; Alt+Tab Terminator alternative; Task Manager close from switcher; portable Windows switcher; Cmd+Tab replacement Mac; macOS app switcher show all windows; Cmd+Tab minimized windows; Cmd+Tab windows on other desktops; Alt+Tab for Mac; switch between windows of the same app on Mac.
