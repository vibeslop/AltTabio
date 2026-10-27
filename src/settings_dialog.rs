mod control_host;
mod controls;
mod drawing;
mod navigation;
#[cfg(test)]
#[path = "settings_dialog/tests.rs"]
mod page_tests;
mod paint;
mod selectors;
#[cfg(test)]
mod test_support;
mod theme;

use crate::dialog_host::{self, DialogFrame, DialogHost, DialogWindow, Keyboard, ModalDialog};
use crate::native_drawing::{OwnedBrush, OwnedFont, system_color};
use crate::win32::{self, high_word, low_word};
use crate::{
    app_icon,
    native_theme::{DarkModeApi, resolve_current_theme},
};
use alttabio::dialog_layout::{Size, known_dpi};
use alttabio::failure_run::SharedFailureRun;
use alttabio::settings::{IconColor, Settings, Theme};
use alttabio::settings_form::{
    Category, Choices, Control, DialogButton, ICON_CHOICES, MIN_CLIENT_HEIGHT, MIN_CLIENT_WIDTH,
    Selector, SettingOption, SettingsLayout, SettingsPages, THEME_CHOICES,
};
use alttabio::theme::ResolvedTheme;
use controls::{DialogControls, is_checked, selected_index};
use std::cell::Cell;
use std::ffi::c_void;
use theme::ThemePalette;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, COLOR_WINDOWTEXT, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, HFONT, InvalidateRect,
};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, CBN_SELCHANGE, MINMAXINFO, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLORDLG, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC,
    WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_MOUSEWHEEL, WM_PRINTCLIENT,
    WM_SIZE, WM_VSCROLL, WS_CAPTION, WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_EX_CONTROLPARENT,
    WS_EX_DLGMODALFRAME, WS_MAXIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_THICKFRAME,
};
use windows::core::Result;

pub(crate) const WINDOW_CLASS_NAME: &str = "AltTabioRustSettings";
const WM_SETTINGS_FOCUS: u32 = WM_APP + 21;
const WM_SETTINGS_NAVIGATE: u32 = WM_APP + 22;
const FRAME: DialogFrame = DialogFrame {
    name: "settings",
    class: WINDOW_CLASS_NAME,
    title: "AltTabio Settings",
    style: WINDOW_STYLE(
        WS_OVERLAPPED.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | WS_THICKFRAME.0
            | WS_MAXIMIZEBOX.0
            | WS_CLIPCHILDREN.0,
    ),
    ex_style: WINDOW_EX_STYLE(WS_EX_DLGMODALFRAME.0 | WS_EX_CONTROLPARENT.0 | WS_EX_APPWINDOW.0),
};

pub fn show(
    owner: HWND,
    settings: &Settings,
    on_open: impl FnOnce(HWND),
) -> Result<Option<Settings>> {
    let instance = win32::module_instance()?;
    register_class(instance)?;
    let dpi = owner_dpi(owner);
    let layout = SettingsLayout::for_dpi(dpi);
    let work_area = win32::work_area_near_window(owner)?;
    let desired_size = FRAME.window_size(layout.client, dpi)?;
    let window_size = Size::new(
        desired_size.width.min(work_area.width),
        desired_size.height.min(work_area.height),
    );
    let window_origin = work_area.centered(window_size);
    let initial_dark = resolve_current_theme(settings.appearance.theme) == ResolvedTheme::Dark;
    let dark_mode_api = match DarkModeApi::load(initial_dark) {
        Ok(api) => Some(api),
        Err(error) => {
            eprintln!("Native dark settings controls are unavailable: {error}");
            None
        }
    };
    let dialog = ModalDialog::create(
        instance,
        window_origin,
        window_size,
        // Native ownership would put Settings in the overlay's topmost Z-order band.
        None,
        DialogState::new(settings.clone(), dpi, instance, dark_mode_api),
    )?;
    let window = dialog.window();
    let host = dialog.host()?;
    // The mutable borrow makes any synchronous callback re-entry during setup fail closed.
    host.state_mut()?.create_controls(window, instance, host)?;
    dialog.show_in_front();
    let ok_button = host
        .state_mut()?
        .controls
        .get(Control::Navigation(Category::General));
    if let Err(error) = dialog_host::focus(ok_button) {
        eprintln!("Could not focus the settings category: {error}");
    }
    on_open(window);
    let Some(state) = dialog.run(Keyboard::DialogNavigation)? else {
        return Ok(None);
    };
    Ok(state.accepted.then_some(state.settings))
}

