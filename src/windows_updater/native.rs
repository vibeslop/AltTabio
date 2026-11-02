//! Small owning adapters for hashing, process handles, permissions, and a clean GUI environment.
use std::fmt::Write;
use std::io::Read;
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash, BCryptDestroyHash,
    BCryptFinishHash, BCryptHashData,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_DUPLICATE, TOKEN_QUERY,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::{REPLACE_FILE_FLAGS, ReplaceFileW};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetCurrentProcess,
    OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::core::PCWSTR;

pub(super) struct Handle(pub(super) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this guard owns one successful handle, never a pseudo or borrowed handle.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            eprintln!("Could not close update handle: {error}");
        }
    }
}

struct Hash(BCRYPT_HASH_HANDLE);
impl Drop for Hash {
    fn drop(&mut self) {
        // SAFETY: owns the hash created below, destroyed once after its final use.
        if let Err(error) = unsafe { BCryptDestroyHash(self.0) }.ok() {
            eprintln!("Could not release update hash: {error}");
        }
    }
}

pub(super) fn sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut handle = BCRYPT_HASH_HANDLE::default();
    // SAFETY: the SHA-256 pseudo provider is OS-owned; BCrypt allocates the hash object and
    // the writable output is guarded immediately after a successful creation.
    unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &raw mut handle, None, None, 0) }
        .ok()
        .map_err(|error| error.to_string())?;
    let hash = Hash(handle);
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        // SAFETY: hash is live and the initialized input slice is borrowed for this call.
        unsafe { BCryptHashData(hash.0, &buffer[..read], 0) }
            .ok()
            .map_err(|error| error.to_string())?;
    }
    let mut digest = [0_u8; 32];
    // SAFETY: the output has precisely the SHA-256 digest length and hash is still live.
    unsafe { BCryptFinishHash(hash.0, &mut digest, 0) }
        .ok()
        .map_err(|error| error.to_string())?;
    let mut text = String::with_capacity(64);
    for byte in digest {
        write!(&mut text, "{byte:02x}").map_err(|error| error.to_string())?;
    }
    Ok(text)
}

pub(super) fn path_wide(path: &Path) -> Result<Vec<u16>, String> {
    use std::os::windows::ffi::OsStrExt;
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err("An update path contains a null character".to_owned());
    }
    Ok(units.into_iter().chain(Some(0)).collect())
}

pub(super) fn replace(target: &Path, source: &Path, backup: &Path) -> Result<(), String> {
    let target = path_wide(target)?;
    let source = path_wide(source)?;
    let backup = path_wide(backup)?;
    // SAFETY: all three paths are terminated, distinct, and live through the synchronous call.
    unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(source.as_ptr()),
            PCWSTR(backup.as_ptr()),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    }
    .map_err(|error| error.to_string())
}

pub(super) struct Permissions {
    descriptor: PSECURITY_DESCRIPTOR,
    dacl: *mut ACL,
    protected: bool,
}
impl Permissions {
    #[cfg(test)]
    pub(super) fn sddl(&self) -> String {
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
        };
        let mut text = windows::core::PWSTR::null();
        // SAFETY: descriptor is live; the returned SDDL is allocated by LocalAlloc.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                self.descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &raw mut text,
                None,
            )
        }
        .unwrap_or_else(|error| panic!("{error}"));
        // SAFETY: successful conversion returns terminated UTF-16 text owned until LocalFree.
        let result = unsafe { text.to_string() }.unwrap_or_else(|error| panic!("{error}"));
        // SAFETY: this is the only release of the conversion's allocation.
        assert!(
            unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) }
                .0
                .is_null()
        );
        result
    }
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        let path = path_wide(path)?;
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let mut dacl = std::ptr::null_mut();
        // SAFETY: writable outputs receive an allocated descriptor and an ACL borrowed from it.
        unsafe {
            GetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&raw mut dacl),
                None,
                &raw mut descriptor,
            )
        }
        .ok()
        .map_err(|error| error.to_string())?;
        let mut permissions = Self {
            descriptor,
            dacl,
            protected: false,
        };
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: descriptor remains owned by permissions and both outputs are writable.
        unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
            .map_err(|error| error.to_string())?;
        permissions.protected = control & 0x1000 != 0;
        Ok(permissions)
    }

    pub(super) fn restore(&self, path: &Path) -> Result<(), String> {
        let path = path_wide(path)?;
        let inheritance = if self.protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SAFETY: dacl is borrowed from the live descriptor; only DACL and its inheritance policy
        // are written, so no owner or audit privilege is requested.
        unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | inheritance,
                None,
                None,
                Some(self.dacl),
                None,
            )
        }
        .ok()
        .map_err(|error| error.to_string())
    }
}
impl Drop for Permissions {
    fn drop(&mut self) {
        // SAFETY: GetNamedSecurityInfoW allocated this descriptor with LocalAlloc.
        if !unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) }
            .0
            .is_null()
        {
            eprintln!("Could not release update permissions");
        }
    }
}

struct Environment(*mut std::ffi::c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the block returned by CreateEnvironmentBlock.
        if let Err(error) = unsafe { DestroyEnvironmentBlock(self.0) } {
            eprintln!("Could not release the update environment: {error}");
        }
    }
}

pub(super) fn launch(target: &Path) -> Result<(), String> {
    let path = path_wide(target)?;
    let directory = path_wide(target.parent().ok_or("Missing application folder")?)?;
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a borrowed pseudo handle; only the returned token is owned.
    // CreateEnvironmentBlock requires query and duplicate access to a primary token.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &raw mut token,
        )
    }
    .map_err(|error| error.to_string())?;
    let token = Handle(token);
    let mut environment = std::ptr::null_mut();
    // SAFETY: token is live; the new block excludes the helper's deliberately minimal environment.
    unsafe { CreateEnvironmentBlock(&raw mut environment, Some(token.0), false) }
        .map_err(|error| error.to_string())?;
    let environment = Environment(environment);
    let startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).map_err(|error| error.to_string())?,
        ..STARTUPINFOW::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: exact executable path, no command line or inherited handles, a live Unicode
    // environment block, and writable outputs. No ShowWindow override reaches the GUI child.
    unsafe {
        CreateProcessW(
            PCWSTR(path.as_ptr()),
            None,
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            Some(environment.0),
            PCWSTR(directory.as_ptr()),
            &raw const startup,
            &raw mut process,
        )
    }
    .map_err(|error| error.to_string())?;
    let _process = Handle(process.hProcess);
    let _thread = Handle(process.hThread);
    Ok(())
}

pub(super) fn helper_command(exe: &Path, work: &Path) -> Result<std::process::Command, String> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let system = crate::win32::system_directory()?;
    let root = system.parent().ok_or("Missing Windows directory")?;
    let Some(std::path::Component::Prefix(drive)) = root.components().next() else {
        return Err("Missing Windows system drive".to_owned());
    };
    let mut command = std::process::Command::new(exe);
    command
        .env_clear()
        .env("SystemRoot", root)
        .env("windir", root)
        .env("SystemDrive", drive.as_os_str())
        .env("TEMP", work)
        .env("TMP", work)
        .current_dir(system)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_sha256_matches_known_digest() {
        let work = super::super::WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let path = work.path.join("hash.txt");
        std::fs::write(&path, b"abc").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            sha256(&path),
            Ok("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned())
        );
    }
}
