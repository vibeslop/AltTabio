//! Windows archive authentication. Keys delivered by a release are never trusted.
use crate::update::Version;
use minisign_verify::{PublicKey, Signature};
use std::io::{self, Read};

pub const PUBLIC_KEY: &str = include_str!("../scripts/windows/update-signing.pub");
pub const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_SIGNATURE_BYTES: u64 = 4096;

/// The Minisign trusted comment binds the archive to this application and release identity.
#[must_use]
pub fn identity(version: Version) -> String {
    format!(
        "AltTabio windows update v1; version={version}; platform=windows-x64; asset=AltTabio-{version}-windows-x64.zip"
    )
}

/// Authenticates a bounded archive stream and its identity before a caller may extract it.
/// `public_key` must come from the executable's trust configuration, never the release API.
///
/// # Errors
/// Returns an error for unreadable, oversized, malformed, legacy, or unauthentic data.
pub fn verify(
    archive: impl Read,
    signature: &str,
    version: Version,
    public_key: &str,
) -> Result<(), String> {
    if signature.len() as u64 > MAX_SIGNATURE_BYTES {
        return Err("The update signature is too large".to_owned());
    }
    let lines: Vec<_> = signature.lines().collect();
    if lines.len() != 4
        || !lines[0].starts_with("untrusted comment: ")
        || !lines[2].starts_with("trusted comment: ")
    {
        return Err("The update signature has an invalid Minisign envelope".to_owned());
    }
    let key = PublicKey::decode(public_key)
        .map_err(|error| format!("Invalid embedded update key: {error}"))?;
    let signature = Signature::decode(signature)
        .map_err(|error| format!("Invalid update signature: {error}"))?;
    // Streaming verification accepts only the current prehashed Minisign format and also
    // verifies the global signature over the trusted comment. No cryptography lives here.
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|error| format!("Invalid update signature: {error}"))?;
    let mut bounded = archive.take(MAX_ARCHIVE_BYTES + 1);
    let length = io::copy(&mut bounded, &mut SignatureWriter(&mut verifier))
        .map_err(|error| format!("Could not read the update archive: {error}"))?;
    if length == 0 || length > MAX_ARCHIVE_BYTES {
        return Err("The update archive is too large or empty".to_owned());
    }
    verifier
        .finalize()
        .map_err(|error| format!("Update signature verification failed: {error}"))?;
    if signature.trusted_comment() != identity(version) {
        return Err("The signed update identity does not match this release".to_owned());
    }
    Ok(())
}

struct SignatureWriter<'a, 'b>(&'a mut minisign_verify::StreamVerifier<'b>);

impl io::Write for SignatureWriter<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const KEY: &str = include_str!("../tests/fixtures/update-signature/fixture.pub");
    const ARCHIVE: &str = include_str!("../tests/fixtures/update-signature/archive.txt");
    const SIGNATURE: &str = include_str!("../tests/fixtures/update-signature/valid.minisig");
    const WRONG_KEY: &str = include_str!("../tests/fixtures/update-signature/wrong-key.minisig");

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("valid test version"))
    }

    #[test]
    fn authenticates_complete_archive_and_signed_identity() {
        assert_eq!(
            verify(Cursor::new(ARCHIVE), SIGNATURE, version("9.0.0"), KEY),
            Ok(())
        );
        assert!(verify(Cursor::new(ARCHIVE), SIGNATURE, version("9.0.1"), KEY).is_err());
        for (old, new) in [
            ("windows-x64", "macos"),
            ("AltTabio-9", "OtherApp-9"),
            ("update v1", "update v2"),
        ] {
            assert!(
                verify(
                    Cursor::new(ARCHIVE),
                    &SIGNATURE.replace(old, new),
                    version("9.0.0"),
                    KEY
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_tampering_missing_malformed_wrong_keys_and_legacy_signatures() {
        for signature in [
            String::new(),
            "malformed".to_owned(),
            SIGNATURE.lines().take(3).collect::<Vec<_>>().join("\n"),
            WRONG_KEY.to_owned(),
            SIGNATURE.replacen("RU", "RW", 1),
            "x".repeat(4097),
        ] {
            assert!(verify(Cursor::new(ARCHIVE), &signature, version("9.0.0"), KEY).is_err());
        }
        for content in [
            "",
            "inert Windows update test archivE",
            "inert Windows update test archive appended",
        ] {
            assert!(verify(Cursor::new(content), SIGNATURE, version("9.0.0"), KEY).is_err());
        }
        // A valid test signature is not trusted by the production pin.
        assert!(
            verify(
                Cursor::new(ARCHIVE),
                SIGNATURE,
                version("9.0.0"),
                PUBLIC_KEY
            )
            .is_err()
        );
        assert!(PublicKey::decode(PUBLIC_KEY).is_ok());
    }

    #[test]
    fn signed_comments_cannot_be_substituted_even_with_a_valid_file_signature() {
        let forged = SIGNATURE.replace("9.0.0", "9.0.1");
        assert_eq!(
            Signature::decode(&forged)
                .unwrap_or_else(|_| panic!("valid forged test envelope"))
                .trusted_comment(),
            identity(version("9.0.1"))
        );
        assert!(verify(Cursor::new(ARCHIVE), &forged, version("9.0.1"), KEY).is_err());
    }

    #[test]
    fn enforces_size_and_propagates_read_failures() {
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture read failure"))
            }
        }
        assert!(
            verify(
                io::repeat(1).take(MAX_ARCHIVE_BYTES + 1),
                SIGNATURE,
                version("9.0.0"),
                KEY
            )
            .is_err()
        );
        assert!(
            verify(FailedRead, SIGNATURE, version("9.0.0"), KEY)
                .is_err_and(|error| error.contains("fixture read failure"))
        );
    }
}
