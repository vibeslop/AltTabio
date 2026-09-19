//! Login item registration through the modern `ServiceManagement` API.
//!
//! `SMAppService` needs a real .app bundle; a bare cargo-built executable reports an error, which
//! the settings window surfaces instead of silently doing nothing.

use objc2_service_management::{SMAppService, SMAppServiceStatus};

#[must_use]
pub fn is_enabled() -> bool {
    let status = unsafe {
        // SAFETY: the main app service is a process-wide singleton with no preconditions.
        SMAppService::mainAppService().status()
    };
    status == SMAppServiceStatus::Enabled
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let service = unsafe {
        // SAFETY: the main app service is a process-wide singleton with no preconditions.
        SMAppService::mainAppService()
    };
    let result = unsafe {
        // SAFETY: registration is a synchronous call on the live service object.
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|error| {
        let action = if enabled { "enable" } else { "disable" };
        format!(
            "Could not {action} launch at login: {}",
            error.localizedDescription()
        )
    })
}
