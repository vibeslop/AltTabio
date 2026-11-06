use crate::process_info::executable_path;
use crate::win32::module_instance;
use std::fs::File;
use std::marker::PhantomData;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, E_FAIL, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM,
    LRESULT, WPARAM,
};
use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, ReleaseMutex,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE,
    RegisterClassW, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCCREATE, WNDCLASSW,
};
use windows::core::{Error, PCWSTR, Result, w};

const MUTEX_NAME: PCWSTR = w!("Local\\AltTabio.SingleInstance");
const PEER_CLASS: PCWSTR = w!("AltTabio.SingleInstance.Peer.v1");
const STARTUP_GRACE: Duration = Duration::from_secs(2);

pub struct SingleInstance {
    // Destroy the rendezvous before releasing the startup gate.
    _peer: PeerWindow,
    _mutex: OwnedMutex,
}

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>> {
        Self::acquire_named(MUTEX_NAME, PEER_CLASS, STARTUP_GRACE)
    }

    fn acquire_named(name: PCWSTR, class: PCWSTR, grace: Duration) -> Result<Option<Self>> {
        Self::acquire_with_peer_probe(name, class, grace, authenticated_peer)
    }

    fn acquire_with_peer_probe(
        name: PCWSTR,
        class: PCWSTR,
        grace: Duration,
        mut probe: impl FnMut(PCWSTR) -> Result<bool>,
    ) -> Result<Option<Self>> {
        let started = Instant::now();
        loop {
            let created = unsafe {
                // SAFETY: name is terminated and live for this call. A successful call gives us
                // one owned handle, but initial mutex ownership only if the object is NEW.
                CreateMutexW(None, true, name)
            };
            let open_error = match created {
                Ok(handle) => {
                    let already_exists = unsafe {
                        // SAFETY: no intervening Win32 call changed CreateMutexW's last error.
                        GetLastError()
                    } == ERROR_ALREADY_EXISTS;
                    if !already_exists {
                        let mutex = OwnedMutex {
                            handle,
                            _same_thread: PhantomData,
                        };
                        return Ok(Some(Self {
                            _peer: PeerWindow::create(class)?,
                            _mutex: mutex,
                        }));
                    }
                    // Never wait on, take ownership of, or release a pre-existing mutex. Close
                    // our handle before retrying so it cannot keep a departing object alive.
                    close_handle(handle, "duplicate-instance mutex");
                    None
                }
                // A lower-integrity duplicate may not open the elevated instance's mutex with
                // CreateMutexW's ALL_ACCESS request. Authenticate its peer independently.
                Err(error) => Some(error),
            };
            // A peer can exit between obtaining its window PID and querying its executable.
            // Retry authentication failures within the same bounded startup grace period;
            // never turn an unverifiable peer into an authenticated duplicate.
            let peer_error = match probe(class) {
                Ok(true) => return Ok(None),
                Ok(false) => None,
                Err(error) => Some(error),
            };
            if started.elapsed() >= grace {
                let detail = peer_error.or(open_error).map_or_else(
                    || "The named startup gate has no verified AltTabio instance.".to_owned(),
                    |error| format!("Could not open the startup gate: {error}"),
                );
                return Err(conflict(&detail));
            }
            // Only startup waits, never a native callback. Allow the winning concurrent launch
            // to publish its peer, or a shutting-down instance to release its gate.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn conflict(detail: &str) -> Error {
    Error::new(
        E_FAIL,
        format!(
            "AltTabio could not verify another running instance. {detail} Close the conflicting \
             application and try again."
        ),
    )
}

fn authenticated_peer(class: PCWSTR) -> Result<bool> {
    let mut previous = None;
    // Window names/classes are untrusted. Bound work even if another process floods this class.
    for _ in 0..64 {
        let found = unsafe {
            // SAFETY: the class is terminated and HWND_MESSAGE limits discovery to this desktop's
            // message-only windows. All returned HWNDs are BORROWED and are never destroyed here.
            FindWindowExW(Some(HWND_MESSAGE), previous, class, None)
        };
        let Ok(window) = found else {
            // FindWindowExW also reports no match via a null result, with no defined last error.
            return Ok(false);
        };
        if authentic_window(window)
            .map_err(|error| conflict(&format!("Could not verify the startup window: {error}")))?
        {
            return Ok(true);
        }
        previous = Some(window);
    }
    Err(conflict(
        "Too many unverified startup windows share the rendezvous class.",
    ))
}

fn authentic_window(window: HWND) -> Result<bool> {
    let process_id = window_process_id(window);
    if process_id == 0 {
        return Ok(false);
    }
    let process = unsafe {
        // SAFETY: the OS supplies this PID. Query-only access works across UAC integrity levels;
        // the owned handle pins this process identity throughout verification.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
    }
    .map_err(|error| {
        conflict(&format!(
            "Could not query the startup window's process: {error}"
        ))
    })?;
    let process = OwnedProcess(process);
    let peer = executable_path(process.0)?;
    let current = unsafe {
        // SAFETY: GetCurrentProcess returns a borrowed pseudo-handle, never closed by this code.
        executable_path(GetCurrentProcess())
    }?;
    // Compare file identity, not just the AltTabio basename or an attacker-supplied window title.
    // This also accepts alternate path spellings and hard links to this same executable.
    Ok(
        file_identity(&peer)? == file_identity(&current)?
            && window_process_id(window) == process_id,
    )
}

fn window_process_id(window: HWND) -> u32 {
    let mut process_id = 0;
    let thread_id = unsafe {
        // SAFETY: the HWND is only queried, and process_id is a live writable scalar.
        GetWindowThreadProcessId(window, Some(&raw mut process_id))
    };
    if thread_id == 0 {
        // A disappearing or invalid HWND is never accepted as a genuine peer.
        return 0;
    }
    process_id
}

fn file_identity(path: &Path) -> Result<(u32, u32, u32)> {
    let file = File::open(path).map_err(Error::from)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        // SAFETY: File owns a live file handle for the entire query; info is writable.
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &raw mut info)?;
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

struct PeerWindow {
    window: HWND,
    class_atom: u16,
    instance: HINSTANCE,
    // HWND destruction and ReleaseMutex must run on the creating thread, even during unwinding.
    _same_thread: PhantomData<Rc<()>>,
}

impl PeerWindow {
    fn create(class: PCWSTR) -> Result<Self> {
        let instance = module_instance()?;
        let definition = WNDCLASSW {
            lpfnWndProc: Some(peer_window_proc),
            hInstance: instance,
            lpszClassName: class,
            ..WNDCLASSW::default()
        };
        let atom = unsafe {
            // SAFETY: the definition and terminated class name are live; Windows copies them.
            RegisterClassW(&raw const definition)
        };
        if atom == 0 {
            return Err(Error::from_thread());
        }
        let mut peer = Self {
            window: HWND::default(),
            class_atom: atom,
            instance,
            _same_thread: PhantomData,
        };
        peer.window = unsafe {
            // SAFETY: this process owns the registered class. No pointer or app state is passed
            // to the procedure, and the resulting message-only window belongs to this guard.
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                None,
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            )
        }?;
        Ok(peer)
    }
}

