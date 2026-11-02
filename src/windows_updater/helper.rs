//! The same GUI executable in a private apply mode. Never initialize the app or its mutex here.
use super::{native, validate_update_target};
use native::{Handle, Permissions};
use std::ffi::OsString;
use std::io::Write;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::core::PWSTR;

pub(super) fn spawn(work: &Path, target: &Path, hash: &str) -> Result<Child, String> {
    let helper = work.join("update-helper.exe");
    if let Err(error) = std::fs::remove_file(&helper)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!(
            "Could not clear the previous update helper: {error}"
        ));
    }
    // Do not reuse a leftover helper. It must be this exact trusted executable.
    let mut source =
        std::fs::File::open(std::env::current_exe().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let mut copy = std::fs::File::create_new(&helper).map_err(|error| error.to_string())?;
    std::io::copy(&mut source, &mut copy).map_err(|error| error.to_string())?;
    copy.sync_all().map_err(|error| error.to_string())?;
    drop(copy);
    native::helper_command(&helper, work)?
        .arg("--apply-update")
        .arg(target)
        .arg(hash)
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|error| format!("Could not start the update helper: {error}"))
}

pub fn run(arguments: &[OsString]) -> bool {
    if arguments
        .get(1)
        .is_none_or(|argument| argument != "--apply-update")
    {
        return false;
    }
    // No fatal-error dialog, app state, or console is created in this internal mode.
    let result = run_arguments(arguments);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
    true
}

