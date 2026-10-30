//! Shared preview content for the selected fields and the native drop-down lists.

use super::paint_state::PaintState;
use crate::app_icon;
use crate::native_drawing::{
    DRAW_TEXT_END_ELLIPSIS, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE, DRAW_TEXT_VCENTER,
    draw_text_with_font, fill_color, frame_color, rgb,
};
use alttabio::dialog_layout::{Point, hairline, scale};
use alttabio::settings::{IconColor, Theme};
use alttabio::settings_form::{Control, Selector, THEME_CHOICES};
use alttabio::theme::{ResolvedTheme, Rgb8};
use windows::Win32::Foundation::{COLORREF, E_FAIL, HINSTANCE, LRESULT, RECT};
use windows::Win32::Graphics::Gdi::{DrawFocusRect, HDC};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED,
    ODT_COMBOBOX,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON};
use windows::core::{Error, Result};

pub(super) struct Previews {
    // LoadIconW borrows shared executable resources, so these handles must not be destroyed.
    icons: [HICON; IconColor::ALL.len()],
    logos: Vec<app_icon::OwnedIcon>,
    github: [app_icon::OwnedIcon; 2],
}

impl Previews {
    pub(super) fn logo(&self, color: IconColor) -> HICON {
        self.logos[icon_index(color)].handle()
    }
    pub(super) fn github(&self, dark: bool) -> HICON {
        self.github[usize::from(dark)].handle()
    }
    pub(super) fn load(instance: HINSTANCE, dpi: u32) -> Result<Self> {
        let mut icons = [HICON::default(); IconColor::ALL.len()];
        for (slot, icon) in icons.iter_mut().zip(IconColor::ALL) {
            *slot = app_icon::load_app(instance, icon)?;
        }
        let logos = IconColor::ALL
            .into_iter()
            .map(|color| app_icon::load_app_sized(instance, color, scale(42, dpi)))
            .collect::<Result<Vec<_>>>()?;
        let github = [
            app_icon::load_github(instance, false, scale(18, dpi))?,
            app_icon::load_github(instance, true, scale(18, dpi))?,
        ];
        Ok(Self {
            icons,
            logos,
            github,
        })
    }
}

fn icon_index(color: IconColor) -> usize {
    IconColor::ALL
        .iter()
        .position(|candidate| *candidate == color)
        .unwrap_or_default()
}

pub(super) fn paint_list_item(item: &DRAWITEMSTRUCT, state: &PaintState) -> Option<LRESULT> {
    if item.CtlType != ODT_COMBOBOX {
        return None;
    }
    let Control::Selector(selector) = state.controls.find(item.hwndItem)? else {
        return None;
    };
    let selected =
        item.itemState.0 & ODS_SELECTED.0 != 0 && item.itemState.0 & ODS_COMBOBOXEDIT.0 == 0;
    let color = if item.itemState.0 & ODS_DISABLED.0 != 0 {
        state.palette.label_text(false)
    } else {
        state.palette.text
    };
    let result = (|| {
        fill_color(
            item.hDC,
            item.rcItem,
            if selected {
                state.palette.selected
            } else {
                state.palette.control_surface
            },
        )?;
        // Windows uses -1 for an empty combo box. Paint its surface, with no entry.
        if item.itemID != u32::MAX {
            let popup = item.itemState.0 & ODS_COMBOBOXEDIT.0 == 0;
            let mut entry = item.rcItem;
            if popup {
                entry.right -= scale(16, state.dpi);
            }
            paint_entry(
                item.hDC,
                entry,
                selector,
                usize::try_from(item.itemID).unwrap_or(usize::MAX),
                color,
                state,
            )?;
            // Native CB_GETCURSEL follows the hot row while the popup is tracking.
            // Keep the check on the choice captured before tracking began.
            if popup && state.checked_index(selector) == usize::try_from(item.itemID).ok() {
                let x = item.rcItem.right - scale(16, state.dpi);
                let y = item.rcItem.top + (item.rcItem.bottom - item.rcItem.top) / 2;
                super::drawing::polyline(
                    item.hDC,
                    &[
                        Point::new(x, y),
                        Point::new(x + scale(3, state.dpi), y + scale(3, state.dpi)),
                        Point::new(x + scale(8, state.dpi), y - scale(3, state.dpi)),
                    ],
                    color,
                    hairline(state.dpi),
                )?;
            }
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
            let inset = scale(2, state.dpi);
            let focus = RECT {
                left: item.rcItem.left + inset,
                top: item.rcItem.top + inset,
                right: item.rcItem.right - inset,
                bottom: item.rcItem.bottom - inset,
            };
            // SAFETY: the native owner-draw message supplies this live DC and item rectangle.
            if !unsafe { DrawFocusRect(item.hDC, &raw const focus) }.as_bool() {
                return Err(Error::from_thread());
            }
        }
        Ok::<(), Error>(())
    })();
    if let Err(error) = result {
        eprintln!("Could not paint a settings choice preview: {error}");
    }
    Some(LRESULT(1))
}

