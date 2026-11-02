//! Immutable paint snapshots remain readable while native calls re-enter the dialog.
use super::{
    DialogFonts, DialogState,
    controls::{DialogControls, selected_index},
    selectors::Previews,
    theme::ThemePalette,
};
use crate::native_drawing::OwnedBrush;
use alttabio::{
    failure_run::SharedFailureRun,
    settings::IconColor,
    settings_form::{Control, Selector, SettingsLayout, SettingsPages},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::Win32::Foundation::HWND;

pub(super) type PaintCell = RefCell<Option<Rc<PaintState>>>;
pub(super) type PaintCache = Rc<PaintCell>;

pub(super) struct PaintState {
    pub(super) hwnd: HWND,
    pub(super) controls: DialogControls,
    pub(super) fonts: Option<Rc<DialogFonts>>,
    pub(super) previews: Option<Rc<Previews>>,
    pub(super) background: Option<Rc<OwnedBrush>>,
    pub(super) palette: ThemePalette,
    pub(super) dpi: u32,
    pub(super) pages: SettingsPages,
    pub(super) hovered: Rc<Cell<HWND>>,
    pub(super) begin_paint_failures: Rc<SharedFailureRun>,
    layout: SettingsLayout,
    icon: IconColor,
    checked_selection: [Option<usize>; 2],
}

impl PaintState {
    pub(super) const fn layout(&self) -> SettingsLayout {
        self.layout
    }
    pub(super) const fn selected_icon(&self) -> IconColor {
        self.icon
    }
    pub(super) const fn checked_index(&self, selector: Selector) -> Option<usize> {
        self.checked_selection[selector as usize]
    }
}

impl DialogState {
    pub(super) fn paint_state(&self) -> PaintState {
        PaintState {
            hwnd: self.hwnd,
            controls: self.controls.clone(),
            fonts: self.fonts.clone(),
            previews: self.previews.clone(),
            background: self.background.clone(),
            palette: self.palette,
            dpi: self.dpi,
            pages: self.pages.clone(),
            hovered: Rc::clone(&self.hovered),
            begin_paint_failures: Rc::clone(&self.begin_paint_failures),
            layout: self.layout(),
            icon: self.selected_icon(),
            checked_selection: [Selector::Theme, Selector::Icon].map(|selector| {
                self.dropdown_selection[selector as usize]
                    .or_else(|| selected_index(self.controls.get(Control::Selector(selector))))
            }),
        }
    }

    pub(super) fn refresh_paint_state(&self) {
        // Build before borrowing the cache. Native queries must never run while its cell
        // is mutably borrowed, and every callback clones the snapshot before forwarding.
        let snapshot = Rc::new(self.paint_state());
        self.paint.replace(Some(snapshot));
    }
}
