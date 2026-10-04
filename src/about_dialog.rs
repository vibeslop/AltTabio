use crate::dialog_host::{
    self, DialogFrame, DialogWindow, Keyboard, ModalDialog, dark, native_rect, point_from_lparam,
};
use crate::native_drawing::{
    DRAW_TEXT_CENTER, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE, DRAW_TEXT_VCENTER, OwnedFont,
    draw_text_with_font, fill_color, frame_color, rgb, system_color,
};
use crate::{app_icon, native_theme::DarkModeApi};
use alttabio::dialog_layout::{MIN_DPI, Point, Rect, Size, hairline, scale};
use alttabio::settings::IconColor;
use alttabio::theme::ResolvedTheme;
use std::ffi::c_void;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    EndPaint, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, InvalidateRect, PAINTSTRUCT,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DrawIconEx, GetClientRect, HICON, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos, WINDOW_STYLE, WM_CAPTURECHANGED, WM_CLOSE, WM_DPICHANGED, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PAINT, WM_PRINTCLIENT, WM_SIZE, WS_CAPTION,
    WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{PCWSTR, Result, w};

pub(crate) const WINDOW_CLASS_NAME: &str = "AltTabioRustAbout";
const FRAME: DialogFrame = DialogFrame {
    name: "About",
    class: WINDOW_CLASS_NAME,
    title: "About AltTabio",
    style: WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0),
    ex_style: WS_EX_DLGMODALFRAME,
};
const REPOSITORY_URL: &str = "https://github.com/vibeslop/AltTabio";
const REPOSITORY_LABEL: &str = "github.com/vibeslop/AltTabio";
const VERSION_LABEL: &str = concat!("Version ", env!("CARGO_PKG_VERSION"));
const CLIENT_WIDTH: i32 = 430;
const CLIENT_HEIGHT: i32 = 344;
const CONTENT_HORIZONTAL_MARGIN: i32 = 94;

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
                background: dark::BACKGROUND,
                footer: rgb(38, 38, 38),
                text: dark::TEXT,
                title: dark::TEXT,
                link: rgb(76, 194, 255),
                separator: rgb(68, 68, 68),
                button: dark::CONTROL_SURFACE,
                button_pressed: dark::PRESSED_SURFACE,
                button_border: dark::CONTROL_BORDER,
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
    icon: HICON,
    dark_mode_api: Option<DarkModeApi>,
    dpi: u32,
    close_pressed: bool,
    open_repository_requested: bool,
}

