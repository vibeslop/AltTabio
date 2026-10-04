//! Which release a macOS copy updates to, and what each step of an update leads to.
//!
//! The macOS adapter does the network and file work; the decisions live here so they are tested
//! on both platforms.

use std::fmt;

/// A release version as the release workflow tags it: `v` and three numbers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// Reads `1.2.3` or the tag `v1.2.3`. Anything else, such as a prerelease suffix, is not a
    /// release version.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let numbers = text
            .strip_prefix('v')
            .unwrap_or(text)
            .split('.')
            // The integer parser takes a leading `+`, which no tag has.
            .map(|part| {
                (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| part.parse().ok())
                    .flatten()
            })
            .collect::<Option<Vec<u32>>>()?;
        match numbers[..] {
            [major, minor, patch] => Some(Self {
                major,
                minor,
                patch,
            }),
            _ => None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One entry of the GitHub release list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A newer release and the address of its macOS archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Update {
    pub version: Version,
    pub url: String,
}

/// The newest published release newer than `current` that has a macOS archive. Windows and
/// macOS share releases, and the early ones are Windows-only, so the newest release is not
/// always one a Mac can install.
#[must_use]
pub fn newest_update(current: Version, releases: &[Release]) -> Option<Update> {
    releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| {
            let version = Version::parse(&release.tag)?;
            let name = format!("AltTabio-{version}-macos.zip");
            let asset = release.assets.iter().find(|asset| asset.name == name)?;
            Some(Update {
                version,
                url: asset.url.clone(),
            })
        })
        .filter(|update| update.version > current)
        .max_by_key(|update| update.version)
}

/// What a look at the release list found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Checked {
    UpToDate,
    Available(Update),
    /// A newer release exists, but this copy cannot replace itself, for the reason given.
    Blocked(Version, String),
}

/// What the user hears about a check they asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Report {
    UpToDate,
    Blocked(Version, String),
    Failed(String),
}

/// What the adapter does next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Step {
    None,
    /// Look at the release list.
    Check,
    /// Ask the user whether to install this release.
    Offer(Update),
    /// Download, verify, and put the release in place of this copy.
    Install(Update),
    Report(Report),
    /// Start the installed copy in place of the running one.
    Relaunch,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Installing(Version),
    /// The new copy is on disk; this process still runs the old one.
    Installed(Version),
}

/// Checks, installs, and the relaunch that puts an installed update to use.
///
/// The user may ask for a check while an automatic one runs, and the other way round, so one
/// value tracks both. Whatever the user asked for reports its outcome and relaunches right
/// away; an automatic update stays silent and relaunches only once the user is away.
#[derive(Debug, Default)]
pub struct Updater {
    phase: Phase,
    asked: bool,
}

impl Updater {
    /// The daily check is due. It is skipped while anything else is under way.
    pub fn check_due(&mut self) -> Step {
        if self.phase != Phase::Idle {
            return Step::None;
        }
        self.phase = Phase::Checking;
        self.asked = false;
        Step::Check
    }

