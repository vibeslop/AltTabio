use alttabio::settings::{Settings, SettingsDocument};
use std::path::PathBuf;

pub struct SettingsStore {
    path: PathBuf,
    document: SettingsDocument,
}

impl SettingsStore {
    pub fn load_adjacent() -> Result<(Self, Settings), String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        let path = executable.with_file_name("AltTabio.ini");
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(format!(
                    "Could not read settings from {}: {error}",
                    path.display()
                ));
            }
        };
        let document = SettingsDocument::parse(&contents);
        let settings = document.settings();
        Ok((Self { path, document }, settings))
    }

    pub fn save(&mut self, settings: &Settings) -> Result<(), String> {
        let rendered = self.document.render(settings);
        std::fs::write(&self.path, &rendered).map_err(|error| {
            format!(
                "Could not save settings to {}: {error}",
                self.path.display()
            )
        })?;
        self.document = SettingsDocument::parse(&rendered);
        Ok(())
    }
}
