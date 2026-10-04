//! Win32 plumbing shared by the native dialogs: window classes, the modal loop, and placement.

use alttabio::dialog_layout::{MIN_DPI, Point, Rect, Size};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::mem::{size_of, size_of_val};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use windows::Win32::Foundation::{
    E_FAIL, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HBRUSH, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON,
    IDC_ARROW, IsDialogMessageW, LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_NCCREATE, WM_NCDESTROY,
    WNDCLASSEXW,
};
use windows::core::{Error, PCWSTR, Result};

// Closing is posted rather than done in place, so the handler that asks for it releases its state
// borrow before WM_DESTROY and WM_NCDESTROY arrive.
const WM_DESTROY_DIALOG: u32 = WM_APP + 20;

/// The dark surfaces both dialogs share, so Settings and About match side by side.
pub(crate) mod dark {
    use crate::native_drawing::rgb;
    use windows::Win32::Foundation::COLORREF;

    pub(crate) const BACKGROUND: COLORREF = rgb(32, 32, 32);
    pub(crate) const TEXT: COLORREF = rgb(240, 240, 240);
    pub(crate) const CONTROL_SURFACE: COLORREF = rgb(45, 45, 45);
    pub(crate) const PRESSED_SURFACE: COLORREF = rgb(66, 66, 66);
    pub(crate) const CONTROL_BORDER: COLORREF = rgb(125, 125, 125);
}

/// The fixed identity of one dialog's top-level window.
pub(crate) struct DialogFrame {
    /// Names the dialog in diagnostics.
    pub(crate) name: &'static str,
    pub(crate) class: &'static str,
    pub(crate) title: &'static str,
    pub(crate) style: WINDOW_STYLE,
    pub(crate) ex_style: WINDOW_EX_STYLE,
}

impl DialogFrame {
    /// The outer size of a window of this frame whose client area is `client` at `dpi`.
    pub(crate) fn window_size(&self, client: Size, dpi: u32) -> Result<Size> {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: client.width,
            bottom: client.height,
        };
        unsafe {
            // SAFETY: rect is writable and both styles are the exact styles used to create the window.
            AdjustWindowRectExForDpi(&raw mut rect, self.style, false, self.ex_style, dpi)?;
        }
        Ok(Size::new(
            rect.right.saturating_sub(rect.left),
            rect.bottom.saturating_sub(rect.top),
        ))
    }
}

/// A dialog's callback state as its window procedure sees it.
pub(crate) trait DialogWindow: Sized + 'static {
    const FRAME: DialogFrame;

    /// Records the window during `WM_NCCREATE`, before any other message reaches the state.
    fn attach(&mut self, window: HWND);

    /// Handles one message; `None` passes it to `DefWindowProcW`.
    fn handle_message(
        &mut self,
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT>;
}

/// The allocation a dialog window reaches through its user data.
pub(crate) struct DialogHost<S> {
    state: RefCell<S>,
    done: Cell<bool>,
}

impl<S> DialogHost<S> {
    /// Borrows the state unless a callback further up the stack already holds it.
    pub(crate) fn state(&self) -> Option<Ref<'_, S>> {
        self.state.try_borrow().ok()
    }

    pub(crate) fn state_mut(&self) -> Result<RefMut<'_, S>> {
        self.state
            .try_borrow_mut()
            .map_err(|_| Error::new(E_FAIL, "the dialog state is already in use"))
    }
}

/// Owns a dialog's boxed state for as long as its window can reach it, so the allocation is
/// released exactly once on every path.
pub(crate) struct ModalDialog<S: DialogWindow> {
    /// `None` once released, or leaked because a live window still points at it.
    host: Option<NonNull<DialogHost<S>>>,
    window: Option<HWND>,
}

/// Who handles the keyboard in a modal dialog's message loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Keyboard {
    /// `IsDialogMessageW` moves focus between child controls and maps Enter and Esc to `IDOK`
    /// and `IDCANCEL`.
    DialogNavigation,
    /// The window procedure receives every key itself.
    WindowProcedure,
}

