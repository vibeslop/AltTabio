use crate::about_dialog;
use crate::hook::{HookThread, WM_HOOK_ACTION, decode_action};
use crate::preview::DwmPreview;
use crate::renderer::{CloseButtonVisualState, RenderOptions, Renderer, TaskListHit};
use crate::settings_dialog;
use crate::settings_io::SettingsStore;
use crate::single_instance::SingleInstance;
use crate::startup;
use crate::tray::{TrayAction, TrayIcon, WM_TRAY_CALLBACK};
use crate::window_commands::{
    execute as execute_window_command, show_menu as show_window_command_menu,
};
use alttabio::input::{HookSettings, InputAction, WindowCommand};
use alttabio::settings::{Settings, Theme};
use alttabio::switcher::{SwitchTask, Switcher, WindowEligibility, is_switchable_window};
use alttabio::theme::{ResolvedTheme, Rgb8, resolve};
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, MonitorFromRect, MonitorFromWindow, PAINTSTRUCT,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetActiveWindow, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
    TrackMouseEvent, VK_0, VK_1, VK_9, VK_BACK, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_F5, VK_F6,
    VK_F7, VK_F8, VK_F9, VK_HOME, VK_LEFT, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD9, VK_RETURN, VK_RIGHT,
    VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GCLP_HICON, GCLP_HICONSM,
    GW_OWNER, GWL_EXSTYLE, GWLP_USERDATA, GetClassLongPtrW, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetLastActivePopup, GetMessageW, GetShellWindow, GetWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, ICON_BIG,
    ICON_SMALL, ICON_SMALL2, IDC_ARROW, IsIconic, IsWindowVisible, KillTimer, LoadCursorW,
    MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageTimeoutW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, ShowWindowAsync, TranslateMessage, WM_CAPTURECHANGED, WM_CHAR, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETICON, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN,
    WM_THEMECHANGED, WM_TIMER, WNDCLASSEXW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_THICKFRAME,
};
use windows::core::{BOOL, Error, PCWSTR, PWSTR, Result, w};

const WINDOW_CLASS: PCWSTR = w!("AltTabioRustOverlay");
const WINDOW_TITLE: PCWSTR = w!("AltTabio");
const WM_SHOW_SETTINGS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;
const WM_DESTROY_APP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;
const WM_SHOW_ABOUT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 5;
const WM_MOUSE_LEAVE: u32 = 0x02A3;
const CLOSE_REFRESH_TIMER_ID: usize = 1;
const CLOSE_REFRESH_DELAY_MS: u32 = 250;
const CLOSE_REFRESH_ATTEMPTS: u8 = 20;

pub fn run(
    preview_mode: bool,
    dwm_preview: bool,
    settings: Settings,
    settings_store: SettingsStore,
) -> Result<()> {
    let Some(_single_instance) = SingleInstance::acquire()? else {
        return Ok(());
    };
    let _apartment = ComApartment::initialize()?;

    let instance = module_instance()?;
    register_window_class(instance)?;
    let visible_borders = settings.appearance.visible_borders;
    let app = App::new(preview_mode, dwm_preview, settings, settings_store)?;
    let resolved_theme = app.resolved_theme;
    let host = Box::new(AppHost::new(app));
    let host_pointer = Box::into_raw(host);
    let create_result = unsafe {
        // SAFETY: `host_pointer` remains allocated until after the window message loop exits. The
        // WM_NCCREATE handler stores it as window user data without taking ownership.
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            WINDOW_CLASS,
            WINDOW_TITLE,
            WS_POPUP | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            600,
            None,
            None,
            Some(instance),
            Some(host_pointer.cast()),
        )
    };
    let hwnd = match create_result {
        Ok(hwnd) => hwnd,
        Err(error) => {
            unsafe {
                // SAFETY: CreateWindowExW failed, so no window retained `host_pointer` and this is
                // the unique Box allocation created above.
                drop(Box::from_raw(host_pointer));
            }
            return Err(error);
        }
    };
    if let Err(error) = apply_window_appearance(hwnd, visible_borders, resolved_theme) {
        eprintln!("Could not apply the overlay window appearance: {error}");
    }

    let hook_result = unsafe {
        // SAFETY: host_pointer remains live for the message loop. The RefCell guard makes any
        // synchronous callback re-entry fail closed instead of creating a mutable alias.
        (*host_pointer)
            .state
            .borrow_mut()
            .initialize(hwnd, !preview_mode)
    };
    if let Err(error) = hook_result {
        unsafe {
            // SAFETY: `hwnd` is the live overlay window created above.
            DestroyWindow(hwnd)?;
            // SAFETY: after DestroyWindow returns no callback retains the unique host allocation.
            drop(Box::from_raw(host_pointer));
        }
        return Err(Error::new(
            windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
            &error,
        ));
    }
    if preview_mode {
        unsafe {
            // SAFETY: host_pointer remains live and initialization released its state borrow.
            (*host_pointer).state.borrow_mut().show_overlay();
        }
    }

    let loop_result = run_message_loop();
    let window_retains_app = unsafe {
        // SAFETY: hwnd is either the application window or an already-destroyed borrowed value;
        // nonzero user data means a live callback can still reach host_pointer.
        GetWindowLongPtrW(hwnd, GWLP_USERDATA) != 0
    };
    if window_retains_app {
        let destroy_result = unsafe {
            // SAFETY: this UI thread created the live window and is cleaning it up before App.
            DestroyWindow(hwnd)
        };
        if let Err(destroy_error) = destroy_result {
            // The live HWND still retains host_pointer. Leaking is safer than freeing callback state.
            eprintln!("Could not destroy AltTabio after its message loop ended: {destroy_error}");
            return match loop_result {
                Ok(()) => Err(destroy_error),
                Err(loop_error) => Err(loop_error),
            };
        }
    }
    unsafe {
        // SAFETY: WM_NCDESTROY cleared window user data, so no callback retains host_pointer.
        drop(Box::from_raw(host_pointer));
    }
    loop_result
}

