use super::commands::show_menu as show_window_command_menu;
use super::{App, AppHost};
use crate::win32::point_from_lparam;
use alttabio::input::InputAction;
use alttabio::overlay_pointer::{close_target_for_hit, select_hovered_position};
use alttabio::switcher::SwitcherEffect;
use alttabio::task_list_hit::{TaskListHit, hit_test_pixels};
use alttabio::task_refresh::ContextMenuCommandOutcome;
use alttabio::window_command::WindowCommand;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos};

pub(super) const WM_MOUSE_LEAVE: u32 = 0x02A3;

impl AppHost {
    pub(super) fn show_task_context_menu(&self, lparam: LPARAM) {
        let Some(owner) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_task_context_menu(lparam))
        else {
            return;
        };
        let command = show_window_command_menu(owner);
        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish the task menu because application state is busy");
            return;
        };
        app.finish_task_context_menu(command);
    }
}

impl App {
    pub(super) fn handle_mouse_move(&mut self, lparam: LPARAM) {
        if !self.mouse_selection_armed {
            let mut cursor = POINT::default();
            let current = unsafe {
                // SAFETY: `cursor` is writable for the call.
                GetCursorPos(&raw mut cursor)
            };
            if current.is_err() || self.mouse_origin == Some(cursor) {
                return;
            }
            self.mouse_selection_armed = true;
        }
        self.track_mouse_leave();

        let mut hit = self.hit_test(lparam);
        let mut needs_redraw = false;
        if self.settings.general.mouse_over_selection
            && !self.close_button.is_pressed()
            && let Some(TaskListHit::Task(position)) = hit
            && select_hovered_position(self.session.switcher_mut(), position)
        {
            needs_redraw = true;
            hit = self.hit_test(lparam);
        }
        let close_target = close_target_for_hit(self.session.switcher(), hit);
        needs_redraw |= self.close_button.update_hover(close_target);
        if needs_redraw {
            self.request_redraw();
        }
    }

    pub(super) fn handle_mouse_leave(&mut self) {
        self.mouse_leave_tracked = false;
        if self.close_button.update_hover(None) {
            self.request_redraw();
        }
    }

    pub(super) fn handle_button_down(&mut self, lparam: LPARAM) {
        let hit = self.hit_test(lparam);
        let target = close_target_for_hit(self.session.switcher(), hit);
        let Some(target) = target else {
            return;
        };
        self.close_button.press(target);
        unsafe {
            // SAFETY: the overlay HWND is live; a null previous HWND is a valid SetCapture result.
            let _previous_capture = SetCapture(self.hwnd);
        }
        self.request_redraw();
    }

    pub(super) fn handle_button_up(&mut self, lparam: LPARAM) {
        let hit = self.hit_test(lparam);
        if self.close_button.is_pressed() {
            let target = close_target_for_hit(self.session.switcher(), hit);
            let command = self.close_button.release(target);
            let release_result = unsafe {
                // SAFETY: this UI thread acquired mouse capture when the close button was pressed.
                ReleaseCapture()
            };
            if let Err(error) = release_result {
                eprintln!("Could not release close-button mouse capture: {error}");
            }
            self.request_redraw();
            if let Some(command) = command {
                self.handle_input_action(InputAction::WindowCommand(command));
            }
            return;
        }

        if let Some(TaskListHit::Task(position)) = hit {
            self.handle_input_action(InputAction::ActivateVisiblePosition(position));
        }
    }

    fn track_mouse_leave(&mut self) {
        if self.mouse_leave_tracked {
            return;
        }
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or_default(),
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        let result = unsafe {
            // SAFETY: `tracking` is writable and the overlay HWND remains live for the call.
            TrackMouseEvent(&raw mut tracking)
        };
        match result {
            Ok(()) => self.mouse_leave_tracked = true,
            Err(error) => eprintln!("Could not track close-button mouse leave: {error}"),
        }
    }

    fn hit_test(&mut self, lparam: LPARAM) -> Option<TaskListHit> {
        let window_dpi = unsafe {
            // SAFETY: the overlay HWND is live and the call returns a scalar DPI value.
            GetDpiForWindow(self.hwnd)
        };
        let mut client = RECT::default();
        let client_result = unsafe {
            // SAFETY: `client` is writable for the call and the overlay HWND is live.
            GetClientRect(self.hwnd, &raw mut client)
        };
        // Every mouse message hit tests, so a failure that lasts would log on each move. Logging
        // the first failure of a run is enough.
        match client_result {
            Ok(()) => self.hit_test_failing = false,
            Err(error) => {
                if !self.hit_test_failing {
                    eprintln!("Could not read the overlay's client area for a hit test: {error}");
                }
                self.hit_test_failing = true;
                return None;
            }
        }
        hit_test_pixels(
            self.session.switcher_mut(),
            (
                client.right.saturating_sub(client.left),
                client.bottom.saturating_sub(client.top),
            ),
            mouse_coordinates(lparam),
            window_dpi,
            self.settings.appearance.compact_list,
        )
    }

    pub(super) fn reset_mouse_selection(&mut self) {
        let mut cursor = POINT::default();
        self.mouse_origin = unsafe {
            // SAFETY: `cursor` is writable for the call.
            GetCursorPos(&raw mut cursor).ok().map(|()| cursor)
        };
        self.mouse_selection_armed = false;
        self.mouse_leave_tracked = false;
        self.close_button.reset();
    }

    fn prepare_task_context_menu(&mut self, lparam: LPARAM) -> Option<HWND> {
        if !self.is_visible() || self.modal_state().any_open() {
            return None;
        }
        let hit = self.hit_test(lparam)?;
        let position = hit.position();
        if !self
            .session
            .switcher_mut()
            .select_visible_position(position)
        {
            return None;
        }
        self.request_redraw();
        if !self.session.open_context_menu() {
            return None;
        }
        self.sync_hook_interception();
        Some(self.hwnd)
    }

    fn finish_task_context_menu(&mut self, command: Option<WindowCommand>) {
        let effect = self.session.finish_context_menu(command);
        self.sync_hook_interception();
        self.ingest_listed_refresh_signal();
        let outcome = match effect {
            SwitcherEffect::Execute(request) => self.execute_window_command(request),
            _ => ContextMenuCommandOutcome::None,
        };
        self.task_refresh.apply_command_outcome(outcome);
        self.run_pending_task_refresh();
    }
}

fn mouse_coordinates(lparam: LPARAM) -> (i32, i32) {
    let point = point_from_lparam(lparam);
    (point.x, point.y)
}
