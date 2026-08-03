use alttabio::input::WindowCommand;
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetWindowThreadProcessId, HMENU,
    MF_STRING, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SetForegroundWindow,
    ShowWindowAsync, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_CLOSE,
};
use windows::core::{PWSTR, w};

pub fn show_menu(owner: HWND) -> Option<WindowCommand> {
    let menu = unsafe {
        // SAFETY: CreatePopupMenu has no pointer preconditions and returns a uniquely owned menu.
        CreatePopupMenu()
    }
    .ok()
    .map(OwnedMenu)?;
    let items = [
        (1, w!("Close\tF4"), WindowCommand::Close),
        (2, w!("Minimize\tF5"), WindowCommand::Minimize),
        (3, w!("Maximize\tF6"), WindowCommand::Maximize),
        (4, w!("Restore\tF7"), WindowCommand::Restore),
        (5, w!("Terminate\tF8"), WindowCommand::Terminate),
        (6, w!("Run\tF9"), WindowCommand::Run),
    ];
    for (id, label, _) in &items {
        let added = unsafe {
            // SAFETY: menu is live, ids are application-owned, and labels are static UTF-16.
            AppendMenuW(menu.0, MF_STRING, *id, *label)
        };
        if added.is_err() {
            return None;
        }
    }
    let mut cursor = windows::Win32::Foundation::POINT::default();
    unsafe {
        // SAFETY: cursor is writable and owner is the live overlay HWND.
        GetCursorPos(&raw mut cursor).ok()?;
        let _foreground = SetForegroundWindow(owner);
    }
    let selected = unsafe {
        // SAFETY: menu and owner are live and cursor contains screen coordinates. TPM_RETURNCMD
        // keeps command delivery synchronous and scalar.
        TrackPopupMenu(
            menu.0,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            None,
            owner,
            None,
        )
    };
    let selected = usize::try_from(selected.0).ok()?;
    items
        .iter()
        .find_map(|(id, _, command)| (*id == selected).then_some(*command))
}

pub fn execute(command: WindowCommand, window_handle: isize) -> bool {
    let window = HWND(window_handle as *mut c_void);
    match command {
        WindowCommand::Close => close_window(window),
        WindowCommand::Minimize => show_window(window, SW_MINIMIZE),
        WindowCommand::Maximize => show_window(window, SW_MAXIMIZE),
        WindowCommand::Restore => show_window(window, SW_RESTORE),
        WindowCommand::Terminate => terminate_window_process(window),
        WindowCommand::Run => run_window_process(window),
    }
}

fn close_window(window: HWND) -> bool {
    let posted = execute_close_with(window, |target, message, wparam, lparam| unsafe {
        // SAFETY: `target` is borrowed from the current switcher snapshot and the posted system
        // message contains only scalar values. No Rust references cross the message boundary.
        PostMessageW(Some(target), message, wparam, lparam).is_ok()
    });
    if !posted {
        eprintln!("Could not post the close request to the selected window");
    }
    posted
}

fn execute_close_with(window: HWND, post: impl FnOnce(HWND, u32, WPARAM, LPARAM) -> bool) -> bool {
    post(window, WM_CLOSE, WPARAM(0), LPARAM(0))
}

fn show_window(
    window: HWND,
    command: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD,
) -> bool {
    unsafe {
        // SAFETY: the HWND is borrowed from the current switcher snapshot and ShowWindowAsync does
        // not transfer ownership or retain a pointer.
        ShowWindowAsync(window, command).as_bool()
    }
}

fn terminate_window_process(window: HWND) -> bool {
    let Some(process_id) = process_id(window) else {
        return false;
    };
    if process_id == std::process::id() {
        return false;
    }
    let process = match unsafe {
        // SAFETY: the process id was read from the selected HWND and the requested access is
        // limited to termination.
        OpenProcess(PROCESS_TERMINATE, false, process_id)
    } {
        Ok(handle) => OwnedHandle(handle),
        Err(error) => {
            eprintln!("Could not open the selected process for termination: {error}");
            return false;
        }
    };
    let result = unsafe {
        // SAFETY: the guard owns a live process handle with PROCESS_TERMINATE access.
        TerminateProcess(process.0, 1)
    };
    if let Err(error) = result {
        eprintln!("Could not terminate the selected process: {error}");
        false
    } else {
        true
    }
}

fn run_window_process(window: HWND) -> bool {
    let Some(path) = executable_path(window) else {
        return false;
    };
    match Command::new(&path).spawn() {
        Ok(_child) => true,
        Err(error) => {
            eprintln!("Could not start {}: {error}", path.display());
            false
        }
    }
}

fn executable_path(window: HWND) -> Option<PathBuf> {
    let process_id = process_id(window)?;
    let process = unsafe {
        // SAFETY: the process id was read from the selected HWND and access is query-only.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
    }
    .ok()
    .map(OwnedHandle)?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = u32::try_from(buffer.len()).ok()?;
    unsafe {
        // SAFETY: the process handle is live and query-only, while the UTF-16 buffer and length are
        // writable for the synchronous call.
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    let length = usize::try_from(length).ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        buffer.get(..length)?,
    )))
}

fn process_id(window: HWND) -> Option<u32> {
    let mut process_id = 0;
    unsafe {
        // SAFETY: process_id is writable and the HWND is borrowed from the current snapshot.
        GetWindowThreadProcessId(window, Some(&raw mut process_id));
    }
    (process_id != 0).then_some(process_id)
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

struct OwnedMenu(HMENU);

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        let result = unsafe {
            // SAFETY: this guard uniquely owns the popup menu and destroys it exactly once.
            DestroyMenu(self.0)
        };
        if let Err(error) = result {
            eprintln!("Could not destroy the task command menu: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_command_posts_to_one_window_without_waiting_for_shutdown() {
        let target = HWND(42_isize as *mut c_void);
        let mut request = None;

        assert!(execute_close_with(
            target,
            |window, message, wparam, lparam| {
                request = Some((window, message, wparam, lparam));
                true
            }
        ));

        let Some((window, message, wparam, lparam)) = request else {
            panic!("close transport was not called");
        };
        assert_eq!(window, target);
        assert_eq!(message, WM_CLOSE);
        assert_eq!(wparam, WPARAM(0));
        assert_eq!(lparam, LPARAM(0));
    }

    #[test]
    fn close_command_reports_transport_failure() {
        let target = HWND(42_isize as *mut c_void);

        assert!(!execute_close_with(target, |_, _, _, _| false));
    }
}
