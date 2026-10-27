//! Input forwarding and lifetime of the Settings control subclasses.
use super::controls::DialogControls;
use super::paint::{paint_control_message, paint_settings_control};
use super::paint_state::{PaintCache, PaintCell, PaintState};
use super::{WM_SETTINGS_FOCUS, WM_SETTINGS_NAVIGATE};
use alttabio::settings_form::{Category, Control};
use std::rc::Rc;
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
#[cfg(test)]
#[path = "repaint_tests.rs"]
mod tests;
type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize, data: usize) -> BOOL;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    fn RemoveWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize) -> BOOL;
}

/// Called once for newly-created children; each subclass owns one cache reference.
pub(super) fn install_custom_control_painting(
    controls: &DialogControls,
    cache: &PaintCache,
) -> Result<()> {
    for (_, hwnd) in controls.iter() {
        let data = Rc::into_raw(Rc::clone(cache));
        // SAFETY: each installed subclass owns one strong reference to the paint cache.
        // WM_NCDESTROY releases it; failed installation releases it immediately.
        if !unsafe {
            SetWindowSubclass(
                hwnd,
                Some(settings_control_subclass_proc),
                SETTINGS_CONTROL_SUBCLASS_ID,
                data as usize,
            )
        }
        .as_bool()
        {
            // SAFETY: installation failed, so the raw reference was never transferred.
            unsafe {
                drop(Rc::from_raw(data));
            }
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
    cache_pointer: usize,
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
            // SAFETY: this subclass owns the strong reference transferred at installation.
            unsafe {
                drop(Rc::from_raw(cache_pointer as *const PaintCell));
            }
            return None;
        }
        // SAFETY: the installed subclass owns a cache reference until WM_NCDESTROY.
        let cache = unsafe { (cache_pointer as *const PaintCell).as_ref() }?;
        // Release the cache borrow before any native call. A nested command may replace
        // the snapshot, while this callback safely retains its own resources.
        let state = cache.try_borrow().ok()?.as_ref()?.clone();
        let (root, control) = (state.hwnd, state.controls.find(hwnd));
        // Only the viewport forwards child notifications. Combos must first process
        // messages from their own listboxes to commit and cancel selections correctly.
        if control == Some(Control::Viewport)
            && matches!(
                message,
                WM_DRAWITEM | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX
            )
        {
            return paint_notification(message, wparam, lparam, &state);
        }
        if control == Some(Control::Viewport) && message == WM_COMMAND {
            // SAFETY: forward this synchronous notification with unchanged scalar payloads.
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

fn paint_notification(
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    state: &PaintState,
) -> Option<LRESULT> {
    if message == WM_DRAWITEM {
        // SAFETY: this synchronous notification supplies a live DRAWITEMSTRUCT.
        let item =
            unsafe { (lparam.0 as *const windows::Win32::UI::Controls::DRAWITEMSTRUCT).as_ref() }?;
        return super::selectors::paint_list_item(item, state);
    }
    state.background.as_ref().map(|brush| {
        super::theme::style_control_dc(
            HDC(wparam.0 as *mut c_void),
            state.palette.background,
            state.palette.text,
            brush.0,
        )
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
