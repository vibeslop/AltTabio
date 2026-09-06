use crate::native_drawing::{
    DRAW_TEXT_CENTER, DRAW_TEXT_END_ELLIPSIS, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE,
    DRAW_TEXT_VCENTER, OwnedBrush, OwnedFont, draw_text_with_font, fill_color, frame_color,
    measure_text, rgb, system_color,
};
use crate::{
    app_icon,
    native_theme::{DarkModeApi, resolve_current_theme},
};
use alttabio::settings::{IconColor, Settings, Theme};
use alttabio::theme::ResolvedTheme;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, DeleteObject, EndPaint, FW_NORMAL,
    FW_SEMIBOLD, FillRect, GetMonitorInfoW, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, PAINTSTRUCT, SetBkColor, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, IsWindowEnabled, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_GETSTATE, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING, CB_ERR, CB_SETCURSEL, CBN_SELCHANGE,
    CBS_DROPDOWNLIST, CBS_HASSTRINGS, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW,
    GetWindowLongPtrW, HMENU, IDC_ARROW, IsDialogMessageW, LoadCursorW, MSG, MoveWindow,
    PostMessageW, PostQuitMessage, RegisterClassExW, SW_SHOW, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLORDLG, WM_CTLCOLORLISTBOX,
    WM_CTLCOLORSTATIC, WM_DPICHANGED, WM_ENABLE, WM_ERASEBKGND, WM_KILLFOCUS, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_THEMECHANGED, WNDCLASSEXW,
    WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_GROUP,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{BOOL, Error, HRESULT, PCWSTR, Result, w};

pub(crate) const WINDOW_CLASS_NAME: &str = "AltTabioRustSettings";
const WINDOW_CLASS: PCWSTR = w!("AltTabioRustSettings");
const WINDOW_TITLE: PCWSTR = w!("AltTabio Settings");
const BASE_DPI: u32 = 96;
const OPTION_COUNT: usize = 16;
const GENERAL_OPTION_COUNT: usize = 8;
const APPEARANCE_OPTION_COUNT: usize = 7;
const OK_ID: usize = 1;
const CANCEL_ID: usize = 2;
const OPTION_ID_BASE: usize = 100;
const THEME_ID: usize = 200;
const ICON_ID: usize = 201;
const CLIENT_WIDTH: i32 = 560;
const CLIENT_HEIGHT: i32 = 747;
const APPEARANCE_SELECTOR_WIDTH: i32 = 180;
const WINDOW_STYLE_VALUE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0);
const WINDOW_EX_STYLE_VALUE: WINDOW_EX_STYLE =
    WINDOW_EX_STYLE(WS_EX_DLGMODALFRAME.0 | WS_EX_CONTROLPARENT.0 | WS_EX_APPWINDOW.0);
const SETTINGS_CONTROL_SUBCLASS_ID: usize = 1;
const BUTTON_STATE_PUSHED: usize = 0x0004;
const SOLID_PEN: i32 = 0;
const WM_DESTROY_DIALOG: u32 = WM_APP + 21;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OptionGroup {
    General,
    Appearance,
    Monitor,
}

// Declare each identity, field, label and placement together. Both directions of the
// binding are generated from the same field, so reads and writes cannot drift apart.
macro_rules! setting_options {
    ($( $name:ident: $group:ident, $row:literal, $section:ident.$field:ident, $label:literal; )+) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        enum SettingOption { $( $name, )+ }

        impl SettingOption {
            const ALL: [Self; OPTION_COUNT] = [$( Self::$name, )+];

            const fn label(self) -> &'static str {
                match self { $( Self::$name => $label, )+ }
            }

            const fn group(self) -> OptionGroup {
                match self { $( Self::$name => OptionGroup::$group, )+ }
            }

            fn rect(self, layout: &DialogLayout) -> ControlRect {
                let row = match self { $( Self::$name => $row, )+ };
                match self.group() {
                    OptionGroup::General => layout.general_options[row],
                    OptionGroup::Appearance => layout.appearance_options[row],
                    OptionGroup::Monitor => layout.monitor_option,
                }
            }

            fn read(self, settings: &Settings) -> bool {
                match self { $( Self::$name => settings.$section.$field, )+ }
            }

            fn write(self, settings: &mut Settings, value: bool) {
                match self { $( Self::$name => settings.$section.$field = value, )+ }
            }

            const fn control_id(self) -> usize { OPTION_ID_BASE + self as usize }
        }
    };
}

setting_options! {
    Autostart: General, 0, general.autostart, "Start AltTabio when I sign in";
    ReplaceAltTab: General, 1, general.replace_alt_tab, "Replace Alt+Tab";
    ReplaceWinTab: General, 2, general.replace_win_tab, "Replace Win+Tab";
    TypedSearch: General, 3, general.typed_search, "Enable typing to search tasks";
    ReleaseAltSwitches: General, 4, general.release_alt_switches, "Switch when Alt is released";
    ReleaseRightButtonSwitches: General, 5, general.release_right_button_switches, "Activate the selected task when the right mouse button is released";
    RightButtonWheelSwitching: General, 6, general.right_button_wheel_switching, "Use right mouse button + wheel switching";
    MouseOverSelection: General, 7, general.mouse_over_selection, "Select tasks when the mouse moves over them";
    CompactList: Appearance, 0, appearance.compact_list, "Use a compact task list";
    LargeIcons: Appearance, 1, appearance.large_icons, "Use large icons";
    ShowNumbers: Appearance, 2, appearance.show_numbers, "Show number shortcuts";
    ShowAppNames: Appearance, 3, appearance.show_app_names, "Show app names under titles";
    VisibleBorders: Appearance, 4, appearance.visible_borders, "Visible borders";
    Preview: Appearance, 5, appearance.preview, "Show a live preview";
    FullDesktopPreview: Appearance, 6, appearance.full_desktop_preview, "Show the window in its position on the desktop";
    CurrentMonitorFilter: Monitor, 0, monitor.use_current_monitor_filter, "Only show tasks from the current monitor";
}
const THEME_LABELS: [&str; 3] = ["Auto", "Light", "Dark"];
const ICON_LABELS: [&str; 8] = [
    "Azure",
    "Copper",
    "Ember",
    "Indigo",
    "Orchid",
    "Rosewood",
    "Vermilion",
    "Violet",
];

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

