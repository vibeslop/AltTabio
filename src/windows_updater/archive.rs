//! Authenticate every archive byte before opening ZIP metadata or extracting anything.
use super::native::{path_wide, sha256};
use alttabio::{update::Version, update_signature};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::PCWSTR;

pub(super) const MAX_EXE: u64 = 50 * 1024 * 1024;

pub(super) fn authenticate_and_extract(
    version: Version,
    work: &Path,
    public_key: &str,
) -> Result<String, String> {
    let mut signature = String::new();
    File::open(work.join("download.zip.minisig"))
        .and_then(|file| {
            file.take(update_signature::MAX_SIGNATURE_BYTES + 1)
                .read_to_string(&mut signature)
        })
        .map_err(|error| error.to_string())?;
    // Keep the same file handle across verification and extraction. The protected staging ACL
    // and read sharing prevent an unprivileged process from substituting the archive.
    let mut archive = File::open(work.join("download.zip")).map_err(|error| error.to_string())?;
    update_signature::verify(&mut archive, &signature, version, public_key)?;
    archive.rewind().map_err(|error| error.to_string())?;
    let exe = work.join("AltTabio.exe");
    extract(archive, &exe)?;
    validate_version(&exe, version)?;
    sha256(&exe)
}

fn extract(archive: File, exe: &Path) -> Result<(), String> {
    let mut archive = archive;
    let entries = declared_entries(&mut archive)?;
    let mut zip = zip::ZipArchive::new(archive).map_err(|error| error.to_string())?;
    // ZipArchive indexes by name and collapses duplicate central-directory entries.
    // Compare its unique count with the ZIP footer before accepting any executable.
    if zip.len() != entries {
        return Err("The update archive contains duplicate file names".to_owned());
    }
    let mut index = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|error| error.to_string())?;
        if entry.name() == "AltTabio.exe" {
            if index.is_some()
                || entry.size() == 0
                || entry.size() > MAX_EXE
                || entry.is_dir()
                || entry.is_symlink()
            {
                return Err(
                    "The archive has no unique Windows executable of an allowed size".to_owned(),
                );
            }
            index = Some(i);
        }
    }
    let index = index.ok_or("The archive has no Windows executable")?;
    let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
    let expected = entry.size();
    let mut output = File::create_new(exe).map_err(|error| error.to_string())?;
    let copied = std::io::copy(&mut (&mut entry).take(MAX_EXE + 1), &mut output)
        .map_err(|error| error.to_string())?;
    if copied != expected || copied > MAX_EXE {
        return Err("The extracted executable has the wrong size".to_owned());
    }
    output.sync_all().map_err(|error| error.to_string())
}

fn declared_entries(file: &mut File) -> Result<usize, String> {
    // Release artifacts are bounded, single-disk ZIP32 archives. Reject ZIP64 and ambiguous
    // footers rather than depending on name deduplication in the ZIP reader.
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    let tail = length.min(65_557);
    let offset = i64::try_from(tail).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::End(-offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = vec![0; usize::try_from(tail).map_err(|error| error.to_string())?];
    file.read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    for i in (0..bytes.len().saturating_sub(21)).rev() {
        if bytes[i..].starts_with(b"PK\x05\x06") {
            let footer = &bytes[i..];
            let word = |index| u16::from_le_bytes([footer[index], footer[index + 1]]);
            if usize::from(word(20)) + 22 != footer.len() {
                continue;
            }
            if word(4) != 0 || word(6) != 0 || word(8) != word(10) || word(10) == u16::MAX {
                return Err("The update must be a single-disk ZIP32 archive".to_owned());
            }
            file.rewind().map_err(|error| error.to_string())?;
            return Ok(usize::from(word(10)));
        }
    }
    Err("The update has no valid ZIP footer".to_owned())
}

pub(super) fn validate_version(exe: &Path, version: Version) -> Result<(), String> {
    let path = path_wide(exe)?;
    // SAFETY: path is a terminated filename borrowed for the call.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None) };
    if size == 0 || size > 1024 * 1024 {
        return Err("The update has no bounded version resource".to_owned());
    }
    let mut bytes = vec![0_u8; size as usize];
    // SAFETY: bytes has the exact size returned by Windows and remains live for every query.
    unsafe { GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, bytes.as_mut_ptr().cast()) }
        .map_err(|error| error.to_string())?;
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
        return Err("The update has no version translations".to_owned());
    }
    for offset in (0..length as usize / 4 * 4).step_by(4) {
        // SAFETY: translation entries contain two unaligned u16 values within the returned length.
        let pair = unsafe {
            std::ptr::read_unaligned(translations.cast::<u8>().add(offset).cast::<[u16; 2]>())
        };
        let prefix = format!("\\StringFileInfo\\{:04x}{:04x}\\", pair[0], pair[1]);
        if version_string(&bytes, &format!("{prefix}ProductVersion"))? == version.to_string()
            && version_string(&bytes, &format!("{prefix}OriginalFilename"))? == "AltTabio.exe"
        {
            return Ok(());
        }
    }
    Err("The downloaded executable does not match the release version or application".to_owned())
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
            assert!(error.to_lowercase().contains("signature"));
            assert!(!work.path.join("AltTabio.exe").exists());
        }
        // The valid signed text fixture gets past authentication, then fails ZIP parsing.
        let error = authenticate_and_extract(version, &work.path, public)
            .err()
            .unwrap_or_else(|| panic!("not a ZIP"));
        assert!(!error.to_lowercase().contains("signature"));
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
