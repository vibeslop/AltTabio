use alttabio::input::WindowCommand;
use alttabio::switcher::ProcessIdentity;
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation,
};
use windows::Win32::System::Threading::{
    CreateProcessWithTokenW, LOGON_WITH_PROFILE, OpenProcess, OpenProcessToken,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, STARTUPINFOW,
    TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetShellWindow,
    GetWindowThreadProcessId, HMENU, MF_STRING, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SetForegroundWindow, ShowWindowAsync, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_CLOSE,
};
use windows::core::{PCWSTR, PWSTR, w};

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

pub fn execute(
    command: WindowCommand,
    window_handle: isize,
    process_identity: ProcessIdentity,
) -> bool {
    let window = HWND(window_handle as *mut c_void);
    match command {
        WindowCommand::Close => close_window(window),
        WindowCommand::Minimize => show_window(window, SW_MINIMIZE),
        WindowCommand::Maximize => show_window(window, SW_MAXIMIZE),
        WindowCommand::Restore => show_window(window, SW_RESTORE),
        WindowCommand::Terminate => terminate_window_process(window, process_identity),
        WindowCommand::Run => run_window_process(window, process_identity),
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

fn terminate_window_process(window: HWND, process_identity: ProcessIdentity) -> bool {
    if process_identity.id == std::process::id() {
        return false;
    }
    let Some(process) = open_selected_process(
        window,
        process_identity,
        PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
    ) else {
        return false;
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

fn run_window_process(window: HWND, process_identity: ProcessIdentity) -> bool {
    let Some(process) =
        open_selected_process(window, process_identity, PROCESS_QUERY_LIMITED_INFORMATION)
    else {
        return false;
    };
    let Some(path) = executable_path(process.0) else {
        return false;
    };
    if let Err(error) = launch_with_shell_token(&path) {
        eprintln!(
            "Could not start {} without elevation: {error}",
            path.display()
        );
        return false;
    }
    true
}

fn executable_path(process: HANDLE) -> Option<PathBuf> {
    let mut buffer = vec![0_u16; 32_768];
    let mut length = u32::try_from(buffer.len()).ok()?;
    unsafe {
        // SAFETY: the process handle is live and query-only, while the UTF-16 buffer and length are
        // writable for the synchronous call.
        QueryFullProcessImageNameW(
            process,
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

fn open_selected_process(
    window: HWND,
    expected: ProcessIdentity,
    access: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS,
) -> Option<OwnedHandle> {
    open_selected_process_with(
        window,
        expected,
        |process_id| unsafe {
            // SAFETY: process_id comes from the immutable switcher snapshot and the caller chooses
            // the minimum access needed for this command.
            OpenProcess(access, false, process_id).ok().map(OwnedHandle)
        },
        |process| process_started_at(process.0),
        process_id,
    )
}

fn open_selected_process_with<P>(
    window: HWND,
    expected: ProcessIdentity,
    mut open: impl FnMut(u32) -> Option<P>,
    mut started_at: impl FnMut(&P) -> Option<u64>,
    mut current_process_id: impl FnMut(HWND) -> Option<u32>,
) -> Option<P> {
    if expected.id == 0 || expected.started_at == 0 || current_process_id(window)? != expected.id {
        return None;
    }
    let process = open(expected.id)?;
    if started_at(&process)? != expected.started_at || current_process_id(window)? != expected.id {
        return None;
    }
    Some(process)
}

fn process_started_at(process: HANDLE) -> Option<u64> {
    let mut creation = windows::Win32::Foundation::FILETIME::default();
    let mut exit = windows::Win32::Foundation::FILETIME::default();
    let mut kernel = windows::Win32::Foundation::FILETIME::default();
    let mut user = windows::Win32::Foundation::FILETIME::default();
    unsafe {
        // SAFETY: process is live and queryable; all four FILETIME outputs are writable.
        windows::Win32::System::Threading::GetProcessTimes(
            process,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .ok()?;
    Some((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

fn launch_with_shell_token(path: &std::path::Path) -> Result<(), String> {
    let shell_window = unsafe {
        // SAFETY: GetShellWindow has no preconditions and returns a borrowed handle.
        GetShellWindow()
    };
    let shell_process_id = process_id(shell_window)
        .ok_or_else(|| "the Windows shell process could not be identified".to_owned())?;
    let shell_process = unsafe {
        // SAFETY: the process id belongs to the current interactive shell and access is query-only.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, shell_process_id)
    }
    .map(OwnedHandle)
    .map_err(|error| format!("the Windows shell process could not be opened: {error}"))?;
    let mut shell_token = HANDLE::default();
    unsafe {
        // SAFETY: shell_process is live and shell_token is writable. The requested rights are the
        // minimum CreateProcessWithTokenW requires for a primary process token.
        OpenProcessToken(
            shell_process.0,
            TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY,
            &raw mut shell_token,
        )
    }
    .map_err(|error| format!("the Windows shell token could not be opened: {error}"))?;
    let shell_token = OwnedHandle(shell_token);
    if token_is_elevated(shell_token.0)? {
        return Err("the Windows shell token is elevated".to_owned());
    }

    let application = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or_default(),
        ..STARTUPINFOW::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        // SAFETY: the shell token is a live primary token; application is null-terminated; startup
        // and process are writable for the synchronous creation call. No Rust references are kept.
        CreateProcessWithTokenW(
            shell_token.0,
            LOGON_WITH_PROFILE,
            PCWSTR(application.as_ptr()),
            None,
            PROCESS_CREATION_FLAGS::default(),
            None,
            PCWSTR::null(),
            &raw mut startup,
            &raw mut process,
        )
    }
    .map_err(|error| format!("the process could not be created with the shell token: {error}"))?;
    let _process = OwnedHandle(process.hProcess);
    let _thread = OwnedHandle(process.hThread);
    Ok(())
}

fn token_is_elevated(token: HANDLE) -> Result<bool, String> {
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned_bytes = 0;
    unsafe {
        // SAFETY: token is live, elevation is writable for its exact size, and returned_bytes is a
        // writable scalar output. GetTokenInformation retains no pointers.
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or_default(),
            &raw mut returned_bytes,
        )
    }
    .map_err(|error| format!("the Windows shell token could not be inspected: {error}"))?;
    Ok(elevation.TokenIsElevated != 0)
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

    #[test]
    fn stale_window_process_identity_is_not_opened_for_a_command() {
        let target = HWND(42_isize as *mut c_void);
        let expected = ProcessIdentity::new(100, 1_000);
        let mut opened = false;

        let process = open_selected_process_with(
            target,
            expected,
            |_| {
                opened = true;
                Some(())
            },
            |()| Some(1_000),
            |_| Some(200),
        );

        assert!(process.is_none());
        assert!(!opened);
    }

    #[test]
    fn reused_process_id_is_rejected_by_creation_time() {
        let target = HWND(42_isize as *mut c_void);
        let expected = ProcessIdentity::new(100, 1_000);

        let process = open_selected_process_with(
            target,
            expected,
            |_| Some(()),
            |()| Some(2_000),
            |_| Some(100),
        );

        assert!(process.is_none());
    }
}
