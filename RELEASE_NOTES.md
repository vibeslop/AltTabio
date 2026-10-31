# AltTabio 1.3.0

## Windows

Windows release builds now check GitHub for updates, download newer stable releases, and restart after five minutes without keyboard or mouse input. An open switcher, menu, or dialog delays the restart. Local builds from source do not install published updates over themselves.

**Upgrade notice:** Windows now reads the existing `AutoUpdate` setting. Older Windows versions saved `AutoUpdate=true` by default but ignored it. Upgrading with that value enables background network checks and automatic installation. Turn off automatic updates in Settings to use only the tray's manual update commands.

Updates require an installation that non-elevated processes cannot replace. They preserve the settings file and autostart task. Elevated helpers ignore user curl configuration, inherit only the required environment, and load PowerShell modules from Windows system paths.

Windows currently verifies archive bytes against the SHA-256 digest supplied by GitHub. This is not an independent publisher signature: Windows updates trust the repository's release-publishing permissions, and installed updates run with AltTabio's administrator rights.

A busy app may display its class or executable icon instead of its per-window icon. The switcher uses a short icon-query timeout to stay responsive.

## macOS

The existing macOS release certificate is retained.
