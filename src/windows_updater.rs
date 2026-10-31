//! Blocking Windows update I/O. Call these functions only on the update worker.
//!
//! Windows supplies PowerShell, JSON/ZIP support, SHA-256, and curl. Keeping the scripts
//! embedded avoids loading elevated code from a writable temporary script or from PATH.

use crate::win32::wide;
use alttabio::update::{Asset, Checked, Release, Update, Version, newest_windows_update};
use std::io::{BufRead, BufReader};
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetTickCount};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::core::{PCWSTR, w};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DOWNLOAD_SCRIPT: &str = include_str!("windows_updater/download.ps1");
const RELAUNCH_SCRIPT: &str = include_str!("windows_updater/relaunch.ps1");

pub fn is_release_build() -> bool {
    option_env!("ALTTABIO_RELEASE") == Some("1") && !cfg!(debug_assertions)
}

pub fn user_is_away() -> bool {
    let mut input = LASTINPUTINFO {
        cbSize: u32::try_from(size_of::<LASTINPUTINFO>()).unwrap_or_default(),
        ..LASTINPUTINFO::default()
    };
    let read = unsafe {
        // SAFETY: input is writable and its size is initialized for the synchronous call.
        GetLastInputInfo(&raw mut input)
    };
    if !read.as_bool() {
        eprintln!(
            "Could not read the last input time: {}",
            windows::core::Error::from_thread()
        );
        return false;
    }
    let now = unsafe {
        // SAFETY: GetTickCount takes no pointers and shares LASTINPUTINFO's wrapping time base.
        GetTickCount()
    };
    now.wrapping_sub(input.dwTime) >= 5 * 60 * 1000
}

pub fn check() -> Result<Checked, String> {
    if !is_release_build() {
        return Err("This development build does not replace itself with published releases. Download a release from GitHub to enable updates.".to_owned());
    }
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| "This copy has no release version".to_owned())?;
    let list = run_script(&format!("$Mode = 'check'\n{DOWNLOAD_SCRIPT}"))?;
    let update = newest_windows_update(current, &read_releases(&list)?);
    Ok(checked_update(update, || {
        let target = std::env::current_exe().map_err(|error| error.to_string())?;
        validate_update_target(&target)
    }))
}

fn checked_update(
    update: Option<Update>,
    validate: impl FnOnce() -> Result<(), String>,
) -> Checked {
    let Some(update) = update else {
        return Checked::UpToDate;
    };
    match validate() {
        Ok(()) => Checked::Available(update),
        Err(reason) => Checked::Blocked(update.version, reason),
    }
}

pub struct StagedUpdate {
    work: WorkFolder,
    hash: String,
    target: PathBuf,
}

pub fn stage(update: &Update) -> Result<StagedUpdate, String> {
    if !is_release_build() {
        return Err("Development builds cannot install updates".to_owned());
    }
    let target = std::env::current_exe().map_err(|error| error.to_string())?;
    validate_update_target(&target)?;
    let work = WorkFolder::create(&target)?;
    let hash = run_script(&format!(
        "$Mode = 'stage'\n$Version = '{}'\n$Work = {}\n{DOWNLOAD_SCRIPT}",
        update.version,
        literal(&work.path)?
    ))?;
    let hash = hash.trim().to_owned();
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The update did not return an executable checksum".to_owned());
    }
    Ok(StagedUpdate { work, hash, target })
}

/// Returns only once the helper has acquired this process and is waiting for it to exit.
pub fn relaunch(staged: &mut StagedUpdate) -> Result<(), String> {
    validate_update_target(&staged.target)?;
    let script = format!(
        "$Work = {}\n$Target = {}\n$Hash = '{}'\n$ParentId = {}\n{RELAUNCH_SCRIPT}",
        literal(&staged.work.path)?,
        literal(&staged.target)?,
        staged.hash,
        std::process::id()
    );
    let mut child = powershell(&script)?
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start the update helper: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "The update helper has no output pipe".to_owned())?;
    let mut ready = String::new();
    let reply = BufReader::new(stdout).read_line(&mut ready);
    if reply.is_err() || ready.trim() != "READY" {
        // An unexpected reply must not leave a helper waiting on a process that will stay up.
        // Wait only after a successful termination, since a failed kill may leave it blocked.
        match child.kill() {
            Ok(()) => {
                if let Err(error) = child.wait() {
                    eprintln!("Could not reap the failed update helper: {error}");
                }
            }
            Err(error) => eprintln!("Could not stop the failed update helper: {error}"),
        }
        return Err(reply.map_or_else(
            |error| format!("Could not read the update helper's reply: {error}"),
            |_| "The update helper did not acknowledge readiness".to_owned(),
        ));
    }
    // The helper owns cleanup now. Its code and paths came from this executable, and the
    // directory's protected ACL prevents another account from changing the staged payload.
    staged.work.helper_owns = true;
    Ok(())
}

fn validate_update_target(target: &Path) -> Result<(), String> {
    validate_update_target_with(target, crate::startup::executable_is_replaceable_by_shell)
}

fn validate_update_target_with(
    target: &Path,
    is_replaceable: impl FnOnce(&Path) -> Result<bool, String>,
) -> Result<(), String> {
    if is_replaceable(target)? {
        Err(format!(
            "Updates are disabled because non-elevated processes can replace {}. Move AltTabio to an administrator-writable-only folder first.",
            target.display()
        ))
    } else {
        Ok(())
    }
}

