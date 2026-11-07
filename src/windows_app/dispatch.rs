use super::App;
use super::mouse::WM_MOUSE_LEAVE;
use crate::app_messages::{
    CLOSE_REFRESH_TIMER_ID, FilterPointerAction, LISTED_REFRESH_RETRY_TIMER_ID,
    SHELL_DISMISS_TIMER_ID, WM_CLEAR_FILTER, WM_FILTER_POINTER, WM_FOREGROUND_CHECK,
    WM_HOOK_ACTION, WM_LISTED_WINDOW_REFRESH, WM_OUTSIDE_POINTER, WM_SHOW_ABOUT, WM_SHOW_SETTINGS,
    WM_TRAY_CALLBACK,
};
use crate::hook::{HookThread, decode_action};
use crate::shell_menu;
use crate::tray::{TrayAction, TrayIcon};
use crate::win32::{high_word, low_word};
use alttabio::input::{InputAction, OverlayKeyEvent, decode_virtual_key, overlay_key_action};
use alttabio::switcher::SwitcherEffect;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_CONTROL, VK_RMENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_MOVE, WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN,
    WM_THEMECHANGED, WM_TIMER,
};

impl App {
    pub(super) fn handle_message(
        &mut self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if let Some(result) = self.handle_posted_message(message, wparam, lparam) {
            return Some(result);
        }
        match message {
            WM_ACTIVATE if u32::from(low_word(wparam.0)) == WA_INACTIVE => {
                if self.is_visible()
                    && self.pending_shell.is_none()
                    && !self.dialog_state().blocks_switcher()
                    && self.overlay_focus.observe(false)
                {
                    self.hide_overlay();
                }
                Some(LRESULT(0))
            }
            WM_MOVE => {
                self.sync_filter_geometry();
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.handle_dpi_changed(lparam);
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE => {
                self.handle_display_changed();
                Some(LRESULT(0))
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                self.handle_focused_key(wparam.0, lparam);
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.handle_character(wparam.0);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                self.handle_mouse_move(lparam);
                Some(LRESULT(0))
            }
            WM_MOUSE_LEAVE => {
                self.handle_mouse_leave();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                self.handle_button_down(lparam);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.handle_button_up(lparam);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                if self.close_button.cancel_press() {
                    self.request_redraw();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = high_word(wparam.0).cast_signed();
                self.handle_mouse_wheel(i32::from(delta), lparam);
                Some(LRESULT(0))
            }
            WM_SIZE => {
                let width = u32::from(low_word(lparam.0.cast_unsigned()));
                let height = u32::from(high_word(lparam.0.cast_unsigned()));
                self.resize_content(width, height);
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                self.sync_filter_geometry();
                match self.refresh_theme() {
                    Ok(true) => self.request_redraw(),
                    Ok(false) => {}
                    Err(error) => eprintln!("Could not refresh the overlay theme: {error}"),
                }
                Some(LRESULT(0))
            }
            WM_PAINT => {
                self.paint();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            _ => None,
        }
    }

    fn handle_posted_message(
        &mut self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if let Some(result) = self
            .tray
            .as_ref()
            .and_then(|tray| tray.restore_for_message(message))
        {
            if let Err(error) = result {
                eprintln!(
                    "Could not restore the AltTabio tray icon after Explorer restarted: {error}"
                );
            }
            return Some(LRESULT(0));
        }
        match message {
            WM_HOOK_ACTION => {
                let current = !self.dialog_state().blocks_switcher()
                    && self
                        .hooks
                        .as_ref()
                        .is_some_and(|hooks| hooks.action_is_current(wparam));
                if let Some(action) = decode_action(wparam, lparam) {
                    if current {
                        self.handle_hook_input(action, wparam);
                    } else {
                        self.retire_hook_opening(action, wparam);
                    }
                }
                Some(LRESULT(0))
            }
            WM_TRAY_CALLBACK => {
                self.handle_tray_message(lparam);
                Some(LRESULT(0))
            }
            WM_FOREGROUND_CHECK => {
                self.handle_foreground_check();
                Some(LRESULT(0))
            }
            WM_LISTED_WINDOW_REFRESH => {
                self.handle_listed_window_refresh();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == CLOSE_REFRESH_TIMER_ID => {
                self.handle_close_refresh_timer();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == LISTED_REFRESH_RETRY_TIMER_ID => {
                self.handle_listed_refresh_retry_timer();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == SHELL_DISMISS_TIMER_ID => {
                self.handle_shell_dismissal();
                Some(LRESULT(0))
            }
            WM_CLEAR_FILTER => {
                self.clear_filter();
                Some(LRESULT(0))
            }
            WM_FILTER_POINTER => {
                if let Some(action) = FilterPointerAction::from_code(lparam.0) {
                    self.handle_filter_pointer(wparam.0, action);
                }
                Some(LRESULT(0))
            }
            WM_OUTSIDE_POINTER => {
                self.handle_outside_pointer(
                    u32::try_from(wparam.0).unwrap_or_default().cast_signed(),
                    i32::try_from(lparam.0).unwrap_or_default(),
                );
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn handle_tray_message(&mut self, lparam: LPARAM) {
        let message = u32::try_from(lparam.0).unwrap_or_default();
        let action = match message {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::OpenSwitcher,
            WM_RBUTTONUP => {
                let _suspension = self.hooks.as_ref().map(HookThread::suspend_interception);
                self.tray
                    .as_ref()
                    .map(TrayIcon::show_menu)
                    .unwrap_or_default()
            }
            _ => TrayAction::None,
        };
        match action {
            TrayAction::OpenSwitcher => {
                self.stop_shell_dismissal();
                self.show_overlay(None);
            }
            TrayAction::Settings => self.request_modal_dialog(WM_SHOW_SETTINGS, "Settings"),
            TrayAction::About => self.request_modal_dialog(WM_SHOW_ABOUT, "About"),
            TrayAction::Update => {
                self.request_modal_dialog(crate::app_messages::WM_SHOW_UPDATES, "Updates");
            }
            TrayAction::Exit => self.request_close("the tray"),
            TrayAction::None => {}
        }
    }

    pub(super) fn handle_input_action(&mut self, action: InputAction) {
        if !self.defer_to_shell_dismissal(action) {
            self.apply_input_action(action);
        }
    }

    pub(super) fn handle_hook_input(&mut self, action: InputAction, origin: WPARAM) {
        if self.pending_shell.is_none()
            && !self.is_visible()
            && matches!(
                action,
                InputAction::Switch(_) | InputAction::MouseWheel(_, _)
            )
            && let Some(window) = shell_menu::foreground_menu()
        {
            self.begin_shell_dismissal(window, action, origin);
            return;
        }
        if !self.defer_to_shell_dismissal(action) {
            self.apply_hook_action(action, origin);
        }
    }

    pub(super) fn retire_hook_opening(&self, action: InputAction, origin: WPARAM) {
        if matches!(
            action,
            InputAction::Switch(_) | InputAction::MouseWheel(_, _)
        ) && let Some(hooks) = &self.hooks
        {
            hooks.retire_openings(origin, 1);
        }
    }

    pub(super) fn apply_hook_action(&mut self, action: InputAction, origin: WPARAM) {
        let Some(_action) = self
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.applying_action(origin, action))
        else {
            return;
        };
        if let InputAction::OverlayWheel(delta, position) = action {
            self.handle_overlay_wheel(i32::from(delta), position);
            return;
        }
        if let InputAction::MouseWheel(steps, position) = action
            && self.handle_gesture_wheel(i32::from(steps), position)
        {
            return;
        }
        self.apply_input_action(action);
    }

    pub(super) fn apply_input_action(&mut self, action: InputAction) {
        if let InputAction::EditSearch(
            command @ (alttabio::text_edit::TextCommand::Copy
            | alttabio::text_edit::TextCommand::Cut
            | alttabio::text_edit::TextCommand::Paste),
        ) = action
        {
            self.handle_clipboard_command(command);
            return;
        }
        let effect = self.session.handle_input(action);
        match effect {
            SwitcherEffect::None => {}
            SwitcherEffect::Open { selection_delta } => self.show_overlay(selection_delta),
            SwitcherEffect::Hide => self.hide_overlay(),
            SwitcherEffect::Redraw => self.request_redraw(),
            SwitcherEffect::Activate(target) => self.activate_target(target, action),
            SwitcherEffect::Execute(request) => {
                let outcome = self.execute_window_command(request);
                self.task_refresh.apply_command_outcome(outcome);
                self.run_pending_task_refresh();
            }
        }
    }

    fn handle_focused_key(&mut self, virtual_key: usize, lparam: LPARAM) {
        let Ok(virtual_key) = u32::try_from(virtual_key) else {
            return;
        };
        let event = OverlayKeyEvent {
            key: decode_virtual_key(virtual_key),
            repeated: key_was_previously_down(lparam),
            shift: key_is_down(VK_SHIFT.0),
        };
        let control = key_is_down(VK_CONTROL.0);
        if self.session.search_active()
            && (virtual_key != u32::from(VK_BACK.0) || control)
            && let Some(command) = alttabio::text_edit::TextCommand::for_key(
                virtual_key,
                control,
                event.shift,
                key_is_down(VK_RMENU.0),
            )
        {
            self.handle_input_action(InputAction::EditSearch(command));
            return;
        }
        if let Some(action) = overlay_key_action(event) {
            // Focused printable query input arrives through WM_CHAR, including its keyboard
            // layout and Shift translation. Handling digits here too would insert them twice.
            if self.session.filter_visible()
                && matches!(action, InputAction::ActivateVisiblePosition(_))
            {
                return;
            }
            self.handle_input_action(action);
        }
    }

    fn handle_character(&mut self, value: usize) {
        if !self.search_active() {
            return;
        }
        let value = u32::try_from(value).unwrap_or_default();
        let action = if value == u32::from(VK_BACK.0) {
            InputAction::BackspaceSearch
        } else if let Some(character) = char::from_u32(value)
            && !character.is_control()
        {
            InputAction::AppendSearchCharacter(character)
        } else {
            return;
        };
        self.handle_input_action(action);
    }

    fn search_active(&self) -> bool {
        self.session.search_active()
    }
}

fn key_is_down(virtual_key: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    unsafe {
        // SAFETY: GetKeyState accepts any virtual-key code and has no pointer preconditions.
        GetKeyState(i32::from(virtual_key)) < 0
    }
}

const fn key_was_previously_down(lparam: LPARAM) -> bool {
    let previous_key_state_mask = 1_isize << 30;
    lparam.0 & previous_key_state_mask != 0
}
