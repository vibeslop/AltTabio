//! Native theming of the Settings window and its controls, and the palette they paint with.

use super::controls::DialogControls;
use crate::dialog_host::{self, dark};
use crate::native_drawing::{rgb, system_color};
use crate::native_theme::DarkModeApi;
use alttabio::settings_form::Control;
use std::mem::size_of;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_WINDOW, COLOR_WINDOWTEXT, FillRect, HBRUSH, HDC, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, SendMessageW, WM_THEMECHANGED};
use windows::core::{Error, HRESULT, PCWSTR, Result, w};

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn SetWindowTheme(hwnd: HWND, sub_app_name: PCWSTR, sub_id_list: PCWSTR) -> HRESULT;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetComboBoxInfo(hwnd: HWND, info: *mut NativeComboBoxInfo) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeThemeClass {
    Explorer,
    DarkModeExplorer,
    Cfd,
}

impl NativeThemeClass {
    const fn name(self) -> PCWSTR {
        match self {
            Self::Explorer => w!("Explorer"),
            Self::DarkModeExplorer => w!("DarkMode_Explorer"),
            Self::Cfd => w!("CFD"),
        }
    }
}

#[derive(Clone, Copy)]
struct ThemeTarget {
    hwnd: HWND,
    kind: ThemeTargetKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThemeTargetKind {
    Standard,
    ComboBox,
    ComboList,
}

const fn native_theme_class(dark: bool, kind: ThemeTargetKind) -> NativeThemeClass {
    match kind {
        ThemeTargetKind::ComboBox => NativeThemeClass::Cfd,
        ThemeTargetKind::ComboList if dark => NativeThemeClass::DarkModeExplorer,
        ThemeTargetKind::Standard | ThemeTargetKind::ComboList => NativeThemeClass::Explorer,
    }
}

#[repr(C)]
struct NativeComboBoxInfo {
    size: u32,
    item_rect: RECT,
    button_rect: RECT,
    button_state: u32,
    combo: HWND,
    item: HWND,
    list: HWND,
}

#[derive(Clone, Copy)]
pub(super) struct ThemePalette {
    pub(super) background: COLORREF,
    pub(super) text: COLORREF,
    disabled_text: COLORREF,
    pub(super) control_surface: COLORREF,
    pub(super) pressed_surface: COLORREF,
    pub(super) control_border: COLORREF,
    pub(super) accent: COLORREF,
    pub(super) accent_text: COLORREF,
}

impl ThemePalette {
    pub(super) fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: dark::BACKGROUND,
                text: dark::TEXT,
                disabled_text: rgb(145, 145, 145),
                control_surface: dark::CONTROL_SURFACE,
                pressed_surface: dark::PRESSED_SURFACE,
                control_border: dark::CONTROL_BORDER,
                accent: system_color(COLOR_HIGHLIGHT),
                accent_text: system_color(COLOR_HIGHLIGHTTEXT),
            }
        } else {
            Self {
                background: system_color(COLOR_WINDOW),
                text: system_color(COLOR_WINDOWTEXT),
                disabled_text: system_color(COLOR_GRAYTEXT),
                control_surface: system_color(COLOR_BTNFACE),
                pressed_surface: system_color(COLOR_BTNSHADOW),
                control_border: system_color(COLOR_BTNSHADOW),
                accent: system_color(COLOR_HIGHLIGHT),
                accent_text: system_color(COLOR_HIGHLIGHTTEXT),
            }
        }
    }

    pub(super) const fn label_text(&self, enabled: bool) -> COLORREF {
        if enabled {
            self.text
        } else {
            self.disabled_text
        }
    }
}

pub(super) fn apply_native_theme_hooks(
    window: HWND,
    controls: &DialogControls,
    dark: bool,
    dark_mode_api: Option<&DarkModeApi>,
) {
    if let Some(api) = dark_mode_api {
        api.allow_for_window(window, dark);
    }
    if let Err(error) = dialog_host::set_dark_title_bar(window, dark) {
        eprintln!("Could not apply the native settings title-bar theme: {error}");
    }

    for control in theme_targets(controls) {
        apply_control_theme(control, dark, dark_mode_api);
        if control.kind == ThemeTargetKind::ComboBox {
            match combo_list_window(control.hwnd) {
                Ok(list) => apply_control_theme(
                    ThemeTarget {
                        hwnd: list,
                        kind: ThemeTargetKind::ComboList,
                    },
                    dark,
                    dark_mode_api,
                ),
                Err(error) => eprintln!("Could not theme the settings drop-down list: {error}"),
            }
        }
    }
}

