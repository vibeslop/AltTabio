//! Showing, drawing, and hiding the switcher panel.

use super::{App, frontmost_pid};
use crate::macos::overlay::{FrameModel, Overlay, Row, Tile};
use crate::macos::permissions;
use crate::macos::runtime::{run_later, schedule, with_app};
use alttabio::panel_layout::{
    Extent, Layout, ListRows, Shown, WindowState, row_number, scroll_into_view,
};
use alttabio::theme::{ResolvedTheme, SwitcherTokens, resolve};
use objc2::rc::Retained;
use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSEvent};
use objc2_foundation::{NSArray, NSTimer};

// The panel waits this long after ⌘ Tab. A quick press and release switches before it passes,
// so flipping between two windows never flashes the panel, as with the system switcher.
const REVEAL_SECONDS: f64 = 0.12;

/// Whether the panel is on screen. After ⌘ Tab the session is open while the panel waits for its
/// timer; nothing draws until it fires.
pub(super) enum Panel {
    Hidden,
    Waiting(Retained<NSTimer>),
    Shown,
}

impl App {
    pub(super) fn show_overlay(&mut self, step: Option<i32>) {
        self.note_focus();
        self.switcher
            .open(self.app_entries(), step, frontmost_pid());
        if !self.switcher.is_active() {
            self.hide_overlay();
            return;
        }
        self.reset_pointer();
        self.extent = Extent::default();
        self.extent.widen(self.switcher.apps());
        self.tile_start = 0;
        self.row_start = 0;
        self.shown = None;
        self.reset_previews();
        self.preview.start_session();
        self.hotkey.set_overlay_active(true);
        self.request_refresh();
        // Only the keyboard gesture waits; a list opened from the menu bar shows at once.
        if step.is_some() {
            self.panel = Panel::Waiting(schedule(self.mtm, REVEAL_SECONDS, App::reveal));
        } else {
            self.reveal();
        }
    }

    fn reveal(&mut self) {
        self.panel = Panel::Hidden;
        if !self.switcher.is_active() {
            return;
        }
        let theme = self.resolved_theme();
        if let Some(overlay) = self.overlay.clone() {
            overlay.set_theme(theme, SwitcherTokens::new(theme));
            overlay.show(self.layout(&overlay).size());
        }
        self.panel = Panel::Shown;
        let location = NSEvent::mouseLocation();
        self.pointer.panel_shown((location.x, location.y));
        self.redraw();
        self.request_preview_capture();
    }

    pub(super) fn hide_overlay(&mut self) {
        let showed_preview = matches!(self.panel, Panel::Shown)
            && self
                .shown
                .is_some_and(|shown| shown.layout.preview_size().is_some());
        self.switcher.hide();
        if let Panel::Waiting(timer) = std::mem::replace(&mut self.panel, Panel::Hidden) {
            timer.invalidate();
        }
        if let Some(overlay) = &self.overlay {
            overlay.hide();
        }
        self.hotkey.set_overlay_active(false);
        self.reset_pointer();
        self.shown = None;
        self.reset_previews();
        if showed_preview
            && !self.screen_recording_asked
            && !permissions::screen_recording_granted()
        {
            // The system prompt takes focus, so it waits until the panel is gone and the
            // switch is done.
            self.screen_recording_asked = true;
            run_later(self.mtm, || {
                let _ = with_app(App::ask_for_screen_recording);
            });
        }
        if self.preview_mode {
            self.shutdown();
            NSApplication::sharedApplication(self.mtm).terminate(None);
        }
    }

