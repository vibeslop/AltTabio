//! A nonactivating, per-pixel-alpha query display. It owns no switcher state or input focus.

mod paint;
mod surface;

use crate::app_messages::WM_CLEAR_FILTER;
use crate::win32::{
    module_instance, point_from_lparam, rect_from_native, set_window_user_data,
    work_area_near_window,
};
use alttabio::dialog_layout::Rect;
use alttabio::filter_feedback::popup_bounds;
use alttabio::overlay_layout::layout_dpi;
use alttabio::theme::ResolvedTheme;
use paint::Painter;
use std::cell::Cell;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetWindowLongPtrW, GetWindowRect, IDC_ARROW, LoadCursorW, MA_NOACTIVATE, PostMessageW,
    RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA, UpdateLayeredWindow,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{Error, Result, w};

const CLASS_NAME: windows::core::PCWSTR = w!("AltTabioFilterFeedback");

#[derive(PartialEq)]
struct Frame {
    query: String,
    bounds: Rect,
    dpi: u16,
    theme: ResolvedTheme,
}

struct Events {
    owner: HWND,
    clear_start: Cell<bool>,
    clear_left: Cell<i32>,
    post_failure: Cell<Option<Error>>,
    destroyed: Cell<bool>,
}

pub(crate) struct FilterPopup {
    hwnd: HWND,
    // The callback only posts bounded events, and reports failures outside the callback.
    events: Option<Box<Events>>,
    painter: Painter,
    frame: Option<Frame>,
}

