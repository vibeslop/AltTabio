use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACL, AccessCheck, DACL_SECURITY_INFORMATION, DuplicateToken, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION, PRIVILEGE_SET,
    PSECURITY_DESCRIPTOR, SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA, FileAttributeTagInfo,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, OPEN_EXISTING, READ_CONTROL,
    VOLUME_NAME_DOS, WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::TaskScheduler::{
    ITaskService, TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN, TaskScheduler,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{BOOL, BSTR, PCWSTR};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

mod migration;
pub(crate) use migration::repair_legacy_task_timeout;

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
    if enabled {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        let target = validate_autostart_target(&executable)?;
        return register_task("AltTabio", &target.executable);
    }
    if !status()?.task_exists {
        return Ok(());
    }
    let output = run(&["/Delete", "/TN", "AltTabio", "/F"])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Autostart task update failed: {}",
            output_details(&output)
        ))
    }
}

struct TrustedTarget {
    executable: PathBuf,
    // No component can be renamed while the in-memory definition is registered.
    _handles: Vec<OwnedHandle>,
}

fn validate_autostart_target(executable: &Path) -> Result<TrustedTarget, String> {
    let token = shell_impersonation_token()?;
    if !executable.is_absolute() {
        return Err("The autostart executable path must be absolute".to_owned());
    }
    let mut handles = Vec::new();
    let mut components = executable.ancestors().collect::<Vec<_>>();
    components.reverse();
    for path in components {
        // A bare drive prefix is not a filesystem object; its root is opened next.
        if path.parent().is_none() && !path.has_root() {
            continue;
        }
        let handle = open_path_component(path, path == executable)?;
        validate_autostart_target_with(path, |_| {
            handle_is_replaceable(handle.0, token.0, path == executable)
        })?;
        handles.push(handle);
    }
    let handle = handles
        .last()
        .ok_or_else(|| "No autostart path components were opened".to_owned())?;
    let mut buffer = vec![0_u16; 32768];
    let length = unsafe {
        // SAFETY: the validated file handle stays live and the output buffer is writable.
        GetFinalPathNameByHandleW(handle.0, &mut buffer, VOLUME_NAME_DOS)
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("Could not resolve the validated autostart executable".to_owned());
    }
    let resolved = PathBuf::from(OsString::from_wide(&buffer[..length]));
    if !paths_match(&resolved, executable) {
        return Err("The autostart path changed while its permissions were checked".to_owned());
    }
    Ok(TrustedTarget {
        executable: resolved,
        _handles: handles,
    })
}

