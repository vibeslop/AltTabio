//! Presentation of update work, independent of native windows and worker ownership.
use crate::localization::{message, tr};
use crate::update::{Update, Version};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Download,
    Verify,
    Extract,
    Prepare,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub stage: Stage,
    pub detail: String,
}

impl Failure {
    #[must_use]
    pub fn new(stage: Stage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Check,
    Install,
    Restart,
    Releases,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Checking,
    Current(SystemTime),
    Available(Update),
    Downloading {
        received: u64,
        total: u64,
    },
    Verifying,
    Preparing,
    Ready(Version),
    Restarting,
    Failed {
        stage: Option<Stage>,
        detail: String,
    },
    Blocked {
        version: Version,
        detail: String,
    },
    LocalBuild,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub title: String,
    pub body: String,
    pub note: String,
    pub detail: String,
    /// None hides progress; Some(None) is indeterminate.
    pub progress: Option<Option<u32>>,
    pub primary: Option<(&'static str, Command)>,
    pub close: &'static str,
}

impl Status {
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive catalogue keeps each update state's copy and actions together"
    )]
    pub fn view(&self, version: &str, automatic: bool, approved: bool) -> View {
        let mut view = View {
            title: String::new(),
            body: String::new(),
            note: String::new(),
            detail: String::new(),
            progress: None,
            primary: None,
            close: "Close",
        };
        let restart_note = if approved {
            "AltTabio will restart when the update is ready."
        } else if automatic {
            "Automatic updates are on. AltTabio will restart when you’re away."
        } else {
            "Restart AltTabio when you’re ready to finish the update."
        };
        match self {
            Self::Checking => {
                view.title = "Checking for updates…".into();
                view.body = "Looking for a newer version of AltTabio.".into();
                view.note = message("Installed version {version}", &[("version", version)]);
                view.progress = Some(None);
            }
            Self::Current(checked) => {
                view.title = "You’re up to date".into();
                view.body = message(
                    "AltTabio {version} is the latest version.",
                    &[("version", version)],
                );
                let minutes = checked.elapsed().unwrap_or_default().as_secs() / 60;
                view.note = if minutes == 0 {
                    "Checked just now".into()
                } else {
                    message(
                        "Checked {minutes} min ago",
                        &[("minutes", &minutes.to_string())],
                    )
                };
                view.primary = Some(("Check again", Command::Check));
            }
            Self::Available(update) => {
                view.title = message(
                    "AltTabio {version} is available",
                    &[("version", &update.version.to_string())],
                );
                view.body = message(
                    "You have version {version}. Installing this update will restart AltTabio.",
                    &[("version", version)],
                );
                view.note = "Your settings will be kept.".into();
                view.primary = Some(("Install and restart", Command::Install));
                view.close = "Later";
            }
            Self::Downloading { received, total } => {
                view.title = "Downloading update…".into();
                view.body = restart_note.into();
                view.note = if *total == 0 {
                    "You can hide this window. The update will continue.".into()
                } else {
                    message(
                        "{received} MB of {total} MB  ({percent}%)",
                        &[
                            ("received", &format!("{:.1}", megabytes(*received))),
                            ("total", &format!("{:.1}", megabytes(*total))),
                            ("percent", &percentage(*received, *total).to_string()),
                        ],
                    )
                };
                view.progress = Some((*total != 0).then(|| percentage(*received, *total)));
                view.close = "Hide";
            }
            Self::Verifying => {
                view.title = "Verifying update…".into();
                view.body = "Checking the download before installing it.".into();
                view.note = restart_note.into();
                view.progress = Some(None);
                view.close = "Hide";
            }
            Self::Preparing => {
                view.title = "Preparing update…".into();
                view.body = "Getting ready to restart AltTabio.".into();
                view.note = "You can hide this window. The update will continue.".into();
                view.progress = Some(None);
                view.close = "Hide";
            }
            Self::Ready(ready) => {
                view.title = "Update ready to install".into();
                view.body = message(
                    "AltTabio {version} has been downloaded and verified.",
                    &[("version", &ready.to_string())],
                );
                view.note = if automatic {
                    "If you choose Later, AltTabio will restart when you’re away."
                } else {
                    "Restart AltTabio to finish installing it. Your settings will be kept."
                }
                .into();
                view.primary = Some(("Restart now", Command::Restart));
                view.close = "Later";
            }
            Self::Restarting => {
                view.title = "Restarting AltTabio…".into();
                view.body = "The update is ready. AltTabio will reopen shortly.".into();
                view.progress = Some(None);
                view.close = "Hide";
            }
            Self::Failed { stage, detail } => {
                view.title = match stage {
                    None => "Couldn’t check for updates",
                    Some(Stage::Download) => "Couldn’t download update",
                    Some(Stage::Verify) => "Couldn’t verify the update",
                    Some(Stage::Extract) => "Couldn’t prepare the update",
                    Some(Stage::Prepare) => "Couldn’t restart AltTabio",
                }
                .into();
                view.body = match stage {
                    None => "The update check didn’t finish. Try again in a moment.",
                    Some(Stage::Download) => "The download didn’t finish. You can try again.",
                    Some(Stage::Verify) => {
                        "The download didn’t pass the security checks. The update wasn’t installed."
                    }
                    Some(Stage::Extract) => "There was a problem reading or saving the update files. Check disk space and folder permissions, then try again.",
                    Some(Stage::Prepare) => {
                        "AltTabio couldn’t prepare the restart. You can try again."
                    }
                }
                .into();
                view.note = message(
                    "You’re still on version {version}.",
                    &[("version", version)],
                );
                view.detail.clone_from(detail);
                view.primary = Some(match stage {
                    None => ("Try again", Command::Check),
                    Some(Stage::Download | Stage::Extract) => {
                        ("Retry and restart", Command::Install)
                    }
                    Some(Stage::Verify) => ("View releases", Command::Releases),
                    Some(Stage::Prepare) => ("Retry restart", Command::Restart),
                });
            }
            Self::Blocked { version, detail } => {
                view.title = "Can’t install this update".into();
                view.body = message(
                    "AltTabio {version} is available, but this folder allows other apps to replace AltTabio.",
                    &[("version", &version.to_string())],
                );
                // Permission-query failures must not be misrepresented as a writable folder.
                if !detail
                    .starts_with("Updates are disabled because non-elevated processes can replace")
                {
                    view.body = message(
                        "AltTabio {version} is available, but the installation folder could not be checked.",
                        &[("version", &version.to_string())],
                    );
                }
                view.note =
                    "Install the release in a folder only administrators can change.".into();
                view.detail.clone_from(detail);
                view.primary = Some(("View releases", Command::Releases));
            }
            Self::LocalBuild => {
                view.title = "You’re using a local build".into();
                view.body = "This copy doesn’t update itself from published releases.".into();
                view.note = "Install a published release to use automatic updates.".into();
                view.primary = Some(("View releases", Command::Releases));
            }
        }
        view.title = tr(&view.title).to_owned();
        view.body = tr(&view.body).to_owned();
        view.note = tr(&view.note).to_owned();
        view.primary = view.primary.map(|(label, command)| (tr(label), command));
        view.close = tr(view.close);
        view
    }

