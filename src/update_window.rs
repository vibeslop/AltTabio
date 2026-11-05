//! Modeless update status, owned and refreshed by the application message loop.
#[cfg(test)]
mod tests;
use crate::dialog_host::{self, DialogFrame, DialogWindow, ModalDialog};
use crate::native_drawing::{OwnedBrush, OwnedFont, rgb, system_color};
use crate::native_theme::{DarkModeApi, resolve_current_theme};
use crate::win32::{self, low_word, wide};
use alttabio::dialog_layout::{Size, known_dpi, scale};
use alttabio::settings::Theme;
use alttabio::theme::ResolvedTheme;
use alttabio::update_status::{Command, View};
use std::cell::RefCell;
use std::rc::Rc;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, COLOR_WINDOWTEXT, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, InvalidateRect,
    SetBkColor, SetTextColor,
};
use windows::Win32::UI::Controls::{
    ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, PBM_SETMARQUEE, PBM_SETPOS,
    PBS_MARQUEE, PROGRESS_CLASSW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, HMENU,
    IsDialogMessageW, IsWindowVisible, MSG, MoveWindow, PostMessageW, SW_HIDE, SW_SHOW,
    SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORDLG, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_DPICHANGED, WM_SETFONT, WM_SETTINGCHANGE, WM_THEMECHANGED, WS_CAPTION,
    WS_CHILD, WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_EX_CONTROLPARENT, WS_OVERLAPPED, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HRESULT, PCWSTR, Result, w};

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn SetWindowTheme(hwnd: HWND, sub_app_name: PCWSTR, sub_id_list: PCWSTR) -> HRESULT;
}

const FRAME: DialogFrame = DialogFrame {
    name: "updates",
    class: "AltTabioUpdateStatus",
    title: "AltTabio updates",
    style: WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_CLIPCHILDREN.0),
    ex_style: WINDOW_EX_STYLE(WS_EX_APPWINDOW.0 | WS_EX_CONTROLPARENT.0),
};
const PRIMARY: usize = 100;
const DETAILS: usize = 101;
const CLOSE: usize = 2;

#[derive(Default)]
pub(crate) struct Events {
    pub close: bool,
    pub command: Option<Command>,
    details: bool,
    dpi: Option<(u32, RECT)>,
    theme: bool,
    error: Option<String>,
}

struct Callback {
    owner: HWND,
    events: Rc<RefCell<Events>>,
    primary: HWND,
    command: Option<Command>,
    background: Rc<OwnedBrush>,
    background_color: COLORREF,
    text: COLORREF,
}