enum LoopExit {
    Closed,
    Quit(i32),
}

/// A failed `ModalDialog::run`.
pub(crate) struct RunError {
    pub(crate) error: Error,
    /// The window failed to close after `WM_QUIT` ended the loop, as the application exits.
    pub(crate) exiting: bool,
}

impl From<Error> for RunError {
    fn from(error: Error) -> Self {
        Self {
            error,
            exiting: false,
        }
    }
}

impl From<RunError> for Error {
    fn from(failure: RunError) -> Self {
        failure.error
    }
}

impl<S: DialogWindow> ModalDialog<S> {
    /// Creates the hidden dialog window around `state`.
    pub(crate) fn create(
        instance: HINSTANCE,
        origin: Point,
        size: Size,
        parent: Option<HWND>,
        state: S,
    ) -> Result<Self> {
        let host = NonNull::from(Box::leak(Box::new(DialogHost {
            state: RefCell::new(state),
            done: Cell::new(false),
        })));
        // A failed creation drops this owner with no window, which frees the host.
        let mut dialog = Self {
            host: Some(host),
            window: None,
        };
        let class = wide(S::FRAME.class);
        let title = wide(S::FRAME.title);
        let window = unsafe {
            // SAFETY: both strings outlive the synchronous call, and the host stays allocated
            // until this owner releases it after WM_NCDESTROY has cleared the window user data.
            CreateWindowExW(
                S::FRAME.ex_style,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                S::FRAME.style,
                origin.x,
                origin.y,
                size.width,
                size.height,
                parent,
                None,
                Some(instance),
                Some(host.as_ptr().cast_const().cast()),
            )
        }?;
        dialog.window = Some(window);
        Ok(dialog)
    }

    pub(crate) fn window(&self) -> HWND {
        self.window.unwrap_or_default()
    }

    pub(crate) fn host(&self) -> Result<&DialogHost<S>> {
        let host = self
            .host
            .ok_or_else(|| Error::new(E_FAIL, "the dialog state was already released"))?;
        Ok(unsafe {
            // SAFETY: the allocation lives until `release`, which needs `&mut self` and so cannot
            // run while this borrow does. Every access goes through shared references to the
            // RefCell and Cell inside it.
            host.as_ref()
        })
    }

    pub(crate) fn show_in_front(&self) {
        let window = self.window();
        unsafe {
            // SAFETY: window is the live dialog owned by this UI thread. The return value reports
            // the previous visibility, not a failure.
            let _was_visible = ShowWindow(window, SW_SHOW);
        }
        bring_to_front(window, S::FRAME.name);
    }

    /// Runs the nested message loop until the dialog closes and returns its final state, or
    /// `None` when `WM_QUIT` ended the loop.
    pub(crate) fn run(mut self, keyboard: Keyboard) -> std::result::Result<Option<S>, RunError> {
        let exit = pump_messages(self.window(), &self.host()?.done, keyboard)?;
        match exit {
            LoopExit::Closed => Ok(Some(self.release()?)),
            LoopExit::Quit(exit_code) => {
                let released = self.release();
                unsafe {
                    // SAFETY: PostQuitMessage only queues WM_QUIT for this thread. The nested loop
                    // consumed it, so the application loop still needs it to exit.
                    PostQuitMessage(exit_code);
                }
                released.map(|_state| None).map_err(|error| RunError {
                    error,
                    exiting: true,
                })
            }
        }
    }

    /// Destroys the window and returns the state, failing where dropping the dialog only logs.
    #[cfg(test)]
    pub(crate) fn destroy(mut self) -> Result<S> {
        self.release()
    }

