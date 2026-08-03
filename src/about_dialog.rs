use crate::{app_icon, native_theme::DarkModeApi};
use alttabio::settings::IconColor;
use alttabio::theme::ResolvedTheme;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BACKGROUND_MODE, BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE,
    COLOR_BTNSHADOW, COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontW,
    CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD,
    FillRect, GetMonitorInfoW, GetSysColor, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DI_NORMAL, DefWindowProcW,
    DestroyWindow, DispatchMessageW, DrawIconEx, GWLP_USERDATA, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, SW_SHOW, SW_SHOWNORMAL, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CAPTURECHANGED,
    WM_CLOSE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SIZE, WNDCLASSEXW, WS_CAPTION, WS_EX_DLGMODALFRAME,
    WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{Error, PCWSTR, Result, w};

pub(crate) const WINDOW_CLASS_NAME: &str = "AltTabioRustAbout";
const WINDOW_CLASS: PCWSTR = w!("AltTabioRustAbout");
const WINDOW_TITLE: PCWSTR = w!("About AltTabio");
const REPOSITORY_URL: &str = "https://github.com/vibeslop/AltTabio";
const REPOSITORY_LABEL: &str = "github.com/vibeslop/AltTabio";
const VERSION_LABEL: &str = concat!("Version ", env!("CARGO_PKG_VERSION"));
const CLIENT_WIDTH: i32 = 430;
const CLIENT_HEIGHT: i32 = 344;
const CONTENT_HORIZONTAL_MARGIN: i32 = 94;
const BASE_DPI: u32 = 96;
const DRAW_TEXT_CENTER: u32 = 0x0001;
const DRAW_TEXT_VCENTER: u32 = 0x0004;
const DRAW_TEXT_SINGLE_LINE: u32 = 0x0020;
const DRAW_TEXT_NO_PREFIX: u32 = 0x0800;
const WINDOW_STYLE_VALUE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0);
const WINDOW_EX_STYLE_VALUE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(WS_EX_DLGMODALFRAME.0);
const WM_DESTROY_DIALOG: u32 = WM_APP + 20;

