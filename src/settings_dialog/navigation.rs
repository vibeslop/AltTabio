//! Native placement and scrolling of the selected category.
use super::DialogState;
use crate::{dialog_host, win32};
use alttabio::dialog_layout::{Size, scale};
use alttabio::settings_form::{Category, Control, SettingsLayout};
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetScrollInfo, SB_VERT, SCROLLINFO, SIF_ALL, SIF_DISABLENOSCROLL, SIF_TRACKPOS,
    SetWindowTextW,
};
use windows::core::{PCWSTR, Result};

impl DialogState {
    pub(super) fn layout(&self) -> SettingsLayout {
        SettingsLayout::for_client(self.client, self.dpi, self.pages.category)
    }

    pub(super) fn resize(&mut self) -> Result<()> {
        let mut rect = RECT::default();
        // SAFETY: hwnd is the live dialog and rect is writable.
        unsafe {
            GetClientRect(self.hwnd, &raw mut rect)?;
        }
        self.client = Size::new(rect.right, rect.bottom);
        self.place_controls()
    }

    pub(super) fn place_controls(&mut self) -> Result<()> {
        let layout = self.layout();
        self.pages.scroll_to(self.pages.offset(), &layout);
        self.refresh_paint_state();
        self.controls.apply_layout(&layout, &self.pages)?;
        self.update_scrollbar();
        self.redraw();
        Ok(())
    }

    pub(super) fn select_category(&mut self, category: Category) -> Result<()> {
        self.pages.select(category);
        for (control, text) in [
            (Control::PageHeading, category.title()),
            (Control::PageIntro, category.intro()),
        ] {
            let text = win32::wide(text);
            // SAFETY: both child HWNDs and the text buffer are live for this synchronous call.
            unsafe {
                SetWindowTextW(self.controls.get(control), PCWSTR(text.as_ptr()))?;
            }
        }
        self.place_controls()
    }

    pub(super) fn focus_category(&mut self, category: Category) -> Result<()> {
        self.select_category(category)?;
        dialog_host::focus(self.controls.get(Control::Navigation(category)))
    }

    pub(super) fn scroll_to(&mut self, offset: i32) -> Result<()> {
        let layout = self.layout();
        self.pages.scroll_to(offset, &layout);
        self.refresh_paint_state();
        self.controls.scroll_page(&layout, &self.pages)?;
        self.update_scrollbar();
        self.redraw();
        Ok(())
    }

    pub(super) fn scroll_wheel(&mut self, delta: i32) -> Result<()> {
        let layout = self.layout();
        self.pages.wheel(delta, scale(45, self.dpi), &layout);
        self.scroll_to(self.pages.offset())
    }

    pub(super) fn scroll_bar(&mut self, command: u16) -> Result<()> {
        let layout = self.layout();
        let current = self.pages.offset();
        let step = scale(30, self.dpi);
        let page = (layout.viewport.height - step).max(step);
        let offset = match command {
            0 => current - step,
            1 => current + step,
            2 => current - page,
            3 => current + page,
            4 | 5 => {
                let mut info = SCROLLINFO {
                    cbSize: u32::try_from(size_of::<SCROLLINFO>()).unwrap_or_default(),
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                // SAFETY: the live viewport owns a vertical scrollbar and info is correctly sized.
                unsafe {
                    GetScrollInfo(self.controls.get(Control::Viewport), SB_VERT, &raw mut info)?;
                }
                info.nTrackPos
            }
            6 => 0,
            7 => layout.max_scroll(self.pages.category),
            _ => return Ok(()),
        };
        self.scroll_to(offset)
    }

    pub(super) fn reveal_control(&mut self, hwnd: HWND) -> Result<()> {
        let Some(control) = self.controls.find(hwnd) else {
            return Ok(());
        };
        if control.category() != Some(self.pages.category) {
            return Ok(());
        }
        let layout = self.layout();
        let previous = self.pages.offset();
        self.pages.reveal(control.rect(&layout), &layout);
        // Moving an already-visible native combo closes the popup it just opened.
        if self.pages.offset() == previous {
            return Ok(());
        }
        self.scroll_to(self.pages.offset())
    }

    fn update_scrollbar(&self) {
        let layout = self.layout();
        let info = SCROLLINFO {
            cbSize: u32::try_from(size_of::<SCROLLINFO>()).unwrap_or_default(),
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMax: layout.content_height(self.pages.category) - 1,
            nPage: u32::try_from(layout.viewport.height).unwrap_or_default(),
            nPos: self.pages.offset(),
            ..Default::default()
        };
        // SAFETY: this is the dialog's live viewport and a correctly sized scalar scroll range.
        // SetScrollInfo returns the previous position and does not expose a failure sentinel.
        unsafe {
            SetScrollInfo(
                self.controls.get(Control::Viewport),
                SB_VERT,
                &raw const info,
                true,
            );
        }
    }
}
