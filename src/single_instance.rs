use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::core::{Result, w};

pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>> {
        let handle = unsafe {
            // SAFETY: no security descriptor is supplied and the static UTF-16 name is valid for
            // the synchronous call. The returned handle is uniquely owned by this guard.
            CreateMutexW(None, true, w!("Local\\AltTabio.SingleInstance"))
        }?;
        let already_exists = unsafe {
            // SAFETY: CreateMutexW has just returned successfully, so its last-error value still
            // reports whether the named object existed before this call.
            GetLastError()
        } == ERROR_ALREADY_EXISTS;
        if already_exists {
            close_handle(handle, "duplicate-instance mutex");
            Ok(None)
        } else {
            Ok(Some(Self { handle }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let release_result = unsafe {
            // SAFETY: this guard acquired initial ownership from CreateMutexW and releases it once
            // on the same process before closing the handle.
            ReleaseMutex(self.handle)
        };
        if let Err(error) = release_result {
            eprintln!("Could not release the single-instance mutex: {error}");
        }
        close_handle(self.handle, "single-instance mutex");
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