#[link(name = "user32")]
unsafe extern "system" {
    fn DrawTextW(dc: HDC, text: *const u16, count: i32, rect: *mut RECT, format: u32) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    const fn as_native(self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.x.saturating_add(self.width),
            bottom: self.y.saturating_add(self.height),
        }
    }

    const fn contains(self, point: POINT) -> bool {
        point.x >= self.x
            && point.x < self.x.saturating_add(self.width)
            && point.y >= self.y
            && point.y < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AboutLayout {
    icon: Rect,
    title: Rect,
    version: Rect,
    description: Rect,
    repository: Rect,
    copyright: Rect,
    license: Rect,
    footer: Rect,
    close_button: Rect,
}

impl AboutLayout {
    fn new(client_width: i32, client_height: i32, dpi: u32) -> Self {
        let footer_height = scale(76, dpi).min(client_height);
        let footer_top = client_height.saturating_sub(footer_height);
        let button_width = scale(116, dpi).min(client_width);
        let button_height = scale(42, dpi).min(footer_height);
        let right_padding = scale(20, dpi);
        let button_y = footer_top.saturating_add(
            footer_height
                .saturating_sub(button_height)
                .saturating_div(2),
        );
        let content_left = scale(CONTENT_HORIZONTAL_MARGIN, dpi);
        let content_width = client_width.saturating_sub(content_left.saturating_mul(2));
        Self {
            icon: Rect::new(
                scale(20, dpi),
                scale(18, dpi),
                scale(48, dpi),
                scale(48, dpi),
            ),
            title: Rect::new(content_left, scale(20, dpi), content_width, scale(38, dpi)),
            version: Rect::new(content_left, scale(68, dpi), content_width, scale(28, dpi)),
            description: Rect::new(content_left, scale(104, dpi), content_width, scale(28, dpi)),
            repository: Rect::new(content_left, scale(140, dpi), content_width, scale(30, dpi)),
            copyright: Rect::new(content_left, scale(184, dpi), content_width, scale(28, dpi)),
            license: Rect::new(content_left, scale(214, dpi), content_width, scale(28, dpi)),
            footer: Rect::new(0, footer_top, client_width, footer_height),
            close_button: Rect::new(
                client_width
                    .saturating_sub(right_padding)
                    .saturating_sub(button_width),
                button_y,
                button_width,
                button_height,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AboutPalette {
    background: COLORREF,
    footer: COLORREF,
    text: COLORREF,
    title: COLORREF,
    link: COLORREF,
    separator: COLORREF,
    button: COLORREF,
    button_pressed: COLORREF,
    button_border: COLORREF,
}

impl AboutPalette {
    fn new(theme: ResolvedTheme) -> Self {
        match theme {
            ResolvedTheme::Dark => Self {
                background: rgb(32, 32, 32),
                footer: rgb(38, 38, 38),
                text: rgb(240, 240, 240),
                title: rgb(240, 240, 240),
                link: rgb(76, 194, 255),
                separator: rgb(68, 68, 68),
                button: rgb(45, 45, 45),
                button_pressed: rgb(66, 66, 66),
                button_border: rgb(125, 125, 125),
            },
            ResolvedTheme::Light => Self {
                background: system_color(COLOR_WINDOW),
                footer: system_color(COLOR_BTNFACE),
                text: system_color(COLOR_WINDOWTEXT),
                title: rgb(0, 76, 153),
                link: rgb(0, 102, 204),
                separator: system_color(COLOR_BTNSHADOW),
                button: system_color(COLOR_BTNFACE),
                button_pressed: system_color(COLOR_BTNSHADOW),
                button_border: system_color(COLOR_HIGHLIGHT),
            },
        }
    }
}

struct DialogState {
    hwnd: HWND,
    palette: AboutPalette,
    fonts: DialogFonts,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    dark_mode_api: Option<DarkModeApi>,
    dpi: u32,
    close_pressed: bool,
    open_repository_requested: bool,
}

struct DialogHost {
    state: RefCell<DialogState>,
    done: Cell<bool>,
}

impl DialogHost {
    fn new(state: DialogState) -> Self {
        Self {
            state: RefCell::new(state),
            done: Cell::new(false),
        }
    }
}

enum DialogLoopExit {
    Closed,
    Quit(i32),
}

impl DialogState {
    fn new(
        theme: ResolvedTheme,
        dpi: u32,
        icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    ) -> Result<Self> {
        let dark_mode_api = match DarkModeApi::load(theme == ResolvedTheme::Dark) {
            Ok(api) => Some(api),
            Err(error) => {
                eprintln!("Native About title-bar themes are unavailable: {error}");
                None
            }
        };
        Ok(Self {
            hwnd: HWND::default(),
            palette: AboutPalette::new(theme),
            fonts: DialogFonts::new(dpi)?,
            icon,
            dark_mode_api,
            dpi,
            close_pressed: false,
            open_repository_requested: false,
        })
    }

    fn update_dpi(&mut self, dpi: u32) -> Result<()> {
        self.fonts = DialogFonts::new(dpi)?;
        self.dpi = dpi;
        Ok(())
    }

    fn layout(&self) -> AboutLayout {
        let mut client = RECT::default();
        let result = unsafe {
            // SAFETY: hwnd is live while the dialog state is reachable and client is writable.
            GetClientRect(self.hwnd, &raw mut client)
        };
        if result.is_err() {
            return AboutLayout::new(
                scale(CLIENT_WIDTH, self.dpi),
                scale(CLIENT_HEIGHT, self.dpi),
                self.dpi,
            );
        }
        AboutLayout::new(
            client.right.saturating_sub(client.left),
            client.bottom.saturating_sub(client.top),
            self.dpi,
        )
    }
}

pub fn show(theme: ResolvedTheme, icon_color: IconColor) -> std::result::Result<(), String> {
    let instance = module_instance().map_err(|error| error.to_string())?;
    let icon = app_icon::load_app(instance, icon_color).map_err(|error| error.to_string())?;
    register_class(instance, icon).map_err(|error| error.to_string())?;
    let dpi = unsafe {
        // SAFETY: GetDpiForSystem has no pointer or lifetime preconditions.
        GetDpiForSystem()
    }
    .max(BASE_DPI / 2);
    let client_width = scale(CLIENT_WIDTH, dpi);
    let client_height = scale(CLIENT_HEIGHT, dpi);
    let window_rect = adjusted_window_rect(client_width, client_height, dpi)
        .map_err(|error| error.to_string())?;
    let origin = centered_window_origin(window_rect.right, window_rect.bottom)
        .map_err(|error| error.to_string())?;
    let host = Box::new(DialogHost::new(
        DialogState::new(theme, dpi, icon)
            .map_err(|error| format!("Could not prepare About: {error}"))?,
    ));
    let host_pointer = Box::into_raw(host);
    let window = unsafe {
        // SAFETY: host_pointer remains allocated through the nested message loop. WM_NCCREATE
        // stores the pointer as window user data without taking ownership.
        CreateWindowExW(
            WINDOW_EX_STYLE_VALUE,
            WINDOW_CLASS,
            WINDOW_TITLE,
            WINDOW_STYLE_VALUE,
            origin.x,
            origin.y,
            window_rect.right,
            window_rect.bottom,
            None,
            None,
            Some(instance),
            Some(host_pointer.cast()),
        )
    };
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            unsafe {
                // SAFETY: window creation failed before any HWND retained the unique allocation.
                drop(Box::from_raw(host_pointer));
            }
            return Err(format!("Could not create the About dialog: {error}"));
        }
    };
    if let Err(error) = app_icon::apply_to_window(window, instance, icon_color) {
        let destroy_result = unsafe {
            // SAFETY: window is live and owned by this UI thread.
            DestroyWindow(window)
        };
        if destroy_result.is_ok() {
            unsafe {
                // SAFETY: successful synchronous destruction cleared the window user data.
                drop(Box::from_raw(host_pointer));
            }
        }
        return Err(format!("Could not apply the selected About icon: {error}"));
    }
    if let Err(error) = apply_initial_window_dpi(window, host_pointer) {
        let destroy_result = unsafe {
            // SAFETY: window is live and owned by this UI thread.
            DestroyWindow(window)
        };
        if destroy_result.is_ok() {
            unsafe {
                // SAFETY: successful synchronous destruction cleared the window user data.
                drop(Box::from_raw(host_pointer));
            }
        }
        return Err(format!("Could not size About for this display: {error}"));
    }
    let host = unsafe {
        // SAFETY: host_pointer remains live until finish_dialog reclaims the allocation.
        &*host_pointer
    };
    if let Ok(state) = host.state.try_borrow() {
        apply_window_theme(window, theme, state.dark_mode_api.as_ref());
    } else {
        apply_window_theme(window, theme, None);
    }
    unsafe {
        // SAFETY: the completed About window is owned by this UI thread and was explicitly opened.
        let _was_visible = ShowWindow(window, SW_SHOW);
        let _foreground = SetForegroundWindow(window);
    }

    let Some(state) = finish_dialog(window, host_pointer)? else {
        return Ok(());
    };
    if state.open_repository_requested {
        open_repository()?;
    }
    Ok(())
}

fn finish_dialog(
    window: HWND,
    host_pointer: *mut DialogHost,
) -> std::result::Result<Option<Box<DialogState>>, String> {
    let loop_exit = match run_dialog_loop(host_pointer) {
        Ok(exit) => exit,
        Err(error) => {
            let destroy_result = unsafe {
                // SAFETY: the nested loop failed while this UI thread still owns the About HWND.
                DestroyWindow(window)
            };
            if let Err(destroy_error) = destroy_result {
                return Err(format!(
                    "About message loop failed ({error}) and its window could not close: {destroy_error}"
                ));
            }
            unsafe {
                // SAFETY: successful synchronous destruction cleared the window user data.
                drop(Box::from_raw(host_pointer));
            }
            return Err(error.to_string());
        }
    };
    if let DialogLoopExit::Quit(exit_code) = loop_exit {
        let destroy_result = unsafe {
            // SAFETY: WM_QUIT ended the nested loop while this UI thread still owns the HWND.
            DestroyWindow(window)
        };
        unsafe {
            // SAFETY: the nested loop consumed WM_QUIT, so the outer app loop still needs it.
            PostQuitMessage(exit_code);
        }
        destroy_result.map_err(|error| {
            format!("Could not close About while AltTabio was exiting: {error}")
        })?;
        unsafe {
            // SAFETY: successful synchronous destruction cleared the window user data.
            drop(Box::from_raw(host_pointer));
        }
        return Ok(None);
    }
    let host = unsafe {
        // SAFETY: a successful loop exit occurs only after WM_NCDESTROY cleared window user data.
        Box::from_raw(host_pointer)
    };
    Ok(Some(Box::new(host.state.into_inner())))
}

fn apply_initial_window_dpi(hwnd: HWND, host: *mut DialogHost) -> Result<()> {
    let dpi = unsafe {
        // SAFETY: hwnd is the live About window whose monitor determines its effective DPI.
        GetDpiForWindow(hwnd)
    }
    .max(BASE_DPI / 2);
    unsafe {
        // SAFETY: setup owns the host allocation and no callback borrow is active here. The
        // mutable reference ends before SetWindowPos can synchronously dispatch messages.
        (&mut *host).state.get_mut().update_dpi(dpi)?;
    }
    let rect = adjusted_window_rect(scale(CLIENT_WIDTH, dpi), scale(CLIENT_HEIGHT, dpi), dpi)?;
    let origin = centered_window_origin(rect.right, rect.bottom)?;
    unsafe {
        // SAFETY: hwnd is live and the computed bounds are within its target monitor work area.
        SetWindowPos(
            hwnd,
            None,
            origin.x,
            origin.y,
            rect.right,
            rect.bottom,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

fn run_dialog_loop(host: *mut DialogHost) -> Result<DialogLoopExit> {
    loop {
        let done = unsafe {
            // SAFETY: show retains the allocation until this nested loop returns. Cell supports
            // reentrant reads on the single UI thread without creating a mutable alias.
            (*host).done.get()
        };
        if done {
            return Ok(DialogLoopExit::Closed);
        }
        let mut message = MSG::default();
        let result = unsafe {
            // SAFETY: message is writable and this UI-thread loop owns dispatch for the dialog.
            GetMessageW(&raw mut message, None, 0, 0)
        };
        if result.0 == -1 {
            return Err(Error::from_thread());
        }
        if result.0 == 0 {
            return Ok(DialogLoopExit::Quit(
                i32::try_from(message.wParam.0).unwrap_or_default(),
            ));
        }
        unsafe {
            // SAFETY: message was initialized by GetMessageW and is dispatched unchanged.
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

unsafe extern "system" fn about_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if message == WM_NCCREATE {
            let create = unsafe {
                // SAFETY: WM_NCCREATE supplies a live CREATESTRUCTW pointer in lparam.
                &*(lparam.0 as *const CREATESTRUCTW)
            };
            let host = create.lpCreateParams.cast::<DialogHost>();
            if host.is_null() {
                return Some(LRESULT(0));
            }
            let host_ref = unsafe {
                // SAFETY: host is the Box allocation passed to CreateWindowExW and remains live.
                &*host
            };
            let Ok(mut state) = host_ref.state.try_borrow_mut() else {
                return Some(LRESULT(0));
            };
            state.hwnd = hwnd;
            drop(state);
            unsafe {
                // SAFETY: host remains live until the nested message loop has ended.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, host as isize);
            }
        }
        let host = unsafe {
            // SAFETY: user data is either zero before WM_NCCREATE or the live DialogHost pointer.
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogHost).as_ref()
        }?;
        if message == WM_DESTROY_DIALOG {
            let result = unsafe {
                // SAFETY: the posted message is handled by the UI thread that owns hwnd.
                DestroyWindow(hwnd)
            };
            if let Err(error) = result {
                eprintln!("Could not close About: {error}");
            }
            return Some(LRESULT(0));
        }
        if message == WM_NCDESTROY {
            host.done.set(true);
            unsafe {
                // SAFETY: clearing user data prevents later messages from observing host.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            return None;
        }
        let Ok(mut state) = host.state.try_borrow_mut() else {
            return None;
        };
        handle_about_message(&mut state, hwnd, message, wparam, lparam)
    }))
    .ok()
    .flatten();
    result.unwrap_or_else(|| unsafe {
        // SAFETY: unhandled messages retain their original scalar payloads.
        DefWindowProcW(hwnd, message, wparam, lparam)
    })
}

fn handle_about_message(
    state: &mut DialogState,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_PAINT => {
            paint_about(state);
            Some(LRESULT(0))
        }
        WM_PRINTCLIENT => {
            if let Err(error) = paint_about_content(HDC(wparam.0 as *mut c_void), state) {
                eprintln!("Could not print About: {error}");
            }
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SIZE => {
            invalidate_dialog(hwnd);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let point = point_from_lparam(lparam);
            if state.layout().close_button.contains(point) {
                state.close_pressed = true;
                unsafe {
                    // SAFETY: hwnd is the live dialog receiving the button press.
                    SetCapture(hwnd);
                }
                invalidate_dialog(hwnd);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            handle_left_button_up(state, hwnd, point_from_lparam(lparam));
            Some(LRESULT(0))
        }
        WM_CAPTURECHANGED => {
            if state.close_pressed {
                state.close_pressed = false;
                invalidate_dialog(hwnd);
            }
            Some(LRESULT(0))
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize || wparam.0 == VK_RETURN.0 as usize => {
            close_dialog(hwnd);
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            handle_dpi_changed(state, hwnd, wparam, lparam);
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            close_dialog(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn handle_left_button_up(state: &mut DialogState, hwnd: HWND, point: POINT) {
    if state.close_pressed {
        state.close_pressed = false;
        unsafe {
            // SAFETY: this UI thread owns any capture taken on button down.
            let _released = ReleaseCapture();
        }
        if state.layout().close_button.contains(point) {
            close_dialog(hwnd);
        } else {
            invalidate_dialog(hwnd);
        }
    } else if state.layout().repository.contains(point) {
        state.open_repository_requested = true;
        close_dialog(hwnd);
    }
}

fn handle_dpi_changed(state: &mut DialogState, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let new_dpi = u32::from(low_word(wparam.0)).max(BASE_DPI / 2);
    if let Err(error) = state.update_dpi(new_dpi) {
        eprintln!("Could not update About for its new display scale: {error}");
    }
    let suggested = unsafe {
        // SAFETY: WM_DPICHANGED supplies a live suggested RECT pointer in lparam.
        &*(lparam.0 as *const RECT)
    };
    let result = unsafe {
        // SAFETY: suggested contains the complete new window rectangle for this HWND.
        SetWindowPos(
            hwnd,
            None,
            suggested.left,
            suggested.top,
            suggested.right.saturating_sub(suggested.left),
            suggested.bottom.saturating_sub(suggested.top),
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        )
    };
    if let Err(error) = result {
        eprintln!("Could not resize About for its new display scale: {error}");
    }
    invalidate_dialog(hwnd);
}

fn paint_about(state: &DialogState) {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe {
        // SAFETY: hwnd is live during WM_PAINT and paint is writable.
        BeginPaint(state.hwnd, &raw mut paint)
    };
    if dc == HDC::default() {
        eprintln!("Could not begin painting About");
        return;
    }
    if let Err(error) = paint_about_content(dc, state) {
        eprintln!("Could not paint About: {error}");
    }
    let ended = unsafe {
        // SAFETY: paint was initialized by BeginPaint for this hwnd.
        EndPaint(state.hwnd, &raw const paint)
    };
    if !ended.as_bool() {
        eprintln!("Could not finish painting About");
    }
}

fn paint_about_content(dc: HDC, state: &DialogState) -> Result<()> {
    let layout = state.layout();
    let client = RECT {
        left: 0,
        top: 0,
        right: layout.footer.width,
        bottom: layout.footer.y.saturating_add(layout.footer.height),
    };
    fill_color(dc, client, state.palette.background)?;
    fill_color(dc, layout.footer.as_native(), state.palette.footer)?;
    fill_color(
        dc,
        RECT {
            bottom: layout.footer.y.saturating_add(scale(1, state.dpi).max(1)),
            ..layout.footer.as_native()
        },
        state.palette.separator,
    )?;
    unsafe {
        // SAFETY: dc is live, icon is a shared resource handle, and the requested bounds are valid.
        DrawIconEx(
            dc,
            layout.icon.x,
            layout.icon.y,
            state.icon,
            layout.icon.width,
            layout.icon.height,
            0,
            None,
            DI_NORMAL,
        )?;
    }
    draw_text(
        dc,
        "AltTabio",
        layout.title.as_native(),
        state.palette.title,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state.fonts.title.0,
    )?;
    for (label, rect) in [
        (VERSION_LABEL, layout.version),
        ("Open-source Windows task switcher.", layout.description),
        ("Copyright (c) 2026 VibeSlop", layout.copyright),
        ("MIT License", layout.license),
    ] {
        draw_text(
            dc,
            label,
            rect.as_native(),
            state.palette.text,
            DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
            state.fonts.body.0,
        )?;
    }
    draw_text(
        dc,
        REPOSITORY_LABEL,
        layout.repository.as_native(),
        state.palette.link,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state.fonts.link.0,
    )?;
    fill_color(
        dc,
        layout.close_button.as_native(),
        if state.close_pressed {
            state.palette.button_pressed
        } else {
            state.palette.button
        },
    )?;
    frame_color(
        dc,
        layout.close_button.as_native(),
        state.palette.button_border,
        scale(1, state.dpi).max(1),
    )?;
    draw_text(
        dc,
        "Close",
        layout.close_button.as_native(),
        state.palette.text,
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state.fonts.body.0,
    )
}

fn draw_text(
    dc: HDC,
    label: &str,
    mut rect: RECT,
    color: COLORREF,
    format: u32,
    font: HFONT,
) -> Result<()> {
    let previous_font = unsafe {
        // SAFETY: dc is live and font is owned by the dialog for this complete paint callback.
        windows::Win32::Graphics::Gdi::SelectObject(dc, HGDIOBJ(font.0))
    };
    let previous_mode = unsafe {
        // SAFETY: dc is live for the current paint callback.
        SetBkMode(dc, TRANSPARENT)
    };
    let previous_color = unsafe {
        // SAFETY: dc is live and color is a scalar COLORREF.
        SetTextColor(dc, color)
    };
    let text = label.encode_utf16().collect::<Vec<_>>();
    let drawn = unsafe {
        // SAFETY: text and rect remain live throughout this synchronous GDI call.
        DrawTextW(
            dc,
            text.as_ptr(),
            i32::try_from(text.len()).unwrap_or(i32::MAX),
            &raw mut rect,
            format,
        )
    };
    unsafe {
        // SAFETY: these values came from the matching selection and color calls above.
        if previous_font != HGDIOBJ::default() {
            windows::Win32::Graphics::Gdi::SelectObject(dc, previous_font);
        }
        if previous_mode != 0 {
            SetBkMode(
                dc,
                BACKGROUND_MODE(u32::try_from(previous_mode).unwrap_or(TRANSPARENT.0)),
            );
        }
        if previous_color.0 != u32::MAX {
            SetTextColor(dc, previous_color);
        }
    }
    if drawn == 0 {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

fn fill_color(dc: HDC, rect: RECT, color: COLORREF) -> Result<()> {
    let brush = OwnedBrush::new(color)?;
    let filled = unsafe {
        // SAFETY: dc is live and brush remains owned for the synchronous fill.
        FillRect(dc, &raw const rect, brush.0)
    };
    if filled == 0 {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

fn frame_color(dc: HDC, rect: RECT, color: COLORREF, thickness: i32) -> Result<()> {
    let thickness = thickness.max(1);
    for edge in [
        RECT {
            right: rect.right,
            bottom: rect.top.saturating_add(thickness),
            ..rect
        },
        RECT {
            top: rect.bottom.saturating_sub(thickness),
            right: rect.right,
            ..rect
        },
        RECT {
            right: rect.left.saturating_add(thickness),
            bottom: rect.bottom,
            ..rect
        },
        RECT {
            left: rect.right.saturating_sub(thickness),
            bottom: rect.bottom,
            ..rect
        },
    ] {
        fill_color(dc, edge, color)?;
    }
    Ok(())
}

fn apply_window_theme(hwnd: HWND, theme: ResolvedTheme, dark_mode_api: Option<&DarkModeApi>) {
    let dark = theme == ResolvedTheme::Dark;
    if let Some(api) = dark_mode_api {
        api.set_effective_theme(dark);
        api.allow_for_window(hwnd, dark);
    }
    let use_dark_mode = i32::from(dark);
    let result = unsafe {
        // SAFETY: use_dark_mode is BOOL-compatible and remains live for the synchronous DWM call.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const use_dark_mode).cast(),
            u32::try_from(size_of::<i32>()).unwrap_or(u32::MAX),
        )
    };
    if let Err(error) = result {
        eprintln!("Could not apply the About title-bar theme: {error}");
    }
}

fn adjusted_window_rect(client_width: i32, client_height: i32, dpi: u32) -> Result<RECT> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_width,
        bottom: client_height,
    };
    unsafe {
        // SAFETY: rect is writable and the style values match the window created by show.
        AdjustWindowRectExForDpi(
            &raw mut rect,
            WINDOW_STYLE_VALUE,
            false,
            WINDOW_EX_STYLE_VALUE,
            dpi,
        )?;
    }
    rect.right = rect.right.saturating_sub(rect.left);
    rect.bottom = rect.bottom.saturating_sub(rect.top);
    rect.left = 0;
    rect.top = 0;
    Ok(rect)
}

fn centered_window_origin(width: i32, height: i32) -> Result<POINT> {
    let mut cursor = POINT::default();
    unsafe {
        // SAFETY: cursor is writable for the synchronous read.
        GetCursorPos(&raw mut cursor)?;
    }
    let monitor = unsafe {
        // SAFETY: cursor is an initialized screen point and the fallback always returns a monitor.
        MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
    };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..MONITORINFO::default()
    };
    let read = unsafe {
        // SAFETY: monitor is valid and info is a correctly sized writable structure.
        GetMonitorInfoW(monitor, &raw mut info)
    };
    if !read.as_bool() {
        return Err(Error::from_thread());
    }
    let work = info.rcWork;
    Ok(POINT {
        x: work
            .left
            .saturating_add(work.right.saturating_sub(work.left).saturating_sub(width) / 2),
        y: work
            .top
            .saturating_add(work.bottom.saturating_sub(work.top).saturating_sub(height) / 2),
    })
}

fn register_class(
    instance: HINSTANCE,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<()> {
    let cursor = unsafe {
        // SAFETY: IDC_ARROW is a predefined shared cursor.
        LoadCursorW(None, IDC_ARROW)
    }?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(about_window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        lpszClassName: WINDOW_CLASS,
        hIconSm: icon,
        ..WNDCLASSEXW::default()
    };
    let atom = unsafe {
        // SAFETY: class points to initialized static callback and class-name data.
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

fn module_instance() -> Result<HINSTANCE> {
    let module = unsafe {
        // SAFETY: None requests the current executable module.
        GetModuleHandleW(None)
    }?;
    Ok(HINSTANCE(module.0))
}

fn invalidate_dialog(hwnd: HWND) {
    let invalidated = unsafe {
        // SAFETY: hwnd is live and None invalidates the complete client area.
        InvalidateRect(Some(hwnd), None, false)
    };
    if !invalidated.as_bool() {
        eprintln!("Could not redraw About");
    }
}

fn close_dialog(hwnd: HWND) {
    let result = unsafe {
        // SAFETY: hwnd is live and the private message carries no borrowed data.
        PostMessageW(Some(hwnd), WM_DESTROY_DIALOG, WPARAM(0), LPARAM(0))
    };
    if let Err(error) = result {
        eprintln!("Could not request About closure: {error}");
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    let raw = lparam.0.cast_unsigned();
    POINT {
        x: i32::from(low_word(raw).cast_signed()),
        y: i32::from(high_word(raw).cast_signed()),
    }
}

fn low_word(value: usize) -> u16 {
    u16::try_from(value & 0xffff).unwrap_or_default()
}

fn high_word(value: usize) -> u16 {
    u16::try_from(value >> 16 & 0xffff).unwrap_or_default()
}

fn scale(value: i32, dpi: u32) -> i32 {
    let numerator = i64::from(value) * i64::from(dpi) + i64::from(BASE_DPI / 2);
    i32::try_from(numerator / i64::from(BASE_DPI)).unwrap_or(i32::MAX)
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}

fn system_color(index: windows::Win32::Graphics::Gdi::SYS_COLOR_INDEX) -> COLORREF {
    let color = unsafe {
        // SAFETY: index is one of the documented system-color constants.
        GetSysColor(index)
    };
    COLORREF(color)
}

struct DialogFonts {
    body: OwnedFont,
    title: OwnedFont,
    link: OwnedFont,
}

impl DialogFonts {
    fn new(dpi: u32) -> Result<Self> {
        Ok(Self {
            body: OwnedFont::new(dpi, 11, FW_NORMAL.0.cast_signed(), false)?,
            title: OwnedFont::new(dpi, 17, FW_SEMIBOLD.0.cast_signed(), false)?,
            link: OwnedFont::new(dpi, 11, FW_NORMAL.0.cast_signed(), true)?,
        })
    }
}

struct OwnedFont(HFONT);

impl OwnedFont {
    fn new(dpi: u32, points: u32, weight: i32, underline: bool) -> Result<Self> {
        let point_height = i32::try_from((u64::from(points) * u64::from(dpi) + 36) / 72)
            .unwrap_or(i32::MAX)
            .max(1);
        let font = unsafe {
            // SAFETY: scalar values describe a standard Segoe UI font and the face name is static.
            CreateFontW(
                -point_height,
                0,
                0,
                0,
                weight,
                0,
                u32::from(u8::from(underline)),
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(FF_DONTCARE.0),
                w!("Segoe UI"),
            )
        };
        if font == HFONT::default() {
            Err(Error::from_thread())
        } else {
            Ok(Self(font))
        }
    }
}

impl Drop for OwnedFont {
    fn drop(&mut self) {
        let deleted = unsafe {
            // SAFETY: this wrapper uniquely owns the font and no paint callback is active on drop.
            DeleteObject(HGDIOBJ::from(self.0))
        };
        if !deleted.as_bool() {
            eprintln!("Could not release an About font");
        }
    }
}

struct OwnedBrush(HBRUSH);

impl OwnedBrush {
    fn new(color: COLORREF) -> Result<Self> {
        let brush = unsafe {
            // SAFETY: color is a scalar COLORREF and the returned brush is uniquely owned.
            CreateSolidBrush(color)
        };
        if brush == HBRUSH::default() {
            Err(Error::from_thread())
        } else {
            Ok(Self(brush))
        }
    }
}

impl Drop for OwnedBrush {
    fn drop(&mut self) {
        let deleted = unsafe {
            // SAFETY: this wrapper uniquely owns the brush and no fill is active on drop.
            DeleteObject(HGDIOBJ::from(self.0))
        };
        if !deleted.as_bool() {
            eprintln!("Could not release an About brush");
        }
    }
}

fn open_repository() -> std::result::Result<(), String> {
    let repository_url = wide(REPOSITORY_URL);
    let result = unsafe {
        // SAFETY: repository_url remains live and all other string pointers are static or null.
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(repository_url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let result_code = result.0 as usize;
    if result_code <= 32 {
        Err(format!(
            "Could not open the AltTabio GitHub page (ShellExecuteW returned {result_code})"
        ))
    } else {
        Ok(())
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_content_uses_package_version_and_canonical_repository() {
        assert_eq!(
            VERSION_LABEL,
            concat!("Version ", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(REPOSITORY_URL, "https://github.com/vibeslop/AltTabio");
        assert_eq!(REPOSITORY_LABEL, "github.com/vibeslop/AltTabio");
    }

    #[test]
    fn about_palette_follows_the_resolved_theme() {
        let light = AboutPalette::new(ResolvedTheme::Light);
        let dark = AboutPalette::new(ResolvedTheme::Dark);

        assert_ne!(light.background, dark.background);
        assert_ne!(light.text, dark.text);
        assert_eq!(dark.background, rgb(32, 32, 32));
        assert_eq!(dark.text, rgb(240, 240, 240));
    }

    #[test]
    fn about_layout_keeps_the_link_and_close_button_inside_the_client() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        assert!(layout.repository.x >= 0);
        assert!(layout.repository.x + layout.repository.width <= CLIENT_WIDTH);
        assert!(layout.close_button.x >= 0);
        assert!(layout.close_button.x + layout.close_button.width <= CLIENT_WIDTH);
        assert!(layout.close_button.y >= layout.footer.y);
        assert!(layout.close_button.y + layout.close_button.height <= CLIENT_HEIGHT);
    }

    #[test]
    fn about_layout_uses_compact_vertical_text_spacing() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        assert_eq!(
            layout.version.y - (layout.title.y + layout.title.height),
            10
        );
        assert_eq!(
            layout.description.y - (layout.version.y + layout.version.height),
            8
        );
        assert_eq!(
            layout.repository.y - (layout.description.y + layout.description.height),
            8
        );
        assert_eq!(
            layout.copyright.y - (layout.repository.y + layout.repository.height),
            14
        );
        assert_eq!(
            layout.license.y - (layout.copyright.y + layout.copyright.height),
            2
        );
    }

    #[test]
    fn about_text_column_has_equal_horizontal_margins() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        for text in [
            layout.title,
            layout.version,
            layout.description,
            layout.repository,
            layout.copyright,
            layout.license,
        ] {
            assert_eq!(text.x, CLIENT_WIDTH - (text.x + text.width));
        }
    }
}