fn read_releases(text: &str) -> Result<Vec<Release>, String> {
    text.lines()
        .map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            let [tag, draft, prerelease, name, url] = fields[..] else {
                return Err("GitHub returned unreadable release metadata".to_owned());
            };
            let boolean = |value: &str| match value {
                "True" => Ok(true),
                "False" => Ok(false),
                _ => Err("GitHub returned unreadable release flags".to_owned()),
            };
            Ok(Release {
                tag: tag.to_owned(),
                draft: boolean(draft)?,
                prerelease: boolean(prerelease)?,
                assets: vec![Asset {
                    name: name.to_owned(),
                    url: url.to_owned(),
                }],
            })
        })
        .collect()
}

fn run_script(script: &str) -> Result<String, String> {
    let output = powershell(script)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Could not run Windows PowerShell: {error}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Windows update failed ({}): {}",
            output.status,
            error.trim()
        ))
    }
}

fn powershell(script: &str) -> Result<Command, String> {
    let system = system_directory()?;
    let mut command = Command::new(system.join("WindowsPowerShell/v1.0/powershell.exe"));
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-OutputFormat",
            "Text",
            "-Command",
        ])
        .arg(format!(
            "& {{\n$Curl = {}\n{script}\n}}",
            literal(&system.join("curl.exe"))?
        ))
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

fn system_directory() -> Result<PathBuf, String> {
    let mut buffer = vec![0_u16; 32768];
    let length = unsafe {
        // SAFETY: buffer is writable and Windows receives its actual length through the slice.
        GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("Could not locate the Windows system directory".to_owned());
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length],
    )))
}

fn literal(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "The installation path is not valid Unicode".to_owned())?;
    Ok(format!("'{}'", text.replace('\'', "''")))
}

struct WorkFolder {
    path: PathBuf,
    helper_owns: bool,
}

impl WorkFolder {
    fn create(target: &Path) -> Result<Self, String> {
        let folder = target
            .parent()
            .ok_or_else(|| "The installation has no parent folder".to_owned())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_checks_report_blocked_installations_before_offering_a_download() {
        let update = Update {
            version: Version::parse("9.0.0").unwrap_or_else(|| panic!("valid version")),
            url: "https://example.com/release.zip".to_owned(),
        };
        assert_eq!(
            checked_update(Some(update.clone()), || Err(
                "Unsafe installation".to_owned()
            )),
            Checked::Blocked(update.version, "Unsafe installation".to_owned())
        );
        assert_eq!(
            checked_update(Some(update.clone()), || Ok(())),
            Checked::Available(update)
        );
        assert_eq!(
            checked_update(None, || panic!("no update needs no validation")),
            Checked::UpToDate
        );
    }

    #[test]
    fn updates_require_a_target_protected_from_non_elevated_replacement() {
        let target = Path::new(r"C:\Users\Example\Downloads\AltTabio.exe");
        assert!(validate_update_target_with(target, |_| Ok(true)).is_err());
        assert_eq!(validate_update_target_with(target, |_| Ok(false)), Ok(()));
        assert_eq!(
            validate_update_target_with(target, |_| Err("Access check failed".to_owned())),
            Err("Access check failed".to_owned())
        );
    }

    #[test]
    fn release_metadata_is_read_without_treating_text_as_code() {
        let releases = read_releases("v1.10.0\tFalse\tFalse\tAltTabio-1.10.0-windows-x64.zip\thttps://example.com/update.zip\r\n")
            .unwrap_or_else(|error| panic!("{error}"));
        let current = Version::parse("1.9.0").unwrap_or_else(|| panic!("valid version"));
        assert_eq!(
            newest_windows_update(current, &releases).map(|update| update.version.to_string()),
            Some("1.10.0".to_owned())
        );
        assert!(read_releases("not metadata").is_err());
        assert!(read_releases("v1.2.3\tunknown\tFalse\tx\ty").is_err());
        assert_eq!(read_releases(""), Ok(Vec::new()));
    }

    #[test]
    fn powershell_paths_are_single_quoted_literals() {
        assert_eq!(
            literal(Path::new("C:\\A user's files\\$test;`x\\AltTabio.exe")),
            Ok("'C:\\A user''s files\\$test;`x\\AltTabio.exe'".to_owned())
        );
    }

    #[test]
    fn the_windows_process_adapter_preserves_literal_script_arguments() {
        let path = Path::new("C:\\A user's [files]\\$test;`x\\AltTabio.exe");
        let quoted = literal(path).unwrap_or_else(|error| panic!("{error}"));
        let result = run_script(&format!("[Console]::Write({quoted})"));
        assert_eq!(result, Ok(path.to_string_lossy().into_owned()));
    }

    #[test]
    fn powershell_release_metadata_reaches_the_shared_selector() {
        let json = r#"[{"tag_name":"v1.10.0","draft":false,"prerelease":false,"assets":[
            {"name":"AltTabio-1.10.0-windows-x64.zip","browser_download_url":"https://github.com/vibeslop/AltTabio/releases/download/v1.10.0/AltTabio-1.10.0-windows-x64.zip"},
            {"name":"AltTabio-1.10.0-windows-x64.zip","browser_download_url":"https://example.com/untrusted.zip"}
        ]}]"#;
        let script = |json: &str| {
            format!(
                "function Mock-Curl {{ $global:LASTEXITCODE = 0; '{json}' }}\n$Curl = 'Mock-Curl'\n$Mode = 'check'\n{DOWNLOAD_SCRIPT}"
            )
        };
        let text = run_script(&script(json)).unwrap_or_else(|error| panic!("{error}"));
        let releases = read_releases(&text).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "v1.10.0");
        assert!(run_script(&script(r#"{"message":"not a release list"}"#)).is_err());
    }
}
