//! Authenticate every archive byte before opening ZIP metadata or extracting anything.
use super::native::{path_wide, sha256};
use alttabio::{
    update::Version,
    update_signature,
    update_status::{Failure, Stage},
};
use std::cell::Cell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use windows::Win32::Foundation::{
    ERROR_BAD_EXE_FORMAT, ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_LANG_NOT_FOUND,
    ERROR_RESOURCE_NAME_NOT_FOUND, ERROR_RESOURCE_TYPE_NOT_FOUND, SetLastError, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::{Error, HRESULT, PCWSTR};

pub(super) const MAX_EXE: u64 = 50 * 1024 * 1024;

pub(super) fn authenticate_and_extract(
    version: Version,
    work: &Path,
    public_key: &str,
) -> Result<String, Failure> {
    let mut signature = Vec::new();
    File::open(work.join("download.zip.minisig"))
        .and_then(|file| {
            file.take(update_signature::MAX_SIGNATURE_BYTES + 1)
                .read_to_end(&mut signature)
        })
        .map_err(storage)?;
    let signature = String::from_utf8(signature)
        .map_err(|error| invalid(format!("The update signature is not UTF-8: {error}")))?;
    // Keep the same file handle across authentication and extraction in the protected folder.
    let mut archive = File::open(work.join("download.zip")).map_err(storage)?;
    let read_failed = Cell::new(false);
    update_signature::verify(
        TrackedReader {
            input: &mut archive,
            failed: &read_failed,
        },
        &signature,
        version,
        public_key,
    )
    .map_err(|detail| {
        if read_failed.get() {
            Failure::new(Stage::Extract, detail)
        } else {
            invalid(detail)
        }
    })?;
    archive.rewind().map_err(storage)?;
    let exe = work.join("AltTabio.exe");
    extract(archive, &exe)?;
    validate_version(&exe, version)?;
    sha256(&exe).map_err(|detail| Failure::new(Stage::Extract, detail))
}

fn invalid(detail: impl Into<String>) -> Failure {
    Failure::new(Stage::Verify, detail)
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err transfers ownership of each I/O error into an owned diagnostic"
)]
fn storage(error: std::io::Error) -> Failure {
    Failure::new(Stage::Extract, error.to_string())
}

fn zip_failure(error: zip::result::ZipError) -> Failure {
    match error {
        zip::result::ZipError::Io(error) => storage(error),
        error => invalid(error.to_string()),
    }
}

fn extract(mut archive: File, exe: &Path) -> Result<(), Failure> {
    let entries = declared_entries(&mut archive)?;
    let read_failed = Cell::new(false);
    let mut zip = zip::ZipArchive::new(TrackedReader {
        input: &mut archive,
        failed: &read_failed,
    })
    .map_err(zip_failure)?;
    // Preserve the existing unique-name, root executable, and size restrictions.
    if zip.len() != entries {
        return Err(invalid("The update archive contains duplicate file names"));
    }
    let mut index = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(zip_failure)?;
        if entry.name() == "AltTabio.exe" {
            if index.is_some()
                || entry.size() == 0
                || entry.size() > MAX_EXE
                || entry.is_dir()
                || entry.is_symlink()
            {
                return Err(invalid(
                    "The archive has no unique Windows executable of an allowed size",
                ));
            }
            index = Some(i);
        }
    }
    let index = index.ok_or_else(|| invalid("The archive has no Windows executable"))?;
    let mut entry = zip.by_index(index).map_err(zip_failure)?;
    let expected = entry.size();
    let mut output = File::create_new(exe).map_err(storage)?;
    let mut write_failed = false;
    let copied = std::io::copy(
        &mut (&mut entry).take(MAX_EXE + 1),
        &mut TrackedWriter {
            output: &mut output,
            failed: &mut write_failed,
        },
    )
    .map_err(|error| {
        if write_failed || read_failed.get() {
            storage(error)
        } else {
            invalid(error.to_string())
        }
    })?;
    if copied != expected || copied > MAX_EXE {
        return Err(invalid("The extracted executable has the wrong size"));
    }
    output.sync_all().map_err(storage)
}