pub fn show_fatal_error(message: &str) {
    let text = null_terminated(message);
    unsafe {
        // SAFETY: both UTF-16 buffers remain alive and null terminated for the synchronous call.
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("AltTabio"),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe {
            // SAFETY: the reserved pointer is null and this UI thread balances successful
            // initialization in ComApartment::drop.
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}

#[derive(Default)]
struct CloseButtonInteraction {
    hovered_target: Option<isize>,
    pressed_target: Option<isize>,
}

impl CloseButtonInteraction {
    fn visual_state(&self, selected_target: Option<isize>) -> CloseButtonVisualState {
        let Some(selected_target) = selected_target else {
            return CloseButtonVisualState::Normal;
        };
        if self.pressed_target == Some(selected_target)
            && self.hovered_target == Some(selected_target)
        {
            CloseButtonVisualState::Pressed
        } else if self.hovered_target == Some(selected_target) {
            CloseButtonVisualState::Hovered
        } else {
            CloseButtonVisualState::Normal
        }
    }

    fn update_hover(&mut self, target: Option<isize>) -> bool {
        if self.hovered_target == target {
            return false;
        }
        self.hovered_target = target;
        true
    }

    fn press(&mut self, target: isize) {
        self.hovered_target = Some(target);
        self.pressed_target = Some(target);
    }

    fn release(&mut self, target: Option<isize>) -> Option<WindowCommand> {
        let pressed_target = self.pressed_target.take();
        self.hovered_target = target;
        (pressed_target.is_some() && pressed_target == target).then_some(WindowCommand::Close)
    }

    fn cancel_press(&mut self) -> bool {
        self.pressed_target.take().is_some()
    }

    const fn is_pressed(&self) -> bool {
        self.pressed_target.is_some()
    }

    fn reset(&mut self) {
        self.hovered_target = None;
        self.pressed_target = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CloseRefresh {
    window_handle: isize,
    attempts_remaining: u8,
}

#[derive(Default)]
struct CloseRefreshTracker {
    pending: Vec<CloseRefresh>,
}

impl CloseRefreshTracker {
    fn track(&mut self, window_handle: isize) -> bool {
        if let Some(refresh) = self
            .pending
            .iter_mut()
            .find(|refresh| refresh.window_handle == window_handle)
        {
            refresh.attempts_remaining = CLOSE_REFRESH_ATTEMPTS;
            return false;
        }
        let timer_needed = self.pending.is_empty();
        self.pending.push(CloseRefresh {
            window_handle,
            attempts_remaining: CLOSE_REFRESH_ATTEMPTS,
        });
        timer_needed
    }

    fn reconcile(&mut self, tasks: &[SwitchTask]) -> bool {
        self.advance(|window_handle| tasks.iter().any(|task| task.window_handle == window_handle))
    }

    fn advance_after_enumeration_error(&mut self) -> bool {
        self.advance(|_| true)
    }

    fn advance(&mut self, target_still_present: impl Fn(isize) -> bool) -> bool {
        self.pending.retain_mut(|refresh| {
            if !target_still_present(refresh.window_handle) || refresh.attempts_remaining <= 1 {
                return false;
            }
            refresh.attempts_remaining -= 1;
            true
        });
        !self.pending.is_empty()
    }

    fn clear(&mut self) {
        self.pending.clear();
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: this guard is dropped on the same thread that successfully initialized COM.
            CoUninitialize();
        }
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Win32 lifecycle and input flags do not form one shared state machine"
)]
struct App {
    hwnd: HWND,
    switcher: Switcher,
    renderer: Renderer,
    resolved_theme: ResolvedTheme,
    preview: Option<DwmPreview>,
    hooks: Option<HookThread>,
    tray: Option<TrayIcon>,
    mouse_origin: Option<POINT>,
    mouse_selection_armed: bool,
    mouse_leave_tracked: bool,
    close_button: CloseButtonInteraction,
    close_refresh_tracker: CloseRefreshTracker,
    exit_when_hidden: bool,
    dwm_preview: bool,
    settings: Settings,
    settings_store: SettingsStore,
    settings_dialog_open: bool,
    about_dialog_open: bool,
    context_menu_open: bool,
}

struct AppHost {
    state: RefCell<App>,
}

impl AppHost {
    fn new(state: App) -> Self {
        Self {
            state: RefCell::new(state),
        }
    }

    fn show_settings(&self) {
        let Some((owner, mut dialog_settings)) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_settings_dialog())
        else {
            return;
        };

        let previous_autostart = match startup::status() {
            Ok(status) => {
                dialog_settings.general.autostart = status.enabled;
                status
            }
            Err(error) => {
                show_error_for_window(
                    owner,
                    &format!("Autostart status could not be read. {error}"),
                );
                startup::AutostartStatus {
                    enabled: dialog_settings.general.autostart,
                    task_exists: false,
                }
            }
        };
        let result = settings_dialog::show(owner, &dialog_settings);

        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish Settings because application state is busy");
            return;
        };
        app.settings_dialog_open = false;
        match result {
            Ok(Some(settings)) => app.apply_settings(settings, previous_autostart),
            Ok(None) => {}
            Err(error) => app.show_error(&format!("Could not open settings: {error}")),
        }
    }

    fn show_about(&self) {
        let Some((theme, icon)) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_about_dialog())
        else {
            return;
        };
        let result = about_dialog::show(theme, icon);

        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish About because application state is busy");
            return;
        };
        app.about_dialog_open = false;
        if let Err(error) = result {
            app.show_error(&error);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OverlayActivationState {
    visible: bool,
    context_menu_open: bool,
    release: ReleaseActivationSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReleaseActivationSettings {
    alt_switches: bool,
    right_button_switches: bool,
}

impl App {
    fn new(
        exit_when_hidden: bool,
        dwm_preview: bool,
        settings: Settings,
        settings_store: SettingsStore,
    ) -> Result<Self> {
        let resolved_theme = resolve_current_theme(settings.appearance.theme);
        Ok(Self {
            hwnd: HWND::default(),
            switcher: Switcher::default(),
            renderer: Renderer::new(resolved_theme)?,
            resolved_theme,
            preview: None,
            hooks: None,
            tray: None,
            mouse_origin: None,
            mouse_selection_armed: false,
            mouse_leave_tracked: false,
            close_button: CloseButtonInteraction::default(),
            close_refresh_tracker: CloseRefreshTracker::default(),
            exit_when_hidden,
            dwm_preview,
            settings,
            settings_store,
            settings_dialog_open: false,
            about_dialog_open: false,
            context_menu_open: false,
        })
    }

    fn initialize(&mut self, hwnd: HWND, install_hooks: bool) -> std::result::Result<(), String> {
        self.hwnd = hwnd;
        if self.dwm_preview && self.settings.appearance.preview {
            self.preview = Some(DwmPreview::new(
                hwnd,
                self.settings.appearance.full_desktop_preview,
                self.settings.appearance.compact_list,
            ));
        }
        if install_hooks {
            let instance = module_instance().map_err(|error| {
                format!("Could not resolve the executable module for the tray icon: {error}")
            })?;
            self.tray = Some(
                TrayIcon::new(
                    hwnd,
                    instance,
                    self.resolved_theme,
                    self.settings.appearance.icon,
                )
                .map_err(|error| format!("Could not create the tray icon: {error}"))?,
            );
            self.hooks = Some(HookThread::start(hwnd, hook_settings(&self.settings))?);
        }
        Ok(())
    }

    fn handle_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if message == WM_HOOK_ACTION {
            if let Some(action) = decode_action(wparam, lparam) {
                self.handle_input_action(action);
            }
            return Some(LRESULT(0));
        }
        if message == WM_TRAY_CALLBACK {
            self.handle_tray_message(lparam);
            return Some(LRESULT(0));
        }
        if message == WM_TIMER && wparam.0 == CLOSE_REFRESH_TIMER_ID {
            self.handle_close_refresh_timer();
            return Some(LRESULT(0));
        }
        match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                self.handle_key(wparam.0, lparam);
                Some(LRESULT(0))
            }
            WM_CHAR => {
                self.handle_character(wparam.0);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                self.handle_mouse_move(lparam);
                Some(LRESULT(0))
            }
            WM_MOUSE_LEAVE => {
                self.handle_mouse_leave();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                self.handle_button_down(lparam);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.handle_button_up(lparam);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                if self.close_button.cancel_press() {
                    self.request_redraw();
                }
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                self.handle_task_context_menu(lparam);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = high_word_usize(wparam.0).cast_signed();
                self.switcher.select_next(if delta > 0 { -1 } else { 1 });
                self.request_redraw();
                Some(LRESULT(0))
            }
            WM_SIZE => {
                let width = u32::from(low_word_isize(lparam.0));
                let height = u32::from(high_word_isize(lparam.0));
                self.resize_content(width, height);
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.handle_dpi_changed(lparam);
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                match self.refresh_theme() {
                    Ok(true) => self.request_redraw(),
                    Ok(false) => {}
                    Err(error) => eprintln!("Could not refresh the overlay theme: {error}"),
                }
                Some(LRESULT(0))
            }
            WM_PAINT => {
                self.paint();
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            _ => None,
        }
    }

    fn handle_tray_message(&mut self, lparam: LPARAM) {
        let message = u32::try_from(lparam.0).unwrap_or_default();
        let action = match message {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::Show,
            WM_RBUTTONUP => self
                .tray
                .as_ref()
                .map(TrayIcon::show_menu)
                .unwrap_or_default(),
            _ => TrayAction::None,
        };
        match action {
            TrayAction::Show => self.show_overlay(),
            TrayAction::Settings => self.request_modal_dialog(WM_SHOW_SETTINGS, "Settings"),
            TrayAction::About => self.request_modal_dialog(WM_SHOW_ABOUT, "About"),
            TrayAction::Exit => self.request_close("the tray"),
            TrayAction::None => {}
        }
    }

    fn prepare_settings_dialog(&mut self) -> Option<(HWND, Settings)> {
        if self.settings_dialog_open || self.about_dialog_open {
            return None;
        }
        self.hide_overlay();
        self.settings_dialog_open = true;
        Some((self.hwnd, self.settings.clone()))
    }

    fn prepare_about_dialog(&mut self) -> Option<(ResolvedTheme, alttabio::settings::IconColor)> {
        if self.about_dialog_open || self.settings_dialog_open {
            return None;
        }
        self.hide_overlay();
        self.about_dialog_open = true;
        Some((self.resolved_theme, self.settings.appearance.icon))
    }

    fn request_modal_dialog(&self, message: u32, name: &str) {
        let result = unsafe {
            // SAFETY: self.hwnd is live and private dialog messages carry no borrowed data.
            PostMessageW(Some(self.hwnd), message, WPARAM(0), LPARAM(0))
        };
        if let Err(error) = result {
            eprintln!("Could not request the {name} dialog: {error}");
        }
    }

    fn apply_settings(&mut self, settings: Settings, previous_autostart: startup::AutostartStatus) {
        let previous_settings = self.settings.clone();
        let icon_changed = settings.appearance.icon != previous_settings.appearance.icon;
        let old_hook_settings = hook_settings(&previous_settings);
        let new_hook_settings = hook_settings(&settings);
        let autostart_changed = settings.general.autostart != previous_autostart.enabled
            || (!settings.general.autostart && previous_autostart.task_exists);
        if autostart_changed && let Err(error) = startup::set_enabled(settings.general.autostart) {
            self.show_error(&error);
            return;
        }
        if let Err(error) = self.settings_store.save(&settings) {
            let rollback_error = autostart_changed
                .then(|| startup::set_enabled(previous_autostart.enabled).err())
                .flatten();
            let message = rollback_error.map_or(error.clone(), |rollback_error| {
                format!("{error}\n\nAutostart rollback also failed: {rollback_error}")
            });
            self.show_error(&message);
            return;
        }

        if old_hook_settings != new_hook_settings || self.hooks.is_none() {
            self.hooks = None;
            match HookThread::start(self.hwnd, new_hook_settings) {
                Ok(hooks) => self.hooks = Some(hooks),
                Err(error) => {
                    let hook_rollback = HookThread::start(self.hwnd, old_hook_settings);
                    if let Ok(hooks) = hook_rollback {
                        self.hooks = Some(hooks);
                    }
                    let settings_rollback = self.settings_store.save(&previous_settings);
                    let autostart_rollback = autostart_changed
                        .then(|| startup::set_enabled(previous_autostart.enabled))
                        .transpose();
                    let mut message =
                        format!("The new input-hook settings could not be activated. {error}");
                    if self.hooks.is_none() {
                        message.push_str("\n\nThe previous input hooks could not be restored.");
                    }
                    if let Err(rollback_error) = settings_rollback {
                        message.push_str("\n\nSettings rollback also failed: ");
                        message.push_str(&rollback_error);
                    }
                    if let Err(rollback_error) = autostart_rollback {
                        message.push_str("\n\nAutostart rollback also failed: ");
                        message.push_str(&rollback_error);
                    }
                    self.show_error(&message);
                    return;
                }
            }
        }

        self.preview = None;
        self.settings = settings;
        if icon_changed {
            let icon_result = self
                .tray
                .as_mut()
                .map(|tray| tray.set_icon(self.settings.appearance.icon))
                .transpose();
            if let Err(error) = icon_result {
                self.show_error(&format!("Could not update the tray icon. {error}"));
            }
        }
        if let Err(error) = self.refresh_theme() {
            self.show_error(&format!("Could not update the overlay theme. {error}"));
        }
        if self.dwm_preview && self.settings.appearance.preview {
            self.preview = Some(DwmPreview::new(
                self.hwnd,
                self.settings.appearance.full_desktop_preview,
                self.settings.appearance.compact_list,
            ));
        }
        self.request_redraw();
    }

    fn resize_content(&mut self, width: u32, height: u32) {
        if let Err(error) = self.renderer.resize(width, height) {
            eprintln!("Could not resize the Direct2D target: {error}");
        }
        if let Some(preview) = &mut self.preview
            && let Err(error) = preview.update()
        {
            eprintln!("Could not resize the DWM preview: {error}");
        }
    }

    fn sync_content_size(&mut self) {
        let mut client = RECT::default();
        let result = unsafe {
            // SAFETY: self.hwnd is live and client is writable for the synchronous query.
            windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd, &raw mut client)
        };
        if let Err(error) = result {
            eprintln!("Could not read the overlay size: {error}");
            return;
        }
        let width = u32::try_from(client.right.saturating_sub(client.left)).unwrap_or_default();
        let height = u32::try_from(client.bottom.saturating_sub(client.top)).unwrap_or_default();
        self.resize_content(width, height);
    }

    fn shutdown(&mut self) {
        if let Some(preview) = &mut self.preview {
            preview.clear();
        }
        self.tray = None;
        self.hooks = None;
    }

    fn request_close(&self, source: &str) {
        let result = unsafe {
            // SAFETY: self.hwnd is live and the private message carries no borrowed data.
            PostMessageW(Some(self.hwnd), WM_DESTROY_APP, WPARAM(0), LPARAM(0))
        };
        if let Err(error) = result {
            eprintln!("Could not request AltTabio closure from {source}: {error}");
        }
    }

    fn refresh_theme(&mut self) -> Result<bool> {
        let resolved_theme = resolve_current_theme(self.settings.appearance.theme);
        let changed = resolved_theme != self.resolved_theme;
        if changed {
            self.resolved_theme = resolved_theme;
            self.renderer.set_theme(resolved_theme);
            if let Some(tray) = self.tray.as_mut() {
                tray.set_theme(resolved_theme);
            }
        }
        apply_window_appearance(
            self.hwnd,
            self.settings.appearance.visible_borders,
            resolved_theme,
        )?;
        Ok(changed)
    }

    fn show_error(&self, message: &str) {
        show_error_for_window(self.hwnd, message);
    }

    fn handle_input_action(&mut self, action: InputAction) {
        match action {
            InputAction::Switch(delta) => {
                if self.is_visible() {
                    self.switcher.select_next(delta);
                    self.request_redraw();
                } else {
                    self.show_overlay();
                    self.switcher.select_next(delta);
                    self.request_redraw();
                }
            }
            InputAction::Navigate(delta) => {
                if self.is_visible() {
                    self.switcher.select_bounded(delta);
                    self.request_redraw();
                }
            }
            InputAction::SelectFirst => {
                if self.is_visible() {
                    self.switcher.select_first();
                    self.request_redraw();
                }
            }
            InputAction::SelectLast => {
                if self.is_visible() {
                    self.switcher.select_last();
                    self.request_redraw();
                }
            }
            InputAction::DismissOverlay => self.hide_overlay(),
            InputAction::CloseSelected => {
                if self.is_visible() {
                    self.execute_selected(WindowCommand::Close);
                }
            }
            InputAction::WindowCommand(command) => {
                if self.is_visible() {
                    self.execute_selected(command);
                }
            }
            InputAction::KeyPressed(virtual_key) => {
                self.handle_key(usize::from(virtual_key), LPARAM(0));
            }
            InputAction::ActivateVisiblePosition(position) => {
                if self.is_visible() && self.switcher.select_visible_position(position) {
                    self.activate_selected();
                }
            }
            InputAction::ActivateSelected
            | InputAction::AltReleased
            | InputAction::RightButtonReleased => {
                self.activate_for_input(action);
            }
            InputAction::RightButtonPressed => self.show_overlay(),
            InputAction::MouseWheel(delta) => {
                if self.is_visible() {
                    self.switcher.select_next(if delta > 0 { -1 } else { 1 });
                    self.request_redraw();
                }
            }
            InputAction::AppendSearchCharacter(character) => {
                if self.search_active() {
                    self.switcher.append_filter_character(character);
                    self.request_redraw();
                }
            }
            InputAction::BackspaceSearch => {
                if self.search_active() {
                    self.switcher.backspace_filter();
                    self.request_redraw();
                }
            }
        }
    }

    fn handle_key(&mut self, virtual_key: usize, lparam: LPARAM) {
        let key = u16::try_from(virtual_key).unwrap_or_default();
        if let Some(position) = number_position(key) {
            if self.switcher.select_visible_position(position) {
                self.activate_selected();
            }
            return;
        }
        if let Some(command) = command_for_key_event(key, lparam) {
            self.execute_selected(command);
            return;
        }
        match key {
            value if value == VK_RETURN.0 => self.activate_selected(),
            value if value == VK_ESCAPE.0 => self.hide_overlay(),
            value if value == VK_DOWN.0 || value == VK_RIGHT.0 => self.switcher.select_bounded(1),
            value if value == VK_UP.0 || value == VK_LEFT.0 => self.switcher.select_bounded(-1),
            value if value == VK_TAB.0 => {
                self.switcher
                    .select_next(if key_is_down(VK_SHIFT.0) { -1 } else { 1 });
            }
            value if value == VK_HOME.0 => self.switcher.select_first(),
            value if value == VK_END.0 => self.switcher.select_last(),
            _ => return,
        }
        self.request_redraw();
    }

    fn handle_character(&mut self, value: usize) {
        if !self.search_active() {
            return;
        }
        let value = u32::try_from(value).unwrap_or_default();
        if value == u32::from(VK_BACK.0) {
            self.switcher.backspace_filter();
        } else if let Some(character) = char::from_u32(value)
            && !character.is_control()
        {
            self.switcher.append_filter_character(character);
        } else {
            return;
        }
        self.request_redraw();
    }

    fn execute_selected(&mut self, command: WindowCommand) {
        let Some(request) = selected_window_command(&self.switcher, command) else {
            return;
        };
        if !execute_window_command(request.command, request.window_handle) {
            eprintln!("Could not execute {command:?} for the selected window");
            return;
        }
        match enumerate_switchable_windows(&self.settings) {
            Ok(tasks) => {
                let close_refresh_target = close_refresh_target_after_enumeration(request, &tasks);
                self.switcher.set_tasks(tasks);
                if let Some(window_handle) = close_refresh_target {
                    self.schedule_close_refresh(window_handle);
                }
            }
            Err(error) => eprintln!("Could not refresh windows after {command:?}: {error}"),
        }
        self.close_button.reset();
        self.request_redraw();
    }

    fn schedule_close_refresh(&mut self, window_handle: isize) {
        if !self.close_refresh_tracker.track(window_handle) {
            return;
        }
        let timer_id = unsafe {
            // SAFETY: the live overlay HWND owns this timer and no callback pointer is retained.
            SetTimer(
                Some(self.hwnd),
                CLOSE_REFRESH_TIMER_ID,
                CLOSE_REFRESH_DELAY_MS,
                None,
            )
        };
        if timer_id == 0 {
            self.close_refresh_tracker.clear();
            eprintln!(
                "Could not schedule a follow-up refresh after closing a window: {}",
                Error::from_thread()
            );
        }
    }

    fn handle_close_refresh_timer(&mut self) {
        let keep_refreshing = match enumerate_switchable_windows(&self.settings) {
            Ok(tasks) => {
                let keep_refreshing = self.close_refresh_tracker.reconcile(&tasks);
                self.switcher.set_tasks(tasks);
                keep_refreshing
            }
            Err(error) => {
                eprintln!("Could not refresh windows after closing: {error}");
                self.close_refresh_tracker.advance_after_enumeration_error()
            }
        };
        if !keep_refreshing {
            let result = unsafe {
                // SAFETY: this handles the timer owned by the live overlay HWND.
                KillTimer(Some(self.hwnd), CLOSE_REFRESH_TIMER_ID)
            };
            if let Err(error) = result {
                eprintln!("Could not stop the close refresh timer: {error}");
            }
        }
        self.request_redraw();
    }

    fn handle_mouse_move(&mut self, lparam: LPARAM) {
        if !self.mouse_selection_armed {
            let mut cursor = POINT::default();
            let current = unsafe {
                // SAFETY: `cursor` is writable for the call.
                GetCursorPos(&raw mut cursor)
            };
            if current.is_err() || self.mouse_origin == Some(cursor) {
                return;
            }
            self.mouse_selection_armed = true;
        }
        self.track_mouse_leave();

        let mut hit = self.hit_test(lparam);
        let mut needs_redraw = false;
        if self.settings.general.mouse_over_selection
            && !self.close_button.is_pressed()
            && let Some(TaskListHit::Task(position)) = hit
            && select_hovered_position(&mut self.switcher, position)
        {
            needs_redraw = true;
            hit = self.hit_test(lparam);
        }
        let close_target = close_target_for_hit(&self.switcher, hit);
        needs_redraw |= self.close_button.update_hover(close_target);
        if needs_redraw {
            self.request_redraw();
        }
    }

    fn handle_mouse_leave(&mut self) {
        self.mouse_leave_tracked = false;
        if self.close_button.update_hover(None) {
            self.request_redraw();
        }
    }

    fn handle_button_down(&mut self, lparam: LPARAM) {
        let hit = self.hit_test(lparam);
        let target = close_target_for_hit(&self.switcher, hit);
        let Some(target) = target else {
            return;
        };
        self.close_button.press(target);
        unsafe {
            // SAFETY: the overlay HWND is live; a null previous HWND is a valid SetCapture result.
            let _previous_capture = SetCapture(self.hwnd);
        }
        self.request_redraw();
    }

    fn handle_button_up(&mut self, lparam: LPARAM) {
        let hit = self.hit_test(lparam);
        if self.close_button.is_pressed() {
            let target = close_target_for_hit(&self.switcher, hit);
            let command = self.close_button.release(target);
            let release_result = unsafe {
                // SAFETY: this UI thread acquired mouse capture when the close button was pressed.
                ReleaseCapture()
            };
            if let Err(error) = release_result {
                eprintln!("Could not release close-button mouse capture: {error}");
            }
            self.request_redraw();
            if let Some(command) = command {
                self.execute_selected(command);
            }
            return;
        }

        if let Some(TaskListHit::Task(position)) = hit
            && self.switcher.select_visible_position(position)
        {
            self.activate_selected();
        }
    }

    fn track_mouse_leave(&mut self) {
        if self.mouse_leave_tracked {
            return;
        }
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or_default(),
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        let result = unsafe {
            // SAFETY: `tracking` is writable and the overlay HWND remains live for the call.
            TrackMouseEvent(&raw mut tracking)
        };
        match result {
            Ok(()) => self.mouse_leave_tracked = true,
            Err(error) => eprintln!("Could not track close-button mouse leave: {error}"),
        }
    }

    fn hit_test(&mut self, lparam: LPARAM) -> Option<TaskListHit> {
        let (x, y) = mouse_coordinates(lparam);
        Renderer::hit_test(
            self.hwnd,
            &mut self.switcher,
            x,
            y,
            self.settings.appearance.compact_list,
        )
    }

    fn handle_task_context_menu(&mut self, lparam: LPARAM) {
        let Some(hit) = self.hit_test(lparam) else {
            return;
        };
        let position = hit.position();
        if !self.switcher.select_visible_position(position) {
            return;
        }
        self.request_redraw();
        self.context_menu_open = true;
        let command = show_window_command_menu(self.hwnd);
        self.context_menu_open = false;
        if let Some(command) = command {
            self.execute_selected(command);
        }
    }

    fn handle_dpi_changed(&mut self, lparam: LPARAM) {
        let suggested = unsafe {
            // SAFETY: WM_DPICHANGED guarantees lParam points to a RECT for the callback duration.
            (lparam.0 as *const RECT).as_ref()
        };
        if let Some(suggested) = suggested {
            let bounds = match monitor_work_area_from_rect(*suggested) {
                Ok(work_area) => overlay_bounds_for_dpi_change(*suggested, work_area),
                Err(error) => {
                    eprintln!("Could not resolve the monitor for the DPI change: {error}");
                    *suggested
                }
            };
            let result = unsafe {
                // SAFETY: `self.hwnd` is live and bounds came from the target monitor selected by
                // the WM_DPICHANGED rectangle.
                SetWindowPos(
                    self.hwnd,
                    None,
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            };
            if let Err(error) = result {
                eprintln!("Could not apply the DPI change: {error}");
            }
        }
        self.sync_content_size();
        self.request_redraw();
    }

    fn show_overlay(&mut self) {
        match enumerate_switchable_windows(&self.settings) {
            Ok(tasks) => prepare_switcher_for_open(&mut self.switcher, tasks),
            Err(error) => {
                self.set_hook_search_active(false);
                self.set_hook_overlay_active(self.is_visible());
                eprintln!("Could not enumerate windows: {error}");
                self.reset_hook_gestures();
                return;
            }
        }
        self.reset_mouse_selection();
        if let Err(error) = position_on_cursor_monitor(self.hwnd) {
            eprintln!("Could not position the overlay: {error}");
        }
        unsafe {
            // SAFETY: the HWND is live and owned by this UI thread.
            let _was_visible = ShowWindow(self.hwnd, SW_SHOW);
            let _foreground = SetForegroundWindow(self.hwnd);
            if let Err(error) = SetFocus(Some(self.hwnd)) {
                eprintln!("Could not focus the overlay: {error}");
            }
        }
        self.sync_content_size();
        self.set_hook_search_active(true);
        self.set_hook_overlay_active(true);
        self.request_redraw();
    }

    fn hide_overlay(&mut self) {
        self.set_hook_search_active(false);
        self.reset_hook_gestures();
        if let Some(preview) = &mut self.preview {
            preview.clear();
        }
        if self.close_button.is_pressed() {
            self.close_button.reset();
            let result = unsafe {
                // SAFETY: this UI thread owns capture only while its close button is pressed.
                ReleaseCapture()
            };
            if let Err(error) = result {
                eprintln!("Could not release close-button mouse capture while hiding: {error}");
            }
        } else {
            self.close_button.reset();
        }
        self.mouse_leave_tracked = false;
        unsafe {
            // SAFETY: the HWND is live and owned by this UI thread.
            let _was_visible = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.set_hook_overlay_active(false);
        if self.exit_when_hidden {
            self.request_close("the preview window");
        }
    }

    fn reset_hook_gestures(&self) {
        if let Some(hooks) = &self.hooks
            && let Err(error) = hooks.reset_gestures()
        {
            eprintln!("{error}");
        }
    }

    fn activate_selected(&mut self) {
        let Some(target) = self.switcher.selected_task().map(|task| task.window_handle) else {
            return;
        };
        self.activate_target(target);
    }

    fn activate_for_input(&mut self, action: InputAction) {
        let mut state = OverlayActivationState {
            visible: self.is_visible(),
            context_menu_open: self.context_menu_open,
            release: ReleaseActivationSettings {
                alt_switches: self.settings.general.release_alt_switches,
                right_button_switches: self.settings.general.release_right_button_switches,
            },
        };
        let selected = self.switcher.selected_task().map(|task| task.window_handle);
        if let Some(target) = take_activation_target(action, &mut state, selected) {
            self.activate_target(target);
        }
    }

    fn activate_target(&mut self, target: isize) {
        self.hide_overlay();
        let target = HWND(target as *mut c_void);
        if !activate_window(target) {
            unsafe {
                // SAFETY: the HWND is live and owned by this UI thread.
                let _was_visible = ShowWindow(self.hwnd, SW_SHOW);
            }
            self.set_hook_search_active(true);
            self.set_hook_overlay_active(true);
            self.request_redraw();
        }
    }

    fn paint(&mut self) {
        let mut paint = PAINTSTRUCT::default();
        unsafe {
            // SAFETY: `paint` is writable and BeginPaint/EndPaint are paired for this WM_PAINT.
            BeginPaint(self.hwnd, &raw mut paint);
        }
        let render_options = RenderOptions::from(&self.settings.appearance);
        let selected_target = self.switcher.selected_task().map(|task| task.window_handle);
        if let Err(error) = self.renderer.draw(
            self.hwnd,
            &self.switcher,
            self.preview.as_ref().and_then(DwmPreview::frame),
            render_options,
            self.close_button.visual_state(selected_target),
        ) {
            eprintln!("Could not render the overlay: {error}");
        }
        Renderer::draw_icons(self.hwnd, paint.hdc, &self.switcher, render_options);
        unsafe {
            // SAFETY: this exactly balances the successful BeginPaint call above.
            let _ended = EndPaint(self.hwnd, &raw const paint);
        }
    }

    fn request_redraw(&mut self) {
        let source = self
            .switcher
            .selected_task()
            .map(|task| HWND(task.window_handle as *mut c_void));
        if let Some(preview) = &mut self.preview
            && let Err(error) = preview.set_source(source)
        {
            eprintln!("Could not update the DWM preview: {error}");
        }
        let invalidated = unsafe {
            // SAFETY: the HWND is live; a null rectangle invalidates the complete client area.
            InvalidateRect(Some(self.hwnd), None, false)
        };
        if !invalidated.as_bool() {
            eprintln!("Could not invalidate the overlay: {}", Error::from_thread());
        }
    }

    fn search_active(&self) -> bool {
        self.settings.general.typed_search && self.is_visible()
    }

    fn set_hook_search_active(&self, overlay_visible: bool) {
        if let Some(hooks) = &self.hooks {
            hooks.set_search_active(overlay_visible && self.settings.general.typed_search);
        }
    }

    fn set_hook_overlay_active(&self, active: bool) {
        if let Some(hooks) = &self.hooks {
            hooks.set_overlay_active(active);
        }
    }

    fn reset_mouse_selection(&mut self) {
        let mut cursor = POINT::default();
        self.mouse_origin = unsafe {
            // SAFETY: `cursor` is writable for the call.
            GetCursorPos(&raw mut cursor).ok().map(|()| cursor)
        };
        self.mouse_selection_armed = false;
        self.mouse_leave_tracked = false;
        self.close_button.reset();
    }

    fn is_visible(&self) -> bool {
        unsafe {
            // SAFETY: the HWND is live and IsWindowVisible has no pointer preconditions.
            IsWindowVisible(self.hwnd).as_bool()
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
        if message == WM_NCCREATE {
            let create = unsafe {
                // SAFETY: WM_NCCREATE guarantees lParam points to CREATESTRUCTW for this callback.
                (lparam.0 as *const CREATESTRUCTW).as_ref()
            }?;
            let host = create.lpCreateParams.cast::<AppHost>();
            if host.is_null() {
                return Some(LRESULT(0));
            }
            let host_ref = unsafe {
                // SAFETY: host is the Box allocation passed to CreateWindowExW and remains live.
                &*host
            };
            let Ok(mut app) = host_ref.state.try_borrow_mut() else {
                return Some(LRESULT(0));
            };
            app.hwnd = hwnd;
            drop(app);
            unsafe {
                // SAFETY: host remains live through the message loop.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, host as isize);
            }
            return Some(LRESULT(1));
        }
        if message == WM_NCCALCSIZE {
            return Some(LRESULT(0));
        }
        if message == WM_NCACTIVATE {
            return Some(LRESULT(1));
        }
        let host = unsafe {
            // SAFETY: user data is either zero or the live AppHost pointer installed above.
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppHost).as_ref()
        }?;
        if message == WM_DESTROY_APP {
            let result = unsafe {
                // SAFETY: the posted message runs on the UI thread that owns hwnd.
                DestroyWindow(hwnd)
            };
            if let Err(error) = result {
                eprintln!("Could not close AltTabio: {error}");
            }
            return Some(LRESULT(0));
        }
        if message == WM_DESTROY {
            if let Ok(mut app) = host.state.try_borrow_mut() {
                app.shutdown();
            }
            unsafe {
                // SAFETY: called on the UI thread to terminate its own message loop.
                PostQuitMessage(0);
            }
            return Some(LRESULT(0));
        }
        if message == WM_NCDESTROY {
            unsafe {
                // SAFETY: clearing user data prevents later messages from observing host.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            return None;
        }
        if is_modal_dialog_message(message) {
            match message {
                WM_SHOW_SETTINGS => host.show_settings(),
                WM_SHOW_ABOUT => host.show_about(),
                _ => {}
            }
            return Some(LRESULT(0));
        }
        let Ok(mut app) = host.state.try_borrow_mut() else {
            return None;
        };
        app.handle_message(message, wparam, lparam)
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| default_window_proc(hwnd, message, wparam, lparam))
}

const fn is_modal_dialog_message(message: u32) -> bool {
    matches!(message, WM_SHOW_SETTINGS | WM_SHOW_ABOUT)
}

fn show_error_for_window(owner: HWND, message: &str) {
    let text = null_terminated(message);
    unsafe {
        // SAFETY: both UTF-16 buffers remain alive and null terminated for the synchronous call.
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            w!("AltTabio"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn default_window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // SAFETY: forwarding unhandled messages with the original values is the window-procedure
        // contract.
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn register_window_class(instance: HINSTANCE) -> Result<()> {
    let cursor = unsafe {
        // SAFETY: IDC_ARROW is a predefined shared cursor and no ownership is transferred.
        LoadCursorW(None, IDC_ARROW)
    }?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or_default(),
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: WINDOW_CLASS,
        ..WNDCLASSEXW::default()
    };
    let atom = unsafe {
        // SAFETY: `class` and its static class-name string remain valid for the synchronous call.
        RegisterClassExW(&raw const class)
    };
    if atom == 0 {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

fn module_instance() -> Result<HINSTANCE> {
    let module = unsafe {
        // SAFETY: None requests a borrowed handle for this executable module.
        GetModuleHandleW(None)
    }?;
    Ok(HINSTANCE(module.0))
}

fn run_message_loop() -> Result<()> {
    let mut message = MSG::default();
    loop {
        let result = unsafe {
            // SAFETY: `message` is writable for the call and this is the owning UI message loop.
            GetMessageW(&raw mut message, None, 0, 0)
        };
        if result.0 == -1 {
            return Err(Error::from_thread());
        }
        if result.0 == 0 {
            return Ok(());
        }
        unsafe {
            // SAFETY: GetMessageW initialized `message` for this UI thread.
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

struct EnumerationContext {
    tasks: Vec<SwitchTask>,
    current_process_id: u32,
    current_monitor: Option<HMONITOR>,
}

fn enumerate_switchable_windows(settings: &Settings) -> Result<Vec<SwitchTask>> {
    let current_monitor = if settings.monitor.use_current_monitor_filter {
        let mut cursor = POINT::default();
        unsafe {
            // SAFETY: cursor is writable for the synchronous call.
            GetCursorPos(&raw mut cursor)?;
        }
        Some(unsafe {
            // SAFETY: cursor was initialized above and nearest-monitor fallback guarantees a result.
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        })
    } else {
        None
    };
    let mut context = EnumerationContext {
        tasks: Vec::new(),
        current_process_id: std::process::id(),
        current_monitor,
    };
    unsafe {
        // SAFETY: EnumWindows is synchronous, so `context` remains exclusively borrowed and live
        // for every invocation of enum_window.
        EnumWindows(
            Some(enum_window),
            LPARAM((&raw mut context).cast::<c_void>() as isize),
        )?;
    }
    Ok(context.tasks)
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe {
        // SAFETY: enumerate_switchable_windows passes a live exclusive EnumerationContext pointer
        // and EnumWindows invokes callbacks synchronously on this thread.
        (lparam.0 as *mut EnumerationContext).as_mut()
    };
    let Some(context) = context else {
        return false.into();
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        if let Some(task) = create_switch_task(
            hwnd,
            context.current_process_id,
            context.current_monitor,
            context.tasks.len(),
        ) {
            context.tasks.push(task);
        }
    }));
    result.is_ok().into()
}

fn create_switch_task(
    hwnd: HWND,
    current_process_id: u32,
    current_monitor: Option<HMONITOR>,
    index: usize,
) -> Option<SwitchTask> {
    let title = window_title(hwnd);
    let class_name = window_class_name(hwnd);
    let mut process_id = 0;
    unsafe {
        // SAFETY: `process_id` is writable and HWND is supplied by EnumWindows.
        GetWindowThreadProcessId(hwnd, Some(&raw mut process_id));
    }
    let extended_style = unsafe {
        // SAFETY: HWND is supplied by EnumWindows and GWL_EXSTYLE requests a scalar style value.
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
    };
    let shell = unsafe {
        // SAFETY: GetShellWindow has no preconditions and returns a borrowed HWND.
        GetShellWindow()
    };
    let has_owner = unsafe {
        // SAFETY: HWND is supplied by EnumWindows; a missing owner is represented as an error/null.
        GetWindow(hwnd, GW_OWNER).is_ok()
    };
    let matches_monitor_filter = current_monitor.is_none_or(|current_monitor| {
        (unsafe {
            // SAFETY: HWND is supplied by EnumWindows and nearest-monitor fallback is requested.
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
        }) == current_monitor
    });
    let eligibility = WindowEligibility {
        title: &title,
        class_name: &class_name,
        is_visible: unsafe {
            // SAFETY: HWND is supplied by EnumWindows.
            IsWindowVisible(hwnd).as_bool()
        },
        is_current_process: excludes_current_process_window(
            process_id,
            current_process_id,
            &class_name,
        ) || hwnd == shell,
        is_cloaked: is_cloaked(hwnd),
        is_tool_window: (extended_style & isize::try_from(WS_EX_TOOLWINDOW.0).unwrap_or_default())
            != 0,
        has_owner,
        is_app_window: (extended_style & isize::try_from(WS_EX_APPWINDOW.0).unwrap_or_default())
            != 0,
        matches_monitor_filter,
    };
    if !is_switchable_window(&eligibility) {
        return None;
    }

    Some(
        SwitchTask::new(
            index + 1,
            hwnd.0 as isize,
            &title,
            &process_name(process_id),
        )
        .with_icon_handle(window_icon(hwnd)),
    )
}

fn excludes_current_process_window(
    process_id: u32,
    current_process_id: u32,
    class_name: &str,
) -> bool {
    process_id == current_process_id
        && !matches!(
            class_name,
            about_dialog::WINDOW_CLASS_NAME | settings_dialog::WINDOW_CLASS_NAME
        )
}

fn window_icon(hwnd: HWND) -> isize {
    for size in [ICON_BIG, ICON_SMALL2, ICON_SMALL] {
        let mut icon = 0_usize;
        let sent = unsafe {
            // SAFETY: hwnd is supplied by EnumWindows and icon is writable for the bounded,
            // abort-if-hung synchronous query.
            SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(size as usize),
                LPARAM(0),
                SMTO_BLOCK | SMTO_ABORTIFHUNG,
                75,
                Some(&raw mut icon),
            )
        };
        if sent.0 != 0 && icon != 0 {
            return isize::try_from(icon).unwrap_or_default();
        }
    }
    for class_index in [GCLP_HICON, GCLP_HICONSM] {
        let icon = unsafe {
            // SAFETY: hwnd is supplied by EnumWindows and class icon handles remain owned by the
            // registered window class.
            GetClassLongPtrW(hwnd, class_index)
        };
        if icon != 0 {
            return isize::try_from(icon).unwrap_or_default();
        }
    }
    0
}

fn window_title(hwnd: HWND) -> String {
    let length = unsafe {
        // SAFETY: HWND is supplied by EnumWindows.
        GetWindowTextLengthW(hwnd)
    };
    if length <= 0 {
        return String::new();
    }
    let capacity = usize::try_from(length)
        .unwrap_or_default()
        .saturating_add(1);
    let mut buffer = vec![0_u16; capacity];
    let written = unsafe {
        // SAFETY: `buffer` is writable and HWND is supplied by EnumWindows.
        GetWindowTextW(hwnd, &mut buffer)
    };
    utf16_prefix(&buffer, written)
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0_u16; 256];
    let written = unsafe {
        // SAFETY: `buffer` is writable and HWND is supplied by EnumWindows.
        GetClassNameW(hwnd, &mut buffer)
    };
    utf16_prefix(&buffer, written)
}

fn utf16_prefix(buffer: &[u16], written: i32) -> String {
    let length = usize::try_from(written).unwrap_or_default();
    String::from_utf16_lossy(buffer.get(..length).unwrap_or_default())
}

fn process_name(process_id: u32) -> String {
    let handle = match unsafe {
        // SAFETY: OpenProcess is called with query-only access for the process id from EnumWindows.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
    } {
        Ok(handle) => OwnedHandle(handle),
        Err(_) => return String::new(),
    };
    let mut buffer = vec![0_u16; 32_768];
    let mut length = u32::try_from(buffer.len()).unwrap_or_default();
    let result = unsafe {
        // SAFETY: the handle is live and query-only; the UTF-16 buffer and length are writable for
        // the synchronous call.
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    };
    if result.is_err() {
        return String::new();
    }
    let length = usize::try_from(length).unwrap_or_default();
    let path = String::from_utf16_lossy(buffer.get(..length).unwrap_or_default());
    Path::new(&path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let result = unsafe {
            // SAFETY: this guard uniquely owns the process HANDLE and closes it exactly once.
            CloseHandle(self.0)
        };
        if let Err(error) = result {
            eprintln!("Could not close a process handle: {error}");
        }
    }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0_u32;
    let result = unsafe {
        // SAFETY: `cloaked` is writable for its exact byte size and HWND is supplied by EnumWindows.
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            u32::try_from(size_of::<u32>()).unwrap_or_default(),
        )
    };
    result.is_ok() && cloaked != 0
}

fn position_on_cursor_monitor(hwnd: HWND) -> Result<()> {
    let mut cursor = POINT::default();
    unsafe {
        // SAFETY: `cursor` is writable for the call.
        GetCursorPos(&raw mut cursor)?;
    }
    let monitor = unsafe {
        // SAFETY: the POINT value is initialized and the flag requests a nearest-monitor fallback.
        MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
    };
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or_default(),
        ..MONITORINFO::default()
    };
    let success = unsafe {
        // SAFETY: `monitor_info` is writable with a correct cbSize and monitor is the handle returned
        // by MonitorFromPoint.
        GetMonitorInfoW(monitor, &raw mut monitor_info)
    };
    if !success.as_bool() {
        return Err(Error::from_thread());
    }
    let area = monitor_info.rcWork;
    let bounds = overlay_bounds(area);
    unsafe {
        // SAFETY: HWND is live; the calculated dimensions are within the selected work area.
        SetWindowPos(
            hwnd,
            None,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )?;
    }
    Ok(())
}

fn monitor_work_area_from_rect(rectangle: RECT) -> Result<RECT> {
    let monitor = unsafe {
        // SAFETY: rectangle is initialized and the fallback flag requests the nearest monitor.
        MonitorFromRect(&raw const rectangle, MONITOR_DEFAULTTONEAREST)
    };
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or_default(),
        ..MONITORINFO::default()
    };
    let success = unsafe {
        // SAFETY: monitor_info is writable with a correct cbSize and monitor came from
        // MonitorFromRect.
        GetMonitorInfoW(monitor, &raw mut monitor_info)
    };
    if !success.as_bool() {
        return Err(Error::from_thread());
    }
    Ok(monitor_info.rcWork)
}

const fn overlay_bounds_for_dpi_change(_suggested: RECT, work_area: RECT) -> RECT {
    // The suggested rectangle preserves the window's old logical size. AltTabio instead owns a
    // monitor-relative size, so scaling that rectangle can make the overlay fill a high-DPI screen.
    overlay_bounds(work_area)
}

const fn overlay_bounds(work_area: RECT) -> RECT {
    let area_width = work_area.right.saturating_sub(work_area.left);
    let area_height = work_area.bottom.saturating_sub(work_area.top);
    let width = area_width.saturating_mul(5) / 8;
    let height = area_height.saturating_mul(5) / 8;
    let left = work_area
        .left
        .saturating_add(area_width.saturating_sub(width) / 2);
    let top = work_area
        .top
        .saturating_add(area_height.saturating_sub(height) / 2);
    RECT {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    }
}

fn resolve_current_theme(theme: Theme) -> ResolvedTheme {
    if theme != Theme::Auto {
        return resolve(theme, ResolvedTheme::Light);
    }
    let windows_theme = match read_windows_app_theme() {
        Ok(theme) => theme,
        Err(error) => {
            eprintln!("Could not read the Windows app theme; using Light: {error}");
            ResolvedTheme::Light
        }
    };
    resolve(theme, windows_theme)
}

fn read_windows_app_theme() -> Result<ResolvedTheme> {
    let mut apps_use_light_theme = 1_u32;
    let mut value_size = u32::try_from(size_of::<u32>()).unwrap_or_default();
    let status = unsafe {
        // SAFETY: the predefined current-user key is borrowed, both strings are static and
        // null-terminated, and the DWORD buffer and byte count remain writable for the call.
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut apps_use_light_theme).cast()),
            Some(&raw mut value_size),
        )
    };
    status.ok()?;
    Ok(if apps_use_light_theme == 0 {
        ResolvedTheme::Dark
    } else {
        ResolvedTheme::Light
    })
}