pub(super) fn paint_entry(
    dc: HDC,
    rect: RECT,
    selector: Selector,
    index: usize,
    text_color: COLORREF,
    state: &PaintState,
) -> Result<()> {
    let entries = selector.entries();
    let Some(label) = entries.get(index) else {
        return Ok(());
    };
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    let padding = scale(8, state.dpi);
    let size = scale(20, state.dpi);
    let width = scale(if selector == Selector::Icon { 20 } else { 32 }, state.dpi);
    let preview = RECT {
        left: rect.left + padding,
        top: rect.top + (rect.bottom - rect.top - size) / 2,
        right: rect.left + padding + width,
        bottom: rect.top + (rect.bottom - rect.top - size) / 2 + size,
    };
    match selector {
        Selector::Icon => {
            let icon = state
                .previews
                .as_ref()
                .and_then(|previews| previews.icons.get(index))
                .ok_or_else(|| Error::from_hresult(E_FAIL))?;
            unsafe {
                // SAFETY: dc is borrowed for this synchronous paint, and icon is a shared
                // executable resource kept live by the application module. Sizes are positive.
                DrawIconEx(
                    dc,
                    preview.left,
                    preview.top,
                    *icon,
                    size,
                    size,
                    0,
                    None,
                    DI_NORMAL,
                )?;
            }
        }
        Selector::Theme => {
            paint_theme_preview(dc, preview, THEME_CHOICES.value_at(index), state.dpi)?;
        }
    }
    draw_text_with_font(
        dc,
        label,
        RECT {
            left: preview.right + padding,
            right: rect.right - padding,
            ..rect
        },
        text_color,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        fonts.body.0,
    )
}

fn paint_theme_preview(dc: HDC, rect: RECT, theme: Theme, dpi: u32) -> Result<()> {
    match theme {
        Theme::Light => paint_theme_sample(dc, rect, ResolvedTheme::Light, dpi),
        Theme::Dark => paint_theme_sample(dc, rect, ResolvedTheme::Dark, dpi),
        Theme::Auto => {
            let middle = rect.left + (rect.right - rect.left) / 2;
            paint_theme_sample(
                dc,
                RECT {
                    right: middle,
                    ..rect
                },
                ResolvedTheme::Light,
                dpi,
            )?;
            paint_theme_sample(
                dc,
                RECT {
                    left: middle,
                    ..rect
                },
                ResolvedTheme::Dark,
                dpi,
            )
        }
    }
}

fn paint_theme_sample(dc: HDC, rect: RECT, theme: ResolvedTheme, dpi: u32) -> Result<()> {
    let palette = theme.palette();
    fill_color(dc, rect, rgb_color(palette.background))?;
    let inset = scale(3, dpi);
    let row = RECT {
        left: rect.left + inset,
        top: rect.top + inset,
        right: rect.right - inset,
        bottom: rect.top + scale(9, dpi),
    };
    fill_color(dc, row, rgb_color(palette.selected))?;
    for (y, ink) in [(5, palette.primary), (13, palette.secondary)] {
        fill_color(
            dc,
            RECT {
                left: row.left + scale(2, dpi),
                top: rect.top + scale(y, dpi),
                right: row.right - scale(2, dpi),
                bottom: rect.top + scale(y + 1, dpi),
            },
            rgb_color(ink),
        )?;
    }
    frame_color(dc, rect, rgb_color(palette.window_border), hairline(dpi))
}

const fn rgb_color(color: Rgb8) -> COLORREF {
    rgb(color.red, color.green, color.blue)
}

#[cfg(test)]
#[path = "selectors_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "selector_hover_tests.rs"]
mod hover_tests;
