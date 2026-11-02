//! Preserve captured popup presses on other Settings controls.
use super::super::paint_state::PaintState;
use super::DefSubclassProc;
use alttabio::settings_form::Control;
use windows::Win32::Foundation::{E_INVALIDARG, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETDROPPEDSTATE, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, ChildWindowFromPointEx, GetWindowRect,
    SendMessageW, WM_LBUTTONDOWN,
};
use windows::core::{Error, Result};

pub(super) fn handle_message(
    list: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    state: &PaintState,
) -> Option<LRESULT> {
    if message == WM_LBUTTONDOWN {
        match outside_press_target(list, lparam, state) {
            Ok(Some((target, point))) => {
                // SAFETY: let the live native list cancel and release capture first.
                // Its cancellation notifications cannot destroy the target control.
                // No state/cache borrow spans either synchronous native dispatch.
                unsafe {
                    let result = DefSubclassProc(list, message, wparam, lparam);
                    SendMessageW(target, message, Some(wparam), Some(point));
                    return Some(result);
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("Could not forward a settings dropdown click: {error}"),
        }
    }
    None
}

/// Hit-test only the dialog and its viewport. Never replay a click into another app,
/// a disabled/hidden control, or the combo whose popup is being dismissed.
fn outside_press_target(
    list: HWND,
    lparam: LPARAM,
    state: &PaintState,
) -> Result<Option<(HWND, LPARAM)>> {
    let point = crate::win32::point_from_lparam(lparam);
    let mut screen = POINT {
        x: point.x,
        y: point.y,
    };
    let mut rect = RECT::default();
    // SAFETY: list is the live popup executing this subclass. Include its border
    // and scrollbar in the hit test so they never click through to the dialog.
    unsafe {
        ClientToScreen(list, &raw mut screen).ok()?;
        GetWindowRect(list, &raw mut rect)?;
    }
    if screen.x >= rect.left
        && screen.x < rect.right
        && screen.y >= rect.top
        && screen.y < rect.bottom
    {
        return Ok(None);
    }
    let mut target = state.hwnd;
    for parent in [state.hwnd, state.controls.get(Control::Viewport)] {
        if target != parent {
            break;
        }
        let mut local = screen;
        // SAFETY: both parents belong to the live dialog; local is writable.
        unsafe {
            ScreenToClient(parent, &raw mut local).ok()?;
            target = ChildWindowFromPointEx(parent, local, CWP_SKIPDISABLED | CWP_SKIPINVISIBLE);
        }
    }
    match state.controls.find(target) {
        Some(Control::Selector(_)) => {
            // SAFETY: the table identifies a live COMBOBOX; this is a scalar query.
            if unsafe { SendMessageW(target, CB_GETDROPPEDSTATE, None, None) }.0 != 0 {
                return Ok(None);
            }
        }
        Some(Control::Checkbox(_) | Control::Navigation(_) | Control::Button(_)) => {}
        _ => return Ok(None),
    }
    // SAFETY: target is an enabled child found by the bounded hit test above.
    unsafe { ScreenToClient(target, &raw mut screen).ok()? };
    let x = i16::try_from(screen.x).map_err(|_| Error::from_hresult(E_INVALIDARG))?;
    let y = i16::try_from(screen.y).map_err(|_| Error::from_hresult(E_INVALIDARG))?;
    let packed = u32::from(x.cast_unsigned()) | (u32::from(y.cast_unsigned()) << 16);
    Ok(Some((
        target,
        LPARAM(isize::try_from(packed.cast_signed()).unwrap_or_default()),
    )))
}
