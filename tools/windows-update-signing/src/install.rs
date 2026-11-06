//! Read-only first-install gate, sharing the app's key and signature policy.
use crate::Result;
use alttabio::{update::Version, update_signature};
use std::{fs, io::Read, path::Path};

pub fn verify(archive: &Path, version: Version) -> Result<()> {
    // The install command cannot accept a key from a release asset or its caller.
    verify_with_key(archive, version, update_signature::PUBLIC_KEY)
}

fn verify_with_key(archive: &Path, version: Version, public: &str) -> Result<()> {
    if archive.file_name().and_then(|name| name.to_str())
        != Some(&format!("AltTabio-{version}-windows-x64.zip"))
    {
        return Err("Archive name does not match the Windows release identity".into());
    }
    let mut signature_path = archive.as_os_str().to_os_string();
    signature_path.push(".minisig");
    let mut signature = String::new();
    fs::File::open(Path::new(&signature_path))?
        .take(update_signature::MAX_SIGNATURE_BYTES + 1)
        .read_to_string(&mut signature)?;
    // Verify as bytes, without opening a ZIP parser or running anything from the archive.
    update_signature::verify(fs::File::open(archive)?, &signature, version, public)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::{KeyPair, sign};
    use std::{
        io::Cursor,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    const DATA: &str = include_str!("../../../tests/fixtures/update-signature/archive.txt");
    const SIGNATURE: &str = include_str!("../../../tests/fixtures/update-signature/valid.minisig");
    const KEY: &str = include_str!("../../../tests/fixtures/update-signature/fixture.pub");

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "alttabio-install-verify-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory)?;
            let fixture = Self(directory);
            fs::write(fixture.archive(), DATA)?;
            fs::write(fixture.signature(), SIGNATURE)?;
            Ok(fixture)
        }

        fn archive(&self) -> PathBuf {
            self.0.join("AltTabio-9.0.0-windows-x64.zip")
        }

        fn signature(&self) -> PathBuf {
            self.0.join("AltTabio-9.0.0-windows-x64.zip.minisig")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the private directory successfully created by this fixture is removed.
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("Could not remove install-verification fixture: {error}");
            }
        }
    }

    #[test]
    fn verifies_bytes_without_extracting_and_production_command_pins_app_key() -> Result<()> {
        let fixture = Fixture::new()?;
        let version = crate::version("9.0.0")?;
        // The signed fixture is deliberately not a ZIP or an executable.
        verify_with_key(&fixture.archive(), version, KEY)?;
        assert!(verify(&fixture.archive(), version).is_err());
        assert_eq!(fs::read(fixture.archive())?, DATA.as_bytes());
        assert_eq!(fs::read_to_string(fixture.signature())?, SIGNATURE);
        assert_eq!(fs::read_dir(&fixture.0)?.count(), 2);
        Ok(())
    }

    #[test]
    fn rejects_missing_invalid_oversized_and_foreign_signatures_and_changed_bytes() -> Result<()> {
        let fixture = Fixture::new()?;
        let version = crate::version("9.0.0")?;
        fs::remove_file(fixture.signature())?;
        assert!(verify_with_key(&fixture.archive(), version, KEY).is_err());
        for signature in [
            String::new(),
            "malformed".to_owned(),
            "x".repeat(4097),
            include_str!("../../../tests/fixtures/update-signature/wrong-key.minisig").to_owned(),
            SIGNATURE.replace("9.0.0", "9.0.1"),
            SIGNATURE.replacen("RU", "RW", 1),
        ] {
            fs::write(fixture.signature(), signature)?;
            assert!(verify_with_key(&fixture.archive(), version, KEY).is_err());
        }
        fs::write(fixture.signature(), SIGNATURE)?;
        fs::write(fixture.archive(), "tampered archive")?;
        assert!(verify_with_key(&fixture.archive(), version, KEY).is_err());
        Ok(())
    }

    #[test]
    fn rejects_authentic_signatures_for_another_release_identity() -> Result<()> {
        let fixture = Fixture::new()?;
        let version = crate::version("9.0.0")?;
        let pair = KeyPair::generate_unencrypted_keypair()?;
        let public = pair.pk.to_box()?.to_string();
        let identity = update_signature::identity(version);
        for comment in [
            identity.clone(),
            identity.replace("9.0.0", "9.0.1"),
            identity.replace("windows-x64", "macos"),
            identity.replace("asset=AltTabio", "asset=OtherApp"),
            identity.replace("AltTabio windows update v1", "OtherApp windows update v1"),
        ] {
            let signature = sign(
                Some(&pair.pk),
                &pair.sk,
                Cursor::new(DATA),
                Some(&comment),
                None,
            )?;
            fs::write(fixture.signature(), signature.to_string())?;
            let result = verify_with_key(&fixture.archive(), version, &public);
            assert_eq!(result.is_ok(), comment == identity);
        }
        // A valid sidecar cannot authorize a differently named local archive.
        let renamed = fixture.0.join("OtherApp.zip");
        fs::rename(fixture.archive(), &renamed)?;
        assert!(verify_with_key(&renamed, version, &public).is_err());
        Ok(())
    }
}
