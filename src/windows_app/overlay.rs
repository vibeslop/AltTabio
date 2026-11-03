use super::App;
use super::activation::request_foreground;
use super::display::position_on_cursor_monitor;
use crate::preview::DwmPreview;
use crate::renderer::RenderOptions;
use crate::task_query::{EnumeratedTasks, enumerate_switchable_windows};
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SW_HIDE, SW_SHOW, SW_SHOWNA, ShowWindow,
};
use windows::core::Error;

impl App {
    pub(super) fn show_overlay(&mut self, selection_delta: Option<i32>) {
        if self.dialog_state().blocks_switcher() {
            return;
        }
        self.overlay_focus.reset();
        self.hide_filter_feedback();
        match enumerate_switchable_windows(&self.settings) {
            Ok(EnumeratedTasks { tasks, icons }) => {
                self.session.open(tasks, selection_delta);
                self.task_icons = icons;
            }
            Err(error) => {
                eprintln!("Could not enumerate windows: {error}");
                self.hide_overlay();
                return;
            }
        }
        if !self.session.is_visible() {
            self.hide_overlay();
            return;
        }
        self.reset_mouse_selection();
        if let Err(error) = position_on_cursor_monitor(self.hwnd) {
            eprintln!("Could not position the overlay: {error}");
        }
        unsafe {
            // SAFETY: the HWND is live and owned by this UI thread.
            let _was_visible = ShowWindow(
                self.hwnd,
                if self.pending_shell.is_some() {
                    SW_SHOWNA
                } else {
                    SW_SHOW
                },
            );
        }
        if self.pending_shell.is_none() {
            self.focus_overlay();
        }
        self.sync_content_size();
        self.set_hook_search_active(true);
        self.set_hook_overlay_active(true);
        self.request_redraw();
    }

    pub(super) fn focus_overlay(&mut self) {
        // SAFETY: the HWND is live and owned by this UI thread.
        unsafe {
            if !request_foreground(self.hwnd) {
                match crate::hook::send_activation_alt() {
                    Ok(()) if request_foreground(self.hwnd) => {}
                    Ok(()) => eprintln!("Could not bring the overlay to the foreground"),
                    Err(error) => eprintln!("Could not retry overlay activation: {error}"),
                }
            }
            if let Err(error) = SetFocus(Some(self.hwnd)) {
                eprintln!("Could not focus the overlay: {error}");
            }
            let _lost = self
                .overlay_focus
                .observe(GetForegroundWindow() == self.hwnd);
        }
    }

    pub(super) fn hide_overlay(&mut self) {
        self.overlay_focus.reset();
        self.hide_filter_feedback();
        self.stop_shell_dismissal();
        self.task_refresh.clear_notices();
        self.session.hide();
        self.set_hook_search_active(false);
        self.reset_hook_gestures();
        if let Some(preview) = &mut self.preview {
            preview.clear();
        }
        if self.close_button.is_pressed() {
            self.close_button.reset();
            let result = unsafe {
                // SAFETY: this UI thread owns capture only while its close button is pressed.
                ReleaseCapture()
            };
            if let Err(error) = result {
                eprintln!("Could not release close-button mouse capture while hiding: {error}");
            }
        } else {
            self.close_button.reset();
        }
        self.mouse_leave_tracked = false;
        unsafe {
            // SAFETY: the HWND is live and owned by this UI thread.
            let _was_visible = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.set_hook_overlay_active(false);
        if self.exit_when_hidden {
            self.request_close("the preview window");
        }
    }

    pub(super) fn paint(&mut self) {
        self.refresh_close_button_visibility();
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe {
            // SAFETY: the HWND is live during WM_PAINT and `paint` is a writable PAINTSTRUCT.
            BeginPaint(self.hwnd, &raw mut paint)
        };
        // Only a successful BeginPaint validates the update region, so after a failure WM_PAINT
        // keeps arriving until a call succeeds. Logging the first failure of a run is enough.
        let begin_failed = dc.is_invalid();
        if begin_failed {
            if self.begin_paint_failures.fail() {
                eprintln!("Could not begin painting the overlay");
            }
        } else {
            self.begin_paint_failures.succeed();
        }
        let render_options = RenderOptions::from(&self.settings.appearance);
        let switcher = self.session.switcher();
        let selected_target = switcher.selected_task().map(|task| task.window_handle);
        if let Err(error) = self.renderer.draw(
            self.hwnd,
            switcher,
            &self.task_icons,
            self.preview.as_ref().and_then(DwmPreview::frame),
            render_options,
            self.close_button.visual_state(selected_target),
        ) {
            eprintln!("Could not render the overlay: {error}");
        }
        // The renderer presents the complete frame, including icons, through its own target.
        // Only a successful BeginPaint needs a matching EndPaint.
        if begin_failed {
            return;
        }
        unsafe {
            // SAFETY: paint was filled by the successful BeginPaint above for this hwnd.
            // EndPaint is documented to always return nonzero, so there is no failure to handle.
            let _always_nonzero = EndPaint(self.hwnd, &raw const paint);
        }
    }

    pub(super) fn request_redraw(&mut self) {
        self.set_hook_search_active(self.is_visible());
        self.sync_filter_popup();
        let source = self
            .session
            .switcher()
            .selected_task()
            .map(|task| HWND(task.window_handle as *mut c_void));
        if let Some(preview) = &mut self.preview
            && let Err(error) = preview.set_source(source)
        {
            eprintln!("Could not update the DWM preview: {error}");
        }
        let invalidated = unsafe {
            // SAFETY: the HWND is live; a null rectangle invalidates the complete client area.
            InvalidateRect(Some(self.hwnd), None, false)
        };
        if !invalidated.as_bool() {
            eprintln!("Could not invalidate the overlay: {}", Error::from_thread());
        }
    }
}
