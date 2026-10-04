use crate::about_dialog;
use crate::hook::{HookThread, WM_HOOK_ACTION, decode_action, decode_virtual_key};
use crate::native_theme::resolve_current_theme;
use crate::preview::DwmPreview;
use crate::process_info::ProcessInfo;
use crate::renderer::{CloseButtonVisualState, RenderOptions, Renderer, TaskListHit};
use crate::settings_dialog;
use crate::settings_io::SettingsStore;
use crate::shell_menu;
use crate::single_instance::SingleInstance;
use crate::startup;
use crate::task_icon::TaskIcons;
use crate::task_query::{EnumeratedTasks, enumerate_switchable_windows, window_class_name};
use crate::tray::{TrayAction, TrayIcon, WM_TRAY_CALLBACK};
use crate::win_events::{
    self, LISTED_REFRESH_RETRY_DELAY_MS, LISTED_REFRESH_RETRY_TIMER_ID, WM_FOREGROUND_CHECK,
    WM_LISTED_WINDOW_REFRESH, WinEventWatcher, is_listed_refresh_wakeup,
};
use crate::window_commands::{
    execute as execute_window_command, show_menu as show_window_command_menu,
};
use alttabio::activation::activation_target;
use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll, SwitchResume};
use alttabio::input::{
    HookSettings, InputAction, OverlayKeyEvent, WindowCommand, overlay_key_action,
};
use alttabio::overlay_pointer::{self, CloseButtonInteraction, select_hovered_position};
use alttabio::overlay_window::{
    ScreenRect, compositor_border_color, overlay_bounds, overlay_bounds_for_dpi_change,
};
use alttabio::passthrough::{PassthroughPolicy, is_remote_desktop_client, window_fills_monitor};
use alttabio::settings::Settings;
use alttabio::switcher::{
    Switcher, SwitcherEffect, SwitcherSession, SwitcherSessionSettings, WindowCommandRequest,
};
use alttabio::task_refresh::{
    ContextMenuCommandOutcome, RefreshDecision, RetryTimer, TaskListRefresh,
    apply_listed_refresh_batch,
};
use alttabio::theme::ResolvedTheme;
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{
    ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SetLastError, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, MonitorFromRect, MonitorFromWindow, PAINTSTRUCT,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetForegroundWindow,
    GetLastActivePopup, GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    IDC_ARROW, IsIconic, IsWindowVisible, IsZoomed, KillTimer, LoadCursorW, MB_ICONERROR, MB_OK,
    MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SW_RESTORE,
    SW_SHOW, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, ShowWindowAsync, TranslateMessage,
    WM_CAPTURECHANGED, WM_CHAR, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP,
    WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN, WM_THEMECHANGED, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_THICKFRAME,
};
use windows::core::{Error, PCWSTR, Result, w};

const WINDOW_CLASS: PCWSTR = w!("AltTabioRustOverlay");
const WINDOW_TITLE: PCWSTR = w!("AltTabio");
const WM_SHOW_SETTINGS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;
const WM_DESTROY_APP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;
const WM_SHOW_ABOUT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 5;
const WM_MOUSE_LEAVE: u32 = 0x02A3;
const CLOSE_REFRESH_TIMER_ID: usize = 1;
const SHELL_DISMISS_TIMER_ID: usize = 3;

struct PendingShellDismissal {
    window: HWND,
    origin: WPARAM,
    started: std::time::Instant,
    input: DeferredSwitch,
}
const CLOSE_REFRESH_DELAY_MS: u32 = 250;

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
    // Temporary repair for autostart tasks created with the old 72-hour default.
    if !preview_mode && let Err(error) = startup::repair_legacy_task_timeout() {
        eprintln!("Could not repair the legacy autostart runtime limit: {error}");
    }

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
        let destroy_result = unsafe {
            // SAFETY: `hwnd` is the live overlay window created above.
            DestroyWindow(hwnd)
        };
        match destroy_result {
            Ok(()) => unsafe {
                // SAFETY: after DestroyWindow returns no callback retains the unique host allocation.
                drop(Box::from_raw(host_pointer));
            },
            // The live HWND still retains host_pointer. Leaking is safer than freeing callback state.
            Err(destroy_error) => {
                eprintln!("Could not destroy AltTabio after its startup failed: {destroy_error}");
            }
        }
        return Err(Error::new(
            windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
            &error,
        ));
    }
    if preview_mode {
        unsafe {
            // SAFETY: host_pointer remains live and initialization released its state borrow.
            (*host_pointer).state.borrow_mut().show_overlay(None);
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
    show_error_box(None, message);
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
    pending_shell: Option<PendingShellDismissal>,
    session: SwitcherSession,
    task_icons: TaskIcons,
    renderer: Renderer,
    resolved_theme: ResolvedTheme,
    preview: Option<DwmPreview>,
    hooks: Option<HookThread>,
    tray: Option<TrayIcon>,
    mouse_origin: Option<POINT>,
    mouse_selection_armed: bool,
    mouse_leave_tracked: bool,
    close_button: CloseButtonInteraction,
    task_refresh: TaskListRefresh,
    exit_when_hidden: bool,
    dwm_preview: bool,
    settings: Settings,
    settings_store: SettingsStore,
    settings_dialog_open: bool,
    about_dialog_open: bool,
    win_event_watcher: Option<WinEventWatcher>,
    listed_refresh_retry_timer_armed: bool,
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
        app.sync_hook_interception();
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
        app.sync_hook_interception();
        if let Err(error) = result {
            app.show_error(&error);
        }
    }

    fn show_task_context_menu(&self, lparam: LPARAM) {
        let Some(owner) = self
            .state
            .try_borrow_mut()
            .ok()
            .and_then(|mut app| app.prepare_task_context_menu(lparam))
        else {
            return;
        };
        let command = show_window_command_menu(owner);
        let Ok(mut app) = self.state.try_borrow_mut() else {
            eprintln!("Could not finish the task menu because application state is busy");
            return;
        };
        app.finish_task_context_menu(command);
    }
}

