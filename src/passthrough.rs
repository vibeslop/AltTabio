//! When local switching is bypassed for a maximized or fullscreen remote-desktop session.

use crate::input::HookSettings;

/// Whether `AltTabio` should intercept desktop switching for the current foreground window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PassthroughPolicy {
    /// `AltTabio` replaces Alt+Tab and Win+Tab as usual.
    #[default]
    Local,
    /// A maximized or fullscreen remote-desktop client owns Alt+Tab and Win+Tab.
    RemoteDesktopFullscreen,
}

impl PassthroughPolicy {
    /// Fresh hook threads start with local switching until the UI thread applies this policy.
    pub const INITIAL: Self = Self::Local;

    #[must_use]
    pub const fn from_foreground(is_rdp_client: bool, maximized_or_fullscreen: bool) -> Self {
        if is_rdp_client && maximized_or_fullscreen {
            Self::RemoteDesktopFullscreen
        } else {
            Self::Local
        }
    }

    #[must_use]
    pub const fn from_bypass_flag(bypass_local_switching: bool) -> Self {
        if bypass_local_switching {
            Self::RemoteDesktopFullscreen
        } else {
            Self::Local
        }
    }

    #[must_use]
    pub const fn bypasses_local_switching(self) -> bool {
        matches!(self, Self::RemoteDesktopFullscreen)
    }

    #[must_use]
    pub fn apply(self, settings: HookSettings) -> HookSettings {
        if !self.bypasses_local_switching() {
            return settings;
        }
        HookSettings {
            replace_alt_tab: false,
            replace_win_tab: false,
            ..settings
        }
    }
}

#[must_use]
pub fn is_remote_desktop_client(class_name: &str, executable_stem: &str) -> bool {
    const WINDOW_CLASSES: &[&str] = &[
        "TscShellContainerClass",
        "IHWindowClass",
        "UIMainClass",
        "TSSHELLWND",
    ];
    const EXECUTABLES: &[&str] = &["mstsc", "msrdc", "msrdcw"];
    WINDOW_CLASSES
        .iter()
        .any(|class| class.eq_ignore_ascii_case(class_name))
        || EXECUTABLES
            .iter()
            .any(|executable| executable.eq_ignore_ascii_case(executable_stem))
}

#[must_use]
pub fn window_fills_monitor(window: [i32; 4], monitor: [i32; 4]) -> bool {
    window[0] <= monitor[0]
        && window[1] <= monitor[1]
        && window[2] >= monitor[2]
        && window[3] >= monitor[3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{HookOutcome, HookState, Key, KeyEvent, Modifiers};

    const ALT: Modifiers = Modifiers {
        alt: true,
        left_windows: false,
        right_windows: false,
    };

    #[test]
    fn remote_desktop_clients_are_detected_by_window_class_or_executable() {
        assert!(is_remote_desktop_client(
            "TscShellContainerClass",
            "explorer"
        ));
        assert!(is_remote_desktop_client("IHWindowClass", "notepad"));
        assert!(is_remote_desktop_client("Chrome_WidgetWin_1", "mstsc"));
        assert!(is_remote_desktop_client("Chrome_WidgetWin_1", "msrdc"));
        assert!(!is_remote_desktop_client("Chrome_WidgetWin_1", "chrome"));
    }

    #[test]
    fn passthrough_applies_only_to_maximized_or_fullscreen_remote_desktop() {
        assert_eq!(
            PassthroughPolicy::from_foreground(true, false),
            PassthroughPolicy::Local
        );
        assert_eq!(
            PassthroughPolicy::from_foreground(true, true),
            PassthroughPolicy::RemoteDesktopFullscreen
        );
        assert_eq!(
            PassthroughPolicy::from_foreground(false, true),
            PassthroughPolicy::Local
        );
        assert!(!window_fills_monitor(
            [100, 100, 900, 700],
            [0, 0, 1920, 1080]
        ));
        assert!(window_fills_monitor([0, 0, 1920, 1080], [0, 0, 1920, 1080]));
    }

    #[test]
    fn fullscreen_rdp_policy_disables_tab_replacement_but_keeps_right_button_wheel() {
        let settings = PassthroughPolicy::RemoteDesktopFullscreen.apply(HookSettings::default());
        assert!(!settings.replace_alt_tab);
        assert!(!settings.replace_win_tab);
        assert!(settings.right_button_wheel_switching);
        assert!(
            PassthroughPolicy::Local
                .apply(HookSettings::default())
                .replace_alt_tab
        );
    }

    #[test]
    fn hook_threads_start_local_and_must_be_synced_to_the_current_foreground_policy() {
        assert_eq!(PassthroughPolicy::INITIAL, PassthroughPolicy::Local);
        let current = PassthroughPolicy::from_foreground(true, true);
        assert_ne!(PassthroughPolicy::INITIAL, current);

        let synced = current;
        let settings = synced.apply(HookSettings::default());
        assert!(!settings.replace_alt_tab);
        assert!(!settings.replace_win_tab);
        assert!(settings.right_button_wheel_switching);
    }

    #[test]
    fn recreating_hooks_without_resync_would_intercept_fullscreen_rdp() {
        let unsynced = PassthroughPolicy::INITIAL.apply(HookSettings::default());
        let mut state = HookState::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftAlt, ALT), unsynced)
                .suppress
        );

        let synced = PassthroughPolicy::from_foreground(true, true).apply(HookSettings::default());
        let mut state = HookState::default();
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), synced),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), synced),
            HookOutcome::default()
        );
    }

    #[test]
    fn bypass_flag_round_trips_the_named_policy() {
        assert_eq!(
            PassthroughPolicy::from_bypass_flag(true),
            PassthroughPolicy::RemoteDesktopFullscreen
        );
        assert!(PassthroughPolicy::RemoteDesktopFullscreen.bypasses_local_switching());
        assert!(!PassthroughPolicy::Local.bypasses_local_switching());
    }
}
