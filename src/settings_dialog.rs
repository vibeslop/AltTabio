use crate::dialog_host::{
    self, DialogFrame, DialogHost, DialogWindow, Keyboard, ModalDialog, OwnerGuard, dark,
    high_word, low_word, rect_from_native, wide,
};
use crate::native_drawing::{
    DRAW_TEXT_CENTER, DRAW_TEXT_END_ELLIPSIS, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE,
    DRAW_TEXT_VCENTER, OwnedBrush, OwnedFont, draw_text_with_font, fill_color, frame_color,
    measure_text, rgb, system_color,
};
use crate::{
    app_icon,
    native_theme::{DarkModeApi, resolve_current_theme},
};
use alttabio::dialog_layout::{BASE_DPI, Point, Rect, hairline, scale};
use alttabio::settings::{IconColor, Settings, Theme};
use alttabio::settings_form::{
    Choices, Control, DialogButton, ICON_CHOICES, Selector, SettingOption, SettingsLayout,
    THEME_CHOICES, checkmark_points,
};
use alttabio::theme::ResolvedTheme;
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{
    COLORREF, E_FAIL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, DeleteObject, EndPaint, FW_NORMAL,
    FW_SEMIBOLD, FillRect, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN, InvalidateRect, PAINTSTRUCT,
    SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, IsWindowEnabled};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_GETSTATE, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE,
    CBS_DROPDOWNLIST, CBS_HASSTRINGS, CreateWindowExW, GetClientRect, HMENU, MoveWindow,
    SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLORDLG, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DPICHANGED, WM_ENABLE, WM_ERASEBKGND,
    WM_KILLFOCUS, WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_THEMECHANGED,
    WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_GROUP,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{BOOL, Error, HRESULT, PCWSTR, Result, w};

pub(crate) const WINDOW_CLASS_NAME: &str = "AltTabioRustSettings";
const FRAME: DialogFrame = DialogFrame {
    name: "settings",
    class: WINDOW_CLASS_NAME,
    title: "AltTabio Settings",
    style: WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0),
    ex_style: WINDOW_EX_STYLE(WS_EX_DLGMODALFRAME.0 | WS_EX_CONTROLPARENT.0 | WS_EX_APPWINDOW.0),
};
const SETTINGS_CONTROL_SUBCLASS_ID: usize = 1;
const BUTTON_STATE_PUSHED: usize = 0x0004;
const SOLID_PEN: i32 = 0;

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn SetWindowTheme(hwnd: HWND, sub_app_name: PCWSTR, sub_id_list: PCWSTR) -> HRESULT;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetComboBoxInfo(hwnd: HWND, info: *mut NativeComboBoxInfo) -> i32;
}

type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize, data: usize) -> BOOL;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    fn RemoveWindowSubclass(hwnd: HWND, proc: Option<SubclassProc>, id: usize) -> BOOL;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreatePen(style: i32, width: i32, color: COLORREF) -> HPEN;
    fn SelectObject(dc: HDC, object: HGDIOBJ) -> HGDIOBJ;
    fn MoveToEx(dc: HDC, x: i32, y: i32, previous: *mut c_void) -> BOOL;
    fn LineTo(dc: HDC, x: i32, y: i32) -> BOOL;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeThemeClass {
    Explorer,
    DarkModeExplorer,
    Cfd,
}

impl NativeThemeClass {
    const fn name(self) -> PCWSTR {
        match self {
            Self::Explorer => w!("Explorer"),
            Self::DarkModeExplorer => w!("DarkMode_Explorer"),
            Self::Cfd => w!("CFD"),
        }
    }
}

