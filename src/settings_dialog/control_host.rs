//! Input forwarding and lifetime of the Settings control subclasses.
use super::controls::DialogControls;
use super::paint::{paint_control_message, paint_settings_control};
use super::{DialogState, WM_SETTINGS_FOCUS, WM_SETTINGS_NAVIGATE};
use crate::dialog_host::DialogHost;
use alttabio::settings_form::{Category, Control};
use std::{
    ffi::c_void,
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{HDC, InvalidateRect};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETDROPPEDSTATE, PostMessageW, SendMessageW, WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLORLISTBOX,
    WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ENABLE, WM_GETDLGCODE, WM_KEYDOWN, WM_KILLFOCUS,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_VSCROLL,
};
use windows::core::{BOOL, Error, Result};
const SETTINGS_CONTROL_SUBCLASS_ID: usize = 1;
type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize, data: usize) -> BOOL;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    fn RemoveWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize) -> BOOL;
}

pub(super) fn install_custom_control_painting(
    controls: &DialogControls,
    host: &DialogHost<DialogState>,
) -> Result<()> {
    for (_, hwnd) in controls.iter() {
        // SAFETY: every child is owned by the dialog, whose stable host outlives its children.
        if !unsafe {
            SetWindowSubclass(
                hwnd,
                Some(settings_control_subclass_proc),
                SETTINGS_CONTROL_SUBCLASS_ID,
                std::ptr::from_ref(host) as usize,
            )
        }
        .as_bool()
        {
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
            // SAFETY: this removes only the currently executing subclass before destruction.
            if !unsafe {
                RemoveWindowSubclass(
                    hwnd,
                    Some(settings_control_subclass_proc),
                    SETTINGS_CONTROL_SUBCLASS_ID,
                )
            }
            .as_bool()
            {
                eprintln!("Could not remove settings control painting during destruction");
            }
            return None;
        }
        // SAFETY: installation stores the stable host pointer, live until all children are destroyed.
        let host = unsafe { (host_pointer as *const DialogHost<DialogState>).as_ref() }?;
        // Drop the state borrow before forwarding anything that can re-enter the parent.
        let (root, control) = {
            let state = host.state()?;
            (state.hwnd, state.controls.find(hwnd))
        };
        if matches!(
            message,
            WM_COMMAND | WM_DRAWITEM | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX
        ) {
            // SAFETY: the viewport forwards unchanged synchronous child notifications. Pointer
            // payloads, including DRAWITEMSTRUCT, stay live until SendMessageW returns.
            return Some(unsafe { SendMessageW(root, message, Some(wparam), Some(lparam)) });
        }
        if message == WM_VSCROLL || message == WM_MOUSEWHEEL {
            // Keep the native popup's wheel handling while it is open.
            let popup_open = matches!(control, Some(Control::Selector(_))) &&
                // SAFETY: this is a live COMBOBOX and the query has no pointer payload.
                unsafe { SendMessageW(hwnd, CB_GETDROPPEDSTATE, None, None) }.0 != 0;
            if !popup_open {
                post(root, message, wparam, lparam);
                return Some(LRESULT(0));
            }
        }
        if let Some(Control::Navigation(category)) = control {
            if message == WM_GETDLGCODE {
                // SAFETY: the original button processes the unmodified query.
                let original = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
                return Some(LRESULT(original.0 | 1)); // DLGC_WANTARROWS
            }
            if message == WM_KEYDOWN {
                let target =
                    navigation_target(category, u16::try_from(wparam.0).unwrap_or_default());
                if let Some(target) = target {
                    post(root, WM_SETTINGS_NAVIGATE, WPARAM(target), LPARAM(0));
                    return Some(LRESULT(0));
                }
            }
        }
        if message == WM_SETFOCUS {
            post(root, WM_SETTINGS_FOCUS, WPARAM(hwnd.0 as usize), LPARAM(0));
        }
        let state = host.state()?;
        match message {
            WM_PAINT => Some(paint_control_message(hwnd, &state)),
            WM_PRINTCLIENT => {
                paint_settings_control(hwnd, HDC(wparam.0 as *mut c_void), &state);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                if state.hovered.replace(hwnd) != hwnd {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or_default(),
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        ..Default::default()
                    };
                    // SAFETY: the sized request tracks only this live child HWND.
                    if let Err(error) = unsafe { TrackMouseEvent(&raw mut track) } {
                        eprintln!("Could not track the settings pointer: {error}");
                    }
                    invalidate(hwnd);
                }
                None
            }
            WM_MOUSELEAVE => {
                if state.hovered.get() == hwnd {
                    state.hovered.set(HWND::default());
                }
                invalidate(hwnd);
                None
            }
            WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
                // SAFETY: the original control procedure receives the unchanged message.
                let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
                invalidate(hwnd);
                Some(result)
            }
            _ => None,
        }
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| {
        // SAFETY: all unhandled messages retain their original payloads; no panic leaves the ABI.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    })
}

fn navigation_target(category: Category, key: u16) -> Option<usize> {
    let index = category as usize;
    let count = Category::ALL.len();
    if key == VK_UP.0 || key == VK_LEFT.0 {
        Some((index + count - 1) % count)
    } else if key == VK_DOWN.0 || key == VK_RIGHT.0 {
        Some((index + 1) % count)
    } else if key == VK_HOME.0 {
        Some(0)
    } else if key == VK_END.0 {
        Some(count - 1)
    } else {
        None
    }
}

fn post(root: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) {
    // SAFETY: the live dialog consumes these bounded events; none of the posted events has a
    // borrowed pointer payload. Focus carries only a child HWND, revalidated against the table.
    if let Err(error) = unsafe { PostMessageW(Some(root), message, wparam, lparam) } {
        eprintln!("Could not post a settings input event: {error}");
    }
}

fn invalidate(hwnd: HWND) {
    // SAFETY: hwnd is live throughout its subclass callback.
    if !unsafe { InvalidateRect(Some(hwnd), None, false) }.as_bool() {
        eprintln!("Could not redraw a settings control");
    }
}
