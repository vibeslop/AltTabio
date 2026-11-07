use super::commands::show_menu as show_window_command_menu;
use super::{App, AppHost};
use crate::win32::point_from_lparam;
use alttabio::input::InputAction;
use alttabio::overlay_pointer::{close_target_for_hit, select_hovered_position};
use alttabio::switcher::SwitcherEffect;
use alttabio::task_list_hit::{
    TaskListHit, hit_test_pixels, pointer_in_list_pixels, wheel_behavior_at_pixels,
};
use alttabio::task_refresh::ContextMenuCommandOutcome;
use alttabio::window_command::WindowCommand;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
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
    pub(super) fn handle_mouse_wheel(&mut self, delta: i32, lparam: LPARAM) {
        let coordinates = point_from_lparam(lparam);
        let screen_point = POINT {
            x: coordinates.x,
            y: coordinates.y,
        };
        self.handle_wheel_at(delta, screen_point, false);
    }

    pub(super) fn handle_overlay_wheel(&mut self, delta: i32, position: (i32, i32)) {
        if self.is_visible() {
            self.handle_wheel_at(
                delta,
                POINT {
                    x: position.0,
                    y: position.1,
                },
                false,
            );
        }
    }

    /// A held-right-button gesture uses the same routing as ordinary captured wheel input.
    pub(super) fn handle_gesture_wheel(&mut self, steps: i32, position: (i32, i32)) -> bool {
        if !self.is_visible() {
            return false;
        }
        self.handle_wheel_at(
            steps.saturating_mul(-120),
            POINT {
                x: position.0,
                y: position.1,
            },
            true,
        )
    }

    fn handle_wheel_at(&mut self, delta: i32, screen_point: POINT, gesture: bool) -> bool {
        let mut point = screen_point;
        let converted = unsafe {
            // SAFETY: the overlay HWND is live and `point` is writable for this call.
            ScreenToClient(self.hwnd, &raw mut point)
        };
        if !converted.as_bool() {
            eprintln!("Could not convert wheel coordinates to the overlay client area");
            return false;
        }
        let Some(client) = self.client_size() else {
            return false;
        };
        let dpi = unsafe {
            // SAFETY: the overlay HWND is live and the call returns a scalar DPI value.
            GetDpiForWindow(self.hwnd)
        };
        let behavior = wheel_behavior_at_pixels(
            client,
            (point.x, point.y),
            dpi,
            self.settings.appearance.compact_list,
        );
        self.hover_selection
            .after_wheel((screen_point.x, screen_point.y));
        let visible_rows = self.renderer.visible_row_count(
            self.settings.appearance.compact_list,
            self.session.filter_visible(),
        );
        let effect = if gesture {
            self.session
                .handle_gesture_wheel(behavior, delta, visible_rows)
        } else {
            self.session
                .handle_mouse_wheel(behavior, delta, visible_rows)
        };
        if effect == SwitcherEffect::Redraw {
            self.close_button.update_hover(None);
            self.request_redraw();
        }
        true
    }

    pub(super) fn handle_mouse_move(&mut self, lparam: LPARAM) {
        let Some(cursor) = self.read_mouse_position() else {
            return;
        };
        let dpi = unsafe {
            // SAFETY: the overlay HWND is live and the call returns a scalar DPI value.
            GetDpiForWindow(self.hwnd)
        };
        let pointer_moved = self
            .hover_selection
            .pointer_moved((cursor.x, cursor.y), dpi);
        self.track_mouse_leave();

        let mut hit = self.hit_test(lparam);
        let mut needs_redraw = self.refresh_close_button_visibility();
        if pointer_moved
            && self.settings.general.mouse_over_selection
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
        if self.close_button.update_pointer_in_list(false) {
            self.request_redraw();
        }
    }

    /// Refresh on paint too, so opening, resizing, or changing selection under a stationary
    /// pointer uses its current pane without arming hover selection.
    pub(super) fn refresh_close_button_visibility(&mut self) -> bool {
        let in_list = self.read_mouse_position().is_some_and(|mut cursor| {
            let converted = unsafe {
                // SAFETY: the overlay HWND is live and `cursor` is writable for this call.
                ScreenToClient(self.hwnd, &raw mut cursor)
            };
            if !converted.as_bool() {
                eprintln!("Could not convert the cursor position for close-button visibility");
                return false;
            }
            let Some(client) = self.client_size() else {
                return false;
            };
            let dpi = unsafe {
                // SAFETY: the overlay HWND is live and the call returns a scalar DPI value.
                GetDpiForWindow(self.hwnd)
            };
            pointer_in_list_pixels(
                client,
                (cursor.x, cursor.y),
                dpi,
                self.settings.appearance.compact_list,
            )
        });
        if in_list {
            self.track_mouse_leave();
        }
        self.close_button.update_pointer_in_list(in_list)
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
        let client = self.client_size()?;
        hit_test_pixels(
            self.session.switcher_mut(),
            client,
            mouse_coordinates(lparam),
            window_dpi,
            self.settings.appearance.compact_list,
        )
    }

    fn client_size(&mut self) -> Option<(i32, i32)> {
        let mut client = RECT::default();
        let client_result = unsafe {
            // SAFETY: `client` is writable for the call and the overlay HWND is live.
            GetClientRect(self.hwnd, &raw mut client)
        };
        // Every mouse message hit tests, so a failure that lasts would log on each move. Logging
        // the first failure of a run is enough.
        match client_result {
            Ok(()) => self.hit_test_failures.succeed(),
            Err(error) => {
                if self.hit_test_failures.fail() {
                    eprintln!("Could not read the overlay's client area for a hit test: {error}");
                }
                return None;
            }
        }
        Some((
            client.right.saturating_sub(client.left),
            client.bottom.saturating_sub(client.top),
        ))
    }

    fn read_mouse_position(&mut self) -> Option<POINT> {
        let mut cursor = POINT::default();
        let read = unsafe {
            // SAFETY: `cursor` is writable for the call.
            GetCursorPos(&raw mut cursor)
        };
        match read {
            Ok(()) => {
                self.cursor_read_failures.succeed();
                Some(cursor)
            }
            Err(error) => {
                if self.cursor_read_failures.fail() {
                    eprintln!("Could not read the cursor position for hover selection: {error}");
                }
                None
            }
        }
    }

    pub(super) fn reset_mouse_selection(&mut self) {
        let position = self.read_mouse_position().map(|point| (point.x, point.y));
        self.hover_selection.reset(position);
        self.mouse_leave_tracked = false;
        self.close_button.reset();
    }

    fn prepare_task_context_menu(&mut self, lparam: LPARAM) -> Option<HWND> {
        if !self.is_visible()
            || self.session.mouse_switching()
            || self.dialog_state().blocks_switcher()
        {
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
