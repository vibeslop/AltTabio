# AltTabio

**Website:** [https://vibeslop.github.io/AltTabio/](https://vibeslop.github.io/AltTabio/)
**Download:** [GitHub Releases](https://github.com/vibeslop/AltTabio/releases/latest)

AltTabio is a free, open-source **Alt+Tab replacement** and window switcher for 64-bit Windows 10 and Windows 11, and a **Cmd+Tab replacement** for macOS 26 and later. On Windows it shows a numbered list of open windows, a live preview of the selected window, and typed search. No ads, no account, no telemetry.

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

### Windows

1. Download the latest Windows archive from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases).
2. Extract it to a permanent folder that non-elevated processes cannot modify, such as `C:\Program Files\AltTabio`.
3. Run `AltTabio.exe` and accept the Windows administrator prompt.

AltTabio is distributed as one self-contained executable with no separate runtime installation.

AltTabio requires administrator privileges for its global input hooks and window-management commands.

Enabling **Autostart** creates a highest-privilege Windows scheduled task that launches the executable from its current location. If a non-elevated process can replace that file, it can gain administrator privileges at the next logon. Move AltTabio to an administrator-writable-only folder before enabling Autostart.

### macOS

AltTabio needs macOS 26 or later. Paste this line into Terminal:

```sh
curl -fsSL https://vibeslop.github.io/AltTabio/install.sh | sh
```

The script downloads the newest release that has a macOS build, checks its signature, puts `AltTabio.app` in `/Applications` (or in `~/Applications` when your account cannot write to `/Applications`), and starts it. AltTabio then asks for its two [permissions](#permissions) and runs from the menu bar. Run the same line again to update; the permissions carry over because every release is signed with the same certificate. An update that fails partway leaves the installed copy as it was.

AltTabio is not notarized by Apple, which takes a paid developer account, so macOS blocks a copy downloaded in a browser. A copy downloaded with curl, as the script does, is not blocked. To install by hand anyway:

1. Download `AltTabio-<version>-macos.zip` from [GitHub Releases](https://github.com/vibeslop/AltTabio/releases) and unzip it.
2. Move `AltTabio.app` to Applications and open it. When macOS says it could not verify the app, choose **Done**.
3. Open **System Settings > Privacy & Security**, click **Open Anyway** next to the message about AltTabio, and confirm.

To uninstall, turn off **Launch at login** in the settings, quit AltTabio from its menu bar icon, delete `AltTabio.app`, and remove its settings and permissions:

```sh
rm -rf ~/Library/Application\ Support/AltTabio
tccutil reset All com.vibeslop.AltTabio
```

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

The macOS build lists every window of the selected app, which is what people miss most in the system Cmd+Tab: minimized windows, windows of hidden apps, and windows on other Spaces are all listed and reachable. It is drawn on Liquid Glass and needs macOS 26 or later.

| Input | Action |
| --- | --- |
| Cmd+Tab | Open the switcher on the previous app; Tab and Shift+Tab step through the apps |
| Option+Tab | The same, when "Also open with ⌥ Tab" is on |
| ` (backtick), Left, Right | Previous or next app |
| Up, Down | The selected app's windows |
| 1-9 | Switch to that window of the selected app |
| Release Cmd, Return, or click | Switch to the selected window, or bring forward an app that has none |
| W or M while holding Cmd | Close or minimize the selected window |
| H or Q while holding Cmd | Hide or quit the selected app |
| Escape or a click outside | Close the switcher |
| Right-click | Window and app commands, including Force Quit |

### Permissions

AltTabio asks for permissions in **System Settings > Privacy & Security**:

- **Accessibility** to see Cmd+Tab before macOS does and to raise, minimize, and close windows.
- **Screen Recording**, only once **Show window previews** is on, to draw the preview. Without it the list still works; the preview area shows a hint instead.

Both can be opened from the settings window (menu bar icon > Settings). AltTabio picks up an Accessibility grant while it runs; after granting Screen Recording, quit and start it again so ScreenCaptureKit sees the change. When AltTabio is started from a terminal, macOS applies the terminal's grants to it.

### Remote desktop and VM clients

Apps such as Microsoft's Windows App, Parallels, or VMware take Cmd+Tab for their guest with a keyboard tap of their own. AltTabio puts its tap back in front of theirs every time the front app changes, so Cmd+Tab keeps opening the switcher inside those apps. The one thing no tap can get past is **secure keyboard input**: while a password field, a terminal with Secure Keyboard Entry, or a remote desktop client holds it, macOS delivers keys only to that app. AltTabio names the app in its log when that happens (`ALTTABIO_TRACE=1` shows every tap reinsertion too); the switcher works again as soon as the app releases it.

### Building on macOS

Xcode is not required; the Command Line Tools and the Rust toolchain are enough.

```sh
scripts/mac/run.sh                 # build target/mac/AltTabio.app and start it with its log in the terminal
scripts/mac/run.sh -- --preview    # show the overlay once without taking over Cmd+Tab
scripts/mac/run.sh -- --list       # print the windows AltTabio sees and exit
scripts/mac/run.sh -- --settings   # start with the settings window open
scripts/mac/build-app.sh           # only build the bundle
```

The bundle is written to `target/mac/AltTabio.app`; copy it to `/Applications` to keep it. macOS ties Accessibility and Screen Recording grants to the certificate the app is signed with, and ad-hoc signatures change with every build. Run `scripts/mac/make-signing-cert.sh` once to create a personal certificate that the build script then uses, so your grants survive rebuilds. If a build asks whether codesign may use the key, choose **Always Allow**.

`ALTTABIO_TRACE=1 scripts/mac/run.sh` prints every intercepted key event and switcher action to the terminal.

Settings live in `~/Library/Application Support/AltTabio/AltTabio.ini`, or in an `AltTabio.ini` next to the executable if one exists there. The file uses the same keys as the Windows build; `ReplaceAltTab` maps to Cmd+Tab and `ReplaceWinTab` to Option+Tab.

The debug helpers `--list` and `--activate <window id>` work from a plain `cargo run` as well.

### Releasing on macOS

Every release carries the same release certificate, so users keep their permissions across updates. Its SHA-1 hash is pinned in `scripts/mac/release-certificate.sha1`, and its key lives in the secrets of the repository's `release` environment, which only `v*` tags can use. A maintainer creates it once with `scripts/mac/make-signing-cert.sh --release`, commits the pin, and stores the backup the script writes, with its password, in the private [vibeslop/release-signing](https://github.com/vibeslop/release-signing) repository, encrypted to the maintainers' SSH keys. Whoever holds the key can sign an app that macOS treats as AltTabio, and a new certificate would make every user grant both permissions again.

To release, set the version in `Cargo.toml` and push the tag `v<version>`. The Release macOS workflow builds a universal bundle for Apple silicon and Intel Macs, signs it with the release certificate, and attaches `AltTabio-<version>-macos.zip` to the tag's release, creating a draft release when there is none. The install script picks the archive up once the release is published. `scripts/mac/package.sh` does the same by hand on a Mac that imported the certificate with `scripts/mac/make-signing-cert.sh --import <backup.p12>`, and refuses a bundle signed with any other certificate.

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
