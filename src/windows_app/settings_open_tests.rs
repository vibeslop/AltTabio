//! First-open checks use a fresh process on a private desktop, without desktop input.

use super::*;
use crate::settings_io::SettingsStore;
use crate::win32::{ComApartment, module_instance, wide};
use alttabio::settings::Theme;
use alttabio::settings_form::Control;
use std::cell::Cell;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Gdi::GetUpdateRect;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, HDESK,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION,
    STARTF_USESHOWWINDOW, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, CreateWindowExW, DestroyWindow, DispatchMessageW, EnumChildWindows, GetDlgItem,
    IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, SHOW_WINDOW_CMD, SW_HIDE, SW_MINIMIZE,
    SW_SHOWMINIMIZED, SendMessageW, WM_CLOSE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, Error, PCWSTR, PWSTR, Result};

#[derive(Default)]
struct StartupPaint {
    visible_children: usize,
    pending_children: usize,
}

unsafe extern "system" fn inspect_startup_paint(window: HWND, data: LPARAM) -> BOOL {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: EnumChildWindows borrows this fixture's writable StartupPaint for the call.
        let paint = unsafe { &mut *(data.0 as *mut StartupPaint) };
        // SAFETY: enumeration supplies a live descendant; both calls only query its state.
        unsafe {
            if IsWindowVisible(window).as_bool() {
                paint.visible_children = paint.visible_children.saturating_add(1);
                if GetUpdateRect(window, None, false).as_bool() {
                    paint.pending_children = paint.pending_children.saturating_add(1);
                }
            }
        }
        BOOL(1)
    }))
    .unwrap_or(BOOL(0))
}

fn assert_startup_painted(window: HWND) {
    let mut paint = StartupPaint::default();
    unsafe {
        // SAFETY: the fixture storage outlives synchronous enumeration. The callback
        // only queries live children and updates bounded counters without panicking.
        let _enumerated = EnumChildWindows(
            Some(window),
            Some(inspect_startup_paint),
            LPARAM(std::ptr::from_mut(&mut paint) as isize),
        );
    }
    assert!(
        paint.visible_children > 0,
        "Settings has no visible controls"
    );
    assert_eq!(
        paint.pending_children, 0,
        "Settings appeared with unpainted child controls before entering its message loop"
    );
    // SAFETY: the dialog remains live and querying its update region does not paint it.
    assert!(
        !unsafe { GetUpdateRect(window, None, false) }.as_bool(),
        "Settings appeared with an unpainted background"
    );
}

struct Desktop(HDESK);

impl Drop for Desktop {
    fn drop(&mut self) {
        // SAFETY: this guard owns the desktop; the fixture process has exited before drop.
        assert!(unsafe { CloseDesktop(self.0) }.is_ok());
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the handle returned by CreateProcessW.
        assert!(unsafe { CloseHandle(self.0) }.is_ok());
    }
}

struct Overlay(HWND);

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: this fixture owns the overlay and its host still outlives the window.
        assert!(unsafe { DestroyWindow(self.0) }.is_ok());
    }
}

#[test]
fn first_settings_request_ignores_startup_show_state() -> Result<()> {
    let mut desktop_name = wide(&format!("AltTabio-settings-open-{}", std::process::id()));
    let desktop = Desktop(unsafe {
        // SAFETY: the name is terminated and the test uniquely owns the new private desktop.
        CreateDesktopW(
            PCWSTR(desktop_name.as_ptr()),
            PCWSTR::null(),
            None,
            DESKTOP_CONTROL_FLAGS(0),
            0x01ff,
            None,
        )
    }?);
    for show_state in [SW_SHOW, SW_HIDE, SW_SHOWMINIMIZED] {
        for fixture in [
            "first_settings_open_fixture",
            "first_dark_settings_open_fixture",
            "first_about_open_fixture",
        ] {
            run_fixture(&mut desktop_name, show_state, fixture)?;
        }
    }
    drop(desktop);
    Ok(())
}

fn run_fixture(desktop_name: &mut [u16], show_state: SHOW_WINDOW_CMD, fixture: &str) -> Result<()> {
    let executable =
        std::env::current_exe().map_err(|error| Error::new(E_FAIL, error.to_string()))?;
    let application: Vec<u16> = executable.as_os_str().encode_wide().chain([0]).collect();
    let mut command = std::ffi::OsString::from("\"");
    command.push(executable);
    command.push(format!(
        "\" --ignored --exact windows_app::dialogs::first_open_tests::{fixture} --quiet",
    ));
    let mut command: Vec<u16> = command.encode_wide().chain([0]).collect();
    let startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or_default(),
        lpDesktop: PWSTR(desktop_name.as_mut_ptr()),
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: u16::try_from(show_state.0).unwrap_or_default(),
        ..STARTUPINFOW::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        // SAFETY: all terminated buffers and output structures outlive this synchronous call.
        // Only the child fixture uses the private desktop; the input desktop is never switched.
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &raw const startup,
            &raw mut process,
        )?;
    }
    let process_handle = Handle(process.hProcess);
    let thread_handle = Handle(process.hThread);
    drop(thread_handle);
    // SAFETY: the owned process handle stays live throughout the bounded wait.
    if unsafe { WaitForSingleObject(process_handle.0, 10_000) } != WAIT_OBJECT_0 {
        unsafe {
            // SAFETY: only this test-created fixture is terminated if it fails to finish.
            TerminateProcess(process_handle.0, 101)?;
        }
        // SAFETY: termination is asynchronous, so wait before releasing its private desktop.
        assert_eq!(
            unsafe { WaitForSingleObject(process_handle.0, 2_000) },
            WAIT_OBJECT_0,
            "the Settings fixture did not terminate"
        );
        return Err(Error::new(E_FAIL, "Settings first-open fixture timed out"));
    }
    let mut exit_code = 0;
    unsafe {
        // SAFETY: the owned process handle is live and exit_code is writable.
        GetExitCodeProcess(process_handle.0, &raw mut exit_code)?;
    }
    assert_eq!(
        exit_code, 0,
        "{fixture} must show a fully painted, non-minimized Settings window with startup state {show_state:?}"
    );
    Ok(())
}

