mod activation;
mod appearance;
mod dialogs;
mod dispatch;
mod display;
mod hooks;
mod mouse;
mod overlay;
mod passthrough;
mod refresh;
mod shell_dismissal;
mod wndproc;

use crate::app_messages::WM_DESTROY_APP;
use crate::hook::HookThread;
use crate::native_theme::resolve_current_theme;
use crate::preview::DwmPreview;
use crate::renderer::Renderer;
use crate::settings_io::SettingsStore;
use crate::single_instance::SingleInstance;
use crate::startup;
use crate::task_icon::TaskIcons;
use crate::tray::TrayIcon;
use crate::win_events::{self, WinEventWatcher};
use crate::win32::{module_instance, wide};
use alttabio::modal_state::ModalState;
use alttabio::overlay_pointer::CloseButtonInteraction;
use alttabio::settings::Settings;
use alttabio::settings_change::{hook_settings, switcher_session_settings};
use alttabio::switcher::SwitcherSession;
use alttabio::task_refresh::TaskListRefresh;
use alttabio::theme::ResolvedTheme;
use appearance::apply_window_appearance;
use shell_dismissal::PendingShellDismissal;
use std::cell::RefCell;
use std::mem::size_of;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MB_ICONERROR, MB_OK,
    MSG, MessageBoxW, PostMessageW, RegisterClassExW, TranslateMessage, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_THICKFRAME,
};
use windows::core::{Error, PCWSTR, Result, w};
use wndproc::window_proc;

const WINDOW_CLASS: PCWSTR = w!("AltTabioRustOverlay");
const WINDOW_TITLE: PCWSTR = w!("AltTabio");

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
    begin_paint_failing: bool,
    foreground_bounds_failing: bool,
    hit_test_failing: bool,
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
            begin_paint_failing: false,
            foreground_bounds_failing: false,
            hit_test_failing: false,
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

    fn show_error(&self, message: &str) {
        let _suspension = self.hooks.as_ref().map(HookThread::suspend_interception);
        show_error_for_window(self.hwnd, message);
    }

    fn is_visible(&self) -> bool {
        self.session.is_visible()
    }

    const fn modal_state(&self) -> ModalState {
        ModalState {
            settings_dialog: self.settings_dialog_open,
            about_dialog: self.about_dialog_open,
            context_menu: self.session.context_menu_open(),
        }
    }
}

fn show_error_for_window(owner: HWND, message: &str) {
    show_error_box(Some(owner), message);
}

fn show_error_box(owner: Option<HWND>, message: &str) {
    let text = wide(message);
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

// Only mouse.rs splits LPARAM through these; elsewhere the words come from `crate::win32`.
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