#[derive(Clone, Copy)]
struct ThemeTarget {
    hwnd: HWND,
    kind: ThemeTargetKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThemeTargetKind {
    Standard,
    ComboBox,
    ComboList,
}

const fn native_theme_class(dark: bool, kind: ThemeTargetKind) -> NativeThemeClass {
    match kind {
        ThemeTargetKind::ComboBox => NativeThemeClass::Cfd,
        ThemeTargetKind::ComboList if dark => NativeThemeClass::DarkModeExplorer,
        ThemeTargetKind::Standard | ThemeTargetKind::ComboList => NativeThemeClass::Explorer,
    }
}

#[repr(C)]
struct NativeComboBoxInfo {
    size: u32,
    item_rect: RECT,
    button_rect: RECT,
    button_state: u32,
    combo: HWND,
    item: HWND,
    list: HWND,
}

pub fn show(owner: HWND, settings: &Settings) -> Result<Option<Settings>> {
    let instance = dialog_host::module_instance()?;
    register_class(instance)?;
    let dpi = owner_dpi(owner);
    let layout = SettingsLayout::for_dpi(dpi);
    let window_size = FRAME.window_size(layout.client, dpi)?;
    let window_origin = dialog_host::work_area_near_window(owner)?.centered(window_size);
    let initial_dark = resolve_current_theme(settings.appearance.theme) == ResolvedTheme::Dark;
    let dark_mode_api = match DarkModeApi::load(initial_dark) {
        Ok(api) => Some(api),
        Err(error) => {
            eprintln!("Native dark settings controls are unavailable: {error}");
            None
        }
    };
    let _owner_guard = OwnerGuard::disable(owner);
    let dialog = ModalDialog::create(
        instance,
        window_origin,
        window_size,
        settings_window_owner(owner),
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
        .get(Control::Button(DialogButton::Ok));
    if let Err(error) = dialog_host::focus(ok_button) {
        eprintln!("Could not focus the settings OK button: {error}");
    }
    let Some(state) = dialog.run(Keyboard::DialogNavigation)? else {
        return Ok(None);
    };
    Ok(state.accepted.then_some(state.settings))
}

fn settings_window_owner(_modal_controller: HWND) -> Option<HWND> {
    // Native ownership would promote Settings into the topmost overlay's Z-order band. OwnerGuard
    // supplies the required modality without that relationship.
    None
}

/// The window of each entry of `Control::all`, in creation order.
#[derive(Default)]
struct DialogControls(Vec<(Control, HWND)>);

impl DialogControls {
    fn create(
        parent: HWND,
        instance: HINSTANCE,
        layout: &SettingsLayout,
        dpi: u32,
        fonts: &DialogFonts,
        settings: &Settings,
    ) -> Result<Self> {
        Control::all()
            .map(|control| {
                let rect = control.window_rect(layout, dpi);
                create_child(parent, instance, control, rect, fonts.of(control), settings)
                    .map(|hwnd| (control, hwnd))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    /// The window of `control`, or a null handle before the controls exist.
    fn get(&self, control: Control) -> HWND {
        self.0
            .iter()
            .find(|(candidate, _)| *candidate == control)
            .map_or_else(HWND::default, |(_, hwnd)| *hwnd)
    }

    fn find(&self, hwnd: HWND) -> Option<Control> {
        self.0
            .iter()
            .find(|(_, candidate)| *candidate == hwnd)
            .map(|(control, _)| *control)
    }

    fn checkbox(&self, option: SettingOption) -> HWND {
        self.get(Control::Checkbox(option))
    }

    fn apply_layout(&self, layout: &SettingsLayout, dpi: u32) -> Result<()> {
        for &(control, hwnd) in &self.0 {
            move_control(hwnd, control.window_rect(layout, dpi))?;
        }
        Ok(())
    }

    fn apply_fonts(&self, fonts: &DialogFonts) {
        for &(control, hwnd) in &self.0 {
            set_control_font(hwnd, fonts.of(control));
        }
    }

    fn theme_targets(&self) -> impl Iterator<Item = ThemeTarget> + '_ {
        self.0.iter().map(|&(control, hwnd)| ThemeTarget {
            hwnd,
            kind: if let Control::Selector(_) = control {
                ThemeTargetKind::ComboBox
            } else {
                ThemeTargetKind::Standard
            },
        })
    }

    fn custom_paint_targets(&self) -> impl Iterator<Item = HWND> + '_ {
        // Static labels already take the palette's colors through WM_CTLCOLORSTATIC.
        self.0
            .iter()
            .filter(|(control, _)| !matches!(control, Control::Label(_)))
            .map(|(_, hwnd)| *hwnd)
    }
}

struct DialogState {
    hwnd: HWND,
    controls: DialogControls,
    fonts: Option<DialogFonts>,
    background: Option<OwnedBrush>,
    background_color: COLORREF,
    text_color: COLORREF,
    palette: ThemePalette,
    dark_mode_api: Option<DarkModeApi>,
    settings: Settings,
    instance: HINSTANCE,
    dpi: u32,
    accepted: bool,
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
            background: None,
            background_color: system_color(COLOR_WINDOW),
            text_color: system_color(COLOR_WINDOWTEXT),
            palette: ThemePalette::new(false),
            dark_mode_api,
            settings,
            instance,
            dpi,
            accepted: false,
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
        self.controls =
            DialogControls::create(parent, instance, &layout, self.dpi, &fonts, &self.settings)?;
        self.fonts = Some(fonts);
        install_custom_control_painting(&self.controls, host)?;
        self.sync_right_button_release_enabled();
        self.apply_selected_icon();
        self.apply_selected_theme();
        Ok(())
    }

    fn update_dpi(&mut self, dpi: u32) -> Result<()> {
        let layout = SettingsLayout::for_dpi(dpi);
        let fonts = DialogFonts::create(dpi)?;
        self.controls.apply_layout(&layout, dpi)?;
        self.controls.apply_fonts(&fonts);
        self.fonts = Some(fonts);
        self.dpi = dpi;
        Ok(())
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

    fn selected_name(&self, selector: Selector) -> &'static str {
        match selector {
            Selector::Theme => self.selected_theme().as_ini_value(),
            Selector::Icon => self.selected_icon().as_ini_value(),
        }
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
        apply_native_theme_hooks(
            self.hwnd,
            self.controls.theme_targets(),
            dark,
            self.dark_mode_api.as_ref(),
        );
        let invalidated = unsafe {
            // SAFETY: hwnd is live and None invalidates its complete client area synchronously.
            InvalidateRect(Some(self.hwnd), None, true)
        };
        if !invalidated.as_bool() {
            eprintln!("Could not redraw settings after changing its theme");
        }
    }

    fn apply_selected_icon(&self) {
        if let Err(error) =
            app_icon::apply_to_window(self.hwnd, self.instance, self.selected_icon())
        {
            eprintln!("Could not preview the selected settings icon: {error}");
        }
    }

    fn accept(&mut self) {
        for option in SettingOption::ALL {
            option.write(
                &mut self.settings,
                is_checked(self.controls.checkbox(option)),
            );
        }
        self.settings.appearance.icon = self.selected_icon();
        self.settings.appearance.theme = self.selected_theme();
        self.accepted = true;
    }

    fn cancel(&mut self) {
        self.accepted = false;
    }
}

struct DialogFonts {
    body: OwnedFont,
    heading: OwnedFont,
}

impl DialogFonts {
    fn create(dpi: u32) -> Result<Self> {
        Ok(Self {
            body: OwnedFont::new(dpi, 9, FW_NORMAL.0.cast_signed(), false)?,
            heading: OwnedFont::new(dpi, 9, FW_SEMIBOLD.0.cast_signed(), false)?,
        })
    }

    const fn of(&self, control: Control) -> HFONT {
        if let Control::Group(_) = control {
            self.heading.0
        } else {
            self.body.0
        }
    }
}

#[derive(Clone, Copy)]
struct ThemePalette {
    background: COLORREF,
    text: COLORREF,
    disabled_text: COLORREF,
    control_surface: COLORREF,
    pressed_surface: COLORREF,
    control_border: COLORREF,
    accent: COLORREF,
    accent_text: COLORREF,
}

impl ThemePalette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: dark::BACKGROUND,
                text: dark::TEXT,
                disabled_text: rgb(145, 145, 145),
                control_surface: dark::CONTROL_SURFACE,
                pressed_surface: dark::PRESSED_SURFACE,
                control_border: dark::CONTROL_BORDER,
                accent: system_color(COLOR_HIGHLIGHT),
                accent_text: system_color(COLOR_HIGHLIGHTTEXT),
            }
        } else {
            Self {
                background: system_color(COLOR_WINDOW),
                text: system_color(COLOR_WINDOWTEXT),
                disabled_text: system_color(COLOR_GRAYTEXT),
                control_surface: system_color(COLOR_BTNFACE),
                pressed_surface: system_color(COLOR_BTNSHADOW),
                control_border: system_color(COLOR_BTNSHADOW),
                accent: system_color(COLOR_HIGHLIGHT),
                accent_text: system_color(COLOR_HIGHLIGHTTEXT),
            }
        }
    }

    const fn label_text(&self, enabled: bool) -> COLORREF {
        if enabled {
            self.text
        } else {
            self.disabled_text
        }
    }
}

