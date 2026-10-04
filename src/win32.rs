//! Win32 helpers that hold no application state: the module handle, monitors, message words,
//! window user data, and wide strings.

use alttabio::dialog_layout::{Point, Rect};
use std::mem::size_of;
use windows::Win32::Foundation::{
    ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, POINT, RECT, SetLastError,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{GWLP_USERDATA, GetCursorPos, SetWindowLongPtrW};
use windows::core::{Error, Result};

pub(crate) fn module_instance() -> Result<HINSTANCE> {
    let module = unsafe {
        // SAFETY: None requests a borrowed handle for this executable module.
        GetModuleHandleW(None)
    }?;
    Ok(HINSTANCE(module.0))
}

pub(crate) fn monitor_info(monitor: HMONITOR) -> Result<MONITORINFO> {
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..MONITORINFO::default()
    };
    let read = unsafe {
        // SAFETY: info is a writable structure with its size field initialized.
        GetMonitorInfoW(monitor, &raw mut info)
    };
    if read.as_bool() {
        Ok(info)
    } else {
        Err(Error::from_thread())
    }
}

pub(crate) fn monitor_near_cursor() -> Result<HMONITOR> {
    let mut cursor = POINT::default();
    unsafe {
        // SAFETY: cursor is writable for the synchronous read.
        GetCursorPos(&raw mut cursor)?;
    }
    Ok(unsafe {
        // SAFETY: cursor is an initialized screen point and the fallback always yields a monitor.
        MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
    })
}

/// The work area of the monitor nearest `window`, including one that is hidden or off-screen.
pub(crate) fn work_area_near_window(window: HWND) -> Result<Rect> {
    let monitor = unsafe {
        // SAFETY: window is a live HWND and the nearest-monitor fallback always yields a monitor.
        MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
    };
    Ok(rect_from_native(monitor_info(monitor)?.rcWork))
}

pub(crate) fn work_area_near_cursor() -> Result<Rect> {
    Ok(rect_from_native(
        monitor_info(monitor_near_cursor()?)?.rcWork,
    ))
}

pub(crate) const fn native_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.x,
        top: rect.y,
        right: rect.right(),
        bottom: rect.bottom(),
    }
}

pub(crate) const fn rect_from_native(rect: RECT) -> Rect {
    Rect::from_edges(rect.left, rect.top, rect.right, rect.bottom)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Win32 packs two 16-bit words into WPARAM and LPARAM"
)]
pub(crate) const fn low_word(value: usize) -> u16 {
    value as u16
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Win32 packs two 16-bit words into WPARAM and LPARAM"
)]
pub(crate) const fn high_word(value: usize) -> u16 {
    (value >> 16) as u16
}

/// Client coordinates from a mouse message, which are signed so they can lie left of or above
/// the window.
pub(crate) fn point_from_lparam(lparam: LPARAM) -> Point {
    let raw = lparam.0.cast_unsigned();
    Point::new(
        i32::from(low_word(raw).cast_signed()),
        i32::from(high_word(raw).cast_signed()),
    )
}

/// Stores `value` as `window`'s user data. A window procedure reads it back as a pointer to its
/// state, so `value` must be zero or that state, kept alive until `WM_NCDESTROY` clears it.
pub(crate) fn set_window_user_data(window: HWND, value: isize) -> Result<()> {
    unsafe {
        // SAFETY: SetLastError only writes this thread's last-error value.
        SetLastError(ERROR_SUCCESS);
    }
    let previous = unsafe {
        // SAFETY: the call takes no pointers and stores value as an integer without reading
        // through it; a stale window handle comes back as an error. Dereferencing the value is
        // left to the window procedure that reads it back.
        SetWindowLongPtrW(window, GWLP_USERDATA, value)
    };
    // Zero is also what a successful call returns when the previous value was zero, so only a
    // last error separates failure from success.
    if previous == 0 {
        let error = Error::from_thread();
        if error.code().is_err() {
            return Err(error);
        }
    }
    Ok(())
}

pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_words_split_wparam_and_signed_lparam_coordinates() {
        assert_eq!(low_word(0x0003_0002), 2);
        assert_eq!(high_word(0x0003_0002), 3);
        assert_eq!(high_word(usize::MAX), u16::MAX);
        assert_eq!(point_from_lparam(LPARAM(0xfffe_fffd)), Point::new(-3, -2));
    }

    #[test]
    fn native_rects_round_trip_through_dialog_rects() {
        let native = RECT {
            left: -40,
            top: 10,
            right: 60,
            bottom: 90,
        };

        assert_eq!(rect_from_native(native), Rect::new(-40, 10, 100, 80));
        assert_eq!(native_rect(rect_from_native(native)), native);
    }
}
