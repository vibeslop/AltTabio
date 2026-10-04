use super::mouse::WM_MOUSE_LEAVE;
use super::{
    App, CLOSE_REFRESH_TIMER_ID, SHELL_DISMISS_TIMER_ID, WM_SHOW_ABOUT, WM_SHOW_SETTINGS,
    high_word_isize, high_word_usize, low_word_isize,
};
use crate::hook::{HookThread, WM_HOOK_ACTION, decode_action, decode_virtual_key};
use crate::shell_menu;
use crate::tray::{TrayAction, TrayIcon, WM_TRAY_CALLBACK};
use crate::win_events::{
    LISTED_REFRESH_RETRY_TIMER_ID, WM_FOREGROUND_CHECK, WM_LISTED_WINDOW_REFRESH,
};
use alttabio::input::{InputAction, OverlayKeyEvent, overlay_key_action};
use alttabio::switcher::SwitcherEffect;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_CAPTURECHANGED, WM_CHAR, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN, WM_THEMECHANGED, WM_TIMER,
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
                let delta = high_word_usize(wparam.0).cast_signed();
                self.handle_input_action(InputAction::MouseWheel(i32::from(delta.signum())));
                Some(LRESULT(0))
            }
            WM_SIZE => {
                let width = u32::from(low_word_isize(lparam.0));
                let height = u32::from(high_word_isize(lparam.0));
                self.resize_content(width, height);
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
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
            WM_HOOK_ACTION | crate::hook::WM_HOOK_HOTKEY_ACTION => {
                if !self.modal_state().any_open()
                    && self
                        .hooks
                        .as_ref()
                        .is_some_and(|hooks| hooks.action_is_current(wparam))
                    && let Some(action) = decode_action(wparam, lparam)
                {
                    if message == crate::hook::WM_HOOK_HOTKEY_ACTION
                        && matches!(action, InputAction::Switch(_))
                    {
                        self.resume_shell_switch_with_hotkey(action);
                    } else {
                        self.handle_hook_input(action, wparam);
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
            _ => None,
        }
    }

    fn handle_tray_message(&mut self, lparam: LPARAM) {
        let message = u32::try_from(lparam.0).unwrap_or_default();
        let action = match message {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::Show,
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
            TrayAction::Show => {
                self.stop_shell_dismissal();
                self.show_overlay(None);
            }
            TrayAction::Settings => self.request_modal_dialog(WM_SHOW_SETTINGS, "Settings"),
            TrayAction::About => self.request_modal_dialog(WM_SHOW_ABOUT, "About"),
            TrayAction::Exit => self.request_close("the tray"),
            TrayAction::None => {}
        }
    }

    pub(super) fn handle_input_action(&mut self, action: InputAction) {
        if !self.defer_to_shell_dismissal(action) {
            self.apply_input_action(action);
        }
    }

    fn handle_hook_input(&mut self, action: InputAction, origin: WPARAM) {
        if self.pending_shell.is_none()
            && !self.is_visible()
            && matches!(action, InputAction::Switch(_))
            && let Some(window) = shell_menu::foreground_menu()
        {
            self.begin_shell_dismissal(window, action, origin);
            return;
        }
        self.handle_input_action(action);
    }

    pub(super) fn apply_input_action(&mut self, action: InputAction) {
        self.apply_input_action_with_reset(action, true);
    }

    pub(super) fn apply_input_action_with_reset(&mut self, action: InputAction, reset_hook: bool) {
        match self.session.handle_input(action) {
            SwitcherEffect::None => {}
            SwitcherEffect::Open { selection_delta } => self.show_overlay(selection_delta),
            SwitcherEffect::Hide => self.hide_overlay_with_reset(reset_hook),
            SwitcherEffect::Redraw => self.request_redraw(),
            SwitcherEffect::Activate(target) => self.activate_target(target, reset_hook),
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
        if let Some(action) = overlay_key_action(event) {
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
