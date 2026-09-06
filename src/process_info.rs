use alttabio::switcher::ProcessIdentity;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::{PWSTR, Result};

/// A process image path and the identity of the process that supplied it.
pub struct ProcessInfo {
    pub executable: PathBuf,
    pub identity: ProcessIdentity,
}

impl ProcessInfo {
    pub fn query(process_id: u32) -> Result<Self> {
        // SAFETY: the process id is a scalar, and this guard owns the returned query-only handle.
        let process = OwnedProcess(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
        }?);
        Ok(Self {
            executable: executable_path(process.0)?,
            identity: ProcessIdentity::new(process_id, process_started_at(process.0)?),
        })
    }

    /// Unqueryable processes stay visible, but cannot be targeted by identity-sensitive commands.
    pub fn unavailable(process_id: u32) -> Self {
        Self {
            executable: PathBuf::new(),
            identity: ProcessIdentity::new(process_id, 0),
        }
    }

    pub fn executable_stem(&self) -> &str {
        self.executable
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }
}

pub fn executable_path(process: HANDLE) -> Result<PathBuf> {
    let mut buffer = vec![0_u16; 32_768];
    let mut length = 32_768;
    // SAFETY: the caller borrows a live queryable process; both outputs are writable for the call.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )?;
    }
    let length = usize::try_from(length)
        .unwrap_or_default()
        .min(buffer.len());
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

pub fn process_started_at(process: HANDLE) -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: process is borrowed and queryable; all four outputs remain writable for this call.
    unsafe {
        GetProcessTimes(
            process,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )?;
    }
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

struct OwnedProcess(HANDLE);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // SAFETY: the guard owns this handle, which has not been closed or transferred.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            eprintln!("Could not close a process query handle: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::passthrough::is_remote_desktop_client;

    #[test]
    fn full_paths_preserve_the_remote_desktop_executable_fallback() {
        for name in ["mstsc", "msrdc", "msrdcw"] {
            let info = ProcessInfo {
                executable: PathBuf::from(format!(r"C:\Program Files\Remote Desktop\{name}.exe")),
                identity: ProcessIdentity::default(),
            };
            assert!(is_remote_desktop_client(
                "UnlistedWindowClass",
                info.executable_stem()
            ));
        }
        assert!(!is_remote_desktop_client(
            "UnlistedWindowClass",
            ProcessInfo::unavailable(42).executable_stem()
        ));
    }

    #[test]
    fn current_process_query_returns_its_path_and_stable_identity() -> Result<()> {
        let first = ProcessInfo::query(std::process::id())?;
        let second = ProcessInfo::query(std::process::id())?;
        assert_eq!(first.identity, second.identity);
        assert_ne!(first.identity.started_at, 0);
        assert!(first.executable.is_absolute());
        assert_eq!(first.executable, second.executable);
        assert!(!first.executable_stem().is_empty());
        Ok(())
    }
}
