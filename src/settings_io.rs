use alttabio::settings::{Settings, SettingsDocument};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMPORARY_FILE: AtomicU64 = AtomicU64::new(0);

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
        self.save_with(settings, |file, contents| {
            file.write_all(contents)?;
            file.sync_all()
        })
    }

    fn save_with(
        &mut self,
        settings: &Settings,
        write_and_sync: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> Result<(), String> {
        let rendered = self.document.render(settings);
        let save_error = |error| {
            format!(
                "Could not save settings to {}: {error}",
                self.path.display()
            )
        };
        let (temporary_path, mut file) =
            create_temporary_sibling(&self.path).map_err(save_error)?;
        let written = write_and_sync(&mut file, rendered.as_bytes());
        // Close before renaming or removing: the temporary file denies sharing while being written.
        drop(file);
        let result = written.and_then(|()| {
            // A sibling stays on the same filesystem. Rename replaces the destination without
            // truncating it, and happens only after every byte has been written and synced.
            std::fs::rename(&temporary_path, &self.path)
        });
        if let Err(error) = result {
            let message = save_error(error);
            return Err(match std::fs::remove_file(&temporary_path) {
                Ok(()) => message,
                Err(cleanup_error) => format!(
                    "{message}; could not remove temporary settings file {}: {cleanup_error}",
                    temporary_path.display()
                ),
            });
        }
        self.document = SettingsDocument::parse(&rendered);
        Ok(())
    }
}

fn create_temporary_sibling(path: &Path) -> io::Result<(PathBuf, File)> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Settings path has no file name",
        )
    })?;
    for _ in 0..128 {
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary_path = path.with_file_name(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not reserve a temporary settings file after 128 attempts",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "alttabio-settings-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path)?;
            Ok(Self(path))
        }

        fn store(&self) -> io::Result<SettingsStore> {
            let path = self.0.join("AltTabio.ini");
            let contents = "\u{feff}[General]\nReplaceAltTab=false\nFutureSetting=keep\n";
            std::fs::write(&path, contents)?;
            Ok(SettingsStore {
                path,
                document: SettingsDocument::parse(contents),
            })
        }

        fn assert_only_settings_remain(&self) -> io::Result<()> {
            let entries = std::fs::read_dir(&self.0)?.collect::<io::Result<Vec<_>>>()?;
            assert_eq!(
                entries.len(),
                1,
                "temporary settings file was not cleaned up"
            );
            assert_eq!(entries[0].file_name(), "AltTabio.ini");
            Ok(())
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("Could not remove settings test directory: {error}");
            }
        }
    }

    #[test]
    fn failed_partial_write_preserves_previous_file_and_cleans_temporary_sibling()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = directory.store()?;
        let previous = std::fs::read(&store.path)?;
        let document = store.document.clone();
        let result = store.save_with(&Settings::default(), |file, contents| {
            file.write_all(&contents[..contents.len() / 2])?;
            Err(io::Error::other("injected disk full during write"))
        });

        assert!(result.is_err_and(|error| error.contains("injected disk full during write")));
        assert_eq!(std::fs::read(&store.path)?, previous);
        assert_eq!(store.document, document);
        directory.assert_only_settings_remain()?;
        Ok(())
    }

    #[test]
    fn failed_sync_preserves_previous_file_and_cleans_temporary_sibling()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = directory.store()?;
        let previous = std::fs::read(&store.path)?;
        let document = store.document.clone();
        let result = store.save_with(&Settings::default(), |file, contents| {
            file.write_all(contents)?;
            Err(io::Error::other("injected sync failure"))
        });

        assert!(result.is_err_and(|error| error.contains("injected sync failure")));
        assert_eq!(std::fs::read(&store.path)?, previous);
        assert_eq!(store.document, document);
        directory.assert_only_settings_remain()?;
        Ok(())
    }

    #[test]
    fn locked_destination_preserves_previous_file_and_cleans_temporary_sibling()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = directory.store()?;
        let previous = std::fs::read(&store.path)?;
        let document = store.document.clone();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&store.path)?;

        let result = store.save(&Settings::default());

        assert!(result.is_err_and(|error| {
            error.contains("Could not save settings")
                && !error.contains("could not remove temporary settings file")
        }));
        drop(locked);
        assert_eq!(std::fs::read(&store.path)?, previous);
        assert_eq!(store.document, document);
        directory.assert_only_settings_remain()?;
        Ok(())
    }

    #[test]
    fn cleanup_failure_is_reported_together_with_write_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = directory.store()?;
        let previous = std::fs::read(&store.path)?;
        let document = store.document.clone();
        let mut locked_temporary_file = None;
        let result = store.save_with(&Settings::default(), |file, _| {
            // A duplicate keeps the real no-sharing handle alive through the cleanup attempt.
            locked_temporary_file = Some(file.try_clone()?);
            Err(io::Error::other("injected write failure"))
        });

        assert!(result.is_err_and(|error| {
            error.contains("injected write failure")
                && error.contains("could not remove temporary settings file")
        }));
        assert_eq!(std::fs::read(&store.path)?, previous);
        assert_eq!(store.document, document);
        drop(locked_temporary_file);
        let entries = std::fs::read_dir(&directory.0)?.collect::<io::Result<Vec<_>>>()?;
        assert_eq!(entries.len(), 2);
        for entry in entries {
            if entry.path() != store.path {
                std::fs::remove_file(entry.path())?;
            }
        }
        directory.assert_only_settings_remain()?;
        Ok(())
    }

    #[test]
    fn saving_bom_settings_replaces_existing_file_and_preserves_unknown_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = directory.store()?;
        let mut settings = store.document.settings();
        assert!(!settings.general.replace_alt_tab);
        settings.general.autostart = false;
        let expected = store.document.render(&settings);

        store.save(&settings)?;

        let contents = std::fs::read_to_string(&store.path)?;
        assert_eq!(contents, expected);
        assert!(contents.contains("[General]\nFutureSetting=keep"));
        let reloaded = SettingsDocument::parse(&contents);
        assert_eq!(reloaded.settings(), settings);
        assert_eq!(store.document, reloaded);
        directory.assert_only_settings_remain()?;
        Ok(())
    }

    #[test]
    fn first_save_creates_a_complete_settings_file() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let mut store = SettingsStore {
            path: directory.0.join("AltTabio.ini"),
            document: SettingsDocument::default(),
        };
        let settings = Settings::default();
        let expected = store.document.render(&settings);

        store.save(&settings)?;

        assert_eq!(std::fs::read_to_string(&store.path)?, expected);
        assert_eq!(store.document.settings(), settings);
        directory.assert_only_settings_remain()?;
        Ok(())
    }
}
