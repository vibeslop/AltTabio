//! Owner-drawn painting of the Settings controls through comctl32 subclassing.

use super::DialogState;
use super::controls::{DialogControls, is_checked, is_control_enabled};
use crate::dialog_host::DialogHost;
use crate::native_drawing::{
    DRAW_TEXT_CENTER, DRAW_TEXT_END_ELLIPSIS, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE,
    DRAW_TEXT_VCENTER, draw_text_with_font, fill_color, frame_color, measure_text,
};
use crate::win32::rect_from_native;
use alttabio::dialog_layout::{Point, hairline, scale};
use alttabio::settings_form::{Control, checkmark_points};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{COLORREF, E_FAIL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DeleteObject, EndPaint, HDC, HGDIOBJ, HPEN, InvalidateRect, PAINTSTRUCT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETSTATE, GetClientRect, SendMessageW, WM_ENABLE, WM_KILLFOCUS, WM_NCDESTROY, WM_PAINT,
    WM_PRINTCLIENT, WM_SETFOCUS,
};
use windows::core::{BOOL, Error, Result};

const SETTINGS_CONTROL_SUBCLASS_ID: usize = 1;
const BUTTON_STATE_PUSHED: usize = 0x0004;
const SOLID_PEN: i32 = 0;

type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize, data: usize) -> BOOL;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    fn RemoveWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize) -> BOOL;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreatePen(style: i32, width: i32, color: COLORREF) -> HPEN;
    fn SelectObject(dc: HDC, object: HGDIOBJ) -> HGDIOBJ;
    fn MoveToEx(dc: HDC, x: i32, y: i32, previous: *mut c_void) -> BOOL;
    fn LineTo(dc: HDC, x: i32, y: i32) -> BOOL;
}

fn custom_paint_targets(controls: &DialogControls) -> impl Iterator<Item = HWND> + '_ {
    // Static labels already take the palette's colors through WM_CTLCOLORSTATIC.
    controls
        .iter()
        .filter(|(control, _)| !matches!(control, Control::Label(_)))
        .map(|(_, hwnd)| hwnd)
}

pub(super) fn install_custom_control_painting(
    controls: &DialogControls,
    host: &DialogHost<DialogState>,
) -> Result<()> {
    for control in custom_paint_targets(controls) {
        let installed = unsafe {
            // SAFETY: every handle is a live child control and host is the ModalDialog allocation,
            // which stays allocated until after all children receive WM_NCDESTROY.
            SetWindowSubclass(
                control,
                Some(settings_control_subclass_proc),
                SETTINGS_CONTROL_SUBCLASS_ID,
                std::ptr::from_ref(host) as usize,
            )
        };
        if !installed.as_bool() {
            return Err(Error::from_thread());
        }
    }
    Ok(())
}

unsafe extern "system" fn settings_control_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    host_pointer: usize,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if message == WM_NCDESTROY {
            let removed = unsafe {
                // SAFETY: this callback is the installed subclass instance being removed before
                // the child HWND finishes destruction.
                RemoveWindowSubclass(
                    hwnd,
                    Some(settings_control_subclass_proc),
                    SETTINGS_CONTROL_SUBCLASS_ID,
                )
            };
            if !removed.as_bool() {
                eprintln!("Could not remove settings control painting during destruction");
            }
            return None;
        }
        let host = unsafe {
            // SAFETY: SetWindowSubclass stored the stable DialogHost pointer for every control.
            (host_pointer as *const DialogHost<DialogState>).as_ref()
        }?;
        let state = host.state()?;
        match message {
            WM_PAINT => Some(paint_control_message(hwnd, &state)),
            WM_PRINTCLIENT => {
                paint_settings_control(hwnd, HDC(wparam.0 as *mut c_void), &state);
                Some(LRESULT(0))
            }
            WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
                let result = unsafe {
                    // SAFETY: the original control procedure receives the unchanged message.
                    DefSubclassProc(hwnd, message, wparam, lparam)
                };
                let invalidated = unsafe {
                    // SAFETY: hwnd is live for this callback and the complete control must redraw.
                    InvalidateRect(Some(hwnd), None, true)
                };
                if !invalidated.as_bool() {
                    eprintln!("Could not redraw a settings control after its state changed");
                }
                Some(result)
            }
            _ => None,
        }
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| unsafe {
        // SAFETY: all unhandled messages retain their original scalar payloads.
        DefSubclassProc(hwnd, message, wparam, lparam)
    })
}

fn paint_control_message(hwnd: HWND, state: &DialogState) -> LRESULT {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe {
        // SAFETY: hwnd is live during WM_PAINT and paint is a writable PAINTSTRUCT.
        BeginPaint(hwnd, &raw mut paint)
    };
    if dc == HDC::default() {
        eprintln!("Could not begin painting a settings control");
    } else {
        paint_settings_control(hwnd, dc, state);
    }
    let ended = unsafe {
        // SAFETY: paint was initialized by BeginPaint for this hwnd.
        EndPaint(hwnd, &raw const paint)
    };
    if !ended.as_bool() {
        eprintln!("Could not finish painting a settings control");
    }
    LRESULT(0)
}