struct TrackedReader<'a> {
    input: &'a mut File,
    failed: &'a Cell<bool>,
}
impl Read for TrackedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let result = self.input.read(buffer);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}
impl Seek for TrackedReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let result = self.input.seek(position);
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}
struct TrackedWriter<'a> {
    output: &'a mut File,
    failed: &'a mut bool,
}
impl std::io::Write for TrackedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let result = self.output.write(bytes);
        if result.is_err() {
            *self.failed = true;
        }
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.output.flush();
        if result.is_err() {
            *self.failed = true;
        }
        result
    }
}

fn declared_entries(file: &mut File) -> Result<usize, Failure> {
    // Release artifacts are bounded, single-disk ZIP32 archives. Reject ZIP64 and ambiguous
    // footers rather than depending on name deduplication in the ZIP reader.
    let length = file.metadata().map_err(storage)?.len();
    let tail = length.min(65_557);
    let offset = i64::try_from(tail).map_err(|error| invalid(error.to_string()))?;
    file.seek(SeekFrom::End(-offset)).map_err(storage)?;
    let mut bytes = vec![0; usize::try_from(tail).map_err(|error| invalid(error.to_string()))?];
    file.read_exact(&mut bytes).map_err(storage)?;
    for i in (0..bytes.len().saturating_sub(21)).rev() {
        if bytes[i..].starts_with(b"PK\x05\x06") {
            let footer = &bytes[i..];
            let word = |index| u16::from_le_bytes([footer[index], footer[index + 1]]);
            if usize::from(word(20)) + 22 != footer.len() {
                continue;
            }
            if word(4) != 0 || word(6) != 0 || word(8) != word(10) || word(10) == u16::MAX {
                return Err(invalid("The update must be a single-disk ZIP32 archive"));
            }
            file.rewind().map_err(storage)?;
            return Ok(usize::from(word(10)));
        }
    }
    Err(invalid("The update has no valid ZIP footer"))
}

pub(super) fn validate_version(exe: &Path, version: Version) -> Result<(), Failure> {
    use std::os::windows::fs::OpenOptionsExt;
    // Version-resource APIs can mask sharing/access failures as missing resources. Establish
    // read access first and deny replacement while Windows reads the same file by path.
    let _read_guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0)
        .open(exe)
        .map_err(storage)?;
    let path = path_wide(exe).map_err(|detail| Failure::new(Stage::Extract, detail))?;
    // SAFETY: path is a terminated filename borrowed for the call.
    let size = unsafe {
        SetLastError(WIN32_ERROR(0));
        GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None)
    };
    if size == 0 {
        return Err(resource_failure(Error::from_thread()));
    }
    if size > 1024 * 1024 {
        return Err(invalid("The update has no bounded version resource"));
    }
    let mut bytes = vec![0_u8; size as usize];
    // SAFETY: bytes has the exact size returned by Windows and remains live for every query.
    unsafe { GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, bytes.as_mut_ptr().cast()) }
        .map_err(resource_failure)?;
    let mut translations = std::ptr::null_mut();
    let mut length = 0;
    let query = crate::win32::wide("\\VarFileInfo\\Translation");
    // SAFETY: output points into the version block, whose lifetime encloses the reads below.
    if !unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            &raw mut translations,
            &raw mut length,
        )
    }
    .as_bool()
        || length < 4
    {
        return Err(invalid("The update has no version translations"));
    }
    for offset in (0..length as usize / 4 * 4).step_by(4) {
        // SAFETY: translation entries contain two unaligned u16 values within the returned length.
        let pair = unsafe {
            std::ptr::read_unaligned(translations.cast::<u8>().add(offset).cast::<[u16; 2]>())
        };
        let prefix = format!("\\StringFileInfo\\{:04x}{:04x}\\", pair[0], pair[1]);
        if version_string(&bytes, &format!("{prefix}ProductVersion")).map_err(invalid)?
            == version.to_string()
            && version_string(&bytes, &format!("{prefix}OriginalFilename")).map_err(invalid)?
                == "AltTabio.exe"
        {
            return Ok(());
        }
    }
    Err(invalid(
        "The downloaded executable does not match the release version or application",
    ))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "maps owned Win32 errors into an owned update failure"
)]
fn resource_failure(error: Error) -> Failure {
    let invalid_resource = [
        ERROR_BAD_EXE_FORMAT,
        ERROR_RESOURCE_DATA_NOT_FOUND,
        ERROR_RESOURCE_TYPE_NOT_FOUND,
        ERROR_RESOURCE_NAME_NOT_FOUND,
        ERROR_RESOURCE_LANG_NOT_FOUND,
    ]
    .into_iter()
    .any(|code| error.code() == HRESULT::from_win32(code.0));
    if error.code().is_ok() || invalid_resource {
        invalid("The update has no valid version resource")
    } else {
        Failure::new(Stage::Extract, error.to_string())
    }
}

