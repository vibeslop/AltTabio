//! Creation and state of the Settings child controls, one window per entry of `Control::all`.

use super::DialogFonts;
use crate::win32::wide;
use alttabio::dialog_layout::{Rect, scale};
use alttabio::settings::Settings;
use alttabio::settings_form::{
    Control, DialogButton, SELECTOR_ITEM_HEIGHT, Selector, SettingOption, SettingsLayout,
};
use std::ffi::c_void;
use windows::Win32::Foundation::{E_FAIL, HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, BS_RADIOBUTTON,
    CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_SETCURSEL, CB_SETITEMHEIGHT, CBS_DROPDOWNLIST,
    CBS_HASSTRINGS, CBS_OWNERDRAWFIXED, CreateWindowExW, HMENU, MoveWindow, SW_HIDE, SW_SHOW,
    SendMessageW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_SETFONT, WS_CHILD, WS_CLIPCHILDREN,
    WS_EX_CONTROLPARENT, WS_GROUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{Error, PCWSTR, Result, w};
// SS_OWNERDRAW is otherwise exposed only by the broad SystemServices bindings.
const OWNER_DRAW_STATIC: WINDOW_STYLE = WINDOW_STYLE(0x000d);

/// The window of each entry of `Control::all`, in creation order.
#[derive(Clone, Default)]
pub(super) struct DialogControls(Vec<(Control, HWND)>);

impl DialogControls {
    pub(super) fn create(
        parent: HWND,
        instance: HINSTANCE,
        layout: &SettingsLayout,
        dpi: u32,
        fonts: &DialogFonts,
        settings: &Settings,
    ) -> Result<Self> {
        let mut controls = Self::default();
        for control in Control::all() {
            let rect = control.window_rect(layout, dpi);
            let control_parent = if control.category().is_some() {
                controls.get(Control::Viewport)
            } else {
                parent
            };
            let hwnd = create_child(
                control_parent,
                instance,
                control,
                rect,
                fonts.of(control),
                settings,
            )?;
            if let Control::Selector(_) = control {
                size_selector_items(hwnd, dpi)?;
            }
            controls.0.push((control, hwnd));
        }
        Ok(controls)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (Control, HWND)> + '_ {
        self.0.iter().copied()
    }

    /// The window of `control`, or a null handle before the controls exist.
    pub(super) fn get(&self, control: Control) -> HWND {
        self.iter()
            .find(|(candidate, _)| *candidate == control)
            .map_or_else(HWND::default, |(_, hwnd)| hwnd)
    }

    pub(super) fn find(&self, hwnd: HWND) -> Option<Control> {
        self.iter()
            .find(|(_, candidate)| *candidate == hwnd)
            .map(|(control, _)| control)
    }

    pub(super) fn checkbox(&self, option: SettingOption) -> HWND {
        self.get(Control::Checkbox(option))
    }

    pub(super) fn apply_layout(
        &self,
        layout: &SettingsLayout,
        pages: &alttabio::settings_form::SettingsPages,
    ) -> Result<()> {
        for (control, hwnd) in self.iter() {
            if let Control::Selector(_) = control {
                size_selector_items(hwnd, layout.dpi)?;
            }
            let mut rect = control.window_rect(layout, layout.dpi);
            if let Some(category) = control.category() {
                // SAFETY: these child windows are owned by the live dialog. ShowWindow
                // reports their previous visibility, not a fallible status.
                unsafe {
                    let _previous = ShowWindow(
                        hwnd,
                        if category == pages.category {
                            SW_SHOW
                        } else {
                            SW_HIDE
                        },
                    );
                }
                if category == pages.category {
                    rect.y -= pages.offset();
                }
            }
            move_control(hwnd, rect)?;
            if let Control::Navigation(category) = control {
                // SAFETY: hwnd is a live radio button. BM_SETCHECK has scalar payloads.
                unsafe {
                    SendMessageW(
                        hwnd,
                        BM_SETCHECK,
                        Some(WPARAM(usize::from(category == pages.category))),
                        None,
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn scroll_page(
        &self,
        layout: &SettingsLayout,
        pages: &alttabio::settings_form::SettingsPages,
    ) -> Result<()> {
        for (control, hwnd) in self
            .iter()
            .filter(|(control, _)| control.category() == Some(pages.category))
        {
            let mut rect = control.window_rect(layout, layout.dpi);
            rect.y -= pages.offset();
            move_control(hwnd, rect)?;
        }
        Ok(())
    }

    pub(super) fn apply_fonts(&self, fonts: &DialogFonts) {
        for (control, hwnd) in self.iter() {
            set_control_font(hwnd, fonts.of(control));
        }
    }
}

fn create_child(
    parent: HWND,
    instance: HINSTANCE,
    control: Control,
    rect: Rect,
    font: HFONT,
    settings: &Settings,
) -> Result<HWND> {
    let (class, kind_style) = match control {
        Control::Viewport => (w!("STATIC"), WS_VSCROLL | WS_CLIPCHILDREN),
        Control::Navigation(_) => (w!("BUTTON"), WINDOW_STYLE(BS_RADIOBUTTON.cast_unsigned())),
        Control::Selector(_) => (
            w!("COMBOBOX"),
            WS_VSCROLL
                | WINDOW_STYLE(
                    (CBS_DROPDOWNLIST | CBS_HASSTRINGS | CBS_OWNERDRAWFIXED).cast_unsigned(),
                ),
        ),
        Control::Checkbox(_) => (w!("BUTTON"), WINDOW_STYLE(BS_AUTOCHECKBOX as u32)),
        Control::Button(button) => {
            let button_style = if button == DialogButton::Ok {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            };
            (w!("BUTTON"), WINDOW_STYLE(button_style.cast_unsigned()))
        }
        // These labels are fully painted by our subclass. SS_OWNERDRAW prevents
        // STATIC's synchronous text redraw from falling back to system colors while
        // the category command still owns the dialog state.
        _ => (w!("STATIC"), OWNER_DRAW_STATIC),
    };
    let mut style = WS_CHILD | WS_VISIBLE | kind_style;
    if control.is_tab_stop() {
        style |= WS_TABSTOP;
    }
    if control == Control::Navigation(alttabio::settings_form::Category::General)
        || control == Control::Button(DialogButton::Cancel)
    {
        style |= WS_GROUP;
    }
    let hwnd = create_control(
        parent,
        instance,
        class,
        control.text(),
        style,
        control.id(),
        rect,
        font,
    )?;
    match control {
        Control::Checkbox(option) if option.read(settings) => unsafe {
            // SAFETY: hwnd is a live checkbox and BM_SETCHECK consumes scalar values only. It
            // always returns zero.
            SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(1)), Some(LPARAM(0)));
        },
        Control::Selector(selector) => fill_selector(hwnd, selector, settings)?,
        _ => {}
    }
    Ok(hwnd)
}

fn size_selector_items(hwnd: HWND, dpi: u32) -> Result<()> {
    // The selected content plus the native frame fits the 35px layout row.
    for (index, height) in [(usize::MAX, 29), (0, SELECTOR_ITEM_HEIGHT)] {
        let result = unsafe {
            // SAFETY: hwnd is a live owner-drawn combo. -1 addresses its selected field;
            // zero sets the fixed height of every list item. Payloads are scalar pixels.
            SendMessageW(
                hwnd,
                CB_SETITEMHEIGHT,
                Some(WPARAM(index)),
                Some(LPARAM(scale(height, dpi) as isize)),
            )
        };
        if result.0 == CB_ERR as isize {
            return Err(Error::new(E_FAIL, "could not size settings preview items"));
        }
    }
    Ok(())
}

fn fill_selector(hwnd: HWND, selector: Selector, settings: &Settings) -> Result<()> {
    for entry in selector.entries() {
        let text = wide(entry);
        let result = unsafe {
            // SAFETY: hwnd is a live combo box and text remains valid throughout the synchronous
            // insertion.
            SendMessageW(
                hwnd,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            )
        };
        if i32::try_from(result.0).unwrap_or(CB_ERR) < 0 {
            return Err(Error::from_hresult(E_FAIL));
        }
    }
    let result = unsafe {
        // SAFETY: hwnd is a live combo box and the index refers to one of the inserted entries.
        SendMessageW(
            hwnd,
            CB_SETCURSEL,
            Some(WPARAM(selector.index(settings))),
            Some(LPARAM(0)),
        )
    };
    if i32::try_from(result.0).unwrap_or(CB_ERR) == CB_ERR {
        Err(Error::from_hresult(E_FAIL))
    } else {
        Ok(())
    }
}

/// The index of the entry selected in a combo box, or `None` while it has no selection.
pub(super) fn selected_index(combo: HWND) -> Option<usize> {
    let selected = unsafe {
        // SAFETY: combo is a live drop-down-list control with scalar message payloads.
        SendMessageW(combo, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    let selected = i32::try_from(selected.0).unwrap_or(CB_ERR);
    (selected != CB_ERR).then(|| usize::try_from(selected).unwrap_or_default())
}

pub(super) fn is_control_enabled(hwnd: HWND) -> bool {
    unsafe {
        // SAFETY: hwnd is a live child control.
        IsWindowEnabled(hwnd).as_bool()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "parameters map directly to one CreateWindowExW child-control call"
)]
fn create_control(
    parent: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    label: &str,
    style: WINDOW_STYLE,
    id: Option<usize>,
    rect: Rect,
    font: HFONT,
) -> Result<HWND> {
    let text = wide(label);
    let menu = id.map(|value| HMENU(value as *mut c_void));
    let control = unsafe {
        // SAFETY: class/text buffers remain live for the synchronous creation call; parent and
        // instance are live and the HMENU value is a documented child-control identifier.
        CreateWindowExW(
            if style.contains(WS_CLIPCHILDREN) {
                WS_EX_CONTROLPARENT
            } else {
                WINDOW_EX_STYLE::default()
            },
            class,
            PCWSTR(text.as_ptr()),
            style,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            Some(parent),
            menu,
            Some(instance),
            None,
        )
    }?;
    set_control_font(control, font);
    Ok(control)
}

fn set_control_font(control: HWND, font: HFONT) {
    unsafe {
        // SAFETY: control is live and WM_SETFONT borrows the dialog-owned font handle.
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

fn move_control(control: HWND, rect: Rect) -> Result<()> {
    unsafe {
        // SAFETY: control is a live child HWND and rect contains bounded, DPI-scaled coordinates.
        // Queue painting after the layout releases its mutable dialog-state borrow.
        MoveWindow(control, rect.x, rect.y, rect.width, rect.height, false)
    }
}

pub(super) fn is_checked(control: HWND) -> bool {
    let result = unsafe {
        // SAFETY: control is a live checkbox and BM_GETCHECK has no pointer payload.
        SendMessageW(control, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    result.0 == 1
}