impl DialogWindow for Callback {
    const FRAME: DialogFrame = FRAME;
    fn attach(&mut self, _: HWND) {}
    fn handle_message(
        &mut self,
        _window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        let mut events = self.events.borrow_mut();
        match message {
            WM_CLOSE => events.close = true,
            WM_COMMAND => match usize::from(low_word(wparam.0)) {
                CLOSE => events.close = true,
                PRIMARY => events.command = self.command,
                1 => {
                    // SAFETY: GetFocus returns a borrowed child on this UI thread.
                    if unsafe { GetFocus() } == self.primary {
                        events.command = self.command;
                    }
                }
                DETAILS => events.details = true,
                _ => return None,
            },
            WM_DPICHANGED => {
                // SAFETY: Windows supplies a readable RECT for the duration of WM_DPICHANGED.
                if let Some(rect) = unsafe { (lparam.0 as *const RECT).as_ref() } {
                    events.dpi = Some((u32::from(low_word(wparam.0)), *rect));
                }
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => events.theme = true,
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORDLG => {
                let dc = HDC(wparam.0 as *mut _);
                // SAFETY: Windows lends this live control DC for synchronous color selection.
                let failed = unsafe { SetTextColor(dc, self.text) }.0 == u32::MAX;
                // SAFETY: same borrowed DC; the background brush lives until child destruction.
                let failed =
                    failed || unsafe { SetBkColor(dc, self.background_color) }.0 == u32::MAX;
                if failed {
                    events.error = Some("Could not set update text colors".into());
                }
                return Some(LRESULT(self.background.0.0 as isize));
            }
            _ => return None,
        }
        // SAFETY: the owning app window is live; the wake carries no pointers or borrowed data.
        if let Err(error) = unsafe {
            PostMessageW(
                Some(self.owner),
                crate::app_messages::WM_UPDATE_UI,
                WPARAM(0),
                LPARAM(0),
            )
        } {
            events.error = Some(format!("Could not wake update UI: {error}"));
        }
        Some(LRESULT(0))
    }
}

pub(crate) struct UpdateWindow {
    dialog: ModalDialog<Callback>,
    controls: [HWND; 10],
    fonts: Vec<OwnedFont>,
    events: Rc<RefCell<Events>>,
    api: Option<DarkModeApi>,
    dpi: u32,
    theme: Theme,
    view: Option<View>,
    details_open: bool,
}

impl UpdateWindow {
    pub(crate) fn create(owner: HWND, theme: Theme) -> Result<Self> {
        let instance = win32::module_instance()?;
        dialog_host::register_class::<Callback>(
            instance,
            windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            HBRUSH::default(),
        )?;
        // SAFETY: the app owns owner and GetDpiForWindow only queries it.
        let dpi = known_dpi(unsafe { GetDpiForWindow(owner) });
        let dark = resolve_current_theme(theme) == ResolvedTheme::Dark;
        let background_color = if dark {
            rgb(32, 32, 32)
        } else {
            rgb(250, 250, 250)
        };
        let events = Rc::new(RefCell::new(Events::default()));
        let size = FRAME.window_size(Size::new(470, 300).scaled(dpi), dpi)?;
        let origin = win32::work_area_near_window(owner)?.centered(size);
        let dialog = ModalDialog::create(
            instance,
            origin,
            size,
            None,
            Callback {
                owner,
                events: events.clone(),
                primary: HWND::default(),
                command: None,
                background: Rc::new(OwnedBrush::new(background_color)?),
                background_color,
                text: if dark {
                    rgb(240, 240, 240)
                } else {
                    rgb(26, 26, 26)
                },
            },
        )?;
        let controls = create_controls(dialog.window(), instance)?;
        dialog.host()?.state_mut()?.primary = controls[5];
        let mut window = Self {
            dialog,
            controls,
            fonts: Vec::new(),
            events,
            api: None,
            dpi,
            theme,
            view: None,
            details_open: false,
        };
        window.apply_theme()?;
        window.layout()?;
        Ok(window)
    }

    pub(crate) fn show(&self) {
        self.dialog.show_in_front();
        if let Err(error) = dialog_host::focus(self.controls[6]) {
            eprintln!("Could not focus update window: {error}");
        }
    }

    pub(crate) fn hide(&self) {
        // SAFETY: window is owned here; ShowWindow reports previous visibility, not failure.
        unsafe {
            let _previous = ShowWindow(self.dialog.window(), SW_HIDE);
        }
    }

    pub(crate) fn translate(&self, message: &MSG) -> bool {
        if self.dialog.is_closed() {
            return false;
        }
        // SAFETY: window and message belong to this thread; no callback-state borrow spans dispatch.
        unsafe {
            IsWindowVisible(self.dialog.window()).as_bool()
                && IsDialogMessageW(self.dialog.window(), message).as_bool()
        }
    }