fn paint_settings_control(hwnd: HWND, dc: HDC, state: &DialogState) {
    let mut client = RECT::default();
    let client_result = unsafe {
        // SAFETY: hwnd is a live child control and client is writable.
        GetClientRect(hwnd, &raw mut client)
    };
    if let Err(error) = client_result {
        eprintln!("Could not read settings control bounds for painting: {error}");
        return;
    }
    let result = match state.controls.find(hwnd) {
        Some(control @ Control::Group(_)) => paint_group_box(dc, client, control.text(), state),
        Some(control @ Control::Checkbox(_)) => paint_checkbox(
            dc,
            client,
            control.text(),
            is_checked(hwnd),
            is_control_enabled(hwnd),
            state,
        ),
        Some(Control::Selector(selector)) => {
            paint_combo_box(dc, client, state.selected_name(selector), state)
        }
        Some(control @ Control::Button(_)) => {
            paint_push_button(dc, client, control.text(), hwnd, state)
        }
        Some(Control::Label(_)) | None => Ok(()),
    };
    if let Err(error) = result {
        eprintln!("Could not paint a settings control: {error}");
    }
}

fn paint_group_box(dc: HDC, client: RECT, label: &str, state: &DialogState) -> Result<()> {
    fill_color(dc, client, state.palette.background)?;
    let border_top = client.top.saturating_add(scale(8, state.dpi));
    let border = RECT {
        top: border_top,
        ..client
    };
    frame_color(
        dc,
        border,
        state.palette.control_border,
        hairline(state.dpi),
    )?;

    let Some(fonts) = state.fonts.as_ref() else {
        return Err(Error::from_hresult(E_FAIL));
    };
    let text_size = measure_text(dc, label, fonts.heading.0)?;
    let horizontal_padding = scale(5, state.dpi);
    let label_left = client.left.saturating_add(scale(7, state.dpi));
    let label_background = RECT {
        left: label_left,
        top: client.top,
        right: label_left
            .saturating_add(text_size.cx)
            .saturating_add(horizontal_padding.saturating_mul(2)),
        bottom: client
            .top
            .saturating_add(text_size.cy.max(scale(18, state.dpi))),
    };
    fill_color(dc, label_background, state.palette.background)?;
    let mut text_rect = label_background;
    text_rect.left = text_rect.left.saturating_add(horizontal_padding);
    text_rect.right = text_rect.right.saturating_sub(horizontal_padding);
    draw_text_with_font(
        dc,
        label,
        text_rect,
        state.palette.text,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        fonts.heading.0,
    )
}

fn paint_checkbox(
    dc: HDC,
    client: RECT,
    label: &str,
    checked: bool,
    enabled: bool,
    state: &DialogState,
) -> Result<()> {
    fill_color(dc, client, state.palette.background)?;
    let size = scale(14, state.dpi).min(client.bottom.saturating_sub(client.top));
    let top = client.top.saturating_add(
        client
            .bottom
            .saturating_sub(client.top)
            .saturating_sub(size)
            / 2,
    );
    let checkbox = RECT {
        left: client.left,
        top,
        right: client.left.saturating_add(size),
        bottom: top.saturating_add(size),
    };
    let surface = if checked {
        state.palette.accent
    } else {
        state.palette.control_surface
    };
    fill_color(dc, checkbox, surface)?;
    frame_color(
        dc,
        checkbox,
        if checked {
            state.palette.accent
        } else {
            state.palette.control_border
        },
        hairline(state.dpi),
    )?;
    if checked {
        draw_checkmark(dc, checkbox, state.palette.accent_text, state.dpi)?;
    }
    let mut text_rect = client;
    text_rect.left = checkbox.right.saturating_add(scale(8, state.dpi));
    draw_text(
        dc,
        label,
        text_rect,
        state.palette.label_text(enabled),
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        state,
    )
}