    #[must_use]
    pub const fn active(&self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading { .. }
                | Self::Verifying
                | Self::Preparing
                | Self::Restarting
        )
    }
}

fn percentage(received: u64, total: u64) -> u32 {
    u32::try_from(u128::from(received.min(total)) * 100 / u128::from(total.max(1))).unwrap_or(100)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display-only byte count, limited to 100 MiB by the downloader"
)]
fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{Checked, Step, Updater};

    #[test]
    fn translated_updates_preserve_commands_versions_progress_and_diagnostic_details() {
        use crate::localization::{Language, PreviewLanguage};
        let version = update().version;
        let statuses = [
            Status::Checking,
            Status::Current(SystemTime::now()),
            Status::Available(update()),
            Status::Downloading {
                received: 50,
                total: 100,
            },
            Status::Verifying,
            Status::Preparing,
            Status::Ready(version),
            Status::Restarting,
            Status::Failed {
                stage: Some(Stage::Verify),
                detail: "diagnostic {version}".into(),
            },
            Status::Blocked {
                version,
                detail: "diagnostic {version}".into(),
            },
            Status::LocalBuild,
        ];
        let preview = PreviewLanguage::new(Language::English);
        for status in statuses {
            preview.set(Language::English);
            let english = status.view("1.3.0", true, false);
            for language in [Language::Russian, Language::SimplifiedChinese] {
                preview.set(language);
                let view = status.view("1.3.0", true, false);
                assert_ne!(view.title, english.title);
                assert_ne!(view.body, english.body);
                assert_ne!(view.close, english.close);
                assert_eq!(view.detail, english.detail);
                assert_eq!(view.progress, english.progress);
                assert_eq!(
                    view.primary.map(|(_, command)| command),
                    english.primary.map(|(_, command)| command)
                );
                assert!(!view.body.contains("{version}"));
                assert!(!view.note.contains("{version}"));
            }
        }
    }

    fn update() -> Update {
        Update {
            version: Version::parse("1.3.1").unwrap_or_else(|| panic!("fixture version")),
            url: "fixture".into(),
        }
    }

    #[test]
    fn observing_background_download_does_not_approve_restart() {
        let mut policy = Updater::default();
        assert_eq!(policy.check_due(), Step::Check);
        assert!(matches!(
            policy.checked(Ok(Checked::Available(update()))),
            Step::Install(_)
        ));
        assert_eq!(policy.check_or_observe(), Step::None);
        assert_eq!(policy.installed(Ok(())), Step::None);
        assert_eq!(policy.check_or_observe(), Step::None);
    }

    #[test]
    fn observing_check_requires_an_offer_and_explicit_approval() {
        let mut policy = Updater::default();
        assert_eq!(policy.check_due(), Step::Check);
        assert_eq!(policy.check_or_observe(), Step::None);
        assert_eq!(
            policy.checked(Ok(Checked::Available(update()))),
            Step::Offer(update())
        );
        assert_eq!(
            policy.offer_answered(update(), true),
            Step::Install(update())
        );
        assert_eq!(policy.installed(Ok(())), Step::Relaunch);
    }

    #[test]
    fn progress_and_restart_copy_are_truthful() {
        let status = Status::Downloading {
            received: 75,
            total: 100,
        };
        let automatic = status.view("1.3.0", true, false);
        assert_eq!(automatic.progress, Some(Some(75)));
        assert!(automatic.body.contains("when you’re away"));
        assert!(
            status
                .view("1.3.0", false, true)
                .body
                .contains("when the update is ready")
        );
        assert!(
            !status
                .view("1.3.0", false, false)
                .body
                .contains("when you’re away")
        );
        assert_eq!(percentage(u64::MAX, 10), 100);
        assert_eq!(
            Status::Downloading {
                received: 0,
                total: 0
            }
            .view("1", false, false)
            .progress,
            Some(None)
        );
    }

    #[test]
    fn verification_failure_does_not_blame_network_or_offer_bypass() {
        let view = Status::Failed {
            stage: Some(Stage::Verify),
            detail: "bad signature".into(),
        }
        .view("1.3.0", false, false);
        assert!(view.body.contains("wasn’t installed"));
        assert_eq!(view.primary, Some(("View releases", Command::Releases)));
    }
}
