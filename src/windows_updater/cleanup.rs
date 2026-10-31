//! Retry receipts and abandoned staging folders, inspected only by the update worker.

use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

const RETAIN: Duration = Duration::from_hours(24);
static MAINTENANCE: Mutex<()> = Mutex::new(());

/// The caller validates the installation before allowing maintenance in its parent folder.
pub(super) fn maintain(folder: &Path) -> Result<bool, String> {
    let _maintenance = MAINTENANCE.lock().map_err(|error| error.to_string())?;
    scan(folder, SystemTime::now(), process_is_running)
        .map_err(|error| format!("Could not clean abandoned update folders: {error}"))
}

fn scan(
    folder: &Path,
    now: SystemTime,
    alive: impl Fn(u32) -> Result<bool, String>,
) -> Result<bool, String> {
    let mut recent_failure = false;
    for entry in fs::read_dir(folder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(owner) = name.to_str().and_then(owner_pid) else {
            continue;
        };
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            continue;
        }
        match fs::symlink_metadata(path.join("error.txt")) {
            Ok(failure)
                if failure.is_file()
                    && failure.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 =>
            {
                let age = now
                    .duration_since(failure.modified().map_err(|error| error.to_string())?)
                    .unwrap_or_default();
                if age < RETAIN {
                    recent_failure = true;
                    continue;
                }
            }
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let age = now
            .duration_since(metadata.modified().map_err(|error| error.to_string())?)
            .unwrap_or_default();
        if age < RETAIN || alive(owner)? {
            continue;
        }
        match fs::read_to_string(path.join("helper.pid")) {
            Ok(text) => {
                let Ok(pid) = text.trim().parse::<u32>() else {
                    continue;
                };
                if pid == 0 || alive(pid)? {
                    continue;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        // Only exact child names in the protected installation are eligible. Reparse points,
        // recent work, live app owners, and live helpers were excluded before recursive cleanup.
        fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
    }
    Ok(recent_failure)
}

fn owner_pid(name: &str) -> Option<u32> {
    let (pid, nonce) = name.strip_prefix(".AltTabio-update-")?.split_once('-')?;
    if pid.is_empty()
        || !pid.bytes().all(|value| value.is_ascii_digit())
        || nonce.is_empty()
        || !nonce.bytes().all(|value| value.is_ascii_digit())
    {
        return None;
    }
    pid.parse().ok().filter(|pid| *pid != 0)
}

fn process_is_running(pid: u32) -> Result<bool, String> {
    // SAFETY: this requests only query access to a numeric process ID and owns a successful handle.
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => return Ok(false),
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => return Ok(true),
        Err(error) => return Err(error.to_string()),
    };
    let mut code = 0;
    // SAFETY: handle is live and the scalar output remains writable for this synchronous read.
    let queried = unsafe { GetExitCodeProcess(handle, &raw mut code) };
    // SAFETY: this is the only release of the handle, on both query success and failure.
    unsafe { CloseHandle(handle) }.map_err(|error| error.to_string())?;
    queried.map_err(|error| error.to_string())?;
    Ok(code == STILL_ACTIVE.0.cast_unsigned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn maintenance_keeps_active_work_and_failure_receipts_but_removes_old_orphans() {
        let now = SystemTime::now();
        let nonce = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("AltTabio-cleanup-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap_or_else(|error| panic!("{error}"));
        let make = |name: &str| {
            let path = root.join(name);
            fs::create_dir(&path).unwrap_or_else(|error| panic!("{error}"));
            fs::write(path.join("download.zip"), b"fixture")
                .unwrap_or_else(|error| panic!("{error}"));
            path
        };
        let abandoned = make(".AltTabio-update-100-1");
        let active = make(".AltTabio-update-101-2");
        let helper = make(".AltTabio-update-102-3");
        fs::write(helper.join("helper.pid"), b"103").unwrap_or_else(|error| panic!("{error}"));
        let unrelated = make(".AltTabio-update-not-a-pid");
        assert_eq!(scan(&root, now, |_| Ok(false)), Ok(false));
        assert!(
            abandoned.exists(),
            "recent work must survive even if its owner has just exited"
        );
        let later = now + Duration::from_hours(48);
        let failed = make(".AltTabio-update-104-4");
        let receipt =
            fs::File::create(failed.join("error.txt")).unwrap_or_else(|error| panic!("{error}"));
        receipt
            .set_times(fs::FileTimes::new().set_modified(later))
            .unwrap_or_else(|error| panic!("{error}"));
        drop(receipt);
        assert_eq!(
            scan(&root, later, |pid| Ok(matches!(pid, 101 | 103))),
            Ok(true)
        );
        assert!(!abandoned.exists());
        assert!(active.exists() && helper.exists() && unrelated.exists() && failed.exists());
        assert_eq!(scan(&root, later + RETAIN, |_| Ok(false)), Ok(false));
        assert!(
            !failed.exists(),
            "expired failure receipts and their payloads must be removed"
        );
        assert!(unrelated.exists());
        fs::remove_dir_all(root).unwrap_or_else(|error| panic!("{error}"));
    }
}
