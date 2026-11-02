use crate::win32::wide;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::core::{PCWSTR, w};
pub(super) struct WorkFolder {
    pub(super) path: PathBuf,
    pub(super) helper_owns: bool,
}

impl WorkFolder {
    #[cfg(test)]
    pub(super) fn temporary() -> Result<Self, String> {
        let system = crate::win32::system_directory()?;
        let root = system
            .parent()
            .ok_or_else(|| "Windows has no system root".to_owned())?;
        let work = Self::create_in(&root.join("Temp"))?;
        if crate::startup::executable_is_replaceable_by_shell(&work.path)? {
            return Err("Windows did not protect the updater's temporary folder".to_owned());
        }
        Ok(work)
    }

    pub(super) fn create(target: &Path) -> Result<Self, String> {
        let folder = target
            .parent()
            .ok_or_else(|| "The installation has no parent folder".to_owned())?;
        Self::create_in(folder)
    }

    fn create_in(folder: &Path) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = folder.join(format!(".AltTabio-update-{}-{nonce}", std::process::id()));
        let name = wide(
            path.to_str()
                .ok_or_else(|| "The installation path is not valid Unicode".to_owned())?,
        );
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            // SAFETY: the descriptor output is writable and the SDDL is a static terminated string.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(|error| error.to_string())?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or_default(),
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let created = unsafe {
            // SAFETY: name and attributes live through the call; the descriptor is valid until
            // LocalFree below. CreateDirectoryW refuses an existing name, including a reparse point.
            CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&raw const attributes))
        };
        let freed = unsafe {
            // SAFETY: the conversion allocated descriptor with LocalAlloc; this is its only release.
            LocalFree(Some(HLOCAL(descriptor.0)))
        };
        if !freed.0.is_null() {
            eprintln!("Could not free the update folder's security descriptor");
        }
        created.map_err(|error| {
            format!("Could not create a protected update folder beside AltTabio: {error}")
        })?;
        Ok(Self {
            path,
            helper_owns: false,
        })
    }
}

impl Drop for WorkFolder {
    fn drop(&mut self) {
        if !self.helper_owns
            && let Err(error) = std::fs::remove_dir_all(&self.path)
        {
            eprintln!(
                "Could not remove update folder {}: {error}",
                self.path.display()
            );
        }
    }
}