fn version_string(bytes: &[u8], name: &str) -> Result<String, String> {
    let query = crate::win32::wide(name);
    let mut value = std::ptr::null_mut();
    let mut length = 0;
    // SAFETY: the query borrows the valid Windows version block; value and length are outputs.
    if !unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            &raw mut value,
            &raw mut length,
        )
    }
    .as_bool()
        || length == 0
    {
        return Ok(String::new());
    }
    // SAFETY: VerQueryValueW returns length UTF-16 code units including the terminating null.
    let units = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), length as usize) };
    String::from_utf16(units.strip_suffix(&[0]).unwrap_or(units)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::WorkFolder;
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn malformed_signature_encoding_is_integrity_not_storage() {
        let work = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let version = Version::parse("9.0.0").unwrap_or_else(|| panic!("fixture version"));
        std::fs::write(
            work.path.join("download.zip"),
            include_bytes!("../../tests/fixtures/update-storage/archive.zip"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(work.path.join("download.zip.minisig"), [0xff, 0xfe, 0xfd])
            .unwrap_or_else(|error| panic!("{error}"));
        let failure = authenticate_and_extract(version, &work.path, update_signature::PUBLIC_KEY)
            .err()
            .unwrap_or_else(|| panic!("expected malformed encoding failure"));
        assert_eq!(failure.stage, Stage::Verify);
        assert!(failure.detail.contains("UTF-8"));
        assert!(!work.path.join("AltTabio.exe").exists());
    }

    #[test]
    fn version_resource_access_failure_is_storage_and_bad_identity_is_integrity() {
        use std::os::windows::fs::OpenOptionsExt;
        let work = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let version =
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|| panic!("fixture version"));
        let current = std::env::current_exe().unwrap_or_else(|error| panic!("{error}"));
        let exe = work.path.join("version-fixture.exe");
        std::fs::copy(current, &exe).unwrap_or_else(|error| panic!("{error}"));
        let exclusive = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&exe)
            .unwrap_or_else(|error| panic!("{error}"));
        let failure = validate_version(&exe, version)
            .err()
            .unwrap_or_else(|| panic!("expected sharing failure"));
        assert_eq!(failure.stage, Stage::Extract);
        drop(exclusive);
        assert_eq!(validate_version(&exe, version), Ok(()));
        let wrong = Version::parse("99.0.0").unwrap_or_else(|| panic!("fixture version"));
        let failure = validate_version(&exe, wrong)
            .err()
            .unwrap_or_else(|| panic!("expected identity failure"));
        assert_eq!(failure.stage, Stage::Verify);
        std::fs::remove_file(&exe).unwrap_or_else(|error| panic!("{error}"));
        let failure = validate_version(&exe, version)
            .err()
            .unwrap_or_else(|| panic!("expected missing file failure"));
        assert_eq!(failure.stage, Stage::Extract);
    }

    #[test]
    fn signed_archive_storage_failure_is_retryable_and_retry_reauthenticates() {
        let work = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let version = Version::parse("9.0.0").unwrap_or_else(|| panic!("fixture version"));
        let archive = include_bytes!("../../tests/fixtures/update-storage/archive.zip");
        let signature = include_str!("../../tests/fixtures/update-storage/valid.minisig");
        let public = include_str!("../../tests/fixtures/update-storage/fixture.pub");
        std::fs::write(work.path.join("download.zip"), archive)
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(work.path.join("download.zip.minisig"), signature)
            .unwrap_or_else(|error| panic!("{error}"));
        // A directory at the executable path deterministically fails File::create_new after
        // authenticating and parsing a real signed ZIP. No filesystem permissions are changed.
        std::fs::create_dir(work.path.join("AltTabio.exe"))
            .unwrap_or_else(|error| panic!("{error}"));
        let failure = authenticate_and_extract(version, &work.path, public)
            .err()
            .unwrap_or_else(|| panic!("expected storage failure"));
        assert_eq!(failure.stage, Stage::Extract);
        let view = alttabio::update_status::Status::Failed {
            stage: Some(failure.stage),
            detail: failure.detail,
        }
        .view("1.3.0", false, false);
        assert!(!view.body.contains("security checks"));
        assert_eq!(
            view.primary,
            Some((
                "Retry and restart",
                alttabio::update_status::Command::Install
            ))
        );
        // A retry must authenticate again, even while the output-path problem is still present.
        let mut tampered = archive.to_vec();
        tampered[40] ^= 1;
        std::fs::write(work.path.join("download.zip"), tampered)
            .unwrap_or_else(|error| panic!("{error}"));
        let failure = authenticate_and_extract(version, &work.path, public)
            .err()
            .unwrap_or_else(|| panic!("expected authentication failure"));
        assert_eq!(failure.stage, Stage::Verify);
        assert!(failure.detail.to_lowercase().contains("signature"));
        assert!(work.path.join("AltTabio.exe").is_dir());
    }

    #[test]
    fn unsigned_or_wrong_identity_archives_never_reach_extraction() {
        let work = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let version = Version::parse("9.0.0").unwrap_or_else(|| panic!("version"));
        let archive = include_bytes!("../../tests/fixtures/update-signature/archive.txt");
        let signature = include_str!("../../tests/fixtures/update-signature/valid.minisig");
        let public = include_str!("../../tests/fixtures/update-signature/fixture.pub");
        std::fs::write(work.path.join("download.zip"), archive)
            .unwrap_or_else(|error| panic!("{error}"));
        for signature in ["malformed", signature] {
            std::fs::write(work.path.join("download.zip.minisig"), signature)
                .unwrap_or_else(|error| panic!("{error}"));
            let error = authenticate_and_extract(version, &work.path, update_signature::PUBLIC_KEY)
                .err()
                .unwrap_or_else(|| panic!("unsigned extraction"));
            assert!(error.detail.to_lowercase().contains("signature"));
            assert!(!work.path.join("AltTabio.exe").exists());
        }
        // The valid signed text fixture gets past authentication, then fails ZIP parsing.
        let error = authenticate_and_extract(version, &work.path, public)
            .err()
            .unwrap_or_else(|| panic!("not a ZIP"));
        assert!(!error.detail.to_lowercase().contains("signature"));
        assert!(!work.path.join("AltTabio.exe").exists());
    }

    #[test]
    fn extraction_uses_only_the_exact_root_executable_and_validates_resource_identity() {
        let work = WorkFolder::temporary().unwrap_or_else(|error| panic!("{error}"));
        let current = std::env::current_exe().unwrap_or_else(|error| panic!("{error}"));
        let version =
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|| panic!("version"));
        assert_eq!(validate_version(&current, version), Ok(()));
        let wrong = Version::parse("99.0.0").unwrap_or_else(|| panic!("version"));
        assert!(validate_version(&current, wrong).is_err());
        for duplicate in [false, true] {
            let archive = work.path.join(format!("fixture-{duplicate}.zip"));
            let mut zip = zip::ZipWriter::new(
                File::create(&archive).unwrap_or_else(|error| panic!("{error}")),
            );
            for name in [
                "../escape.exe",
                "AltTabio.exe",
                if duplicate { "AltTabio.exE" } else { "README" },
            ] {
                zip.start_file(name, SimpleFileOptions::default())
                    .unwrap_or_else(|error| panic!("{error}"));
                zip.write_all(b"inert executable")
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            zip.finish().unwrap_or_else(|error| panic!("{error}"));
            if duplicate {
                let bytes = std::fs::read(&archive).unwrap_or_else(|error| panic!("{error}"));
                // Both local and central-directory names become exact duplicates.
                let bytes = bytes
                    .windows(12)
                    .enumerate()
                    .filter_map(|(index, bytes)| (bytes == b"AltTabio.exE").then_some(index + 11))
                    .collect::<Vec<_>>();
                let mut content = std::fs::read(&archive).unwrap_or_else(|error| panic!("{error}"));
                for index in bytes {
                    content[index] = b'e';
                }
                std::fs::write(&archive, content).unwrap_or_else(|error| panic!("{error}"));
            }
            let output = work.path.join(format!("extracted-{duplicate}.exe"));
            let result = extract(
                File::open(&archive).unwrap_or_else(|error| panic!("{error}")),
                &output,
            );
            assert_eq!(result.is_err(), duplicate);
            assert!(!work.path.join("escape.exe").exists());
        }
    }
}
