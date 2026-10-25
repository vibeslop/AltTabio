use super::App;
use crate::app_messages::FILTER_FEEDBACK_TIMER_ID;
use crate::filter_popup::FilterPopup;
use alttabio::filter_feedback::HIDE_DELAY;
use std::time::Instant;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::{Error, Result};

impl App {
    pub(super) fn filter_edited(&mut self) {
        let query = self.session.switcher().filter();
        self.filter_feedback.edited(query, Instant::now());
        if query.is_empty() {
            self.hide_filter_feedback();
            return;
        }
        let timer = unsafe {
            // SAFETY: the live overlay owns this timer. Reusing the ID resets its idle delay;
            // no callback pointer or borrowed data is retained by Windows.
            SetTimer(
                Some(self.hwnd),
                FILTER_FEEDBACK_TIMER_ID,
                u32::try_from(HIDE_DELAY.as_millis()).unwrap_or(u32::MAX),
                None,
            )
        };
        if timer == 0 {
            eprintln!(
                "Could not schedule filter feedback hiding: {}",
                Error::from_thread()
            );
            self.hide_filter_feedback();
        } else {
            self.filter_timer_armed = true;
        }
    }

    pub(super) fn handle_filter_feedback_timer(&mut self) {
        if let Some(popup) = &self.filter_popup
            && let Err(error) = popup.check_events()
        {
            eprintln!("Could not post a filter popup action: {error}");
        }
        // A timer tick queued before another edit must not hide that edit's fresh feedback.
        if !self.filter_feedback.is_visible(Instant::now()) {
            self.hide_filter_feedback();
        }
    }

    pub(super) fn hide_filter_feedback(&mut self) {
        self.filter_feedback.hide();
        if let Some(popup) = &self.filter_popup {
            popup.hide();
        }
        if self.filter_timer_armed {
            self.filter_timer_armed = false;
            let result = unsafe {
                // SAFETY: the timer belongs to this live overlay and is stopped once per arm.
                KillTimer(Some(self.hwnd), FILTER_FEEDBACK_TIMER_ID)
            };
            if let Err(error) = result {
                eprintln!("Could not stop filter feedback hiding: {error}");
            }
        }
    }

    pub(super) fn clear_filter(&mut self) {
        if self.session.search_active() && !self.session.context_menu_open() {
            self.session.clear_filter();
            self.hide_filter_feedback();
            self.request_redraw();
        }
    }

    pub(super) fn sync_filter_popup(&mut self) {
        if !self.session.search_active()
            || self.session.context_menu_open()
            || !self.filter_feedback.is_visible(Instant::now())
        {
            if let Some(popup) = &self.filter_popup {
                popup.hide();
            }
            return;
        }
        match self.update_filter_popup() {
            Ok(()) => {
                self.filter_popup_failures.succeed();
            }
            Err(error) => {
                if self.filter_popup_failures.fail() {
                    eprintln!("Could not show the typed filter: {error}");
                }
                if let Some(popup) = &self.filter_popup {
                    popup.hide();
                }
            }
        }
    }

    fn update_filter_popup(&mut self) -> Result<()> {
        if self.filter_popup.is_none() {
            self.filter_popup = Some(FilterPopup::new(self.hwnd)?);
        }
        if let Some(popup) = &mut self.filter_popup {
            popup.update(
                self.hwnd,
                self.session.switcher().filter(),
                self.resolved_theme,
                self.settings.appearance.compact_list,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_io::SettingsStore;
    use crate::win32::{ComApartment, module_instance};
    use alttabio::input::InputAction;
    use alttabio::settings::Settings;
    use alttabio::switcher::SwitchTask;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::w;

    struct Owner(HWND);
    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: this test uniquely owns its hidden window on the creating thread.
            assert!(unsafe { DestroyWindow(self.0) }.is_ok());
        }
    }

    #[test]
    fn typing_idle_and_clear_use_the_app_path_without_moving_the_panel() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let owner = Owner(unsafe {
            // SAFETY: STATIC is a system class, and the test owns this hidden, bounded popup.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Query feedback app test"),
                WS_POPUP,
                300,
                250,
                900,
                600,
                None,
                None,
                Some(module_instance()?),
                None,
            )
        }?);
        let (store, settings) = SettingsStore::load_from(
            std::env::temp_dir().join(format!("alttabio-filter-test-{}.ini", std::process::id())),
            &Settings::default(),
        )
        .map_err(|error| {
            Error::new(windows::core::HRESULT(0x8000_4005_u32.cast_signed()), error)
        })?;
        let mut app = App::new(false, false, settings, store)?;
        app.hwnd = owner.0;
        app.session.open(
            [
                SwitchTask::new(1, 10, "Alpha", "editor"),
                SwitchTask::new(2, 20, "Beta", "browser"),
            ],
            None,
        );
        let mut before = RECT::default();
        // SAFETY: the test window is live and before is writable for this query.
        unsafe {
            GetWindowRect(owner.0, &raw mut before)?;
        }
        app.apply_input_action(InputAction::AppendSearchCharacter('a'));
        assert!(app.filter_timer_armed);
        assert!(app.filter_feedback.is_visible(Instant::now()));
        assert!(app.filter_popup.is_some());
        let expired = Instant::now()
            .checked_sub(HIDE_DELAY)
            .ok_or_else(Error::from_thread)?;
        app.filter_feedback.edited("a", expired);
        app.handle_filter_feedback_timer();
        assert!(!app.filter_timer_armed);
        assert_eq!(
            app.session.switcher().filter(),
            "a",
            "idle hiding keeps the filter"
        );
        app.apply_input_action(InputAction::AppendSearchCharacter('b'));
        assert!(app.filter_feedback.is_visible(Instant::now()));
        app.clear_filter();
        assert!(!app.filter_timer_armed);
        assert!(!app.filter_feedback.is_visible(Instant::now()));
        assert_eq!(app.session.switcher().filter(), "");
        assert_eq!(app.session.switcher().visible_task_count(), 2);
        let mut after = RECT::default();
        // SAFETY: the test window is still live and after is writable for the comparison.
        unsafe {
            GetWindowRect(owner.0, &raw mut after)?;
        }
        assert_eq!(before, after);
        app.shutdown();
        Ok(())
    }
}
