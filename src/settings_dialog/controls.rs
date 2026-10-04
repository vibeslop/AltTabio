//! Creation and state of the Settings child controls, one window per entry of `Control::all`.

use super::DialogFonts;
use crate::dialog_host::wide;
use alttabio::dialog_layout::Rect;
use alttabio::settings::Settings;
use alttabio::settings_form::{Control, DialogButton, Selector, SettingOption, SettingsLayout};
use std::ffi::c_void;
use windows::Win32::Foundation::{E_FAIL, HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CBS_HASSTRINGS,
    CreateWindowExW, HMENU, MoveWindow, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_SETFONT,
    WS_CHILD, WS_GROUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{Error, PCWSTR, Result, w};

/// The window of each entry of `Control::all`, in creation order.
#[derive(Default)]
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
        Control::all()
            .map(|control| {
                let rect = control.window_rect(layout, dpi);
                create_child(parent, instance, control, rect, fonts.of(control), settings)
                    .map(|hwnd| (control, hwnd))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    #[cfg(test)]
    pub(super) const fn from_handles(handles: Vec<(Control, HWND)>) -> Self {
        Self(handles)
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

    pub(super) fn apply_layout(&self, layout: &SettingsLayout, dpi: u32) -> Result<()> {
        for (control, hwnd) in self.iter() {
            move_control(hwnd, control.window_rect(layout, dpi))?;
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
    let (class, style) = match control {
        Control::Group(_) => (w!("BUTTON"), WINDOW_STYLE(BS_GROUPBOX as u32)),
        Control::Label(_) => (w!("STATIC"), WINDOW_STYLE::default()),
        Control::Selector(_) => (
            w!("COMBOBOX"),
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS).cast_unsigned()),
        ),
        Control::Checkbox(option) => {
            let group_style = if option == SettingOption::Autostart {
                WS_GROUP
            } else {
                WINDOW_STYLE::default()
            };
            (
                w!("BUTTON"),
                WS_TABSTOP | group_style | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            )
        }
        Control::Button(button) => {
            let button_style = if button == DialogButton::Ok {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            };
            (
                w!("BUTTON"),
                WS_TABSTOP | WINDOW_STYLE(button_style.cast_unsigned()),
            )
        }
    };
    let hwnd = create_control(
        parent,
        instance,
        class,
        control.text(),
        WS_CHILD | WS_VISIBLE | style,
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
            WINDOW_EX_STYLE::default(),
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
        MoveWindow(control, rect.x, rect.y, rect.width, rect.height, true)
    }
}

pub(super) fn is_checked(control: HWND) -> bool {
    let result = unsafe {
        // SAFETY: control is a live checkbox and BM_GETCHECK has no pointer payload.
        SendMessageW(control, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    result.0 == 1
}
