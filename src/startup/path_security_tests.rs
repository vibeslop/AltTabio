//! Real NTFS permission fixtures for the elevation boundary; no tasks are registered.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SetSecurityInfo,
};
use windows::Win32::Security::{
    CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, GetSecurityDescriptorDacl,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID, SID_AND_ATTRIBUTES, SecurityIdentification,
    UNPROTECTED_DACL_SECURITY_INFORMATION, WinBuiltinAdministratorsSid,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, READ_CONTROL,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::w;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn test_token() -> Result<OwnedHandle, String> {
    let mut primary = HANDLE::default();
    unsafe {
        // SAFETY: GetCurrentProcess is borrowed and primary is writable for the owned token.
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &raw mut primary,
        )
    }
    .map_err(|error| format!("Could not open the test token: {error}"))?;
    let primary = OwnedHandle(primary);
    let primary = if token_is_elevated(primary.0)? {
        restricted_test_token(primary.0)?
    } else {
        primary
    };
    let mut impersonation = HANDLE::default();
    unsafe {
        // SAFETY: primary is live and impersonation receives a uniquely owned token. Using this
        // process's token keeps NTFS tests independent of Explorer or an interactive desktop.
        // AccessCheck only needs identification; no thread impersonation is performed.
        DuplicateToken(primary.0, SecurityIdentification, &raw mut impersonation)
    }
    .map_err(|error| format!("Could not duplicate the test token: {error}"))?;
    Ok(OwnedHandle(impersonation))
}

fn restricted_test_token(primary: HANDLE) -> Result<OwnedHandle, String> {
    // Remove administrative access even when the suite runs elevated. Unlike a linked UAC token,
    // this also works on CI accounts without an interactive shell or a split-token logon.
    let mut administrators = [0_u32; 17];
    let mut sid_bytes =
        u32::try_from(size_of_val(&administrators)).map_err(|error| error.to_string())?;
    let sid = PSID(administrators.as_mut_ptr().cast());
    unsafe {
        // SAFETY: the aligned buffer holds SECURITY_MAX_SID_SIZE bytes and sid_bytes is writable.
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            None,
            Some(sid),
            &raw mut sid_bytes,
        )
    }
    .map_err(|error| format!("Could not create the administrator SID: {error}"))?;
    let disabled = [SID_AND_ATTRIBUTES {
        Sid: sid,
        Attributes: 0,
    }];
    let mut restricted = HANDLE::default();
    unsafe {
        // SAFETY: primary and the SID buffer remain live; Windows copies the disabled SID and
        // returns a uniquely owned token. Disabled SIDs are deny-only, not removed from checks.
        CreateRestrictedToken(
            primary,
            DISABLE_MAX_PRIVILEGE,
            Some(&disabled),
            None,
            None,
            &raw mut restricted,
        )
    }
    .map_err(|error| format!("Could not restrict the test token: {error}"))?;
    Ok(OwnedHandle(restricted))
}

struct Fixture {
    root: PathBuf,
    protected: Vec<RestoreDacl>,
}

impl Fixture {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "AltTabio-path-security-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).map_err(|error| error.to_string())?;
        Ok(Self {
            root,
            protected: Vec::new(),
        })
    }

    fn protect(&mut self, path: &Path) -> Result<(), String> {
        self.protect_with(path, w!("D:P(A;;FRFX;;;WD)(A;;RC;;;OW)"))
    }

    fn protect_with(&mut self, path: &Path, sddl: PCWSTR) -> Result<(), String> {
        self.protected.push(RestoreDacl::protect(path, sddl)?);
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Restore access through handles opened before protecting the objects, even on panic.
        self.protected.clear();
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!("Could not remove path-security fixture: {error}");
        }
    }
}

struct RestoreDacl {
    handle: OwnedHandle,
    // Owns the allocation containing dacl until restoration has finished.
    _original: OwnedSecurityDescriptor,
    dacl: *mut ACL,
}

