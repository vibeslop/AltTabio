use super::App;
use crate::app_messages::FilterPointerAction;
use crate::filter_popup::FilterPopup;
use alttabio::switcher::SwitcherSession;
use alttabio::text_edit::TextCommand;
use windows::core::Result;

impl App {
    pub(super) fn handle_outside_pointer(&mut self, x: i32, y: i32) {
        if !self.is_visible()
            || self.pending_shell.is_some()
            || self.dialog_state().blocks_switcher()
        {
            return;
        }
        let mut panel = windows::Win32::Foundation::RECT::default();
        if let Err(error) = unsafe {
            // SAFETY: the live overlay owns this initialized output for a synchronous read.
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.hwnd, &raw mut panel)
        } {
            eprintln!("Could not check an outside click: {error}");
            return;
        }
        let point = alttabio::dialog_layout::Point::new(x, y);
        let in_panel = crate::win32::rect_from_native(panel).contains_point(point);
        let in_query = self
            .filter_popup
            .as_ref()
            .and_then(FilterPopup::visible_bounds)
            .is_some_and(|bounds| bounds.contains_point(point));
        if !in_panel && !in_query {
            self.hide_overlay();
        }
    }
    pub(super) fn hide_filter_feedback(&self) {
        if let Some(popup) = &self.filter_popup {
            popup.hide();
        }
    }

    pub(super) fn clear_filter(&mut self) {
        if self.session.search_active() && !self.session.context_menu_open() {
            self.session.clear_filter();
            self.request_redraw();
        }
    }

    pub(super) fn handle_filter_pointer(&mut self, x: usize, action: FilterPointerAction) {
        if !self.session.filter_visible() || self.dialog_state().blocks_switcher() {
            return;
        }
        if action == FilterPointerAction::SelectAll {
            let _effect = self
                .session
                .handle_input(alttabio::input::InputAction::EditSearch(
                    TextCommand::SelectAll,
                ));
        } else if let Some(popup) = &self.filter_popup {
            match popup.hit_test(i32::try_from(x).unwrap_or(i32::MAX)) {
                Ok(position) => self.session.select_filter_position(
                    position,
                    action == FilterPointerAction::ExtendSelection,
                ),
                Err(error) => eprintln!("Could not position the filter caret: {error}"),
            }
        }
        self.request_redraw();
    }

    pub(super) fn handle_clipboard_command(&mut self, command: TextCommand) {
        let owner = self.hwnd;
        if let Err(error) = clipboard_command(
            &mut self.session,
            command,
            || crate::clipboard::read(owner),
            |text| crate::clipboard::write(owner, text),
        ) {
            eprintln!("Could not edit the filter through the clipboard: {error}");
        }
        self.request_redraw();
    }

    pub(super) fn sync_filter_geometry(&mut self) {
        if let Some(popup) = &mut self.filter_popup {
            popup.invalidate_geometry();
        }
        self.sync_filter_popup();
    }

    pub(super) fn sync_filter_popup(&mut self) {
        if !self.session.filter_visible() || self.session.context_menu_open() {
            self.hide_filter_feedback();
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
                self.hide_filter_feedback();
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
                self.session.switcher().filter_editor(),
                self.resolved_theme,
                self.settings.appearance.compact_list,
            )?;
        }
        Ok(())
    }
}