struct DialogState {
    hwnd: HWND,
    controls: DialogControls,
    fonts: Option<DialogFonts>,
    previews: Option<selectors::Previews>,
    background: Option<OwnedBrush>,
    background_color: COLORREF,
    text_color: COLORREF,
    palette: ThemePalette,
    dark_mode_api: Option<DarkModeApi>,
    settings: Settings,
    instance: HINSTANCE,
    dpi: u32,
    client: Size,
    pages: SettingsPages,
    hovered: Cell<HWND>,
    accepted: bool,
    /// Failures of `BeginPaint` across every custom-painted control. Painting runs from the
    /// control subclass, which only borrows the state shared.
    begin_paint_failures: SharedFailureRun,
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
        handle_settings_message(self, window, message, wparam, lparam)
    }
}

impl DialogState {
    fn new(
        settings: Settings,
        dpi: u32,
        instance: HINSTANCE,
        dark_mode_api: Option<DarkModeApi>,
    ) -> Self {
        Self {
            hwnd: HWND::default(),
            controls: DialogControls::default(),
            fonts: None,
            previews: None,
            background: None,
            background_color: system_color(COLOR_WINDOW),
            text_color: system_color(COLOR_WINDOWTEXT),
            palette: ThemePalette::new(false),
            dark_mode_api,
            settings,
            instance,
            dpi,
            client: SettingsLayout::for_dpi(dpi).client,
            pages: SettingsPages::default(),
            hovered: Cell::new(HWND::default()),
            accepted: false,
            begin_paint_failures: SharedFailureRun::default(),
        }
    }

    fn create_controls(
        &mut self,
        parent: HWND,
        instance: HINSTANCE,
        host: &DialogHost<Self>,
    ) -> Result<()> {
        let layout = SettingsLayout::for_dpi(self.dpi);
        let fonts = DialogFonts::create(self.dpi)?;
        self.previews = Some(selectors::Previews::load(instance)?);
        self.controls =
            DialogControls::create(parent, instance, &layout, self.dpi, &fonts, &self.settings)?;
        self.fonts = Some(fonts);
        control_host::install_custom_control_painting(&self.controls, host)?;
        self.sync_right_button_release_enabled();
        self.apply_selected_icon();
        self.apply_selected_theme();
        self.resize()?;
        self.select_category(Category::General)?;
        self.sync_save_enabled();
        Ok(())
    }

    fn update_dpi(&mut self, dpi: u32) -> Result<()> {
        let fonts = DialogFonts::create(dpi)?;
        self.controls.apply_fonts(&fonts);
        self.fonts = Some(fonts);
        self.pages.rescale(self.dpi, dpi);
        self.dpi = dpi;
        self.resize()
    }

    /// The value chosen in `selector`, or `current` while the list has no selection.
    fn selected<T: Copy + Default + PartialEq>(
        &self,
        selector: Selector,
        choices: &Choices<T>,
        current: T,
    ) -> T {
        selected_index(self.controls.get(Control::Selector(selector)))
            .map_or(current, |index| choices.value_at(index))
    }

    fn selected_theme(&self) -> Theme {
        self.selected(
            Selector::Theme,
            &THEME_CHOICES,
            self.settings.appearance.theme,
        )
    }

    fn selected_icon(&self) -> IconColor {
        self.selected(Selector::Icon, &ICON_CHOICES, self.settings.appearance.icon)
    }

    fn sync_right_button_release_enabled(&self) {
        let enabled = is_checked(
            self.controls
                .checkbox(SettingOption::RightButtonWheelSwitching),
        );
        unsafe {
            // SAFETY: both option handles are live controls owned by this dialog. The result
            // reports the previous state, not a failure.
            let _was_disabled = EnableWindow(
                self.controls
                    .checkbox(SettingOption::ReleaseRightButtonSwitches),
                enabled,
            );
        }
    }

    fn apply_selected_theme(&mut self) {
        let theme = self.selected_theme();
        let dark = resolve_current_theme(theme) == ResolvedTheme::Dark;
        let palette = ThemePalette::new(dark);
        match OwnedBrush::new(palette.background) {
            Ok(brush) => self.background = Some(brush),
            Err(error) => eprintln!("Could not create the settings background brush: {error}"),
        }
        self.background_color = palette.background;
        self.text_color = palette.text;
        self.palette = palette;
        if let Some(api) = self.dark_mode_api.as_ref() {
            api.set_effective_theme(dark);
        }
        theme::apply_native_theme_hooks(
            self.hwnd,
            &self.controls,
            dark,
            self.dark_mode_api.as_ref(),
        );
        self.redraw();
    }

