//! Refuses to run a second copy of the bundled app.

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSBundle;

#[must_use]
pub fn another_instance_running() -> bool {
    let Some(identifier) = NSBundle::mainBundle().bundleIdentifier() else {
        // Unbundled development binaries have no identifier to compare.
        return false;
    };
    let current = std::process::id();
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&identifier)
        .iter()
        .any(|application| u32::try_from(application.processIdentifier()) != Ok(current))
}
