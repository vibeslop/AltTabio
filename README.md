# AltTabio

A free, open-source window switcher. It replaces Alt+Tab on 64-bit Windows 10 and 11 and Cmd+Tab on macOS 26 and later.

[Website](https://vibeslop.github.io/AltTabio/) · [Download](https://github.com/vibeslop/AltTabio/releases/latest)

![AltTabio on Windows, showing a numbered list of File Explorer windows beside a live preview of the selected one](docs/images/hero-switcher.png)

On Windows it shows a list of open windows and a live preview of the selected one, and filters the list as you type. Number labels for the first nine rows are optional and hidden by default. On macOS it lists every window of the selected app, including minimized windows, windows of hidden apps, and windows on other Spaces, which the system Cmd+Tab leaves out.

It was created because Alt+Tab Terminator has had a critical issue for years that causes Alt+Tab to stop working correctly. The issue was reported, but never fixed.

AltTabio is an independent project and is not affiliated with Alt+Tab Terminator.

## Install

### Windows

1. Download the Windows archive from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest).
2. Extract it to a folder that only administrators can change, such as `C:\Program Files\AltTabio`.
3. Run `AltTabio.exe` and accept the administrator prompt.

AltTabio needs administrator rights. With **Autostart** on, it gets them at every logon, and so would anyone who replaced `AltTabio.exe`. That is why it belongs in a folder only administrators can change. Autostart and the Windows updater both refuse an executable that non-elevated processes can replace.

Release builds check GitHub for updates about a minute after startup and then daily, download newer releases, verify the Windows archive's Minisign signature with a public key embedded in the app and GitHub's SHA-256 digest, and restart after five minutes without keyboard or mouse input, once the switcher and dialogs are closed. The signature also binds the version, Windows x64 platform and archive name. Missing or invalid signatures stop the update before extraction. You can turn off automatic updates in Settings and still use **Check for Updates** or **Relaunch to Update** in the tray menu. Updates replace the executable in its current folder and preserve `AltTabio.ini` and the startup task. Ordinary Windows builds from source do not check for or install published updates.

When upgrading to 1.3.0, an existing `AutoUpdate=true` enables Windows updates; earlier Windows versions saved the key but ignored it. Signing is required by builds containing this change; older executables do not gain signature enforcement retroactively. This is update authenticity, and does not change SmartScreen or the administrator prompt. The initial manual download still needs a trusted source. See the [Windows updater trust model](docs/windows-updater.md).

### macOS

Paste this into Terminal:

```sh
curl -fsSL https://vibeslop.github.io/AltTabio/install.sh | sh
```

The script installs AltTabio in `/Applications` and starts it in the menu bar. AltTabio asks for [two permissions](https://vibeslop.github.io/AltTabio/mac/#permissions): Accessibility right away, and Screen Recording for previews after your first Cmd+Tab. It updates itself from then on.

AltTabio is not notarized, so macOS blocks a copy downloaded in a browser. The [Mac page](https://vibeslop.github.io/AltTabio/mac/#install) explains how to install it by hand anyway, and how to remove it.

## Keys

### Windows

| Input | Action |
| --- | --- |
| Alt+Tab or Win+Tab | Open the switcher and move through it |
| Up, Down, Tab, or Shift+Tab | Change the selected window |
| Left, Right, Home, or End | Edit an active query; otherwise navigate the windows |
| Mouse wheel | Over the left list, scroll without changing selection; everywhere else, including outside the open switcher, select the previous or next window. Background apps do not scroll. |
| Enter or click | Switch to the selected window |
| 1-9, on the number row or the number pad | Switch to that window directly when the query is empty; otherwise type the digit |
| Type | Filter by window title, app name, or number |
| Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V | Select, copy, cut, or paste query text |
| Ctrl+Z or Ctrl+Y | Undo or redo a query edit |
| Escape or click outside | Close the switcher |
| F4 to F9 | Close, minimize, maximize, or restore the selected window, end its process, or start another instance of its app |

### macOS

| Input | Action |
| --- | --- |
| Cmd+Tab | Open the switcher on the previous app; Tab and Shift+Tab step through the apps |
| Left, Right, or ` (backtick) | Previous or next app |
| Up, Down | The selected app's windows |
| 1-9 | Switch to that window of the selected app |
| Release Cmd, Return, or click | Switch to the selected window |
| W, M, H, or Q while holding Cmd | Close or minimize the window, or hide or quit the app |
| Right-click | More commands, including Force Quit |

Settings open from the tray icon on Windows and from the menu bar icon on macOS. The website lists every shortcut and setting for [Windows](https://vibeslop.github.io/AltTabio/windows/#shortcuts) and [macOS](https://vibeslop.github.io/AltTabio/mac/#shortcuts).

The interface supports English, Russian, and Simplified Chinese. Choose a language in Settings, or leave **System** selected to use the first supported language in your system's preferred languages, with English as the fallback. On Windows, language changes preview immediately and are kept with **Save changes**; **Cancel** restores the previous language. macOS settings apply immediately.

## Building

```sh
cargo build --release   # Windows: target\release\AltTabio.exe
scripts/mac/run.sh      # macOS: build target/mac/AltTabio.app and start it
```

[CONTRIBUTING.md](CONTRIBUTING.md) covers the checks a change must pass, the debug flags, and releasing.

## License

AltTabio is licensed under the [MIT License](LICENSE). Third-party components and their licenses are listed in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