impl FilterPopup {
    pub fn new(owner: HWND) -> Result<Self> {
        let instance = module_instance()?;
        let cursor = unsafe {
            // SAFETY: IDC_ARROW is a shared cursor; no ownership transfers.
            LoadCursorW(None, IDC_ARROW)
        }?;
        let class = WNDCLASSEXW {
            cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or_default(),
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: CLASS_NAME,
            ..WNDCLASSEXW::default()
        };
        if unsafe {
            // SAFETY: the initialized class and its static name outlive the synchronous call.
            RegisterClassExW(&raw const class)
        } == 0
        {
            let error = Error::from_thread();
            if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(error);
            }
        }
        let painter = Painter::new()?;
        let mut events = Box::new(Events {
            owner,
            clear_start: Cell::new(false),
            clear_left: Cell::new(0),
            post_failure: Cell::new(None),
            destroyed: Cell::new(false),
        });
        let hwnd = unsafe {
            // SAFETY: the Box allocation remains stable, moves into this owner after creation,
            // and is freed only after DestroyWindow has removed every callback reference.
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!("AltTabio filter"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(owner),
                None,
                Some(instance),
                Some((&raw mut *events).cast()),
            )
        }?;
        Ok(Self {
            hwnd,
            events: Some(events),
            painter,
            frame: None,
        })
    }

    pub fn update(
        &mut self,
        owner: HWND,
        query: &str,
        theme: ResolvedTheme,
        compact: bool,
    ) -> Result<()> {
        self.check_events()?;
        let mut panel = RECT::default();
        let window_dpi = unsafe {
            // SAFETY: owner is the live borrowed overlay and panel is writable for the call.
            GetWindowRect(owner, &raw mut panel)?;
            GetDpiForWindow(owner)
        };
        let Some(bounds) = popup_bounds(
            rect_from_native(panel),
            work_area_near_window(owner)?,
            window_dpi,
            compact,
        ) else {
            self.hide();
            return Ok(());
        };
        let frame = Frame {
            query: query.to_owned(),
            bounds,
            dpi: layout_dpi(window_dpi),
            theme,
        };
        if self.frame.as_ref() != Some(&frame) {
            let surface =
                self.painter
                    .draw(query, bounds.width, bounds.height, frame.dpi, theme)?;
            let position = POINT {
                x: bounds.x,
                y: bounds.y,
            };
            let source = POINT::default();
            let blend = BLENDFUNCTION {
                BlendOp: u8::try_from(AC_SRC_OVER).unwrap_or_default(),
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: u8::try_from(AC_SRC_ALPHA).unwrap_or(1),
            };
            unsafe {
                // SAFETY: the owned popup is layered; the source bitmap is premultiplied BGRA,
                // and every initialized position/size/blend pointer outlives this call. Constant
                // alpha stays 255, so it does not reduce the text's own fully opaque alpha.
                UpdateLayeredWindow(
                    self.hwnd,
                    None,
                    Some(&raw const position),
                    Some(&raw const surface.size),
                    Some(surface.dc),
                    Some(&raw const source),
                    COLORREF(0),
                    Some(&raw const blend),
                    ULW_ALPHA,
                )?;
            }
            if let Some(events) = &self.events {
                events
                    .clear_left
                    .set(bounds.width - i32::from(frame.dpi) * 32 / 96);
            }
            self.frame = Some(frame);
        }
        unsafe {
            // SAFETY: the owned popup has valid layered contents and this show cannot activate
            // it. Its owner keeps it above the switcher without making it independently topmost.
            let _previous_visibility = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    pub fn hide(&self) {
        if let Some(events) = &self.events {
            events.clear_start.set(false);
            if events.destroyed.get() {
                return;
            }
        }
        unsafe {
            // SAFETY: the popup remains live until this wrapper is dropped.
            let _previous_visibility = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn check_events(&self) -> Result<()> {
        if let Some(error) = self
            .events
            .as_ref()
            .and_then(|events| events.post_failure.take())
        {
            Err(error)
        } else {
            Ok(())
        }
    }
}

impl Drop for FilterPopup {
    fn drop(&mut self) {
        if self
            .events
            .as_ref()
            .is_some_and(|events| events.destroyed.get())
        {
            return;
        }
        let result = unsafe {
            // SAFETY: this wrapper uniquely owns the popup HWND and its callback state is live.
            DestroyWindow(self.hwnd)
        };
        if let Err(error) = result {
            // A live window could still call the procedure. Retain its small state allocation.
            if let Some(events) = self.events.take() {
                let _retained = Box::leak(events);
            }
            eprintln!("Could not destroy the filter popup: {error}");
        }
        if let Err(error) = self.check_events() {
            eprintln!("Could not detach the filter popup: {error}");
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        handle_window_message(hwnd, message, lparam)
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| unsafe {
        // SAFETY: unhandled messages retain the original parameters required by DefWindowProcW.
        DefWindowProcW(hwnd, message, wparam, lparam)
    })
}

fn handle_window_message(hwnd: HWND, message: u32, lparam: LPARAM) -> Option<LRESULT> {
    if message == WM_MOUSEACTIVATE {
        return Some(LRESULT(isize::try_from(MA_NOACTIVATE).unwrap_or_default()));
    }
    if message == WM_NCCREATE {
        let create = unsafe {
            // SAFETY: WM_NCCREATE supplies this structure for the callback duration.
            (lparam.0 as *const CREATESTRUCTW).as_ref()
        }?;
        let attached = unsafe {
            // SAFETY: CreateWindowExW passed a stable Events Box whose owner outlives the HWND.
            set_window_user_data(hwnd, create.lpCreateParams as isize)
        };
        return Some(LRESULT(isize::from(attached.is_ok())));
    }
    let events = unsafe {
        // SAFETY: user data is either zero or the stable Events Box owned by FilterPopup.
        (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Events).as_ref()
    }?;
    match message {
        WM_NCDESTROY => {
            events.destroyed.set(true);
            if let Err(error) = unsafe {
                // SAFETY: zero removes this procedure's reference to the Box before it is freed.
                set_window_user_data(hwnd, 0)
            } {
                events.post_failure.set(Some(error));
            }
            None
        }
        WM_LBUTTONDOWN => {
            events
                .clear_start
                .set(point_from_lparam(lparam).x >= events.clear_left.get());
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            if events.clear_start.replace(false)
                && point_from_lparam(lparam).x >= events.clear_left.get()
                && let Err(error) = unsafe {
                    // SAFETY: the owner outlives its popup and the bounded event has no pointers.
                    PostMessageW(Some(events.owner), WM_CLEAR_FILTER, WPARAM(0), LPARAM(0))
                }
            {
                events.post_failure.set(Some(error));
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::ComApartment;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, SendMessageW,
    };

    struct Owner(HWND);

    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: the test uniquely owns this hidden window on its creating thread.
            assert!(unsafe { DestroyWindow(self.0) }.is_ok());
        }
    }

    #[test]
    fn popup_keeps_owner_geometry_and_focus_and_posts_clear_as_an_event() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let owner = Owner(unsafe {
            // SAFETY: STATIC is a system class and the test owns this hidden popup.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Filter popup test"),
                WS_POPUP,
                300,
                250,
                900,
                600,
                None,
                None,
                Some(module_instance()?),
                None,
            )
        }?);
        let mut original = RECT::default();
        let foreground = unsafe {
            // SAFETY: the test owner is live and original is writable for this query.
            GetWindowRect(owner.0, &raw mut original)?;
            GetForegroundWindow()
        };
        let mut popup = FilterPopup::new(owner.0)?;
        popup.update(owner.0, "firefox", ResolvedTheme::Dark, true)?;
        let mut current = RECT::default();
        let mut bounds = RECT::default();
        unsafe {
            // SAFETY: both owned windows are live and the output rectangles remain writable.
            GetWindowRect(owner.0, &raw mut current)?;
            GetWindowRect(popup.hwnd, &raw mut bounds)?;
            assert!(IsWindowVisible(popup.hwnd).as_bool());
            assert_eq!(GetForegroundWindow(), foreground);
        }
        assert_eq!(current, original);
        assert!(bounds.top < original.top && bounds.bottom > original.top);
        let mut message = MSG::default();
        let click = |x: i32| {
            let point = LPARAM(
                isize::try_from((18_u32 << 16) | u32::try_from(x).unwrap_or_default())
                    .unwrap_or_default(),
            );
            unsafe {
                // SAFETY: these scalar mouse messages target only this test-owned popup.
                SendMessageW(popup.hwnd, WM_LBUTTONDOWN, Some(WPARAM(0)), Some(point));
                SendMessageW(popup.hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(point));
            }
        };
        click(10);
        unsafe {
            // SAFETY: message is writable, and the filter restricts the read to our owner's event.
            assert!(
                !PeekMessageW(
                    &raw mut message,
                    Some(owner.0),
                    WM_CLEAR_FILTER,
                    WM_CLEAR_FILTER,
                    PM_REMOVE
                )
                .as_bool()
            );
        }
        click(bounds.right - bounds.left - 18);
        unsafe {
            // SAFETY: the same writable output reads only the queued clear event for our owner.
            assert!(
                PeekMessageW(
                    &raw mut message,
                    Some(owner.0),
                    WM_CLEAR_FILTER,
                    WM_CLEAR_FILTER,
                    PM_REMOVE
                )
                .as_bool()
            );
        }
        popup.hide();
        // SAFETY: the popup is still owned and this only queries its visibility.
        assert!(!unsafe { IsWindowVisible(popup.hwnd) }.as_bool());
        drop(owner);
        assert!(
            popup
                .events
                .as_ref()
                .is_some_and(|events| events.destroyed.get())
        );
        Ok(())
    }
}