fn store_started_hook_then_sync<H, E>(
    slot: &mut Option<H>,
    started: std::result::Result<H, E>,
    sync: impl FnOnce(&H),
) -> std::result::Result<(), E> {
    *slot = Some(started?);
    if let Some(hook) = slot.as_ref() {
        sync(hook);
    }
    Ok(())
}

impl App {
    fn new(
        exit_when_hidden: bool,
        dwm_preview: bool,
        settings: Settings,
        settings_store: SettingsStore,
    ) -> Result<Self> {
        let resolved_theme = resolve_current_theme(settings.appearance.theme);
        let session = SwitcherSession::new(switcher_session_settings(&settings));
        Ok(Self {
            hwnd: HWND::default(),
            pending_shell: None,
            session,
            task_icons: TaskIcons::default(),
            renderer: Renderer::new(resolved_theme)?,
            resolved_theme,
            preview: None,
            hooks: None,
            tray: None,
            mouse_origin: None,
            mouse_selection_armed: false,
            mouse_leave_tracked: false,
            close_button: CloseButtonInteraction::default(),
            task_refresh: TaskListRefresh::default(),
            exit_when_hidden,
            dwm_preview,
            settings,
            settings_store,
            settings_dialog_open: false,
            about_dialog_open: false,
            win_event_watcher: None,
            listed_refresh_retry_timer_armed: false,
        })
    }