    pub(super) fn resolved_theme(&self) -> ResolvedTheme {
        let appearance = NSApplication::sharedApplication(self.mtm).effectiveAppearance();
        let (aqua, dark) = unsafe {
            // SAFETY: the appearance name constants are static strings exported by AppKit.
            (NSAppearanceNameAqua, NSAppearanceNameDarkAqua)
        };
        let names = NSArray::from_slice(&[aqua, dark]);
        let system = if appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == dark)
        {
            ResolvedTheme::Dark
        } else {
            ResolvedTheme::Light
        };
        resolve(self.settings.appearance.theme, system)
    }

    fn layout(&self, overlay: &Overlay) -> Layout {
        Layout::new(
            self.extent.apps,
            self.extent.windows,
            self.settings.appearance.preview,
            overlay.max_size(),
        )
    }

    fn window_state(&self, window_handle: isize) -> WindowState {
        match self.record(window_handle) {
            Some(record) if record.is_minimized => WindowState::Minimized,
            Some(record) if record.is_hidden => WindowState::Hidden,
            Some(record) if !record.is_on_screen => WindowState::OtherDesktop,
            _ => WindowState::Normal,
        }
    }

    pub(super) fn redraw(&mut self) {
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        if !self.switcher.is_active() || !matches!(self.panel, Panel::Shown) {
            return;
        }
        let layout = self.layout(&overlay);
        overlay.resize(layout.size());
        let selected_app = self.switcher.selected_app_index().unwrap_or_default();
        let selected_process = self.switcher.selected_app().map(|app| app.process);
        if self.shown.and_then(|shown| shown.app) != selected_process {
            self.row_start = 0;
        }

        let tiles = self.tiles(layout, selected_app);
        let (rows, list) = self.rows(layout);
        let preview = self
            .settings
            .appearance
            .preview
            .then(|| self.preview_model());

        let shown = Shown {
            layout,
            app: selected_process,
            tile_start: self.tile_start,
            tiles: tiles.len(),
            row_start: self.row_start,
            rows: rows.len(),
            selected_row: list.selected_row,
        };
        self.shown = Some(shown);
        // Hit-test this frame, not the last one, before close_state draws its button.
        let selected_window = self.switcher.selected_window();
        let hit = overlay.mouse_point().and_then(|(x, y)| shown.hit(x, y));
        self.pointer.relocated(hit, selected_window);
        overlay.present(FrameModel {
            layout,
            tokens: SwitcherTokens::new(self.resolved_theme()),
            tiles,
            rows,
            empty_note: list.empty_note,
            more_note: list.more_note,
            close_state: self.pointer.close_state(selected_window),
            preview,
        });
    }

    /// The strip's tiles, scrolled so the selected app shows.
    fn tiles(&mut self, layout: Layout, selected_app: usize) -> Vec<Tile> {
        let apps = self.switcher.apps();
        self.tile_start =
            scroll_into_view(self.tile_start, selected_app, apps.len(), layout.tile_slots);
        apps.iter()
            .enumerate()
            .skip(self.tile_start)
            .take(layout.tile_slots)
            .map(|(index, app)| Tile {
                name: app.name.clone(),
                icon: i32::try_from(app.process.id)
                    .ok()
                    .and_then(|pid| self.icons.get(&pid).cloned()),
                selected: index == selected_app,
            })
            .collect()
    }

    /// The selected app's rows scrolled so the selected window shows, and where they sit in
    /// its list.
    fn rows(&mut self, layout: Layout) -> (Vec<Row>, ListRows) {
        let windows = self
            .switcher
            .selected_app()
            .map_or(&[][..], |app| &app.windows[..]);
        let selected_window = self.switcher.selected_window_index();
        let list = ListRows::new(
            self.row_start,
            selected_window,
            windows.len(),
            layout.row_slots,
        );
        self.row_start = list.start;
        let rows = windows
            .iter()
            .enumerate()
            .skip(list.start)
            .take(list.count)
            .map(|(index, handle)| Row {
                number: row_number(index),
                title: self
                    .record(*handle)
                    .map(|record| record.title.clone())
                    .unwrap_or_default(),
                state: self.window_state(*handle),
                selected: selected_window == Some(index),
            })
            .collect();
        (rows, list)
    }
}