fn apply_window_appearance(hwnd: HWND, visible_borders: bool, theme: ResolvedTheme) -> Result<()> {
    let preference = DWMWCP_ROUND;
    let border_color = compositor_border_color(visible_borders, theme);
    let use_dark_mode = i32::from(theme == ResolvedTheme::Dark);
    unsafe {
        // SAFETY: hwnd is the live top-level overlay window and the preference pointer remains
        // valid for the duration of this synchronous compositor call.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const preference).cast(),
            u32::try_from(std::mem::size_of_val(&preference)).unwrap_or(u32::MAX),
        )?;
        // SAFETY: use_dark_mode is a valid BOOL-compatible value and the pointer remains valid for
        // the duration of this synchronous compositor call.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const use_dark_mode).cast(),
            u32::try_from(std::mem::size_of_val(&use_dark_mode)).unwrap_or(u32::MAX),
        )?;
        // SAFETY: hwnd is unchanged and border_color is a valid COLORREF sentinel accepted by DWM.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const border_color).cast(),
            u32::try_from(std::mem::size_of_val(&border_color)).unwrap_or(u32::MAX),
        )
    }
}

const fn compositor_border_color(visible_borders: bool, theme: ResolvedTheme) -> u32 {
    if visible_borders {
        colorref(theme.palette().window_border)
    } else {
        DWMWA_COLOR_NONE
    }
}

