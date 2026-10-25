use super::{App, AppHost, show_error_for_window};
use crate::{about_dialog, dialog_host, settings_dialog, startup};
use alttabio::input::HookSettings;
use alttabio::settings::Settings;
use alttabio::settings_change::{
    AutostartState, SettingsChange, SettingsEffects, switcher_session_settings,
};
use alttabio::theme::ResolvedTheme;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, PostMessageW, SW_RESTORE, SW_SHOW, ShowWindow,
};

impl AppHost {
    pub(super) fn show_settings(&self) {
        if let Some(window) = self.settings_window.get() {
            unsafe {
                // SAFETY: the Settings dialog owns this window on this UI thread until its
                // nested loop returns. The result reports previous visibility, not failure.
                let command = if IsIconic(window).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                };
                let _was_visible = ShowWindow(window, command);
            }
            dialog_host::bring_to_front(window, "settings");
            return;
        }
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
                AutostartState {
                    enabled: dialog_settings.general.autostart,
                    task_exists: false,
                }
            }
        };
        let result = settings_dialog::show(owner, &dialog_settings, |window| {
            self.settings_window.set(Some(window));
        });
        self.settings_window.set(None);

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

    fn apply_settings(&mut self, settings: Settings, previous_autostart: AutostartState) {
        let previous_settings = self.settings.clone();
        let change = SettingsChange {
            previous: &previous_settings,
            next: &settings,
            autostart: previous_autostart,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_messages::{WM_SHOW_SETTINGS, WM_TRAY_CALLBACK};
    use crate::settings_io::SettingsStore;
    use crate::win32::{ComApartment, module_instance};
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsIconic, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW,
        SW_HIDE, SW_MINIMIZE, ShowWindow, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WS_EX_TOOLWINDOW,
        WS_OVERLAPPEDWINDOW,
    };
    use windows::core::{Error, Result, w};

    struct TestWindow(HWND);

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: this test uniquely owns the window on the creating thread.
            assert!(unsafe { DestroyWindow(self.0) }.is_ok());
        }
    }

    #[test]
    fn tray_show_during_settings_keeps_the_disabled_overlay_hidden() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let window = TestWindow(unsafe {
            // SAFETY: STATIC is a system class, and the test owns the resulting window.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Modal switcher test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(module_instance()?),
                None,
            )
        }?);
        let (store, settings) = SettingsStore::load_from(
            std::env::temp_dir().join(format!(
                "alttabio-modal-switcher-test-{}.ini",
                std::process::id()
            )),
            &Settings::default(),
        )
        .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error))?;
        let mut app = App::new(false, false, settings, store)?;
        app.hwnd = window.0;
        app.settings_dialog_open = true;
        let _owner_guard = dialog_host::OwnerGuard::disable(window.0);

        for tray_message in [WM_LBUTTONUP, WM_LBUTTONDBLCLK] {
            let tray_message = isize::try_from(tray_message).map_err(|error| {
                Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
            })?;
            app.handle_message(WM_TRAY_CALLBACK, WPARAM(0), LPARAM(tray_message));
            assert!(
                !app.is_visible(),
                "a modal dialog must keep the session hidden"
            );
            let mut message = MSG::default();
            unsafe {
                // SAFETY: these calls query the live test-owned window and its posted messages.
                assert!(!IsWindowVisible(window.0).as_bool());
                assert!(!IsWindowEnabled(window.0).as_bool());
                assert!(
                    PeekMessageW(
                        &raw mut message,
                        Some(window.0),
                        WM_SHOW_SETTINGS,
                        WM_SHOW_SETTINGS,
                        PM_REMOVE,
                    )
                    .as_bool(),
                    "Show must request the existing Settings window instead of the disabled overlay"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn repeated_settings_request_shows_and_restores_the_existing_window() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let window = TestWindow(unsafe {
            // SAFETY: STATIC is a system class, and the test owns the resulting window.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Settings activation test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(module_instance()?),
                None,
            )
        }?);
        let (store, settings) = SettingsStore::load_from(
            std::env::temp_dir().join(format!(
                "alttabio-settings-activation-test-{}.ini",
                std::process::id()
            )),
            &Settings::default(),
        )
        .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error))?;
        let mut app = App::new(false, false, settings, store)?;
        app.settings_dialog_open = true;
        let host = AppHost::new(app);
        host.settings_window.set(Some(window.0));

        for state in [SW_HIDE, SW_MINIMIZE] {
            unsafe {
                // SAFETY: window stays live and belongs to this test thread; the result is
                // previous visibility rather than success.
                let _was_visible = ShowWindow(window.0, state);
            }
            host.show_settings();
            unsafe {
                // SAFETY: these queries only read the live test window's state.
                assert!(
                    IsWindowVisible(window.0).as_bool(),
                    "invoking Settings again must show the existing window"
                );
                assert!(
                    !IsIconic(window.0).as_bool(),
                    "invoking Settings again must restore the existing window"
                );
            }
            assert_eq!(host.settings_window.get(), Some(window.0));
            assert!(host.state.borrow().settings_dialog_open);
        }
        Ok(())
    }
}