    fn redraw(&self) {
        for (_, child) in self.controls.iter() {
            // SAFETY: the child HWND is borrowed from this live dialog.
            if !unsafe { InvalidateRect(Some(child), None, false) }.as_bool() {
                eprintln!("Could not redraw a settings child control");
            }
        }
        let invalidated = unsafe {
            // SAFETY: hwnd is live and None invalidates its complete client area synchronously.
            InvalidateRect(Some(self.hwnd), None, true)
        };
        if !invalidated.as_bool() {
            eprintln!("Could not redraw settings");
        }
    }

    fn apply_selected_icon(&self) {
        if let Err(error) =
            app_icon::apply_to_window(self.hwnd, self.instance, self.selected_icon())
        {
            eprintln!("Could not preview the selected settings icon: {error}");
        }
    }

    fn current_settings(&self) -> Settings {
        let mut settings = self.settings.clone();
        for option in SettingOption::ALL {
            option.write(&mut settings, is_checked(self.controls.checkbox(option)));
        }
        settings.appearance.icon = self.selected_icon();
        settings.appearance.theme = self.selected_theme();
        settings
    }

    fn sync_save_enabled(&self) {
        // SAFETY: the Save button is owned by this dialog. EnableWindow returns the previous state.
        unsafe {
            let _previous = EnableWindow(
                self.controls.get(Control::Button(DialogButton::Ok)),
                self.current_settings() != self.settings,
            );
        }
    }

    fn accept(&mut self) {
        self.settings = self.current_settings();
        self.accepted = true;
    }

    fn cancel(&mut self) {
        self.accepted = false;
    }
}

struct DialogFonts {
    body: OwnedFont,
    heading: OwnedFont,
    sidebar_title: OwnedFont,
    strong: OwnedFont,
    help: OwnedFont,
    section: OwnedFont,
    button: OwnedFont,
}

impl DialogFonts {
    fn create(dpi: u32) -> Result<Self> {
        Ok(Self {
            body: OwnedFont::pixels(dpi, 14, FW_NORMAL.0.cast_signed())?,
            heading: OwnedFont::pixels(dpi, 22, FW_SEMIBOLD.0.cast_signed())?,
            sidebar_title: OwnedFont::pixels(dpi, 21, FW_SEMIBOLD.0.cast_signed())?,
            strong: OwnedFont::pixels(dpi, 14, FW_SEMIBOLD.0.cast_signed())?,
            help: OwnedFont::pixels(dpi, 12, FW_NORMAL.0.cast_signed())?,
            section: OwnedFont::pixels(dpi, 12, FW_SEMIBOLD.0.cast_signed())?,
            button: OwnedFont::pixels(dpi, 13, FW_NORMAL.0.cast_signed())?,
        })
    }

    const fn of(&self, control: Control) -> HFONT {
        match control {
            Control::SidebarTitle => self.sidebar_title.0,
            Control::PageHeading => self.heading.0,
            Control::PageIntro => self.help.0,
            Control::Section(_) => self.section.0,
            Control::Button(_) => self.button.0,
            _ => self.body.0,
        }
    }
}

fn register_class(instance: HINSTANCE) -> Result<()> {
    let icon = app_icon::load_app(instance, IconColor::Azure)?;
    let background = HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void);
    dialog_host::register_class::<DialogState>(instance, icon, background)
}

fn owner_dpi(owner: HWND) -> u32 {
    known_dpi(unsafe {
        // SAFETY: owner is the live application window used for this modal dialog.
        GetDpiForWindow(owner)
    })
}