impl DialogState {
    fn new(theme: ResolvedTheme, dpi: u32, icon: HICON) -> Result<Self> {
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

impl DialogWindow for DialogState {
    const FRAME: DialogFrame = FRAME;

    fn attach(&mut self, window: HWND) {
        self.hwnd = window;
    }

    fn handle_message(
        &mut self,
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        handle_about_message(self, window, message, wparam, lparam)
    }
}

pub fn show(theme: ResolvedTheme, icon_color: IconColor) -> std::result::Result<(), String> {
    let instance = dialog_host::module_instance().map_err(|error| error.to_string())?;
    let icon = app_icon::load_app(instance, icon_color).map_err(|error| error.to_string())?;
    dialog_host::register_class::<DialogState>(instance, icon, HBRUSH::default())
        .map_err(|error| error.to_string())?;
    let dpi = unsafe {
        // SAFETY: GetDpiForSystem has no pointer or lifetime preconditions.
        GetDpiForSystem()
    }
    .max(MIN_DPI);
    let (origin, window_size) = window_bounds(dpi).map_err(|error| error.to_string())?;
    let state = DialogState::new(theme, dpi, icon)
        .map_err(|error| format!("Could not prepare About: {error}"))?;
    let dialog = ModalDialog::create(instance, origin, window_size, None, state)
        .map_err(|error| format!("Could not create the About dialog: {error}"))?;
    let window = dialog.window();
    app_icon::apply_to_window(window, instance, icon_color)
        .map_err(|error| format!("Could not apply the selected About icon: {error}"))?;
    apply_initial_window_dpi(&dialog)
        .map_err(|error| format!("Could not size About for this display: {error}"))?;
    {
        let state = dialog.host().ok().and_then(dialog_host::DialogHost::state);
        let dark_mode_api = state
            .as_ref()
            .and_then(|state| state.dark_mode_api.as_ref());
        apply_window_theme(window, theme, dark_mode_api);
    }
    dialog.show_in_front();

    let Some(state) = dialog
        .run(Keyboard::WindowProcedure)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    if state.open_repository_requested {
        open_repository()?;
    }
    Ok(())
}

/// The origin and outer size that center About's client area at `dpi` on the cursor's monitor.
fn window_bounds(dpi: u32) -> Result<(Point, Size)> {
    let size = FRAME.window_size(Size::new(CLIENT_WIDTH, CLIENT_HEIGHT).scaled(dpi), dpi)?;
    let origin = dialog_host::work_area_near_cursor()?.centered(size);
    Ok((origin, size))
}

fn apply_initial_window_dpi(dialog: &ModalDialog<DialogState>) -> Result<()> {
    let window = dialog.window();
    let dpi = unsafe {
        // SAFETY: window is the live About window whose monitor determines its effective DPI.
        GetDpiForWindow(window)
    }
    .max(MIN_DPI);
    // The state borrow ends with this statement, before SetWindowPos dispatches messages.
    dialog.host()?.state_mut()?.update_dpi(dpi)?;
    let (origin, size) = window_bounds(dpi)?;
    unsafe {
        // SAFETY: window is live and the computed bounds are within its target monitor work area.
        SetWindowPos(
            window,
            None,
            origin.x,
            origin.y,
            size.width,
            size.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
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
            if state.layout().close_button.contains_point(point) {
                state.close_pressed = true;
                unsafe {
                    // SAFETY: hwnd is the live dialog receiving the button press. The result is
                    // the previous capture window, not a failure.
                    let _previous_capture = SetCapture(hwnd);
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
            dialog_host::handle_dpi_changed(hwnd, wparam, lparam, FRAME.name, |dpi| {
                state.update_dpi(dpi)
            });
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            close_dialog(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn handle_left_button_up(state: &mut DialogState, hwnd: HWND, point: Point) {
    if state.close_pressed {
        state.close_pressed = false;
        let released = unsafe {
            // SAFETY: this UI thread owns any capture taken on button down.
            ReleaseCapture()
        };
        if let Err(error) = released {
            eprintln!("Could not release the About mouse capture: {error}");
        }
        if state.layout().close_button.contains_point(point) {
            close_dialog(hwnd);
        } else {
            invalidate_dialog(hwnd);
        }
    } else if state.layout().repository.contains_point(point) {
        state.open_repository_requested = true;
        close_dialog(hwnd);
    }
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
        bottom: layout.footer.bottom(),
    };
    fill_color(dc, client, state.palette.background)?;
    fill_color(dc, native_rect(layout.footer), state.palette.footer)?;
    fill_color(
        dc,
        RECT {
            bottom: layout.footer.y.saturating_add(hairline(state.dpi)),
            ..native_rect(layout.footer)
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
    draw_text_with_font(
        dc,
        "AltTabio",
        native_rect(layout.title),
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
        draw_text_with_font(
            dc,
            label,
            native_rect(rect),
            state.palette.text,
            DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
            state.fonts.body.0,
        )?;
    }
    draw_text_with_font(
        dc,
        REPOSITORY_LABEL,
        native_rect(layout.repository),
        state.palette.link,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state.fonts.link.0,
    )?;
    fill_color(
        dc,
        native_rect(layout.close_button),
        if state.close_pressed {
            state.palette.button_pressed
        } else {
            state.palette.button
        },
    )?;
    frame_color(
        dc,
        native_rect(layout.close_button),
        state.palette.button_border,
        hairline(state.dpi),
    )?;
    draw_text_with_font(
        dc,
        "Close",
        native_rect(layout.close_button),
        state.palette.text,
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state.fonts.body.0,
    )
}

fn apply_window_theme(hwnd: HWND, theme: ResolvedTheme, dark_mode_api: Option<&DarkModeApi>) {
    let dark = theme == ResolvedTheme::Dark;
    if let Some(api) = dark_mode_api {
        api.set_effective_theme(dark);
        api.allow_for_window(hwnd, dark);
    }
    if let Err(error) = dialog_host::set_dark_title_bar(hwnd, dark) {
        eprintln!("Could not apply the About title-bar theme: {error}");
    }
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
    if let Err(error) = dialog_host::request_close(hwnd) {
        eprintln!("Could not request About closure: {error}");
    }
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

fn open_repository() -> std::result::Result<(), String> {
    let repository_url = dialog_host::wide(REPOSITORY_URL);
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

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::dialog_layout::BASE_DPI;

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
