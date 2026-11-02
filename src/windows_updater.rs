//! Blocking update work stays on the worker. No shell or script interpreter is used.
mod archive;
mod cleanup;
mod download;
mod helper;
mod native;
mod readiness;
mod work_folder;

use alttabio::update::{Checked, Update, Version, newest_windows_update};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use work_folder::WorkFolder;

pub use helper::run as run_helper;

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
                        .ok_or("The installation has no parent folder")?,
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
    // SAFETY: input is writable and its size is initialized for the synchronous call.
    if !unsafe { GetLastInputInfo(&raw mut input) }.as_bool() {
        eprintln!(
            "Could not read the last input time: {}",
            windows::core::Error::from_thread()
        );
        return false;
    }
    // SAFETY: GetTickCount takes no pointers and shares LASTINPUTINFO's wrapping time base.
    unsafe { GetTickCount() }.wrapping_sub(input.dwTime) >= 5 * 60 * 1000
}

pub fn check(manual: bool) -> Result<Checked, String> {
    if !is_release_build() {
        return Err("This development build does not replace itself with published releases. Download a release from GitHub to enable updates.".to_owned());
    }
    let current =
        Version::parse(env!("CARGO_PKG_VERSION")).ok_or("This copy has no release version")?;
    let target = std::env::current_exe().map_err(|error| error.to_string())?;
    let status = validate_update_target(&target);
    if status.is_ok()
        && cleanup::maintain(
            target
                .parent()
                .ok_or("The installation has no parent folder")?,
        )
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            false
        })
        && !manual
    {
        return Err("The previous update failed. Automatic installation is deferred for 24 hours; use Check for Updates to retry manually.".to_owned());
    }
    let update = newest_windows_update(current, &download::releases()?);
    Ok(checked_update(update, || status))
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
    download::stage(update.version, &work.path)?;
    let hash = archive::authenticate_and_extract(
        update.version,
        &work.path,
        alttabio::update_signature::PUBLIC_KEY,
    )?;
    Ok(StagedUpdate { work, hash, target })
}

/// A temporary copy of this EXE waits for us, then replaces and restarts the installation.
pub fn relaunch(staged: &mut StagedUpdate) -> Result<(), String> {
    validate_update_target(&staged.target)?;
    let mut child = helper::spawn(&staged.work.path, &staged.target, &staged.hash)?;
    readiness::wait(&mut child, Duration::from_secs(30))?;
    staged.work.helper_owns = true;
    Ok(())
}

/// Called outside native callbacks only when the app can exit immediately.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_installations_are_reported_before_downloading() {
        let version = Version::parse("9.0.0").unwrap_or_else(|| panic!("version"));
        let update = Update {
            version,
            url: String::new(),
        };
        assert_eq!(
            checked_update(Some(update.clone()), || Err("unsafe folder".to_owned())),
            Checked::Blocked(version, "unsafe folder".to_owned())
        );
        assert_eq!(
            checked_update(Some(update.clone()), || Ok(())),
            Checked::Available(update)
        );
        assert_eq!(
            checked_update(None, || panic!("no update")),
            Checked::UpToDate
        );
        assert!(validate_update_target_with(Path::new("AltTabio.exe"), |_| Ok(true)).is_err());
        assert!(
            validate_update_target_with(Path::new("AltTabio.exe"), |_| Err("denied".to_owned()))
                .is_err()
        );
    }
}