const fn theme_selector_index(theme: Theme) -> usize {
    match theme {
        Theme::Auto => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}

const fn theme_from_selector_index(index: usize) -> Theme {
    match index {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::Auto,
    }
}

const fn icon_selector_index(icon: IconColor) -> usize {
    match icon {
        IconColor::Azure => 0,
        IconColor::Copper => 1,
        IconColor::Ember => 2,
        IconColor::Indigo => 3,
        IconColor::Orchid => 4,
        IconColor::Rosewood => 5,
        IconColor::Vermilion => 6,
        IconColor::Violet => 7,
    }
}

const fn icon_from_selector_index(index: usize) -> IconColor {
    match index {
        1 => IconColor::Copper,
        2 => IconColor::Ember,
        3 => IconColor::Indigo,
        4 => IconColor::Orchid,
        5 => IconColor::Rosewood,
        6 => IconColor::Vermilion,
        7 => IconColor::Violet,
        _ => IconColor::Azure,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Size {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ControlRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl ControlRect {
    const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    const fn right(self) -> i32 {
        self.x + self.width
    }

    const fn bottom(self) -> i32 {
        self.y + self.height
    }

    #[cfg(test)]
    const fn contains(self, child: Self) -> bool {
        child.x >= self.x
            && child.y >= self.y
            && child.right() <= self.right()
            && child.bottom() <= self.bottom()
    }

    fn scaled(self, dpi: u32) -> Self {
        let x = scale(self.x, dpi);
        let y = scale(self.y, dpi);
        Self::new(
            x,
            y,
            scale(self.right(), dpi).saturating_sub(x),
            scale(self.bottom(), dpi).saturating_sub(y),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DialogLayout {
    client: Size,
    general_group: ControlRect,
    general_options: [ControlRect; GENERAL_OPTION_COUNT],
    appearance_group: ControlRect,
    theme_label: ControlRect,
    theme_selector: ControlRect,
    icon_label: ControlRect,
    icon_selector: ControlRect,
    appearance_options: [ControlRect; APPEARANCE_OPTION_COUNT],
    monitor_group: ControlRect,
    monitor_option: ControlRect,
    ok_button: ControlRect,
    cancel_button: ControlRect,
}

impl DialogLayout {
    fn for_dpi(dpi: u32) -> Self {
        let logical = Self::logical();
        Self {
            client: Size {
                width: scale(logical.client.width, dpi),
                height: scale(logical.client.height, dpi),
            },
            general_group: logical.general_group.scaled(dpi),
            general_options: logical.general_options.map(|rect| rect.scaled(dpi)),
            appearance_group: logical.appearance_group.scaled(dpi),
            theme_label: logical.theme_label.scaled(dpi),
            theme_selector: logical.theme_selector.scaled(dpi),
            icon_label: logical.icon_label.scaled(dpi),
            icon_selector: logical.icon_selector.scaled(dpi),
            appearance_options: logical.appearance_options.map(|rect| rect.scaled(dpi)),
            monitor_group: logical.monitor_group.scaled(dpi),
            monitor_option: logical.monitor_option.scaled(dpi),
            ok_button: logical.ok_button.scaled(dpi),
            cancel_button: logical.cancel_button.scaled(dpi),
        }
    }

    const fn logical() -> Self {
        Self {
            client: Size {
                width: CLIENT_WIDTH,
                height: CLIENT_HEIGHT,
            },
            general_group: ControlRect::new(20, 16, 520, 250),
            general_options: option_rows::<GENERAL_OPTION_COUNT>(38, 42, 484, 24, 27),
            appearance_group: ControlRect::new(20, 280, 520, 316),
            theme_label: ControlRect::new(38, 309, 64, 24),
            theme_selector: ControlRect::new(112, 304, APPEARANCE_SELECTOR_WIDTH, 30),
            icon_label: ControlRect::new(38, 347, 64, 24),
            icon_selector: ControlRect::new(112, 342, APPEARANCE_SELECTOR_WIDTH, 30),
            appearance_options: option_rows::<APPEARANCE_OPTION_COUNT>(38, 380, 484, 24, 27),
            monitor_group: ControlRect::new(20, 610, 520, 64),
            monitor_option: ControlRect::new(38, 635, 484, 24),
            ok_button: ControlRect::new(338, 695, 96, 32),
            cancel_button: ControlRect::new(444, 695, 96, 32),
        }
    }
}

const fn option_rows<const COUNT: usize>(
    x: i32,
    first_y: i32,
    width: i32,
    height: i32,
    step: i32,
) -> [ControlRect; COUNT] {
    let mut rows = [ControlRect::new(0, 0, 0, 0); COUNT];
    let mut index = 0;
    let mut y = first_y;
    while index < COUNT {
        rows[index] = ControlRect::new(x, y, width, height);
        y += step;
        index += 1;
    }
    rows
}

fn scale(value: i32, dpi: u32) -> i32 {
    let numerator = i64::from(value) * i64::from(dpi) + i64::from(BASE_DPI / 2);
    i32::try_from(numerator / i64::from(BASE_DPI)).unwrap_or(i32::MAX)
}

pub fn show(owner: HWND, settings: &Settings) -> Result<Option<Settings>> {
    let instance = module_instance()?;
    register_class(instance)?;
    let dpi = owner_dpi(owner);
    let layout = DialogLayout::for_dpi(dpi);
    let window_size = adjusted_window_size(layout.client, dpi)?;
    let window_origin = centered_window_origin(owner, window_size)?;
    let initial_dark = resolve_current_theme(settings.appearance.theme) == ResolvedTheme::Dark;
    let dark_mode_api = match DarkModeApi::load(initial_dark) {
        Ok(api) => Some(api),
        Err(error) => {
            eprintln!("Native dark settings controls are unavailable: {error}");
            None
        }
    };
    let _owner_guard = OwnerGuard::disable(owner);
    let host = Box::new(DialogHost::new(DialogState::new(
        settings.clone(),
        dpi,
        instance,
        dark_mode_api,
    )));
    let host_pointer = Box::into_raw(host);
    let window = unsafe {
        // SAFETY: host_pointer remains allocated through the nested dialog loop and WM_NCCREATE
        // stores it as window user data without taking ownership.
        CreateWindowExW(
            WINDOW_EX_STYLE_VALUE,
            WINDOW_CLASS,
            WINDOW_TITLE,
            WINDOW_STYLE_VALUE,
            window_origin.x,
            window_origin.y,
            window_size.width,
            window_size.height,
            settings_window_owner(owner),
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
            return Err(error);
        }
    };

    let controls_result = unsafe {
        // SAFETY: host_pointer remains live for the nested loop. The RefCell guard makes any
        // synchronous callback re-entry fail closed instead of creating a mutable alias.
        (*host_pointer)
            .state
            .borrow_mut()
            .create_controls(window, instance, host_pointer)
    };
    if let Err(error) = controls_result {
        let destroy_result = unsafe {
            // SAFETY: window is live and owned by this UI thread.
            DestroyWindow(window)
        };
        if let Err(destroy_error) = destroy_result {
            // The live HWND still retains host_pointer. Leaking is safer than freeing callback state.
            eprintln!("Could not destroy the incomplete settings window: {destroy_error}");
            return Err(error);
        }
        unsafe {
            // SAFETY: successful synchronous destruction cleared the window user data.
            drop(Box::from_raw(host_pointer));
        }
        return Err(error);
    }

    unsafe {
        // SAFETY: window is fully initialized and owned by this UI thread.
        let _was_visible = ShowWindow(window, SW_SHOW);
        let _foreground = SetForegroundWindow(window);
    }
    let ok_button = unsafe {
        // SAFETY: host_pointer remains live and setup has released its mutable state borrow.
        (*host_pointer).state.borrow().controls.ok_button
    };
    unsafe {
        // SAFETY: ok_button is a fully initialized child of the live settings window.
        let _focused = SetFocus(Some(ok_button));
    }
    let loop_result = run_dialog_loop(window, host_pointer);
    if let Err(loop_error) = loop_result {
        let destroy_result = unsafe {
            // SAFETY: the nested loop failed while this UI thread still owns the dialog HWND.
            DestroyWindow(window)
        };
        if let Err(destroy_error) = destroy_result {
            // The live HWND still retains host_pointer. Leaking is safer than freeing callback state.
            eprintln!("Could not destroy settings after its message loop failed: {destroy_error}");
            return Err(loop_error);
        }
        unsafe {
            // SAFETY: successful synchronous destruction cleared the window user data.
            drop(Box::from_raw(host_pointer));
        }
        return Err(loop_error);
    }
    let host = unsafe {
        // SAFETY: a successful loop exit occurs only after WM_NCDESTROY cleared window user data.
        Box::from_raw(host_pointer)
    };
    let state = host.state.into_inner();
    Ok(state.accepted.then_some(state.settings))
}

fn settings_window_owner(_modal_controller: HWND) -> Option<HWND> {
    // Native ownership would promote Settings into the topmost overlay's Z-order band. OwnerGuard
    // supplies the required modality without that relationship.
    None
}

#[derive(Default)]
struct DialogControls {
    general_group: HWND,
    appearance_group: HWND,
    monitor_group: HWND,
    theme_label: HWND,
    theme_selector: HWND,
    icon_label: HWND,
    icon_selector: HWND,
    options: [HWND; OPTION_COUNT],
    ok_button: HWND,
    cancel_button: HWND,
}

impl DialogControls {
    fn option(&self, option: SettingOption) -> HWND {
        self.options[option as usize]
    }

    fn apply_layout(&self, layout: &DialogLayout, dpi: u32) -> Result<()> {
        move_control(self.general_group, layout.general_group)?;
        for option in SettingOption::ALL {
            move_control(self.option(option), option.rect(layout))?;
        }
        move_control(self.appearance_group, layout.appearance_group)?;
        move_control(self.theme_label, layout.theme_label)?;
        move_control(
            self.theme_selector,
            combo_window_rect(layout.theme_selector, dpi),
        )?;
        move_control(self.icon_label, layout.icon_label)?;
        move_control(
            self.icon_selector,
            combo_window_rect(layout.icon_selector, dpi),
        )?;
        move_control(self.monitor_group, layout.monitor_group)?;
        move_control(self.ok_button, layout.ok_button)?;
        move_control(self.cancel_button, layout.cancel_button)
    }

    fn apply_fonts(&self, fonts: &DialogFonts) {
        for group in [
            self.general_group,
            self.appearance_group,
            self.monitor_group,
        ] {
            set_control_font(group, fonts.heading.0);
        }
        for control in [
            self.theme_label,
            self.theme_selector,
            self.icon_label,
            self.icon_selector,
        ]
        .into_iter()
        .chain(self.options)
        .chain([self.ok_button, self.cancel_button])
        {
            set_control_font(control, fonts.body.0);
        }
    }

    fn theme_targets(&self) -> impl Iterator<Item = ThemeTarget> + '_ {
        [
            self.general_group,
            self.appearance_group,
            self.monitor_group,
            self.theme_label,
            self.icon_label,
        ]
        .into_iter()
        .chain(self.options)
        .chain([self.ok_button, self.cancel_button])
        .map(|hwnd| ThemeTarget {
            hwnd,
            kind: ThemeTargetKind::Standard,
        })
        .chain(
            [self.theme_selector, self.icon_selector].map(|hwnd| ThemeTarget {
                hwnd,
                kind: ThemeTargetKind::ComboBox,
            }),
        )
    }

    fn custom_paint_targets(&self) -> impl Iterator<Item = HWND> + '_ {
        [
            self.general_group,
            self.appearance_group,
            self.monitor_group,
        ]
        .into_iter()
        .chain(self.options)
        .chain([
            self.theme_selector,
            self.icon_selector,
            self.ok_button,
            self.cancel_button,
        ])
    }
}

fn group_label(controls: &DialogControls, hwnd: HWND) -> Option<&'static str> {
    if hwnd == controls.general_group {
        Some("General")
    } else if hwnd == controls.appearance_group {
        Some("Appearance")
    } else if hwnd == controls.monitor_group {
        Some("Monitor")
    } else {
        None
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
        host: *mut DialogHost,
    ) -> Result<()> {
        let layout = DialogLayout::for_dpi(self.dpi);
        let fonts = DialogFonts::create(self.dpi)?;
        self.controls.general_group = create_group(
            parent,
            instance,
            "General",
            layout.general_group,
            fonts.heading.0,
        )?;
        self.create_options(parent, instance, &layout, &fonts, OptionGroup::General)?;
        self.create_appearance_controls(parent, instance, &layout, &fonts)?;
        self.controls.monitor_group = create_group(
            parent,
            instance,
            "Monitor",
            layout.monitor_group,
            fonts.heading.0,
        )?;
        self.create_options(parent, instance, &layout, &fonts, OptionGroup::Monitor)?;
        self.controls.ok_button = create_button(
            parent,
            instance,
            "OK",
            OK_ID,
            layout.ok_button,
            true,
            fonts.body.0,
        )?;
        self.controls.cancel_button = create_button(
            parent,
            instance,
            "Cancel",
            CANCEL_ID,
            layout.cancel_button,
            false,
            fonts.body.0,
        )?;
        self.fonts = Some(fonts);
        install_custom_control_painting(&self.controls, host)?;
        self.sync_right_button_release_enabled();
        self.apply_selected_icon();
        self.apply_selected_theme();
        Ok(())
    }

    fn create_appearance_controls(
        &mut self,
        parent: HWND,
        instance: HINSTANCE,
        layout: &DialogLayout,
        fonts: &DialogFonts,
    ) -> Result<()> {
        self.controls.appearance_group = create_group(
            parent,
            instance,
            "Appearance",
            layout.appearance_group,
            fonts.heading.0,
        )?;
        self.controls.theme_label =
            create_label(parent, instance, "Theme", layout.theme_label, fonts.body.0)?;
        self.controls.theme_selector = create_theme_selector(
            parent,
            instance,
            combo_window_rect(layout.theme_selector, self.dpi),
            self.settings.appearance.theme,
            fonts.body.0,
        )?;
        self.controls.icon_label =
            create_label(parent, instance, "Icon", layout.icon_label, fonts.body.0)?;
        self.controls.icon_selector = create_icon_selector(
            parent,
            instance,
            combo_window_rect(layout.icon_selector, self.dpi),
            self.settings.appearance.icon,
            fonts.body.0,
        )?;
        self.create_options(parent, instance, layout, fonts, OptionGroup::Appearance)
    }

    fn create_options(
        &mut self,
        parent: HWND,
        instance: HINSTANCE,
        layout: &DialogLayout,
        fonts: &DialogFonts,
        group: OptionGroup,
    ) -> Result<()> {
        for option in SettingOption::ALL
            .into_iter()
            .filter(|option| option.group() == group)
        {
            self.controls.options[option as usize] = create_checkbox(
                parent,
                instance,
                option.label(),
                option.control_id(),
                option.rect(layout),
                option.read(&self.settings),
                fonts.body.0,
                option == SettingOption::Autostart,
            )?;
        }
        Ok(())
    }

    fn update_dpi(&mut self, dpi: u32) -> Result<()> {
        let dpi = dpi.max(BASE_DPI / 2);
        let layout = DialogLayout::for_dpi(dpi);
        let fonts = DialogFonts::create(dpi)?;
        self.controls.apply_layout(&layout, dpi)?;
        self.controls.apply_fonts(&fonts);
        self.fonts = Some(fonts);
        self.dpi = dpi;
        Ok(())
    }

    fn selected_theme(&self) -> Theme {
        let selected = unsafe {
            // SAFETY: theme_selector is a live drop-down-list control with scalar message payloads.
            SendMessageW(
                self.controls.theme_selector,
                windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
        };
        let selected = i32::try_from(selected.0).unwrap_or(CB_ERR);
        if selected == CB_ERR {
            self.settings.appearance.theme
        } else {
            theme_from_selector_index(usize::try_from(selected).unwrap_or_default())
        }
    }

    fn selected_icon(&self) -> IconColor {
        let selected = unsafe {
            // SAFETY: icon_selector is a live drop-down-list control with scalar message payloads.
            SendMessageW(
                self.controls.icon_selector,
                windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
        };
        let selected = i32::try_from(selected.0).unwrap_or(CB_ERR);
        if selected == CB_ERR {
            self.settings.appearance.icon
        } else {
            icon_from_selector_index(usize::try_from(selected).unwrap_or_default())
        }
    }

    fn sync_right_button_release_enabled(&self) {
        let enabled = is_checked(
            self.controls
                .option(SettingOption::RightButtonWheelSwitching),
        );
        unsafe {
            // SAFETY: both option handles are live controls owned by this dialog.
            let _was_enabled = EnableWindow(
                self.controls
                    .option(SettingOption::ReleaseRightButtonSwitches),
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
            option.write(&mut self.settings, is_checked(self.controls.option(option)));
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
                background: rgb(32, 32, 32),
                text: rgb(240, 240, 240),
                disabled_text: rgb(145, 145, 145),
                control_surface: rgb(45, 45, 45),
                pressed_surface: rgb(66, 66, 66),
                control_border: rgb(125, 125, 125),
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
    let immersive_dark = i32::from(dark);
    let dwm_result = unsafe {
        // SAFETY: window is a live top-level HWND and immersive_dark remains valid for the
        // synchronous DWM attribute call.
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const immersive_dark).cast(),
            u32::try_from(size_of_val(&immersive_dark)).unwrap_or(u32::MAX),
        )
    };
    if let Err(error) = dwm_result {
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

fn create_group(
    parent: HWND,
    instance: HINSTANCE,
    label: &str,
    rect: ControlRect,
    font: HFONT,
) -> Result<HWND> {
    create_control(
        parent,
        instance,
        w!("BUTTON"),
        label,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        None,
        rect,
        font,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "checkbox construction keeps the Win32 parent, geometry, state, font, and tab-group contract explicit"
)]
fn create_checkbox(
    parent: HWND,
    instance: HINSTANCE,
    label: &str,
    id: usize,
    rect: ControlRect,
    checked: bool,
    font: HFONT,
    starts_group: bool,
) -> Result<HWND> {
    let group_style = if starts_group {
        WS_GROUP
    } else {
        WINDOW_STYLE::default()
    };
    let control = create_control(
        parent,
        instance,
        w!("BUTTON"),
        label,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | group_style | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        Some(id),
        rect,
        font,
    )?;
    if checked {
        unsafe {
            // SAFETY: control is a live checkbox and BM_SETCHECK consumes scalar values only.
            SendMessageW(control, BM_SETCHECK, Some(WPARAM(1)), Some(LPARAM(0)));
        }
    }
    Ok(control)
}

fn create_label(
    parent: HWND,
    instance: HINSTANCE,
    label: &str,
    rect: ControlRect,
    font: HFONT,
) -> Result<HWND> {
    create_control(
        parent,
        instance,
        w!("STATIC"),
        label,
        WS_CHILD | WS_VISIBLE,
        None,
        rect,
        font,
    )
}

fn create_theme_selector(
    parent: HWND,
    instance: HINSTANCE,
    rect: ControlRect,
    selected: Theme,
    font: HFONT,
) -> Result<HWND> {
    create_selector(
        parent,
        instance,
        THEME_ID,
        rect,
        &THEME_LABELS,
        theme_selector_index(selected),
        font,
    )
}

fn create_icon_selector(
    parent: HWND,
    instance: HINSTANCE,
    rect: ControlRect,
    selected: IconColor,
    font: HFONT,
) -> Result<HWND> {
    create_selector(
        parent,
        instance,
        ICON_ID,
        rect,
        &ICON_LABELS,
        icon_selector_index(selected),
        font,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "selector construction keeps the native control contract explicit"
)]
fn create_selector(
    parent: HWND,
    instance: HINSTANCE,
    id: usize,
    rect: ControlRect,
    labels: &[&str],
    selected_index: usize,
    font: HFONT,
) -> Result<HWND> {
    let selector = create_control(
        parent,
        instance,
        w!("COMBOBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WS_TABSTOP
            | WS_VSCROLL
            | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS).cast_unsigned()),
        Some(id),
        rect,
        font,
    )?;
    for label in labels {
        let text = null_terminated(label);
        let result = unsafe {
            // SAFETY: selector is live and text remains valid throughout the synchronous insertion.
            SendMessageW(
                selector,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            )
        };
        if i32::try_from(result.0).unwrap_or(CB_ERR) < 0 {
            return Err(Error::from_hresult(HRESULT(0x8000_4005_u32.cast_signed())));
        }
    }
    let result = unsafe {
        // SAFETY: selector is live and selected_index refers to one of the inserted items.
        SendMessageW(
            selector,
            CB_SETCURSEL,
            Some(WPARAM(selected_index)),
            Some(LPARAM(0)),
        )
    };
    if i32::try_from(result.0).unwrap_or(CB_ERR) == CB_ERR {
        Err(Error::from_hresult(HRESULT(0x8000_4005_u32.cast_signed())))
    } else {
        Ok(selector)
    }
}

fn create_button(
    parent: HWND,
    instance: HINSTANCE,
    label: &str,
    id: usize,
    rect: ControlRect,
    default_button: bool,
    font: HFONT,
) -> Result<HWND> {
    let button_style = if default_button {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    create_control(
        parent,
        instance,
        w!("BUTTON"),
        label,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(button_style.cast_unsigned()),
        Some(id),
        rect,
        font,
    )
}

fn install_custom_control_painting(controls: &DialogControls, host: *mut DialogHost) -> Result<()> {
    for control in controls.custom_paint_targets() {
        let installed = unsafe {
            // SAFETY: every handle is a live child control and host is the stable Box allocation
            // retained until after all children receive WM_NCDESTROY.
            SetWindowSubclass(
                control,
                Some(settings_control_subclass_proc),
                SETTINGS_CONTROL_SUBCLASS_ID,
                host as usize,
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
            (host_pointer as *const DialogHost).as_ref()
        }?;
        let Ok(state) = host.state.try_borrow() else {
            return None;
        };
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
    let result = if let Some(label) = group_label(&state.controls, hwnd) {
        paint_group_box(dc, client, label, state)
    } else if let Some(option) = SettingOption::ALL
        .into_iter()
        .find(|option| state.controls.option(*option) == hwnd)
    {
        paint_checkbox(
            dc,
            client,
            option.label(),
            is_checked(hwnd),
            is_control_enabled(hwnd),
            state,
        )
    } else if hwnd == state.controls.theme_selector {
        paint_combo_box(dc, client, state.selected_theme().as_ini_value(), state)
    } else if hwnd == state.controls.icon_selector {
        paint_combo_box(dc, client, state.selected_icon().as_ini_value(), state)
    } else if hwnd == state.controls.ok_button {
        paint_push_button(dc, client, "OK", hwnd, state)
    } else if hwnd == state.controls.cancel_button {
        paint_push_button(dc, client, "Cancel", hwnd, state)
    } else {
        Ok(())
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
        scale(1, state.dpi).max(1),
    )?;

    let Some(fonts) = state.fonts.as_ref() else {
        return Err(Error::from_hresult(HRESULT(0x8000_4005_u32.cast_signed())));
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
        scale(1, state.dpi).max(1),
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
        if enabled {
            state.palette.text
        } else {
            state.palette.disabled_text
        },
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
        if enabled {
            state.palette.text
        } else {
            state.palette.disabled_text
        },
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        state,
    )
}

fn paint_combo_box(dc: HDC, client: RECT, label: &str, state: &DialogState) -> Result<()> {
    fill_color(dc, client, state.palette.control_surface)?;
    let border = scale(1, state.dpi).max(1);
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
        scale(1, state.dpi).max(1),
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
        return Err(Error::from_hresult(HRESULT(0x8000_4005_u32.cast_signed())));
    };
    draw_text_with_font(dc, label, rect, color, format, fonts.body.0)
}

fn draw_checkmark(dc: HDC, rect: RECT, color: COLORREF, dpi: u32) -> Result<()> {
    let points = checkmark_points(rect);
    draw_polyline(dc, &points, color, scale(2, dpi).max(2))
}

fn checkmark_points(rect: RECT) -> [Point; 3] {
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    [
        Point {
            x: rect.left.saturating_add(width * 3 / 14),
            y: rect.top.saturating_add(height * 7 / 14),
        },
        Point {
            x: rect.left.saturating_add(width * 6 / 14),
            y: rect.top.saturating_add(height * 10 / 14),
        },
        Point {
            x: rect.left.saturating_add(width * 11 / 14),
            y: rect.top.saturating_add(height * 4 / 14),
        },
    ]
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
    let mut success = unsafe {
        // SAFETY: dc is live and the previous-point output is intentionally unused.
        MoveToEx(dc, first.x, first.y, std::ptr::null_mut()).as_bool()
    };
    for point in remaining {
        success &= unsafe {
            // SAFETY: dc remains live with the owned pen selected.
            LineTo(dc, point.x, point.y).as_bool()
        };
    }
    unsafe {
        // SAFETY: previous is the object returned by SelectObject for this dc.
        if previous != HGDIOBJ::default() {
            SelectObject(dc, previous);
        }
        // SAFETY: pen is uniquely owned and no longer selected into dc.
        if !DeleteObject(HGDIOBJ(pen.0)).as_bool() {
            eprintln!("Could not release a settings drawing pen");
        }
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
    rect: ControlRect,
    font: HFONT,
) -> Result<HWND> {
    let text = null_terminated(label);
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

fn move_control(control: HWND, rect: ControlRect) -> Result<()> {
    unsafe {
        // SAFETY: control is a live child HWND and rect contains bounded, DPI-scaled coordinates.
        MoveWindow(control, rect.x, rect.y, rect.width, rect.height, true)
    }
}

fn combo_window_rect(rect: ControlRect, dpi: u32) -> ControlRect {
    ControlRect {
        height: rect.height.saturating_add(scale(96, dpi)),
        ..rect
    }
}

fn is_checked(control: HWND) -> bool {
    let result = unsafe {
        // SAFETY: control is a live checkbox and BM_GETCHECK has no pointer payload.
        SendMessageW(control, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0)))
    };
    result.0 == 1
}

fn adjusted_window_size(client: Size, dpi: u32) -> Result<Size> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client.width,
        bottom: client.height,
    };
    unsafe {
        // SAFETY: rect is writable and both styles are the exact styles used to create the window.
        AdjustWindowRectExForDpi(
            &raw mut rect,
            WINDOW_STYLE_VALUE,
            false,
            WINDOW_EX_STYLE_VALUE,
            dpi,
        )?;
    }
    Ok(Size {
        width: rect.right.saturating_sub(rect.left),
        height: rect.bottom.saturating_sub(rect.top),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

fn centered_window_origin(owner: HWND, window: Size) -> Result<Point> {
    let monitor = unsafe {
        // SAFETY: owner is the live application window and the nearest-monitor fallback guarantees
        // a monitor for off-screen or hidden owner bounds.
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST)
    };
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..MONITORINFO::default()
    };
    unsafe {
        // SAFETY: monitor identifies the nearest display and monitor_info is a writable structure
        // with its size field initialized.
        if !GetMonitorInfoW(monitor, &raw mut monitor_info).as_bool() {
            return Err(Error::from_thread());
        }
    }
    Ok(centered_in_rect(monitor_info.rcWork, window))
}

fn centered_in_rect(area: RECT, window: Size) -> Point {
    let area_width = area.right.saturating_sub(area.left);
    let area_height = area.bottom.saturating_sub(area.top);
    Point {
        x: area
            .left
            .saturating_add(area_width.saturating_sub(window.width) / 2),
        y: area
            .top
            .saturating_add(area_height.saturating_sub(window.height) / 2),
    }
}

fn owner_dpi(owner: HWND) -> u32 {
    let dpi = unsafe {
        // SAFETY: owner is the live application window used for this modal dialog.
        GetDpiForWindow(owner)
    };
    if dpi == 0 { BASE_DPI } else { dpi }
}

fn run_dialog_loop(window: HWND, host: *mut DialogHost) -> Result<()> {
    let mut message = MSG::default();
    loop {
        let done = unsafe {
            // SAFETY: host remains allocated for this nested loop. Cell supports reentrant reads
            // on the single UI thread without creating a mutable alias.
            (*host).done.get()
        };
        if done {
            break;
        }
        let result = unsafe {
            // SAFETY: message is writable and this UI thread owns the nested dialog loop.
            GetMessageW(&raw mut message, None, 0, 0)
        };
        if result.0 == -1 {
            return Err(Error::from_thread());
        }
        if result.0 == 0 {
            let exit_code = i32::try_from(message.wParam.0).unwrap_or_default();
            unsafe {
                // SAFETY: this UI thread owns the live dialog HWND. Destruction synchronously clears
                // the state pointer. WM_QUIT must be preserved even if destruction fails.
                let destroy_result = DestroyWindow(window);
                PostQuitMessage(exit_code);
                destroy_result?;
            }
            return Ok(());
        }
        let handled = unsafe {
            // SAFETY: window and message are live on this UI thread for the synchronous call.
            IsDialogMessageW(window, &raw const message).as_bool()
        };
        if !handled {
            unsafe {
                // SAFETY: GetMessageW initialized message for this UI thread.
                let _translated = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn settings_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if message == WM_NCCREATE {
            let create = unsafe {
                // SAFETY: WM_NCCREATE guarantees lParam points to CREATESTRUCTW for this callback.
                (lparam.0 as *const CREATESTRUCTW).as_ref()
            }?;
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
                // SAFETY: host remains live for the complete nested dialog loop.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, host as isize);
            }
            return None;
        }
        let host = unsafe {
            // SAFETY: user data is either zero or the live DialogHost pointer installed above.
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogHost).as_ref()
        }?;
        if message == WM_DESTROY_DIALOG {
            let result = unsafe {
                // SAFETY: the posted message runs on the UI thread that owns hwnd.
                DestroyWindow(hwnd)
            };
            if let Err(error) = result {
                eprintln!("Could not close the settings window: {error}");
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
        handle_settings_message(&mut state, hwnd, message, wparam, lparam)
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| default_window_proc(hwnd, message, wparam, lparam))
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
            handle_dpi_changed(state, hwnd, wparam, lparam);
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

fn handle_dpi_changed(state: &mut DialogState, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let suggested = unsafe {
        // SAFETY: WM_DPICHANGED guarantees lParam points to a suggested window RECT.
        (lparam.0 as *const RECT).as_ref()
    };
    if let Some(suggested) = suggested {
        let resize_result = unsafe {
            // SAFETY: hwnd is live and the suggested rectangle comes from WM_DPICHANGED.
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
        if let Err(error) = resize_result {
            eprintln!("Could not resize settings for its new display scale: {error}");
        }
    }
    let new_dpi = u32::try_from(low_word(wparam.0)).unwrap_or(BASE_DPI);
    if let Err(error) = state.update_dpi(new_dpi) {
        eprintln!("Could not lay out settings for its new display scale: {error}");
    }
    let invalidated = unsafe {
        // SAFETY: hwnd is live for the callback and the full dialog must redraw.
        InvalidateRect(Some(hwnd), None, true)
    };
    if !invalidated.as_bool() {
        eprintln!("Could not redraw settings after its display scale changed");
    }
}

fn handle_command(hwnd: HWND, state: &mut DialogState, wparam: WPARAM) -> Option<LRESULT> {
    let command = low_word(wparam.0);
    if command == OK_ID {
        state.accept();
        request_dialog_close(hwnd);
        Some(LRESULT(0))
    } else if command == CANCEL_ID {
        state.cancel();
        request_dialog_close(hwnd);
        Some(LRESULT(0))
    } else if command == THEME_ID && high_word(wparam.0) == CBN_SELCHANGE as usize {
        state.apply_selected_theme();
        Some(LRESULT(0))
    } else if command == ICON_ID && high_word(wparam.0) == CBN_SELCHANGE as usize {
        state.apply_selected_icon();
        Some(LRESULT(0))
    } else if command == SettingOption::RightButtonWheelSwitching.control_id()
        && high_word(wparam.0) == BN_CLICKED as usize
    {
        state.sync_right_button_release_enabled();
        Some(LRESULT(0))
    } else {
        None
    }
}

fn request_dialog_close(hwnd: HWND) {
    let result = unsafe {
        // SAFETY: hwnd is live and the private message carries no borrowed data.
        PostMessageW(Some(hwnd), WM_DESTROY_DIALOG, WPARAM(0), LPARAM(0))
    };
    if let Err(error) = result {
        eprintln!("Could not request settings closure: {error}");
    }
}

fn paint_background(hwnd: HWND, dc: HDC, brush: HBRUSH) -> LRESULT {
    let mut client = RECT::default();
    let client_result = unsafe {
        // SAFETY: hwnd and dc are the live handles supplied for WM_ERASEBKGND; client is writable.
        windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &raw mut client)
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

fn default_window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // SAFETY: unhandled messages are forwarded with their original scalar values.
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn register_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe {
        // SAFETY: IDC_ARROW is a predefined shared cursor.
        LoadCursorW(None, IDC_ARROW)
    }?;
    let icon = app_icon::load_app(instance, IconColor::Azure)?;
    let background = HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void);
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(settings_window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        hbrBackground: background,
        lpszClassName: WINDOW_CLASS,
        hIconSm: icon,
        ..WNDCLASSEXW::default()
    };
    let atom = unsafe {
        // SAFETY: class and its static class name remain valid for the synchronous call.
        RegisterClassExW(&raw const class)
    };
    if atom != 0
        || unsafe {
            // SAFETY: RegisterClassExW just failed and its last-error value is still available.
            GetLastError()
        } == ERROR_CLASS_ALREADY_EXISTS
    {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}

fn module_instance() -> Result<HINSTANCE> {
    let module = unsafe {
        // SAFETY: None requests a borrowed handle for this executable module.
        GetModuleHandleW(None)
    }?;
    Ok(HINSTANCE(module.0))
}

struct OwnerGuard(HWND);

impl OwnerGuard {
    fn disable(owner: HWND) -> Self {
        unsafe {
            // SAFETY: owner is the live application HWND and remains live through the dialog loop.
            let _was_enabled = EnableWindow(owner, false);
        }
        Self(owner)
    }
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: owner remains live after the nested dialog closes.
            let _was_disabled = EnableWindow(self.0, true);
            let _foreground = SetForegroundWindow(self.0);
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Win32 command ids are stored in the low 16 bits of WPARAM"
)]
fn low_word(value: usize) -> usize {
    value & usize::from(u16::MAX)
}

fn high_word(value: usize) -> usize {
    value >> 16 & usize::from(u16::MAX)
}

fn null_terminated(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_checkbox_reads_only_its_bound_setting() {
        let fields: [fn(&mut Settings) -> &mut bool; 16] = [
            |s| &mut s.general.autostart,
            |s| &mut s.general.replace_alt_tab,
            |s| &mut s.general.replace_win_tab,
            |s| &mut s.general.typed_search,
            |s| &mut s.general.release_alt_switches,
            |s| &mut s.general.release_right_button_switches,
            |s| &mut s.general.right_button_wheel_switching,
            |s| &mut s.general.mouse_over_selection,
            |s| &mut s.appearance.compact_list,
            |s| &mut s.appearance.large_icons,
            |s| &mut s.appearance.show_numbers,
            |s| &mut s.appearance.show_app_names,
            |s| &mut s.appearance.visible_borders,
            |s| &mut s.appearance.preview,
            |s| &mut s.appearance.full_desktop_preview,
            |s| &mut s.monitor.use_current_monitor_filter,
        ];
        let baseline = Settings::default();
        for (index, field) in fields.into_iter().enumerate() {
            let mut changed = baseline.clone();
            *field(&mut changed) = !*field(&mut changed);
            let mut expected = SettingOption::ALL.map(|option| option.read(&baseline));
            expected[index] = !expected[index];
            assert_eq!(
                SettingOption::ALL.map(|option| option.read(&changed)),
                expected,
                "option {index}"
            );
            let option = SettingOption::ALL[index];
            let mut roundtrip = baseline.clone();
            option.write(&mut roundtrip, option.read(&changed));
            assert_eq!(roundtrip, changed, "write for {option:?}");
            option.write(&mut roundtrip, option.read(&baseline));
            assert_eq!(roundtrip, baseline, "restore for {option:?}");
        }
    }

    #[test]
    fn named_options_preserve_unique_ids_and_layout_rows_at_each_dpi() {
        for dpi in [96, 120, 144, 192] {
            let layout = DialogLayout::for_dpi(dpi);
            let rows = layout
                .general_options
                .into_iter()
                .chain(layout.appearance_options)
                .chain([layout.monitor_option]);
            for (index, (option, row)) in SettingOption::ALL.into_iter().zip(rows).enumerate() {
                assert_eq!(option.control_id(), OPTION_ID_BASE + index);
                assert_eq!(
                    option.rect(&layout),
                    row,
                    "layout for {option:?} at {dpi} DPI"
                );
            }
        }
    }

    #[test]
    fn settings_window_is_not_owned_by_the_topmost_overlay() {
        let mut owner_storage = 0_u8;
        let owner = HWND(std::ptr::from_mut(&mut owner_storage).cast());

        assert_eq!(settings_window_owner(owner), None);
    }

    #[test]
    fn settings_window_requests_app_window_alt_tab_presence() {
        let app_window = windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW;

        assert_ne!(WINDOW_EX_STYLE_VALUE.0 & app_window.0, 0);
    }

    #[test]
    fn settings_window_reports_its_caption() {
        let instance =
            module_instance().unwrap_or_else(|error| panic!("could not get test module: {error}"));
        register_class(instance)
            .unwrap_or_else(|error| panic!("could not register Settings test window: {error}"));
        let host = Box::new(DialogHost::new(DialogState::new(
            Settings::default(),
            BASE_DPI,
            instance,
            None,
        )));
        let host_pointer = Box::into_raw(host);
        let created = unsafe {
            // SAFETY: host_pointer remains allocated until the synchronously destroyed hidden
            // test window clears its user data.
            CreateWindowExW(
                WINDOW_EX_STYLE_VALUE,
                WINDOW_CLASS,
                WINDOW_TITLE,
                WINDOW_STYLE_VALUE,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(instance),
                Some(host_pointer.cast()),
            )
        };
        let window = match created {
            Ok(window) => window,
            Err(error) => {
                unsafe {
                    // SAFETY: failed creation did not retain the unique state allocation.
                    drop(Box::from_raw(host_pointer));
                }
                panic!("could not create Settings test window: {error}");
            }
        };
        let mut title = [0_u16; 64];
        let written = unsafe {
            // SAFETY: window is live and title is writable for the synchronous query.
            windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(window, &mut title)
        };
        let title = String::from_utf16_lossy(
            title
                .get(..usize::try_from(written).unwrap_or_default())
                .unwrap_or_default(),
        );
        let destroyed = unsafe {
            // SAFETY: window is the live hidden test window owned by this thread.
            DestroyWindow(window)
        };
        if destroyed.is_ok() {
            unsafe {
                // SAFETY: synchronous destruction cleared the HWND's state pointer.
                drop(Box::from_raw(host_pointer));
            }
        } else {
            panic!("could not destroy Settings test window");
        }

        assert_eq!(title, "AltTabio Settings");
    }

    #[test]
    fn visible_borders_option_tracks_the_appearance_setting() {
        let mut settings = Settings::default();

        assert_eq!(SettingOption::VisibleBorders.label(), "Visible borders");
        assert!(!SettingOption::VisibleBorders.read(&settings));

        settings.appearance.visible_borders = true;
        assert!(SettingOption::VisibleBorders.read(&settings));
    }

    #[test]
    fn typed_search_option_tracks_the_general_setting() {
        let mut settings = Settings::default();

        assert_eq!(
            SettingOption::TypedSearch.label(),
            "Enable typing to search tasks"
        );
        assert!(SettingOption::TypedSearch.read(&settings));

        settings.general.typed_search = false;
        assert!(!SettingOption::TypedSearch.read(&settings));
    }

    #[test]
    fn theme_values_map_to_selector_indices_and_canonical_settings() {
        for (index, theme) in [Theme::Auto, Theme::Light, Theme::Dark]
            .into_iter()
            .enumerate()
        {
            assert_eq!(theme_selector_index(theme), index);
            assert_eq!(theme_from_selector_index(index), theme);
            assert_eq!(THEME_LABELS[index], theme.as_ini_value());
        }
        assert_eq!(theme_from_selector_index(usize::MAX), Theme::Auto);
    }

    #[test]
    fn icon_values_map_to_selector_indices_and_canonical_labels() {
        for (index, icon) in IconColor::ALL.into_iter().enumerate() {
            assert_eq!(icon_selector_index(icon), index);
            assert_eq!(icon_from_selector_index(index), icon);
            assert_eq!(ICON_LABELS[index], icon.as_ini_value());
        }
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
        let mut general = 0_u8;
        let mut appearance = 0_u8;
        let mut monitor = 0_u8;
        let controls = DialogControls {
            general_group: HWND(std::ptr::from_mut(&mut general).cast()),
            appearance_group: HWND(std::ptr::from_mut(&mut appearance).cast()),
            monitor_group: HWND(std::ptr::from_mut(&mut monitor).cast()),
            ..DialogControls::default()
        };

        assert_eq!(
            group_label(&controls, controls.general_group),
            Some("General")
        );
        assert_eq!(
            group_label(&controls, controls.appearance_group),
            Some("Appearance")
        );
        assert_eq!(
            group_label(&controls, controls.monitor_group),
            Some("Monitor")
        );
        let targets = controls.custom_paint_targets().collect::<Vec<_>>();
        assert!(targets.contains(&controls.general_group));
        assert!(targets.contains(&controls.appearance_group));
        assert!(targets.contains(&controls.monitor_group));
    }

    #[test]
    fn checkmark_has_even_opposing_insets_inside_its_square() {
        let square = RECT {
            left: 0,
            top: 0,
            right: 14,
            bottom: 14,
        };

        let points = checkmark_points(square);

        assert_eq!(points[0], Point { x: 3, y: 7 });
        assert_eq!(points[1], Point { x: 6, y: 10 });
        assert_eq!(points[2], Point { x: 11, y: 4 });
        assert_eq!(points[0].x - square.left, square.right - points[2].x);
        assert_eq!(
            points.iter().map(|point| point.y).min(),
            Some(square.top + 4)
        );
        assert_eq!(
            points.iter().map(|point| point.y).max(),
            Some(square.bottom - 4)
        );
    }

    #[test]
    fn logical_layout_keeps_every_control_inside_its_section_or_client() {
        let layout = DialogLayout::logical();
        let client = ControlRect::new(0, 0, layout.client.width, layout.client.height);

        assert!(
            layout
                .general_options
                .into_iter()
                .all(|rect| layout.general_group.contains(rect))
        );
        assert!(layout.appearance_group.contains(layout.theme_label));
        assert!(layout.appearance_group.contains(layout.theme_selector));
        assert!(layout.appearance_group.contains(layout.icon_label));
        assert!(layout.appearance_group.contains(layout.icon_selector));
        assert!(
            layout
                .appearance_options
                .into_iter()
                .all(|rect| layout.appearance_group.contains(rect))
        );
        assert!(layout.monitor_group.contains(layout.monitor_option));
        assert!(client.contains(layout.general_group));
        assert!(client.contains(layout.appearance_group));
        assert!(client.contains(layout.monitor_group));
        assert!(client.contains(layout.ok_button));
        assert!(client.contains(layout.cancel_button));
        assert!(layout.ok_button.right() < layout.cancel_button.x);
        assert_eq!(layout.cancel_button.right(), layout.monitor_group.right());
    }

    #[test]
    fn appearance_selectors_share_one_aligned_column() {
        let layout = DialogLayout::logical();

        assert_eq!(layout.theme_label.x, layout.icon_label.x);
        assert_eq!(layout.theme_selector.x, layout.icon_selector.x);
        assert_eq!(layout.theme_selector.width, layout.icon_selector.width);
        assert_eq!(layout.theme_selector.width, APPEARANCE_SELECTOR_WIDTH);
        assert!(layout.theme_selector.bottom() <= layout.icon_selector.y);
    }

    #[test]
    fn layout_scales_consistently_at_one_hundred_fifty_percent() {
        let normal = DialogLayout::for_dpi(96);
        let scaled = DialogLayout::for_dpi(144);

        assert_eq!(scaled.client.width, normal.client.width * 3 / 2);
        assert_eq!(scaled.client.height, scale(CLIENT_HEIGHT, 144));
        assert_eq!(scaled.general_group.x, normal.general_group.x * 3 / 2);
        assert_eq!(
            scaled.general_options[7].y,
            scale(normal.general_options[7].y, 144)
        );
        assert_eq!(
            scaled.theme_selector.width,
            normal.theme_selector.width * 3 / 2
        );
        assert_eq!(
            scaled.icon_selector.width,
            normal.icon_selector.width * 3 / 2
        );
        assert_eq!(
            scaled.cancel_button.right(),
            normal.cancel_button.right() * 3 / 2
        );
        assert_eq!(
            scaled.cancel_button.bottom(),
            scale(DialogLayout::logical().cancel_button.bottom(), 144)
        );
    }

    #[test]
    fn centering_handles_negative_monitor_coordinates() {
        let origin = centered_in_rect(
            RECT {
                left: -1920,
                top: -120,
                right: 0,
                bottom: 960,
            },
            Size {
                width: 560,
                height: 709,
            },
        );

        assert_eq!(origin.x, -1240);
        assert_eq!(origin.y, 65);
    }
}