fn handle_settings_message(
    state: &mut DialogState,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_COMMAND => handle_command(hwnd, state, wparam),
        WM_SIZE if state.fonts.is_some() && wparam.0 != 1 => {
            report_layout(state.resize());
            Some(LRESULT(0))
        }
        WM_SETTINGS_FOCUS => {
            report_layout(state.reveal_control(HWND(wparam.0 as *mut c_void)));
            Some(LRESULT(0))
        }
        WM_SETTINGS_NAVIGATE => {
            if let Some(category) = Category::ALL.get(wparam.0) {
                report_layout(state.focus_category(*category));
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            report_layout(state.scroll_wheel(i32::from(high_word(wparam.0).cast_signed())));
            Some(LRESULT(0))
        }
        WM_VSCROLL => {
            report_layout(state.scroll_bar(low_word(wparam.0)));
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            // SAFETY: WM_GETMINMAXINFO supplies writable storage for the synchronous callback.
            if let Some(info) = unsafe { (lparam.0 as *mut MINMAXINFO).as_mut() } {
                match FRAME.window_size(
                    Size::new(
                        alttabio::dialog_layout::scale(MIN_CLIENT_WIDTH, state.dpi),
                        alttabio::dialog_layout::scale(MIN_CLIENT_HEIGHT, state.dpi),
                    ),
                    state.dpi,
                ) {
                    Ok(size) => {
                        info.ptMinTrackSize.x = size.width;
                        info.ptMinTrackSize.y = size.height;
                    }
                    Err(error) => {
                        eprintln!("Could not determine the minimum settings size: {error}");
                    }
                }
            }
            Some(LRESULT(0))
        }
        WM_DRAWITEM => {
            let item = unsafe {
                // SAFETY: WM_DRAWITEM supplies a DRAWITEMSTRUCT valid for this synchronous
                // callback. It is borrowed only until the handler returns.
                (lparam.0 as *const DRAWITEMSTRUCT).as_ref()
            }?;
            selectors::paint_list_item(item, state)
        }
        WM_DPICHANGED => {
            dialog_host::handle_dpi_changed(hwnd, wparam, lparam, FRAME.name, |dpi| {
                state.update_dpi(dpi)
            });
            Some(LRESULT(0))
        }
        WM_ERASEBKGND | WM_PRINTCLIENT => {
            if let Err(error) = paint::paint_window_background(HDC(wparam.0 as *mut c_void), state)
            {
                eprintln!("Could not paint the settings background: {error}");
            }
            Some(LRESULT(1))
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
            state.background.as_ref().map(|brush| {
                theme::style_control_dc(
                    HDC(wparam.0 as *mut c_void),
                    state.background_color,
                    state.text_color,
                    brush.0,
                )
            })
        }
        WM_CLOSE => {
            state.cancel();
            request_dialog_close(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn handle_command(hwnd: HWND, state: &mut DialogState, wparam: WPARAM) -> Option<LRESULT> {
    let control = Control::with_id(usize::from(low_word(wparam.0)))?;
    let notification = u32::from(high_word(wparam.0));
    match control {
        Control::Navigation(category) if notification == BN_CLICKED => {
            report_layout(state.select_category(category));
        }
        Control::Button(DialogButton::Ok) => {
            if state.current_settings() == state.settings {
                return Some(LRESULT(0));
            }
            state.accept();
            request_dialog_close(hwnd);
        }
        Control::Button(DialogButton::Cancel) => {
            state.cancel();
            request_dialog_close(hwnd);
        }
        Control::Selector(Selector::Theme) if notification == CBN_SELCHANGE => {
            state.apply_selected_theme();
        }
        Control::Selector(Selector::Icon) if notification == CBN_SELCHANGE => {
            state.apply_selected_icon();
        }
        Control::Checkbox(SettingOption::RightButtonWheelSwitching)
            if notification == BN_CLICKED =>
        {
            state.sync_right_button_release_enabled();
        }
        Control::Checkbox(_) if notification == BN_CLICKED => {}
        _ => return None,
    }
    state.sync_save_enabled();
    state.redraw();
    Some(LRESULT(0))
}

fn report_layout(result: Result<()>) {
    if let Err(error) = result {
        eprintln!("Could not update settings layout: {error}");
    }
}

fn request_dialog_close(hwnd: HWND) {
    if let Err(error) = dialog_host::request_close(hwnd) {
        eprintln!("Could not request settings closure: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::dialog_layout::{BASE_DPI, Point, Size};

    #[test]
    fn settings_window_requests_app_window_alt_tab_presence() {
        let app_window = windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW;

        assert_ne!(FRAME.ex_style.0 & app_window.0, 0);
    }

    #[test]
    fn settings_window_reports_its_caption() -> Result<()> {
        let instance = win32::module_instance()?;
        register_class(instance)?;
        let dialog = ModalDialog::create(
            instance,
            Point::default(),
            Size::new(100, 100),
            None,
            DialogState::new(Settings::default(), BASE_DPI, instance, None),
        )?;
        let window = dialog.window();
        let mut title = [0_u16; 64];
        let written = unsafe {
            // SAFETY: the dialog's hidden window is live and title is writable for the
            // synchronous query.
            windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(window, &mut title)
        };
        let title = String::from_utf16_lossy(
            title
                .get(..usize::try_from(written).unwrap_or_default())
                .unwrap_or_default(),
        );
        let destroyed = dialog.destroy().map(|_state| ());
        let still_exists = unsafe {
            // SAFETY: IsWindow only asks whether the handle still names a window.
            windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window))
        };

        assert_eq!(title, "AltTabio Settings");
        assert_eq!(destroyed, Ok(()), "destroying the Settings test window");
        assert!(
            !still_exists.as_bool(),
            "the Settings test window outlived its dialog"
        );
        Ok(())
    }
}
