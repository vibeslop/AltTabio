//! The alerts: a fatal start error, About, and the update offer and report.

use alttabio::localization::{message, tr};
use alttabio::update::{Report, Version};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

const GITHUB_URL: &str = "https://github.com/vibeslop/AltTabio";

pub(super) fn show_fatal_error(mtm: MainThreadMarker, message: &str) {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(tr(
        "AltTabio could not complete this action.",
    )));
    alert.setInformativeText(&NSString::from_str(message));
    let _ok = alert.addButtonWithTitle(&NSString::from_str(tr("OK")));
    let _response = alert.runModal();
}

pub(super) fn show_about(mtm: MainThreadMarker) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("AltTabio"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "{}\n{}\nCopyright (c) 2026 VibeSlop",
        message(
            "Version {version}",
            &[("version", env!("CARGO_PKG_VERSION"))]
        ),
        tr("Open-source window switcher for macOS and Windows.")
    )));
    let _ok = alert.addButtonWithTitle(&NSString::from_str(tr("OK")));
    let _github = alert.addButtonWithTitle(&NSString::from_str("GitHub"));
    bring_alert_forward(mtm);
    // The second button returns NSAlertSecondButtonReturn (1001).
    if alert.runModal() == 1001
        && let Some(url) = NSURL::URLWithString(&NSString::from_str(GITHUB_URL))
        && !NSWorkspace::sharedWorkspace().openURL(&url)
    {
        eprintln!("Could not open {GITHUB_URL}");
    }
}

/// Asks whether to install `version` now.
pub(super) fn offer_update(mtm: MainThreadMarker, version: Version) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&message(
        "AltTabio {version} is available",
        &[("version", &version.to_string())],
    )));
    alert.setInformativeText(&NSString::from_str(&message(
        "You have {version}. AltTabio quits and opens again to finish, and keeps its permissions.",
        &[("version", env!("CARGO_PKG_VERSION"))],
    )));
    let _install = alert.addButtonWithTitle(&NSString::from_str(tr("Install Update")));
    let _later = alert.addButtonWithTitle(&NSString::from_str(tr("Not Now")));
    bring_alert_forward(mtm);
    // The first button returns NSAlertFirstButtonReturn (1000).
    alert.runModal() == 1000
}

pub(super) fn show_update_report(mtm: MainThreadMarker, report: &Report) {
    let (heading, information) = match report {
        Report::UpToDate => (
            tr("AltTabio is up to date").to_owned(),
            message(
                "Version {version} is the newest.",
                &[("version", env!("CARGO_PKG_VERSION"))],
            ),
        ),
        Report::Blocked(version, reason) => (
            message(
                "AltTabio {version} is available",
                &[("version", &version.to_string())],
            ),
            tr(reason).to_owned(),
        ),
        Report::Failed(error) => (tr("AltTabio could not update").to_owned(), error.clone()),
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&heading));
    alert.setInformativeText(&NSString::from_str(&information));
    let _ok = alert.addButtonWithTitle(&NSString::from_str(tr("OK")));
    bring_alert_forward(mtm);
    let _response = alert.runModal();
}

fn bring_alert_forward(mtm: MainThreadMarker) {
    #[allow(
        deprecated,
        reason = "an accessory app has no other way to bring its alert forward"
    )]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}