const fn colorref(color: Rgb8) -> u32 {
    color.red as u32 | ((color.green as u32) << 8) | ((color.blue as u32) << 16)
}

fn activate_window(owner: HWND) -> bool {
    let popup = unsafe {
        // SAFETY: owner is a borrowed HWND selected from the current EnumWindows snapshot.
        GetLastActivePopup(owner)
    };
    let popup_is_visible = popup != HWND::default()
        && popup != owner
        && unsafe {
            // SAFETY: popup is the borrowed HWND returned by GetLastActivePopup.
            IsWindowVisible(popup).as_bool()
        };
    let target = activation_target(owner, popup, popup_is_visible);

    if unsafe {
        // SAFETY: target is a borrowed top-level or owned-popup HWND.
        IsIconic(target).as_bool()
    } {
        unsafe {
            // SAFETY: target is a borrowed HWND; ShowWindowAsync does not transfer ownership.
            let _was_visible = ShowWindowAsync(target, SW_RESTORE);
        }
    }

    let current_thread = unsafe {
        // SAFETY: GetCurrentThreadId has no preconditions.
        GetCurrentThreadId()
    };
    let foreground = unsafe {
        // SAFETY: GetForegroundWindow has no preconditions and returns a borrowed HWND.
        GetForegroundWindow()
    };
    let foreground_thread = unsafe {
        // SAFETY: foreground is a borrowed HWND and no process-id output is requested.
        GetWindowThreadProcessId(foreground, None)
    };
    let target_thread = unsafe {
        // SAFETY: target is a borrowed HWND and no process-id output is requested.
        GetWindowThreadProcessId(target, None)
    };
    let _foreground_attachment = ThreadInputAttachment::new(current_thread, foreground_thread);
    let _target_attachment = ThreadInputAttachment::new(current_thread, target_thread);

    // Leave keyboard focus to the target's activation handling so it can restore its prior child.
    let mut foreground_set = false;
    for action in activation_actions() {
        match action {
            ActivationAction::BringToTop => {
                if let Err(error) = unsafe {
                    // SAFETY: target is a borrowed top-level or owned-popup HWND.
                    BringWindowToTop(target)
                } {
                    eprintln!("Could not bring the selected window to the top: {error}");
                }
            }
            ActivationAction::SetForeground => {
                foreground_set = unsafe {
                    // SAFETY: target is a borrowed top-level or owned-popup HWND.
                    SetForegroundWindow(target).as_bool()
                };
            }
            ActivationAction::SetActive => {
                if let Err(error) = unsafe {
                    // SAFETY: input queues are attached for the duration of this activation attempt.
                    SetActiveWindow(target)
                } {
                    eprintln!("Could not set the selected window active: {error}");
                }
            }
        }
    }
    foreground_set
        || unsafe {
            // SAFETY: GetForegroundWindow has no preconditions and returns a borrowed HWND.
            GetForegroundWindow()
        } == target
}