fn paint_push_button(
    dc: HDC,
    client: RECT,
    label: &str,
    hwnd: HWND,
    state: &DialogState,
) -> Result<()> {
    let enabled = is_control_enabled(hwnd);
    let button_state = unsafe {
        // SAFETY: hwnd is a live BUTTON control and BM_GETSTATE has no pointer payload.
        SendMessageW(hwnd, BM_GETSTATE, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    let pressed = button_state.0.cast_unsigned() & BUTTON_STATE_PUSHED != 0;
    fill_color(
        dc,
        client,
        if pressed {
            state.palette.pressed_surface
        } else {
            state.palette.control_surface
        },
    )?;
    let focused = unsafe {
        // SAFETY: GetFocus has no preconditions and returns a borrowed HWND.
        GetFocus()
    } == hwnd;
    frame_color(
        dc,
        client,
        if focused {
            state.palette.accent
        } else {
            state.palette.control_border
        },
        scale(if focused { 2 } else { 1 }, state.dpi).max(1),
    )?;
    draw_text(
        dc,
        label,
        client,
        state.palette.label_text(enabled),
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state,
    )
}

fn paint_combo_box(dc: HDC, client: RECT, label: &str, state: &DialogState) -> Result<()> {
    fill_color(dc, client, state.palette.control_surface)?;
    let border = hairline(state.dpi);
    frame_color(dc, client, state.palette.control_border, border)?;
    let button_width = scale(28, state.dpi).min(client.right.saturating_sub(client.left));
    let button_left = client.right.saturating_sub(button_width);
    let separator = RECT {
        left: button_left,
        top: client.top.saturating_add(border),
        right: button_left.saturating_add(border),
        bottom: client.bottom.saturating_sub(border),
    };
    fill_color(dc, separator, state.palette.control_border)?;
    let padding = scale(8, state.dpi);
    let mut text_rect = client;
    text_rect.left = text_rect.left.saturating_add(padding);
    text_rect.right = button_left.saturating_sub(padding);
    draw_text(
        dc,
        label,
        text_rect,
        state.palette.text,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        state,
    )?;
    let center_x = button_left.saturating_add(button_width / 2);
    let center_y = client
        .top
        .saturating_add(client.bottom.saturating_sub(client.top) / 2);
    let half_width = scale(4, state.dpi);
    let half_height = scale(2, state.dpi);
    draw_polyline(
        dc,
        &[
            Point {
                x: center_x.saturating_sub(half_width),
                y: center_y.saturating_sub(half_height),
            },
            Point {
                x: center_x,
                y: center_y.saturating_add(half_height),
            },
            Point {
                x: center_x.saturating_add(half_width),
                y: center_y.saturating_sub(half_height),
            },
        ],
        state.palette.text,
        hairline(state.dpi),
    )
}

fn draw_text(
    dc: HDC,
    label: &str,
    rect: RECT,
    color: COLORREF,
    format: u32,
    state: &DialogState,
) -> Result<()> {
    let Some(fonts) = state.fonts.as_ref() else {
        return Err(Error::from_hresult(E_FAIL));
    };
    draw_text_with_font(dc, label, rect, color, format, fonts.body.0)
}

fn draw_checkmark(dc: HDC, rect: RECT, color: COLORREF, dpi: u32) -> Result<()> {
    let points = checkmark_points(rect_from_native(rect));
    draw_polyline(dc, &points, color, scale(2, dpi).max(2))
}

fn draw_polyline(dc: HDC, points: &[Point], color: COLORREF, width: i32) -> Result<()> {
    let Some((first, remaining)) = points.split_first() else {
        return Ok(());
    };
    let pen = unsafe {
        // SAFETY: scalar parameters describe a solid GDI pen.
        CreatePen(SOLID_PEN, width, color)
    };
    if pen == HPEN::default() {
        return Err(Error::from_thread());
    }
    let previous = unsafe {
        // SAFETY: dc is live and pen remains owned through the complete drawing operation.
        SelectObject(dc, HGDIOBJ(pen.0))
    };
    let mut success = previous != HGDIOBJ::default();
    if success {
        success = unsafe {
            // SAFETY: dc is live and the previous-point output is intentionally unused.
            MoveToEx(dc, first.x, first.y, std::ptr::null_mut()).as_bool()
        };
        for point in remaining {
            success &= unsafe {
                // SAFETY: dc remains live with the owned pen selected.
                LineTo(dc, point.x, point.y).as_bool()
            };
        }
        let restored = unsafe {
            // SAFETY: previous is the object returned by SelectObject for this dc.
            SelectObject(dc, previous)
        };
        if restored == HGDIOBJ::default() {
            eprintln!("Could not restore the settings drawing pen");
        }
    }
    let deleted = unsafe {
        // SAFETY: pen is uniquely owned and no longer selected into dc.
        DeleteObject(HGDIOBJ(pen.0))
    };
    if !deleted.as_bool() {
        eprintln!("Could not release a settings drawing pen");
    }
    if success {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::settings_form::Group;

    #[test]
    fn group_headers_are_custom_paint_targets_with_stable_labels() {
        let mut storage = [0_u8; 32];
        let controls = DialogControls::from_handles(
            Control::all()
                .zip(storage.iter_mut())
                .map(|(control, byte)| (control, HWND(std::ptr::from_mut(byte).cast())))
                .collect(),
        );
        let targets = custom_paint_targets(&controls).collect::<Vec<_>>();

        for (group, title) in [
            (Group::General, "General"),
            (Group::Appearance, "Appearance"),
            (Group::Monitor, "Monitor"),
        ] {
            let hwnd = controls.get(Control::Group(group));
            assert_eq!(controls.find(hwnd).map(Control::text), Some(title));
            assert!(targets.contains(&hwnd), "{group:?}");
        }
    }
}
