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
    CB_GETDROPPEDSTATE, IDC_HAND, LoadCursorW, PostMessageW, SendMessageW, SetCursor, WM_COMMAND,
    WM_CTLCOLORBTN, WM_CTLCOLORDLG, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ENABLE,
    WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN, WM_KILLFOCUS, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_NCPAINT, WM_PAINT, WM_PRINTCLIENT, WM_SETCURSOR, WM_SETFOCUS, WM_VSCROLL,
};
use windows::core::{BOOL, Error, Result};
const SETTINGS_CONTROL_SUBCLASS_ID: usize = 1;
#[path = "dropdown_input.rs"]
mod dropdown_input;
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

/// Called once for the dialog and its children; each subclass owns one cache reference.
pub(super) fn install_custom_control_painting(
    root: HWND,
    controls: &DialogControls,
    cache: &PaintCache,
) -> Result<()> {
    install_subclass(root, cache)?;
    for (control, hwnd) in controls.iter() {
        install_subclass(hwnd, cache)?;
        if matches!(control, Control::Selector(_)) {
            install_subclass(super::theme::combo_list_window(hwnd)?, cache)?;
        }
    }
    Ok(())
}

fn install_subclass(hwnd: HWND, cache: &PaintCache) -> Result<()> {
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
            // SAFETY: destruction consumes the cache reference owned by this subclass.
            unsafe {
                detach_paint_cache(hwnd, cache_pointer);
            }
            return None;
        }
        // SAFETY: the installed subclass owns a cache reference until WM_NCDESTROY.
        let cache = unsafe { (cache_pointer as *const PaintCell).as_ref() }?;
        // Release the cache borrow before any native call. A nested command may replace
        // the snapshot, while this callback safely retains its own resources.
        let state = cache.try_borrow().ok()?.as_ref()?.clone();
        let root = state.hwnd;
        if hwnd == root {
            return paint_dialog_message(message, wparam, lparam, &state);
        }
        // Keep the popup's native selection/capture behavior and owner-drawn items.
        // Only replace its nonclient edge with the same neutral edge as the field.
        let Some(control) = state.controls.find(hwnd) else {
            return popup_message(hwnd, message, wparam, lparam, &state);
        };
        if control == Control::AboutRepository && message == WM_SETCURSOR {
            return link_cursor();
        }
        // Only the viewport forwards child notifications. Combos must first process
        // messages from their own listboxes to commit and cancel selections correctly.
        if control == Control::Viewport
            && matches!(
                message,
                WM_DRAWITEM | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX
            )
        {
            return paint_notification(message, wparam, lparam, &state);
        }
        if control == Control::Viewport && message == WM_COMMAND {
            // SAFETY: forward this synchronous notification with unchanged scalar payloads.
            return Some(unsafe { SendMessageW(root, message, Some(wparam), Some(lparam)) });
        }
        if message == WM_VSCROLL || message == WM_MOUSEWHEEL {
            // Keep the native popup's wheel handling while it is open.
            let popup_open = matches!(control, Control::Selector(_)) &&
                // SAFETY: this is a live COMBOBOX and the query has no pointer payload.
                unsafe { SendMessageW(hwnd, CB_GETDROPPEDSTATE, None, None) }.0 != 0;
            if !popup_open {
                post(root, message, wparam, lparam);
                return Some(LRESULT(0));
            }
        }
        if let Control::Navigation(category) = control {
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
    .unwrap_or_default();
    handled.unwrap_or_else(|| {
        // SAFETY: all unhandled messages retain their original payloads; no panic leaves the ABI.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    })
}

fn popup_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    state: &PaintState,
) -> Option<LRESULT> {
    if message != WM_NCPAINT {
        return dropdown_input::handle_message(hwnd, message, wparam, lparam, state);
    }
    // SAFETY: the live native popup processes its unchanged nonclient paint first.
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if let Err(error) = super::paint::paint_popup_border(hwnd, state) {
        eprintln!("Could not paint the settings popup border: {error}");
    }
    Some(result)
}

fn link_cursor() -> Option<LRESULT> {
    // SAFETY: IDC_HAND is a shared system cursor; SetCursor borrows it without ownership.
    match unsafe { LoadCursorW(None, IDC_HAND) } {
        Ok(cursor) => {
            // SAFETY: the loaded system cursor remains live for the process lifetime.
            unsafe {
                let _previous = SetCursor(Some(cursor));
            }
            Some(LRESULT(1))
        }
        Err(error) => {
            eprintln!("Could not load the link cursor: {error}");
            None
        }
    }
}
unsafe fn detach_paint_cache(hwnd: HWND, cache_pointer: usize) {
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
    // SAFETY: the caller transfers the subclass's sole raw cache reference at WM_NCDESTROY.
    unsafe {
        drop(Rc::from_raw(cache_pointer as *const PaintCell));
    }
}

fn paint_dialog_message(
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    state: &PaintState,
) -> Option<LRESULT> {
    match message {
        WM_ERASEBKGND | WM_PRINTCLIENT => {
            // Scrolling and combo resizing can repaint the parent synchronously while
            // DialogHost holds mutable state. Use the same snapshot as child painting.
            if let Err(error) =
                super::paint::paint_window_background(HDC(wparam.0 as *mut c_void), state)
            {
                eprintln!("Could not paint the settings background: {error}");
            }
            Some(LRESULT(1))
        }
        WM_DRAWITEM | WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
            paint_notification(message, wparam, lparam, state)
        }
        _ => None,
    }
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