fn apply_native_theme_hooks(
    window: HWND,
    controls: impl Iterator<Item = ThemeTarget>,
    dark: bool,
    dark_mode_api: Option<&DarkModeApi>,
) {
    if let Some(api) = dark_mode_api {
        api.allow_for_window(window, dark);
    }
    if let Err(error) = dialog_host::set_dark_title_bar(window, dark) {
        eprintln!("Could not apply the native settings title-bar theme: {error}");
    }

    for control in controls {
        apply_control_theme(control, dark, dark_mode_api);
        if control.kind == ThemeTargetKind::ComboBox {
            match combo_list_window(control.hwnd) {
                Ok(list) => apply_control_theme(
                    ThemeTarget {
                        hwnd: list,
                        kind: ThemeTargetKind::ComboList,
                    },
                    dark,
                    dark_mode_api,
                ),
                Err(error) => eprintln!("Could not theme the settings drop-down list: {error}"),
            }
        }
    }
}

fn apply_control_theme(control: ThemeTarget, dark: bool, dark_mode_api: Option<&DarkModeApi>) {
    let sub_app_name = native_theme_class(dark, control.kind).name();
    let theme_result = unsafe {
        // SAFETY: control is a live child HWND and both theme-name buffers are static.
        SetWindowTheme(control.hwnd, sub_app_name, PCWSTR::null()).ok()
    };
    if let Err(error) = theme_result {
        eprintln!("Could not apply the native settings control theme: {error}");
    }
    if let Some(api) = dark_mode_api {
        api.allow_for_window(control.hwnd, dark);
    }
    unsafe {
        // SAFETY: control is live and WM_THEMECHANGED has no pointer payload.
        SendMessageW(
            control.hwnd,
            WM_THEMECHANGED,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
}

fn combo_list_window(combo: HWND) -> Result<HWND> {
    let mut info = NativeComboBoxInfo {
        size: u32::try_from(size_of::<NativeComboBoxInfo>()).unwrap_or(u32::MAX),
        item_rect: RECT::default(),
        button_rect: RECT::default(),
        button_state: 0,
        combo: HWND::default(),
        item: HWND::default(),
        list: HWND::default(),
    };
    let result = unsafe {
        // SAFETY: combo is a live COMBOBOX HWND and info is a correctly sized writable structure.
        GetComboBoxInfo(combo, &raw mut info)
    };
    if result == 0 || info.list == HWND::default() {
        Err(Error::from_thread())
    } else {
        Ok(info.list)
    }
}

fn create_child(
    parent: HWND,
    instance: HINSTANCE,
    control: Control,
    rect: Rect,
    font: HFONT,
    settings: &Settings,
) -> Result<HWND> {
    let (class, style) = match control {
        Control::Group(_) => (w!("BUTTON"), WINDOW_STYLE(BS_GROUPBOX as u32)),
        Control::Label(_) => (w!("STATIC"), WINDOW_STYLE::default()),
        Control::Selector(_) => (
            w!("COMBOBOX"),
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS).cast_unsigned()),
        ),
        Control::Checkbox(option) => {
            let group_style = if option == SettingOption::Autostart {
                WS_GROUP
            } else {
                WINDOW_STYLE::default()
            };
            (
                w!("BUTTON"),
                WS_TABSTOP | group_style | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            )
        }
        Control::Button(button) => {
            let button_style = if button == DialogButton::Ok {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            };
            (
                w!("BUTTON"),
                WS_TABSTOP | WINDOW_STYLE(button_style.cast_unsigned()),
            )
        }
    };
    let hwnd = create_control(
        parent,
        instance,
        class,
        control.text(),
        WS_CHILD | WS_VISIBLE | style,
        control.id(),
        rect,
        font,
    )?;
    match control {
        Control::Checkbox(option) if option.read(settings) => unsafe {
            // SAFETY: hwnd is a live checkbox and BM_SETCHECK consumes scalar values only. It
            // always returns zero.
            SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(1)), Some(LPARAM(0)));
        },
        Control::Selector(selector) => fill_selector(hwnd, selector, settings)?,
        _ => {}
    }
    Ok(hwnd)
}