#[test]
#[ignore = "run by the parent test in a fresh process on a private desktop"]
fn first_settings_open_fixture() -> Result<()> {
    check_first_settings_open(Theme::Light, Category::General)
}

#[test]
#[ignore = "run by the parent test in a fresh process on a private desktop"]
fn first_dark_settings_open_fixture() -> Result<()> {
    check_first_settings_open(Theme::Dark, Category::General)
}

#[test]
#[ignore = "run by the parent test in a fresh process on a private desktop"]
fn first_about_open_fixture() -> Result<()> {
    check_first_settings_open(Theme::Dark, Category::About)
}

fn assert_selected_page(window: HWND, category: Category) -> Result<()> {
    for candidate in Category::ALL {
        let id = Control::Navigation(candidate).id().unwrap_or_default();
        // SAFETY: the live dialog owns navigation buttons with these bounded control IDs.
        let navigation =
            unsafe { GetDlgItem(Some(window), i32::try_from(id).unwrap_or_default()) }?;
        // SAFETY: this is the live native radio button, queried without pointer payloads.
        assert_eq!(
            unsafe { SendMessageW(navigation, BM_GETCHECK, None, None) }.0 != 0,
            candidate == category
        );
    }
    Ok(())
}

fn check_first_settings_open(theme: Theme, category: Category) -> Result<()> {
    let _apartment = ComApartment::initialize()?;
    let (store, mut settings) = SettingsStore::load_from(
        std::env::temp_dir().join(format!(
            "alttabio-first-settings-test-{}.ini",
            std::process::id()
        )),
        &Settings::default(),
    )
    .map_err(|error| Error::new(E_FAIL, error))?;
    settings.appearance.theme = theme;
    let host = Box::new(AppHost::new(App::new(false, false, settings, store)?));
    let instance = module_instance()?;
    super::super::register_window_class(instance)?;
    let overlay = Overlay(unsafe {
        // SAFETY: this stable host allocation outlives the production overlay procedure.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            super::super::WINDOW_CLASS,
            super::super::WINDOW_TITLE,
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance),
            Some(std::ptr::from_ref(host.as_ref()).cast()),
        )
    }?);
    let Some((owner, settings)) = host.state.borrow_mut().prepare_settings_dialog() else {
        panic!("the first Settings request must be accepted");
    };
    let visible = Cell::new(false);
    let minimized = Cell::new(false);
    let on_open = |window: HWND| {
        unsafe {
            // SAFETY: Settings owns this live window on the fixture's UI thread.
            visible.set(IsWindowVisible(window).as_bool());
            minimized.set(IsIconic(window).as_bool());
        }
        assert_startup_painted(window);
        assert!(assert_selected_page(window, category).is_ok());
        host.settings_window.set(Some(window));
        for _ in 0..2 {
            host.show_settings();
            unsafe {
                // SAFETY: the same Settings window remains owned by this fixture.
                assert!(
                    IsWindowVisible(window).as_bool(),
                    "reopening Settings hid it"
                );
                assert!(
                    !IsIconic(window).as_bool(),
                    "reopening Settings minimized it"
                );
            }
        }
        assert!(
            assert_selected_page(window, category).is_ok(),
            "ordinary Settings requests must retain the selected page"
        );
        for show_state in [SW_HIDE, SW_MINIMIZE] {
            // SAFETY: only this fixture's private-desktop dialog changes visibility.
            unsafe {
                let _previous = ShowWindow(window, show_state);
            }
            // Use the same overlay message as the tray's About command.
            // SAFETY: the overlay and host belong to this fixture; no state borrow spans dispatch.
            unsafe {
                SendMessageW(owner, crate::app_messages::WM_SHOW_ABOUT, None, None);
            }
            let mut message = MSG::default();
            // SAFETY: drain only this fixture's Settings events on the owning UI thread.
            while unsafe { PeekMessageW(&raw mut message, Some(window), 0, 0, PM_REMOVE) }.as_bool()
            {
                // SAFETY: PeekMessageW initialized this scalar event and target window.
                unsafe {
                    DispatchMessageW(&raw const message);
                }
            }
            assert_eq!(host.settings_window.get(), Some(window));
            assert!(assert_selected_page(window, Category::About).is_ok());
            // SAFETY: both queries read the same fixture-owned window's visibility.
            unsafe {
                assert!(IsWindowVisible(window).as_bool());
                assert!(!IsIconic(window).as_bool());
            }
            assert!(!host.state.borrow().dialog_state().blocks_switcher());
        }
        unsafe {
            // SAFETY: the close request contains no borrowed data and runs on this UI thread.
            assert!(PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok());
        }
    };
    let result = settings_dialog::show(owner, &settings, category, on_open, |_| false)?;
    assert!(result.is_none(), "the fixture must close without saving");
    assert!(
        visible.get(),
        "the first Settings request left its window hidden"
    );
    assert!(
        !minimized.get(),
        "Settings inherited the startup minimized state"
    );
    drop(overlay);
    Ok(())
}