    /// Frees the host once its window is gone, destroying the window first if it still exists.
    fn release(&mut self) -> Result<S> {
        let host = self
            .host
            .take()
            .ok_or_else(|| Error::new(E_FAIL, "the dialog state was already released"))?;
        let done = unsafe {
            // SAFETY: the host has not been freed yet; see `host`.
            host.as_ref()
        }
        .done
        .get();
        if let Some(window) = self.window.filter(|_| !done) {
            unsafe {
                // SAFETY: window belongs to this UI thread and has not received WM_NCDESTROY.
                // Destruction synchronously clears its user data. On failure `host` stays leaked,
                // because the live window may still dispatch to it.
                DestroyWindow(window)
            }?;
        }
        let host = unsafe {
            // SAFETY: the window is gone, so this owner holds the only pointer to the allocation
            // and `take` above guarantees it is reclaimed once.
            Box::from_raw(host.as_ptr())
        };
        Ok(host.state.into_inner())
    }
}

impl<S: DialogWindow> Drop for ModalDialog<S> {
    fn drop(&mut self) {
        if self.host.is_some()
            && let Err(error) = self.release()
        {
            eprintln!(
                "Could not destroy the {} window, so its state stays allocated: {error}",
                S::FRAME.name
            );
        }
    }
}

fn pump_messages(window: HWND, done: &Cell<bool>, keyboard: Keyboard) -> Result<LoopExit> {
    let mut message = MSG::default();
    while !done.get() {
        let result = unsafe {
            // SAFETY: message is writable and this UI thread owns the nested dialog loop.
            GetMessageW(&raw mut message, None, 0, 0)
        };
        if result.0 == -1 {
            return Err(Error::from_thread());
        }
        if result.0 == 0 {
            return Ok(LoopExit::Quit(
                i32::try_from(message.wParam.0).unwrap_or_default(),
            ));
        }
        if keyboard == Keyboard::DialogNavigation
            && unsafe {
                // SAFETY: window and message are live on this UI thread for the synchronous call.
                IsDialogMessageW(window, &raw const message)
            }
            .as_bool()
        {
            continue;
        }
        unsafe {
            // SAFETY: GetMessageW initialized message for this UI thread. The results report
            // whether a character was produced and the window procedure's answer, not failures.
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    Ok(LoopExit::Closed)
}

/// Registers the window class of dialog `S`; a class from an earlier open is reused.
pub(crate) fn register_class<S: DialogWindow>(
    instance: HINSTANCE,
    icon: HICON,
    background: HBRUSH,
) -> Result<()> {
    let cursor = unsafe {
        // SAFETY: IDC_ARROW is a predefined shared cursor.
        LoadCursorW(None, IDC_ARROW)
    }?;
    let class_name = wide(S::FRAME.class);
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(dialog_window_proc::<S>),
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        hbrBackground: background,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        hIconSm: icon,
        ..WNDCLASSEXW::default()
    };
    let atom = unsafe {
        // SAFETY: class and its class-name buffer remain valid for the synchronous call.
        RegisterClassExW(&raw const class)
    };
    if atom != 0 {
        return Ok(());
    }
    let error = unsafe {
        // SAFETY: called immediately after the failed registration on the same thread.
        GetLastError()
    };
    if error == ERROR_CLASS_ALREADY_EXISTS {
        Ok(())
    } else {
        Err(Error::from(error))
    }
}

unsafe extern "system" fn dialog_window_proc<S: DialogWindow>(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if message == WM_NCCREATE {
            return attach_host::<S>(hwnd, lparam);
        }
        let host = unsafe {
            // SAFETY: user data is either zero or the DialogHost<S> pointer installed by
            // attach_host, which ModalDialog keeps allocated until WM_NCDESTROY clears it.
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const DialogHost<S>).as_ref()
        }?;
        match message {
            WM_DESTROY_DIALOG => {
                let result = unsafe {
                    // SAFETY: the posted message runs on the UI thread that owns hwnd.
                    DestroyWindow(hwnd)
                };
                if let Err(error) = result {
                    eprintln!("Could not close {}: {error}", S::FRAME.name);
                }
                Some(LRESULT(0))
            }
            WM_NCDESTROY => {
                host.done.set(true);
                if let Err(error) = set_user_data(hwnd, 0) {
                    eprintln!(
                        "Could not detach {} from its closing window: {error}",
                        S::FRAME.name
                    );
                }
                None
            }
            _ => host
                .state
                .try_borrow_mut()
                .ok()?
                .handle_message(hwnd, message, wparam, lparam),
        }
    }));
    let handled = match handled {
        Ok(handled) => handled,
        // A panic while attaching may leave the window without its host. DefWindowProcW would
        // still accept creation, and the modal loop would never learn that such a window closed.
        Err(_) if message == WM_NCCREATE => Some(LRESULT(0)),
        Err(_) => None,
    };
    handled.unwrap_or_else(|| unsafe {
        // SAFETY: unhandled messages are forwarded with their original scalar values.
        DefWindowProcW(hwnd, message, wparam, lparam)
    })
}