fn activation_target(owner: HWND, popup: HWND, popup_is_visible: bool) -> HWND {
    if popup != HWND::default() && popup != owner && popup_is_visible {
        popup
    } else {
        owner
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivationAction {
    BringToTop,
    SetForeground,
    SetActive,
}

const fn activation_actions() -> &'static [ActivationAction] {
    &[
        ActivationAction::BringToTop,
        ActivationAction::SetForeground,
        ActivationAction::SetActive,
    ]
}

struct ThreadInputAttachment {
    source: u32,
    target: u32,
    attached: bool,
}

impl ThreadInputAttachment {
    fn new(source: u32, target: u32) -> Self {
        let attached = source != 0
            && target != 0
            && source != target
            && unsafe {
                // SAFETY: both values are live GUI thread ids queried immediately before this call.
                AttachThreadInput(source, target, true).as_bool()
            };
        Self {
            source,
            target,
            attached,
        }
    }
}

impl Drop for ThreadInputAttachment {
    fn drop(&mut self) {
        if self.attached {
            let detached = unsafe {
                // SAFETY: this exactly balances the successful AttachThreadInput call owned by the
                // guard and occurs on the same source thread.
                AttachThreadInput(self.source, self.target, false).as_bool()
            };
            if !detached {
                eprintln!("Could not detach temporary window-activation input queues");
            }
        }
    }
}

fn key_is_down(virtual_key: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    unsafe {
        // SAFETY: GetKeyState accepts any virtual-key code and has no pointer preconditions.
        GetKeyState(i32::from(virtual_key)) < 0
    }
}

fn hook_settings(settings: &Settings) -> HookSettings {
    HookSettings {
        replace_alt_tab: settings.general.replace_alt_tab,
        replace_win_tab: settings.general.replace_win_tab,
        right_button_wheel_switching: settings.general.right_button_wheel_switching,
        typed_search: settings.general.typed_search,
        search_active: false,
    }
}

fn number_position(virtual_key: u16) -> Option<usize> {
    if (VK_1.0..=VK_9.0).contains(&virtual_key) {
        return Some(usize::from(virtual_key - VK_0.0));
    }
    if (VK_NUMPAD1.0..=VK_NUMPAD9.0).contains(&virtual_key) {
        return Some(usize::from(virtual_key - VK_NUMPAD0.0));
    }
    None
}

const fn command_for_key(virtual_key: u16) -> Option<WindowCommand> {
    match virtual_key {
        value if value == VK_F4.0 => Some(WindowCommand::Close),
        value if value == VK_F5.0 => Some(WindowCommand::Minimize),
        value if value == VK_F6.0 => Some(WindowCommand::Maximize),
        value if value == VK_F7.0 => Some(WindowCommand::Restore),
        value if value == VK_F8.0 => Some(WindowCommand::Terminate),
        value if value == VK_F9.0 => Some(WindowCommand::Run),
        _ => None,
    }
}

fn command_for_key_event(virtual_key: u16, lparam: LPARAM) -> Option<WindowCommand> {
    let command = command_for_key(virtual_key)?;
    if command == WindowCommand::Close && key_was_previously_down(lparam) {
        None
    } else {
        Some(command)
    }
}

const fn key_was_previously_down(lparam: LPARAM) -> bool {
    let previous_key_state_mask = 1_isize << 30;
    lparam.0 & previous_key_state_mask != 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectedWindowCommand {
    command: WindowCommand,
    window_handle: isize,
}

fn selected_window_command(
    switcher: &Switcher,
    command: WindowCommand,
) -> Option<SelectedWindowCommand> {
    switcher.selected_task().map(|task| SelectedWindowCommand {
        command,
        window_handle: task.window_handle,
    })
}

fn close_refresh_target_after_enumeration(
    request: SelectedWindowCommand,
    tasks: &[SwitchTask],
) -> Option<isize> {
    (request.command == WindowCommand::Close
        && tasks
            .iter()
            .any(|task| task.window_handle == request.window_handle))
    .then_some(request.window_handle)
}

fn mouse_coordinates(lparam: LPARAM) -> (i32, i32) {
    let x = low_word_isize(lparam.0).cast_signed();
    let y = high_word_isize(lparam.0).cast_signed();
    (i32::from(x), i32::from(y))
}

fn select_hovered_position(switcher: &mut Switcher, position: usize) -> bool {
    let previous = switcher.selected_visible_index();
    switcher.select_visible_position(position) && switcher.selected_visible_index() != previous
}

fn prepare_switcher_for_open(switcher: &mut Switcher, tasks: impl IntoIterator<Item = SwitchTask>) {
    switcher.clear_filter();
    switcher.set_tasks(tasks);
}

fn close_target_for_hit(switcher: &Switcher, hit: Option<TaskListHit>) -> Option<isize> {
    let TaskListHit::CloseButton(position) = hit? else {
        return None;
    };
    let selected_position = switcher.selected_visible_index()?.checked_add(1)?;
    (position == selected_position).then_some(switcher.selected_task()?.window_handle)
}

fn take_activation_target(
    action: InputAction,
    state: &mut OverlayActivationState,
    selected_window: Option<isize>,
) -> Option<isize> {
    let should_activate = match action {
        InputAction::ActivateSelected => state.visible,
        InputAction::AltReleased => {
            state.release.alt_switches && state.visible && !state.context_menu_open
        }
        InputAction::RightButtonReleased => {
            state.release.right_button_switches && state.visible && !state.context_menu_open
        }
        _ => false,
    };
    if !should_activate {
        return None;
    }

    let target = selected_window?;
    state.visible = false;
    Some(target)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Win32 packs two unsigned 16-bit values into LPARAM and WPARAM words"
)]
const fn low_word_isize(value: isize) -> u16 {
    value as u16
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Win32 packs two unsigned 16-bit values into LPARAM and WPARAM words"
)]
const fn high_word_isize(value: isize) -> u16 {
    (value >> 16) as u16
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Win32 packs a signed 16-bit wheel delta into the high word of WPARAM"
)]
const fn high_word_usize(value: usize) -> u16 {
    (value >> 16) as u16
}