unsafe extern "system" fn peer_window_proc(
    _window: HWND,
    message: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    // No application state, I/O, or blocking work. Ignore all messages, including WM_CLOSE, so
    // foreign messages cannot destroy the guard's window. Contain panics at the native boundary.
    std::panic::catch_unwind(|| LRESULT(isize::from(message == WM_NCCREATE))).unwrap_or_default()
}

impl Drop for PeerWindow {
    fn drop(&mut self) {
        if !self.window.is_invalid() {
            let destroyed = unsafe {
                // SAFETY: this is our uniquely owned HWND, destroyed once on its creating thread.
                DestroyWindow(self.window)
            };
            if let Err(error) = destroyed {
                eprintln!("Could not destroy the single-instance rendezvous: {error}");
            }
        }
        let unregistered = unsafe {
            // SAFETY: this guard owns the registered class atom. Win32 accepts MAKEINTATOM
            // values here, so no caller-owned string has to outlive the guard.
            UnregisterClassW(
                PCWSTR(usize::from(self.class_atom) as *const u16),
                Some(self.instance),
            )
        };
        if let Err(error) = unregistered {
            eprintln!("Could not unregister the single-instance rendezvous: {error}");
        }
    }
}

struct OwnedMutex {
    handle: HANDLE,
    _same_thread: PhantomData<Rc<()>>,
}

impl Drop for OwnedMutex {
    fn drop(&mut self) {
        let release_result = unsafe {
            // SAFETY: only a freshly created mutex enters this guard. Initial ownership is
            // released exactly once on its creating thread before closing its owned handle.
            ReleaseMutex(self.handle)
        };
        if let Err(error) = release_result {
            eprintln!("Could not release the single-instance mutex: {error}");
        }
        close_handle(self.handle, "single-instance mutex");
    }
}

struct OwnedProcess(HANDLE);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        close_handle(self.0, "single-instance process query");
    }
}

fn close_handle(handle: HANDLE, description: &str) {
    let result = unsafe {
        // SAFETY: the caller transfers one uniquely owned kernel handle for exactly one close.
        CloseHandle(handle)
    };
    if let Err(error) = result {
        eprintln!("Could not close the {description}: {error}");
    }
}

#[cfg(test)]
mod tests;