/// Connects a new window to the host passed to `CreateWindowExW`; failing refuses creation.
fn attach_host<S: DialogWindow>(hwnd: HWND, lparam: LPARAM) -> Option<LRESULT> {
    const REFUSE_CREATION: Option<LRESULT> = Some(LRESULT(0));
    let Some(create) = (unsafe {
        // SAFETY: WM_NCCREATE guarantees lParam points to the CREATESTRUCTW for this window.
        (lparam.0 as *const CREATESTRUCTW).as_ref()
    }) else {
        return REFUSE_CREATION;
    };
    let host = create.lpCreateParams.cast::<DialogHost<S>>().cast_const();
    let Some(host_ref) = (unsafe {
        // SAFETY: lpCreateParams is null or the host ModalDialog::create keeps allocated.
        host.as_ref()
    }) else {
        return REFUSE_CREATION;
    };
    let Ok(mut state) = host_ref.state.try_borrow_mut() else {
        return REFUSE_CREATION;
    };
    state.attach(hwnd);
    drop(state);
    if let Err(error) = set_user_data(hwnd, host as isize) {
        eprintln!("Could not attach {} to its window: {error}", S::FRAME.name);
        return REFUSE_CREATION;
    }
    None
}

fn set_user_data(window: HWND, value: isize) -> Result<()> {
    let previous = unsafe {
        // SAFETY: window is live on this thread. Clearing the last error first is the documented
        // way to tell a failure from a previous value of zero.
        SetLastError(WIN32_ERROR(0));
        SetWindowLongPtrW(window, GWLP_USERDATA, value)
    };
    if previous != 0 {
        return Ok(());
    }
    let error = Error::from_thread();
    if error.code().is_err() {
        Err(error)
    } else {
        Ok(())
    }
}

/// Asks the dialog's window procedure to destroy the window once the current message returns.
pub(crate) fn request_close(window: HWND) -> Result<()> {
    unsafe {
        // SAFETY: window is live and the private message carries no borrowed data.
        PostMessageW(Some(window), WM_DESTROY_DIALOG, WPARAM(0), LPARAM(0))
    }
}

