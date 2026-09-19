//! Accessibility and Screen Recording permission checks and System Settings deep links.

use objc2_app_kit::NSWorkspace;
use objc2_application_services::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::{NSString, NSURL};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionStatus {
    pub accessibility: bool,
    pub screen_recording: bool,
}

#[must_use]
pub fn status() -> PermissionStatus {
    PermissionStatus {
        accessibility: accessibility_trusted(false),
        screen_recording: screen_recording_granted(),
    }
}

#[must_use]
pub fn accessibility_trusted(prompt: bool) -> bool {
    if !prompt {
        return unsafe {
            // SAFETY: the query has no preconditions.
            AXIsProcessTrusted()
        };
    }
    let key = CFString::from_static_str("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_slices(&[&*key], &[CFBoolean::new(true)]);
    unsafe {
        // SAFETY: `options` is a live dictionary for the synchronous call.
        AXIsProcessTrustedWithOptions(Some(options.as_opaque()))
    }
}

#[must_use]
pub fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Shows the system prompt once; later calls only report the current state.
pub fn request_screen_recording() -> bool {
    CGRequestScreenCaptureAccess()
}

pub fn open_accessibility_settings() {
    open_settings_pane("Privacy_Accessibility");
}

pub fn open_screen_recording_settings() {
    open_settings_pane("Privacy_ScreenCapture");
}

fn open_settings_pane(pane: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) else {
        eprintln!("Could not build the System Settings link for {pane}");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        eprintln!("Could not open System Settings for {pane}");
    }
}