fn open_path_component(path: &Path, file: bool) -> Result<OwnedHandle, String> {
    let name = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let handle = unsafe {
        // SAFETY: name is terminated; this guard owns the handle. No delete sharing keeps
        // the component in place, and OPEN_REPARSE_POINT prevents following a final link.
        CreateFileW(
            PCWSTR(name.as_ptr()),
            READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0,
            if file {
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            },
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map(OwnedHandle)
    .map_err(|error| format!("Could not securely open {}: {error}", path.display()))?;
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    unsafe {
        // SAFETY: the handle is live and attributes is writable for its exact size.
        GetFileInformationByHandleEx(
            handle.0,
            FileAttributeTagInfo,
            (&raw mut attributes).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>()).unwrap_or_default(),
        )
    }
    .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(format!(
            "Autostart paths cannot contain reparse points: {}",
            path.display()
        ));
    }
    Ok(handle)
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

fn handle_is_replaceable(handle: HANDLE, token: HANDLE, file: bool) -> Result<bool, String> {
    for access in [
        WRITE_DAC.0,
        WRITE_OWNER.0,
        DELETE.0,
        if file {
            FILE_WRITE_DATA.0
        } else {
            FILE_DELETE_CHILD.0
        },
    ] {
        if token_has_access(handle, token, access)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn token_has_access(handle: HANDLE, token: HANDLE, desired_access: u32) -> Result<bool, String> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut dacl = std::ptr::null_mut::<ACL>();
    unsafe {
        // SAFETY: handle is live and descriptor is writable. Windows allocates the
        // returned descriptor with LocalAlloc; OwnedSecurityDescriptor frees it exactly once.
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
            None,
            None,
            Some(&raw mut dacl),
            None,
            Some(&raw mut descriptor),
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
            // SAFETY: GetSecurityInfo allocated this descriptor with LocalAlloc and ownership
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

fn autostart_task_xml(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "The autostart executable path is not valid Unicode".to_owned())?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger /></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
  <Actions Context="Author"><Exec><Command>{executable}</Command></Exec></Actions>
</Task>"#
    ))
}

fn register_task(name: &str, executable: &Path) -> Result<(), String> {
    let xml = autostart_task_xml(executable)?;
    unsafe {
        // SAFETY: the caller initialized COM; every BSTR/variant remains live during
        // synchronous calls. Task Scheduler copies the XML without a filesystem handoff.
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?;
        let empty = VARIANT::default();
        service
            .Connect(&empty, &empty, &empty, &empty)
            .map_err(|error| error.to_string())?;
        let folder = service
            .GetFolder(&BSTR::from(r"\"))
            .map_err(|error| error.to_string())?;
        folder
            .RegisterTask(
                &BSTR::from(name),
                &BSTR::from(xml),
                TASK_CREATE_OR_UPDATE.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &empty,
            )
            .map_err(|error| format!("Could not register autostart: {error}"))?;
    }
    Ok(())
}

fn run(arguments: &[&str]) -> Result<Output, String> {
    run_owned(arguments.iter().map(OsString::from))
}

fn run_owned(arguments: impl IntoIterator<Item = OsString>) -> Result<Output, String> {
    scheduler_command(arguments)?
        .output()
        .map_err(|error| format!("Could not run schtasks.exe: {error}"))
}

fn scheduler_command(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new(system_directory()?.join("schtasks.exe"));
    command.args(arguments).creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

fn system_directory() -> Result<PathBuf, String> {
    // Reserve the maximum Windows path length, including the terminating null.
    let mut buffer = vec![0_u16; 32768];
    let length = unsafe {
        // SAFETY: buffer is writable for its full length and the API retains no pointers.
        GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    if length == 0 {
        return Err(format!(
            "Could not locate the Windows system directory: {}",
            windows::core::Error::from_thread()
        ));
    }
    if length >= buffer.len() {
        return Err("The Windows system directory path exceeds the supported length".to_owned());
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..length]));
    if !path.is_absolute() {
        return Err("Windows returned a non-absolute system directory path".to_owned());
    }
    Ok(path)
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
    fn scheduler_command_uses_an_absolute_system_executable_without_launching_it()
    -> Result<(), String> {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

        let mut directory = vec![0_u16; 32768];
        let length = unsafe {
            // SAFETY: directory is writable for the complete slice; no pointers are retained.
            GetSystemDirectoryW(Some(&mut directory))
        } as usize;
        assert!(length > 0 && length < directory.len());
        let expected =
            PathBuf::from(OsString::from_wide(&directory[..length])).join("schtasks.exe");

        for arguments in [
            vec![
                "/Query".into(),
                "/TN".into(),
                "AltTabio".into(),
                "/XML".into(),
            ],
            vec![
                "/Delete".into(),
                "/TN".into(),
                "AltTabio".into(),
                "/F".into(),
            ],
        ] {
            let command = scheduler_command(arguments.clone())?;
            assert!(Path::new(command.get_program()).is_absolute());
            assert_eq!(command.get_program(), expected.as_os_str());
            assert_eq!(command.get_args().collect::<Vec<_>>(), arguments);
        }
        Ok(())
    }

    #[test]
    fn autostart_definition_preserves_the_path_and_disables_the_runtime_limit() -> Result<(), String>
    {
        let executable = Path::new(r"C:\Apps & Tools\中文\AltTabio.exe");
        let xml = autostart_task_xml(executable)?;
        assert!(task_targets_executable(&xml, executable)?);
        assert_eq!(element_contents(&xml, "ExecutionTimeLimit"), Some("PT0S"));
        assert_eq!(element_contents(&xml, "RunLevel"), Some("HighestAvailable"));
        assert_eq!(
            element_contents(&xml, "LogonType"),
            Some("InteractiveToken")
        );
        assert!(xml.contains("<LogonTrigger />"));
        Ok(())
    }

    #[test]
    #[ignore = "requires administrator privileges; registers and deletes a temporary task"]
    fn registered_autostart_task_has_no_time_limit() -> Result<(), String> {
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                // SAFETY: the test initializes COM once on this same thread.
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        }
        struct RegisteredTask(String);
        impl Drop for RegisteredTask {
            fn drop(&mut self) {
                match run(&["/Delete", "/TN", &self.0, "/F"]) {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        eprintln!("Could not remove test task: {}", output_details(&output));
                    }
                    Err(error) => eprintln!("Could not remove test task: {error}"),
                }
            }
        }
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let task_name = format!("AltTabio-Autostart-Regression-{}", std::process::id());
        // SAFETY: this test owns and balances COM initialization on its thread.
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
            .ok()
        }
        .map_err(|error| error.to_string())?;
        let _apartment = Apartment;
        register_task(&task_name, &executable)?;
        let _task = RegisteredTask(task_name.clone());
        let output = run(&["/Query", "/TN", &task_name, "/XML"])?;
        if !output.status.success() {
            return Err(output_details(&output));
        }
        let xml = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
        let settings = element_contents(&xml, "Settings")
            .ok_or_else(|| "Registered task has no Settings".to_owned())?;
        assert_eq!(
            element_contents(settings, "ExecutionTimeLimit"),
            Some("PT0S")
        );
        assert!(task_targets_executable(&xml, &executable)?);
        assert_eq!(element_contents(&xml, "RunLevel"), Some("HighestAvailable"));
        assert_eq!(
            element_contents(&xml, "LogonType"),
            Some("InteractiveToken")
        );
        assert!(
            element_contents(&xml, "LogonTrigger").is_some() || xml.contains("<LogonTrigger />")
        );
        Ok(())
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