/// Follows `WM_DPICHANGED`: moves the window to the suggested bounds, lets `rescale` adopt the new
/// DPI, and repaints the whole window.
pub(crate) fn handle_dpi_changed(
    window: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    name: &str,
    rescale: impl FnOnce(u32) -> Result<()>,
) {
    let suggested = unsafe {
        // SAFETY: WM_DPICHANGED guarantees lParam points to a suggested window RECT.
        (lparam.0 as *const RECT).as_ref()
    };
    if let Some(suggested) = suggested {
        let resize_result = unsafe {
            // SAFETY: window is live and the suggested rectangle comes from WM_DPICHANGED.
            SetWindowPos(
                window,
                None,
                suggested.left,
                suggested.top,
                suggested.right.saturating_sub(suggested.left),
                suggested.bottom.saturating_sub(suggested.top),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if let Err(error) = resize_result {
            eprintln!("Could not resize {name} for its new display scale: {error}");
        }
    }
    let dpi = u32::from(low_word(wparam.0)).max(MIN_DPI);
    if let Err(error) = rescale(dpi) {
        eprintln!("Could not lay out {name} for its new display scale: {error}");
    }
    let invalidated = unsafe {
        // SAFETY: window is live for the callback and the full dialog must redraw.
        InvalidateRect(Some(window), None, true)
    };
    if !invalidated.as_bool() {
        eprintln!("Could not redraw {name} after its display scale changed");
    }
}

pub(crate) fn set_dark_title_bar(window: HWND, dark: bool) -> Result<()> {
    let immersive_dark = i32::from(dark);
    unsafe {
        // SAFETY: window is a live top-level HWND and immersive_dark is BOOL-compatible and
        // remains valid for the synchronous DWM attribute call.
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const immersive_dark).cast(),
            u32::try_from(size_of_val(&immersive_dark)).unwrap_or(u32::MAX),
        )
    }
}

/// Requests the foreground for `window` and logs a refusal, which Windows issues whenever the user
/// is interacting with another process; the window still shows.
pub(crate) fn bring_to_front(window: HWND, name: &str) {
    let accepted = unsafe {
        // SAFETY: window is a live top-level window owned by this UI thread.
        SetForegroundWindow(window)
    };
    if !accepted.as_bool() {
        eprintln!("Windows kept {name} out of the foreground");
    }
}

pub(crate) fn focus(window: HWND) -> Result<()> {
    let result = unsafe {
        // SAFETY: window is a live control on this thread. Clearing the last error first keeps
        // an older failure out of the error reported below.
        SetLastError(WIN32_ERROR(0));
        SetFocus(Some(window))
    };
    // SetFocus returns the previous focus, which may be null, and the WM_KILLFOCUS and
    // WM_SETFOCUS handlers it runs may leave a last error behind, so neither tells whether the
    // focus moved. Where the focus ends up does.
    let focused = unsafe {
        // SAFETY: GetFocus only reads the calling thread's focus.
        GetFocus()
    };
    if focused == window {
        return Ok(());
    }
    Err(match result {
        Err(error) if error.code().is_err() => error,
        _ => Error::new(E_FAIL, "the focus stayed on another window"),
    })
}

/// Disables a dialog's owner while the dialog is open, which gives the dialog modality without
/// native ownership.
pub(crate) struct OwnerGuard(HWND);

impl OwnerGuard {
    pub(crate) fn disable(owner: HWND) -> Self {
        unsafe {
            // SAFETY: owner is the live application HWND and remains live through the dialog
            // loop. The result reports the previous state, not a failure.
            let _was_disabled = EnableWindow(owner, false);
        }
        Self(owner)
    }
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: owner remains live after the nested dialog closes. The result reports the
            // previous state, not a failure.
            let _was_disabled = EnableWindow(self.0, true);
        }
        bring_to_front(self.0, "the AltTabio window");
    }
}

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

/// The work area of the monitor nearest `window`, including one that is hidden or off-screen.
pub(crate) fn work_area_near_window(window: HWND) -> Result<Rect> {
    let monitor = unsafe {
        // SAFETY: window is a live HWND and the nearest-monitor fallback always yields a monitor.
        MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
    };
    Ok(rect_from_native(monitor_info(monitor)?.rcWork))
}

pub(crate) fn work_area_near_cursor() -> Result<Rect> {
    let mut cursor = POINT::default();
    unsafe {
        // SAFETY: cursor is writable for the synchronous read.
        GetCursorPos(&raw mut cursor)?;
    }
    let monitor = unsafe {
        // SAFETY: cursor is an initialized screen point and the fallback always yields a monitor.
        MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
    };
    Ok(rect_from_native(monitor_info(monitor)?.rcWork))
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