impl RestoreDacl {
    fn protect(path: &Path, sddl: PCWSTR) -> Result<Self, String> {
        let path = path
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let handle = unsafe {
            // SAFETY: path is terminated. The owned handle retains WRITE_DAC for fixture cleanup.
            CreateFileW(
                PCWSTR(path.as_ptr()),
                READ_CONTROL.0 | WRITE_DAC.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map(OwnedHandle)
        .map_err(|error| error.to_string())?;
        let mut original = PSECURITY_DESCRIPTOR::default();
        let mut dacl = std::ptr::null_mut();
        unsafe {
            // SAFETY: outputs are writable. original owns the allocation that contains dacl.
            GetSecurityInfo(
                handle.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&raw mut dacl),
                None,
                Some(&raw mut original),
            )
        }
        .ok()
        .map_err(|error| error.to_string())?;
        let restore = Self {
            handle,
            _original: OwnedSecurityDescriptor(original),
            dacl,
        };
        let mut restricted = PSECURITY_DESCRIPTOR::default();
        unsafe {
            // SAFETY: the static SDDL is terminated and restricted is writable. OWNER RIGHTS
            // suppresses the owner's implicit WRITE_DAC so this fixture needs no elevation.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl,
                SDDL_REVISION_1,
                &raw mut restricted,
                None,
            )
        }
        .map_err(|error| error.to_string())?;
        let restricted = OwnedSecurityDescriptor(restricted);
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut restricted_dacl = std::ptr::null_mut();
        unsafe {
            // SAFETY: restricted owns its descriptor, and all outputs are writable.
            GetSecurityDescriptorDacl(
                restricted.0,
                &raw mut present,
                &raw mut restricted_dacl,
                &raw mut defaulted,
            )
            .map_err(|error| error.to_string())?;
            // SAFETY: the handle has WRITE_DAC. Windows copies the live ACL during this call.
            SetSecurityInfo(
                restore.handle.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(restricted_dacl),
                None,
            )
            .ok()
            .map_err(|error| error.to_string())?;
        }
        Ok(restore)
    }
}

impl Drop for RestoreDacl {
    fn drop(&mut self) {
        let restored = unsafe {
            // SAFETY: the retained handle has WRITE_DAC and original still owns dacl. Only newly
            // created fixture objects use this guard; their original ACL inherited from parent.
            SetSecurityInfo(
                self.handle.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(self.dacl),
                None,
            )
        }
        .ok();
        if let Err(error) = restored {
            eprintln!("Could not restore path-security fixture permissions: {error}");
        }
    }
}

#[test]
fn protected_executable_beneath_replaceable_ancestor_is_rejected() -> Result<(), String> {
    let mut fixture = Fixture::new()?;
    let installation = fixture.root.join("protected-installation");
    std::fs::create_dir(&installation).map_err(|error| error.to_string())?;
    let executable = installation.join("AltTabio.exe");
    std::fs::write(&executable, b"path-validation fixture").map_err(|error| error.to_string())?;
    fixture.protect(&executable)?;
    fixture.protect(&installation)?;
    let token = test_token()?;
    // Prove that the leaf and immediate parent are protected, while the ancestor is mutable.
    for path in [&executable, &installation] {
        for access in [FILE_WRITE_DATA.0, WRITE_DAC.0, WRITE_OWNER.0] {
            assert!(!token_has_access(path, token.0, access)?);
        }
    }
    assert!(token_has_access(&fixture.root, token.0, WRITE_DAC.0)?);
    assert!(executable_is_replaceable(&executable, token.0)?);
    Ok(())
}

#[test]
fn parent_delete_and_add_subdirectory_rights_allow_directory_replacement() -> Result<(), String> {
    let mut fixture = Fixture::new()?;
    let installation = fixture.root.join("protected-installation");
    std::fs::create_dir(&installation).map_err(|error| error.to_string())?;
    fixture.protect(&installation)?;
    let root = fixture.root.clone();
    // 0x44 is FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD; no ACL control or FILE_ADD_FILE.
    fixture.protect_with(&root, w!("D:P(A;;FRFX;;;WD)(A;;0x44;;;WD)(A;;RC;;;OW)"))?;
    let token = test_token()?;
    assert!(!token_has_access(&installation, token.0, DELETE.0)?);
    assert!(!token_has_access(&root, token.0, FILE_ADD_FILE.0)?);
    assert!(!token_has_access(&root, token.0, WRITE_DAC.0)?);
    assert!(path_component_is_replaceable(&installation, token.0)?);
    Ok(())
}

