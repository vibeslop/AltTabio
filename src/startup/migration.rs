//! Temporary compatibility repair for autostart tasks with the inherited 72-hour limit.
//! Remove this module and its startup call once legacy installations have migrated.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskDefinition, ITaskFolder, ITaskService, TASK_ACTION_EXEC,
    TASK_DONT_ADD_PRINCIPAL_ACE, TASK_IGNORE_REGISTRATION_TRIGGERS, TASK_LOGON_INTERACTIVE_TOKEN,
    TASK_LOGON_NONE, TASK_UPDATE, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, HRESULT, Interface, Result};

pub(crate) fn repair_legacy_task_timeout() -> std::result::Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
    let service = connect_scheduler().map_err(|error| error.to_string())?;
    let folder = unsafe {
        // SAFETY: COM is initialized and the owned service synchronously copies the root path.
        service.GetFolder(&BSTR::from(r"\"))
    }
    .map_err(|error| error.to_string())?;
    repair_task(&folder, &BSTR::from("AltTabio"), &executable)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn connect_scheduler() -> Result<ITaskService> {
    unsafe {
        // SAFETY: the caller initialized COM on this thread. Empty variants carry no borrowed
        // pointers, and the returned COM interface owns its reference until it is dropped.
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        Ok(service)
    }
}

fn repair_task(folder: &ITaskFolder, name: &BSTR, executable: &Path) -> Result<bool> {
    unsafe {
        // SAFETY: the caller initialized COM. Interfaces own their references, every BSTR output
        // is initialized, and registration synchronously copies its owned variants and definition.
        let task = match folder.GetTask(name) {
            Ok(task) => task,
            Err(error)
                if [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND]
                    .iter()
                    .any(|code| error.code() == HRESULT::from_win32(code.0)) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let definition = task.Definition()?;
        if !repair_definition(&definition, executable)? {
            return Ok(false);
        }
        let mut user_id = BSTR::new();
        definition.Principal()?.UserId(&raw mut user_id)?;
        let user_id = VARIANT::from(user_id);
        let empty = VARIANT::default();
        // Update only: never recreate a task deleted during the check, run registration triggers,
        // or change its principal's access-control entries.
        folder.RegisterTaskDefinition(
            name,
            &definition,
            TASK_UPDATE.0 | TASK_IGNORE_REGISTRATION_TRIGGERS.0 | TASK_DONT_ADD_PRINCIPAL_ACE.0,
            &user_id,
            &empty,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &empty,
        )?;
        Ok(true)
    }
}

fn repair_definition(definition: &ITaskDefinition, executable: &Path) -> Result<bool> {
    unsafe {
        // SAFETY: COM is initialized; the borrowed definition remains live for these synchronous
        // calls, and initialized scalar/BSTR outputs own all data returned by the interfaces.
        let actions = definition.Actions()?;
        let mut count = 0;
        actions.Count(&raw mut count)?;
        if count != 1 {
            return Ok(false);
        }
        let action = actions.get_Item(1)?;
        let mut action_type = TASK_ACTION_EXEC;
        action.Type(&raw mut action_type)?;
        if action_type != TASK_ACTION_EXEC {
            return Ok(false);
        }
        let action: IExecAction = action.cast()?;
        let mut path = BSTR::new();
        action.Path(&raw mut path)?;
        if !super::paths_match(
            &PathBuf::from(path.to_string().trim_matches('"')),
            executable,
        ) {
            return Ok(false);
        }
        let mut logon_type = TASK_LOGON_NONE;
        definition.Principal()?.LogonType(&raw mut logon_type)?;
        if logon_type != TASK_LOGON_INTERACTIVE_TOKEN {
            return Ok(false);
        }
        let settings = definition.Settings()?;
        let mut limit = BSTR::new();
        settings.ExecutionTimeLimit(&raw mut limit)?;
        // This is a migration of the old default, not a permanent policy overriding custom limits.
        if limit != "PT72H" {
            return Ok(false);
        }
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    struct Apartment;

    impl Apartment {
        fn initialize() -> Result<Self> {
            unsafe {
                // SAFETY: this test thread balances every successful initialization in Drop.
                CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            }
            Ok(Self)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                // SAFETY: this guard drops on the same test thread that initialized COM.
                CoUninitialize();
            }
        }
    }

    fn definition(
        service: &ITaskService,
        executable: &Path,
        limit: &str,
    ) -> Result<ITaskDefinition> {
        let xml = crate::startup::autostart_task_xml(executable)
            .map_err(|error| windows::core::Error::new(E_FAIL, &error))?
            .replace(
                "<LogonTrigger />",
                "<LogonTrigger><Enabled>false</Enabled></LogonTrigger>",
            )
            .replace("<Settings>", "<Settings><Enabled>false</Enabled>")
            .replace("PT0S", limit);
        unsafe {
            // SAFETY: COM is initialized, the service is owned, and XML is copied synchronously.
            let definition = service.NewTask(0)?;
            definition.SetXmlText(&BSTR::from(xml))?;
            Ok(definition)
        }
    }

    fn xml(definition: &ITaskDefinition) -> Result<String> {
        let mut xml = BSTR::new();
        unsafe {
            // SAFETY: the interface is live and the initialized output owns the returned BSTR.
            definition.XmlText(&raw mut xml)?;
        }
        Ok(xml.to_string())
    }

    #[test]
    fn legacy_definition_changes_only_the_runtime_limit() -> Result<()> {
        let _apartment = Apartment::initialize()?;
        let service = connect_scheduler()?;
        let executable = Path::new(r"C:\Apps & Tools\中文\AltTabio.exe");
        let definition = definition(&service, executable, "PT72H")?;
        let before = xml(&definition)?;
        assert!(repair_definition(&definition, executable)?);
        assert_eq!(xml(&definition)?, before.replace("PT72H", "PT0S"));
        assert!(!repair_definition(&definition, executable)?);
        Ok(())
    }

    #[test]
    fn unlimited_and_custom_limits_are_untouched() -> Result<()> {
        let _apartment = Apartment::initialize()?;
        let service = connect_scheduler()?;
        let executable = Path::new(r"C:\Program Files\AltTabio\AltTabio.exe");
        for limit in ["PT0S", "PT1H"] {
            let definition = definition(&service, executable, limit)?;
            let before = xml(&definition)?;
            assert!(!repair_definition(&definition, executable)?);
            assert_eq!(xml(&definition)?, before);
        }
        Ok(())
    }

    #[test]
    fn a_task_for_another_installation_is_untouched() -> Result<()> {
        let _apartment = Apartment::initialize()?;
        let service = connect_scheduler()?;
        let definition = definition(&service, Path::new(r"C:\Old\AltTabio.exe"), "PT72H")?;
        let before = xml(&definition)?;
        assert!(!repair_definition(
            &definition,
            Path::new(r"C:\New\AltTabio.exe")
        )?);
        assert_eq!(xml(&definition)?, before);
        Ok(())
    }

    #[test]
    fn extra_actions_and_password_logon_are_untouched() -> Result<()> {
        use windows::Win32::System::TaskScheduler::TASK_LOGON_PASSWORD;
        let _apartment = Apartment::initialize()?;
        let service = connect_scheduler()?;
        let executable = Path::new(r"C:\Program Files\AltTabio\AltTabio.exe");
        let extra_action = definition(&service, executable, "PT72H")?;
        let password_logon = definition(&service, executable, "PT72H")?;
        unsafe {
            // SAFETY: these are owned in-memory test definitions and copy their BSTR arguments.
            let action: IExecAction = extra_action.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
            action.SetPath(&BSTR::from(r"C:\Other.exe"))?;
            password_logon
                .Principal()?
                .SetLogonType(TASK_LOGON_PASSWORD)?;
        }
        for definition in [extra_action, password_logon] {
            let before = xml(&definition)?;
            assert!(!repair_definition(&definition, executable)?);
            assert_eq!(xml(&definition)?, before);
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires administrator privileges; registers and deletes a disabled temporary task"]
    fn registered_legacy_task_is_repaired_once() -> Result<()> {
        use windows::Win32::System::TaskScheduler::TASK_CREATE;
        struct RegisteredTask {
            folder: ITaskFolder,
            name: BSTR,
        }
        impl Drop for RegisteredTask {
            fn drop(&mut self) {
                let result = unsafe {
                    // SAFETY: this guard owns the name of the disabled fixture task it created.
                    self.folder.DeleteTask(&self.name, 0)
                };
                if let Err(error) = result {
                    eprintln!("Could not remove migration test task: {error}");
                }
            }
        }
        let _apartment = Apartment::initialize()?;
        let service = connect_scheduler()?;
        let executable = std::env::current_exe()
            .map_err(|error| windows::core::Error::new(E_FAIL, error.to_string()))?;
        let definition = definition(&service, &executable, "PT72H")?;
        let name = BSTR::from(format!(
            "AltTabio-Migration-Regression-{}",
            std::process::id()
        ));
        let empty = VARIANT::default();
        let folder = unsafe {
            // SAFETY: COM is initialized and the root path is copied synchronously.
            service.GetFolder(&BSTR::from(r"\"))?
        };
        assert!(!repair_task(&folder, &name, &executable)?);
        let original = unsafe {
            // SAFETY: the disabled fixture cannot run and all registration inputs are owned.
            folder.RegisterTaskDefinition(
                &name,
                &definition,
                TASK_CREATE.0 | TASK_IGNORE_REGISTRATION_TRIGGERS.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &empty,
            )?
        };
        let fixture = RegisteredTask { folder, name };
        unsafe {
            // SAFETY: this disabled fixture is owned by the cleanup guard. Adding read-only
            // access verifies that migration also preserves a customized task DACL.
            let security =
                original.GetSecurityDescriptor(DACL_SECURITY_INFORMATION.0.cast_signed())?;
            original.SetSecurityDescriptor(
                &BSTR::from(format!("{security}(A;;FR;;;WD)")),
                TASK_DONT_ADD_PRINCIPAL_ACE.0,
            )?;
            assert_ne!(
                original.GetSecurityDescriptor(DACL_SECURITY_INFORMATION.0.cast_signed())?,
                security
            );
        }
        let before = unsafe {
            // SAFETY: original is an owned registered-task interface used on its COM thread.
            (
                xml(&original.Definition()?)?,
                original.GetSecurityDescriptor(DACL_SECURITY_INFORMATION.0.cast_signed())?,
            )
        };
        assert!(repair_task(&fixture.folder, &fixture.name, &executable)?);
        let repaired = unsafe {
            // SAFETY: the fixture still exists and both borrowed inputs remain live.
            fixture.folder.GetTask(&fixture.name)?
        };
        let after = unsafe {
            // SAFETY: repaired is owned on this COM thread; the outputs are owned strings.
            (
                xml(&repaired.Definition()?)?,
                repaired.GetSecurityDescriptor(DACL_SECURITY_INFORMATION.0.cast_signed())?,
            )
        };
        assert_eq!(after.0, before.0.replace("PT72H", "PT0S"));
        assert_eq!(after.1, before.1);
        assert!(!repair_task(&fixture.folder, &fixture.name, &executable)?);
        Ok(())
    }
}
