//! Looking for a newer release and putting it in place of the running copy, the way
//! `docs/install.sh` does: the GitHub release list, `curl`, `ditto`, and `codesign` against the
//! pinned release certificate. All of it blocks, so each job runs on a thread of its own and
//! reports back through `post_to_app`.

use super::runtime::post_to_app;
use alttabio::update::{Asset, Checked, Release, Update, Version, newest_update};
use objc2::DowncastTarget;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSNumber,
    NSString,
};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RELEASES_URL: &str = "https://api.github.com/repos/vibeslop/AltTabio/releases?per_page=30";
// scripts/mac/package.sh packages nothing signed with another certificate.
const RELEASE_CERTIFICATE: &str = include_str!("../../scripts/mac/release-certificate.sha1");

/// The running copy's bundle, which an update replaces.
pub fn bundle_path() -> PathBuf {
    PathBuf::from(NSBundle::mainBundle().bundlePath().to_string())
}

/// Seconds since the last key press, click, or pointer movement in this login session.
pub fn seconds_since_input() -> f64 {
    // kCGAnyInputEventType, which covers every kind of input event.
    let any_input = CGEventType(u32::MAX);
    CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        any_input,
    )
}

/// Looks for a release newer than this copy.
pub fn check(bundle: PathBuf) {
    let started = spawn(move || {
        let result = Version::parse(env!("CARGO_PKG_VERSION"))
            .ok_or_else(|| "This copy has no release version".to_owned())
            .and_then(|current| check_now(current, &bundle));
        post_to_app(move |app| app.update_checked(result));
    });
    if let Err(error) = started {
        post_to_app(move |app| app.update_checked(Err(error)));
    }
}

/// Downloads `update` and puts it in place of the copy at `bundle`.
pub fn install(update: Update, bundle: PathBuf) {
    let started = spawn(move || {
        let result = install_now(&update, &bundle);
        post_to_app(move |app| app.update_installed(result));
    });
    if let Err(error) = started {
        post_to_app(move |app| app.update_installed(Err(error)));
    }
}

/// Opens the copy at `bundle` once this process has ended, with the settings window when
/// `open_settings` is set.
///
/// The new copy refuses to run beside this one, so a shell outside the app waits for the end.
/// Launch Services then opens the bundle, which makes the new copy an app of its own with its
/// own permissions rather than a child of the shell. The shell's own process group keeps it
/// alive when the app's group ends with the app.
pub fn relaunch_after_exit(bundle: &Path, open_settings: bool) -> Result<(), String> {
    // A copy still running after ten seconds did not quit, and opening the bundle would only
    // bring it forward, so the shell gives up and the update waits for the next start.
    const SCRIPT: &str = r#"tries=0
while /bin/kill -0 "$1" 2>/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -le 100 ] || exit 1
    /bin/sleep 0.1
done
shift
exec /usr/bin/open "$@""#;
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", SCRIPT, "sh"])
        .arg(std::process::id().to_string())
        .arg(bundle);
    if open_settings {
        command.args(["--args", "--settings"]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|error| format!("Could not start the updated AltTabio: {error}"))
}

fn spawn(work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new()
        .name("alttabio-update".to_owned())
        // AppKit drains no pool on a thread it didn't start.
        .spawn(move || autoreleasepool(|_| work()))
        .map(drop)
        .map_err(|error| format!("Could not start the update thread: {error}"))
}

fn check_now(current: Version, bundle: &Path) -> Result<Checked, String> {
    let list = run(curl()
        .args(["--max-time", "30", "--header"])
        .arg("Accept: application/vnd.github+json")
        .arg(RELEASES_URL))
    .map_err(|error| format!("Could not get the list of releases from GitHub: {error}"))?;
    let Some(update) = newest_update(current, &read_releases(&list)?) else {
        return Ok(Checked::UpToDate);
    };
    Ok(match obstacle(bundle) {
        Some(reason) => Checked::Blocked(update.version, reason),
        None => Checked::Available(update),
    })
}

