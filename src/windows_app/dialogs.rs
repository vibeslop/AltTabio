use super::{App, AppHost, show_error_for_window};
use crate::{dialog_host, settings_dialog, startup};
use alttabio::input::HookSettings;
use alttabio::settings::Settings;
use alttabio::settings_change::{
    AutostartState, SettingsChange, SettingsEffects, switcher_session_settings,
};
use alttabio::settings_form::Category;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, PostMessageW, PostQuitMessage, SW_RESTORE, SW_SHOW, ShowWindow,
};

#[cfg(test)]
#[path = "settings_open_tests.rs"]
mod first_open_tests;

impl AppHost {
    pub(super) fn show_settings(&self) {
        self.open_settings(None);
    }

    pub(super) fn show_about(&self) {
        self.open_settings(Some(Category::About));
    }

    fn open_settings(&self, category: Option<Category>) {
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
            if let Some(category) = category
                && let Err(error) = settings_dialog::navigate(window, category)
            {
                show_error_for_window(
                    window,
                    &format!("Could not select the Settings page: {error}"),
                );
            }
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
        let result = settings_dialog::show(
            owner,
            &dialog_settings,
            category.unwrap_or_default(),
            |window| {
                self.settings_window.set(Some(window));
            },
            |message| {
                if !super::update::is_update_timer(message, owner) {
                    return false;
                }
                // The Settings loop has no state borrow here. Work may proceed, while the
                // settings_dialog flag still prevents committing an update restart.
                if self.update_tick() {
                    // SAFETY: propagate a requested application exit through the nested loop.
                    unsafe { PostQuitMessage(0) };
                }
                true
            },
        );
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
}

impl App {
    fn prepare_settings_dialog(&mut self) -> Option<(HWND, Settings)> {
        if self.dialog_state().dialog_open() {
            return None;
        }
        self.hide_overlay();
        self.settings_dialog_open = true;
        self.sync_hook_interception();
        Some((self.hwnd, self.settings.clone()))
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
    use crate::app_messages::{UPDATE_TIMER_ID, WM_TRAY_CALLBACK};
    use crate::settings_io::SettingsStore;
    use crate::win32::{ComApartment, module_instance};
    use windows::Win32::UI::Input::KeyboardAndMouse::{IsWindowEnabled, VK_ESCAPE, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GW_OWNER, GetWindow, IsIconic, IsWindowVisible, SW_HIDE,
        SW_MINIMIZE, ShowWindow, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_TIMER,
        WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
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
    fn settings_nested_loop_delivers_owner_update_timers() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let owner = TestWindow(unsafe {
            // SAFETY: STATIC is a built-in class, with no callback state or retained pointers.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Update timer owner"),
                WS_POPUP,
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
        let observed = std::cell::Cell::new(false);
        let on_open = |window| unsafe {
            // SAFETY: both windows are owned by this test; posted events carry only scalar IDs.
            assert!(
                PostMessageW(Some(owner.0), WM_TIMER, WPARAM(UPDATE_TIMER_ID), LPARAM(0)).is_ok()
            );
            assert!(PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok());
        };
        let result = settings_dialog::show(
            owner.0,
            &Settings::default(),
            Category::General,
            on_open,
            |message| {
                if super::super::update::is_update_timer(message, owner.0) {
                    observed.set(true);
                    true
                } else {
                    false
                }
            },
        )?;
        assert!(result.is_none());
        assert!(observed.get(), "Settings consumed the app's update timer");
        Ok(())
    }

    #[test]
    fn settings_keeps_switcher_open_select_and_cancel_available() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let (store, settings) = SettingsStore::load_from(
            std::env::temp_dir().join(format!(
                "alttabio-settings-switcher-test-{}.ini",
                std::process::id()
            )),
            &Settings::default(),
        )
        .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error))?;
        let host = Box::new(AppHost::new(App::new(false, false, settings, store)?));
        let instance = module_instance()?;
        super::super::register_window_class(instance)?;
        let overlay = TestWindow(unsafe {
            // SAFETY: the registered production procedure receives this stable host allocation,
            // which outlives the test-owned window and is never borrowed across a native loop.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                super::super::WINDOW_CLASS,
                super::super::WINDOW_TITLE,
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(instance),
                Some(std::ptr::from_ref(host.as_ref()).cast()),
            )
        }?);
        let Some((owner, settings)) = host.state.borrow_mut().prepare_settings_dialog() else {
            panic!("Settings must open from an idle switcher");
        };
        let on_open = |settings_window: HWND| {
            unsafe {
                // SAFETY: both windows are live and belong to this test thread.
                assert!(
                    IsWindowEnabled(owner).as_bool(),
                    "Settings must not disable the switcher"
                );
                assert!(
                    GetWindow(settings_window, GW_OWNER).is_err(),
                    "Settings must stay independent of the overlay's Z-order band"
                );
            }
            for tray_message in [WM_LBUTTONUP, WM_LBUTTONDBLCLK] {
                let tray_message = isize::try_from(tray_message)
                    .unwrap_or_else(|error| panic!("invalid tray message: {error}"));
                let mut app = host.state.borrow_mut();
                app.handle_message(WM_TRAY_CALLBACK, WPARAM(0), LPARAM(tray_message));
                assert!(
                    app.is_visible(),
                    "the tray command must open the switcher during Settings"
                );
                let Some(position) = app
                    .session
                    .switcher()
                    .positioned_visible_tasks()
                    .find(|(_, task)| task.window_handle == settings_window.0 as isize)
                    .map(|(position, _)| position)
                else {
                    panic!("the open Settings window must remain switchable");
                };
                assert!(app.session.switcher_mut().select_visible_position(position));
                app.handle_message(WM_KEYDOWN, WPARAM(usize::from(VK_RETURN.0)), LPARAM(0));
                assert!(!app.is_visible(), "Enter must choose the selected window");
                unsafe {
                    // SAFETY: Settings remains live through this callback and its nested loop.
                    assert!(IsWindowVisible(settings_window).as_bool());
                }
            }
            host.state.borrow_mut().handle_message(
                WM_TRAY_CALLBACK,
                WPARAM(0),
                LPARAM(isize::try_from(WM_LBUTTONUP).unwrap_or_default()),
            );
            assert!(host.state.borrow().is_visible());
            unsafe {
                // SAFETY: the posted keys and close request carry no borrowed data. The real
                // Settings loop must dispatch Escape to the overlay before closing Settings.
                assert!(
                    PostMessageW(
                        Some(owner),
                        WM_KEYDOWN,
                        WPARAM(usize::from(VK_ESCAPE.0)),
                        LPARAM(0)
                    )
                    .is_ok()
                );
                assert!(
                    PostMessageW(Some(settings_window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
                );
            }
        };
        let result =
            settings_dialog::show(owner, &settings, Category::General, on_open, |_| false)?;
        assert!(
            result.is_none(),
            "the test must cancel Settings without saving"
        );
        assert!(
            !host.state.borrow().is_visible(),
            "Escape must dismiss the switcher inside the Settings loop"
        );
        drop(overlay);
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
