//! The settings window, applying its changes, and the login item.

use super::App;
use crate::macos::alerts::show_fatal_error;
use crate::macos::autostart;
use crate::macos::hotkey::HotkeySettings;
use crate::macos::permissions;
use crate::macos::runtime::{run_later, with_app};
use crate::macos::settings_window::{SettingsEvent, SettingsWindow};
use alttabio::settings::Settings;
use alttabio::theme::SwitcherTokens;
use std::rc::Rc;

impl App {
    pub(crate) fn show_settings(&mut self) {
        if self.settings_window.is_none() {
            let handler: Rc<dyn Fn(SettingsEvent)> = Rc::new(|event| match event {
                SettingsEvent::Changed(settings) => {
                    let _ = with_app(|app| app.apply_settings(settings));
                }
                SettingsEvent::Autostart(enabled) => {
                    let _ = with_app(|app| app.set_autostart(enabled));
                }
                SettingsEvent::OpenAccessibility => permissions::open_accessibility_settings(),
                SettingsEvent::OpenScreenRecording => {
                    permissions::open_screen_recording_settings();
                }
            });
            self.settings_window = Some(SettingsWindow::new(self.mtm, &self.settings, handler));
        }
        if let Some(window) = &self.settings_window {
            window.show(&self.settings);
        }
    }

    fn apply_settings(&mut self, settings: Settings) {
        let previews_turned_on = settings.appearance.preview && !self.settings.appearance.preview;
        let updates_changed = settings.general.auto_update != self.settings.general.auto_update;
        self.settings = settings;
        self.hotkey_settings = hotkey_settings(&self.settings);
        self.save_settings();
        if previews_turned_on {
            self.ask_for_screen_recording();
        }
        if updates_changed {
            if self.settings.general.auto_update {
                self.update_check_due();
            } else if let Some(timer) = self.update_timer.take() {
                timer.invalidate();
            }
        }
        if !self.settings.appearance.preview {
            self.preview.clear();
            self.previews.clear();
        }
        if self.switcher.is_active() {
            if let Some(overlay) = &self.overlay {
                let theme = self.resolved_theme();
                overlay.set_theme(theme, SwitcherTokens::new(theme));
            }
            self.redraw();
            self.request_preview_capture();
        }
    }

    /// Registers or removes the login item. The system owns that state, so the settings file
    /// records whatever it reports afterwards rather than what was asked for.
    fn set_autostart(&mut self, enabled: bool) {
        if let Err(error) = autostart::set_enabled(enabled) {
            let mtm = self.mtm;
            run_later(move || show_fatal_error(mtm, &error));
        }
        self.settings.general.autostart = autostart::is_enabled();
        self.save_settings();
    }

    /// The defaults promise launch at login, but only a registration makes it true. Saving right
    /// away marks the first start as done, so a login item the user later removes in System
    /// Settings stays removed.
    pub(crate) fn complete_first_start(&mut self) {
        if self.settings.general.autostart
            && let Err(error) = autostart::set_enabled(true)
        {
            eprintln!("{error}");
        }
        self.settings.general.autostart = autostart::is_enabled();
        self.save_settings();
    }

    fn save_settings(&mut self) {
        if let Err(error) = self.store.save(&self.settings) {
            eprintln!("{error}");
        }
    }
}

pub(super) fn hotkey_settings(settings: &Settings) -> HotkeySettings {
    HotkeySettings {
        command_tab: settings.general.replace_alt_tab,
        option_tab: settings.general.replace_win_tab,
    }
}
