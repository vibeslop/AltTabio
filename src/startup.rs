use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACL, AccessCheck, DACL_SECURITY_INFORMATION, DuplicateToken, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION, PRIVILEGE_SET,
    PSECURITY_DESCRIPTOR, SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_FILE, FILE_ALL_ACCESS, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_WRITE_DATA, WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{BOOL, PCWSTR};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub task_exists: bool,
}

pub fn status() -> Result<AutostartStatus, String> {
    let task_query = run(&["/Query", "/TN", "AltTabio", "/XML"])?;
    if task_query.status.success() {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        let xml = String::from_utf8(task_query.stdout)
            .map_err(|error| format!("Autostart task XML is not valid UTF-8: {error}"))?;
        return Ok(AutostartStatus {
            enabled: task_targets_executable(&xml, &executable)?,
            task_exists: true,
        });
    }

    // schtasks uses the same nonzero status for a missing task and operational failures. A general
    // query distinguishes an absent named task from an unavailable or inaccessible scheduler
    // without parsing localized error text.
    let scheduler_query = run(&["/Query", "/FO", "CSV", "/NH"])?;
    if scheduler_query.status.success() {
        Ok(AutostartStatus {
            enabled: false,
            task_exists: false,
        })
    } else {
        Err(format!(
            "Autostart status query failed: {}",
            output_details(&task_query)
        ))
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let output = if enabled {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        validate_autostart_target(&executable)?;
        run_owned(create_arguments(&executable))?
    } else {
        if !status()?.task_exists {
            return Ok(());
        }
        run(&["/Delete", "/TN", "AltTabio", "/F"])?
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Autostart task update failed: {}",
            output_details(&output)
        ))
    }
}

fn validate_autostart_target(executable: &Path) -> Result<(), String> {
    let token = shell_impersonation_token()?;
    validate_autostart_target_with(executable, |path| executable_is_replaceable(path, token.0))
}

fn validate_autostart_target_with(
    executable: &Path,
    mut is_replaceable: impl FnMut(&Path) -> Result<bool, String>,
) -> Result<(), String> {
    if is_replaceable(executable)? {
        Err(format!(
            "Autostart was not enabled because non-elevated processes can replace {}. Move AltTabio to an administrator-writable-only folder first.",
            executable.display()
        ))
    } else {
        Ok(())
    }
}

fn shell_impersonation_token() -> Result<OwnedHandle, String> {
    let shell_window = unsafe {
        // SAFETY: GetShellWindow has no preconditions and returns a borrowed handle.
        GetShellWindow()
    };
    let mut shell_process_id = 0;
    unsafe {
        // SAFETY: shell_process_id is writable and shell_window is borrowed from Windows.
        GetWindowThreadProcessId(shell_window, Some(&raw mut shell_process_id));
    }
    if shell_process_id == 0 {
        return Err("Could not identify the non-elevated Windows shell process".to_owned());
    }
    let shell_process = unsafe {
        // SAFETY: the process id belongs to the interactive shell and access is query-only.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, shell_process_id)
    }
    .map(OwnedHandle)
    .map_err(|error| format!("Could not open the non-elevated Windows shell process: {error}"))?;
    let mut primary_token = HANDLE::default();
    unsafe {
        // SAFETY: shell_process is live and primary_token is writable.
        OpenProcessToken(
            shell_process.0,
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &raw mut primary_token,
        )
    }
    .map_err(|error| format!("Could not open the non-elevated Windows shell token: {error}"))?;
    let primary_token = OwnedHandle(primary_token);
    if token_is_elevated(primary_token.0)? {
        return Err(
            "The Windows shell token is elevated, so non-elevated path access cannot be verified"
                .to_owned(),
        );
    }
    let mut impersonation_token = HANDLE::default();
    unsafe {
        // SAFETY: primary_token is live and impersonation_token is writable. AccessCheck requires
        // an impersonation token and does not retain it after returning.
        DuplicateToken(
            primary_token.0,
            SecurityImpersonation,
            &raw mut impersonation_token,
        )
    }
    .map_err(|error| {
        format!("Could not duplicate the non-elevated Windows shell token: {error}")
    })?;
    Ok(OwnedHandle(impersonation_token))
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
    .map_err(|error| format!("Could not inspect the Windows shell token: {error}"))?;
    Ok(elevation.TokenIsElevated != 0)
}