fn run_arguments(arguments: &[OsString]) -> Result<(), String> {
    if arguments.len() != 5 {
        return Err("Invalid update helper arguments".to_owned());
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let work = exe.parent().ok_or("Missing update work folder")?;
    let target = PathBuf::from(&arguments[2]);
    let hash = arguments[3]
        .to_str()
        .filter(|hash| super::download::valid_hash(hash))
        .ok_or("Invalid update checksum")?;
    let pid = arguments[4]
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or("Invalid update parent")?;
    if target.parent() != work.parent()
        || exe
            .file_name()
            .is_none_or(|name| name != "update-helper.exe")
        || work
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(super::cleanup::owner_pid)
            != Some(pid)
    {
        return Err("The update helper is outside its protected staging folder".to_owned());
    }
    validate_update_target(&target)?;
    validate_update_target(&exe)?;
    // SAFETY: acquire one handle to this exact process before readiness, so a reused PID cannot
    // be mistaken for the original app when it exits later. The owning guard closes it once.
    let parent = Handle(
        unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .map_err(|error| error.to_string())?,
    );
    let mut path = vec![0_u16; 32768];
    let mut length = u32::try_from(path.len()).map_err(|error| error.to_string())?;
    // SAFETY: parent is live and path has the supplied writable capacity.
    unsafe {
        QueryFullProcessImageNameW(
            parent.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &raw mut length,
        )
    }
    .map_err(|error| error.to_string())?;
    let parent_path = PathBuf::from(OsString::from_wide(&path[..length as usize]));
    if std::fs::canonicalize(parent_path).map_err(|error| error.to_string())?
        != std::fs::canonicalize(&target).map_err(|error| error.to_string())?
    {
        return Err("The update parent is not the installed application".to_owned());
    }
    std::fs::write(work.join("helper.pid"), std::process::id().to_string())
        .map_err(|error| error.to_string())?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(b"READY\n")
        .and_then(|()| stdout.flush())
        .map_err(|error| error.to_string())?;
    drop(stdout);
    // SAFETY: parent remains owned for the entire wait. This helper is dedicated to blocking.
    if unsafe { WaitForSingleObject(parent.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(windows::core::Error::from_thread().to_string());
    }
    drop(parent);
    apply(work, &target, hash, native::launch)
}

fn apply(
    work: &Path,
    target: &Path,
    hash: &str,
    mut launch: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    match std::fs::read_to_string(work.join("apply-approved")) {
        Ok(approved) if approved == hash => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not read update approval: {error}")),
    }
    let previous = work.join("previous.exe");
    let mut replaced = false;
    let mut permissions = None;
    let result = (|| {
        if native::sha256(&work.join("AltTabio.exe"))? != hash {
            return Err("The staged executable failed its SHA-256 check".to_owned());
        }
        permissions = Some(Permissions::read(target)?);
        native::replace(target, &work.join("AltTabio.exe"), &previous)?;
        replaced = true;
        permissions
            .as_ref()
            .ok_or("Missing update permissions")?
            .restore(target)?;
        launch(target)
    })();
    if let Err(error) = result {
        let recovery = (|| {
            if replaced {
                native::replace(target, &previous, &work.join("failed.exe"))?;
            } else if !target.exists() && previous.exists() {
                // ReplaceFile may move the original aside before a partial failure.
                std::fs::rename(&previous, target).map_err(|error| error.to_string())?;
            }
            if let Some(permissions) = &permissions {
                permissions.restore(target)?;
            }
            launch(target)
        })();
        let message = match recovery {
            Ok(()) => error,
            Err(recovery) => {
                format!("{error}; could not restore/restart the previous AltTabio: {recovery}")
            }
        };
        if let Err(error) = std::fs::write(work.join("error.txt"), &message) {
            eprintln!("Could not record update failure: {error}");
        }
        return Err(message);
    }
    // Windows locks this helper's image until exit. Leave its protected directory for the
    // existing maintenance pass after retention, rather than launching a shell to delete it.
    for name in [
        "previous.exe",
        "download.zip",
        "download.zip.minisig",
        "apply-approved",
    ] {
        if let Err(error) = std::fs::remove_file(work.join(name))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("Could not clean update {name}: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::WorkFolder;
    use super::*;

    #[test]
    fn replacement_preserves_permissions_and_settings_and_rolls_back_on_failure() {
        for failure in [false, true] {
            let folder = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
            let target = folder.path.join("user's [copy] $test.exe");
            std::fs::write(&target, b"old executable").unwrap_or_else(|error| panic!("{error}"));
            let permissions = Permissions::read(&target).unwrap_or_else(|error| panic!("{error}"));
            // A different protected staging ACL exercises the ReplaceFile inheritance issue.
            let work = WorkFolder::create(&target).unwrap_or_else(|error| panic!("{error}"));
            std::fs::write(work.path.join("AltTabio.exe"), b"new executable")
                .unwrap_or_else(|error| panic!("{error}"));
            std::fs::write(folder.path.join("AltTabio.ini"), b"keep settings")
                .unwrap_or_else(|error| panic!("{error}"));
            let hash = native::sha256(&work.path.join("AltTabio.exe"))
                .unwrap_or_else(|error| panic!("{error}"));
            // Ordinary exits must neither replace nor relaunch.
            assert_eq!(
                apply(&work.path, &target, &hash, |_| panic!("unapproved launch")),
                Ok(())
            );
            assert_eq!(
                std::fs::read(&target).unwrap_or_default(),
                b"old executable"
            );
            std::fs::write(work.path.join("apply-approved"), &hash)
                .unwrap_or_else(|error| panic!("{error}"));
            let mut launches = Vec::new();
            let result = apply(&work.path, &target, &hash, |path| {
                launches.push(std::fs::read(path).unwrap_or_default());
                if failure && launches.len() == 1 {
                    Err("simulated launch failure".to_owned())
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), failure);
            assert_eq!(launches[0], b"new executable");
            if failure {
                assert_eq!(launches[1], b"old executable");
            }
            assert_eq!(
                std::fs::read(folder.path.join("AltTabio.ini")).unwrap_or_default(),
                b"keep settings"
            );
            let restored = Permissions::read(&target).unwrap_or_else(|error| panic!("{error}"));
            // SetNamedSecurityInfo converts legacy ACL bookkeeping to auto-inheritance.
            // The ACEs, their order and flags, and DACL protection must stay identical.
            assert_eq!(
                permissions.sddl().replace("AI", ""),
                restored.sddl().replace("AI", "")
            );
        }
    }

    #[test]
    fn final_hash_mismatch_never_installs_staged_payload() {
        let folder = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let target = folder.path.join("AltTabio.exe");
        let work = WorkFolder::create(&target).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&target, b"old").unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(work.path.join("AltTabio.exe"), b"tampered")
            .unwrap_or_else(|error| panic!("{error}"));
        let hash = "0".repeat(64);
        std::fs::write(work.path.join("apply-approved"), &hash)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut launches = 0;
        assert!(
            apply(&work.path, &target, &hash, |path| {
                launches += 1;
                assert_eq!(std::fs::read(path).unwrap_or_default(), b"old");
                Ok(())
            })
            .is_err()
        );
        assert_eq!(launches, 1);
        assert!(work.path.join("error.txt").exists());
    }
}
