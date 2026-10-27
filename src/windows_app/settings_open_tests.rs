//! First-open checks use a fresh process on a private desktop, without desktop input.

use super::*;
use crate::settings_io::SettingsStore;
use crate::win32::{ComApartment, module_instance, wide};
use std::cell::Cell;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, HDESK,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION,
    STARTF_USESHOWWINDOW, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, IsWindowVisible, SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWMINIMIZED,
    WM_CLOSE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{Error, PCWSTR, PWSTR, Result};

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
        run_fixture(&mut desktop_name, show_state)?;
    }
    drop(desktop);
    Ok(())
}

fn run_fixture(desktop_name: &mut [u16], show_state: SHOW_WINDOW_CMD) -> Result<()> {
    let executable =
        std::env::current_exe().map_err(|error| Error::new(E_FAIL, error.to_string()))?;
    let application: Vec<u16> = executable.as_os_str().encode_wide().chain([0]).collect();
    let mut command = std::ffi::OsString::from("\"");
    command.push(executable);
    command.push(
        "\" --ignored --exact windows_app::dialogs::first_open_tests::first_settings_open_fixture --quiet",
    );
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
        "the first Settings request must show a non-minimized window with startup state {show_state:?}"
    );
    Ok(())
}

#[test]
#[ignore = "run by the parent test in a fresh process on a private desktop"]
fn first_settings_open_fixture() -> Result<()> {
    let _apartment = ComApartment::initialize()?;
    let (store, settings) = SettingsStore::load_from(
        std::env::temp_dir().join(format!(
            "alttabio-first-settings-test-{}.ini",
            std::process::id()
        )),
        &Settings::default(),
    )
    .map_err(|error| Error::new(E_FAIL, error))?;
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
    let result = settings_dialog::show(owner, &settings, |window| {
        unsafe {
            // SAFETY: Settings owns this live window on the fixture's UI thread.
            visible.set(IsWindowVisible(window).as_bool());
            minimized.set(IsIconic(window).as_bool());
        }
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
        unsafe {
            // SAFETY: the close request contains no borrowed data and runs on this UI thread.
            assert!(PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok());
        }
    })?;
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