    pub(crate) fn take_events(&mut self) -> Result<Events> {
        let mut events = std::mem::take(&mut *self.events.borrow_mut());
        if let Some(error) = events.error.take() {
            eprintln!("{error}");
        }
        if let Some((dpi, rect)) = events.dpi.take() {
            self.dpi = known_dpi(dpi);
            // SAFETY: suggested rectangle was copied from Windows; no state borrow spans moving.
            unsafe {
                SetWindowPos(
                    self.dialog.window(),
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }?;
            self.layout()?;
        }
        if events.theme {
            self.apply_theme()?;
        }
        if events.details {
            self.details_open = !self.details_open;
            self.layout()?;
        }
        Ok(events)
    }

    pub(crate) fn refresh(&mut self, view: View, theme: Theme) -> Result<()> {
        set_text(
            self.dialog.window(),
            alttabio::localization::tr("AltTabio updates"),
        )?;
        if self.theme != theme {
            self.theme = theme;
            self.apply_theme()?;
        }
        if self.view.as_ref() == Some(&view) {
            return Ok(());
        }
        let old_command = self.view.as_ref().and_then(|v| v.primary);
        if self
            .view
            .as_ref()
            .is_some_and(|old| old.title != view.title)
        {
            self.details_open = false;
        }
        for (index, text) in [
            (0, view.title.as_str()),
            (1, &view.body),
            (2, &view.note),
            (6, view.close),
            (8, &view.detail),
        ] {
            set_text(self.controls[index], text)?;
        }
        set_text(self.controls[5], view.primary.map_or("", |p| p.0))?;
        self.dialog.host()?.state_mut()?.command = view.primary.map(|p| p.1);
        // SAFETY: controls are owned children and control messages carry scalar progress only.
        unsafe {
            SendMessageW(
                self.controls[3],
                PBM_SETMARQUEE,
                Some(WPARAM(usize::from(view.progress == Some(None)))),
                Some(LPARAM(50)),
            );
            SendMessageW(
                self.controls[4],
                PBM_SETPOS,
                Some(WPARAM(view.progress.flatten().unwrap_or_default() as usize)),
                Some(LPARAM(0)),
            );
        }
        if old_command != view.primary {
            // SAFETY: GetFocus returns a borrowed child on this UI thread.
            if unsafe { GetFocus() } == self.controls[5] {
                dialog_host::focus(self.controls[6])?;
            }
        }
        self.view = Some(view);
        self.layout()
    }

    fn layout(&mut self) -> Result<()> {
        let heading = OwnedFont::pixels(self.dpi, 22, FW_SEMIBOLD.0.cast_signed())?;
        let body = OwnedFont::pixels(self.dpi, 14, FW_NORMAL.0.cast_signed())?;
        let small = OwnedFont::pixels(self.dpi, 12, FW_NORMAL.0.cast_signed())?;
        for (i, control) in self.controls.iter().enumerate() {
            let font = if i == 0 {
                heading.0
            } else if i == 2 {
                small.0
            } else {
                body.0
            };
            // SAFETY: fonts outlive all controls' use, replaced only after assigning every child.
            unsafe {
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
        self.fonts = vec![heading, body, small];
        let extra = if self.details_open { 120 } else { 0 };
        // Russian action labels name both installation and restart. Keep their full wording.
        let (wider, primary_extra, close_extra) = if alttabio::localization::current_language()
            == alttabio::localization::Language::Russian
        {
            (116, 100, 16)
        } else {
            (0, 0, 0)
        };
        let size = FRAME.window_size(
            Size::new(470 + wider, 300 + extra).scaled(self.dpi),
            self.dpi,
        )?;
        // SAFETY: retain position/z-order/focus; only resize this owned window.
        unsafe {
            SetWindowPos(
                self.dialog.window(),
                None,
                0,
                0,
                size.width,
                size.height,
                SWP_NOZORDER | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE,
            )
        }?;
        for (index, (x, y, width, height)) in [
            (28, 26, 414 + wider, 34),
            (28, 72, 414 + wider, 52),
            (28, 155, 414 + wider, 44),
            (28, 135, 414 + wider, 5),
            (28, 135, 414 + wider, 5),
            (220, 238 + extra, 150 + primary_extra, 34),
            (378 + primary_extra, 238 + extra, 64 + close_extra, 34),
            (28, 201, 150 + wider, 28),
            (28, 238, 414 + wider, 108),
            (0, 0, 470 + wider, 300 + extra),
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: every HWND is an owned child, geometry is scaled without retaining pointers.
            unsafe {
                MoveWindow(
                    self.controls[index],
                    scale(x, self.dpi),
                    scale(y, self.dpi),
                    scale(width, self.dpi),
                    scale(height, self.dpi),
                    true,
                )
            }?;
        }
        if let Some(view) = &self.view {
            for (i, visible) in [
                (3, view.progress == Some(None)),
                (4, view.progress.is_some_and(|p| p.is_some())),
                (5, view.primary.is_some()),
                (7, !view.detail.is_empty()),
                (8, self.details_open && !view.detail.is_empty()),
            ] {
                // SAFETY: ShowWindow only changes visibility of owned children.
                unsafe {
                    let _previous =
                        ShowWindow(self.controls[i], if visible { SW_SHOW } else { SW_HIDE });
                }
            }
        }
        set_text(
            self.controls[7],
            alttabio::localization::tr(if self.details_open {
                "Hide details"
            } else {
                "Technical details"
            }),
        )?;
        // SAFETY: the full owned window is invalidated after layout, without synchronous paint.
        unsafe { InvalidateRect(Some(self.dialog.window()), None, true) }.ok()
    }

    fn apply_theme(&mut self) -> Result<()> {
        let dark = resolve_current_theme(self.theme) == ResolvedTheme::Dark;
        let mut high = windows::Win32::UI::Accessibility::HIGHCONTRASTW {
            cbSize: u32::try_from(std::mem::size_of::<
                windows::Win32::UI::Accessibility::HIGHCONTRASTW,
            >())
            .unwrap_or_default(),
            ..Default::default()
        };
        // SAFETY: high is a correctly sized writable system parameter buffer.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
                windows::Win32::UI::WindowsAndMessaging::SPI_GETHIGHCONTRAST,
                high.cbSize,
                Some((&raw mut high).cast()),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        }?;
        let high_contrast = high.dwFlags.0 & 1 != 0;
        let background = if high_contrast {
            system_color(COLOR_WINDOW)
        } else if dark {
            rgb(32, 32, 32)
        } else {
            rgb(250, 250, 250)
        };
        let text = if high_contrast {
            system_color(COLOR_WINDOWTEXT)
        } else if dark {
            rgb(240, 240, 240)
        } else {
            rgb(26, 26, 26)
        };
        {
            let mut state = self.dialog.host()?.state_mut()?;
            state.background = Rc::new(OwnedBrush::new(background)?);
            state.background_color = background;
            state.text = text;
        }
        if self.api.is_none() && !high_contrast {
            match DarkModeApi::load(dark) {
                Ok(api) => self.api = Some(api),
                Err(error) => eprintln!("Native update controls could not load dark mode: {error}"),
            }
        }
        dialog_host::set_dark_title_bar(self.dialog.window(), dark && !high_contrast)?;
        for control in self.controls {
            if let Some(api) = &self.api {
                api.allow_for_window(control, dark && !high_contrast);
            }
            let name = if high_contrast {
                w!("")
            } else if dark {
                w!("DarkMode_Explorer")
            } else {
                w!("Explorer")
            };
            // SAFETY: all HWNDs are live controls; static theme strings remain valid during the call.
            unsafe { SetWindowTheme(control, name, PCWSTR::null()).ok() }?;
        }
        Ok(())
    }
}

fn set_text(window: HWND, text: &str) -> Result<()> {
    let text = wide(text);
    // SAFETY: the owned child copies the NUL-terminated buffer synchronously.
    unsafe { SetWindowTextW(window, PCWSTR(text.as_ptr())) }
}
fn create_controls(
    parent: HWND,
    instance: windows::Win32::Foundation::HINSTANCE,
) -> Result<[HWND; 10]> {
    // SAFETY: this struct is initialized for a synchronous control-class registration.
    unsafe {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: u32::try_from(std::mem::size_of::<INITCOMMONCONTROLSEX>()).unwrap_or_default(),
            dwICC: ICC_PROGRESS_CLASS,
        })
    }
    .ok()?;
    let mut controls = [HWND::default(); 10];
    // A native STATIC paints the client background using WM_CTLCOLORSTATIC's cached brush.
    // It is created before its siblings, so no custom drawing runs in our window callback.
    // SAFETY: this child belongs to parent; static class and instance live for creation.
    controls[9] = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(""),
            WS_CHILD | WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }?;
    for (index, class, flags, id) in [
        (0, w!("STATIC"), 0, 200),
        (1, w!("STATIC"), 0, 201),
        (2, w!("STATIC"), 0, 202),
        (3, PROGRESS_CLASSW, PBS_MARQUEE, 203),
        (4, PROGRESS_CLASSW, 0, 204),
        (
            5,
            w!("BUTTON"),
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            PRIMARY,
        ),
        (6, w!("BUTTON"), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, CLOSE),
        (
            7,
            w!("BUTTON"),
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            DETAILS,
        ),
        (
            8,
            w!("EDIT"),
            WS_TABSTOP.0 | WS_VSCROLL.0 | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
            205,
        ),
    ] {
        // SAFETY: strings are static, parent and instance are owned; children die with the dialog.
        controls[index] = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | flags),
                0,
                0,
                0,
                0,
                Some(parent),
                Some(HMENU(id as *mut _)),
                Some(instance),
                None,
            )
        }?;
    }

    // SAFETY: place the native background behind every other owned child without activating it.
    unsafe {
        SetWindowPos(
            controls[9],
            Some(windows::Win32::UI::WindowsAndMessaging::HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
        )
    }?;
    Ok(controls)
}