fn clipboard_command(
    session: &mut SwitcherSession,
    command: TextCommand,
    read: impl FnOnce() -> Result<String>,
    write: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    if !session.search_active() || session.context_menu_open() {
        return Ok(());
    }
    match command {
        TextCommand::Copy | TextCommand::Cut => {
            let selected = session
                .switcher()
                .filter_editor()
                .selected_text()
                .to_owned();
            if !selected.is_empty() {
                write(&selected)?;
                if command == TextCommand::Cut {
                    session.replace_filter_selection("");
                }
            }
        }
        TextCommand::Paste => {
            let text = read()?;
            if !text.is_empty() {
                session.replace_filter_selection(&text);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_io::SettingsStore;
    use crate::win32::{ComApartment, module_instance};
    use alttabio::input::InputAction;
    use alttabio::settings::Settings;
    use alttabio::switcher::SwitchTask;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardState, SetKeyboardState, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RMENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WA_INACTIVE, WM_ACTIVATE, WM_CHAR,
        WM_KEYDOWN, WM_SYSKEYDOWN, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::{Error, w};

    struct Owner(HWND);
    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: this test uniquely owns its hidden window on the creating thread.
            assert!(unsafe { DestroyWindow(self.0) }.is_ok());
        }
    }

    struct KeyboardState([u8; 256]);

    impl Drop for KeyboardState {
        fn drop(&mut self) {
            // SAFETY: this restores only this test thread's original input-queue key state.
            assert!(unsafe { SetKeyboardState(&self.0) }.is_ok());
        }
    }

    fn assert_altgr_preserves_the_query(app: &mut App) -> Result<()> {
        let mut original = KeyboardState([0; 256]);
        // SAFETY: the complete writable buffer belongs to this test thread.
        unsafe { GetKeyboardState(&mut original.0) }?;
        for (left_alt, right_alt) in [(false, false), (true, false), (false, true)] {
            let mut keys = [0; 256];
            for key in [VK_CONTROL, VK_LCONTROL] {
                keys[usize::from(key.0)] = 0x80;
            }
            if right_alt {
                for key in [VK_MENU, VK_RMENU] {
                    keys[usize::from(key.0)] = 0x80;
                }
            }
            if left_alt {
                for key in [VK_MENU, VK_LMENU] {
                    keys[usize::from(key.0)] = 0x80;
                }
            }
            app.session.clear_filter();
            app.apply_input_action(InputAction::AppendSearchCharacter('a'));
            let message = if right_alt { WM_SYSKEYDOWN } else { WM_KEYDOWN };
            // SAFETY: install this thread's synthetic modifiers after redraw work, immediately
            // before the key event whose GetKeyState snapshot the fixture controls.
            unsafe { SetKeyboardState(&keys) }?;
            let _handled = app.handle_message(message, WPARAM(0x41), LPARAM(0));
            if right_alt {
                let _handled = app.handle_message(WM_CHAR, WPARAM('ą' as usize), LPARAM(0));
                assert_eq!(app.session.switcher().filter(), "aą");
            } else {
                assert_eq!(app.session.switcher().filter_editor().selected_text(), "a");
            }
        }
        Ok(())
    }

    #[test]
    fn query_stays_visible_through_selection_and_empty_results_then_click_away_dismisses()
    -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let owner = Owner(unsafe {
            // SAFETY: STATIC is a system class, and the test owns this hidden popup.
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
        app.apply_input_action(InputAction::AppendSearchCharacter('z'));
        assert!(app.session.filter_visible());
        assert_eq!(app.session.switcher().visible_task_count(), 0);
        assert!(app.filter_popup.is_some());
        app.apply_input_action(InputAction::EditSearch(TextCommand::SelectAll));
        assert_eq!(app.session.switcher().filter_editor().selected_text(), "z");
        app.request_redraw();
        assert!(app.session.filter_visible());
        app.apply_input_action(InputAction::AppendSearchCharacter('a'));
        assert_eq!(app.session.switcher().filter(), "a");
        // Preview mode has no hooks: each printable press reaches keydown and then WM_CHAR.
        for (key, character, expected) in
            [(0x31, '1', "a1"), (0x62, '2', "a12"), (0x33, '#', "a12#")]
        {
            let _handled = app.handle_message(WM_KEYDOWN, WPARAM(key), LPARAM(0));
            let _handled = app.handle_message(WM_CHAR, WPARAM(character as usize), LPARAM(0));
            assert_eq!(app.session.switcher().filter(), expected);
        }
        assert_altgr_preserves_the_query(&mut app)?;
        app.clear_filter();
        assert!(!app.session.filter_visible());
        assert_eq!(app.session.switcher().visible_task_count(), 2);
        let mut after = RECT::default();
        // SAFETY: the test window remains live and after is writable for the comparison.
        unsafe {
            GetWindowRect(owner.0, &raw mut after)?;
        }
        assert_eq!(before, after);
        app.handle_outside_pointer(before.left + 40, before.top + 40);
        assert!(app.session.is_visible());
        assert!(app.session.open_context_menu());
        app.handle_outside_pointer(before.left - 40, before.top);
        assert!(
            app.session.is_visible(),
            "an open context menu must retain the switcher"
        );
        let _effect = app.session.finish_context_menu(None);
        app.handle_outside_pointer(before.left - 40, before.top);
        assert!(!app.session.is_visible());
        app.session
            .open([SwitchTask::new(1, 10, "Alpha", "editor")], None);
        let _handled = app.handle_message(
            WM_ACTIVATE,
            WPARAM(usize::try_from(WA_INACTIVE).unwrap_or_default()),
            LPARAM(0),
        );
        assert!(
            app.session.is_visible(),
            "an opening that never acquired focus must remain usable"
        );
        let _lost = app.overlay_focus.observe(true);
        let _handled = app.handle_message(
            WM_ACTIVATE,
            WPARAM(usize::try_from(WA_INACTIVE).unwrap_or_default()),
            LPARAM(0),
        );
        assert!(!app.session.is_visible());
        app.shutdown();
        Ok(())
    }

    #[test]
    fn clipboard_copy_cut_paste_and_failed_cut_preserve_the_expected_text() -> Result<()> {
        let settings = alttabio::switcher::SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        };
        let mut session = SwitcherSession::new(settings);
        session.open([SwitchTask::new(1, 10, "Alpha", "editor")], None);
        session.replace_filter_selection("Документы 🦀");
        let _effect = session.handle_input(InputAction::EditSearch(TextCommand::SelectAll));
        clipboard_command(
            &mut session,
            TextCommand::Copy,
            || Ok(String::new()),
            |text| {
                assert_eq!(text, "Документы 🦀");
                Ok(())
            },
        )?;
        assert_eq!(session.switcher().filter(), "Документы 🦀");
        let failed = clipboard_command(
            &mut session,
            TextCommand::Cut,
            || Ok(String::new()),
            |_| {
                Err(Error::from_hresult(windows::core::HRESULT(
                    0x8000_4005_u32.cast_signed(),
                )))
            },
        );
        assert!(failed.is_err());
        assert_eq!(session.switcher().filter(), "Документы 🦀");
        clipboard_command(
            &mut session,
            TextCommand::Cut,
            || Ok(String::new()),
            |_| Ok(()),
        )?;
        assert_eq!(session.switcher().filter(), "");
        clipboard_command(
            &mut session,
            TextCommand::Paste,
            || Ok("browser".to_owned()),
            |_| Ok(()),
        )?;
        assert_eq!(session.switcher().filter(), "browser");
        let _effect = session.handle_input(InputAction::EditSearch(TextCommand::Undo));
        assert_eq!(session.switcher().filter(), "");
        Ok(())
    }
}