fn theme_targets(controls: &DialogControls) -> impl Iterator<Item = ThemeTarget> + '_ {
    controls.iter().map(|(control, hwnd)| ThemeTarget {
        hwnd,
        kind: if let Control::Selector(_) = control {
            ThemeTargetKind::ComboBox
        } else {
            ThemeTargetKind::Standard
        },
    })
}

fn apply_control_theme(control: ThemeTarget, dark: bool, dark_mode_api: Option<&DarkModeApi>) {
    let sub_app_name = native_theme_class(dark, control.kind).name();
    let theme_result = unsafe {
        // SAFETY: control is a live child HWND and both theme-name buffers are static.
        SetWindowTheme(control.hwnd, sub_app_name, PCWSTR::null()).ok()
    };
    if let Err(error) = theme_result {
        eprintln!("Could not apply the native settings control theme: {error}");
    }
    if let Some(api) = dark_mode_api {
        api.allow_for_window(control.hwnd, dark);
    }
    unsafe {
        // SAFETY: control is live and WM_THEMECHANGED has no pointer payload.
        SendMessageW(
            control.hwnd,
            WM_THEMECHANGED,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
}

fn combo_list_window(combo: HWND) -> Result<HWND> {
    let mut info = NativeComboBoxInfo {
        size: u32::try_from(size_of::<NativeComboBoxInfo>()).unwrap_or(u32::MAX),
        item_rect: RECT::default(),
        button_rect: RECT::default(),
        button_state: 0,
        combo: HWND::default(),
        item: HWND::default(),
        list: HWND::default(),
    };
    let result = unsafe {
        // SAFETY: combo is a live COMBOBOX HWND and info is a correctly sized writable structure.
        GetComboBoxInfo(combo, &raw mut info)
    };
    if result == 0 || info.list == HWND::default() {
        Err(Error::from_thread())
    } else {
        Ok(info.list)
    }
}

pub(super) fn paint_background(hwnd: HWND, dc: HDC, brush: HBRUSH) -> LRESULT {
    let mut client = RECT::default();
    let client_result = unsafe {
        // SAFETY: hwnd and dc are the live handles supplied for WM_ERASEBKGND; client is writable.
        GetClientRect(hwnd, &raw mut client)
    };
    if let Err(error) = client_result {
        eprintln!("Could not read the settings client area for painting: {error}");
        return LRESULT(0);
    }
    let filled = unsafe {
        // SAFETY: dc is valid for this paint callback, client is initialized, and brush is owned by
        // the dialog state for the complete synchronous call.
        FillRect(dc, &raw const client, brush)
    };
    if filled == 0 {
        eprintln!("Could not paint the settings background");
        LRESULT(0)
    } else {
        LRESULT(1)
    }
}

pub(super) fn style_control_dc(
    dc: HDC,
    background_color: COLORREF,
    text_color: COLORREF,
    brush: HBRUSH,
) -> LRESULT {
    let previous_mode = unsafe {
        // SAFETY: dc is the live control paint context supplied by the current WM_CTLCOLOR message.
        SetBkMode(dc, TRANSPARENT)
    };
    if previous_mode == 0 {
        eprintln!("Could not make a settings control background transparent");
    }
    let previous_background = unsafe {
        // SAFETY: dc remains valid and brush color is represented by this COLORREF.
        SetBkColor(dc, background_color)
    };
    if previous_background.0 == u32::MAX {
        eprintln!("Could not set a settings control background color");
    }
    let previous_text = unsafe {
        // SAFETY: dc remains valid and text_color is a scalar COLORREF.
        SetTextColor(dc, text_color)
    };
    if previous_text.0 == u32::MAX {
        eprintln!("Could not set a settings control text color");
    }
    LRESULT(brush.0 as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_native_controls_use_their_supported_theme_classes() {
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::Standard),
            NativeThemeClass::Explorer
        );
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::ComboBox),
            NativeThemeClass::Cfd
        );
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::ComboList),
            NativeThemeClass::DarkModeExplorer
        );
    }

    #[test]
    fn dark_interactive_control_surfaces_are_not_light() {
        let palette = ThemePalette::new(true);

        for color in [
            palette.control_surface,
            palette.pressed_surface,
            palette.control_border,
        ] {
            let red = color.0 & 0xff;
            let green = color.0 >> 8 & 0xff;
            let blue = color.0 >> 16 & 0xff;
            assert!(red < 160 && green < 160 && blue < 160);
        }
    }
}