    /// The user chose Check for Updates, or Relaunch to Update once one is installed.
    pub fn requested(&mut self) -> Step {
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Checking;
                self.asked = true;
                Step::Check
            }
            // The work under way reports to the user when it ends.
            Phase::Checking | Phase::Installing(_) => {
                self.asked = true;
                Step::None
            }
            Phase::Installed(_) => Step::Relaunch,
        }
    }

    pub fn checked(&mut self, result: Result<Checked, String>) -> Step {
        if self.phase != Phase::Checking {
            return Step::None;
        }
        self.phase = Phase::Idle;
        if !self.asked {
            return match result {
                Ok(Checked::Available(update)) => {
                    self.phase = Phase::Installing(update.version);
                    Step::Install(update)
                }
                Ok(Checked::UpToDate | Checked::Blocked(..)) | Err(_) => Step::None,
            };
        }
        match result {
            Ok(Checked::UpToDate) => Step::Report(Report::UpToDate),
            Ok(Checked::Available(update)) => Step::Offer(update),
            Ok(Checked::Blocked(version, reason)) => Step::Report(Report::Blocked(version, reason)),
            Err(error) => Step::Report(Report::Failed(error)),
        }
    }

    /// The user answered the offer of `update`.
    pub fn offer_answered(&mut self, update: Update, install: bool) -> Step {
        if !install || self.phase != Phase::Idle {
            return Step::None;
        }
        self.phase = Phase::Installing(update.version);
        self.asked = true;
        Step::Install(update)
    }

    pub fn installed(&mut self, result: Result<(), String>) -> Step {
        let Phase::Installing(version) = self.phase else {
            return Step::None;
        };
        match result {
            Ok(()) => {
                self.phase = Phase::Installed(version);
                if self.asked {
                    Step::Relaunch
                } else {
                    Step::None
                }
            }
            Err(error) => {
                self.phase = Phase::Idle;
                if self.asked {
                    Step::Report(Report::Failed(error))
                } else {
                    Step::None
                }
            }
        }
    }

    /// The user has left the keyboard and mouse alone long enough that a relaunch goes
    /// unnoticed.
    #[must_use]
    pub fn user_away(&self) -> Step {
        if matches!(self.phase, Phase::Installed(_)) {
            Step::Relaunch
        } else {
            Step::None
        }
    }

    /// The version waiting for a relaunch.
    #[must_use]
    pub fn installed_version(&self) -> Option<Version> {
        match self.phase {
            Phase::Installed(version) => Some(version),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
    }

    fn release(tag: &str, assets: &[&str]) -> Release {
        Release {
            tag: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    url: format!("https://example.com/{name}"),
                })
                .collect(),
        }
    }

    fn update(text: &str) -> Update {
        Update {
            version: version(text),
            url: format!("https://example.com/AltTabio-{text}-macos.zip"),
        }
    }

    #[test]
    fn versions_parse_from_tags_and_compare_by_number() {
        assert_eq!(version("v1.2.3"), version("1.2.3"));
        assert_eq!(version("v1.10.0").to_string(), "1.10.0");
        assert!(version("1.10.0") > version("1.9.9"));
        assert!(version("2.0.0") > version("1.99.99"));
        for text in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-beta",
            "1.+2.3",
            "1..3",
        ] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
    }

    #[test]
    fn the_newest_release_with_a_macos_archive_is_the_update() {
        let releases = [
            release("v1.3.0", &["AltTabio-1.3.0-windows-x64.zip"]),
            release(
                "v1.2.0",
                &["AltTabio-1.2.0-windows-x64.zip", "AltTabio-1.2.0-macos.zip"],
            ),
            release("v1.1.6", &["AltTabio-1.1.6-macos.zip"]),
        ];

        assert_eq!(
            newest_update(version("1.1.5"), &releases),
            Some(update("1.2.0"))
        );
        assert_eq!(newest_update(version("1.2.0"), &releases), None);
        assert_eq!(newest_update(version("1.4.0"), &releases), None);
    }

    #[test]
    fn the_list_order_does_not_decide_the_update() {
        let releases = [
            release("v1.1.6", &["AltTabio-1.1.6-macos.zip"]),
            release("v1.2.0", &["AltTabio-1.2.0-macos.zip"]),
        ];

        assert_eq!(
            newest_update(version("1.1.5"), &releases),
            Some(update("1.2.0"))
        );
    }

    #[test]
    fn drafts_prereleases_and_odd_tags_are_never_updates() {
        let mut draft = release("v1.3.0", &["AltTabio-1.3.0-macos.zip"]);
        draft.draft = true;
        let mut prerelease = release("v1.2.0", &["AltTabio-1.2.0-macos.zip"]);
        prerelease.prerelease = true;
        let releases = [
            draft,
            prerelease,
            release("v1.1.7-beta", &["AltTabio-1.1.7-beta-macos.zip"]),
            release("v1.1.6", &["AltTabio-1.1.5-macos.zip"]),
        ];

        assert_eq!(newest_update(version("1.1.5"), &releases), None);
    }

    #[test]
    fn an_automatic_check_installs_quietly_and_waits_for_the_user_to_leave() {
        let mut updater = Updater::default();

        assert_eq!(updater.check_due(), Step::Check);
        assert_eq!(
            updater.checked(Ok(Checked::Available(update("1.2.0")))),
            Step::Install(update("1.2.0"))
        );
        assert_eq!(updater.installed_version(), None);
        assert_eq!(updater.installed(Ok(())), Step::None);
        assert_eq!(updater.installed_version(), Some(version("1.2.0")));
        assert_eq!(updater.check_due(), Step::None);
        assert_eq!(updater.user_away(), Step::Relaunch);
    }

    #[test]
    fn an_automatic_check_keeps_its_outcomes_to_itself() {
        let mut updater = Updater::default();

        for result in [
            Ok(Checked::UpToDate),
            Ok(Checked::Blocked(version("1.2.0"), "read-only".to_owned())),
            Err("offline".to_owned()),
        ] {
            assert_eq!(updater.check_due(), Step::Check);
            assert_eq!(updater.checked(result), Step::None);
        }
        assert_eq!(updater.check_due(), Step::Check);
        assert_eq!(
            updater.checked(Ok(Checked::Available(update("1.2.0")))),
            Step::Install(update("1.2.0"))
        );
        assert_eq!(updater.installed(Err("disk full".to_owned())), Step::None);
        assert_eq!(updater.user_away(), Step::None);
        assert_eq!(updater.check_due(), Step::Check);
    }

    #[test]
    fn a_requested_check_reports_what_it_finds() {
        let mut updater = Updater::default();

        for (result, step) in [
            (Ok(Checked::UpToDate), Step::Report(Report::UpToDate)),
            (
                Ok(Checked::Blocked(version("1.2.0"), "read-only".to_owned())),
                Step::Report(Report::Blocked(version("1.2.0"), "read-only".to_owned())),
            ),
            (
                Err("offline".to_owned()),
                Step::Report(Report::Failed("offline".to_owned())),
            ),
            (
                Ok(Checked::Available(update("1.2.0"))),
                Step::Offer(update("1.2.0")),
            ),
        ] {
            assert_eq!(updater.requested(), Step::Check);
            assert_eq!(updater.checked(result), step);
        }
    }

    #[test]
    fn an_accepted_offer_installs_and_relaunches_at_once() {
        let mut updater = Updater::default();
        updater.requested();
        updater.checked(Ok(Checked::Available(update("1.2.0"))));

        assert_eq!(
            updater.offer_answered(update("1.2.0"), true),
            Step::Install(update("1.2.0"))
        );
        assert_eq!(updater.installed(Ok(())), Step::Relaunch);
    }

    #[test]
    fn a_declined_offer_leaves_the_update_to_the_next_check() {
        let mut updater = Updater::default();
        updater.requested();
        updater.checked(Ok(Checked::Available(update("1.2.0"))));

        assert_eq!(updater.offer_answered(update("1.2.0"), false), Step::None);
        assert_eq!(updater.check_due(), Step::Check);
    }

    #[test]
    fn a_failed_install_the_user_asked_for_is_reported() {
        let mut updater = Updater::default();
        updater.requested();
        updater.checked(Ok(Checked::Available(update("1.2.0"))));
        updater.offer_answered(update("1.2.0"), true);

        assert_eq!(
            updater.installed(Err("disk full".to_owned())),
            Step::Report(Report::Failed("disk full".to_owned()))
        );
        assert_eq!(updater.requested(), Step::Check);
    }

    #[test]
    fn asking_during_an_automatic_check_takes_over_its_outcome() {
        let mut updater = Updater::default();
        updater.check_due();

        assert_eq!(updater.requested(), Step::None);
        assert_eq!(
            updater.checked(Ok(Checked::Available(update("1.2.0")))),
            Step::Offer(update("1.2.0"))
        );
    }

    #[test]
    fn asking_during_an_automatic_install_relaunches_when_it_is_done() {
        let mut updater = Updater::default();
        updater.check_due();
        updater.checked(Ok(Checked::Available(update("1.2.0"))));

        assert_eq!(updater.requested(), Step::None);
        assert_eq!(updater.installed(Ok(())), Step::Relaunch);
    }

    #[test]
    fn asking_once_an_update_is_installed_relaunches() {
        let mut updater = Updater::default();
        updater.check_due();
        updater.checked(Ok(Checked::Available(update("1.2.0"))));
        updater.installed(Ok(()));

        assert_eq!(updater.requested(), Step::Relaunch);
    }

    #[test]
    fn late_answers_change_nothing() {
        let mut updater = Updater::default();

        assert_eq!(updater.checked(Ok(Checked::UpToDate)), Step::None);
        assert_eq!(updater.installed(Ok(())), Step::None);
        assert_eq!(updater.user_away(), Step::None);
        updater.check_due();
        assert_eq!(updater.offer_answered(update("1.2.0"), true), Step::None);
    }
}