    fn initialize(&mut self, hwnd: HWND, install_hooks: bool) -> std::result::Result<(), String> {
        self.hwnd = hwnd;
        self.recreate_preview();
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
            self.start_input_hooks(hook_settings(&self.settings))?;
            win_events::publish_notify_hwnd(hwnd);
            match WinEventWatcher::install() {
                Ok(watcher) => {
                    self.win_event_watcher = Some(watcher);
                    if let Err(error) = self.start_listed_refresh_retry_timer() {
                        self.win_event_watcher = None;
                        self.show_error(&error);
                    }
                }
                Err(error) => {
                    self.show_error(&WinEventWatcher::install_failure_message(&error));
                }
            }
        }
        Ok(())
    }

    fn handle_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(result) = self.handle_posted_message(message, wparam, lparam) {
            return Some(result);
        }
        match message {
            WM_DPICHANGED => {
                self.handle_dpi_changed(lparam);
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE => {
                self.handle_display_changed();
                Some(LRESULT(0))
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                self.handle_focused_key(wparam.0, lparam);
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
            WM_MOUSEWHEEL => {
                let delta = high_word_usize(wparam.0).cast_signed();
                self.handle_input_action(InputAction::MouseWheel(i32::from(delta.signum())));
                Some(LRESULT(0))
            }
            WM_SIZE => {
                let width = u32::from(low_word_isize(lparam.0));
                let height = u32::from(high_word_isize(lparam.0));
                self.resize_content(width, height);
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

    fn handle_posted_message(
        &mut self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if let Some(result) = self
            .tray
            .as_ref()
            .and_then(|tray| tray.restore_for_message(message))
        {
            if let Err(error) = result {
                eprintln!(
                    "Could not restore the AltTabio tray icon after Explorer restarted: {error}"
                );
            }
            return Some(LRESULT(0));
        }
        match message {
            WM_HOOK_ACTION | crate::hook::WM_HOOK_HOTKEY_ACTION => {
                if hook_actions_enabled(self.settings_dialog_open, self.about_dialog_open)
                    && !self.session.context_menu_open()
                    && self
                        .hooks
                        .as_ref()
                        .is_some_and(|hooks| hooks.action_is_current(wparam))
                    && let Some(action) = decode_action(wparam, lparam)
                {
                    if message == crate::hook::WM_HOOK_HOTKEY_ACTION
                        && matches!(action, InputAction::Switch(_))
                    {
                        // The actual Tab was delivered as a registered hotkey. Taking focus
                        // now dismisses the shell naturally, without injecting Escape first.
                        let pending = self.pending_shell.take();
                        if pending.is_some() {
                            self.kill_shell_dismissal_timer();
                        }
                        let pending = pending.filter(|pending| {
                            let current = self
                                .hooks
                                .as_ref()
                                .is_some_and(|hooks| hooks.action_is_current(pending.origin));
                            if !current {
                                // A new physical gesture must not replay actions from before
                                // a desktop or modal boundary, or inherit its selection.
                                self.session.hide();
                            }
                            current
                        });
                        DeferredSwitch::resume_with_hotkey(
                            pending.map(|pending| pending.input),
                            action,
                            |step| match step {
                                SwitchResume::FocusOverlay => self.focus_overlay(),
                                SwitchResume::Replay(action) => {
                                    self.apply_input_action_with_reset(action, false);
                                }
                                SwitchResume::Input(action) => self.apply_input_action(action),
                            },
                        );
                    } else {
                        self.handle_hook_input(action, wparam);
                    }
                }
                Some(LRESULT(0))
            }
            WM_TRAY_CALLBACK => {
                self.handle_tray_message(lparam);
                Some(LRESULT(0))
            }
            WM_FOREGROUND_CHECK => {
                self.handle_foreground_check();
                Some(LRESULT(0))
            }
            WM_LISTED_WINDOW_REFRESH => {
                self.handle_listed_window_refresh();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == CLOSE_REFRESH_TIMER_ID => {
                self.handle_close_refresh_timer();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == LISTED_REFRESH_RETRY_TIMER_ID => {
                self.handle_listed_refresh_retry_timer();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == SHELL_DISMISS_TIMER_ID => {
                self.handle_shell_dismissal();
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn handle_tray_message(&mut self, lparam: LPARAM) {
        let message = u32::try_from(lparam.0).unwrap_or_default();
        let action = match message {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::Show,
            WM_RBUTTONUP => {
                let _suspension = self.hooks.as_ref().map(HookThread::suspend_interception);
                self.tray
                    .as_ref()
                    .map(TrayIcon::show_menu)
                    .unwrap_or_default()
            }
            _ => TrayAction::None,
        };
        match action {
            TrayAction::Show => {
                self.stop_shell_dismissal();
                self.show_overlay(None);
            }
            TrayAction::Settings => self.request_modal_dialog(WM_SHOW_SETTINGS, "Settings"),
            TrayAction::About => self.request_modal_dialog(WM_SHOW_ABOUT, "About"),
            TrayAction::Exit => self.request_close("the tray"),
            TrayAction::None => {}
        }
    }

    fn handle_foreground_check(&mut self) {
        win_events::acknowledge_foreground_check();
        let policy = foreground_passthrough_policy(self.hwnd);
        if policy.bypasses_local_switching() && (self.is_visible() || self.pending_shell.is_some())
        {
            self.hide_overlay();
        }
        let Some(hooks) = self.hooks.as_ref() else {
            return;
        };
        if let Err(error) = hooks.set_remote_desktop_passthrough(policy) {
            eprintln!("{error}");
        }
    }

    fn start_input_hooks(&mut self, settings: HookSettings) -> std::result::Result<(), String> {
        let policy = foreground_passthrough_policy(self.hwnd);
        if policy.bypasses_local_switching() && self.is_visible() {
            self.hide_overlay();
        }
        store_started_hook_then_sync(
            &mut self.hooks,
            HookThread::start(self.hwnd, settings),
            |hooks| {
                if let Err(error) = hooks.set_remote_desktop_passthrough(policy) {
                    eprintln!("{error}");
                }
            },
        )
    }

    fn prepare_settings_dialog(&mut self) -> Option<(HWND, Settings)> {
        if self.settings_dialog_open || self.about_dialog_open {
            return None;
        }
        self.hide_overlay();
        self.settings_dialog_open = true;
        self.sync_hook_interception();
        Some((self.hwnd, self.settings.clone()))
    }

    fn prepare_about_dialog(&mut self) -> Option<(ResolvedTheme, alttabio::settings::IconColor)> {
        if self.about_dialog_open || self.settings_dialog_open {
            return None;
        }
        self.hide_overlay();
        self.about_dialog_open = true;
        self.sync_hook_interception();
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
            match self.start_input_hooks(new_hook_settings) {
                Ok(()) => {}
                Err(error) => {
                    let hooks_rollback = self.start_input_hooks(old_hook_settings);
                    let settings_rollback = self.settings_store.save(&previous_settings);
                    let autostart_rollback = autostart_changed
                        .then(|| startup::set_enabled(previous_autostart.enabled))
                        .transpose();
                    let mut message =
                        format!("The new input-hook settings could not be activated. {error}");
                    if let Err(rollback_error) = hooks_rollback {
                        message.push_str("\n\nThe previous input hooks could not be restored. ");
                        message.push_str(&rollback_error);
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

        self.settings = settings;
        self.session
            .update_settings(switcher_session_settings(&self.settings));
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
        self.recreate_preview();
        self.request_redraw();
    }

    fn resize_content(&mut self, width: u32, height: u32) {
        if let Err(error) = self.renderer.resize(self.hwnd, width, height) {
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
        self.stop_shell_dismissal();
        self.stop_listed_refresh_retry_timer();
        win_events::clear_notify_hwnd();
        self.win_event_watcher = None;
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
        let _suspension = self.hooks.as_ref().map(HookThread::suspend_interception);
        show_error_for_window(self.hwnd, message);
    }

    fn handle_input_action(&mut self, action: InputAction) {
        if let Some(pending) = &mut self.pending_shell {
            if pending.input.push(action) {
                self.preview_shell_switch();
            } else {
                self.hide_overlay();
            }
            return;
        }
        self.apply_input_action(action);
    }

    fn handle_hook_input(&mut self, action: InputAction, origin: WPARAM) {
        if self.pending_shell.is_none()
            && !self.is_visible()
            && matches!(action, InputAction::Switch(_))
            && let Some(window) = shell_menu::foreground_menu()
        {
            self.begin_shell_dismissal(window, action, origin);
            return;
        }
        self.handle_input_action(action);
    }

    fn apply_input_action(&mut self, action: InputAction) {
        self.apply_input_action_with_reset(action, true);
    }

    fn apply_input_action_with_reset(&mut self, action: InputAction, reset_hook: bool) {
        match self.session.handle_input(action) {
            SwitcherEffect::None => {}
            SwitcherEffect::Open { selection_delta } => self.show_overlay(selection_delta),
            SwitcherEffect::Hide => self.hide_overlay_with_reset(reset_hook),
            SwitcherEffect::Redraw => self.request_redraw(),
            SwitcherEffect::Activate(target) => self.activate_target(target, reset_hook),
            SwitcherEffect::Execute(request) => {
                let outcome = self.execute_window_command(request);
                self.task_refresh.apply_command_outcome(outcome);
                self.run_pending_task_refresh();
            }
        }
    }

    fn begin_shell_dismissal(&mut self, window: HWND, first: InputAction, origin: WPARAM) {
        // SAFETY: this timer belongs to the live overlay; no callback pointer is retained.
        if unsafe { SetTimer(Some(self.hwnd), SHELL_DISMISS_TIMER_ID, 16, None) } == 0 {
            eprintln!(
                "Could not start the shell dismissal timer: {}",
                Error::from_thread()
            );
            self.hide_overlay();
            return;
        }
        self.pending_shell = Some(PendingShellDismissal {
            window,
            origin,
            started: std::time::Instant::now(),
            input: DeferredSwitch::new(first),
        });
        if let Err(error) = shell_menu::dismiss(window) {
            eprintln!("{error}");
            self.hide_overlay();
            return;
        }
        self.preview_shell_switch();
    }

    fn preview_shell_switch(&mut self) {
        let Some(pending) = &mut self.pending_shell else {
            return;
        };
        for action in pending.input.take_preview_actions() {
            self.apply_input_action(action);
        }
    }

    fn stop_shell_dismissal(&mut self) {
        if self.pending_shell.take().is_none() {
            return;
        }
        self.kill_shell_dismissal_timer();
    }

    fn kill_shell_dismissal_timer(&self) {
        // SAFETY: this balances the timer started for the live overlay's pending opening.
        if let Err(error) = unsafe { KillTimer(Some(self.hwnd), SHELL_DISMISS_TIMER_ID) } {
            eprintln!("Could not stop the shell dismissal timer: {error}");
        }
    }

    fn handle_shell_dismissal(&mut self) {
        let Some(pending) = &mut self.pending_shell else {
            return;
        };
        if !self
            .hooks
            .as_ref()
            .is_some_and(|hooks| hooks.action_is_current(pending.origin))
        {
            self.hide_overlay();
            return;
        }
        let shell_has_focus =
            if let Some(window) = shell_menu::remaining_foreground_menu(pending.window) {
                pending.window = window;
                true
            } else {
                false
            };
        let poll = pending
            .input
            .poll(shell_has_focus, pending.started.elapsed());
        match poll {
            DeferredSwitchPoll::Wait => {}
            DeferredSwitchPoll::RetryDismissal => {
                if let Err(error) = shell_menu::dismiss(pending.window) {
                    eprintln!("{error}");
                    self.hide_overlay();
                }
            }
            DeferredSwitchPoll::Cancel => {
                eprintln!("Start/Search did not relinquish foreground within the switch deadline");
                self.hide_overlay();
            }
            DeferredSwitchPoll::Ready(actions) => {
                self.stop_shell_dismissal();
                if self.is_visible() {
                    self.focus_overlay();
                }
                for action in actions {
                    self.handle_input_action(action);
                }
            }
        }
    }

    fn handle_focused_key(&mut self, virtual_key: usize, lparam: LPARAM) {
        let Ok(virtual_key) = u32::try_from(virtual_key) else {
            return;
        };
        let event = OverlayKeyEvent {
            key: decode_virtual_key(virtual_key),
            repeated: key_was_previously_down(lparam),
            shift: key_is_down(VK_SHIFT.0),
        };
        if let Some(action) = overlay_key_action(event) {
            self.handle_input_action(action);
        }
    }

    fn handle_character(&mut self, value: usize) {
        if !self.search_active() {
            return;
        }
        let value = u32::try_from(value).unwrap_or_default();
        let action = if value == u32::from(VK_BACK.0) {
            InputAction::BackspaceSearch
        } else if let Some(character) = char::from_u32(value)
            && !character.is_control()
        {
            InputAction::AppendSearchCharacter(character)
        } else {
            return;
        };
        self.handle_input_action(action);
    }

    fn execute_window_command(
        &mut self,
        request: WindowCommandRequest,
    ) -> ContextMenuCommandOutcome {
        let command = request.command;
        if !execute_window_command(
            request.command,
            request.window_handle,
            request.process_identity,
        ) {
            eprintln!("Could not execute {command:?} for the selected window");
            return ContextMenuCommandOutcome::Failed;
        }
        self.close_button.reset();
        ContextMenuCommandOutcome::Succeeded {
            close_window: (command == WindowCommand::Close).then_some(request.window_handle),
        }
    }

    fn start_close_refresh_timer(&mut self) {
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
            self.task_refresh.cancel_retries();
            eprintln!(
                "Could not schedule a follow-up refresh after closing a window: {}",
                Error::from_thread()
            );
        }
    }

    fn stop_close_refresh_timer(&mut self) {
        let result = unsafe {
            // SAFETY: this handles the timer owned by the live overlay HWND.
            KillTimer(Some(self.hwnd), CLOSE_REFRESH_TIMER_ID)
        };
        if let Err(error) = result {
            eprintln!("Could not stop the close refresh timer: {error}");
        }
    }

    fn handle_close_refresh_timer(&mut self) {
        if self.session.context_menu_open() {
            return;
        }
        if !self.task_refresh.has_pending_retries() {
            self.stop_close_refresh_timer();
            return;
        }
        self.refresh_switcher_tasks();
    }

    fn handle_listed_window_refresh(&mut self) {
        self.ingest_listed_refresh_signal();
        self.run_pending_task_refresh();
    }

    fn start_listed_refresh_retry_timer(&mut self) -> std::result::Result<(), String> {
        let timer_id = unsafe {
            // SAFETY: the live overlay HWND owns this timer and no callback pointer is retained.
            SetTimer(
                Some(self.hwnd),
                LISTED_REFRESH_RETRY_TIMER_ID,
                LISTED_REFRESH_RETRY_DELAY_MS,
                None,
            )
        };
        if timer_id == 0 {
            return Err(format!(
                "Could not start reliable live window-list updates: {}\n\nThe window event watcher has been disabled.",
                Error::from_thread()
            ));
        }
        self.listed_refresh_retry_timer_armed = true;
        Ok(())
    }

    fn stop_listed_refresh_retry_timer(&mut self) {
        if !self.listed_refresh_retry_timer_armed {
            return;
        }
        self.listed_refresh_retry_timer_armed = false;
        let result = unsafe {
            // SAFETY: this stops the timer owned by the live overlay HWND during shutdown.
            KillTimer(Some(self.hwnd), LISTED_REFRESH_RETRY_TIMER_ID)
        };
        if let Err(error) = result {
            eprintln!("Could not stop the listed-window refresh retry timer: {error}");
        }
    }

    fn handle_listed_refresh_retry_timer(&mut self) {
        if win_events::foreground_check_needs_retry() {
            self.handle_foreground_check();
        }
        let Some(batch) = win_events::take_listed_refresh_retry() else {
            return;
        };
        apply_listed_refresh_batch(&mut self.task_refresh, batch);
        self.run_pending_task_refresh();
    }

    fn ingest_listed_refresh_signal(&mut self) {
        apply_listed_refresh_batch(
            &mut self.task_refresh,
            win_events::take_listed_refresh_notices(),
        );
    }

    fn run_pending_task_refresh(&mut self) {
        match self.task_refresh.decision(
            self.session.is_visible(),
            |window_handle| {
                self.session
                    .switcher()
                    .contains_window_handle(window_handle)
            },
            self.session.context_menu_open(),
        ) {
            RefreshDecision::Ignore => {
                self.task_refresh.clear_notices();
            }
            RefreshDecision::Defer => {}
            RefreshDecision::Refresh => self.refresh_switcher_tasks(),
        }
    }

    fn refresh_switcher_tasks(&mut self) {
        let timer = match enumerate_switchable_windows(&self.settings) {
            Ok(EnumeratedTasks { tasks, icons }) => {
                let timer = self.task_refresh.complete_enumeration(Ok(&tasks));
                self.session.refresh_tasks(tasks);
                self.task_icons = icons;
                timer
            }
            Err(error) => {
                eprintln!("Could not refresh windows: {error}");
                self.task_refresh.complete_enumeration(Err(()))
            }
        };
        match timer {
            RetryTimer::Start => self.start_close_refresh_timer(),
            RetryTimer::Stop => self.stop_close_refresh_timer(),
            RetryTimer::Keep => {}
        }
        self.sync_overlay_after_task_refresh();
    }

    fn sync_overlay_after_task_refresh(&mut self) {
        if self.session.is_visible() {
            self.request_redraw();
        } else {
            self.hide_overlay();
        }
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
            && select_hovered_position(self.session.switcher_mut(), position)
        {
            needs_redraw = true;
            hit = self.hit_test(lparam);
        }
        let close_target = close_target_for_hit(self.session.switcher(), hit);
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
        let target = close_target_for_hit(self.session.switcher(), hit);
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
            let target = close_target_for_hit(self.session.switcher(), hit);
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
                self.handle_input_action(InputAction::WindowCommand(command));
            }
            return;
        }

        if let Some(TaskListHit::Task(position)) = hit {
            self.handle_input_action(InputAction::ActivateVisiblePosition(position));
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
            self.session.switcher_mut(),
            x,
            y,
            self.settings.appearance.compact_list,
        )
    }

    fn prepare_task_context_menu(&mut self, lparam: LPARAM) -> Option<HWND> {
        if !self.is_visible()
            || self.settings_dialog_open
            || self.about_dialog_open
            || self.session.context_menu_open()
        {
            return None;
        }
        let hit = self.hit_test(lparam)?;
        let position = hit.position();
        if !self
            .session
            .switcher_mut()
            .select_visible_position(position)
        {
            return None;
        }
        self.request_redraw();
        if !self.session.open_context_menu() {
            return None;
        }
        self.sync_hook_interception();
        Some(self.hwnd)
    }

    fn finish_task_context_menu(&mut self, command: Option<WindowCommand>) {
        let effect = self.session.finish_context_menu(command);
        self.sync_hook_interception();
        self.ingest_listed_refresh_signal();
        let outcome = match effect {
            SwitcherEffect::Execute(request) => self.execute_window_command(request),
            _ => ContextMenuCommandOutcome::None,
        };
        self.task_refresh.apply_command_outcome(outcome);
        self.run_pending_task_refresh();
    }

    fn handle_dpi_changed(&mut self, lparam: LPARAM) {
        let suggested = unsafe {
            // SAFETY: WM_DPICHANGED guarantees lParam points to a RECT for the callback duration.
            (lparam.0 as *const RECT).as_ref()
        };
        if let Some(suggested) = suggested {
            let bounds = match monitor_work_area_from_rect(*suggested) {
                Ok(work_area) => win32_rect(overlay_bounds_for_dpi_change(
                    screen_rect(*suggested),
                    screen_rect(work_area),
                )),
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
        self.refresh_display_content();
    }

    fn handle_display_changed(&mut self) {
        self.recreate_preview();
        if !self.is_visible() {
            return;
        }
        if let Err(error) = position_on_cursor_monitor(self.hwnd) {
            eprintln!("Could not reposition the overlay after the display changed: {error}");
        }
        self.sync_content_size();
        self.request_redraw();
    }

    fn refresh_display_content(&mut self) {
        self.recreate_preview();
        self.sync_content_size();
        self.request_redraw();
    }

    fn recreate_preview(&mut self) {
        self.preview = None;
        if self.dwm_preview && self.settings.appearance.preview {
            self.preview = Some(DwmPreview::new(
                self.hwnd,
                self.settings.appearance.full_desktop_preview,
                self.settings.appearance.compact_list,
            ));
        }
    }

    fn show_overlay(&mut self, selection_delta: Option<i32>) {
        match enumerate_switchable_windows(&self.settings) {
            Ok(EnumeratedTasks { tasks, icons }) => {
                self.session.open(tasks, selection_delta);
                self.task_icons = icons;
            }
            Err(error) => {
                eprintln!("Could not enumerate windows: {error}");
                self.hide_overlay();
                return;
            }
        }
        if !self.session.is_visible() {
            self.hide_overlay();
            return;
        }
        self.reset_mouse_selection();
        if let Err(error) = position_on_cursor_monitor(self.hwnd) {
            eprintln!("Could not position the overlay: {error}");
        }
        unsafe {
            // SAFETY: the HWND is live and owned by this UI thread.
            let _was_visible = ShowWindow(
                self.hwnd,
                if self.pending_shell.is_some() {
                    SW_SHOWNA
                } else {
                    SW_SHOW
                },
            );
        }
        if self.pending_shell.is_none() {
            self.focus_overlay();
        }
        self.sync_content_size();
        self.set_hook_search_active(true);
        self.set_hook_overlay_active(true);
        self.request_redraw();
    }

    fn focus_overlay(&self) {
        // SAFETY: the HWND is live and owned by this UI thread.
        unsafe {
            if !request_foreground(self.hwnd) {
                eprintln!("Could not bring the overlay to the foreground");
            }
            if let Err(error) = SetFocus(Some(self.hwnd)) {
                eprintln!("Could not focus the overlay: {error}");
            }
        }
    }

    fn hide_overlay(&mut self) {
        self.hide_overlay_with_reset(true);
    }

    fn hide_overlay_with_reset(&mut self, reset_hook: bool) {
        self.stop_shell_dismissal();
        self.task_refresh.clear_notices();
        self.session.hide();
        self.set_hook_search_active(false);
        if reset_hook {
            self.reset_hook_gestures();
        }
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

    fn activate_target(&mut self, target: isize, reset_hook: bool) {
        let target = HWND(target as *mut c_void);
        if !activate_and_hide(target, || self.hide_overlay_with_reset(reset_hook)) {
            // The completed gesture has released ownership. Reopening here leaves an
            // overlay with no matching Alt release left to dismiss it.
            eprintln!("Could not activate the selected window");
        }
    }

    fn paint(&mut self) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe {
            // SAFETY: `paint` is writable and BeginPaint/EndPaint are paired for this WM_PAINT.
            BeginPaint(self.hwnd, &raw mut paint)
        };
        if dc.is_invalid() {
            eprintln!("Could not begin painting the overlay");
        }
        let render_options = RenderOptions::from(&self.settings.appearance);
        let switcher = self.session.switcher();
        let selected_target = switcher.selected_task().map(|task| task.window_handle);
        if let Err(error) = self.renderer.draw(
            self.hwnd,
            switcher,
            self.preview.as_ref().and_then(DwmPreview::frame),
            render_options,
            renderer_close_button_state(self.close_button.visual_state(selected_target)),
        ) {
            eprintln!("Could not render the overlay: {error}");
        }
        // Direct2D draws through its own window target; only the GDI icons need the paint DC.
        if !dc.is_invalid() {
            Renderer::draw_icons(self.hwnd, dc, switcher, render_options);
        }
        let ended = unsafe {
            // SAFETY: this balances the BeginPaint call above for the same PAINTSTRUCT.
            EndPaint(self.hwnd, &raw const paint)
        };
        if !ended.as_bool() {
            eprintln!("Could not finish painting the overlay");
        }
    }

    fn request_redraw(&mut self) {
        let source = self
            .session
            .switcher()
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
        self.session.search_active()
    }

    fn sync_hook_interception(&self) {
        let Some(hooks) = self.hooks.as_ref() else {
            return;
        };
        let suspended =
            self.settings_dialog_open || self.about_dialog_open || self.session.context_menu_open();
        if suspended {
            hooks.set_interception_suspended(true);
        }
        hooks.set_search_active(!suspended && self.session.search_active());
        hooks.set_overlay_active(!suspended && self.session.is_visible());
        if !suspended {
            hooks.set_interception_suspended(false);
        }
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
        self.session.is_visible()
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
            // SAFETY: host remains live through the message loop.
            if let Err(error) = unsafe { set_window_user_data(hwnd, host as isize) } {
                // Failing creation lets `run` free the host instead of running a window that
                // can never reach it.
                eprintln!("Could not attach AltTabio to its window: {error}");
                return Some(LRESULT(0));
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
            // SAFETY: clearing user data prevents later messages from observing host.
            if let Err(error) = unsafe { set_window_user_data(hwnd, 0) } {
                eprintln!("Could not detach AltTabio from its window: {error}");
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
        if message == WM_RBUTTONUP {
            host.show_task_context_menu(lparam);
            return Some(LRESULT(0));
        }
        let Ok(mut app) = host.state.try_borrow_mut() else {
            return match busy_overlay_message_action(message, wparam) {
                BusyOverlayMessage::RetryForegroundCheck => {
                    win_events::foreground_check_message_dropped();
                    Some(LRESULT(0))
                }
                BusyOverlayMessage::AcknowledgeDroppedRefresh => {
                    win_events::listed_refresh_message_dropped();
                    Some(LRESULT(0))
                }
                BusyOverlayMessage::IgnoreRetryTick => Some(LRESULT(0)),
                BusyOverlayMessage::DeferToDefault => None,
            };
        };
        app.handle_message(message, wparam, lparam)
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| default_window_proc(hwnd, message, wparam, lparam))
}

/// # Safety
///
/// `value` must be zero or an `AppHost` pointer that stays live until `WM_NCDESTROY` clears it,
/// because `window_proc` dereferences any nonzero user data.
unsafe fn set_window_user_data(hwnd: HWND, value: isize) -> Result<()> {
    unsafe {
        // SAFETY: SetLastError only writes this thread's last-error value.
        SetLastError(ERROR_SUCCESS);
    }
    let previous = unsafe {
        // SAFETY: hwnd is the window being handled, and the caller upholds the contract for the
        // stored value.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, value)
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

const fn is_modal_dialog_message(message: u32) -> bool {
    matches!(message, WM_SHOW_SETTINGS | WM_SHOW_ABOUT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BusyOverlayMessage {
    RetryForegroundCheck,
    AcknowledgeDroppedRefresh,
    IgnoreRetryTick,
    DeferToDefault,
}

const fn busy_overlay_message_action(message: u32, wparam: WPARAM) -> BusyOverlayMessage {
    if message == WM_FOREGROUND_CHECK {
        BusyOverlayMessage::RetryForegroundCheck
    } else if message == WM_LISTED_WINDOW_REFRESH {
        BusyOverlayMessage::AcknowledgeDroppedRefresh
    } else if is_listed_refresh_wakeup(message, wparam) {
        BusyOverlayMessage::IgnoreRetryTick
    } else {
        BusyOverlayMessage::DeferToDefault
    }
}

const fn hook_actions_enabled(settings_dialog_open: bool, about_dialog_open: bool) -> bool {
    !settings_dialog_open && !about_dialog_open
}

fn show_error_for_window(owner: HWND, message: &str) {
    show_error_box(Some(owner), message);
}

fn show_error_box(owner: Option<HWND>, message: &str) {
    let text = null_terminated(message);
    let result = unsafe {
        // SAFETY: both UTF-16 buffers remain alive and null terminated for the synchronous call.
        MessageBoxW(
            owner,
            PCWSTR(text.as_ptr()),
            w!("AltTabio"),
            MB_OK | MB_ICONERROR,
        )
    };
    if result.0 == 0 {
        eprintln!(
            "Could not show an error dialog ({}): {message}",
            Error::from_thread()
        );
    }
}

fn foreground_passthrough_policy(overlay: HWND) -> PassthroughPolicy {
    let hwnd = unsafe {
        // SAFETY: GetForegroundWindow has no pointer preconditions.
        GetForegroundWindow()
    };
    if hwnd.0.is_null() || hwnd == overlay {
        return PassthroughPolicy::Local;
    }
    let class_name = window_class_name(hwnd);
    let mut process_id = 0_u32;
    let thread_id = unsafe {
        // SAFETY: process_id is writable and hwnd is the live foreground window.
        GetWindowThreadProcessId(hwnd, Some(&raw mut process_id))
    };
    // A process that refuses the query or has exited leaves the executable unknown. The window
    // class still identifies remote desktop clients, so the policy proceeds without it.
    let process = if thread_id == 0 {
        ProcessInfo::unavailable(process_id)
    } else {
        ProcessInfo::query(process_id).unwrap_or_else(|_| ProcessInfo::unavailable(process_id))
    };
    PassthroughPolicy::from_foreground(
        is_remote_desktop_client(&class_name, process.executable_stem()),
        is_maximized_or_fullscreen(hwnd),
    )
}

fn is_maximized_or_fullscreen(hwnd: HWND) -> bool {
    if unsafe {
        // SAFETY: hwnd is the live foreground window.
        IsZoomed(hwnd)
    }
    .as_bool()
    {
        return true;
    }
    let mut window = RECT::default();
    if unsafe {
        // SAFETY: `window` is writable and hwnd is a live top-level window.
        GetWindowRect(hwnd, &raw mut window)
    }
    .is_err()
    {
        return false;
    }
    let monitor = unsafe {
        // SAFETY: hwnd is live and nearest-monitor fallback is requested.
        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
    };
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or_default(),
        ..MONITORINFO::default()
    };
    let success = unsafe {
        // SAFETY: `monitor_info` is writable with a correct cbSize.
        GetMonitorInfoW(monitor, &raw mut monitor_info)
    };
    success.as_bool()
        && window_fills_monitor(
            [window.left, window.top, window.right, window.bottom],
            [
                monitor_info.rcMonitor.left,
                monitor_info.rcMonitor.top,
                monitor_info.rcMonitor.right,
                monitor_info.rcMonitor.bottom,
            ],
        )
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
    let bounds = win32_rect(overlay_bounds(screen_rect(monitor_info.rcWork)));
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

const fn screen_rect(rect: RECT) -> ScreenRect {
    ScreenRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

const fn win32_rect(rect: ScreenRect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn key_is_down(virtual_key: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    unsafe {
        // SAFETY: GetKeyState accepts any virtual-key code and has no pointer preconditions.
        GetKeyState(i32::from(virtual_key)) < 0
    }
}

fn apply_window_appearance(hwnd: HWND, visible_borders: bool, theme: ResolvedTheme) -> Result<()> {
    let preference = DWMWCP_ROUND;
    let border_color = compositor_border_color(visible_borders, theme).unwrap_or(DWMWA_COLOR_NONE);
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

// The renderer still declares its own copy of the library's close-button state.
const fn renderer_close_button_state(
    state: overlay_pointer::CloseButtonVisualState,
) -> CloseButtonVisualState {
    match state {
        overlay_pointer::CloseButtonVisualState::Normal => CloseButtonVisualState::Normal,
        overlay_pointer::CloseButtonVisualState::Hovered => CloseButtonVisualState::Hovered,
        overlay_pointer::CloseButtonVisualState::Pressed => CloseButtonVisualState::Pressed,
    }
}

fn request_foreground(window: HWND) -> bool {
    // SAFETY: window is a borrowed overlay or selected application HWND.
    unsafe { SetForegroundWindow(window).as_bool() }
}

fn activate_and_hide(target: HWND, hide: impl FnOnce()) -> bool {
    // Keep the overlay's foreground permission until the target queue has received the
    // activation request. Hiding first can return foreground ownership to another process.
    let activated = activate_window(target);
    hide();
    activated
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
        let restore_posted = unsafe {
            // SAFETY: target is a borrowed HWND; ShowWindowAsync does not transfer ownership.
            ShowWindowAsync(target, SW_RESTORE)
        };
        if !restore_posted.as_bool() {
            eprintln!("Could not restore the minimized window before activating it");
        }
    }

    // Separate queues make target activation asynchronous even if the target is hung.
    // Ordinary switching still owns the foreground overlay here; Start/Search switching
    // retains the permission supplied by the physical registered Tab hotkey.
    let activated = request_foreground(target);
    activated
        || unsafe {
            // SAFETY: GetForegroundWindow has no preconditions and returns a borrowed window.
            GetForegroundWindow()
        } == target
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

const fn switcher_session_settings(settings: &Settings) -> SwitcherSessionSettings {
    SwitcherSessionSettings {
        typed_search: settings.general.typed_search,
        release_alt_switches: settings.general.release_alt_switches,
        release_right_button_switches: settings.general.release_right_button_switches,
    }
}

const fn key_was_previously_down(lparam: LPARAM) -> bool {
    let previous_key_state_mask = 1_isize << 30;
    lparam.0 & previous_key_state_mask != 0
}

fn mouse_coordinates(lparam: LPARAM) -> (i32, i32) {
    let x = low_word_isize(lparam.0).cast_signed();
    let y = high_word_isize(lparam.0).cast_signed();
    (i32::from(x), i32::from(y))
}

fn close_target_for_hit(switcher: &Switcher, hit: Option<TaskListHit>) -> Option<isize> {
    let TaskListHit::CloseButton(position) = hit? else {
        return None;
    };
    let selected_position = switcher.selected_visible_index()?.checked_add(1)?;
    (position == selected_position).then_some(switcher.selected_task()?.window_handle)
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
#[path = "activation_tests.rs"]
mod activation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::switcher::SwitchTask;

    #[test]
    #[ignore = "requires two responsive desktop windows; changes foreground focus"]
    fn activation_switches_between_foreign_windows() {
        let handles = std::env::var("ALTTABIO_TEST_ACTIVATION_WINDOWS")
            .unwrap_or_else(|_| panic!("Set ALTTABIO_TEST_ACTIVATION_WINDOWS to two HWNDs"));
        let handles: Vec<isize> = handles
            .split(',')
            .map(|value| {
                value
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("Invalid HWND"))
            })
            .collect();
        assert_eq!(handles.len(), 2);
        assert_ne!(handles[0], handles[1]);
        for handle in handles.iter().cycle().take(10) {
            let window = HWND(*handle as *mut c_void);
            let mut process = 0;
            // SAFETY: the supplied HWND is borrowed and the process output is writable.
            assert_ne!(
                unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) },
                0
            );
            assert_ne!(
                process,
                std::process::id(),
                "Use windows from other processes"
            );
            assert!(
                activate_window(window),
                "Activation was rejected for {handle}"
            );
            // SetForegroundWindow may return before a foreign input queue processes activation.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                // SAFETY: this query has no preconditions and transfers no ownership.
                if unsafe { GetForegroundWindow() } == window {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Window {handle} never became foreground"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn modal_dialog_messages_run_outside_the_app_state_borrow() {
        assert!(is_modal_dialog_message(WM_SHOW_SETTINGS));
        assert!(is_modal_dialog_message(WM_SHOW_ABOUT));
        assert!(!is_modal_dialog_message(WM_HOOK_ACTION));
    }

    #[test]
    fn listed_refresh_messages_are_acknowledged_when_app_state_is_busy() {
        assert_eq!(
            busy_overlay_message_action(WM_FOREGROUND_CHECK, WPARAM(0)),
            BusyOverlayMessage::RetryForegroundCheck
        );
        assert_eq!(
            busy_overlay_message_action(WM_LISTED_WINDOW_REFRESH, WPARAM(0)),
            BusyOverlayMessage::AcknowledgeDroppedRefresh
        );
        assert_eq!(
            busy_overlay_message_action(WM_TIMER, WPARAM(LISTED_REFRESH_RETRY_TIMER_ID)),
            BusyOverlayMessage::IgnoreRetryTick
        );
        assert_eq!(
            busy_overlay_message_action(WM_PAINT, WPARAM(0)),
            BusyOverlayMessage::DeferToDefault
        );
    }

    #[test]
    fn modal_menu_reentry_cannot_lose_a_listed_refresh_notice() {
        use alttabio::task_refresh::{ListedRefreshSignal, RefreshWakeup};

        let signal = ListedRefreshSignal::new();
        let mut refresh = TaskListRefresh::default();

        assert_eq!(signal.record(10), RefreshWakeup::PostNow);
        assert_eq!(
            busy_overlay_message_action(WM_LISTED_WINDOW_REFRESH, WPARAM(0)),
            BusyOverlayMessage::AcknowledgeDroppedRefresh
        );

        signal.post_failed();
        assert!(signal.needs_retry_wakeup());
        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Ignore
        );

        let batch = signal.take_retry();
        assert!(batch.is_some());
        apply_listed_refresh_batch(
            &mut refresh,
            batch.unwrap_or_else(alttabio::task_refresh::RefreshBatch::empty),
        );
        assert!(!signal.is_queued());
        assert!(!signal.is_dirty());
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        let stale = [alttabio::switcher::SwitchTask::new(
            1, 10, "Closing", "editor",
        )];
        assert_eq!(refresh.complete_enumeration(Ok(&stale)), RetryTimer::Start);
        assert!(refresh.has_pending_retries());
        assert_eq!(signal.record(20), RefreshWakeup::PostNow);
    }

    #[test]
    fn production_hook_starts_synchronize_passthrough_immediately() {
        let mut hooks = None;
        let mut synchronized = None;

        assert_eq!(
            store_started_hook_then_sync(&mut hooks, Ok::<_, ()>(42), |hook| {
                synchronized = Some(*hook);
            }),
            Ok(())
        );

        assert_eq!(hooks, Some(42));
        assert_eq!(synchronized, Some(42));

        let mut failed_hooks = None;
        let mut failure_synchronized = false;
        assert_eq!(
            store_started_hook_then_sync(&mut failed_hooks, Err::<i32, _>("start failed"), |_| {
                failure_synchronized = true;
            },),
            Err("start failed")
        );
        assert_eq!(failed_hooks, None);
        assert!(!failure_synchronized);
    }

    #[test]
    fn modal_dialogs_gate_hook_actions() {
        assert!(hook_actions_enabled(false, false));
        assert!(!hook_actions_enabled(true, false));
        assert!(!hook_actions_enabled(false, true));
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
}
