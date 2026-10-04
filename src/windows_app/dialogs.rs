use super::{App, AppHost, show_error_for_window};
use crate::{about_dialog, settings_dialog, startup};
use alttabio::input::HookSettings;
use alttabio::settings::Settings;
use alttabio::settings_change::{
    AutostartState, SettingsChange, SettingsEffects, switcher_session_settings,
};
use alttabio::theme::ResolvedTheme;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

impl AppHost {
    pub(super) fn show_settings(&self) {
        let Some((owner, mut dialog_settings)) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_settings_dialog())
        else {
            return;
        };

        let previous_autostart = match startup::status() {
            Ok(status) => {
                dialog_settings.general.autostart = status.enabled;
                status
            }
            Err(error) => {
                show_error_for_window(
                    owner,
                    &format!("Autostart status could not be read. {error}"),
                );
                startup::AutostartStatus {
                    enabled: dialog_settings.general.autostart,
                    task_exists: false,
                }
            }
        };
        let result = settings_dialog::show(owner, &dialog_settings);

        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish Settings because application state is busy");
            return;
        };
        app.settings_dialog_open = false;
        app.sync_hook_interception();
        match result {
            Ok(Some(settings)) => app.apply_settings(settings, previous_autostart),
            Ok(None) => {}
            Err(error) => app.show_error(&format!("Could not open settings: {error}")),
        }
    }

    pub(super) fn show_about(&self) {
        let Some((theme, icon)) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_about_dialog())
        else {
            return;
        };
        let result = about_dialog::show(theme, icon);

        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish About because application state is busy");
            return;
        };
        app.about_dialog_open = false;
        app.sync_hook_interception();
        if let Err(error) = result {
            app.show_error(&error);
        }
    }
}

impl App {
    fn prepare_settings_dialog(&mut self) -> Option<(HWND, Settings)> {
        if self.modal_state().dialog_open() {
            return None;
        }
        self.hide_overlay();
        self.settings_dialog_open = true;
        self.sync_hook_interception();
        Some((self.hwnd, self.settings.clone()))
    }

    fn prepare_about_dialog(&mut self) -> Option<(ResolvedTheme, alttabio::settings::IconColor)> {
        if self.modal_state().dialog_open() {
            return None;
        }
        self.hide_overlay();
        self.about_dialog_open = true;
        self.sync_hook_interception();
        Some((self.resolved_theme, self.settings.appearance.icon))
    }

    pub(super) fn request_modal_dialog(&self, message: u32, name: &str) {
        let result = unsafe {
            // SAFETY: self.hwnd is live and private dialog messages carry no borrowed data.
            PostMessageW(Some(self.hwnd), message, WPARAM(0), LPARAM(0))
        };
        if let Err(error) = result {
            eprintln!("Could not request the {name} dialog: {error}");
        }
    }

    fn apply_settings(&mut self, settings: Settings, previous_autostart: startup::AutostartStatus) {
        let previous_settings = self.settings.clone();
        let change = SettingsChange {
            previous: &previous_settings,
            next: &settings,
            autostart: AutostartState {
                enabled: previous_autostart.enabled,
                task_exists: previous_autostart.task_exists,
            },
            hooks_running: self.hooks.is_some(),
        };
        if let Err(message) = change.apply(self) {
            self.show_error(&message);
            return;
        }

        let icon_changed = settings.appearance.icon != previous_settings.appearance.icon;
        self.settings = settings;
        self.session
            .update_settings(switcher_session_settings(&self.settings));
        if icon_changed {
            let icon_result = self
                .tray
                .as_mut()
                .map(|tray| tray.set_icon(self.settings.appearance.icon))
                .transpose();
            if let Err(error) = icon_result {
                self.show_error(&format!("Could not update the tray icon. {error}"));
            }
        }
        if let Err(error) = self.refresh_theme() {
            self.show_error(&format!("Could not update the overlay theme. {error}"));
        }
        self.recreate_preview();
        self.request_redraw();
    }
}

impl SettingsEffects for App {
    fn set_autostart(&mut self, enabled: bool) -> std::result::Result<(), String> {
        startup::set_enabled(enabled)
    }

    fn save(&mut self, settings: &Settings) -> std::result::Result<(), String> {
        self.settings_store.save(settings)
    }

    fn restart_hooks(&mut self, settings: HookSettings) -> std::result::Result<(), String> {
        self.hooks = None;
        self.start_input_hooks(settings)
    }
}