#[test]
fn adding_sibling_directories_alone_does_not_make_a_component_replaceable() -> Result<(), String> {
    let mut fixture = Fixture::new()?;
    let installation = fixture.root.join("protected-installation");
    std::fs::create_dir(&installation).map_err(|error| error.to_string())?;
    fixture.protect(&installation)?;
    let root = fixture.root.clone();
    fixture.protect_with(&root, w!("D:P(A;;FRFX;;;WD)(A;;0x4;;;WD)(A;;RC;;;OW)"))?;
    let token = test_token()?;
    assert!(token_has_access(&root, token.0, FILE_ADD_SUBDIRECTORY.0)?);
    assert!(!path_component_is_replaceable(&installation, token.0)?);
    Ok(())
}

#[test]
fn append_only_access_to_the_executable_is_rejected() -> Result<(), String> {
    let mut fixture = Fixture::new()?;
    let installation = fixture.root.join("protected-installation");
    std::fs::create_dir(&installation).map_err(|error| error.to_string())?;
    let executable = installation.join("AltTabio.exe");
    std::fs::write(&executable, b"path-validation fixture").map_err(|error| error.to_string())?;
    fixture.protect_with(
        &executable,
        w!("D:P(A;;FRFX;;;WD)(A;;0x4;;;WD)(A;;RC;;;OW)"),
    )?;
    fixture.protect(&installation)?;
    let token = test_token()?;
    assert!(!token_has_access(&executable, token.0, FILE_WRITE_DATA.0)?);
    assert!(token_has_access(&executable, token.0, FILE_APPEND_DATA.0)?);
    assert!(path_component_is_replaceable(&executable, token.0)?);
    Ok(())
}

#[test]
fn junctions_are_rejected_without_following_their_target_permissions() -> Result<(), String> {
    let fixture = Fixture::new()?;
    let target = fixture.root.join("target");
    let junction = fixture.root.join("junction");
    std::fs::create_dir(&target).map_err(|error| error.to_string())?;
    // Junction creation needs no symlink privilege. Pass paths as environment values so shell
    // metacharacters in TEMP are never interpreted as PowerShell source.
    let created = Command::new(system_directory()?.join("WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:ALTTABIO_TEST_JUNCTION -Target $env:ALTTABIO_TEST_TARGET | Out-Null",
        ])
        .env("ALTTABIO_TEST_JUNCTION", &junction)
        .env("ALTTABIO_TEST_TARGET", &target)
        .output()
        .map_err(|error| error.to_string())?;
    if !created.status.success() {
        return Err(format!(
            "Could not create junction fixture: {}",
            output_details(&created)
        ));
    }
    let token = test_token()?;
    let result = path_component_is_replaceable(&junction, token.0);
    assert!(result.is_err_and(|error| error.contains("reparse point")));
    Ok(())
}

#[test]
fn missing_or_relative_installation_paths_fail_closed() -> Result<(), String> {
    let fixture = Fixture::new()?;
    let token = test_token()?;
    assert!(executable_is_replaceable(&fixture.root.join("missing.exe"), token.0).is_err());
    assert!(executable_is_replaceable(Path::new("relative.exe"), token.0).is_err());
    Ok(())
}

#[test]
fn protected_file_and_parent_components_are_accepted() -> Result<(), String> {
    let mut fixture = Fixture::new()?;
    let installation = fixture.root.join("protected-installation");
    std::fs::create_dir(&installation).map_err(|error| error.to_string())?;
    let executable = installation.join("AltTabio.exe");
    std::fs::write(&executable, b"path-validation fixture").map_err(|error| error.to_string())?;
    fixture.protect(&executable)?;
    fixture.protect(&installation)?;
    let root = fixture.root.clone();
    fixture.protect(&root)?;
    let token = test_token()?;
    // Test ordinary files we own instead of system binaries, which can be WOF reparse files.
    // The temporary directory's higher ancestors remain outside this controlled fixture.
    assert!(!path_component_is_replaceable(&executable, token.0)?);
    assert!(!path_component_is_replaceable(&installation, token.0)?);
    Ok(())
}
