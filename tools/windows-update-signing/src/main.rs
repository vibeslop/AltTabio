//! Offline release preparation. Never contacts GitHub or publishes anything.
use alttabio::{update::Version, update_signature};
use minisign::{KeyPair, PublicKey, SecretKey, SecretKeyBox};
use std::{env, error::Error, fs, io::Cursor, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("Windows update signing failed: {error}");
        std::process::exit(1);
    }
}

fn required_secret(name: &str) -> Result<String> {
    let value = env::var(name).map_err(|_| format!("Missing {name}"))?;
    if value.trim().is_empty() {
        return Err(format!("Missing {name}").into());
    }
    Ok(value)
}

fn version(text: &str) -> Result<Version> {
    let version = Version::parse(text).ok_or("Invalid release version")?;
    if text != version.to_string() {
        return Err("Release version must be canonical, without a v prefix".into());
    }
    Ok(version)
}

fn sign(archive: &Path, version: Version, key: &SecretKey, public: &str) -> Result<String> {
    let public_key = PublicKey::from_box(minisign::PublicKeyBox::from_string(public)?)?;
    let data = fs::read(archive)?;
    let signature = minisign::sign(
        Some(&public_key),
        key,
        Cursor::new(&data),
        Some(&update_signature::identity(version)),
        None,
    )?
    .to_string();
    // This uses the app's verifier, including the complete signed identity check, and
    // catches secrets that do not match the pinned key before writing any metadata.
    update_signature::verify(Cursor::new(data), &signature, version, public)?;
    Ok(signature)
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", directory] => {
            // The caller must create a private directory outside the checkout first.
            let directory = Path::new(directory);
            let public = directory.join("windows-update.pub");
            let secret = directory.join("windows-update.key");
            let mut public_file = fs::OpenOptions::new().write(true).create_new(true).open(public)?;
            let mut secret_file = fs::OpenOptions::new().write(true).create_new(true).open(secret)?;
            KeyPair::generate_and_write_encrypted_keypair(
                &mut public_file, &mut secret_file,
                Some("AltTabio Windows updates only"),
                Some(required_secret("WINDOWS_UPDATE_SIGNING_PASSWORD")?),
            )?;
        }
        ["sign", archive, release_version] => {
            let version = version(release_version)?;
            let path = Path::new(archive);
            if path.file_name().and_then(|name| name.to_str()) != Some(&format!("AltTabio-{version}-windows-x64.zip")) {
                return Err("Archive name does not match the Windows release identity".into());
            }
            let secret = SecretKey::from_box(
                SecretKeyBox::from_string(&required_secret("WINDOWS_UPDATE_SIGNING_KEY")?)?,
                Some(required_secret("WINDOWS_UPDATE_SIGNING_PASSWORD")?),
            )?;
            let signature = sign(path, version, &secret, update_signature::PUBLIC_KEY)?;
            fs::write(format!("{archive}.minisig"), signature)?;
        }
        ["verify", archive, signature, release_version, public] => {
            update_signature::verify(fs::File::open(archive)?, &fs::read_to_string(signature)?, version(release_version)?, &fs::read_to_string(public)?)?;
        }
        ["fixtures", archive, release_version, directory] => {
            // Ephemeral test keys are unrelated to production. Never persist private keys.
            let version = version(release_version)?;
            let pair = KeyPair::generate_unencrypted_keypair()?;
            let public = pair.pk.to_box()?.to_string();
            let directory = Path::new(directory);
            fs::write(directory.join("fixture.pub"), &public)?;
            fs::write(directory.join("valid.minisig"), sign(Path::new(archive), version, &pair.sk, &public)?)?;
            let other = KeyPair::generate_unencrypted_keypair()?;
            fs::write(directory.join("wrong-key.minisig"), sign(Path::new(archive), version, &other.sk, &other.pk.to_box()?.to_string())?)?;
        }
        _ => return Err("Usage: windows-update-signing keygen PRIVATE_DIRECTORY | sign ARCHIVE VERSION | verify ARCHIVE SIGNATURE VERSION PUBLIC_KEY | fixtures ARCHIVE VERSION DIRECTORY".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_noncanonical_release_names() {
        for text in ["v1.2.3", "01.2.3", "1.2.3-beta", "1.2", "1.2.3;malicious"] {
            assert!(version(text).is_err());
        }
        assert!(version("1.2.3").is_ok());
    }

    #[test]
    fn valid_signatures_with_wrong_release_identity_are_rejected() -> Result<()> {
        let pair = KeyPair::generate_unencrypted_keypair()?;
        let public = pair.pk.to_box()?.to_string();
        let version = version("9.0.0")?;
        let data = b"inert fixture";
        for comment in [
            update_signature::identity(version),
            update_signature::identity(version).replace("windows-x64", "macos"),
            update_signature::identity(version).replace("asset=AltTabio", "asset=OtherApp"),
        ] {
            let signature = minisign::sign(
                Some(&pair.pk),
                &pair.sk,
                Cursor::new(data),
                Some(&comment),
                None,
            )?
            .to_string();
            let result = update_signature::verify(Cursor::new(data), &signature, version, &public);
            if comment == update_signature::identity(version) {
                assert!(result.is_ok());
                assert!(
                    update_signature::verify(
                        Cursor::new(data),
                        &signature,
                        version,
                        update_signature::PUBLIC_KEY
                    )
                    .is_err()
                );
            } else {
                assert!(result.is_err_and(|error| error.contains("identity")));
            }
        }
        let other = KeyPair::generate_unencrypted_keypair()?;
        assert!(minisign::sign(Some(&pair.pk), &other.sk, Cursor::new(data), None, None).is_err());
        Ok(())
    }
}
