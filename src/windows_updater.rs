//! Blocking Windows update I/O runs on the update worker. Only the final approval marker is
//! written by the outer app loop immediately before it exits for an update.
//!
//! Windows supplies PowerShell, JSON/ZIP support, SHA-256, and curl. Keeping the scripts
//! embedded avoids loading elevated code from a writable temporary script or from PATH.

mod cleanup;

use crate::win32::{system_directory, wide};
use alttabio::update::{Asset, Checked, Release, Update, Version, newest_windows_update};
use std::io::{BufRead, BufReader};
use std::mem::size_of;
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
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::core::{PCWSTR, w};

const DOWNLOAD_SCRIPT: &str = include_str!("windows_updater/download.ps1");
const RELAUNCH_SCRIPT: &str = include_str!("windows_updater/relaunch.ps1");

pub fn is_release_build() -> bool {
    option_env!("ALTTABIO_RELEASE") == Some("1") && !cfg!(debug_assertions)
}

pub fn start_maintenance() -> Result<(), String> {
    if !is_release_build() {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("alttabio-update-cleanup".to_owned())
        .spawn(|| {
            let result = (|| {
                let target = std::env::current_exe().map_err(|error| error.to_string())?;
                validate_update_target(&target)?;
                cleanup::maintain(
                    target
                        .parent()
                        .ok_or_else(|| "The installation has no parent folder".to_owned())?,
                )
            })();
            if let Err(error) = result {
                eprintln!("{error}");
            }
        })
        .map_err(|error| format!("Could not start update maintenance: {error}"))?;
    Ok(())
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

pub fn check(manual: bool) -> Result<Checked, String> {
    if !is_release_build() {
        return Err("This development build does not replace itself with published releases. Download a release from GitHub to enable updates.".to_owned());
    }
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| "This copy has no release version".to_owned())?;
    let target = std::env::current_exe().map_err(|error| error.to_string())?;
    let target_status = validate_update_target(&target);
    if target_status.is_ok()
        && cleanup::maintain(
            target
                .parent()
                .ok_or_else(|| "The installation has no parent folder".to_owned())?,
        )?
        && !manual
    {
        return Err("The previous update failed. Automatic installation is deferred for 24 hours; use Check for Updates to retry manually.".to_owned());
    }
    let temporary = WorkFolder::temporary()?;
    let list = run_script_in(
        &format!("$Mode = 'check'\n{DOWNLOAD_SCRIPT}"),
        &temporary.path,
    )?;
    let update = newest_windows_update(current, &read_releases(&list)?);
    Ok(checked_update(update, || target_status))
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
    let hash = run_script_in(
        &format!(
            "$Mode = 'stage'\n$Version = '{}'\n$Work = {}\n{DOWNLOAD_SCRIPT}",
            update.version,
            literal(&work.path)?
        ),
        &work.path,
    )?;
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
    let mut child = powershell(&script, &staged.work.path)?
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

/// Commits an update exit once the app can quit immediately, outside native callbacks.
pub fn approve_relaunch(staged: &StagedUpdate) -> Result<(), String> {
    std::fs::write(staged.work.path.join("apply-approved"), &staged.hash)
        .map_err(|error| format!("Could not authorize the update restart: {error}"))
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

fn run_script_in(script: &str, temporary: &Path) -> Result<String, String> {
    let output = powershell(script, temporary)?
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

fn powershell(script: &str, temporary: &Path) -> Result<Command, String> {
    let system = system_directory()?;
    let root = system
        .parent()
        .ok_or_else(|| "Windows has no system root".to_owned())?;
    let modules = system.join("WindowsPowerShell/v1.0/Modules");
    let mut imports = String::new();
    for module in ["Management", "Utility", "Security"] {
        let name = format!("Microsoft.PowerShell.{module}");
        imports.push_str("Import-Module ");
        imports.push_str(&literal(&modules.join(&name).join(format!("{name}.psd1")))?);
        imports.push_str(" -ErrorAction Stop\n");
    }
    let mut command = Command::new(system.join("WindowsPowerShell/v1.0/powershell.exe"));
    command
        .env_clear()
        .env("SystemRoot", root)
        .env("windir", root)
        .env("TEMP", temporary)
        .env("TMP", temporary)
        .env("PSModulePath", &modules)
        .current_dir(&system)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-OutputFormat",
            "Text",
            "-Command",
        ])
        .arg(format!(
            "& {{\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n$ErrorActionPreference = 'Stop'\n$PSModuleAutoLoadingPreference = 'None'\n$env:PSModulePath = {}\n{imports}$Curl = {}\n{script}\n}}",
            literal(&modules)?, literal(&system.join("curl.exe"))?
        ))
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
}

fn literal(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "The installation path is not valid Unicode".to_owned())?;
    // PowerShell tokenizes smart quotes as delimiters too. Numeric UTF-16 code units keep every
    // path character out of its lexer while preserving non-BMP characters exactly.
    let units = text
        .encode_utf16()
        .map(|unit| unit.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("([string]::new([char[]]@({units})))"))
}

struct WorkFolder {
    path: PathBuf,
    helper_owns: bool,
}

impl WorkFolder {
    fn temporary() -> Result<Self, String> {
        let system = system_directory()?;
        let root = system
            .parent()
            .ok_or_else(|| "Windows has no system root".to_owned())?;
        let work = Self::create_in(&root.join("Temp"))?;
        if crate::startup::executable_is_replaceable_by_shell(&work.path)? {
            return Err("Windows did not protect the updater's temporary folder".to_owned());
        }
        Ok(work)
    }

    fn create(target: &Path) -> Result<Self, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run_script(script: &str) -> Result<String, String> {
        // Test scripts use inert fixtures and never download or execute an update.
        run_script_in(
            script,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("target"),
        )
    }

    #[test]
    fn updater_powershell_does_not_inherit_parent_configuration() {
        const FIXTURE: &str = "ALTTABIO_UPDATER_ENV_FIXTURE";
        if std::env::var(FIXTURE).as_deref() == Ok("1") {
            assert_eq!(
                run_script(
                    "[Console]::Write([string]::IsNullOrEmpty($env:ALTTABIO_UNTRUSTED_ENV))"
                ),
                Ok("True".to_owned())
            );
            assert_eq!(
                run_script(
                    "$expected = $PSHOME + [IO.Path]::DirectorySeparatorChar; [Console]::Write((Get-Command Get-FileHash).Module.Path.StartsWith($expected, [StringComparison]::OrdinalIgnoreCase))"
                ),
                Ok("True".to_owned())
            );
            return;
        }
        let output = Command::new(
            std::env::current_exe().unwrap_or_else(|error| panic!("{error}")),
        )
        .args([
            "--exact",
            "windows_updater::tests::updater_powershell_does_not_inherit_parent_configuration",
            "--nocapture",
        ])
        .env(FIXTURE, "1")
        .env("ALTTABIO_UNTRUSTED_ENV", "parent-controlled")
        .env("CURL_HOME", "parent-controlled")
        .env("PSModulePath", "parent-controlled")
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "requires an elevated Windows token to create an administrator-only directory"]
    fn runtime_temporary_folder_is_protected_from_the_shell() {
        let temporary = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            crate::startup::executable_is_replaceable_by_shell(&temporary.path),
            Ok(false)
        );
        assert_eq!(
            run_script_in("[Console]::Write($env:TEMP)", &temporary.path),
            Ok(temporary.path.to_string_lossy().into_owned())
        );
    }

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
    fn helper_output_stays_utf8_under_a_legacy_console_code_page() {
        let temporary = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        let command = powershell(
            "[Console]::Write([string]::new([char[]]@(8217,945,55357,56832)))",
            &temporary,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let source = command
            .get_args()
            .last()
            .and_then(|argument| argument.to_str())
            .unwrap_or_else(|| panic!("the helper command must contain its script"));
        let script =
            format!("[Console]::OutputEncoding = [Text.Encoding]::GetEncoding(437);\n{source}");
        assert_eq!(run_script(&script), Ok("’α😀".to_owned()));
    }

    #[test]
    fn the_windows_process_adapter_preserves_literal_script_arguments() {
        for text in [
            "C:\\A user's [files]\\$test;`x\\AltTabio.exe",
            "C:\\Bob’s Tools\\AltTabio.exe",
            "C:\\quotes‘’‚‛ and 😀\\AltTabio.exe",
        ] {
            let path = Path::new(text);
            let quoted = literal(path).unwrap_or_else(|error| panic!("{error}"));
            let result = run_script(&format!("[Console]::Write({quoted})"));
            assert_eq!(result, Ok(path.to_string_lossy().into_owned()));
        }
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
