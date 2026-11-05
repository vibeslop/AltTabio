use crate::win32::system_directory;
use alttabio::settings_change::AutostartState;
use std::ffi::OsString;
use std::io::Write;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACL, AccessCheck, DACL_SECURITY_INFORMATION, DuplicateToken, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION, PRIVILEGE_SET,
    PSECURITY_DESCRIPTOR, SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS, FILE_APPEND_DATA,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_WRITE_DATA, WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskDefinition, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TaskScheduler,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{BOOL, BSTR, HRESULT, Interface, PCWSTR};

mod migration;
pub(crate) use migration::repair_legacy_task_timeout;

#[cfg(test)]
mod path_security_tests;

/// Reads only this installation's task. The caller must initialize COM on this thread.
pub fn status() -> Result<AutostartState, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
    let service = connect_scheduler().map_err(|error| error.to_string())?;
    let folder = unsafe {
        // SAFETY: the caller initialized COM on this thread; the service copies the root path.
        service.GetFolder(&BSTR::from(r"\"))
    }
    .map_err(|error| format!("Autostart status query failed: {error}"))?;
    task_status(&folder, &BSTR::from("AltTabio"), &executable)
}

fn connect_scheduler() -> windows::core::Result<ITaskService> {
    unsafe {
        // SAFETY: the caller initialized COM on this thread. Empty variants have no borrowed
        // pointers, and the returned interface owns its reference on the calling thread.
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        Ok(service)
    }
}

fn task_is_missing(error: &windows::core::Error) -> bool {
    [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND]
        .iter()
        .any(|code| error.code() == HRESULT::from_win32(code.0))
}

fn task_status(
    folder: &ITaskFolder,
    name: &BSTR,
    executable: &Path,
) -> Result<AutostartState, String> {
    let task = unsafe {
        // SAFETY: COM is initialized and the live folder copies the owned task name.
        folder.GetTask(name)
    };
    let task = match task {
        Ok(task) => task,
        Err(error) if task_is_missing(&error) => {
            return Ok(AutostartState {
                enabled: false,
                owned_task_exists: false,
            });
        }
        Err(error) => return Err(format!("Autostart status query failed: {error}")),
    };
    let definition = unsafe {
        // SAFETY: the registered task is live on this COM thread; the result owns its reference.
        task.Definition()
    }
    .map_err(|error| format!("Autostart task definition could not be read: {error}"))?;
    let owned_task_exists = task_targets_executable(&definition, executable)
        .map_err(|error| format!("Autostart task ownership could not be read: {error}"))?;
    let enabled = if owned_task_exists {
        unsafe {
            // SAFETY: task is a live COM interface and Enabled returns an owned scalar value.
            task.Enabled()
        }
        .map_err(|error| format!("Autostart task enabled state could not be read: {error}"))?
        .as_bool()
    } else {
        false
    };
    Ok(AutostartState {
        enabled,
        owned_task_exists,
    })
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let output = if enabled {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        validate_autostart_target(&executable)?;
        let task_file = AutostartTaskFile::create(&executable)?;
        run_owned(create_arguments(&task_file.path))?
    } else {
        if !status()?.owned_task_exists {
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
    validate_autostart_target_with(executable, executable_is_replaceable_by_shell)
}

pub(crate) fn executable_is_replaceable_by_shell(executable: &Path) -> Result<bool, String> {
    let token = shell_impersonation_token()?;
    executable_is_replaceable(executable, token.0)
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
    if !executable.is_absolute() || executable.file_name().is_none() {
        return Err("The AltTabio executable path must be an absolute file path".to_owned());
    }
    // Check the original name before resolving it: canonicalization would otherwise hide a
    // junction or symbolic link that an unelevated process could redirect later.
    if path_chain_is_replaceable(executable, token)? {
        return Ok(true);
    }
    let resolved = std::fs::canonicalize(executable)
        .map_err(|error| format!("Could not resolve the installation path: {error}"))?;
    // Drive aliases can resolve to a different directory chain. Both names must be protected.
    path_chain_is_replaceable(&resolved, token)
}

fn path_chain_is_replaceable(executable: &Path, token: HANDLE) -> Result<bool, String> {
    for component in executable.ancestors() {
        if path_component_is_replaceable(component, token)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn path_component_is_replaceable(path: &Path, token: HANDLE) -> Result<bool, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Could not inspect installation path {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(format!(
            "The installation path contains a reparse point: {}. Use a protected folder without links or mounted folders.",
            path.display()
        ));
    }
    if token_has_access(path, token, WRITE_DAC.0)? || token_has_access(path, token, WRITE_OWNER.0)?
    {
        return Ok(true);
    }
    let directory = metadata.is_dir();
    if !directory
        && (token_has_access(path, token, FILE_WRITE_DATA.0)?
            || token_has_access(path, token, FILE_APPEND_DATA.0)?)
    {
        return Ok(true);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    // A directory can be renamed/replaced without touching the executable's own ACL. Creation
    // of unrelated siblings alone is safe; replacement also requires deleting this component.
    let add = if directory {
        FILE_ADD_SUBDIRECTORY
    } else {
        FILE_ADD_FILE
    };
    Ok(token_has_access(parent, token, add.0)?
        && (token_has_access(path, token, DELETE.0)?
            || token_has_access(parent, token, FILE_DELETE_CHILD.0)?))
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

fn task_targets_executable(
    definition: &ITaskDefinition,
    executable: &Path,
) -> windows::core::Result<bool> {
    unsafe {
        // SAFETY: the definition and its owned action interfaces stay on this initialized COM
        // thread. Each initialized output owns its returned value, and no pointer is retained.
        let actions = definition.Actions()?;
        let mut count = 0;
        actions.Count(&raw mut count)?;
        if count != 1 {
            return Ok(false);
        }
        let action = actions.get_Item(1)?;
        let mut kind = TASK_ACTION_EXEC;
        action.Type(&raw mut kind)?;
        if kind != TASK_ACTION_EXEC {
            return Ok(false);
        }
        let executable_action: IExecAction = action.cast()?;
        let mut command = BSTR::new();
        executable_action.Path(&raw mut command)?;
        let command = command.to_string();
        Ok(paths_match(
            Path::new(command.trim().trim_matches('"')),
            executable,
        ))
    }
}

#[cfg(test)]
fn element_contents<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = xml.find(&opening)? + opening.len();
    let end = xml.get(start..)?.find(&closing)? + start;
    xml.get(start..end)
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

struct AutostartTaskFile {
    path: PathBuf,
}

impl AutostartTaskFile {
    fn create(executable: &Path) -> Result<Self, String> {
        let xml = autostart_task_xml(executable)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("Could not timestamp the autostart definition: {error}"))?
            .as_nanos();
        // The caller validates this directory's permissions before enabling elevated autostart.
        // Keeping the XML beside the executable prevents an unelevated process from replacing it.
        let path = executable.with_file_name(format!(
            "AltTabio-autostart-{}-{timestamp}.xml",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("Could not create the autostart definition: {error}"))?;
        let task_file = Self { path };
        // schtasks reads task definitions as UTF-16, including the byte-order mark.
        let bytes = [0xff, 0xfe]
            .into_iter()
            .chain(xml.encode_utf16().flat_map(u16::to_le_bytes))
            .collect::<Vec<_>>();
        let write_result = file
            .write_all(&bytes)
            .map_err(|error| format!("Could not write the autostart definition: {error}"));
        drop(file);
        write_result?;
        Ok(task_file)
    }
}

impl Drop for AutostartTaskFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            eprintln!("Could not remove the temporary autostart definition: {error}");
        }
    }
}

fn create_arguments(task_file: &Path) -> Vec<OsString> {
    vec![
        "/Create".into(),
        "/TN".into(),
        "AltTabio".into(),
        "/XML".into(),
        task_file.as_os_str().to_owned(),
        "/F".into(),
    ]
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
    command.args(arguments).creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
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
            create_arguments(Path::new(r"C:\Program Files\AltTabio\AltTabio.exe")),
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
        let _apartment =
            crate::win32::ComApartment::initialize().map_err(|error| error.to_string())?;
        let service = connect_scheduler().map_err(|error| error.to_string())?;
        let definition = unregistered_task(&service, &xml)?;
        assert!(
            task_targets_executable(&definition, executable).map_err(|error| error.to_string())?
        );
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
    fn create_arguments_import_the_task_definition() {
        let task_file = Path::new(r"C:\Program Files\AltTabio\task.xml");
        let arguments = create_arguments(task_file);
        let arguments = arguments
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["/XML", task_file.to_str().unwrap_or_default()])
        );
    }

    #[test]
    #[ignore = "requires administrator privileges; registers and deletes a temporary task"]
    fn registered_autostart_task_has_no_time_limit() -> Result<(), String> {
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
        let _apartment =
            crate::win32::ComApartment::initialize().map_err(|error| error.to_string())?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let task_file = AutostartTaskFile::create(&executable)?;
        let task_name = format!("AltTabio-Autostart-Regression-{}", std::process::id());
        let service = connect_scheduler().map_err(|error| error.to_string())?;
        let folder = unsafe {
            // SAFETY: this test initialized COM and the service copies the root path.
            service.GetFolder(&BSTR::from(r"\"))
        }
        .map_err(|error| error.to_string())?;
        let name = BSTR::from(&task_name);
        assert_eq!(
            task_status(&folder, &name, &executable)?,
            AutostartState {
                enabled: false,
                owned_task_exists: false
            },
        );
        let mut arguments = create_arguments(&task_file.path);
        arguments[2] = task_name.clone().into();
        let output = run_owned(arguments)?;
        if !output.status.success() {
            return Err(output_details(&output));
        }
        let task_guard = RegisteredTask(task_name.clone());
        assert_eq!(
            task_status(&folder, &name, &executable)?,
            AutostartState {
                enabled: true,
                owned_task_exists: true
            },
        );
        assert_eq!(
            task_status(&folder, &name, Path::new(r"C:\Other\AltTabio.exe"))?,
            AutostartState {
                enabled: false,
                owned_task_exists: false
            },
        );
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
        assert!(task_status(&folder, &name, &executable)?.enabled);
        assert_eq!(element_contents(&xml, "RunLevel"), Some("HighestAvailable"));
        assert_eq!(
            element_contents(&xml, "LogonType"),
            Some("InteractiveToken")
        );
        assert!(
            element_contents(&xml, "LogonTrigger").is_some() || xml.contains("<LogonTrigger />")
        );
        unsafe {
            // SAFETY: only the temporary task owned by this test's cleanup guard is disabled.
            folder
                .GetTask(&name)
                .and_then(|task| task.SetEnabled(false.into()))
        }
        .map_err(|error| error.to_string())?;
        assert_eq!(
            task_status(&folder, &name, &executable)?,
            AutostartState {
                enabled: false,
                owned_task_exists: true
            },
        );
        drop(task_guard);
        assert_eq!(
            task_status(&folder, &name, &executable)?,
            AutostartState {
                enabled: false,
                owned_task_exists: false
            },
        );
        Ok(())
    }

    #[test]
    fn scheduler_failures_are_not_mistaken_for_missing_tasks() {
        use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL};
        use windows::core::Error;

        for code in [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND] {
            assert!(task_is_missing(&Error::from_hresult(HRESULT::from_win32(
                code.0
            ))));
        }
        for code in [E_ACCESSDENIED, E_FAIL] {
            assert!(!task_is_missing(&Error::from_hresult(code)));
        }
    }

    fn unregistered_task(service: &ITaskService, xml: &str) -> Result<ITaskDefinition, String> {
        let definition = unsafe {
            // SAFETY: the test initialized COM and the owned service returns a new definition.
            service.NewTask(0)
        }
        .map_err(|error| error.to_string())?;
        unsafe {
            // SAFETY: definition is live on this COM thread and synchronously copies XML. This
            // creates an in-memory definition only; it never registers or runs a task.
            definition.SetXmlText(&BSTR::from(xml))
        }
        .map_err(|error| error.to_string())?;
        Ok(definition)
    }

    #[test]
    fn task_ownership_requires_the_current_executable_even_when_disabled() -> Result<(), String> {
        let _apartment =
            crate::win32::ComApartment::initialize().map_err(|error| error.to_string())?;
        let service = connect_scheduler().map_err(|error| error.to_string())?;
        let executable = Path::new(r"C:\Apps & Tools\中文\AltTabio.exe");
        for enabled in [true, false] {
            let xml = autostart_task_xml(executable)?.replace(
                "<Settings>",
                &format!("<Settings><Enabled>{enabled}</Enabled>"),
            );
            let definition = unregistered_task(&service, &xml)?;
            assert!(
                task_targets_executable(&definition, executable)
                    .map_err(|error| error.to_string())?
            );
            assert!(
                !task_targets_executable(&definition, Path::new(r"C:\Other\AltTabio.exe"))
                    .map_err(|error| error.to_string())?
            );
        }
        Ok(())
    }

    #[test]
    fn empty_non_executable_and_multi_action_tasks_are_not_owned() -> Result<(), String> {
        use windows::Win32::System::TaskScheduler::TASK_ACTION_COM_HANDLER;
        let _apartment =
            crate::win32::ComApartment::initialize().map_err(|error| error.to_string())?;
        let service = connect_scheduler().map_err(|error| error.to_string())?;
        let executable = Path::new(r"C:\Apps\AltTabio.exe");
        let definition = unsafe {
            // SAFETY: this test initialized COM and creates only an unregistered definition.
            service.NewTask(0)
        }
        .map_err(|error| error.to_string())?;
        assert!(
            !task_targets_executable(&definition, executable).map_err(|error| error.to_string())?
        );
        unsafe {
            // SAFETY: these interfaces remain live on this COM thread. Creating an action only
            // adds it to the in-memory definition; no COM handler is activated.
            definition
                .Actions()
                .and_then(|actions| actions.Create(TASK_ACTION_COM_HANDLER))
        }
        .map_err(|error| error.to_string())?;
        assert!(
            !task_targets_executable(&definition, executable).map_err(|error| error.to_string())?
        );
        let definition = unregistered_task(&service, &autostart_task_xml(executable)?)?;
        unsafe {
            // SAFETY: this modifies only the unregistered definition owned by this test.
            definition
                .Actions()
                .and_then(|actions| actions.Create(TASK_ACTION_EXEC))
        }
        .map_err(|error| error.to_string())?;
        assert!(
            !task_targets_executable(&definition, executable).map_err(|error| error.to_string())?
        );
        Ok(())
    }

    #[test]
    fn autostart_rejects_an_executable_replaceable_by_non_elevated_processes() {
        let executable = Path::new(r"C:\Users\Example\AltTabio\AltTabio.exe");

        let result = validate_autostart_target_with(executable, |_| Ok(true));

        assert!(result.is_err());
    }
}
