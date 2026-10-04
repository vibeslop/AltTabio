use super::App;
use super::activation::request_foreground;
use super::display::position_on_cursor_monitor;
use crate::preview::DwmPreview;
use crate::renderer::{CloseButtonVisualState, RenderOptions, Renderer};
use crate::task_query::{EnumeratedTasks, enumerate_switchable_windows};
use alttabio::overlay_pointer;
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, SW_SHOWNA, ShowWindow};
use windows::core::Error;

impl App {
    pub(super) fn show_overlay(&mut self, selection_delta: Option<i32>) {
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

    pub(super) fn focus_overlay(&self) {
        // SAFETY: the HWND is live and owned by this UI thread.
        unsafe {
            if !request_foreground(self.hwnd) {
                eprintln!("Could not bring the overlay to the foreground");
            }
            if let Err(error) = SetFocus(Some(self.hwnd)) {
                eprintln!("Could not focus the overlay: {error}");
            }
        }
    }

    pub(super) fn hide_overlay(&mut self) {
        self.hide_overlay_with_reset(true);
    }

    pub(super) fn hide_overlay_with_reset(&mut self, reset_hook: bool) {
        self.stop_shell_dismissal();
        self.task_refresh.clear_notices();
        self.session.hide();
        self.set_hook_search_active(false);
        if reset_hook {
            self.reset_hook_gestures();
        }
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
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe {
            // SAFETY: `paint` is writable and BeginPaint/EndPaint are paired for this WM_PAINT.
            BeginPaint(self.hwnd, &raw mut paint)
        };
        if dc.is_invalid() {
            eprintln!("Could not begin painting the overlay");
        }
        let render_options = RenderOptions::from(&self.settings.appearance);
        let switcher = self.session.switcher();
        let selected_target = switcher.selected_task().map(|task| task.window_handle);
        if let Err(error) = self.renderer.draw(
            self.hwnd,
            switcher,
            self.preview.as_ref().and_then(DwmPreview::frame),
            render_options,
            renderer_close_button_state(self.close_button.visual_state(selected_target)),
        ) {
            eprintln!("Could not render the overlay: {error}");
        }
        // Direct2D draws through its own window target; only the GDI icons need the paint DC.
        if !dc.is_invalid() {
            Renderer::draw_icons(self.hwnd, dc, switcher, render_options);
        }
        unsafe {
            // SAFETY: this balances the BeginPaint call above for the same PAINTSTRUCT.
            // EndPaint is documented to always return nonzero, so there is no failure to handle.
            let _always_nonzero = EndPaint(self.hwnd, &raw const paint);
        }
    }

    pub(super) fn request_redraw(&mut self) {
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

// The renderer still declares its own copy of the library's close-button state.
const fn renderer_close_button_state(
    state: overlay_pointer::CloseButtonVisualState,
) -> CloseButtonVisualState {
    match state {
        overlay_pointer::CloseButtonVisualState::Normal => CloseButtonVisualState::Normal,
        overlay_pointer::CloseButtonVisualState::Hovered => CloseButtonVisualState::Hovered,
        overlay_pointer::CloseButtonVisualState::Pressed => CloseButtonVisualState::Pressed,
    }
}
