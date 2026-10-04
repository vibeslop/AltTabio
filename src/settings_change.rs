//! Applying a confirmed Settings dialog to the subsystems that read it.

use crate::input::HookSettings;
use crate::settings::Settings;
use crate::switcher::SwitcherSessionSettings;

/// Search becomes active only while the overlay is open, so hooks always start without it.
#[must_use]
pub fn hook_settings(settings: &Settings) -> HookSettings {
    HookSettings {
        replace_alt_tab: settings.general.replace_alt_tab,
        replace_win_tab: settings.general.replace_win_tab,
        right_button_wheel_switching: settings.general.right_button_wheel_switching,
        typed_search: settings.general.typed_search,
        search_active: false,
    }
}

#[must_use]
pub const fn switcher_session_settings(settings: &Settings) -> SwitcherSessionSettings {
    SwitcherSessionSettings {
        typed_search: settings.general.typed_search,
        release_alt_switches: settings.general.release_alt_switches,
        release_right_button_switches: settings.general.release_right_button_switches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_settings_copy_the_switches_and_start_without_search() {
        let mut settings = Settings::default();
        settings.general.replace_alt_tab = false;
        settings.general.replace_win_tab = true;
        settings.general.right_button_wheel_switching = true;
        settings.general.typed_search = false;

        assert_eq!(
            hook_settings(&settings),
            HookSettings {
                replace_alt_tab: false,
                replace_win_tab: true,
                right_button_wheel_switching: true,
                typed_search: false,
                search_active: false,
            }
        );
    }

    #[test]
    fn session_settings_copy_search_and_release_switches() {
        let mut settings = Settings::default();
        settings.general.typed_search = true;
        settings.general.release_alt_switches = false;
        settings.general.release_right_button_switches = true;

        assert_eq!(
            switcher_session_settings(&settings),
            SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: false,
                release_right_button_switches: true,
            }
        );
    }
}