/// Why the copy at `bundle` cannot replace itself, if it cannot.
fn obstacle(bundle: &Path) -> Option<String> {
    // macOS keeps the permissions only for a copy signed with the same certificate. A copy
    // built from source carries another one, and an update would cost it both permissions.
    if verify(bundle).is_err() {
        return Some(
            "This copy is not signed with the release certificate, so a release would lose its \
             Accessibility and Screen Recording permissions."
                .to_owned(),
        );
    }
    let Some(folder) = bundle.parent() else {
        return Some("This copy is not in a folder it could be replaced in.".to_owned());
    };
    if !writable(folder) {
        return Some(format!(
            "This account cannot change {}. Run the install command as an administrator to \
             update.",
            folder.display()
        ));
    }
    None
}

/// Puts `update` in place of the copy at `bundle`. Nothing next to the copy changes until the
/// download has passed its signature check, and a failed swap puts the old copy back.
fn install_now(update: &Update, bundle: &Path) -> Result<(), String> {
    let version = update.version;
    let folder = bundle
        .parent()
        .ok_or_else(|| "This copy is not in a folder it could be replaced in".to_owned())?;
    let work = WorkFolder::create()?;
    let archive = work.path.join("AltTabio.zip");
    run(curl()
        .args([
            "--max-time",
            "600",
            "--max-filesize",
            "104857600",
            "--output",
        ])
        .arg(&archive)
        .arg(&update.url))
    .map_err(|error| format!("Could not download AltTabio {version}: {error}"))?;
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&archive)
        .arg(&work.path))
    .map_err(|error| {
        format!("The download of AltTabio {version} is not a readable archive: {error}")
    })?;
    let app = work.path.join("AltTabio.app");
    verify(&app).map_err(|error| {
        format!("The download of AltTabio {version} failed its signature check: {error}")
    })?;
    let found = run(Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist")))?;
    let found = String::from_utf8_lossy(&found);
    if found.trim() != version.to_string() {
        return Err(format!(
            "The download of AltTabio {version} holds version {}",
            found.trim()
        ));
    }

    // The new copy goes next to the old one first, so the swap is two renames on one volume.
    let staged = folder.join(".AltTabio.app.new");
    let previous = folder.join(".AltTabio.app.old");
    remove(&staged)?;
    remove(&previous)?;
    run(Command::new("/usr/bin/ditto").arg(&app).arg(&staged)).map_err(|error| {
        format!(
            "Could not copy AltTabio {version} into {}: {error}",
            folder.display()
        )
    })?;
    if let Err(error) = std::fs::rename(bundle, &previous) {
        discard(&staged);
        return Err(format!(
            "Could not move the installed AltTabio aside: {error}"
        ));
    }
    if let Err(error) = std::fs::rename(&staged, bundle) {
        if let Err(restore) = std::fs::rename(&previous, bundle) {
            return Err(format!(
                "Could not put AltTabio {version} in place ({error}) or the old copy back \
                 ({restore}); the old copy is at {}",
                previous.display()
            ));
        }
        discard(&staged);
        return Err(format!(
            "Could not put AltTabio {version} in place: {error}"
        ));
    }
    // This process still runs from the old copy, which keeps its files open; what it might load
    // later comes from the new copy at the same path.
    discard(&previous);
    Ok(())
}

/// Checks that the bundle at `app` is intact and signed with the release certificate under
/// `AltTabio`'s identifier, the requirement macOS ties the permissions to.
fn verify(app: &Path) -> Result<(), String> {
    let requirement = format!(
        "=identifier \"com.vibeslop.AltTabio\" and certificate leaf = H\"{}\"",
        RELEASE_CERTIFICATE.trim()
    );
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "-R"])
        .arg(requirement)
        .arg(app))
    .map(drop)
}

/// `curl` as `docs/install.sh` uses it, held to HTTPS through every redirect.
fn curl() -> Command {
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--user-agent",
        "AltTabio",
    ]);
    command
}

