//! Pointer events on the panel, decided by `panel_pointer` and applied here.

use super::App;
use crate::macos::overlay::ViewEvent;
use crate::macos::runtime::{run_later, schedule, with_app};
use alttabio::app_switcher::Action;
use alttabio::panel_pointer::{Dwell, MenuFor, Pointer, Response, TILE_DWELL_SECONDS};
use alttabio::window_command::WindowCommand;
use objc2_app_kit::NSEvent;

impl App {
    pub(super) fn reset_pointer(&mut self) {
        self.pointer = Pointer::default();
        self.cancel_dwell();
    }

    pub(super) fn handle_view_event(&mut self, event: ViewEvent) {
        if !self.switcher.is_active() {
            return;
        }
        let Some(shown) = self.shown else {
            return;
        };
        let selected_app = self.switcher.selected_app_index();
        let selected_window = self.switcher.selected_window();
        match event {
            ViewEvent::MouseMoved(x, y) => {
                let location = NSEvent::mouseLocation();
                let response = self.pointer.moved(
                    shown.hit(x, y),
                    (location.x, location.y),
                    &shown,
                    selected_app,
                );
                self.apply_pointer(response);
            }
            ViewEvent::MouseDown(x, y) => {
                let response =
                    self.pointer
                        .pressed(shown.hit(x, y), &shown, selected_app, selected_window);
                self.apply_pointer(response);
            }
            ViewEvent::MouseUp(x, y) => {
                let response = self.pointer.released(shown.hit(x, y), selected_window);
                self.apply_pointer(response);
            }
            ViewEvent::RightMouseDown(x, y) => {
                let response = self
                    .pointer
                    .right_pressed(shown.hit(x, y), &shown, selected_app);
                self.apply_pointer(response);
                if let Some(menu) = response.menu {
                    self.show_context_menu(menu, x, y);
                }
            }
            ViewEvent::MouseExited => {
                let response = self.pointer.exited();
                self.apply_pointer(response);
            }
            ViewEvent::Scroll(step) => self.apply_action(Action::StepWindow(step)),
        }
    }

    fn apply_pointer(&mut self, response: Response) {
        match response.dwell {
            Dwell::Keep => {}
            Dwell::Cancel => self.cancel_dwell(),
            Dwell::Start(app) => {
                self.cancel_dwell();
                self.dwell_timer = Some(schedule(self.mtm, TILE_DWELL_SECONDS, move |state| {
                    state.finish_dwell(app);
                }));
            }
        }
        if let Some((app, window)) = response.select {
            self.select(app, window);
        }
        if let Some(action) = response.action {
            self.apply_action(action);
        }
        if response.redraw {
            self.redraw();
        }
    }

    /// Selects the app at `app` and its window `window`, or its last-used window.
    fn select(&mut self, app: usize, window: Option<usize>) {
        let before = self.switcher.selected_target();
        if self.switcher.select(app, window) {
            self.selection_changed(before);
        }
    }

    fn finish_dwell(&mut self, app: usize) {
        self.dwell_timer = None;
        if self.switcher.is_active() {
            let response = self.pointer.dwell_elapsed(app);
            self.apply_pointer(response);
        }
    }

    pub(super) fn cancel_dwell(&mut self) {
        if let Some(timer) = self.dwell_timer.take() {
            timer.invalidate();
        }
    }

    fn show_context_menu(&mut self, menu: MenuFor, x: f64, y: f64) {
        let window = menu == MenuFor::Window && self.switcher.selected_window().is_some();
        let app_name = self
            .switcher
            .selected_app()
            .map(|app| app.name.clone())
            .unwrap_or_default();
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        if self.switcher.open_context_menu() {
            run_later(self.mtm, move || {
                let command = overlay.show_context_menu(x, y, window, &app_name);
                let _ = with_app(|app| app.finish_context_menu(command));
            });
        }
    }

    fn finish_context_menu(&mut self, command: Option<WindowCommand>) {
        let before = self.switcher.selected_target();
        let effect = self.switcher.finish_context_menu(command);
        self.apply_effect(effect, before);
    }
}
