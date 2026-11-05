//! One writer owns the document and preserves the order of settings changes.

use crate::settings::Settings;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

/// Serializes settings writes without blocking the application thread.
pub struct SettingsWriter {
    sender: Sender<Settings>,
    thread: JoinHandle<()>,
}

impl SettingsWriter {
    /// Starts the sole owner of the settings document and reports each save's result.
    ///
    /// # Errors
    /// Returns an error if the writer thread cannot start.
    pub fn spawn(
        mut save: impl FnMut(&Settings) -> Result<(), String> + Send + 'static,
        mut completed: impl FnMut(Result<(), String>) + Send + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("alttabio-settings".to_owned())
            .spawn(move || {
                while let Ok(settings) = receiver.recv() {
                    completed(save(&settings));
                }
            })
            .map_err(|error| format!("Could not start the settings writer: {error}"))?;
        Ok(Self { sender, thread })
    }

    /// Queues a snapshot without waiting for storage.
    ///
    /// # Errors
    /// Returns an error if the writer is no longer running.
    pub fn save(&self, settings: Settings) -> Result<(), String> {
        self.sender
            .send(settings)
            .map_err(|_| "The settings writer stopped before saving the change".to_owned())
    }

    /// Called outside the app-state borrow at exit, after accepting the last settings change.
    ///
    /// # Errors
    /// Returns an error if the writer panicked before it could drain the pending saves.
    pub fn finish(self) -> Result<(), String> {
        drop(self.sender);
        self.thread
            .join()
            .map_err(|_| "The settings writer failed before shutdown completed".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn slow_save_does_not_block_requests_and_shutdown_drains_them_in_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let mut first = true;
        let mut saved = Vec::new();
        let writer = SettingsWriter::spawn(
            move |settings| {
                if first {
                    first = false;
                    started.send(()).map_err(|error| error.to_string())?;
                    release_rx.recv().map_err(|error| error.to_string())?;
                }
                saved.push(settings.general.auto_update);
                finished
                    .send(saved.clone())
                    .map_err(|error| error.to_string())
            },
            |result| assert!(result.is_ok()),
        )?;
        let mut settings = Settings::macos_default();
        writer.save(settings.clone())?;
        started_rx.recv_timeout(Duration::from_secs(5))?;
        settings.general.auto_update = false;
        writer.save(settings.clone())?;
        settings.general.auto_update = true;
        writer.save(settings)?;
        // The first save is still blocked while both subsequent requests return.
        assert!(finished_rx.try_recv().is_err());
        release.send(())?;
        writer.finish()?;
        assert_eq!(finished_rx.try_iter().last(), Some(vec![true, false, true]));
        Ok(())
    }

    #[test]
    fn failed_save_is_reported_and_does_not_stop_subsequent_saves()
    -> Result<(), Box<dyn std::error::Error>> {
        let (completed, results) = mpsc::channel();
        let mut fail = true;
        let writer = SettingsWriter::spawn(
            move |_| {
                if std::mem::take(&mut fail) {
                    Err("injected write failure".to_owned())
                } else {
                    Ok(())
                }
            },
            move |result| {
                assert!(completed.send(result).is_ok());
            },
        )?;
        writer.save(Settings::macos_default())?;
        writer.save(Settings::macos_default())?;
        writer.finish()?;
        assert_eq!(results.recv()?, Err("injected write failure".to_owned()));
        assert_eq!(results.recv()?, Ok(()));
        Ok(())
    }
}