/// Runs `command` and returns what it printed, or what it said about failing.
fn run(command: &mut Command) -> Result<Vec<u8>, String> {
    let program = Path::new(command.get_program()).display().to_string();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if message.is_empty() {
        format!("{program} ended with {}", output.status)
    } else {
        message
    })
}

fn read_releases(json: &[u8]) -> Result<Vec<Release>, String> {
    let unreadable = || "GitHub sent a list of releases AltTabio cannot read".to_owned();
    let list = NSJSONSerialization::JSONObjectWithData_options_error(
        &NSData::with_bytes(json),
        NSJSONReadingOptions::empty(),
    )
    .map_err(|_| unreadable())?
    .downcast::<NSArray>()
    .map_err(|_| unreadable())?;
    Ok(list.iter().filter_map(|item| read_release(&item)).collect())
}

fn read_release(item: &AnyObject) -> Option<Release> {
    let release = item.downcast_ref::<NSDictionary>()?;
    let assets = field::<NSArray>(release, "assets")?
        .iter()
        .filter_map(|item| {
            let asset = item.downcast_ref::<NSDictionary>()?;
            Some(Asset {
                name: field::<NSString>(asset, "name")?.to_string(),
                url: field::<NSString>(asset, "browser_download_url")?.to_string(),
            })
        })
        .collect();
    Some(Release {
        tag: field::<NSString>(release, "tag_name")?.to_string(),
        draft: field::<NSNumber>(release, "draft")?.boolValue(),
        prerelease: field::<NSNumber>(release, "prerelease")?.boolValue(),
        assets,
    })
}

fn field<T: DowncastTarget>(object: &NSDictionary, key: &str) -> Option<Retained<T>> {
    object
        .objectForKey(&NSString::from_str(key))?
        .downcast::<T>()
        .ok()
}

fn writable(folder: &Path) -> bool {
    let Ok(path) = CString::new(folder.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe {
        // SAFETY: `path` is a NUL-terminated string that outlives the call.
        libc::access(path.as_ptr(), libc::W_OK) == 0
    }
}

fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("Could not remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Removes what a failed or finished install leaves; a leftover only takes space until the next
/// install removes it.
fn discard(path: &Path) {
    if let Err(error) = remove(path) {
        eprintln!("{error}");
    }
}

/// A folder of this process's own for the download, gone again when the install ends.
struct WorkFolder {
    path: PathBuf,
}

impl WorkFolder {
    fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("AltTabio-update-{}", std::process::id()));
        // A process that had the same id before this one may have ended mid-install.
        remove(&path)?;
        std::fs::create_dir(&path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for WorkFolder {
    fn drop(&mut self) {
        discard(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_list_reads_like_the_github_api_writes_it() {
        let json = br#"[
            {"tag_name": "v1.2.0", "draft": false, "prerelease": false, "body": null,
             "assets": [
                {"name": "AltTabio-1.2.0-macos.zip", "size": 5,
                 "browser_download_url": "https://github.com/a/AltTabio-1.2.0-macos.zip"},
                {"name": "notes.txt", "browser_download_url": null}
             ]},
            {"tag_name": "v1.1.9", "draft": true, "prerelease": false, "assets": []},
            {"tag_name": "v1.1.8", "draft": "no", "prerelease": false, "assets": []},
            "not a release"
        ]"#;

        assert_eq!(
            read_releases(json),
            Ok(vec![
                Release {
                    tag: "v1.2.0".to_owned(),
                    draft: false,
                    prerelease: false,
                    assets: vec![Asset {
                        name: "AltTabio-1.2.0-macos.zip".to_owned(),
                        url: "https://github.com/a/AltTabio-1.2.0-macos.zip".to_owned(),
                    }],
                },
                Release {
                    tag: "v1.1.9".to_owned(),
                    draft: true,
                    prerelease: false,
                    assets: Vec::new(),
                },
            ])
        );
    }

    #[test]
    fn anything_but_a_list_is_unreadable() {
        for json in [
            &b"{\"message\": \"API rate limit exceeded\"}"[..],
            b"<html>",
            b"",
        ] {
            assert!(read_releases(json).is_err());
        }
    }
}