fn executable_is_replaceable(executable: &Path, token: HANDLE) -> Result<bool, String> {
    let parent = executable
        .parent()
        .ok_or_else(|| "The AltTabio executable has no parent folder".to_owned())?;
    let file_write = token_has_access(executable, token, FILE_WRITE_DATA.0)?
        || token_has_access(executable, token, WRITE_DAC.0)?
        || token_has_access(executable, token, WRITE_OWNER.0)?;
    let file_delete = token_has_access(executable, token, DELETE.0)?;
    let parent_add = token_has_access(parent, token, FILE_ADD_FILE.0)?;
    let parent_delete = token_has_access(parent, token, FILE_DELETE_CHILD.0)?;
    let parent_controls_acl = token_has_access(parent, token, WRITE_DAC.0)?
        || token_has_access(parent, token, WRITE_OWNER.0)?;
    Ok(file_write || parent_controls_acl || (parent_add && (file_delete || parent_delete)))
}

fn token_has_access(path: &Path, token: HANDLE, desired_access: u32) -> Result<bool, String> {
    let path = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut dacl = std::ptr::null_mut::<ACL>();
    unsafe {
        // SAFETY: path is null-terminated and descriptor is writable. Windows allocates the
        // returned descriptor with LocalAlloc; OwnedSecurityDescriptor frees it exactly once.
        GetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
            None,
            None,
            Some(&raw mut dacl),
            None,
            &raw mut descriptor,
        )
    }
    .ok()
    .map_err(|error| format!("Could not read autostart path permissions: {error}"))?;
    if descriptor.is_invalid() {
        return Err("Windows returned no security descriptor for the autostart path".to_owned());
    }
    let descriptor = OwnedSecurityDescriptor(descriptor);
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ.0,
        GenericWrite: FILE_GENERIC_WRITE.0,
        GenericExecute: FILE_GENERIC_EXECUTE.0,
        GenericAll: FILE_ALL_ACCESS.0,
    };
    let privilege_words = 128;
    let mut privileges = [0_usize; 128];
    let mut privilege_bytes = u32::try_from(privilege_words * size_of::<usize>())
        .map_err(|error| format!("Privilege buffer is too large: {error}"))?;
    let mut granted_access = 0;
    let mut access_status = BOOL::default();
    unsafe {
        // SAFETY: descriptor and token are live; mapping and all outputs are writable. The aligned
        // privilege buffer is larger than a Windows privilege set and is not retained.
        AccessCheck(
            descriptor.0,
            token,
            desired_access,
            &raw const mapping,
            Some(privileges.as_mut_ptr().cast::<PRIVILEGE_SET>()),
            &raw mut privilege_bytes,
            &raw mut granted_access,
            &raw mut access_status,
        )
    }
    .map_err(|error| format!("Could not evaluate autostart path permissions: {error}"))?;
    Ok(access_status.as_bool() && granted_access & desired_access == desired_access)
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let result = unsafe {
            // SAFETY: this guard uniquely owns the HANDLE and closes it exactly once.
            CloseHandle(self.0)
        };
        if let Err(error) = result {
            eprintln!("Could not close an autostart security handle: {error}");
        }
    }
}

struct OwnedSecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for OwnedSecurityDescriptor {
    fn drop(&mut self) {
        let remaining = unsafe {
            // SAFETY: GetNamedSecurityInfoW allocated this descriptor with LocalAlloc and ownership
            // remains unique until this drop.
            LocalFree(Some(HLOCAL(self.0.0)))
        };
        if !remaining.is_invalid() {
            eprintln!("Could not release an autostart path security descriptor");
        }
    }
}