fn null_terminated(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_visible_dialogs_are_not_excluded_from_switching() {
        let current_process_id = 42;

        assert!(!excludes_current_process_window(
            current_process_id,
            current_process_id,
            about_dialog::WINDOW_CLASS_NAME,
        ));
        assert!(!excludes_current_process_window(
            current_process_id,
            current_process_id,
            settings_dialog::WINDOW_CLASS_NAME,
        ));
        assert!(excludes_current_process_window(
            current_process_id,
            current_process_id,
            "AltTabioRustOverlay",
        ));
        assert!(!excludes_current_process_window(
            current_process_id + 1,
            current_process_id,
            "EditorWindow",
        ));
    }

    #[test]
    fn modal_dialog_messages_run_outside_the_app_state_borrow() {
        assert!(is_modal_dialog_message(WM_SHOW_SETTINGS));
        assert!(is_modal_dialog_message(WM_SHOW_ABOUT));
        assert!(!is_modal_dialog_message(WM_HOOK_ACTION));
    }

    #[test]
    fn visible_number_shortcuts_match_the_existing_keyboard_ranges() {
        assert_eq!(number_position(VK_1.0), Some(1));
        assert_eq!(number_position(VK_9.0), Some(9));
        assert_eq!(number_position(VK_NUMPAD1.0), Some(1));
        assert_eq!(number_position(VK_NUMPAD9.0), Some(9));
        assert_eq!(number_position(VK_0.0), None);
    }

    #[test]
    fn function_keys_map_to_window_commands() {
        assert_eq!(command_for_key(VK_F4.0), Some(WindowCommand::Close));
        assert_eq!(command_for_key(VK_F5.0), Some(WindowCommand::Minimize));
        assert_eq!(command_for_key(VK_F6.0), Some(WindowCommand::Maximize));
        assert_eq!(command_for_key(VK_F7.0), Some(WindowCommand::Restore));
        assert_eq!(command_for_key(VK_F8.0), Some(WindowCommand::Terminate));
        assert_eq!(command_for_key(VK_F9.0), Some(WindowCommand::Run));
        assert_eq!(command_for_key(VK_ESCAPE.0), None);
    }

    #[test]
    fn repeated_f4_message_does_not_issue_another_close_command() {
        let first_press = LPARAM(0);
        let repeated_press = LPARAM(1_isize << 30);

        assert_eq!(
            command_for_key_event(VK_F4.0, first_press),
            Some(WindowCommand::Close)
        );
        assert_eq!(command_for_key_event(VK_F4.0, repeated_press), None);
        assert_eq!(
            command_for_key_event(VK_F5.0, repeated_press),
            Some(WindowCommand::Minimize)
        );
    }

    #[test]
    fn close_command_targets_the_current_switcher_selection() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(vec![
            SwitchTask::new(1, 10, "Previously active", "first"),
            SwitchTask::new(2, 20, "Selected", "second"),
            SwitchTask::new(3, 30, "Unrelated", "third"),
        ]);
        assert!(switcher.select_visible_position(2));

        assert_eq!(
            selected_window_command(&switcher, WindowCommand::Close),
            Some(SelectedWindowCommand {
                command: WindowCommand::Close,
                window_handle: 20,
            })
        );
    }

    #[test]
    fn accepted_close_schedules_another_refresh_when_the_first_snapshot_is_stale() {
        let request = SelectedWindowCommand {
            command: WindowCommand::Close,
            window_handle: 20,
        };
        let first_snapshot = vec![
            SwitchTask::new(1, 10, "Browser", "browser"),
            SwitchTask::new(2, 20, "Closing editor", "editor"),
        ];

        assert_eq!(
            close_refresh_target_after_enumeration(request, &first_snapshot),
            Some(20)
        );
    }

    #[test]
    fn close_refresh_is_not_scheduled_when_the_target_is_gone_or_not_closing() {
        let tasks = vec![SwitchTask::new(1, 20, "Editor", "editor")];

        assert_eq!(
            close_refresh_target_after_enumeration(
                SelectedWindowCommand {
                    command: WindowCommand::Close,
                    window_handle: 10,
                },
                &tasks,
            ),
            None
        );
        assert_eq!(
            close_refresh_target_after_enumeration(
                SelectedWindowCommand {
                    command: WindowCommand::Minimize,
                    window_handle: 20,
                },
                &tasks,
            ),
            None
        );
    }

    #[test]
    fn consecutive_closes_keep_independent_follow_up_refreshes() {
        let mut tracker = CloseRefreshTracker::default();

        assert!(tracker.track(10));
        assert!(!tracker.track(20));
        assert!(tracker.reconcile(&[SwitchTask::new(1, 20, "Second", "second")]));
        assert_eq!(tracker.pending.len(), 1);
        assert_eq!(tracker.pending[0].window_handle, 20);
    }

    #[test]
    fn slow_close_rearms_refresh_while_the_target_is_still_present() {
        let mut tracker = CloseRefreshTracker::default();
        assert!(tracker.track(20));
        tracker.pending[0].attempts_remaining = 2;

        let stale_snapshot = [SwitchTask::new(1, 20, "Closing", "editor")];
        assert!(tracker.reconcile(&stale_snapshot));
        assert_eq!(tracker.pending[0].attempts_remaining, 1);
        assert!(!tracker.reconcile(&[]));
        assert!(tracker.pending.is_empty());
    }

    #[test]
    fn function_key_commands_target_the_selected_item() {
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 10, "First", "first"),
            SwitchTask::new(2, 20, "Selected", "selected"),
        ]);
        assert!(switcher.select_visible_position(2));

        for (virtual_key, expected_command) in [
            (VK_F5.0, WindowCommand::Minimize),
            (VK_F6.0, WindowCommand::Maximize),
            (VK_F7.0, WindowCommand::Restore),
            (VK_F8.0, WindowCommand::Terminate),
            (VK_F9.0, WindowCommand::Run),
        ] {
            assert_eq!(
                command_for_key(virtual_key)
                    .and_then(|command| selected_window_command(&switcher, command)),
                Some(SelectedWindowCommand {
                    command: expected_command,
                    window_handle: 20,
                })
            );
        }
    }

    #[test]
    fn hover_selection_requests_redraw_only_when_the_item_changes() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(vec![
            SwitchTask::new(1, 10, "First", "first"),
            SwitchTask::new(2, 20, "Second", "second"),
        ]);

        for _ in 0..1_000 {
            assert!(!select_hovered_position(&mut switcher, 1));
        }
        assert!(select_hovered_position(&mut switcher, 2));
        for _ in 0..1_000 {
            assert!(!select_hovered_position(&mut switcher, 2));
        }
        assert!(!select_hovered_position(&mut switcher, 3));
    }

    #[test]
    fn reopening_starts_with_an_empty_query_and_the_complete_new_list() {
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 10, "Editor", "editor"),
            SwitchTask::new(2, 20, "Browser", "browser"),
        ]);
        switcher.set_filter("editor");
        assert_eq!(switcher.visible_task_count(), 1);

        prepare_switcher_for_open(
            &mut switcher,
            [
                SwitchTask::new(1, 30, "Terminal", "terminal"),
                SwitchTask::new(2, 40, "Files", "explorer"),
            ],
        );

        assert!(switcher.filter().is_empty());
        assert_eq!(switcher.visible_task_count(), 2);
        assert_eq!(
            switcher.selected_task().map(|task| task.window_handle),
            Some(30)
        );
    }

    #[test]
    fn enter_activation_takes_the_selected_target_and_dismisses_overlay_once() {
        let mut state = OverlayActivationState {
            visible: true,
            context_menu_open: false,
            release: ReleaseActivationSettings {
                alt_switches: true,
                right_button_switches: true,
            },
        };

        assert_eq!(
            take_activation_target(InputAction::ActivateSelected, &mut state, Some(20)),
            Some(20)
        );
        assert!(!state.visible);
        assert_eq!(
            take_activation_target(InputAction::ActivateSelected, &mut state, Some(20)),
            None
        );
    }

    #[test]
    fn alt_release_activation_keeps_its_existing_application_state_rules() {
        let mut state = OverlayActivationState {
            visible: true,
            context_menu_open: false,
            release: ReleaseActivationSettings {
                alt_switches: true,
                right_button_switches: true,
            },
        };

        assert_eq!(
            take_activation_target(InputAction::AltReleased, &mut state, Some(20)),
            Some(20)
        );
        assert!(!state.visible);
    }

    #[test]
    fn right_button_release_activation_requires_its_setting_and_unobstructed_overlay() {
        let mut state = OverlayActivationState {
            visible: true,
            context_menu_open: false,
            release: ReleaseActivationSettings {
                alt_switches: true,
                right_button_switches: true,
            },
        };

        assert_eq!(
            take_activation_target(InputAction::RightButtonReleased, &mut state, Some(20)),
            Some(20)
        );

        for (visible, context_menu_open, release_right_button_switches) in [
            (false, false, true),
            (true, true, true),
            (true, false, false),
        ] {
            state = OverlayActivationState {
                visible,
                context_menu_open,
                release: ReleaseActivationSettings {
                    alt_switches: true,
                    right_button_switches: release_right_button_switches,
                },
            };
            assert_eq!(
                take_activation_target(InputAction::RightButtonReleased, &mut state, Some(20)),
                None
            );
        }
    }

    #[test]
    fn close_button_visual_state_tracks_hover_press_and_leave() {
        let mut interaction = CloseButtonInteraction::default();

        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Normal
        );
        assert!(interaction.update_hover(Some(10)));
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Hovered
        );
        interaction.press(10);
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Pressed
        );
        assert!(interaction.update_hover(None));
        assert_eq!(
            interaction.visual_state(Some(10)),
            CloseButtonVisualState::Normal
        );
        assert!(interaction.cancel_press());
    }

    #[test]
    fn close_button_release_emits_only_the_existing_safe_close_command() {
        let mut interaction = CloseButtonInteraction::default();
        interaction.press(10);

        assert_eq!(interaction.release(Some(10)), Some(WindowCommand::Close));
        assert!(!interaction.is_pressed());

        interaction.press(10);
        assert_eq!(interaction.release(None), None);

        interaction.press(10);
        assert_eq!(interaction.release(Some(20)), None);
    }

    #[test]
    fn close_hit_resolves_only_for_the_current_selected_window() {
        let mut switcher = Switcher::default();
        switcher.set_tasks(vec![
            SwitchTask::new(1, 10, "First", "first"),
            SwitchTask::new(2, 20, "Second", "second"),
        ]);

        assert_eq!(
            close_target_for_hit(&switcher, Some(TaskListHit::CloseButton(1))),
            Some(10)
        );
        assert_eq!(
            close_target_for_hit(&switcher, Some(TaskListHit::CloseButton(2))),
            None
        );
        assert_eq!(
            close_target_for_hit(&switcher, Some(TaskListHit::Task(1))),
            None
        );
    }

    #[test]
    fn compositor_border_tracks_the_visible_borders_setting() {
        assert_eq!(
            compositor_border_color(true, ResolvedTheme::Dark),
            0x0064_6161
        );
        assert_eq!(
            compositor_border_color(true, ResolvedTheme::Light),
            0x009A_9A9A
        );
        assert_eq!(
            compositor_border_color(false, ResolvedTheme::Dark),
            DWMWA_COLOR_NONE
        );
    }

    #[test]
    fn colorref_preserves_windows_bgr_storage_order() {
        assert_eq!(colorref(Rgb8::new(0x12, 0x34, 0x56)), 0x0056_3412);
    }

    #[test]
    fn activation_preserves_target_app_child_focus_by_not_forcing_frame_focus() {
        assert_eq!(
            activation_actions(),
            &[
                ActivationAction::BringToTop,
                ActivationAction::SetForeground,
                ActivationAction::SetActive,
            ]
        );
    }

    #[test]
    fn dpi_change_keeps_overlay_relative_to_positive_secondary_monitor() {
        let suggested = RECT {
            left: 1_920,
            top: 0,
            right: 3_945,
            bottom: 1_350,
        };
        let work_area = RECT {
            left: 1_920,
            top: 0,
            right: 4_480,
            bottom: 1_400,
        };

        assert_eq!(
            overlay_bounds_for_dpi_change(suggested, work_area),
            RECT {
                left: 2_400,
                top: 262,
                right: 4_000,
                bottom: 1_137,
            }
        );
    }

    #[test]
    fn activation_targets_the_visible_last_active_owned_popup() {
        let owner = HWND(100usize as *mut c_void);
        let popup = HWND(200usize as *mut c_void);

        assert_eq!(activation_target(owner, popup, true), popup);
    }

    #[test]
    fn activation_keeps_the_owner_for_an_unusable_popup() {
        let owner = HWND(100usize as *mut c_void);
        let popup = HWND(200usize as *mut c_void);

        assert_eq!(activation_target(owner, popup, false), owner);
        assert_eq!(activation_target(owner, HWND::default(), true), owner);
        assert_eq!(activation_target(owner, owner, true), owner);
    }

    #[test]
    fn dpi_change_keeps_overlay_relative_to_negative_monitor_origin() {
        let suggested = RECT {
            left: -2_560,
            top: -120,
            right: -535,
            bottom: 1_230,
        };
        let work_area = RECT {
            left: -2_560,
            top: -120,
            right: 0,
            bottom: 1_280,
        };

        assert_eq!(
            overlay_bounds_for_dpi_change(suggested, work_area),
            RECT {
                left: -2_080,
                top: 142,
                right: -480,
                bottom: 1_017,
            }
        );
    }
}