fn fill_selector(hwnd: HWND, selector: Selector, settings: &Settings) -> Result<()> {
    for entry in selector.entries() {
        let text = wide(entry);
        let result = unsafe {
            // SAFETY: hwnd is a live combo box and text remains valid throughout the synchronous
            // insertion.
            SendMessageW(
                hwnd,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            )
        };
        if i32::try_from(result.0).unwrap_or(CB_ERR) < 0 {
            return Err(Error::from_hresult(E_FAIL));
        }
    }
    let result = unsafe {
        // SAFETY: hwnd is a live combo box and the index refers to one of the inserted entries.
        SendMessageW(
            hwnd,
            CB_SETCURSEL,
            Some(WPARAM(selector.index(settings))),
            Some(LPARAM(0)),
        )
    };
    if i32::try_from(result.0).unwrap_or(CB_ERR) == CB_ERR {
        Err(Error::from_hresult(E_FAIL))
    } else {
        Ok(())
    }
}

/// The index of the entry selected in a combo box, or `None` while it has no selection.
fn selected_index(combo: HWND) -> Option<usize> {
    let selected = unsafe {
        // SAFETY: combo is a live drop-down-list control with scalar message payloads.
        SendMessageW(combo, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    let selected = i32::try_from(selected.0).unwrap_or(CB_ERR);
    (selected != CB_ERR).then(|| usize::try_from(selected).unwrap_or_default())
}

fn install_custom_control_painting(
    controls: &DialogControls,
    host: &DialogHost<DialogState>,
) -> Result<()> {
    for control in controls.custom_paint_targets() {
        let installed = unsafe {
            // SAFETY: every handle is a live child control and host is the ModalDialog allocation,
            // which stays allocated until after all children receive WM_NCDESTROY.
            SetWindowSubclass(
                control,
                Some(settings_control_subclass_proc),
                SETTINGS_CONTROL_SUBCLASS_ID,
                std::ptr::from_ref(host) as usize,
            )
        };
        if !installed.as_bool() {
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
            let removed = unsafe {
                // SAFETY: this callback is the installed subclass instance being removed before
                // the child HWND finishes destruction.
                RemoveWindowSubclass(
                    hwnd,
                    Some(settings_control_subclass_proc),
                    SETTINGS_CONTROL_SUBCLASS_ID,
                )
            };
            if !removed.as_bool() {
                eprintln!("Could not remove settings control painting during destruction");
            }
            return None;
        }
        let host = unsafe {
            // SAFETY: SetWindowSubclass stored the stable DialogHost pointer for every control.
            (host_pointer as *const DialogHost<DialogState>).as_ref()
        }?;
        let state = host.state()?;
        match message {
            WM_PAINT => Some(paint_control_message(hwnd, &state)),
            WM_PRINTCLIENT => {
                paint_settings_control(hwnd, HDC(wparam.0 as *mut c_void), &state);
                Some(LRESULT(0))
            }
            WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS => {
                let result = unsafe {
                    // SAFETY: the original control procedure receives the unchanged message.
                    DefSubclassProc(hwnd, message, wparam, lparam)
                };
                let invalidated = unsafe {
                    // SAFETY: hwnd is live for this callback and the complete control must redraw.
                    InvalidateRect(Some(hwnd), None, true)
                };
                if !invalidated.as_bool() {
                    eprintln!("Could not redraw a settings control after its state changed");
                }
                Some(result)
            }
            _ => None,
        }
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| unsafe {
        // SAFETY: all unhandled messages retain their original scalar payloads.
        DefSubclassProc(hwnd, message, wparam, lparam)
    })
}

fn paint_control_message(hwnd: HWND, state: &DialogState) -> LRESULT {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe {
        // SAFETY: hwnd is live during WM_PAINT and paint is a writable PAINTSTRUCT.
        BeginPaint(hwnd, &raw mut paint)
    };
    if dc == HDC::default() {
        eprintln!("Could not begin painting a settings control");
    } else {
        paint_settings_control(hwnd, dc, state);
    }
    let ended = unsafe {
        // SAFETY: paint was initialized by BeginPaint for this hwnd.
        EndPaint(hwnd, &raw const paint)
    };
    if !ended.as_bool() {
        eprintln!("Could not finish painting a settings control");
    }
    LRESULT(0)
}

fn paint_settings_control(hwnd: HWND, dc: HDC, state: &DialogState) {
    let mut client = RECT::default();
    let client_result = unsafe {
        // SAFETY: hwnd is a live child control and client is writable.
        GetClientRect(hwnd, &raw mut client)
    };
    if let Err(error) = client_result {
        eprintln!("Could not read settings control bounds for painting: {error}");
        return;
    }
    let result = match state.controls.find(hwnd) {
        Some(control @ Control::Group(_)) => paint_group_box(dc, client, control.text(), state),
        Some(control @ Control::Checkbox(_)) => paint_checkbox(
            dc,
            client,
            control.text(),
            is_checked(hwnd),
            is_control_enabled(hwnd),
            state,
        ),
        Some(Control::Selector(selector)) => {
            paint_combo_box(dc, client, state.selected_name(selector), state)
        }
        Some(control @ Control::Button(_)) => {
            paint_push_button(dc, client, control.text(), hwnd, state)
        }
        Some(Control::Label(_)) | None => Ok(()),
    };
    if let Err(error) = result {
        eprintln!("Could not paint a settings control: {error}");
    }
}

fn paint_group_box(dc: HDC, client: RECT, label: &str, state: &DialogState) -> Result<()> {
    fill_color(dc, client, state.palette.background)?;
    let border_top = client.top.saturating_add(scale(8, state.dpi));
    let border = RECT {
        top: border_top,
        ..client
    };
    frame_color(
        dc,
        border,
        state.palette.control_border,
        hairline(state.dpi),
    )?;

    let Some(fonts) = state.fonts.as_ref() else {
        return Err(Error::from_hresult(E_FAIL));
    };
    let text_size = measure_text(dc, label, fonts.heading.0)?;
    let horizontal_padding = scale(5, state.dpi);
    let label_left = client.left.saturating_add(scale(7, state.dpi));
    let label_background = RECT {
        left: label_left,
        top: client.top,
        right: label_left
            .saturating_add(text_size.cx)
            .saturating_add(horizontal_padding.saturating_mul(2)),
        bottom: client
            .top
            .saturating_add(text_size.cy.max(scale(18, state.dpi))),
    };
    fill_color(dc, label_background, state.palette.background)?;
    let mut text_rect = label_background;
    text_rect.left = text_rect.left.saturating_add(horizontal_padding);
    text_rect.right = text_rect.right.saturating_sub(horizontal_padding);
    draw_text_with_font(
        dc,
        label,
        text_rect,
        state.palette.text,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        fonts.heading.0,
    )
}

fn paint_checkbox(
    dc: HDC,
    client: RECT,
    label: &str,
    checked: bool,
    enabled: bool,
    state: &DialogState,
) -> Result<()> {
    fill_color(dc, client, state.palette.background)?;
    let size = scale(14, state.dpi).min(client.bottom.saturating_sub(client.top));
    let top = client.top.saturating_add(
        client
            .bottom
            .saturating_sub(client.top)
            .saturating_sub(size)
            / 2,
    );
    let checkbox = RECT {
        left: client.left,
        top,
        right: client.left.saturating_add(size),
        bottom: top.saturating_add(size),
    };
    let surface = if checked {
        state.palette.accent
    } else {
        state.palette.control_surface
    };
    fill_color(dc, checkbox, surface)?;
    frame_color(
        dc,
        checkbox,
        if checked {
            state.palette.accent
        } else {
            state.palette.control_border
        },
        hairline(state.dpi),
    )?;
    if checked {
        draw_checkmark(dc, checkbox, state.palette.accent_text, state.dpi)?;
    }
    let mut text_rect = client;
    text_rect.left = checkbox.right.saturating_add(scale(8, state.dpi));
    draw_text(
        dc,
        label,
        text_rect,
        state.palette.label_text(enabled),
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        state,
    )
}

fn paint_push_button(
    dc: HDC,
    client: RECT,
    label: &str,
    hwnd: HWND,
    state: &DialogState,
) -> Result<()> {
    let enabled = is_control_enabled(hwnd);
    let button_state = unsafe {
        // SAFETY: hwnd is a live BUTTON control and BM_GETSTATE has no pointer payload.
        SendMessageW(hwnd, BM_GETSTATE, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    let pressed = button_state.0.cast_unsigned() & BUTTON_STATE_PUSHED != 0;
    fill_color(
        dc,
        client,
        if pressed {
            state.palette.pressed_surface
        } else {
            state.palette.control_surface
        },
    )?;
    let focused = unsafe {
        // SAFETY: GetFocus has no preconditions and returns a borrowed HWND.
        GetFocus()
    } == hwnd;
    frame_color(
        dc,
        client,
        if focused {
            state.palette.accent
        } else {
            state.palette.control_border
        },
        scale(if focused { 2 } else { 1 }, state.dpi).max(1),
    )?;
    draw_text(
        dc,
        label,
        client,
        state.palette.label_text(enabled),
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state,
    )
}

fn paint_combo_box(dc: HDC, client: RECT, label: &str, state: &DialogState) -> Result<()> {
    fill_color(dc, client, state.palette.control_surface)?;
    let border = hairline(state.dpi);
    frame_color(dc, client, state.palette.control_border, border)?;
    let button_width = scale(28, state.dpi).min(client.right.saturating_sub(client.left));
    let button_left = client.right.saturating_sub(button_width);
    let separator = RECT {
        left: button_left,
        top: client.top.saturating_add(border),
        right: button_left.saturating_add(border),
        bottom: client.bottom.saturating_sub(border),
    };
    fill_color(dc, separator, state.palette.control_border)?;
    let padding = scale(8, state.dpi);
    let mut text_rect = client;
    text_rect.left = text_rect.left.saturating_add(padding);
    text_rect.right = button_left.saturating_sub(padding);
    draw_text(
        dc,
        label,
        text_rect,
        state.palette.text,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        state,
    )?;
    let center_x = button_left.saturating_add(button_width / 2);
    let center_y = client
        .top
        .saturating_add(client.bottom.saturating_sub(client.top) / 2);
    let half_width = scale(4, state.dpi);
    let half_height = scale(2, state.dpi);
    draw_polyline(
        dc,
        &[
            Point {
                x: center_x.saturating_sub(half_width),
                y: center_y.saturating_sub(half_height),
            },
            Point {
                x: center_x,
                y: center_y.saturating_add(half_height),
            },
            Point {
                x: center_x.saturating_add(half_width),
                y: center_y.saturating_sub(half_height),
            },
        ],
        state.palette.text,
        hairline(state.dpi),
    )
}

fn is_control_enabled(hwnd: HWND) -> bool {
    unsafe {
        // SAFETY: hwnd is a live child control.
        IsWindowEnabled(hwnd).as_bool()
    }
}

fn draw_text(
    dc: HDC,
    label: &str,
    rect: RECT,
    color: COLORREF,
    format: u32,
    state: &DialogState,
) -> Result<()> {
    let Some(fonts) = state.fonts.as_ref() else {
        return Err(Error::from_hresult(E_FAIL));
    };
    draw_text_with_font(dc, label, rect, color, format, fonts.body.0)
}

fn draw_checkmark(dc: HDC, rect: RECT, color: COLORREF, dpi: u32) -> Result<()> {
    let points = checkmark_points(rect_from_native(rect));
    draw_polyline(dc, &points, color, scale(2, dpi).max(2))
}

fn draw_polyline(dc: HDC, points: &[Point], color: COLORREF, width: i32) -> Result<()> {
    let Some((first, remaining)) = points.split_first() else {
        return Ok(());
    };
    let pen = unsafe {
        // SAFETY: scalar parameters describe a solid GDI pen.
        CreatePen(SOLID_PEN, width, color)
    };
    if pen == HPEN::default() {
        return Err(Error::from_thread());
    }
    let previous = unsafe {
        // SAFETY: dc is live and pen remains owned through the complete drawing operation.
        SelectObject(dc, HGDIOBJ(pen.0))
    };
    let mut success = previous != HGDIOBJ::default();
    if success {
        success = unsafe {
            // SAFETY: dc is live and the previous-point output is intentionally unused.
            MoveToEx(dc, first.x, first.y, std::ptr::null_mut()).as_bool()
        };
        for point in remaining {
            success &= unsafe {
                // SAFETY: dc remains live with the owned pen selected.
                LineTo(dc, point.x, point.y).as_bool()
            };
        }
        let restored = unsafe {
            // SAFETY: previous is the object returned by SelectObject for this dc.
            SelectObject(dc, previous)
        };
        if restored == HGDIOBJ::default() {
            eprintln!("Could not restore the settings drawing pen");
        }
    }
    let deleted = unsafe {
        // SAFETY: pen is uniquely owned and no longer selected into dc.
        DeleteObject(HGDIOBJ(pen.0))
    };
    if !deleted.as_bool() {
        eprintln!("Could not release a settings drawing pen");
    }
    if success {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "parameters map directly to one CreateWindowExW child-control call"
)]
fn create_control(
    parent: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    label: &str,
    style: WINDOW_STYLE,
    id: Option<usize>,
    rect: Rect,
    font: HFONT,
) -> Result<HWND> {
    let text = wide(label);
    let menu = id.map(|value| HMENU(value as *mut c_void));
    let control = unsafe {
        // SAFETY: class/text buffers remain live for the synchronous creation call; parent and
        // instance are live and the HMENU value is a documented child-control identifier.
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(text.as_ptr()),
            style,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            Some(parent),
            menu,
            Some(instance),
            None,
        )
    }?;
    set_control_font(control, font);
    Ok(control)
}

fn set_control_font(control: HWND, font: HFONT) {
    unsafe {
        // SAFETY: control is live and WM_SETFONT borrows the dialog-owned font handle.
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

fn move_control(control: HWND, rect: Rect) -> Result<()> {
    unsafe {
        // SAFETY: control is a live child HWND and rect contains bounded, DPI-scaled coordinates.
        MoveWindow(control, rect.x, rect.y, rect.width, rect.height, true)
    }
}

fn is_checked(control: HWND) -> bool {
    let result = unsafe {
        // SAFETY: control is a live checkbox and BM_GETCHECK has no pointer payload.
        SendMessageW(control, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    result.0 == 1
}

fn register_class(instance: HINSTANCE) -> Result<()> {
    let icon = app_icon::load_app(instance, IconColor::Azure)?;
    let background = HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void);
    dialog_host::register_class::<DialogState>(instance, icon, background)
}

fn owner_dpi(owner: HWND) -> u32 {
    let dpi = unsafe {
        // SAFETY: owner is the live application window used for this modal dialog.
        GetDpiForWindow(owner)
    };
    if dpi == 0 { BASE_DPI } else { dpi }
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
        WM_DPICHANGED => {
            dialog_host::handle_dpi_changed(hwnd, wparam, lparam, FRAME.name, |dpi| {
                state.update_dpi(dpi)
            });
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => state
            .background
            .as_ref()
            .map(|brush| paint_background(hwnd, HDC(wparam.0 as *mut c_void), brush.0)),
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORLISTBOX => {
            state.background.as_ref().map(|brush| {
                style_control_dc(
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
        Control::Button(DialogButton::Ok) => {
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
        _ => return None,
    }
    Some(LRESULT(0))
}

fn request_dialog_close(hwnd: HWND) {
    if let Err(error) = dialog_host::request_close(hwnd) {
        eprintln!("Could not request settings closure: {error}");
    }
}

fn paint_background(hwnd: HWND, dc: HDC, brush: HBRUSH) -> LRESULT {
    let mut client = RECT::default();
    let client_result = unsafe {
        // SAFETY: hwnd and dc are the live handles supplied for WM_ERASEBKGND; client is writable.
        GetClientRect(hwnd, &raw mut client)
    };
    if let Err(error) = client_result {
        eprintln!("Could not read the settings client area for painting: {error}");
        return LRESULT(0);
    }
    let filled = unsafe {
        // SAFETY: dc is valid for this paint callback, client is initialized, and brush is owned by
        // the dialog state for the complete synchronous call.
        FillRect(dc, &raw const client, brush)
    };
    if filled == 0 {
        eprintln!("Could not paint the settings background");
        LRESULT(0)
    } else {
        LRESULT(1)
    }
}

fn style_control_dc(
    dc: HDC,
    background_color: COLORREF,
    text_color: COLORREF,
    brush: HBRUSH,
) -> LRESULT {
    let previous_mode = unsafe {
        // SAFETY: dc is the live control paint context supplied by the current WM_CTLCOLOR message.
        SetBkMode(dc, TRANSPARENT)
    };
    if previous_mode == 0 {
        eprintln!("Could not make a settings control background transparent");
    }
    let previous_background = unsafe {
        // SAFETY: dc remains valid and brush color is represented by this COLORREF.
        SetBkColor(dc, background_color)
    };
    if previous_background.0 == u32::MAX {
        eprintln!("Could not set a settings control background color");
    }
    let previous_text = unsafe {
        // SAFETY: dc remains valid and text_color is a scalar COLORREF.
        SetTextColor(dc, text_color)
    };
    if previous_text.0 == u32::MAX {
        eprintln!("Could not set a settings control text color");
    }
    LRESULT(brush.0 as isize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::dialog_layout::Size;
    use alttabio::settings_form::Group;

    #[test]
    fn settings_window_is_not_owned_by_the_topmost_overlay() {
        let mut owner_storage = 0_u8;
        let owner = HWND(std::ptr::from_mut(&mut owner_storage).cast());

        assert_eq!(settings_window_owner(owner), None);
    }

    #[test]
    fn settings_window_requests_app_window_alt_tab_presence() {
        let app_window = windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW;

        assert_ne!(FRAME.ex_style.0 & app_window.0, 0);
    }

    #[test]
    fn settings_window_reports_its_caption() -> Result<()> {
        let instance = dialog_host::module_instance()?;
        register_class(instance)?;
        let dialog = ModalDialog::create(
            instance,
            Point::default(),
            Size::new(100, 100),
            None,
            DialogState::new(Settings::default(), BASE_DPI, instance, None),
        )?;
        let mut title = [0_u16; 64];
        let written = unsafe {
            // SAFETY: the dialog's hidden window is live and title is writable for the
            // synchronous query.
            windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(dialog.window(), &mut title)
        };
        let title = String::from_utf16_lossy(
            title
                .get(..usize::try_from(written).unwrap_or_default())
                .unwrap_or_default(),
        );

        assert_eq!(title, "AltTabio Settings");
        Ok(())
    }

    #[test]
    fn dark_native_controls_use_their_supported_theme_classes() {
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::Standard),
            NativeThemeClass::Explorer
        );
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::ComboBox),
            NativeThemeClass::Cfd
        );
        assert_eq!(
            native_theme_class(true, ThemeTargetKind::ComboList),
            NativeThemeClass::DarkModeExplorer
        );
    }

    #[test]
    fn dark_interactive_control_surfaces_are_not_light() {
        let palette = ThemePalette::new(true);

        for color in [
            palette.control_surface,
            palette.pressed_surface,
            palette.control_border,
        ] {
            let red = color.0 & 0xff;
            let green = color.0 >> 8 & 0xff;
            let blue = color.0 >> 16 & 0xff;
            assert!(red < 160 && green < 160 && blue < 160);
        }
    }

    #[test]
    fn group_headers_are_custom_paint_targets_with_stable_labels() {
        let mut storage = [0_u8; 32];
        let controls = DialogControls(
            Control::all()
                .zip(storage.iter_mut())
                .map(|(control, byte)| (control, HWND(std::ptr::from_mut(byte).cast())))
                .collect(),
        );
        let targets = controls.custom_paint_targets().collect::<Vec<_>>();

        for (group, title) in [
            (Group::General, "General"),
            (Group::Appearance, "Appearance"),
            (Group::Monitor, "Monitor"),
        ] {
            let hwnd = controls.get(Control::Group(group));
            assert_eq!(controls.find(hwnd).map(Control::text), Some(title));
            assert!(targets.contains(&hwnd), "{group:?}");
        }
    }
}