fn task_targets_executable(xml: &str, executable: &Path) -> Result<bool, String> {
    let settings = element_contents(xml, "Settings")
        .ok_or_else(|| "Autostart task XML has no Settings element".to_owned())?;
    let enabled = element_contents(settings, "Enabled").map_or(Ok(true), |value| {
        match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            value => Err(format!(
                "Autostart task XML has an invalid Enabled value: {value}"
            )),
        }
    })?;
    if !enabled {
        return Ok(false);
    }

    let action = element_contents(xml, "Exec")
        .and_then(|action| element_contents(action, "Command"))
        .ok_or_else(|| "Autostart task XML has no executable command".to_owned())?;
    let action = decode_xml_text(action.trim().trim_matches('"'));
    Ok(paths_match(&PathBuf::from(action), executable))
}

fn element_contents<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = xml.find(&opening)? + opening.len();
    let end = xml.get(start..)?.find(&closing)? + start;
    xml.get(start..end)
}

fn decode_xml_text(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn paths_match(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        path.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', r"\")
    }

    normalized(left).eq_ignore_ascii_case(&normalized(right))
}

fn create_arguments(executable: &Path) -> Vec<OsString> {
    vec![
        "/Create".into(),
        "/TN".into(),
        "AltTabio".into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/RL".into(),
        "HIGHEST".into(),
        "/TR".into(),
        format!("\"{}\"", executable.display()).into(),
        "/F".into(),
    ]
}

fn run(arguments: &[&str]) -> Result<Output, String> {
    run_owned(arguments.iter().map(OsString::from))
}

fn run_owned(arguments: impl IntoIterator<Item = OsString>) -> Result<Output, String> {
    use std::os::windows::process::CommandExt;

    Command::new("schtasks.exe")
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Could not run schtasks.exe: {error}"))
}

fn output_details(output: &Output) -> String {
    let details = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let details = String::from_utf8_lossy(details);
    let details = details.trim();
    if details.is_empty() {
        format!("schtasks.exe exited with {}", output.status)
    } else {
        details.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_arguments_quote_the_executable_and_request_highest_on_logon() {
        let arguments = create_arguments(Path::new(r"C:\Program Files\AltTabio\AltTabio.exe"));
        let arguments = arguments
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["/SC", "ONLOGON"]));
        assert!(arguments.windows(2).any(|pair| pair == ["/RL", "HIGHEST"]));
        assert!(
            arguments
                .iter()
                .any(|value| { value.as_ref() == r#""C:\Program Files\AltTabio\AltTabio.exe""# })
        );
    }

    #[test]
    fn task_status_requires_an_enabled_task_targeting_the_current_executable() {
        let executable = Path::new(r"C:\Apps & Tools\AltTabio.exe");
        let matching = r"<Task><Settings></Settings><Actions><Exec><Command>C:\Apps &amp; Tools\AltTabio.exe</Command></Exec></Actions></Task>";
        let disabled = r"<Task><Settings><Enabled>false</Enabled></Settings><Actions><Exec><Command>C:\Apps &amp; Tools\AltTabio.exe</Command></Exec></Actions></Task>";
        let stale = r"<Task><Settings><Enabled>true</Enabled></Settings><Actions><Exec><Command>C:\Old\AltTabio.exe</Command></Exec></Actions></Task>";

        assert_eq!(task_targets_executable(matching, executable), Ok(true));
        assert_eq!(task_targets_executable(disabled, executable), Ok(false));
        assert_eq!(task_targets_executable(stale, executable), Ok(false));
    }

    #[test]
    fn malformed_task_status_is_reported_instead_of_assumed_enabled() {
        assert!(
            task_targets_executable("<Task><Settings></Settings></Task>", Path::new("app.exe"))
                .is_err()
        );
        assert!(
            task_targets_executable(
                "<Task><Settings><Enabled>maybe</Enabled></Settings></Task>",
                Path::new("app.exe")
            )
            .is_err()
        );
    }

    #[test]
    fn autostart_rejects_an_executable_replaceable_by_non_elevated_processes() {
        let executable = Path::new(r"C:\Users\Example\AltTabio\AltTabio.exe");

        let result = validate_autostart_target_with(executable, |_| Ok(true));

        assert!(result.is_err());
    }
}
