use minisign::KeyPair;
use std::{error::Error, fs, process::Command};

#[test]
fn production_signing_fails_closed_without_matching_secrets() -> Result<(), Box<dyn Error>> {
    let directory =
        std::env::temp_dir().join(format!("alttabio-signing-cli-{}", std::process::id()));
    fs::create_dir(&directory)?;
    let archive = directory.join("AltTabio-9.0.0-windows-x64.zip");
    let signature = directory.join("AltTabio-9.0.0-windows-x64.zip.minisig");
    fs::write(&archive, b"inert CLI fixture")?;
    // This disposable test pair never leaves memory and cannot sign production updates.
    let password = "disposable test password";
    let pair = KeyPair::generate_encrypted_keypair(Some(password.to_owned()))?;
    let key = pair.sk.to_box(None)?.to_string();
    for (secret, password) in [
        (None, None),
        (Some(key.as_str()), None),
        (Some(""), Some("")),
        (Some("malformed"), Some(password)),
        (Some(key.as_str()), Some("wrong password")),
        (Some(key.as_str()), Some(password)),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_windows-update-signing"));
        command
            .args(["sign"])
            .arg(&archive)
            .arg("9.0.0")
            .env_remove("WINDOWS_UPDATE_SIGNING_KEY")
            .env_remove("WINDOWS_UPDATE_SIGNING_PASSWORD");
        if let Some(secret) = secret {
            command.env("WINDOWS_UPDATE_SIGNING_KEY", secret);
        }
        if let Some(password) = password {
            command.env("WINDOWS_UPDATE_SIGNING_PASSWORD", password);
        }
        let output = command.output()?;
        assert!(!output.status.success());
        assert!(!signature.exists());
        assert!(output.stdout.is_empty());
    }
    fs::remove_file(archive)?;
    fs::remove_dir(directory)?;
    Ok(())
}
